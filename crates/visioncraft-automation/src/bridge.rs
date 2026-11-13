//! The MCP bridge (`visioncraft-cli mcp --bridge`): an MCP server on stdin and stdout without a
//! session of its own. Every message is passed unchanged to a running window's control channel
//! (`mcp.message {line}`), answered there by the same MCP code on the project the window
//! shows, and the reply is passed back. One short connection per message: challenge, auth,
//! the message, the answer.
//!
//! Ported from 3dcraft `crates/automation/src/bridge.rs` (MIT). When the window cannot be
//! reached, `initialize`, `ping` and `tools/list` are answered here (so a client that starts
//! the bridge first keeps the server) and every other request gets a JSON-RPC error (-32000)
//! that says how to start the window; the bridge keeps serving.

use crate::lines::{Line, read_line};
use crate::rpc::serialize;
use crate::{MAX_REPLY_BYTES, MAX_REQUEST_BYTES};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

const BRIDGE_ERROR: i64 = -32000;
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;

/// Which window the bridge talks to.
#[derive(Clone, Debug)]
pub enum Window {
    /// `--bridge PORT --control-token-file FILE`.
    Fixed { address: SocketAddr, token: String },
    /// Found in a state folder ([`crate::discovery`]), on `port` only when given.
    Discover { folder: PathBuf, port: Option<u16> },
}

#[derive(Clone, Debug)]
pub struct BridgeConfig {
    pub window: Window,
    /// How the bridge names itself in `auth`.
    pub client: Option<String>,
    pub connect_timeout: Duration,
    /// How long the window may take to answer the challenge.
    pub challenge_timeout: Duration,
    /// How long one message may take in the window (a render with many plants).
    pub reply_timeout: Duration,
}

impl BridgeConfig {
    pub fn new(address: SocketAddr, token: String) -> Self {
        Self::with(Window::Fixed { address, token })
    }

    pub fn discover(folder: PathBuf, port: Option<u16>) -> Self {
        Self::with(Window::Discover { folder, port })
    }

    fn with(window: Window) -> Self {
        Self {
            window,
            client: Some(format!("MCP bridge (process {})", std::process::id())),
            connect_timeout: Duration::from_secs(3),
            challenge_timeout: Duration::from_secs(5),
            reply_timeout: Duration::from_secs(330),
        }
    }
}

#[derive(Clone, Debug)]
struct Target {
    address: SocketAddr,
    token: String,
    pid: Option<u32>,
}

/// The bridge while it runs: its configuration and the window it chose.
pub struct Bridge {
    config: BridgeConfig,
    chosen: Option<Target>,
    reached: bool,
}

enum Failure {
    Unreachable(String),
    Other(String),
}

impl Bridge {
    pub fn new(config: BridgeConfig) -> Self {
        Self {
            config,
            chosen: None,
            reached: false,
        }
    }

    fn target(&mut self) -> Result<Target, String> {
        match &self.config.window {
            Window::Fixed { address, token } => Ok(Target {
                address: *address,
                token: token.clone(),
                pid: None,
            }),
            Window::Discover { folder, port } => {
                if let Some(chosen) = &self.chosen {
                    return Ok(chosen.clone());
                }
                let live = crate::discovery::live_entries(folder, None);
                let entry = crate::discovery::choose(&live, *port)?;
                let token = crate::discovery::read_token(&entry.token_file)?;
                let target = Target {
                    address: SocketAddr::from((Ipv4Addr::LOCALHOST, entry.port)),
                    token,
                    pid: Some(entry.pid),
                };
                self.chosen = Some(target.clone());
                Ok(target)
            }
        }
    }

    fn exchange(&mut self, line: &str) -> Result<Option<String>, Failure> {
        let target = self.target().map_err(Failure::Unreachable)?;
        let result = match exchange(&self.config, &target, line) {
            // The window chosen went away: look once more.
            Err(Failure::Unreachable(why)) => {
                if !matches!(self.config.window, Window::Discover { .. })
                    || self.chosen.take().is_none()
                {
                    return Err(Failure::Unreachable(why));
                }
                let again = self.target().map_err(Failure::Unreachable)?;
                exchange(&self.config, &again, line)
            }
            other => other,
        };
        if result.is_ok() {
            self.reached = true;
        }
        result
    }

    /// Tells the window this bridge is done. Best effort.
    pub fn bye(&mut self) {
        if !self.reached {
            return;
        }
        if let Ok(target) = self.target() {
            let _ = call(
                &self.config,
                &target,
                &json!({"id": 1, "method": "bye"}),
                Duration::from_secs(2),
            );
        }
    }
}

