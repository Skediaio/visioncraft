//! The Model Context Protocol over stdio (`visioncraft-cli mcp`): JSON-RPC 2.0, one message per
//! line.
//!
//! Ported from 3dcraft `crates/automation/src/mcp.rs` (MIT): the same revisions, the same tool
//! names and argument conventions, the same two kinds of error. A request the server cannot act
//! on (unknown method or tool, an unknown or ill-typed argument) is a JSON-RPC error, raised
//! before anything runs; a command that ran and failed is a result with `isError: true`, so the
//! model reads the message and corrects itself.
//!
//! | Tool | Does |
//! |---|---|
//! | `command_list` | the commands, with their parameter docs and enablement |
//! | `command_run` | runs one command |
//! | `command_batch` | runs several, all or nothing |
//! | `doc_inspect` | `document.inspect` |
//! | `doc_render_preview` | a PNG of the project as an image block |
//! | `control_call` | a method of the window (only when the host has some) |

use crate::commands::{self, ListQuery, Stopwatch};
use crate::fields::{Fields, describe, shorten};
use crate::host::Host;
use crate::rpc::{panic_text, serialize};
use crate::{MAX_BATCH_STEPS, MAX_REPLY_BYTES, MAX_REQUEST_BYTES};
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, Write};

/// The MCP revisions this server speaks, latest first (as 3dcraft). `initialize` echoes the
/// client's revision when it is one of these, and offers the first otherwise.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const SERVER_NAME: &str = "visioncraft";

/// Default and largest longest side of a preview, in pixels.
pub const PREVIEW_DEFAULT_SIDE: u64 = 1024;
pub const PREVIEW_MAX_SIDE: u64 = 2048;
const PREVIEW_MIN_SIDE: u64 = 16;
const PREVIEW_MAX_PNG_BYTES: usize = 5 << 20;

/// The tool a server that belongs to a window offers besides [`tool_names`].
pub const WINDOW_TOOL: &str = "control_call";

const INSTRUCTIONS: &str = "VisionCraft visualises houses and gardens: models, drawn shapes, materials, plants, lights and viewpoints, \
seen in real time under the sun of a site and a date. Every action is a command with a stable id and JSON params. \
Start with command_list {\"brief\": true} (then {\"id\": ...} for one command's parameter docs) and doc_inspect. \
Build with command_batch (add atomic: true for all or nothing), check with doc_inspect and look with doc_render_preview. \
Units are metres, Y is up, +X east, -Z north; outlines are [[x, z], ...]. Plants are flora:<species id> (species.list). \
Open and save with command_run file.open / file.save; undo with edit.undo. The resource visioncraft://guide is the agent guide.";

const GUIDE_URI: &str = "visioncraft://guide";
const PROJECT_URI: &str = "visioncraft://project";
const GUIDE: &str = include_str!("../docs/agent.md");

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

struct RpcError {
    code: i64,
    message: String,
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: INVALID_PARAMS,
        message: message.into(),
    }
}

/// Handles one message. Returns the reply line, or `None` for a notification, a response or a
/// blank line. Never panics, whatever the line holds.
pub fn handle_line(host: &mut dyn Host, line: &str) -> Option<String> {
    handle_line_capped(host, line, MAX_REPLY_BYTES)
}

/// Serves messages from `input` until it ends, flushing each reply.
pub fn serve(host: &mut dyn Host, input: impl BufRead, output: impl Write) -> io::Result<()> {
    crate::lines::serve(input, output, |line| match line {
        Ok(line) => handle_line(host, line),
        Err(why) => Some(error_reply(&Value::Null, PARSE_ERROR, &why)),
    })
}

