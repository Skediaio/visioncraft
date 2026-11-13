//! The JSON-lines protocol (`visioncraft-cli serve`, and the window's control port): one
//! request per line, one reply per line with the same `id`.
//!
//! ```text
//! {"id": 1, "method": "engine.execute", "params": {"command": "time.set", "params": {"hour": 20}}}
//! {"id": 1, "ok": true, "result": null, "ms": 0.1}
//! {"id": 2, "method": "engine.execute", "params": {"command": "time.sett"}}
//! {"id": 2, "ok": false, "error": "unknown command `time.sett`; did you mean `time.set`?", "ms": 0.0}
//! ```
//!
//! Ported from 3dcraft `crates/automation/src/rpc.rs` (MIT): the same envelope, methods,
//! limits and checks. Every request gets exactly one reply; when its id cannot be read the
//! reply has `"id": null`. Blank lines are skipped.

use crate::commands::{self, ListQuery, Stopwatch};
use crate::fields::{Fields, describe, shorten};
use crate::host::Host;
use crate::{MAX_BATCH_STEPS, MAX_REPLY_BYTES, MAX_REQUEST_BYTES};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

/// A method, as `methods` lists it.
pub struct Method {
    pub name: &'static str,
    /// The parameters, in the notation of the command list.
    pub params: &'static str,
    pub summary: &'static str,
}

/// Every method of the protocol, in a fixed order.
pub const METHODS: &[Method] = &[
    Method {
        name: "engine.execute",
        params: r#"{"command": string, "params": object = {}}"#,
        summary: "Runs a command by id and returns its result; the reply has ms, the wall-clock milliseconds it took.",
    },
    Method {
        name: "engine.commands",
        params: r#"{"filter": string, "id": string, "brief": boolean = false}"#,
        summary: "Lists the commands with their parameters (name, type, unit, required, default, allowed values, doc), result and example, and whether each can run now. filter is a case-insensitive part of the id or label; id picks one command; brief gives {id, params, summary, enabled}.",
    },
    Method {
        name: "batch",
        params: r#"{"steps": [{"command": string, "params": object = {}}], "terse": boolean = false}"#,
        summary: "Runs up to 256 commands in order, all or nothing: the first step that fails takes back the steps before it (rolledBack: true). Returns {completed, failed, rolledBack, results: [{ok, result | error, ms}], ms}; terse gives {completed, failed, rolledBack, errors, last, ms}. A batch that changes the project is one undo step.",
    },
    Method {
        name: "methods",
        params: "{}",
        summary: "Lists the methods of this protocol and its limits.",
    },
];

/// Handles one request line. Returns the reply line (without a newline), or `None` for a
/// blank line. Never panics, whatever the line holds.
pub fn handle_line(host: &mut dyn Host, line: &str) -> Option<String> {
    handle_line_capped(host, line, MAX_REPLY_BYTES)
}

/// The reply to a request that failed, as one line.
pub fn error_reply(id: &Value, error: &str) -> String {
    serialize(&json!({ "id": id, "ok": false, "error": error }))
}

/// The reply to a request that succeeded, as one line.
pub fn ok_reply(id: &Value, result: &Value) -> String {
    serialize(&json!({ "id": id, "ok": true, "result": result }))
}

/// Serves requests from `input` until it ends, flushing each reply before the next request is
/// read.
pub fn serve(host: &mut dyn Host, input: impl BufRead, output: impl Write) -> io::Result<()> {
    crate::lines::serve(input, output, |line| match line {
        Ok(line) => handle_line(host, line),
        Err(why) => Some(error_reply(&Value::Null, &why)),
    })
}

