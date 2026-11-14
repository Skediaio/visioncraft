# Control protocol and MCP

> **Status (2026-11-13, second pass):** built and tested: the JSON-lines server (`visioncraft-cli serve`), the
> MCP server (`visioncraft-cli mcp`), the loopback control channel (server side
> `visioncraft_automation::control::serve_control`, used by `visioncraft-cli serve --control`;
> the desktop window does not call it yet) and the MCP bridge (`visioncraft-cli mcp --bridge`).
> The browser build has none of them.

This is 3DCraft's control protocol (`docs/control-protocol.md` of 3dcraft v0.1.1, MIT), with the
same envelope, method names, tool names, limits and error conventions, on VisionCraft's commands.
Where VisionCraft differs it says so under *Differences from 3DCraft* at the end.

## Envelope (JSON lines)

Each request is one JSON line; each reply is one JSON line with the same `id`:

```json
{"id": 1, "method": "engine.execute", "params": {"command": "time.set", "params": {"hour": 20.5}}}
{"id": 1, "ok": true, "result": null, "ms": 0.4}
{"id": 2, "method": "engine.execute", "params": {"command": "time.sett", "params": {}}}
{"id": 2, "ok": false, "error": "unknown command `time.sett`; did you mean `time.set`?", "ms": 0.0}
```

- `ms` (only in an `engine.execute` reply, ok or not) is the wall-clock milliseconds the command
  took, to a tenth; it is the only part of a reply that differs from run to run (`null` in a
  browser).
- A request has exactly `id`, `method` and `params`; any other key is refused. `params` of a
  command must be an object or absent.
- Errors are sentences an agent can act on. A command's error starts with its id and names the
  parameter: `shape.add_area: bad parameters: missing field \`outline\``,
  `light.add: unknown parameter \`lumen\`; its parameters are: name, position, kind, ...`.
- Invalid JSON gets `{"id": null, "ok": false, "error": "the request is not valid JSON: ..."}`
  and the next line is still read. Blank lines are skipped. No line makes the server stop.

## Methods

| Method | Params | Result |
|---|---|---|
| `engine.execute` | `{command, params?, intent?}` | the command's result |
| `engine.commands` | `{filter?, id?, brief? = false}` | `{commands: [...]}` (below) |
| `batch` | `{steps: [{command, params?, intent?}], stopOnError? = true, stopOnBreak? = false, terse? = false, intent?, atomic? = false}` | `{completed, failed, results: [{ok, result \| error, ms}], ms}`; terse: `{completed, failed, errors: [{step, error, ms}], last: {step, result, ms}, ms}`; with `atomic` also `rolledBack` |
| `methods` | `{}` | `{methods: [{name, params, summary}], limits: {requestBytes, replyBytes, batchSteps}}` |

