# Browser AI agent (WebMCP, experimental)

WebMCP is a W3C Web Machine Learning Community Group proposal that lets a web page hand its
own tools to the browser's AI agent. In the browser version of VisionCraft, the page can
register a curated set of tools with `document.modelContext.registerTool`, so the agent in
Chrome can look at the open project, change it, set the light and weather and build a film
while the person watches. It is **experimental and off by default**, and the browser offers
it only in Chrome builds that have WebMCP (an origin trial, early preview since Chrome 146).

It is the browser's twin of `visioncraft-cli mcp --bridge`, which drives the desktop window
(docs/agent.md). The commands, their parameters and their undo behaviour are the same.

## Turn it on

1. Use a Chrome that has WebMCP, with the flag or the trial:
   - Flag, for trying it yourself: open `chrome://flags`, search for "WebMCP" and set
     *WebMCP for testing* (or the flag the Chrome version you run lists) to Enabled, then
     restart Chrome.
   - Origin trial, for a deployed site: register the site's origin at
     <https://developer.chrome.com/origintrials/> for the WebMCP trial and paste the token into
     the commented `origin-trial` tag in `apps/visioncraft/index.html`. No token is committed.
2. Open VisionCraft with `?webmcp=1` at the end of the address, or tick **Experimental:
   browser AI agent (WebMCP)** in the box at the top right (the same place the desktop
   window has *Allow AI agents*). The tick is remembered; `?webmcp=1` switches it on for that
   visit.