pub(crate) fn handle_line_capped(
    host: &mut dyn Host,
    line: &str,
    max_reply: usize,
) -> Option<String> {
    if line.trim().is_empty() {
        return None;
    }
    if line.len() > MAX_REQUEST_BYTES {
        return Some(error_reply(&Value::Null, &crate::lines::too_long_message()));
    }
    let request: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(e) => {
            return Some(error_reply(
                &Value::Null,
                &format!("the request is not valid JSON: {e}"),
            ));
        }
    };
    let Value::Object(request) = request else {
        return Some(error_reply(
            &Value::Null,
            &format!(
                "a request must be a JSON object {{\"id\", \"method\", \"params\"}}, got {}",
                describe(&request)
            ),
        ));
    };
    let id = request.get("id").unwrap_or(&Value::Null);
    if let Some(unknown) = request
        .keys()
        .find(|k| !matches!(k.as_str(), "id" | "method" | "params"))
    {
        return Some(error_reply(
            id,
            &format!(
                "unknown request field `{}`; a request has id, method and params",
                shorten(unknown)
            ),
        ));
    }
    let method = match request.get("method") {
        Some(Value::String(m)) => m.as_str(),
        Some(other) => {
            return Some(error_reply(
                id,
                &format!("`method` must be a string, got {}", describe(other)),
            ));
        }
        None => {
            return Some(error_reply(
                id,
                "the request has no `method`; call `methods` for the list",
            ));
        }
    };
    let params = request.get("params");
    let clock = Stopwatch::start();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch(host, method, params)
    }))
    .unwrap_or_else(|payload| {
        Err(format!(
            "{}: internal error (a bug): {}",
            shorten(method),
            panic_text(payload.as_ref())
        ))
    });
    let timed = method == "engine.execute";
    let reply = match outcome {
        Ok(result) if timed => {
            serialize(&json!({ "id": id, "ok": true, "result": result, "ms": clock.ms() }))
        }
        Ok(result) => ok_reply(id, &result),
        Err(error) if timed => {
            return Some(serialize(
                &json!({ "id": id, "ok": false, "error": error, "ms": clock.ms() }),
            ));
        }
        Err(error) => return Some(error_reply(id, &error)),
    };
    if reply.len() > max_reply {
        return Some(error_reply(
            id,
            &format!(
                "{}: the reply is {} bytes, over the limit of {max_reply} bytes; ask for less at a time (a filter, fewer batch steps, a smaller image, a path)",
                shorten(method),
                reply.len()
            ),
        ));
    }
    Some(reply)
}

pub(crate) fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".into())
}

fn dispatch(host: &mut dyn Host, method: &str, params: Option<&Value>) -> Result<Value, String> {
    match method {
        "engine.execute" => {
            let p = Fields::new(method, "parameter", params, &["command", "params"])?;
            let command = p.required_str("command")?;
            let command_params = p.object("params")?;
            commands::execute(host, command, &command_params)
        }
        "engine.commands" => {
            let p = Fields::new(method, "parameter", params, &["filter", "id", "brief"])?;
            let query = ListQuery {
                filter: p.str("filter")?,
                id: p.str("id")?,
                enabled_only: false,
                brief: p.bool("brief")?.unwrap_or(false),
            };
            Ok(commands::command_list(host.session(), &query))
        }
        "batch" => {
            let p = Fields::new(method, "parameter", params, &["steps", "terse"])?;
            let terse = p.bool("terse")?.unwrap_or(false);
            let steps = commands::parse_steps(method, "parameter", &p, "command")?;
            Ok(commands::run_batch(host, &steps, terse).report)
        }
        "methods" => {
            Fields::new(method, "parameter", params, &[])?;
            let methods: Vec<Value> = METHODS
                .iter()
                .chain(host.methods())
                .map(|m| json!({ "name": m.name, "params": m.params, "summary": m.summary }))
                .collect();
            Ok(json!({
                "methods": methods,
                "limits": {"requestBytes": MAX_REQUEST_BYTES, "replyBytes": MAX_REPLY_BYTES, "batchSteps": MAX_BATCH_STEPS},
            }))
        }
        other => {
            let empty = Value::Null;
            if let Some(outcome) = host.call(other, params.unwrap_or(&empty)) {
                return outcome;
            }
            let names: Vec<&str> = METHODS
                .iter()
                .chain(host.methods())
                .map(|m| m.name)
                .collect();
            Err(format!(
                "unknown method `{}`; the methods are: {}",
                shorten(other),
                names.join(", ")
            ))
        }
    }
}

/// One line of JSON; the fallback keeps a failure to serialise an error reply, not a panic.
pub(crate) fn serialize(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| {
        r#"{"id":null,"ok":false,"error":"the reply could not be written as JSON (a bug)"}"#.into()
    })
}