fn handle_line_capped(host: &mut dyn Host, line: &str, max_reply: usize) -> Option<String> {
    if line.trim().is_empty() {
        return None;
    }
    if line.len() > MAX_REQUEST_BYTES {
        return Some(error_reply(
            &Value::Null,
            INVALID_REQUEST,
            &crate::lines::too_long_message(),
        ));
    }
    let message: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_reply(
                &Value::Null,
                PARSE_ERROR,
                &format!("the message is not valid JSON: {e}"),
            ));
        }
    };
    let Value::Object(message) = message else {
        let why = if message.is_array() {
            "JSON-RPC batches are not supported; send one message per line".to_string()
        } else {
            format!(
                "a message must be a JSON-RPC 2.0 object, got {}",
                describe(&message)
            )
        };
        return Some(error_reply(&Value::Null, INVALID_REQUEST, &why));
    };
    let id = match message.get("id") {
        None => None,
        Some(id @ (Value::String(_) | Value::Number(_))) => Some(id),
        Some(other) => {
            return Some(error_reply(
                &Value::Null,
                INVALID_REQUEST,
                &format!("`id` must be a string or a number, got {}", describe(other)),
            ));
        }
    };
    let reply_id = id.unwrap_or(&Value::Null);
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error_reply(
            reply_id,
            INVALID_REQUEST,
            "`jsonrpc` must be \"2.0\"",
        ));
    }
    let method = match message.get("method") {
        Some(Value::String(m)) => m.as_str(),
        // A response to a request of ours: this server sends none.
        None if id.is_some()
            && (message.contains_key("result") || message.contains_key("error")) =>
        {
            return None;
        }
        Some(other) => {
            return Some(error_reply(
                reply_id,
                INVALID_REQUEST,
                &format!("`method` must be a string, got {}", describe(other)),
            ));
        }
        None => {
            return Some(error_reply(
                reply_id,
                INVALID_REQUEST,
                "the message has no `method`",
            ));
        }
    };
    // Notifications are never answered (JSON-RPC 2.0 §4.1).
    let id = id?;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch(host, method, message.get("params"))
    }))
    .unwrap_or_else(|payload| {
        Err(RpcError {
            code: INTERNAL_ERROR,
            message: format!(
                "{}: internal error (a bug): {}",
                shorten(method),
                panic_text(payload.as_ref())
            ),
        })
    });
    let reply = match outcome {
        Ok(result) => serialize(&json!({ "jsonrpc": "2.0", "id": id, "result": result })),
        Err(error) => return Some(error_reply(id, error.code, &error.message)),
    };
    if reply.len() > max_reply {
        return Some(error_reply(
            id,
            INTERNAL_ERROR,
            &format!(
                "{}: the reply is {} bytes, over the limit of {max_reply} bytes; ask for less at a time (a filter, fewer batch steps, a smaller max_side)",
                shorten(method),
                reply.len()
            ),
        ));
    }
    Some(reply)
}

fn dispatch(host: &mut dyn Host, method: &str, params: Option<&Value>) -> Result<Value, RpcError> {
    let params = match params {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => Some(map),
        Some(other) => {
            return Err(invalid(format!(
                "{}: `params` must be an object, got {}",
                shorten(method),
                describe(other)
            )));
        }
    };
    match method {
        "initialize" => {
            if let Some(client) = params
                .and_then(|p| p.get("clientInfo"))
                .and_then(|c| c.get("name"))
                .and_then(Value::as_str)
            {
                host.state().client = commands::clean_intent(client);
            }
            initialize(params)
        }
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut list = tools();
            if !host.methods().is_empty() {
                list.push(control_call_tool(host));
            }
            Ok(json!({ "tools": list }))
        }
        "tools/call" => call_tool(host, params),
        "resources/list" => Ok(json!({ "resources": [
            {"uri": PROJECT_URI, "name": "project", "title": "The open project", "description": "document.inspect of the open project, as the tool doc_inspect gives it.", "mimeType": "application/json"},
            {"uri": GUIDE_URI, "name": "guide", "title": "Driving VisionCraft", "description": "The agent guide: conventions, the working loop, worked examples and known traps.", "mimeType": "text/markdown"},
        ]})),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "resources/read" => {
            let uri = params
                .and_then(|p| p.get("uri"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    invalid("resources/read: `uri` must be a string; resources/list lists them")
                })?;
            let (mime, text) = match uri {
                GUIDE_URI => ("text/markdown", GUIDE.to_string()),
                PROJECT_URI => {
                    let v =
                        commands::execute(host, "document.inspect", &Value::Null).map_err(|m| {
                            RpcError {
                                code: INTERNAL_ERROR,
                                message: m,
                            }
                        })?;
                    ("application/json", serialize(&v))
                }
                other => {
                    return Err(invalid(format!(
                        "resources/read: unknown resource `{}`; the resources are: {PROJECT_URI}, {GUIDE_URI}",
                        shorten(other)
                    )));
                }
            };
            Ok(json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] }))
        }
        "prompts/list" => Ok(json!({ "prompts": [{
            "name": "build_garden",
            "title": "Build a garden from a description",
            "description": "Turns a description of a house and garden into a VisionCraft project, step by step, and checks it with pictures.",
            "arguments": [{"name": "description", "description": "The garden in words: sizes, what goes where, plants, lights, the views to keep.", "required": true}],
        }]})),
        "prompts/get" => {
            let name = params
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if name != "build_garden" {
                return Err(invalid(format!(
                    "prompts/get: unknown prompt `{}`; the prompts are: build_garden",
                    shorten(name)
                )));
            }
            let description = params
                .and_then(|p| p.get("arguments"))
                .and_then(|a| a.get("description"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    invalid("prompts/get: build_garden needs the argument `description`")
                })?;
            Ok(json!({
                "description": "Build a garden from a description",
                "messages": [{"role": "user", "content": {"type": "text", "text": build_garden_prompt(description)}}],
            }))
        }
        other => Err(RpcError {
            code: METHOD_NOT_FOUND,
            message: format!(
                "unknown method `{}`; this server implements initialize, ping, tools/list, tools/call, resources/list, resources/templates/list, resources/read, prompts/list and prompts/get",
                shorten(other)
            ),
        }),
    }
}

