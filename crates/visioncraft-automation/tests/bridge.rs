//! The bridge client against a fake control server and against the real one, and what it
//! answers when no window is there.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Cursor, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::Duration;
use visioncraft_automation::bridge::{self, BridgeConfig};
use visioncraft_automation::control::{ControlOptions, serve_control};
use visioncraft_automation::proof;
use visioncraft_automation::{Access, Headless};

fn replies(output: Vec<u8>) -> Vec<Value> {
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("bridge-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fake window: proves itself, checks the bridge's proof, answers `mcp.message` with a
/// canned reply that echoes the line. Returns the lines it saw.
fn fake_window(
    listener: TcpListener,
    token: &'static str,
    connections: usize,
) -> std::thread::JoinHandle<Vec<Value>> {
    std::thread::spawn(move || {
        let port = listener.local_addr().unwrap().port();
        let mut seen = Vec::new();
        for stream in listener.incoming().take(connections) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut ours = String::new();
            let mut theirs = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let v: Value = serde_json::from_str(&line).unwrap();
                seen.push(v.clone());
                let reply = match v["method"].as_str().unwrap() {
                    "challenge" => {
                        theirs = v["params"]["nonce"].as_str().unwrap().to_string();
                        ours = "ab".repeat(32);
                        let p = proof::proof(token, proof::WINDOW, port, 4242, &theirs, &ours);
                        json!({"id": v["id"], "ok": true, "result": {"nonce": ours, "pid": 4242, "proof": p}})
                    }
                    "auth" => {
                        let want = proof::proof(token, proof::CLIENT, port, 4242, &ours, &theirs);
                        assert_eq!(v["params"]["proof"], want, "the bridge's proof");
                        assert!(
                            v["params"].get("token").is_none(),
                            "the token never crosses the socket"
                        );
                        json!({"id": v["id"], "ok": true, "result": {"authenticated": true}})
                    }
                    "mcp.message" => {
                        let inner: Value =
                            serde_json::from_str(v["params"]["line"].as_str().unwrap()).unwrap();
                        let answer = json!({"jsonrpc": "2.0", "id": inner["id"], "result": {"echo": inner["method"]}});
                        json!({"id": v["id"], "ok": true, "result": {"reply": answer.to_string()}})
                    }
                    "bye" => json!({"id": v["id"], "ok": true, "result": null}),
                    other => panic!("unexpected {other}"),
                };
                writeln!(stream, "{reply}").unwrap();
            }
        }
        seen
    })
}

#[test]
fn the_bridge_proves_authenticates_and_relays() {
    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    // Two messages and the goodbye: three connections.
    let window = fake_window(listener, TOKEN, 3);
    let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
    let mut output = Vec::new();
    bridge::serve(
        &BridgeConfig::new(address, TOKEN.into()),
        Cursor::new(input),
        &mut output,
    )
    .unwrap();
    let out = replies(output);
    assert_eq!(
        out[0],
        json!({"jsonrpc": "2.0", "id": 1, "result": {"echo": "ping"}})
    );
    assert_eq!(out[1]["result"]["echo"], "tools/list");
    let seen = window.join().unwrap();
    let methods: Vec<&str> = seen.iter().map(|v| v["method"].as_str().unwrap()).collect();
    assert_eq!(
        methods,
        [
            "challenge",
            "auth",
            "mcp.message",
            "challenge",
            "auth",
            "mcp.message",
            "challenge",
            "auth",
            "bye"
        ]
    );
}

#[test]
fn a_window_with_another_token_is_not_believed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let window = fake_window(
        listener,
        "ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00",
        1,
    );
    let token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let mut output = Vec::new();
    bridge::serve(
        &BridgeConfig::new(address, token.into()),
        Cursor::new("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{}}\n"),
        &mut output,
    )
    .unwrap();
    let out = replies(output);
    assert_eq!(out[0]["error"]["code"], -32000);
    assert!(
        out[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("is not the VisionCraft window"),
        "{}",
        out[0]
    );
    // The fake saw the challenge and nothing more.
    let seen = window.join().unwrap();
    assert_eq!(seen.len(), 1);
}

#[test]
fn without_a_window_the_bridge_answers_what_it_can() {
    // A port nobody listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = BridgeConfig::new(SocketAddr::from(([127, 0, 0, 1], port)), "00".repeat(32));
    let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"doc_inspect\"}}\n";
    let mut output = Vec::new();
    bridge::serve(&config, Cursor::new(input), &mut output).unwrap();
    let out = replies(output);
    assert_eq!(out[0]["result"]["serverInfo"]["name"], "visioncraft");
    assert_eq!(out[1]["error"]["code"], -32000);
    assert!(
        out[1]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not reachable"),
        "{}",
        out[1]
    );
}

#[test]
fn the_bridge_drives_a_real_control_server_and_finds_it_by_discovery() {
    let state = tmp("discovery");
    let handle = serve_control(ControlOptions {
        port: Some(0),
        token_file: None,
        state_dir: Some(state.clone()),
    })
    .unwrap();
    let mut host = Headless::new(Access::default());
    let config = BridgeConfig::discover(state.clone(), None);
    let client = std::thread::spawn(move || {
        let input = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "command_run", "arguments": {"id": "light.add", "params": {"name": "Lamp", "position": [0, 2, 0]}}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "doc_inspect", "arguments": {"faces": false}}}),
        ]
        .iter()
        .map(|m| format!("{m}\n"))
        .collect::<String>();
        let mut output = Vec::new();
        bridge::serve(&config, Cursor::new(input), &mut output).unwrap();
        replies(output)
    });
    // The host stays on this thread; the requests come to it.
    while !client.is_finished() {
        handle.serve_one(&mut host, Duration::from_millis(50));
    }
    let out = client.join().unwrap();
    assert_eq!(out[0]["result"]["isError"], false, "{}", out[0]);
    let inspected: Value =
        serde_json::from_str(out[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(inspected["lights"][0]["name"], "Lamp");
    assert_eq!(host.session.project().lights.len(), 1);
    // The token file is private and the entry goes with the handle.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(handle.token_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    drop(handle);
    assert!(
        std::fs::read_dir(state.join("windows"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn the_control_server_refuses_a_wrong_token_and_http() {
    let dir = tmp("refuse");
    let handle = serve_control(ControlOptions {
        port: Some(0),
        token_file: Some(dir.join("t.token")),
        state_dir: Some(dir.clone()),
    })
    .unwrap();
    let talk = |lines: &str| -> String {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", handle.port())).unwrap();
        s.write_all(lines.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = BufReader::new(s).read_line(&mut out);
        out
    };
    assert!(
        talk("{\"id\":1,\"method\":\"auth\",\"params\":{\"token\":\"00\"}}\n")
            .contains("the token or proof is wrong")
    );
    assert_eq!(talk("GET / HTTP/1.1\r\n\r\n"), "");
    assert!(
        talk("{\"id\":1,\"method\":\"methods\"}\n").contains("the first request must be `auth`")
    );
    // A token file of its own: no discovery entry.
    assert!(!dir.join("windows").exists());
}
