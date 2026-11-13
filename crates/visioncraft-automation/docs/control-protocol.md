# Control protocol and MCP

> **Status (2026-11-13):** built and tested: the JSON-lines server (`visioncraft-cli serve`), the
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
| `engine.execute` | `{command, params?}` | the command's result |
| `engine.commands` | `{filter?, id?, brief? = false}` | `{commands: [...]}` (below) |
| `batch` | `{steps: [{command, params?}], terse? = false}` | `{completed, failed, rolledBack, results: [{ok, result \| error, ms}], ms}`; terse: `{completed, failed, rolledBack, errors: [{step, error, ms}], last: {step, result, ms}, ms}` |
| `methods` | `{}` | `{methods: [{name, params, summary}], limits: {requestBytes, replyBytes, batchSteps}}` |

On the control channel also: `auth`, `challenge`, `bye` (below), `mcp.message {line}` →
`{reply}` (the bridge's), and the host's own methods (a window's `ui.*`, listed by `methods`).

**A batch is all or nothing.** Its steps run on a scratch copy of the project; the first step
that fails stops it and nothing it did stays: the project, the history and the open file are as
before (`rolledBack: true`; the failing step's error starts with `step N:`). A batch that
succeeds and changes the project is **one** undo step. Every step is checked (its shape, at most
256 steps) before the first runs. Files a step wrote (`file.save`, `view.render` with a path)
stay written. Inside a batch, `edit.undo` takes back earlier steps of the same batch only.

### The command list

`engine.commands` (MCP: `command_list`) gives per command:

```json
{"id": "viewpoint.add", "label": "Save a camera",
 "params": "{\"name\": string, \"position\": [x, y, z] (m), \"target\": [x, y, z] (m), \"fov_y_deg\": number (deg) = 60, \"time\": object = not set}",
 "summary": "Save a camera: {name, position, target, fov_y_deg? (60), time?}; returns {id}",
 "parameters": [{"name": "fov_y_deg", "type": "number", "unit": "deg", "required": false, "default": "60", "allowed": [], "doc": ""}, "..."],
 "openParams": false, "result": "{id}", "example": {"name": "From the gate", "position": [0, 1.7, 14], "target": [0, 1.5, 0]},
 "undoable": true, "derivedDocs": true, "enabled": true, "disabledReason": null}
```

`brief: true` gives `{id, params, summary (its first sentence), enabled}`; `filter` is a
case-insensitive part of the id or label; `id` picks one command. `params` is 3DCraft's
notation; `parameters` the same as structured data. `derivedDocs: true` marks a core command
whose parameter docs were read from its one-line description (types and units then follow the
naming conventions: `_m` metres, `_deg` degrees, `position` `[x, y, z]`, `outline` `[[x, z], ...]`).
Unknown parameter keys are refused for every command whose parameters are all listed
(`openParams: false`).

### Commands

The core's commands (`model.*`, `shape.*`, `material.*`, `planting.*`, `species.add|update|remove`,
`light.*`, `viewpoint.*`, `layer.*`, `terrain.*`, `site.set`, `time.set`, `view.set`,
`style.set`) change the project; each is one undo step. The host adds:

| Command | Params | Result |
|---|---|---|
| `file.new` | `{}` | `{models: 0}`: an empty project, a new history |
| `file.open` | `{path}` | `{path, models, shapes, generators, lights, viewpoints, missingModels}`; loads the models it refers to; a new history |
| `file.save` | `{path?, overwrite? = false}` | `{path, bytes}`; model paths follow the file to a new folder |
| `edit.undo`, `edit.redo` | `{}` | `{can_undo, can_redo}` |
| `document.inspect` | `{faces? = true, measures? = true, placements? = false, full? = false}` | below |
| `document.measure` | `{points?, outline?, path?}` | `{length_m, height_difference_m, slope_percent}` / `{area_m2}` / `{ground_length_m}` |
| `query.raycast` | `{origin, direction}` | `{hit, owner, kind, face, body, point, normal, distance}` or `{hit: false}` |
| `query.ground_height` | `{x, z}` | `{x, z, height}` (terrain, areas and models; `null` over nothing) |
| `query.sun` | `{time?}` | `{azimuth_deg, elevation_deg, direction, sunrise_hour, sunset_hour, above_horizon}` |
| `catalog.search` | `{query?, category?, exposure?, max_height_m?, limit? = 50}` | `{total, assets: [{id, name, category, size_m, license, author, plant}]}` |
| `species.list` | `{filter?, category?}` | `{species: [{asset: "flora:<id>", common_name, ...}], custom: [{asset, name, base}]}` |
| `planting.list_presets` | `{}` | `{presets: [{id, name, layers}]}` |
| `view.render` | `{path?, width? = 800, height? = 600, viewpoint?, camera?, time?, quality? = "high"}` | a PNG: to `path`, else `{mime: "image/png", base64}`; with `camera`, `sun`, `time`, `plantsPending`, `renderer` |

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
| `command_run` | `id`, `params?` | one command; `_meta.ms`; a refused command is `isError: true` with a message that starts with its id |
| `command_batch` | `steps: [{id, params?}]` (≤ 256), `terse?` | as `batch`: all or nothing; `isError` when a step failed, the report is the content |
| `doc_inspect` | `faces?`, `measures?`, `placements?`, `full?` | `document.inspect` |
| `doc_render_preview` | `viewpoint?`, `camera?`, `time?`, `quality?`, `max_side?` (16..2048, default 1024) | an image block (PNG, 4:3) and the render's JSON as text |
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

| 3DCraft | VisionCraft | Why |
|---|---|---|
| `batch`/`command_batch` is not atomic; each step one undo step; `stopOnError`, `stopOnBreak` | all or nothing, one undo step, always stops at the first failure; only `terse` | asked for (all-or-nothing batches); there are no failing features to judge |
| `intent` on `engine.execute`, `batch`, `command_run`, `command_batch` | none | no window agent log yet |
| `doc_inspect` takes no arguments | takes `faces`, `measures`, `placements`, `full` | a 3dcraft house has hundreds of face names; agents need to turn them off |
| `selection_get`, resource `craft3d://selection` | none; resources `visioncraft://project` and `visioncraft://guide` | the core has no selection; the guide replaces 3DCraft's skill text |
| no prompts | `prompts/list`, `prompts/get` with `build_garden` | asked for |
| `doc_render_preview {view, mode, projection, zoom, box, tolerance, max_side}` | `{viewpoint, camera, time, quality, max_side}` | the domain: viewpoints and the sun instead of views of a part |
| commands `document.measure` (bodies) | `document.measure` (polylines, areas, walks over the ground), plus `query.*`, `catalog.search`, `species.list` | the domain |
| `file.close`, tabs, several documents | one project per session | the app has one project |
| recovery, autosave | none | not built |
| ports 7878–7897, roles `3dcraft window/client`, state folder `3dcraft/` | 7920–7939, `visioncraft window/client`, `visioncraft/` | both apps run side by side |
| `agents.json` preference, consent prompts, `ui.*` methods | not here | the window's part, not built yet |