fn build_garden_prompt(description: &str) -> String {
    format!(
        "Build this garden in VisionCraft:\n\n{description}\n\n\
Work in this order, and check after each stage:\n\
1. Read the resource visioncraft://guide once, then doc_inspect {{\"faces\": false}} to see what is there.\n\
2. Plan in numbers first: metres, Y up, +X east, -Z north. Write down every outline as [[x, z], ...], every height, every plant species (command_run species.list with a category) and every light position.\n\
3. Build in a few command_batch calls (all or nothing): shapes (shape.add_area for lawns, terraces and paths; shape.add_extrusion for houses, walls and hedge blocks), materials (material.assign), planting (planting.add_spline for hedges, planting.add_area for beds, planting.add_single for trees), lights (light.add), viewpoints (viewpoint.add).\n\
4. After each batch: doc_inspect and compare areas, counts and positions with your plan; then doc_render_preview from a viewpoint and look at it.\n\
5. Check the light: time.set to the hours that matter (morning, afternoon, dusk) and look again.\n\
6. Save with command_run file.save {{\"path\": \"...\"}} and say what you built, with the numbers."
    )
}

fn initialize(params: Option<&Map<String, Value>>) -> Result<Value, RpcError> {
    let latest = PROTOCOL_VERSIONS[0];
    let version = match params.and_then(|p| p.get("protocolVersion")) {
        None | Some(Value::Null) => latest,
        Some(Value::String(requested)) => PROTOCOL_VERSIONS
            .iter()
            .copied()
            .find(|v| v == requested)
            .unwrap_or(latest),
        Some(other) => {
            return Err(invalid(format!(
                "initialize: `protocolVersion` must be a string, got {}",
                describe(other)
            )));
        }
    };
    Ok(json!({
        "protocolVersion": version,
        "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    }))
}

/// The names of the tools every server offers, in `tools/list` order.
pub fn tool_names() -> Vec<String> {
    tools()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect()
}

fn tools() -> Vec<Value> {
    let inspect_props = json!({
        "faces": {"type": "boolean", "description": "List the face names of models and shapes. Default true."},
        "measures": {"type": "boolean", "description": "Evaluate generators (plant counts) and measure shapes (areas, lengths). Default true."},
        "placements": {"type": "boolean", "description": "Every plant's asset, position, rotation and scale. Default false (large)."},
        "full": {"type": "boolean", "description": "Materials, generators and species with all their fields. Default false."}
    });
    vec![
        json!({
            "name": "command_list",
            "description": "Lists VisionCraft's commands: id, label, params notation, summary, parameters (name, type, unit, required, default, allowed, doc), result, example, and whether each can run now. Call it with brief: true first (id, params and the first sentence of each summary), then with id for the full entry of a command you use.",
            "inputSchema": {"type": "object", "properties": {
                "brief": {"type": "boolean", "description": "Only id, params, first sentence of the summary and enabled. Default false. Start with this."},
                "id": {"type": "string", "description": "One command by its exact id, e.g. \"shape.add_area\": its full entry."},
                "filter": {"type": "string", "description": "Case-insensitive part of a command id or label, e.g. \"planting\"."},
                "enabled_only": {"type": "boolean", "description": "Only commands that can run now. Default false."}
            }, "additionalProperties": false}
        }),
        json!({
            "name": "command_run",
            "description": "Runs one command by id and returns its JSON result (its wall-clock milliseconds in the tool result's _meta.ms). A failed command returns isError with a message that starts with the command's id and names the parameter. Files: file.new, file.open, file.save; history: edit.undo, edit.redo; queries: query.measure, query.sun, query.raycast, query.ground_height, query.ground_length, catalog.search, species.list; agent.log.",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string", "description": "A command id from command_list, e.g. \"shape.add_area\"."},
                "params": {"type": "object", "description": "The command's parameters, as command_list documents them. Omit for none."},
                "intent": {"type": "string", "maxLength": 300, "description": "Why you run it, in a short sentence of your own (\"a hornbeam hedge along the south boundary\"). Optional but asked for: agent.log keeps it, and a window shows it."}
            }, "required": ["id"], "additionalProperties": false}
        }),
        json!({
            "name": "command_batch",
            "description": "Runs up to 256 commands in order and returns {completed, failed, results: [{ok, result | error, ms}], ms}. failed counts the commands that returned an error; stop_on_error (default true) stops at the first, and the batch is then an error (isError) with the report as its content. Not atomic: steps that ran stay applied, each its own undo step; edit.undo takes back the last applied step, once per step. terse reports {completed, failed, errors: [{step, error, ms}], last: {step, result, ms}, ms}. stop_on_break is accepted for 3DCraft's shape (nothing in a VisionCraft project breaks). atomic: true makes it all or nothing: the first failure takes back every step (rolledBack: true), and a batch that succeeds is one undo step. Ids made by a step (S1, g1, li1, v1) are the prefix and the lowest free number, so later steps can use them.",
            "inputSchema": {"type": "object", "properties": {
                "steps": {"type": "array", "maxItems": MAX_BATCH_STEPS, "items": {"type": "object", "properties": {
                    "id": {"type": "string", "description": "A command id."},
                    "params": {"type": "object", "description": "The command's parameters."},
                    "intent": {"type": "string", "maxLength": 300, "description": "Why this step, if it differs from the batch's intent."}
                }, "required": ["id"], "additionalProperties": false}},
                "stop_on_error": {"type": "boolean", "description": "Stop at the first command that returns an error. Default true."},
                "stop_on_break": {"type": "boolean", "description": "Accepted for 3DCraft's shape; nothing breaks in a VisionCraft project. Default false."},
                "terse": {"type": "boolean", "description": "Report only the failures and the last result. Default false."},
                "atomic": {"type": "boolean", "description": "All or nothing: the first failure takes back every step; success is one undo step. Default false."},
                "intent": {"type": "string", "maxLength": 300, "description": "Why you run these steps, in a short sentence of your own (\"a hornbeam hedge along the south boundary\"). Optional but asked for: agent.log keeps it, and a window shows it."}
            }, "required": ["steps"], "additionalProperties": false}
        }),
        json!({
            "name": "doc_inspect",
            "description": "Returns the open project as JSON (document.inspect): site, time and sun, layers, models with bounding boxes and face names, shapes with areas and face names, materials and assignments, planting generators with evaluated plant counts, species, lights, viewpoints, terrain, whether it is saved, and the history.",
            "inputSchema": {"type": "object", "properties": inspect_props, "additionalProperties": false}
        }),
        json!({
            "name": "doc_render_preview",
            "description": "Renders the project to a PNG image (4:3) with VisionCraft's own renderer, plants grown, and returns it with what was drawn (camera, time, sun). From a viewpoint, a camera, or the whole scene framed. The project does not change; time only changes the picture.",
            "inputSchema": {"type": "object", "properties": {
                "viewpoint": {"type": "string", "description": "A viewpoint's id or name."},
                "camera": {"type": "object", "description": "{position: [x, y, z], target: [x, y, z], fov_y_deg?: 50}"},
                "time": {"type": "object", "description": "{year?, month?, day?, hour?}: local time for this picture only."},
                "quality": {"type": "string", "enum": ["low", "medium", "high"], "description": "Default high."},
                "max_side": {"type": "integer", "minimum": PREVIEW_MIN_SIDE, "maximum": PREVIEW_MAX_SIDE, "description": "Longest side in pixels. Default 1024."}
            }, "additionalProperties": false}
        }),
    ]
}

