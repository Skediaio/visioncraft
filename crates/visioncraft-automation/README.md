# visioncraft-automation

Scripts and AI agents drive VisionCraft through this crate, the same way they drive 3DCraft:
JSON lines (`visioncraft-cli serve`), MCP (`visioncraft-cli mcp`), the command line
(`visioncraft-cli run`), and a loopback control channel to a running window
(`visioncraft-cli mcp --bridge`).

- [`docs/control-protocol.md`](docs/control-protocol.md): the protocol, the MCP tools, the
  control channel, limits, and every difference from 3DCraft.
- [`docs/agent.md`](docs/agent.md): the guide for an agent (also served as the MCP resource
  `visioncraft://guide`), with a worked example.
- [`docs/examples/`](docs/examples): `garden.json` (a script for `run --script`, used by the tests)
  and `mcp_demo.py` (drives `visioncraft-cli mcp` over stdio).

## Layout

| Module | Does |
|---|---|
| `host` | `Host` (a core `Session`, its `HostState`: open file, file roots, model geometry, catalogue; optional pictures) and `Headless`, the CLI's host |
| `commands` | `execute`: the core's commands plus the host's (`file.*`, `edit.*`, `document.inspect`, `document.measure`, `query.*`, `catalog.search`, `species.list`, `planting.list_presets`, `view.render`); the command list; all-or-nothing batches |
| `docs` | `CommandDoc`, the per-command documentation, and the adapter that derives it from the core's descriptions |
| `rpc`, `mcp` | the two line protocols: `handle_line(host, line)` and `serve(host, input, output)` |
| `control`, `bridge`, `discovery`, `proof` | the loopback control channel (native only): server, MCP bridge client, finding a window, the HMAC challenge |

`wasm32-unknown-unknown`: the crate builds (`control`, `bridge` and `discovery` are left out
there; clocks report `ms: null`).

## Registering the MCP server with Claude Code

`.mcp.json` in a project (not installed anywhere by this crate):

```json
{
  "mcpServers": {
    "visioncraft": {
      "command": "visioncraft-cli",
      "args": ["mcp", "--automation-read-root", ".", "--automation-write-root", "."]
    }
  }
}
```

To work in a running window instead (once the window hosts the control channel):
`"args": ["mcp", "--bridge"]`. The same with the command line:
`claude mcp add visioncraft -- visioncraft-cli mcp --automation-read-root . --automation-write-root .`

## What the desktop window must call to host the bridge

```rust
let control = visioncraft_automation::control::serve_control(ControlOptions::default())?; // 127.0.0.1:7920..7939, token + discovery entry
*control.waker.lock().unwrap() = Some(Box::new(move || ctx.request_repaint()));
// every frame, on the UI thread, with a Host over the studio:
control.poll(&mut studio_host);
// when the window gets the focus:
control.focused();
// dropping `control` stops listening and removes the discovery entry
```

`studio_host` implements `Host` for the window: `parts()` gives `&mut studio.session` and a
`HostState` kept beside the studio; after a request, copy new `HostState::models` geometry into
`studio.loaded` (`Loaded { model, version }`) and call `studio.set_draft(None)`; `render` draws
with the window's own renderer; `methods`/`call` add the window's `ui.*` methods (they become the
MCP tool `control_call`).

## Command documentation wanted from the core

Today `Session::commands()` gives `(name, description)` only; `docs::derive` reads the rest from
the description text. When `visioncraft-core` gives the following, `derive` and its tables go
and `commands::docs` maps the core's specs one to one:

```rust
pub enum ParamType { String, Number, Integer, Boolean, Point3, Point2, Outline, Object, Array, Any }
pub struct ParamSpec {
    pub name: &'static str,
    pub ty: ParamType,
    pub unit: Option<&'static str>,        // "m", "deg", "K", "lm", "h", "1/m2"
    pub required: bool,
    pub default: Option<&'static str>,      // as JSON text: "0.5", "true", "\"point\""
    pub allowed: &'static [&'static str],   // enum values, e.g. ["3dcraft", "obj"]
    pub doc: &'static str,                  // one line
}
pub struct CommandSpec {
    pub name: &'static str,
    pub label: &'static str,                // "Add Area"
    pub summary: &'static str,              // one or two sentences
    pub params: &'static [ParamSpec],
    pub open: bool,                         // takes keys not listed (planting.update)
    pub result: &'static str,               // "{id}" or "null"
    pub example: &'static str,              // params as JSON, run by a core test
}
impl Session { pub fn command_specs(&self) -> Vec<&CommandSpec>; }
// and `Session::register` taking a `CommandSpec` (or `register_with_spec`), and handlers that
// refuse unknown keys themselves (`#[serde(deny_unknown_fields)]`).
```

## Attribution

Ported from and modelled on **3DCraft** v0.1.1 (<https://github.com/johancoppens/3dcraft>, MIT):
the JSON-lines envelope and methods (`crates/automation/src/rpc.rs`), the MCP server
(`crates/automation/src/mcp.rs`), argument checking (`fields.rs`), line reading (`lines.rs`), the
HMAC challenge (`proof.rs`), window discovery (`discovery.rs`), the bridge (`bridge.rs`), and the
CLI's subcommands and conventions (`apps/craft3d-cli`). Ported files say so at the top.