3. The box shows how many tools were registered ("40 tools"), or why not ("this browser
   offers no WebMCP"). Ask the browser's agent to use VisionCraft.

Unticking the box unregisters every tool at once (through the abort signal of
`registerTool`; browsers without it get `unregisterTool`). The code detects both
`document.modelContext` and `navigator.modelContext` (the older name) and does nothing when
neither exists.

## Consent

On the desktop, nothing listens until the person ticks *Allow AI agents*. In the browser the
page's own scripts have always reached the app through `vcRpc`, without a switch, because
they are the page. The WebMCP box is therefore the person's consent for the *browser's agent*:
the tools exist for the browser only while it is ticked, and every step the agent takes then
goes through the same path as a script: it runs on the window's own `Session`, is one undo step
(Ctrl+Z takes it back), and shows as "agent: <command>" beside the box.

## The tools

40, named `visioncraft_<task>`, in groups. The design tools tell the agent to look at the selection (`visioncraft_get_selection`) first when the person says "this" or "here". Each command tool takes exactly the
command's parameters; its JSON Schema is generated from the command's spec
(`CommandDoc::input_schema`), so types, units, defaults, allowed values and required fields are
the ones `visioncraft_list_commands` reports.

| Group | Tools |
| --- | --- |
| Inspect | `visioncraft_project_summary` (document.inspect), `visioncraft_list` (shapes, plantings, layers, materials, viewpoints, lights, films, species or models), `visioncraft_quantities`, `visioncraft_sun_hours`, `visioncraft_light_check`, `visioncraft_plants_for_light`, `visioncraft_window_state` (ui.get: mode, camera, the selection), `visioncraft_get_selection` (just `ui.get`'s `selection`), `visioncraft_select` (ui.select) |
| Design | `visioncraft_shape_add_area`, `_shape_add_extrusion`, `_shape_move_vertex`, `_shape_remove`, `_terrace_add`, `_stairs_add`, `_fence_add`, `_bed_add`, `_furniture_add`, `_furniture_add_set`, `_planting_add_border`, `_planting_place` (planting.add_single), `_material_update` (all with the `visioncraft` prefix) |
| View | `visioncraft_view_set` (sun, season, weather: rain, mist, light shafts, gustiness, white balance, grade), `visioncraft_time_set`, `visioncraft_camera_set` (the window's camera: position, target, field of view), `visioncraft_viewpoint_add` |
| Film | `visioncraft_film_add`, `_film_add_shot`, `_film_update_shot`, `_film_remove`, `_film_orbit`, `_film_flythrough`, `_film_reveal`, `_film_preview`, `_film_render` |

Film targets are `[x, y, z]` or a name (`"Pond"`, `S6`, `g4#12`). `visioncraft_film_orbit` with
`distance: "frame"` frames its target and takes `from` as a compass word; `visioncraft_film_reveal`
with `end: "top_down"` rises to straight above, north up; `visioncraft_film_preview` with
`sheet: true` returns a contact sheet image the agent can look at.
| History | `visioncraft_undo`, `visioncraft_redo` |
| Meta | `visioncraft_run_command {command, params, intent}` (any command), `visioncraft_list_commands` (what exists), `visioncraft_snapshot` |

`visioncraft_snapshot` returns the 3D view as an image block (JPEG by default, at most 1024 px
wide, `format: "png"` and `max_width` change that); `as_text: true` returns a data URL in
text for clients that cannot show images. The agent should frame the view with
`visioncraft_camera_set` first.

The film helpers are core commands, so the CLI and MCP have them too. Each makes a film of
one shot from the existing shot kinds, or with `id` adds the shot to a film you made:

- `film.orbit {center, radius, height, seconds, from_deg, to_deg}`: an `orbit` shot, a full turn
  by default.
- `film.flythrough {points, seconds, height, pitch_deg}`: a `flyover` shot (a drone along
  `points`, looking ahead and down).
- `film.reveal {target, from, to, seconds}`: a `glide` shot from a high, far viewpoint towards
  the target, which it keeps looking at.

All three take `extras` for further shot fields (`ease`, `time_of_day: [from, to]`, `rain`,
`mist`, `shafts`...).

## What the person has selected

The person clicks something and says "make this a birch". `visioncraft_get_selection` (or
`selection` in `visioncraft_window_state`) says what "this" is, in any mode:

```json
{"items": [{"kind": "placement", "id": "g3", "name": "Silver birch", "sub": {"index": 2},
            "summary": "Silver birch (flora:betula-pendula), plant 3 of 5 in planting `g3` (Grove), about 12 m tall when grown (scale 1), turned 40 deg, at [8.2, 0, 7.1]"}],
 "point": [8.2, 0.0, 7.1],
 "revision": 14}
```

- `kind` is `shape` (area, terrace, stairs, fence, bed, furniture, block or wall: the summary
  gives the spec, size and materials; `sub.vertex` and `sub.edge` when a corner or an edge is
  picked), `placement` (one plant: `id` is its planting, `sub.index` its number in it), `planting`
  (a whole planting), `model`, `lamp` (a light) or `underlay` (a plan image). The window
  selects one thing at a time, so `items` has at most one entry.
- `point` is the last ground or surface point the person clicked, in metres. A click on empty
  ground records it without changing `items`, so "put a tree here" has its place.
- `revision` bumps whenever the items or the point change. There is no blocking wait (a request
  is answered within one frame, so the window never stalls for an agent): poll
  `visioncraft_get_selection` and compare `revision` to see a new selection.

`visioncraft_select {items: [{kind, id, sub?}]}` does the reverse: it selects as a click would
(Draw with its handles for a shape, Objects for a planting, plant or lamp) so the person sees
what the agent means. `items: null` or `[]` clears; an unknown id is an error. The old
`{planting, model}` form still works, and `ui.get` still reports `selected: {planting, model}`.

> **Person:** (clicks a tree in the grove) Make this a birch.
>
> **Agent:** `visioncraft_get_selection` returns a `placement` of planting `g3`, `sub.index: 2`,
> a pedunculate oak. Then `visioncraft_run_command {command: "planting.replace_placement",
> params: {planting: "g3", index: 2, asset: "flora:betula-pendula"}}`. The planting stays one
> planting with its recipe (palette, density, seed, layers); the swap is stored as an edit on
> plant 2 and applied after the planting is evaluated, so the person's selection stays valid.
> `planting.remove_placement` takes one plant out the same way, `scale` and `offset` of
> `planting.replace_placement` resize and move it, and `planting.reset_placements` takes the
> edits off again. Ctrl+Z undoes each.

A plant keeps its number as long as the planting's recipe is unchanged (a planting is
deterministic). When a change to the recipe (`planting.update`, a brush stroke, a preset) leaves
fewer plants, the edits of plants that no longer exist are dropped and the answer carries a
`warnings` entry that names them. A planting on a surface (`SurfaceFill`) cannot be edited
plant by plant.

## Safety

- **Confirmation.** Destructive calls ask the person first, in a small window over the 3D
  view (Allow or Deny; no answer in two minutes is a Deny). Destructive means the commands
  `remove` and `clear` of any kind (`shape.remove`, `planting.remove`, `film.remove`,
  `fence.clear`...), `file.new`, `file.open` and `project.load_example` (replacing the whole
  project). This holds for `visioncraft_run_command` as well as for the dedicated tools, which
  carry `consequentialHint`. The WebMCP API of today has no `requestUserInteraction`, so the
  question is VisionCraft's own egui window. A denied call fails with a message and changes
  nothing.
- **Undo.** Agent commands are ordinary history steps; `visioncraft_undo` and Ctrl+Z take
  them back. The last agent command is shown beside the box.
- **One render at a time.** `film.render` refuses to start while another is running. In the
  browser pictures cannot be read back from the GPU, so `film.render` reports that; check a film
  with `visioncraft_film_preview` and `visioncraft_snapshot`, and render it with
  `visioncraft-cli film` on a computer.
- Tools return text or images; nothing is sent anywhere but to the browser's own agent.

## An example conversation: a golden-hour flythrough

> **Person:** Make a 12 second flythrough of the garden at golden hour, from the gate to the terrace.
>
> **Agent:** `visioncraft_list {what: "shapes"}` and `visioncraft_window_state` to see the garden.
> Then `visioncraft_sun_hours` to confirm the terrace gets evening sun, and `visioncraft_view_set
> {mist: 0.15, shafts: 0.6}` for a warm, hazy evening.
>
> **Agent:** `visioncraft_film_flythrough {points: [[-8, 14], [-2, 8], [4, 2], [9, -3]],
> height: 6, seconds: 12, name: "Golden hour", extras: {time_of_day: [18, 19.5], ease: "in_out"}}`
> returns `{id: "f1", ...}`.
>
> **Agent:** `visioncraft_film_preview {id: "f1", t: 6}` for the camera half way, then
> `visioncraft_camera_set` to that position and `visioncraft_snapshot` to look at it.
> The view is too high, so `visioncraft_film_update_shot {id: "f1", index: 0, shot: {height_m: 4}}`.
>
> **Agent:** It plays 12 seconds, 300 frames at 25 fps. Render it on a computer with
> `visioncraft-cli film`; Ctrl+Z (or `visioncraft_undo`) takes any of these steps back.

## Limits

- Chrome only for now, and only where WebMCP exists; the API is moving (names and the shape
  of results change between Chrome versions). The glue is one small file,
  `apps/visioncraft/webmcp.js`, tested with mocks (`node --test apps/visioncraft/webmcp.test.mjs`).
- `visioncraft_snapshot` reads the canvas in the frame that drew it; a GPU-composited canvas
  may come back blank in some browsers, which is reported as an error, not a black image.
- Results follow the tool format of the preview builds: `{content: [{type: "text" | "image", ...}]}`.