On the control channel also: `auth`, `challenge`, `bye` (below), `mcp.message {line}` →
`{reply}` (the bridge's), and the host's own methods (a window's `ui.*`, listed by `methods`).

**A batch runs as in 3DCraft**: in order, checked as a whole (its shape, at most 256 steps)
before the first step runs; `failed` counts the commands that returned an error and
`stopOnError` (default true) stops at the first. **Not a transaction**: steps that ran stay
applied, each its own undo step; `edit.undo` takes back the last applied step, once per step.
`stopOnBreak` is accepted for 3DCraft's shape; a VisionCraft project has no features that fail,
so no step "breaks" anything and there is no `broke` or `featuresFailed`.

**`atomic: true`** (VisionCraft's extra) makes a batch all or nothing, inside one core transaction
(`Session::transaction`): the first step that fails stops it and takes back every step; the
project, the history and the open file are as before (`rolledBack: true`). An atomic batch that
succeeds and changes the project is **one** undo step. `file.new`, `file.open`, `edit.undo` and
`edit.redo` cannot be part of one. Files a step wrote (`file.save`, `view.render` with a path)
stay written either way.

**`intent`** (as 3DCraft): why the agent runs the command, in a short sentence; on
`engine.execute`, `batch` and each batch step (MCP: `command_run`, `command_batch`). Control
characters become spaces, direction marks and invisible characters go, at most 300 characters
(cut with `…`); not a string is an error naming `intent`. Every command an agent runs (but
`agent.log` and `document.inspect`) is noted in the session's **agent log**, with the intent:
the command `agent.log {limit? = 50}` → `{total, entries: [{client, timeMs, command, label, ok,
message, created, changed, intent}]}`, newest first, the last 500. `client` is the MCP client's
`clientInfo.name` (`agent` otherwise; a `run --script` step's `agent`). Session state: not saved,
not undone.

### The command list

`engine.commands` (MCP: `command_list`) gives per command, from the core's command specs
(`Session::command_specs()`) and the host's own:

```json
{"id": "viewpoint.add", "label": "Add Viewpoint",
 "params": "{\"name\": string, \"position\": [x, y, z] (m), \"target\": [x, y, z] (m), \"fov_y_deg\": number (deg) = 60, \"time\": object = not set}",
 "summary": "Saves a camera position and target, optionally with a time of day.",
 "parameters": [{"name": "fov_y_deg", "type": "number", "unit": "deg", "required": false, "default": "60", "allowed": [], "doc": "..."}, "..."],
 "openParams": false, "result": "{id}", "example": {"...": "..."},
 "undoable": true, "enabled": true, "disabledReason": null}
```

`brief: true` gives `{id, params, summary (its first sentence), enabled}`; `filter` is a
case-insensitive part of the id or label; `id` picks one command. `params` is 3DCraft's
notation; `parameters` the same as structured data; `example` runs (the core tests it on its
example project). The core checks every command's parameters against its spec before it runs:
unknown keys are refused with the nearest name, unless `openParams` is true.

### Commands

The core's commands (`model.*`, `shape.*`, `material.*`, `planting.*`, `species.add|update|remove`,
`light.*`, `viewpoint.*`, `layer.*`, `terrain.*`, `site.set`, `time.set`, `view.set`,
`style.set`) change the project, each one undo step; its queries (`query.sun {time?}`,
`query.measure {kind, points?, outline?}`) never do. The host adds what needs files, history,
loaded geometry or a GPU:

| Command | Params | Result |
|---|---|---|
| `file.new` | `{}` | `{models: 0}`: an empty project, a new history |
| `file.open` | `{path}` | `{path, models, shapes, generators, lights, viewpoints, missingModels}`; loads the models it refers to; a new history |
| `file.save` | `{path?, overwrite? = false}` | `{path, bytes}`; model paths follow the file to a new folder |
| `edit.undo`, `edit.redo` | `{}` | `{can_undo, can_redo}` |
| `document.inspect` | `{faces? = true, measures? = true, placements? = false, full? = false}` | below |
| `query.ground_length` | `{path}` | `{ground_length_m}`: a walk over terrain, areas and models |
| `query.raycast` | `{origin, direction}` | `{hit, owner, kind, face, body, point, normal, distance}` or `{hit: false}` |
| `query.ground_height` | `{x, z}` | `{x, z, height}` (terrain, areas and models; `null` over nothing) |
| `agent.log` | `{limit? = 50}` | `{total, entries}` (above) |
| `catalog.search` | `{query?, category?, exposure?, max_height_m?, limit? = 50}` | `{total, assets: [{id, name, category, size_m, license, author, plant}]}` |
| `species.list` | `{filter?, category?}` | `{species: [{asset: "flora:<id>", common_name, ...}], custom: [{asset, name, base}]}` |
| `planting.list_presets` | `{}` | `{presets: [{id, name, layers}]}` |
| `view.render` | `{path?, width? = 800, height? = 600, viewpoint?, camera?, time?, years?, quality? = "high", samples? = 64, aperture_m? = 0, focus_m?}`; quality `"photo"` averages `samples` frames at ULTRA (soft shadows, smooth edges; `aperture_m`/`focus_m` for depth of field) | a PNG: to `path`, else `{mime: "image/png", base64}`; with `camera`, `sun`, `time`, `plantsPending`, `renderer` |

`view.render` draws with the app's own renderer (the window-free studio) on a headless GPU
device made at the first picture; plants are grown before the picture is taken. The camera is
the viewpoint's (by id or name, with its time when it has one), else `camera {position, target,
fov_y_deg? = 50}`, else the whole scene framed from the default orbit. `time` changes the picture
only, never the project.

## What `document.inspect` returns

The contract that lets an agent check its work without a picture. Field names are part of the
protocol. Items are given in the project's own words, as the commands take them (snake_case),
plus what is computed:

```json
{
  "path": "/work/garden.visioncraft", "dirty": false, "revision": 7,
  "site": {"latitude": 51.05, "longitude": 3.72, "utc_offset_hours": 2.0},
  "time": {"year": 2026, "month": 6, "day": 21, "hour": 15.0},
  "sun": {"azimuth_deg": 213.9, "elevation_deg": 59.0, "direction": [-0.29, 0.86, 0.43], "sunrise_hour": 5.50, "sunset_hour": 22.06, "above_horizon": true},
  "view": {"...": "..."}, "style": {"clay": false},
  "layers": [{"id": "shapes", "name": "Shapes", "visible": true, "items": 2}],
  "models": [{"id": "m1", "name": "plate", "kind": "3dcraft", "source": "plate.3dcraft", "layer": "model", "transform": {"translation": [0, 0, 0], "rotation_y_deg": 0, "scale": 1},
              "loaded": true, "bodies": 1, "triangles": 12, "bbox": [[0, 0, -0.02], [0.04, 0.01, 0]],
              "faces": ["F2:side(e5)", "F2:side(e6)", "F2:side(e7)", "F2:side(e8)", "F2:startCap(e5,e6,e7,e8)", "F2:endCap(e5,e6,e7,e8)"]}],
  "shapes": [{"type": "area", "id": "S1", "name": "Lawn", "layer": "shapes", "outline": [[-10, -2], [10, -2], [10, 10], [-10, 10]], "elevation": 0.0, "drape": false, "area_m2": 240.0, "faces": ["S1:top"]},
             {"type": "extrusion", "id": "S2", "name": "House", "...": "...", "footprint_m2": 70.0, "faces": ["S2:top", "S2:bottom", "S2:side(0)", "..."]}],
  "materials": [{"id": "lawn", "name": "Lawn"}],
  "assignments": [{"target": {"type": "owner", "owner": "S1"}, "material": "lawn"}],
  "generators": [{"id": "g1", "type": "spline_mass", "name": "Hedge", "layer": "planting", "assets": ["flora:carpinus-hedge"], "count": 26, "truncated": false}],
  "species": [], "lights": [{"id": "li1", "name": "Garden lamp", "...": "..."}], "viewpoints": [{"id": "v1", "...": "..."}],
  "terrain": null,
  "history": {"can_undo": true, "can_redo": false}
}
```

Verbosity: `faces: false` leaves out the face names (a large 3dcraft house has hundreds);
`measures: false` skips evaluating generators and measuring shapes; `placements: true` lists
every plant (`asset, position, rotation_y_deg, scale`); `full: true` gives materials, generators
and species with all their fields. A model whose file could not be read has `loaded: false` and
`error`, and is listed under `missingModels`.

## MCP

`visioncraft-cli mcp` serves MCP on stdio (JSON-RPC 2.0, one message per line). Protocol
versions 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05 (the client's when it is one of
these, else the latest); capabilities `tools`, `resources`, `prompts`; `initialize` carries
`instructions` telling the client how to start. Methods: `initialize`, `ping`, `tools/list`,
`tools/call`, `resources/list`, `resources/templates/list`, `resources/read`, `prompts/list`,
`prompts/get`. Notifications are never answered.

| Tool | Arguments | Notes |
|---|---|---|
| `command_list` | `brief?`, `id?`, `filter?`, `enabled_only?` | as `engine.commands` |
| `command_run` | `id`, `params?`, `intent?` | one command; `_meta.ms`; a refused command is `isError: true` with a message that starts with its id |
| `command_batch` | `steps: [{id, params?, intent?}]` (≤ 256), `stop_on_error? = true`, `stop_on_break? = false`, `terse? = false`, `intent?`, `atomic? = false` | as `batch`; `isError` when a command failed, the report is the content; not atomic unless `atomic` |
| `doc_inspect` | `faces?`, `measures?`, `placements?`, `full?` | `document.inspect` |
| `doc_render_preview` | `viewpoint?`, `camera?`, `time?`, `quality?` (also `"photo"`), `samples?`, `max_side?` (16..2048, default 1024) | an image block (PNG, 4:3) and the render's JSON as text |
| `control_call` (only when the host has methods, i.e. a window) | `method`, `params?` | a window method; a picture comes back as an image block |

Resources: `visioncraft://project` (`document.inspect` as JSON) and `visioncraft://guide` (the
agent guide, [`agent.md`](agent.md)). Prompt: `build_garden {description}`.

Errors come in MCP's two kinds: a request the server cannot act on (unknown method `-32601`,
unknown tool or argument `-32602`, bad JSON `-32700` with `id: null`, not an object `-32600`) is
a JSON-RPC error raised before anything runs; a command that ran and failed is a normal result
with `isError: true`. A panic becomes an error (`-32603`) and the server keeps serving.

