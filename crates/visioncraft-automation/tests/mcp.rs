//! An MCP conversation over an in-memory stdio pair: initialize, tools/list, several
//! tools/call, resources, a prompt and the errors.

use serde_json::{Value, json};
use std::io::Cursor;
use visioncraft_automation::{Access, Headless, mcp};

fn converse(host: &mut Headless, messages: &[Value]) -> Vec<Value> {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let mut output = Vec::new();
    mcp::serve(host, Cursor::new(input.into_bytes()), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

fn text(reply: &Value) -> Value {
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn a_whole_conversation() {
    let mut host = Headless::new(Access::default());
    let replies = converse(
        &mut host,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            call(
                3,
                "command_list",
                json!({"brief": true, "filter": "light."}),
            ),
            call(
                4,
                "command_batch",
                json!({"atomic": true, "steps": [
                    {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[-10, -2], [10, -2], [10, 10], [-10, 10]], "material": "lawn"}},
                    {"id": "light.add", "params": {"name": "Lamp", "position": [3, 2.2, -1]}}
                ]}),
            ),
            call(5, "doc_inspect", json!({"faces": false, "measures": true})),
            call(
                6,
                "command_run",
                json!({"id": "shape.remove", "params": {"id": "S9"}}),
            ),
            call(7, "command_run", json!({"id": "edit.undo"})),
            call(8, "command_bach", json!({})),
            call(9, "command_run", json!({"id": "time.set", "parms": {}})),
            call(10, "doc_render_preview", json!({})),
            json!({"jsonrpc": "2.0", "id": 11, "method": "resources/read", "params": {"uri": "visioncraft://guide"}}),
            json!({"jsonrpc": "2.0", "id": 12, "method": "prompts/get", "params": {"name": "build_garden", "arguments": {"description": "a lawn"}}}),
            json!({"jsonrpc": "2.0", "id": 13, "method": "nope"}),
        ],
    );
    // The notification got no reply.
    assert_eq!(replies.len(), 13);
    let init = &replies[0]["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "visioncraft");
    assert_eq!(
        init["capabilities"],
        json!({"tools": {}, "resources": {}, "prompts": {}})
    );
    let tools: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        [
            "command_list",
            "command_run",
            "command_batch",
            "doc_inspect",
            "doc_render_preview"
        ]
    );
    let listed: Vec<String> = text(&replies[2])["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(listed, ["light.add", "light.remove", "light.update"]);
    let batch = &replies[3]["result"];
    assert_eq!(batch["isError"], false);
    assert_eq!(text(&replies[3])["completed"], 2);
    let doc = text(&replies[4]);
    assert_eq!(doc["shapes"][0]["area_m2"], 240.0);
    assert!(doc["shapes"][0].get("faces").is_none());
    // A command that ran and failed is a tool error naming the command.
    assert_eq!(replies[5]["result"]["isError"], true);
    assert_eq!(
        replies[5]["result"]["content"][0]["text"],
        "shape.remove: no shape `S9`"
    );
    // The batch was one undo step.
    assert_eq!(
        text(&replies[6]),
        json!({"can_undo": false, "can_redo": true})
    );
    assert!(replies[6]["result"]["_meta"]["ms"].is_number());
    // Wrong tools and arguments are JSON-RPC errors, before anything runs.
    assert_eq!(replies[7]["error"]["code"], -32602);
    assert!(
        replies[7]["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("unknown tool `command_bach`")
    );
    assert_eq!(
        replies[8]["error"]["message"],
        "command_run: unknown argument `parms`; its arguments are: id, params, intent"
    );
    // No GPU in this host: the preview says so as a tool error.
    assert_eq!(replies[9]["result"]["isError"], true);
    assert!(
        replies[9]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("view.render: this session has no renderer")
    );
    assert!(
        replies[10]["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("# ")
    );
    assert!(
        replies[11]["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("a lawn")
    );
    assert_eq!(replies[12]["error"]["code"], -32601);
}

#[test]
fn malformed_messages_get_errors_and_the_server_keeps_serving() {
    let mut host = Headless::new(Access::default());
    let input = "{bad json\n[1, 2]\n{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"ping\"}\n";
    let mut output = Vec::new();
    mcp::serve(
        &mut host,
        Cursor::new(input.as_bytes().to_vec()),
        &mut output,
    )
    .unwrap();
    let replies: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[0]["id"], Value::Null);
    assert_eq!(replies[1]["error"]["code"], -32600);
    assert_eq!(replies[2], json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
}
