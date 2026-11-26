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
`fence.*`, `stairs.*`, `bed.*`, `terrace.add|set|clear`, `furniture.add|set|clear|add_set`, `underlay.*`, `wall_photo.*`, `light.*`, `viewpoint.*`, `film.add|update|add_shot|update_shot|remove_shot|remove`, `layer.*`, `terrain.*`, `site.set`, `time.set`, `view.set`, `view.set_photo`, `view.set_sun_map`,
`view.clear_sun_map`, `style.set`) change the project, each one undo step; its queries (`query.sun {time?}`,
`query.measure {kind, points?, outline?}`, `query.quantities {ids?, layer?}`) never do. Curves are `shape.*` commands too:
`shape.set_curve {id, edge, radius_m?, sagitta_m?, through?, side?}`, `shape.clear_curve {id, edge}`,
`shape.set_smooth {id, smooth}`, `shape.add_circle {center, radius_m, kind, ...}` and `shape.add_ellipse {center, radius_x, radius_z, rotation_deg?, kind, ...}`; `shape.insert_vertex` splits an arc in two on the same circle and answers `{position}`; `stairs.add` and `stairs.set` take `front_bow_m` (a round front: an arc on the bottom edge with concentric risers); `shape.add_area`,
`shape.add_extrusion`, `bed.add` and `fence.add` take `curves` and `smooth`. `fence.add` and `fence.set` take `slope` (`level`, `stepped`, `raked`) and `gravel_board`, and answer with `warnings`. `stairs.add` and `stairs.set` take `set_in` (`{cheek_style: plain|masonry|sleepers|corten|gabion|none, thickness_m?, coping_m?}`, `null` in `stairs.set` takes it out): the flight is set into the slope it climbs, its goings (`goings_m` in the answer) follow the original ground, its footprint is cut out of the terrain like a terrace's (a ground question inside it answers with the tread's level) and cheek walls stand along both sides (faces `<id>:cheeks`, `<id>:cheeks_rim`); the answer's `set_in` and the `stairs.set_in` row of `query.quantities` give the goings, cheek length and face areas, and the cut and fill in m3. A shape's `outline` in
`document.inspect` is its control points, with `curves` and `smooth` next to it when it has them; areas,
lengths and quantities measure the curved outline. Weathering is a material field and a view field:
`material.add|update {weathering: {amount, dirt?, moss?, wear?, streaks?}}` (each 0..1; the rest
default to 0.6, 0.4, 0.4, 0.5; an update merges key by key, `null` clears it) and `view.set
{weathering: 0..1}` (every standard material at once, added to each one's own); both undoable, both
stored in the project (not written while off) and drawn by the renderer as dirt, moss, wear and
rain streaks on albedo and roughness only; `document.inspect` shows them under `materials` (with `full: true`) and
`view`. The weather is view fields too, each 0..1, undoable and not written while 0: `view.set {wetness, rain,
mist, gustiness}`. `mist` is a low layer of droplets (visibility 1.3 km at 0.3, 330 m at 0.6, 120 m at 1) under
the dry-air `haze` (a turbidity), and rain brings its own fog (400 m of visibility at 0.8); both fade the distance
and the horizon into a veil lit by the sun and the sky. `gustiness` is gusts travelling downwind at the wind speed
over lawns (waves of bending, paler grass), through crowns and in the rain's slant; `wind_gustiness` remains the
slow swell of the wind. `shafts` (0..1, not written while 0) shadows the sun's light in the haze, mist and
rain fog with the sun's cascades (beams through crowns, lanes past house edges; nothing without a medium).
The camera is view fields too: `white_balance` (`"daylight"`, the default and not written; `"auto"` from the
sun's and the sky's light; or kelvin 2000..12000) and `grade` (`neutral`, the default and not written; `warm`,
`cool`, `soft`), with `vignette` and `grain` (each 0..1, not written while 0: the lens' fall-off towards
the corners before the tone map, film grain in the luminance after it). `view.set_photo` keeps how a photo is
taken (frames, f-number, focus, shutter; see `view.render`). The host adds what needs files, history,
loaded geometry or a GPU:

| Command | Params | Result |
|---|---|---|
| `file.new` | `{}` | `{models: 0}`: an empty project, a new history |
| `file.open` | `{path}` | `{path, models, shapes, generators, lights, viewpoints, missingModels}`; loads the models it refers to; a new history |
| `file.save` | `{path?, overwrite? = false}` | `{path, bytes}`; model paths follow the file to a new folder |
| `edit.undo`, `edit.redo` | `{}` | `{can_undo, can_redo}` |
| `edit.state` | `{}` | `{can_undo, can_redo, dirty, revision}` |
| `document.inspect` | `{faces? = true, measures? = true, placements? = false, full? = false}` | below |
| `query.ground_length` | `{path}` | `{ground_length_m}`: a walk over terrain, areas and models |
| `query.sky_view` | `{point: [x, z] \| [x, y, z], normal?, years?}` | `{point, normal, sky_view, crowns, years, month}`: how open a spot is, the cosine-weighted share of the sky it sees (0..1), with buildings, shapes, models, terrain and crowns by their leaf density |
| `query.quantities` | `{ids?, layer?}` | `{counted_with: "scene" \| "plan", shapes, plantings, models, models_missing, materials, assets, totals}`; the host adds `models`, one row per linked model `{id, name, layer, source, scale, bodies: [{id, name, hidden, surface_m2, volume_m3 (null unless the body is closed), materials}], surface_m2, volume_m3 (closed bodies), open_bodies, materials}` with the transform's scale applied (area x scale squared, volume x scale cubed), folded into `materials` and `totals`; `models_missing` lists models whose file is not loaded. Plants are counted as the renderer draws them (`counted_with` `scene`: the building rule applies); a core-only `Session::run` has no `models` and counts `plan`. |
| `query.raycast` | `{origin, direction, plants? = false, years?}` | `{hit, owner, kind: model \| shape \| terrain \| plant, face, body, point, normal, distance}` or `{hit: false}`; with `plants` the crowns (ellipsoids) and trunks (cylinders) of the plantings, sized by the growth model `years` after planting (else the view's years, else full grown), are hit too: `owner` is `g4#12` (planting and plant), with `part` (`crown` \| `trunk`), `planting`, `name`, `asset` |
| `query.clearance` | `{from, to, clearance_m? = 0.4, eye_height_m? = 1.6, years?}` | `{from, to, length_m, clear, crossings: [{what, at, distance_m}], nearest, route?: {path, length_m, warnings}, sight: {clear, blocked_by?}}`: can one walk and see from one place to another; `from` and `to` are `[x, z]`, `[x, y, z]` or a reference (a shape id, a planting, `g4#12`, a model, a lamp, a viewpoint, `plot` or a name such as `"Pond"`: its centre); the walk is the straight line against houses, blocks, walls, fences, water, hedges, shrubs and trunks, and a route round them is planned when it is blocked |
| `query.sun_hours` | `{point \| outline, season \| date?, cell_m? = 1, height_m? = 0.05, step_minutes? = 10, plants? = true, years?}` | a point: `{point, height, date, sunrise_hour, sunset_hour, daylight_hours, sun_hours, intervals}`; an outline: `{date, cell_m, origin, cols, rows, hours, min, max, mean, daylight_hours, histogram, full_sun_share, part_shade_share, shade_share}` |
| `query.plants_for_light` | `{point \| outline \| hours, season \| date?, grower?, limit? = 25, cell_m? = 1, plants? = true, years?}` | the spot's hours (`spot`: min, mean, max) and the species ranked by fit (`full` or `partial`) with reason, estimated light range and growth flags; `hours` alone runs without a host; the day defaults to the summer solstice |
| `query.light_check` | `{season \| date?, years?, ids?, layer?, suggestions? = 3, include_ok?, limit? = 200, step_minutes? = 15, plants? = true}` | `summary` (checked, ok, too_sunny, too_shady), the misfit `plants` with hours, light range and swap `suggestions`, `by_species`, `by_planting` |
| `query.ground_height` | `{x, z}` | `{x, z, height}` (terrain, areas and models; `null` over nothing) |
| `query.clips` | `{model}` | `{clips: [{name, duration_s, channels}], warnings}`: the animation clips of a model that moves in parts (glTF node animation), a catalogue asset id or `user:<id>`; `channels` is how many channels a clip has, an empty list for a model that stands still; the model's file must be at hand (an imported object's always is; a library model's once loaded, or from the library folder on the command line) |
| `model.play_clip` | `{object, clip, start_s?, loop?}` | `{object, planting, plant, plants, clip, start_s, loop, duration_s, restarted, playing}`: plays an animation clip on the objects a planting places (`object` a planting `g4` or one of its objects `g4#12`); `clip` a clip's name or its index from 0 (an exact name first, of a doubled name the lowest index); sampled at the session's clock from `start_s` (now: in a window its frame time, so a clip for `film.render` gives `start_s` on the film's time); the same clip starts again, of two on the same property the later wins, an ended clip holds its pose until stopped unless `loop`; not saved, not an undo step |
| `model.stop_clip` | `{object, clip?}` | `{object, stopped, playing}`: stops the clips on a planting's objects, of `clip` only or all; a planting (`g4`) also stops its objects' (`g4#3`), one object of a planting that plays as a whole is an error (stop the planting); a stopped part is at rest |
| `query.part_pose` | `{object, t?}` | `{object, t, plants: [{plant, asset, posed, clips, parts}]}`: per object the matrix in model space (column-major, 16 numbers) of every moving part at clock time `t` (now), the clips on it composed as drawn |
| `effect.flash` | `{position, color, intensity, range_m, size_m? = 0.15, frames? = 2, seconds?, start_s?}` | `{id, start_s, frames | seconds, size_m, flashes, note?}`: a short-lived point light with a glowing quad facing the camera, for `frames` frames of the rate in force (a film's fps, the same frames every time) or `seconds`, from `start_s` (now: forgotten once over; with `start_s`, on a film's time, kept for every render of the film, and in a window needed for `film.render` to show it); at most 16 at once (whose times overlap, `frames` counted at 60 a second at least), the oldest of those dropped, flashes at other times not counted; at most 256 with `start_s` kept, the one that ends first dropped (`note` says so); not saved, not an undo step |
| `agent.log` | `{limit? = 50}` | `{total, entries}` (above) |
| `catalog.search` | `{query?, category?, exposure?, max_height_m?, limit? = 50}` | `{total, assets: [{id, name, category, size_m, license, author, plant}]}` |
| `species.list` | `{filter?, category?}` | `{species: [{asset: "flora:<id>", common_name, ...}], custom: [{asset, name, base}]}` |
| `planting.list_presets` | `{}` | `{presets: [{id, name, layers}]}` |
| `planting.add_border` | `{outline \| area, style, name?, front?, facing?, height_range?, species?, closure? = 1.3, drift_m? = 2.4, seed?, avoid_buildings?, dry?, layer?}` | `{id, facing, height_range, species, plants}`; a layered perennial border, an area scatter with a `border`: species by height from the front (`facing` `[x, z]` from back to front, or `front` edge indices, else away from the nearest wall, building or hedge within 2.5 m, else the longest edge's outward side) in drifts with a matrix species between them, `closure / w^2` plants per m2 from each species' mature width |
| `underlay.add` | `{file, name?, layer? = "underlay", origin? = [0, 0], metres_per_pixel? = 0.05, rotation_deg? = 0, opacity? = 0.5, elevation? = 0.02, size_px?}` | `{id}`; the host reads the PNG or JPEG (relative to the project file or the read root) and fills `size_px`; a missing, unreadable or other file is an error unless `size_px` is given for a file the host does not have |
| `underlay.set` | `{id, name?, file?, layer?, origin?, metres_per_pixel?, rotation_deg?, opacity?, elevation?, size_px?}` | `null` |
| `underlay.calibrate` | `{id, pixel_a, pixel_b, world_a, world_b}` | `{origin, metres_per_pixel, rotation_deg}`: scale, turn and origin from two image positions and the drawing points they land on |
| `underlay.scale_by_distance` | `{id, pixel_a, pixel_b, distance_m}` | `{metres_per_pixel}`; origin and rotation stay |
| `underlay.remove` | `{id}` | `null` |
| `view.set_photo` | `{samples? = 64, aperture? = null, focus? = "target", focus_point? = [0.5, 0.5], shutter_s? = 0}` | `null`; how a photo is taken, stored with the project (only the fields given change): frames averaged (1..1024), the f-number (1..32; null all sharp), the focus (`"target"`, `"auto"` at `focus_point` or metres 0.1..1000) and the shutter time (0..1 s); the app's photo export and `view.render` at quality `"photo"` use them |
| `view.set_sun_map` | `{shape \| outline, season \| date {month, day}, years?, cell_m? = 0.5, opacity? = 0.6}` | `null`; draws the hours of direct sun a day gives over an area on the ground (`shape` an area's id, or an `outline` such as the box round the plot), in the app and in `view.render`: deep violet for no sun through blue, teal and green to amber at 8 hours or more, see-through (`opacity`), flat a few cm over the area (also on terrain); cells under an area drawn on top of the target (later in the shape list and overlapping it: a pond, a path) are clear and left out of the legend's shares. The sun is cast as `query.sun_hours` does it; `years` null follows the view's years. When a map is on, only the fields given change (a new target replaces the old, `date` and `season` replace each other, null clears `date`, `season` or `years`). Stored in `document.inspect`'s `view.sun_map`. |
| `view.clear_sun_map` | `{}` | `null`; takes the map off |
| `wall_photo.add` | `{file, target, corners_px?, region?, name?, size_px?}` | `{id}`; a photo of a facade on one wall face (`target` `{type: "face", owner, face}`), straightened by `corners_px` (the facade's four corners in the photo, bottom-left, bottom-right, top-right, top-left as seen from outside; default: the photo's own corners) to fill `region` `{u0, v0, u1, v1}` metres on the face (default: the whole face); the host reads the PNG or JPEG as for `underlay.add` and fills `size_px` |
| `wall_photo.set` | `{id, name?, file?, target?, corners_px?, region?, size_px?}` | `null`; `region: null` is the whole face again |
| `wall_photo.remove` | `{id}` | `null` |
| `film.preview` | `{id, t?, sheet? = false, frames? = 9, width? = 320, quality? = "low"}` | `{id, name, fps, duration_s, frame_count, shots: [{index, kind, start_s, duration_s}]}` and, with `t` (seconds, clamped into the film), `{t, shot, progress, camera: {position, target, fov_y_deg}, overrides: {time_of_day?, date?, season?, years?, snow?, wetness?, rain?, haze?, mist?, gustiness?, shafts?, lights?, white_balance?, grade?, grade_to?, grade_mix?, exposure_ev?}, time, sun}`: read-only; a walk follows the project's real ground (terrain, areas, models); with `sheet` also `{mime: "image/png", base64, sheet: {columns, frames, cell_px, size_px, times_s, shots, order}}`: the film at `frames` evenly spaced times, `width` pixels wide each, tiled on one picture (an image block over MCP: an agent sees the film with no other tool; needs the renderer) |
| `film.render` | `{id, dir, width? = 960, height? = 540, quality? = "high" \| "low" \| "medium" \| "ultra" \| "photo", from?, to?, every? = 1, samples?, aperture?, focus_m?, focus_point?, shutter_s?, vignette?, grain?, preview?}` (the photo keys as `view.render`'s, for every frame: `samples` frames averaged per picture, `shutter_s` the shutter time, `vignette` and `grain` 0..1 as `view.set`'s; quality `"photo"` averages 8 samples a frame unless `samples` says; `preview: "sheet"` also returns a contact sheet of up to nine of the frames drawn) | `{dir, frames, framePattern: "frame_%05d.png", fps, width, height, duration_s, first_t, last_t, renderer, ffmpeg}` (and with `preview` `mime`, `base64`, `sheet`); one `view.render` per frame into `dir` (beneath the write root, as `view.render`'s `path`): the film's camera, `animation_s` = the film time, and the shot's time of day, date or season, years, snow, wetness, rain, haze, mist, gustiness, shafts, lamp light, white balance, grade and exposure compensation for that picture only; frames are numbered from 0 in what is drawn, `every` N makes a film of fps / N; at most 10000 frames a call; `ffmpeg` is the command that makes a video of them (VisionCraft has no codec; `visioncraft-cli film --video` runs ffmpeg when it is on the PATH) |
| `view.render` | `{path?, width? = 800, height? = 600, viewpoint?, camera?, time?, years?, season?, snow?, wetness?, rain?, haze?, mist?, gustiness?, shafts?, weather?, weather_to?, weather_mix?, clouds?, lightning?, sky_occlusion?, white_balance?, grade?, grade_to?, grade_mix?, lights?, exposure_offset_ev?, vignette?, grain?, animation_s?, quality? = "high", samples?, aperture?, aperture_m?, focus_m?, focus_point?, shutter_s?}`; the weather and look keys hold for that picture only; `camera` is `{position, target, fov_y_deg?}` whose `position` and `target` may be references; `samples` frames (1..1024) are averaged in linear HDR before bloom and the tone map, at any quality; quality `"photo"` is ULTRA with every lens field not given from `view.set_photo` (64 samples by default), other qualities average only with `samples` over 1, an `aperture` (f-number 1..32, or `aperture_m` a radius) or a `shutter_s`; `focus_m` is metres, `"auto"` (the depth at `focus_point`) or `"target"`; `lights` is true, false or the share (0..1) of the lamps' light (any value forces the lamps on or off), `grade_to` and `grade_mix` mix a second grade over `grade`, `weather` (clear, fair, cloudy, overcast, rain, storm) sets the clouds, rain, wetness, mist, gusts and wind of that weather and `weather_to` and `weather_mix` mix a second over it, `clouds` is a cover 0..1 or an object merged into the project's as `view.set` takes it, and `exposure_offset_ev` is stops relative to the automatic exposure (0: none; not `view.set`'s `exposure_ev`, an absolute value); `sky_occlusion` lights the picture by the sky each point sees (the project's, else on) | a PNG: to `path`, else `{mime: "image/png", base64}` (over MCP an image block); with `camera`, `sun`, `time`, `plantsPending`, `renderer` |

The film commands (`film.add {name, fps? = 25}`, `film.update {id, name?, fps?}`,
`film.add_shot {id, shot, index?}`, `film.update_shot {id, index, shot}`, `film.remove_shot {id, index}`,
`film.remove {id}`; `shot` is `{kind: walk | glide | rise | orbit | flyover | hold, ...}` with
`duration_s` or `speed_mps`, `ease`, `look_at`, `fov_y_deg` and `time_of_day`, `date`, `season`,
`years`, `snow` along the shot, all in `command_list {"id": "film.add_shot"}`) change the project,
each one undo step; the films are in `document.inspect`'s `films`. The orbit's angle is from east of
the centre, positive counter-clockwise seen from above (like `rotation_y_deg`).

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
  "layers": [{"id": "shapes", "name": "Shapes", "visible": true, "context": false, "items": 2}],
  "models": [{"id": "m1", "name": "plate", "kind": "3dcraft", "source": "plate.3dcraft", "layer": "model", "transform": {"translation": [0, 0, 0], "rotation_y_deg": 0, "scale": 1},
              "loaded": true, "bodies": 1, "triangles": 12, "bbox": [[0, 0, -0.02], [0.04, 0.01, 0]],
              "faces": ["F2:side(e5)", "F2:side(e6)", "F2:side(e7)", "F2:side(e8)", "F2:startCap(e5,e6,e7,e8)", "F2:endCap(e5,e6,e7,e8)"]}],
  "shapes": [{"type": "area", "id": "S1", "name": "Lawn", "layer": "shapes", "outline": [[-10, -2], [10, -2], [10, 10], [-10, 10]], "elevation": 0.0, "drape": false, "area_m2": 240.0, "faces": ["S1:top"]},
             {"type": "extrusion", "id": "S2", "name": "House", "...": "...", "footprint_m2": 70.0, "faces": ["S2:top", "S2:bottom", "S2:side(0)", "..."]}],
  "materials": [{"id": "lawn", "name": "Lawn"}],
  "assignments": [{"target": {"type": "owner", "owner": "S1"}, "material": "lawn"}],
  "generators": [{"id": "g1", "type": "spline_mass", "name": "Hedge", "layer": "planting", "assets": ["flora:carpinus-hedge"], "count": 26, "truncated": false}],
  "species": [], "lights": [{"id": "li1", "name": "Garden lamp", "...": "..."}], "viewpoints": [{"id": "v1", "...": "..."}],
  "underlays": [{"id": "u1", "name": "Site plan", "file": "site-plan.png", "layer": "underlay", "origin": [-4, -3], "metres_per_pixel": 0.1, "rotation_deg": 0, "size_px": [200, 120], "opacity": 0.6, "elevation": 0.02, "corners": [[-4, -3], [16, -3], [16, 9], [-4, 9]], "bounds": [[-4, -3], [16, 9]], "loaded": true}],
  "terrain": null,
  "history": {"can_undo": true, "can_redo": false}
}
```

Verbosity: `faces: false` leaves out the face names (a large 3dcraft house has hundreds);
`measures: false` skips evaluating generators and measuring shapes; `placements: true` lists
every plant (`asset, position, rotation_y_deg, scale`); `full: true` gives materials, generators
and species with all their fields. A model whose file could not be read has `loaded: false` and
`error`, and is listed under `missingModels`. An underlay is listed with its `corners` (top-left,
top-right, bottom-right, bottom-left, `[x, z]`) and `bounds` on the ground, `loaded` (its image is
held) and, when the file could not be read, `error`. A plan image is a picture under the
drawing: `view.render` shows it (flat, unlit, see-through), it is not picked and not in
`query.quantities`. A wall photo is listed under `wall_photos` with `file`, `size_px`, `target`,
`corners_px`, `region` (null: the whole face), `loaded`, `face_size_m` (`[width, height]` of the
face it is on, in its own frame: `u` to the right seen from outside, `v` up) or `face_error` (no
such face, flat or curved), and `error` when its file could not be read; `view.render` shows it
on the wall, lit like any surface.

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
| `doc_render_preview` | `viewpoint?`, `camera?`, `time?`, `quality?` (also `"photo"`), `samples?`, `aperture?`, `focus_m?`, `shutter_s?`, `vignette?`, `grain?`, `max_side?` (16..2048, default 1024) | an image block (PNG, 4:3) and the render's JSON as text |
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
| `document.measure` (bodies), `view.pick`, `view.replay` | `query.measure`, `query.quantities`, `query.raycast`, `query.ground_*`, `query.sun`, `catalog.search`, `species.list`; replay is a viewpoint tour (`visioncraft-cli replay`) | the domain |
| `file.close`, tabs, several documents; `file.export` | one project per session; pictures only (`view.render`, `visioncraft-cli export`) | the app has one project; the core has no mesh exporter |
| recovery, autosave | none | not built |
| ports 7878–7897, roles `3dcraft window/client`, state folder `3dcraft/` | 7920–7939, `visioncraft window/client`, `visioncraft/` | both apps run side by side |
| `agents.json` switch, consent prompts, `ui.*` methods | not here | the window's part, not built yet |