fn control_call_tool(host: &dyn Host) -> Value {
    let mut description = String::from(
        "Runs one method of the VisionCraft window this server belongs to (what the person sees, not the project). The methods:",
    );
    for m in host.methods() {
        description.push_str(&format!("\n{} {}: {}", m.name, m.params, m.summary));
    }
    json!({
        "name": WINDOW_TOOL,
        "description": description,
        "inputSchema": {"type": "object", "properties": {
            "method": {"type": "string", "description": "The method, e.g. \"ui.get\"."},
            "params": {"type": "object", "description": "The method's parameters."}
        }, "required": ["method"], "additionalProperties": false}
    })
}

enum Failure {
    Invalid(String),
    Failed(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Invalid(message)
    }
}

fn text_block(text: String) -> Value {
    json!({ "type": "text", "text": text })
}

fn call_tool(host: &mut dyn Host, params: Option<&Map<String, Value>>) -> Result<Value, RpcError> {
    let name = match params.and_then(|p| p.get("name")) {
        Some(Value::String(n)) => n.as_str(),
        Some(other) => {
            return Err(invalid(format!(
                "tools/call: `name` must be a string, got {}",
                describe(other)
            )));
        }
        None => return Err(invalid("tools/call: missing `name`")),
    };
    let args = params.and_then(|p| p.get("arguments"));
    let clock = Stopwatch::start();
    let outcome: Result<Vec<Value>, Failure> = match name {
        "command_list" => (|| {
            let a = Fields::new(
                name,
                "argument",
                args,
                &["filter", "id", "enabled_only", "brief"],
            )?;
            let q = ListQuery {
                filter: a.str("filter")?,
                id: a.str("id")?,
                enabled_only: a.bool("enabled_only")?.unwrap_or(false),
                brief: a.bool("brief")?.unwrap_or(false),
            };
            Ok(vec![text_block(serialize(&commands::command_list(
                host.session(),
                &q,
            )))])
        })(),
        "command_run" => (|| {
            let a = Fields::new(name, "argument", args, &["id", "params", "intent"])?;
            let id = a.required_str("id")?;
            let p = a.object("params")?;
            commands::execute_logged(host, id, &p, a.str("intent")?)
                .map(|r| vec![text_block(serialize(&r))])
                .map_err(Failure::Failed)
        })(),
        "command_batch" => (|| {
            let a = Fields::new(
                name,
                "argument",
                args,
                &[
                    "steps",
                    "stop_on_error",
                    "stop_on_break",
                    "terse",
                    "intent",
                    "atomic",
                ],
            )?;
            let options = commands::BatchOptions {
                stop_on_error: a.bool("stop_on_error")?.unwrap_or(true),
                stop_on_break: a.bool("stop_on_break")?.unwrap_or(false),
                terse: a.bool("terse")?.unwrap_or(false),
                atomic: a.bool("atomic")?.unwrap_or(false),
                intent: a.str("intent")?,
            };
            let steps = commands::parse_steps(name, "argument", &a, "id")?;
            let outcome = commands::run_batch(host, &steps, &options);
            let block = text_block(serialize(&outcome.report));
            if outcome.failed {
                Err(Failure::Failed(
                    block["text"].as_str().unwrap_or_default().to_string(),
                ))
            } else {
                Ok(vec![block])
            }
        })(),
        "doc_inspect" => (|| {
            Fields::new(
                name,
                "argument",
                args,
                &["faces", "measures", "placements", "full"],
            )?;
            let p = args.cloned().unwrap_or(Value::Null);
            commands::execute(host, "document.inspect", &p)
                .map(|r| vec![text_block(serialize(&r))])
                .map_err(Failure::Failed)
        })(),
        "doc_render_preview" => preview(host, args),
        WINDOW_TOOL if !host.methods().is_empty() => (|| {
            let a = Fields::new(name, "argument", args, &["method", "params"])?;
            let method = a.required_str("method")?;
            if !host.methods().iter().any(|m| m.name == method) {
                let names: Vec<&str> = host.methods().iter().map(|m| m.name).collect();
                return Err(Failure::Invalid(format!(
                    "control_call: unknown method `{}`; the methods are: {}",
                    shorten(method),
                    names.join(", ")
                )));
            }
            let p = a.object("params")?;
            match host.call(method, &p) {
                Some(Ok(mut r)) => Ok(picture_blocks(&mut r)),
                Some(Err(e)) => Err(Failure::Failed(e)),
                None => Err(Failure::Failed(format!(
                    "control_call: the window does not answer `{}`",
                    shorten(method)
                ))),
            }
        })(),
        other => Err(Failure::Invalid(format!(
            "unknown tool `{}`; the tools are: {}",
            shorten(other),
            tool_names().join(", ")
        ))),
    };
    let (content, is_error) = match outcome {
        Ok(content) => (content, false),
        Err(Failure::Invalid(message)) => return Err(invalid(message)),
        Err(Failure::Failed(message)) => (vec![text_block(message)], true),
    };
    let mut reply = json!({ "content": content, "isError": is_error });
    if name == "command_run" {
        reply["_meta"] = json!({ "ms": clock.ms() });
    }
    Ok(reply)
}