fn rpc_error(id: &Value, code: i64, message: &str) -> String {
    serialize(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
}

fn send(stream: &mut TcpStream, request: &Value) -> io::Result<()> {
    let mut line = serialize(request);
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()
}

/// One reply of the control channel as `Ok(result)` or `Err(error)`; the outer error is a lost
/// connection or a reply that is not the channel's.
fn receive(reader: &mut BufReader<TcpStream>) -> Result<Result<Value, String>, (bool, String)> {
    let bytes = match read_line(reader, MAX_REPLY_BYTES + 4096) {
        Ok(Line::Bytes(b)) => b,
        Ok(Line::End) => {
            return Err((
                true,
                "the VisionCraft window closed the connection without answering".into(),
            ));
        }
        Ok(Line::TooLong) => {
            return Err((false, "the VisionCraft window's answer is too large".into()));
        }
        Err(e) => return Err((true, format!("no answer from the VisionCraft window: {e}"))),
    };
    let reply: Value = serde_json::from_slice(&bytes).map_err(|_| {
        (
            false,
            "the VisionCraft window's answer is not JSON".to_string(),
        )
    })?;
    match reply.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(Ok(reply.get("result").cloned().unwrap_or(Value::Null))),
        Some(false) => Ok(Err(reply["error"]
            .as_str()
            .unwrap_or("it gave no reason")
            .to_string())),
        None => Err((
            false,
            "the VisionCraft window's answer has no `ok` field".into(),
        )),
    }
}

fn not_reachable(at: SocketAddr, fixed: bool, why: &dyn std::fmt::Display) -> String {
    if fixed {
        format!(
            "the VisionCraft window is not reachable at {at} ({why}); start it with its control port {} and the same token file, then try again",
            at.port()
        )
    } else {
        format!(
            "the VisionCraft window is not reachable at {at} ({why}); open VisionCraft and allow AI agents, then try again"
        )
    }
}

/// Connects, proves both ways (`proof.rs`), authenticates, sends `request`, returns the answer.
fn call(
    config: &BridgeConfig,
    target: &Target,
    request: &Value,
    timeout: Duration,
) -> Result<Result<Value, String>, Failure> {
    let at = target.address;
    let fixed = matches!(config.window, Window::Fixed { .. });
    let mut stream = TcpStream::connect_timeout(&at, config.connect_timeout)
        .map_err(|e| Failure::Unreachable(not_reachable(at, fixed, &e)))?;
    let setup = |e: io::Error| {
        Failure::Other(format!(
            "could not set up the connection to the VisionCraft window: {e}"
        ))
    };
    stream
        .set_read_timeout(Some(config.challenge_timeout.min(timeout)))
        .map_err(setup)?;
    stream.set_write_timeout(Some(timeout)).map_err(setup)?;
    let _ = stream.set_nodelay(true);
    let mut reader = BufReader::new(stream.try_clone().map_err(setup)?);
    let ours = crate::proof::nonce().map_err(Failure::Other)?;
    // Before the window proved itself, a connection that goes away means no window is there.
    let lost = |why: String| Failure::Unreachable(not_reachable(at, fixed, &why));
    send(
        &mut stream,
        &json!({"id": 0, "method": "challenge", "params": {"nonce": ours}}),
    )
    .map_err(|e| lost(e.to_string()))?;
    let not_the_window = |why: &str| {
        Failure::Other(format!(
            "the program at {at} is not the VisionCraft window that holds this token ({why}); nothing was sent to it"
        ))
    };
    let answer = match receive(&mut reader) {
        Ok(Ok(answer)) => answer,
        Ok(Err(reason)) if reason.starts_with("too many connections") => {
            return Err(Failure::Other(format!(
                "the VisionCraft window at {at} refused the connection: {reason}"
            )));
        }
        Ok(Err(reason)) => return Err(not_the_window(&reason)),
        Err((true, why)) => return Err(lost(why)),
        Err((false, why)) => return Err(not_the_window(&why)),
    };
    stream.set_read_timeout(Some(timeout)).map_err(setup)?;
    let theirs = answer["nonce"]
        .as_str()
        .filter(|n| crate::proof::is_nonce(n))
        .ok_or_else(|| not_the_window("it gave no nonce"))?
        .to_ascii_lowercase();
    let pid = answer["pid"]
        .as_u64()
        .and_then(|p| u32::try_from(p).ok())
        .ok_or_else(|| not_the_window("it gave no process id"))?;
    if target.pid.is_some_and(|wanted| wanted != pid) {
        return Err(not_the_window(
            "another process than the one that announced the window",
        ));
    }
    let expected = crate::proof::proof(
        &target.token,
        crate::proof::WINDOW,
        at.port(),
        pid,
        &ours,
        &theirs,
    );
    if !crate::proof::same(answer["proof"].as_str().unwrap_or_default(), &expected) {
        return Err(not_the_window(
            "its proof does not match; give the bridge the token file the window uses",
        ));
    }
    let mut auth = json!({"proof": crate::proof::proof(&target.token, crate::proof::CLIENT, at.port(), pid, &theirs, &ours)});
    if let Some(client) = &config.client {
        auth["client"] = client.as_str().into();
    }
    let other = |e: String| Failure::Other(e);
    send(
        &mut stream,
        &json!({"id": 1, "method": "auth", "params": auth}),
    )
    .map_err(|e| other(e.to_string()))?;
    if let Err(reason) = receive(&mut reader).map_err(|(_, w)| other(w))? {
        return Err(other(format!(
            "the VisionCraft window at {at} refused this agent ({reason})"
        )));
    }
    send(&mut stream, request).map_err(|e| other(e.to_string()))?;
    receive(&mut reader).map_err(|(_, w)| other(w))
}