## The control channel (a running window)

A program that shows a project listens on **127.0.0.1 only**, behind a token:
`serve_control(ControlOptions {port, token_file, state_dir}) -> ControlHandle`; the program
answers requests on its own thread with `handle.poll(&mut host)` (every frame) or
`handle.serve_one(&mut host, timeout)`, so its host never leaves that thread. `visioncraft-cli
serve --control PORT` runs one headless.

- **Token**: 64 hexadecimal characters in a file of mode 0600 (made when missing, written whole).
  A token file that other users can read or that is not the user's own is refused.
- **First request**: `auth {token}`, or better `challenge {nonce}` → `{nonce, pid, proof}` then
  `auth {proof}`: HMAC-SHA256 proofs over the port, the process id and both nonces with the roles
  `"visioncraft window"` and `"visioncraft client"`, so the token never crosses the socket and a
  program squatting the port learns nothing. Anything else first, or a wrong proof, closes the
  connection; an HTTP request line closes it without a reply; 30 s of silence closes it; at most
  16 connections (`too many connections`).
- **Discovery**: the state folder is `$VISIONCRAFT_STATE_DIR`, else `visioncraft` in the per-user
  state directory (`$XDG_STATE_HOME` or `~/.local/state`; macOS `~/Library/Application Support`;
  Windows `%LOCALAPPDATA%`). It holds `control.token` and `windows/<pid>.json` =
  `{pid, port, tokenFile, focusedAt, startedAt}` for each listening program that uses the default
  token file (one with a token file of its own writes none). Ports: the first free of 7920 to
  7939. `ControlHandle::focused()` updates `focusedAt`; dropping the handle removes the entry.
