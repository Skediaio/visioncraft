//! The server side of the loopback control channel, for a program that shows a project (the
//! desktop window, or `visioncraft-cli serve --control`): the JSON-lines protocol of
//! [`crate::rpc`] on 127.0.0.1, behind a token, plus `mcp.message {line}` for the bridge.
//!
//! [`serve_control`] binds the port and accepts connections on threads of their own, which do
//! the handshake (`challenge` then `auth {proof}`, or `auth {token}`) and pass every request
//! on through a channel; the program answers them on its own thread, where its host lives,
//! with [`ControlHandle::poll`] (once per frame) or [`ControlHandle::serve_one`]. The host
//! never leaves that thread, so it need not be `Send`.
//!
//! The rules are 3dcraft's (`docs/control-protocol.md` › Authentication, MIT): loopback only;
//! the first request of a connection authenticates or the connection closes; an HTTP request
//! line closes it without a reply; 30 s of silence closes it; at most 16 connections.

use crate::discovery::{self, WindowEntry};
use crate::host::Host;
use crate::lines::{Line, read_line};
use crate::rpc::{error_reply, ok_reply, serialize};
use crate::{MAX_REQUEST_BYTES, proof};
use serde_json::{Value, json};
use std::io::{BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Called from a connection thread when a request waits (a window asks for a repaint).
pub type Waker = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

const MAX_CONNECTIONS: usize = 16;
const IDLE: Duration = Duration::from_secs(30);
/// How long a connection waits for the program to answer one request.
const ANSWER: Duration = Duration::from_secs(300);

/// Where to listen and with which token.
#[derive(Clone, Debug, Default)]
pub struct ControlOptions {
    /// `None`: the first free port of 7920 to 7939; `Some(0)`: any free port.
    pub port: Option<u16>,
    /// `None`: `control.token` in the state folder, and a discovery entry is written so that
    /// `visioncraft-cli mcp --bridge` finds this program. A token file of its own writes none.
    pub token_file: Option<PathBuf>,
    /// The state folder; `None`: [`discovery::state_folder`].
    pub state_dir: Option<PathBuf>,
}

struct Request {
    line: String,
    reply: Sender<String>,
}

/// A listening control channel. Dropping it stops listening and removes the discovery entry.
pub struct ControlHandle {
    port: u16,
    token_file: PathBuf,
    entry: Option<(PathBuf, WindowEntry)>,
    requests: Receiver<Request>,
    stop: Arc<AtomicBool>,
    /// Called from a connection thread when a request waits (a window's repaint).
    pub waker: Waker,
}

/// Starts listening on 127.0.0.1.
pub fn serve_control(options: ControlOptions) -> Result<ControlHandle, String> {
    let folder = options.state_dir.clone().or_else(discovery::state_folder);
    let discoverable = options.token_file.is_none();
    let token_file = match (&options.token_file, &folder) {
        (Some(f), _) => f.clone(),
        (None, Some(folder)) => discovery::default_token_file(folder),
        (None, None) => {
            return Err("no state folder to keep the control token in; give a token file".into());
        }
    };
    let token_file = std::path::absolute(&token_file).unwrap_or(token_file);
    let token = discovery::ensure_token(&token_file)?;
    let ports: Vec<u16> = match options.port {
        Some(p) => vec![p],
        None => (discovery::FIRST_PORT..discovery::FIRST_PORT + discovery::PORT_TRIES).collect(),
    };
    let listener = ports
        .iter()
        .find_map(|p| TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, *p))).ok())
        .ok_or_else(|| format!("no free port among {ports:?} on 127.0.0.1"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let entry = match (discoverable, folder) {
        (true, Some(folder)) => {
            let now = discovery::now_ms();
            let e = WindowEntry {
                pid: std::process::id(),
                port,
                token_file: token_file.clone(),
                focused_at: now,
                started_at: now,
            };
            discovery::write_entry(&folder, &e)?;
            Some((folder, e))
        }
        _ => None,
    };
    let (send, requests) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let waker: Waker = Arc::new(Mutex::new(None));
    let open = Arc::new(AtomicUsize::new(0));
    {
        let (stop, waker, token) = (stop.clone(), waker.clone(), token.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let (send, waker, token, open) =
                    (send.clone(), waker.clone(), token.clone(), open.clone());
                std::thread::spawn(move || {
                    let busy = open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS;
                    let _ = connection(stream, port, &token, busy, &send, &waker);
                    open.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
    }
    Ok(ControlHandle {
        port,
        token_file,
        entry,
        requests,
        stop,
        waker,
    })
}

fn write_line(stream: &mut TcpStream, line: &str) -> std::io::Result<()> {
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

/// One connection: the handshake, then every request through the channel.
fn connection(
    mut stream: TcpStream,
    port: u16,
    token: &str,
    busy: bool,
    send: &Sender<Request>,
    waker: &Waker,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IDLE))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut challenged: Option<String> = None;
    let mut authenticated = false;
    loop {
        let bytes = match read_line(&mut reader, MAX_REQUEST_BYTES)? {
            Line::End => return Ok(()),
            Line::TooLong => {
                write_line(
                    &mut stream,
                    &error_reply(&Value::Null, &crate::lines::too_long_message()),
                )?;
                continue;
            }
            Line::Bytes(b) => b,
        };
        let line = String::from_utf8_lossy(&bytes).into_owned();
        if !authenticated {
            let word = line.split(' ').next().unwrap_or("").to_ascii_uppercase();
            if [
                "GET", "POST", "PUT", "OPTIONS", "HEAD", "DELETE", "PATCH", "CONNECT", "TRACE",
            ]
            .contains(&word.as_str())
            {
                return Ok(());
            }
            let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            if busy {
                write_line(
                    &mut stream,
                    &error_reply(&id, "too many connections; close another one first"),
                )?;
                return Ok(());
            }
            match (request["method"].as_str(), challenged.as_deref()) {
                (Some("challenge"), None) => {
                    let Some(theirs) = request["params"]["nonce"]
                        .as_str()
                        .filter(|n| proof::is_nonce(n))
                    else {
                        write_line(
                            &mut stream,
                            &error_reply(
                                &id,
                                "challenge: `nonce` must be 32 to 128 hexadecimal characters",
                            ),
                        )?;
                        return Ok(());
                    };
                    let ours = proof::nonce().map_err(std::io::Error::other)?;
                    let pid = std::process::id();
                    let shown = proof::proof(token, proof::WINDOW, port, pid, theirs, &ours);
                    write_line(
                        &mut stream,
                        &ok_reply(&id, &json!({"nonce": ours, "pid": pid, "proof": shown})),
                    )?;
                    challenged = Some(format!("{}\n{ours}", theirs.to_ascii_lowercase()));
                }
                (Some("auth"), _) => {
                    let params = &request["params"];
                    let ok = match (
                        params["proof"].as_str(),
                        params["token"].as_str(),
                        challenged.as_deref(),
                    ) {
                        (Some(shown), _, Some(nonces)) => {
                            let (theirs, ours) = nonces.split_once('\n').unwrap_or_default();
                            proof::same(
                                shown,
                                &proof::proof(
                                    token,
                                    proof::CLIENT,
                                    port,
                                    std::process::id(),
                                    ours,
                                    theirs,
                                ),
                            )
                        }
                        (None, Some(given), _) => proof::same(given, token),
                        _ => false,
                    };
                    if !ok {
                        write_line(
                            &mut stream,
                            &error_reply(&id, "auth: the token or proof is wrong"),
                        )?;
                        return Ok(());
                    }
                    authenticated = true;
                    write_line(&mut stream, &ok_reply(&id, &json!({"authenticated": true})))?;
                }
                _ => {
                    write_line(
                        &mut stream,
                        &error_reply(
                            &id,
                            "the first request must be `auth` (or `challenge`, then `auth`)",
                        ),
                    )?;
                    return Ok(());
                }
            }
            continue;
        }
        if serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v["method"].as_str().map(str::to_string))
            .as_deref()
            == Some("bye")
        {
            let id = serde_json::from_str::<Value>(&line)
                .map(|v| v["id"].clone())
                .unwrap_or(Value::Null);
            write_line(&mut stream, &ok_reply(&id, &Value::Null))?;
            return Ok(());
        }
        let (reply, answer) = channel();
        if send.send(Request { line, reply }).is_err() {
            return Ok(());
        }
        if let Some(wake) = waker.lock().ok().as_ref().and_then(|w| w.as_ref()) {
            wake();
        }
        let out = answer.recv_timeout(ANSWER).unwrap_or_else(|_| {
            error_reply(&Value::Null, "the program did not answer within 300 s")
        });
        write_line(&mut stream, &out)?;
    }
}

/// The reply to one request line, on the host's own thread.
fn answer(host: &mut dyn Host, line: &str) -> String {
    let request: Value = serde_json::from_str(line).unwrap_or(Value::Null);
    if request["method"].as_str() == Some("mcp.message") {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let Some(message) = request["params"]["line"].as_str() else {
            return error_reply(&id, "mcp.message: `line` must be a string");
        };
        let reply = crate::mcp::handle_line(host, message);
        return ok_reply(&id, &json!({ "reply": reply }));
    }
    crate::rpc::handle_line(host, line).unwrap_or_else(|| serialize(&Value::Null))
}

impl ControlHandle {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token_file(&self) -> &Path {
        &self.token_file
    }

    /// Answers every request that waits; returns how many.
    pub fn poll(&self, host: &mut dyn Host) -> usize {
        let mut n = 0;
        while let Ok(request) = self.requests.try_recv() {
            let _ = request.reply.send(answer(host, &request.line));
            n += 1;
        }
        n
    }

    /// Waits up to `timeout` for one request and answers it; false when none came.
    pub fn serve_one(&self, host: &mut dyn Host, timeout: Duration) -> bool {
        match self.requests.recv_timeout(timeout) {
            Ok(request) => {
                let _ = request.reply.send(answer(host, &request.line));
                true
            }
            Err(_) => false,
        }
    }

    /// The window got the focus: the bridge prefers it over other windows.
    pub fn focused(&mut self) {
        if let Some((folder, entry)) = &mut self.entry {
            entry.focused_at = discovery::now_ms();
            let _ = discovery::write_entry(folder, entry);
        }
    }
}

impl Drop for ControlHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop so that it sees the flag.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)),
            Duration::from_millis(200),
        );
        if let Some((folder, entry)) = &self.entry {
            discovery::remove_entry(folder, entry.pid);
        }
    }
}