/// A result with `{mime: "image/png", base64}` as an image block and the rest as text.
fn picture_blocks(result: &mut Value) -> Vec<Value> {
    let mut content = Vec::new();
    if let Some(map) = result.as_object_mut()
        && map.get("mime").and_then(Value::as_str) == Some("image/png")
        && let Some(Value::String(data)) = map.remove("base64")
    {
        map.remove("mime");
        content.push(json!({ "type": "image", "data": data, "mimeType": "image/png" }));
    }
    content.push(text_block(serialize(result)));
    content
}

fn preview(host: &mut dyn Host, args: Option<&Value>) -> Result<Vec<Value>, Failure> {
    let name = "doc_render_preview";
    let a = Fields::new(
        name,
        "argument",
        args,
        &["viewpoint", "camera", "time", "quality", "max_side"],
    )?;
    let side = a
        .integer("max_side", PREVIEW_MIN_SIDE, PREVIEW_MAX_SIDE)?
        .unwrap_or(PREVIEW_DEFAULT_SIDE);
    let mut params = Map::new();
    params.insert("width".into(), side.into());
    params.insert("height".into(), (side * 3 / 4).max(PREVIEW_MIN_SIDE).into());
    for key in ["viewpoint", "camera", "time", "quality"] {
        if let Some(v) = a.get(key) {
            params.insert(key.into(), v.clone());
        }
    }
    let mut result =
        commands::execute(host, "view.render", &Value::Object(params)).map_err(Failure::Failed)?;
    let bytes = result["base64"].as_str().map_or(0, |b| b.len() / 4 * 3);
    if bytes > PREVIEW_MAX_PNG_BYTES {
        return Err(Failure::Failed(format!(
            "{name}: the PNG is about {bytes} bytes, over the limit of {PREVIEW_MAX_PNG_BYTES}; use a smaller max_side"
        )));
    }
    Ok(picture_blocks(&mut result))
}

fn error_reply(id: &Value, code: i64, message: &str) -> String {
    serialize(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_latest_protocol_version_is_listed_first() {
        let mut sorted = super::PROTOCOL_VERSIONS.to_vec();
        sorted.sort_unstable();
        sorted.reverse();
        assert_eq!(sorted, super::PROTOCOL_VERSIONS);
    }
}