- **The bridge** (`visioncraft-cli mcp --bridge [PORT] [--state-dir DIR]`, or `--bridge PORT
  --control-token-file FILE`): an MCP server without a session; every message goes to the
  window's `mcp.message`, one short connection each (challenge, auth, message). It picks the
  only live entry, else the one focused last (an error names them when that does not decide);
  an entry whose process or port is gone is removed. Without a window it answers `initialize`,
  `ping` and `tools/list` itself and every tool call with `-32000` and how to start the window.
  At the end of its input it says `bye`.

## File access

`serve` and `mcp` read files only beneath `--automation-read-root` and write only beneath
`--automation-write-root`; without one, that direction is denied (`this server may not write
files; start it with --automation-write-root DIR`). A relative path is beneath the root; an
absolute one must lie beneath it; `..` and links that lead out are refused. Models are loaded
from paths relative to the project file. `run`, `info`, `render`, `export`, `replay`, `convert`
and `new` are for the local user and read and write anywhere.

## Limits

1 MiB request line, 8 MiB reply (a larger reply becomes an error that says to ask for less), 256
steps per batch, previews 16 to 2048 px with at most 5 MiB of PNG, renders 16 to 4096 px a side.

## Differences from 3DCraft

Only what the domain or a missing part forces:

| 3DCraft | VisionCraft | Why |
|---|---|---|
| batch `broke`, `featuresFailed`, `stopOnBreak` stops | `stopOnBreak` accepted, never stops; no `broke`/`featuresFailed` | a project has no features that fail |
| — | batch `atomic: true` (`rolledBack`) | an extra, off by default |
| agent log only in the window (headless `craft3d-cli mcp` keeps none); `madeBy`/`lastChangedBy` written into the document; `features` in `agent.log` | kept headless too; nothing written into the project; no `features` | the project format is the core's; nothing records who made an item |
| `doc_inspect` takes no arguments | takes `faces`, `measures`, `placements`, `full` (all optional; none gives the defaults) | a 3dcraft house has hundreds of face names |
| `selection_get`, resource `craft3d://selection` | none; resources `visioncraft://project`, `visioncraft://guide`; prompt `build_garden` | the core has no selection; the guide is the MCP-served agent guidance (3DCraft serves only `instructions`) |
| `doc_render_preview {view, mode, projection, zoom, box, tolerance, max_side}` | `{viewpoint, camera, time, quality, max_side}` | viewpoints and the sun instead of views of a part |
| `document.measure` (bodies), `view.pick`, `view.replay` | `query.measure`, `query.raycast`, `query.ground_*`, `query.sun`, `catalog.search`, `species.list`; replay is a viewpoint tour (`visioncraft-cli replay`) | the domain |
| `file.close`, tabs, several documents; `file.export` | one project per session; pictures only (`view.render`, `visioncraft-cli export`) | the app has one project; the core has no mesh exporter |
| recovery, autosave | none | not built |
| ports 7878–7897, roles `3dcraft window/client`, state folder `3dcraft/` | 7920–7939, `visioncraft window/client`, `visioncraft/` | both apps run side by side |
| `agents.json` switch, consent prompts, `ui.*` methods | not here | the window's part, not built yet |