fn exchange(config: &BridgeConfig, target: &Target, line: &str) -> Result<Option<String>, Failure> {
    let request = json!({"id": 1, "method": "mcp.message", "params": {"line": line}});
    if serialize(&request).len() >= MAX_REQUEST_BYTES {
        return Err(Failure::Other(format!(
            "the message is too large to pass on to the VisionCraft window (the limit is {MAX_REQUEST_BYTES} bytes)"
        )));
    }
    let result = call(config, target, &request, config.reply_timeout)?.map_err(|reason| {
        Failure::Other(format!(
            "the VisionCraft window did not take the message: {reason}"
        ))
    })?;
    match result.get("reply") {
        Some(Value::String(reply)) => Ok(Some(reply.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(Failure::Other(
            "the VisionCraft window's answer has an unexpected shape".into(),
        )),
    }
}

/// Handles one line from the MCP client; never panics and never fails.
pub fn relay(bridge: &mut Bridge, line: &str) -> Option<String> {
    if line.trim().is_empty() {
        return None;
    }
    match bridge.exchange(line) {
        Ok(reply) => reply,
        Err(Failure::Unreachable(reason)) => {
            offline(line).or_else(|| answer_failure(line, &reason))
        }
        Err(Failure::Other(reason)) => answer_failure(line, &reason),
    }
}

fn answer_failure(line: &str, reason: &str) -> Option<String> {
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(m)) => match m.get("id") {
            Some(id) if !id.is_null() && m.contains_key("method") => {
                Some(rpc_error(id, BRIDGE_ERROR, reason))
            }
            _ => None,
        },
        Ok(_) => Some(rpc_error(&Value::Null, INVALID_REQUEST, reason)),
        Err(_) => Some(rpc_error(&Value::Null, PARSE_ERROR, reason)),
    }
}

/// `initialize`, `ping` and `tools/list` without a window: the same MCP code on an empty host.
fn offline(line: &str) -> Option<String> {
    let message: Value = serde_json::from_str(line).ok()?;
    let method = message.get("method")?.as_str()?;
    if !["initialize", "ping", "tools/list"].contains(&method) {
        return None;
    }
    crate::mcp::handle_line(&mut crate::Headless::new(crate::Access::default()), line)
}

/// Serves MCP messages from `input` until it ends; at the end the window is told `bye`.
pub fn serve(
    config: &BridgeConfig,
    mut input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    let mut bridge = Bridge::new(config.clone());
    loop {
        let reply = match read_line(&mut input, MAX_REQUEST_BYTES)? {
            Line::End => {
                bridge.bye();
                return Ok(());
            }
            Line::TooLong => Some(rpc_error(
                &Value::Null,
                INVALID_REQUEST,
                &format!("the message is longer than {MAX_REQUEST_BYTES} bytes"),
            )),
            Line::Bytes(bytes) => match std::str::from_utf8(&bytes) {
                Ok(line) => relay(&mut bridge, line),
                Err(_) => Some(rpc_error(
                    &Value::Null,
                    PARSE_ERROR,
                    "the message is not UTF-8 text",
                )),
            },
        };
        if let Some(reply) = reply {
            output.write_all(reply.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}
