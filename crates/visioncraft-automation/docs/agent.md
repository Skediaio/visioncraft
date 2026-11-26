# Driving VisionCraft as an agent

VisionCraft shows a house and garden as they will look: models (from 3DCraft or OBJ), shapes you
draw (lawns, terraces, house blocks, walls), materials, procedural plants, lights and viewpoints,
under the sun of a real site and date. Everything is a command with an id and JSON params, the
same over MCP (`visioncraft-cli mcp`), JSON lines (`visioncraft-cli serve`) and the command line
(`visioncraft-cli run`), in the manner of 3DCraft. The protocol is in
[`control-protocol.md`](control-protocol.md).

## Conventions

- **Metres, Y up, +X east, -Z north** (+Z is south; `site.set north_deg` turns true north away from -Z by that many degrees clockwise seen from above, and every compass bearing is then a true bearing). A house "north of the lawn" has smaller z.
- **Outlines and paths are `[[x, z], ...]`** on the ground; positions are `[x, y, z]`.
- **Ids** are a prefix and the lowest free number: shapes `S1`, models `m1`, generators `g1`,
  lights `li1`, viewpoints `v1`, materials `mat1`, custom species `sp1`. Every `add` returns
  `{id}`. Renders take a viewpoint's id or its name.
- **Face names**: a drawn area has `S1:top`; an extrusion `S2:top`, `S2:bottom`, `S2:side(0)` …
  (side n runs from outline point n to n+1), an open one also `S2:start` and `S2:end`. A 3DCraft
  model keeps its persistent names, `F2:side(e7)`, `F2:endCap(e5,e6,e7,e8)`.
  `document.inspect` lists them under `faces`.
- **Materials**: built in `plaster`, `brick`, `concrete`, `timber`, `roof`, `glass`, `water`,
  `lawn`, `gravel`, `paving`, `metal`, `soil`, `sand`. A face assignment beats a body one, which beats
  the owner's.
- **Asset ids** in a palette: `flora:<species>` for a built-in procedural plant
  (`species.list {category}`), a custom species of the project (`sp1`), or a catalogue asset id
  (`catalog.search {query}`; garden lights such as `vc-path-light` have no `flora:` prefix).
  Every `planting.*` command refuses an id that resolves to nothing, naming it and the five
  closest valid ids; a saved file that already has one still opens (a grey cube is drawn).
- **Sun**: `site.set {latitude, longitude, utc_offset_hours}`, `time.set {year?, month?, day?,
  hour?}` in local time; the offset is fixed, so give summer time yourself (Belgium in June: 2).
- **Selection.** `ui.get` → `selection` is what the person has picked, in any mode: `{items:
  [{kind, id, name, summary, sub?}], point?: [x, y, z], revision}`. `kind` is shape, placement,
  planting, model, lamp or underlay; a single tree of a planting is `placement` (`id` the
  planting, `sub.index` the plant's number). `point` is the last ground or surface point clicked,
  so "put a tree here" has a position. When the person says "this", "here" or "the selected
  one", read the selection first; "these" and "all of them" mean every entry of `items` (the
  person adds one with Shift+click, or takes one out the same way), so act on all of them, not
  the first. `ui.select {items: [{kind, id, sub?}, ...]}` selects as a click would, several at
  once (`null` or `[]` clears; the first item's mode opens). `ui.edit_selection {action:
  "delete" | "move" | "material", offset?: [x, z] | [x, y, z], material?}` edits everything
  selected as one undo step (one `ui.edit_selection`, not a loop of commands, when the person
  says "delete these" or "move them 2 m east"); what it does not apply to comes back under
  `skipped`.
- **Loading.** `ui.load` is the window's load timeline (docs/performance.md): when the first
  frame reached the screen, when plants, library files and the sky bakes were done, the stages
  by time, and per placed catalogue asset which level of detail it shows and why (`lods`). Read
  it when the person says loading is slow or a model looks coarse up close.
- **Editing one plant.** `planting.replace_placement {planting, index, asset?, scale?, offset?}`
  swaps, resizes or moves one plant; `planting.remove_placement {planting, index}` takes it out;
  `planting.reset_placements {planting, index?}` restores one or all. The planting stays one
  planting with its recipe: the edit is stored on the planting (`overrides`, by plant number) and
  applied after it evaluates, so quantities, renders and checks all see it. A plant keeps its
  number while the recipe is unchanged; when `planting.update`, a brush stroke or a preset leaves
  fewer plants, the edits of plants that no longer exist are dropped and the answer carries
  `warnings`. A single-plant planting is just changed (or removed); a planting on a surface
  (`surface_fill`) cannot be edited plant by plant.
- **Names as targets.** Wherever a point is taken (shot fields, `viewpoint.*`, the `camera` of
  `view.render`, `film.orbit`, `film.flythrough`, `film.reveal`), a shape id (`S6`), a planting
  or plant (`g4`, `g4#12`), a model, a lamp, a viewpoint or a name (`"Pond"`) works as well as
  `[x, y, z]`; the answer's `resolved` shows what each name became.

## The loop: inspect, plan, batch, render, verify

1. **Inspect.** `doc_inspect {"faces": false}`; `command_list {"brief": true}`, then
   `command_list {"id": ...}` for a command's parameters, units, defaults and a tested example.
2. **Plan in numbers**: outlines, heights, spacings, positions, before the first command.
3. **Batch.** `command_batch` runs the steps in order, as in 3DCraft: it stops at the first error
   (`stop_on_error`, default true), the steps before it stay applied, each is its own undo step.
   With `"atomic": true` it is all or nothing instead: the first failure takes back every step
   (`rolledBack: true`) and a batch that succeeds is one undo step. Give an `intent` (why, in a
   sentence); `agent.log` keeps it.
4. **Render.** `doc_render_preview {"viewpoint": ...}`: a PNG and what was drawn (camera, sun,
   `plantsPending`: 0 means every plant is drawn). Its `time` changes the picture only.
5. **Verify** against the plan: `doc_inspect` (`area_m2`, `footprint_m2`, plant `count`),
   `query.measure`, `query.plants_for_light {point | outline | hours}` (species ranked by the light a spot gets), `query.light_check` (every placed plant against its light range), `query.quantities {ids?, layer?}` (areas, lengths, volumes and plant counts per object, per material and per asset; linked models too, per body with surface and volume (closed bodies), scale applied, in `models`; plants are counted with the renderer's building rule, `counted_with: "scene"`), `query.raycast`, `query.sun_hours {point | outline, season | date, cell_m?, years?, plants?}` (hours of direct sun at a spot or as a grid over an area, with shadow from models, shapes, terrain and plant crowns; `view.set_sun_map` draws the same grid on the ground, recipe 26), `query.sky_view {point, normal?, years?}` (how open a spot is: the sky view factor 0..1, crowns letting sky through by their leaf density; recipe 43), `query.ground_height`, `query.ground_length`, `query.sun`, and `query.clips {model}` (the animation clips of a model that moves in parts: name, length and channel count; recipe 14, which also plays them with `model.play_clip`). A film of camera moves is `film.*` (recipe 27); `film.preview` reads it without drawing.
   Save with `command_run file.save {"path": ...}`.

## Recipes

Each recipe is the arguments of one `command_batch` on a new project, and is run by the tests
(`render` steps where there is a GPU). The model file of recipe 2 lies beside the project.

### 1. A plot and a house block

```json recipe
{"intent": "the plot and the house", "steps": [
  {"id": "shape.add_area", "params": {"name": "Plot", "outline": [[-10, -15], [10, -15], [10, 15], [-10, 15]], "material": "lawn"}},
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[-6, -14], [6, -14], [6, -5], [-6, -5]], "height": 6.5, "material": "plaster"}},
  {"id": "query.measure", "params": {"kind": "area", "outline": [[-10, -15], [10, -15], [10, 15], [-10, 15]]}}
]}
```

`S1` is the plot (600 m²), `S2` the house (108 m² footprint) at the north end.

### 2. Load a .3dcraft model and scale it

```json recipe
{"intent": "the client's design, at its size", "steps": [
  {"id": "model.add", "params": {"name": "Plate", "kind": "3dcraft", "source": "plate.3dcraft"}},
  {"id": "model.set_transform", "params": {"id": "m1", "translation": [2, 0, -3], "rotation_y_deg": 30, "scale": 100}},
  {"id": "document.inspect", "params": {"faces": false, "measures": false}}
]}
```

A 3DCraft file is in millimetres made metres (a 40 mm plate is 0.04 m); `scale` multiplies it.
The model's `bbox` in `document.inspect` is in world space, after the transform.

### 3. Materials by face

```json recipe
{"steps": [
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "height": 6}},
  {"id": "material.assign", "params": {"target": {"type": "owner", "owner": "S1"}, "material": "brick"}},
  {"id": "material.assign", "params": {"target": {"type": "face", "owner": "S1", "face": "S1:top"}, "material": "roof"}},
  {"id": "material.assign", "params": {"target": {"type": "face", "owner": "S1", "face": "S1:side(2)"}, "material": "glass"}}
]}
```

`S1:side(2)` runs from outline point 2 to point 3: here the south facade at z = 8.

A lawn can be mown in stripes: `material.update {"id": "lawn", "kind": {"type": "lawn3d",
"density_per_m2": 2000, "blade_height_m": 0.04, "seed": 1, "stripes_m": 0.9, "stripes_deg": 0}}`
(bands 0.3 to 5 m wide, their direction, a true bearing clockwise from north; `stripes_m: 0` for none). The bands
read lighter and darker by the side they are seen from, as mown grass does.

### 4. A hedge along a boundary

```json recipe
{"intent": "a beech hedge round the garden", "steps": [
  {"id": "shape.add_area", "params": {"name": "Garden", "outline": [[0, 0], [20, 0], [20, 12], [0, 12]], "material": "lawn"}},
  {"id": "species.list", "params": {"category": "hedge", "filter": "fagus"}},
  {"id": "planting.add_spline", "params": {"name": "Beech hedge", "asset_palette": [{"asset": "flora:fagus-hedge", "weight": 1}],
    "points": [[0, 0], [20, 0], [20, 12], [0, 12]], "closed": true, "spacing": {"every": 0.6}, "offset_m": 0.4}}
]}
```

`spacing` is `{"every": metres}` or `{"count": n}`; `offset_m` moves the row to the right of the
direction of travel.

### 5. A mixed woodland brushed with a preset

```json recipe
{"steps": [
  {"id": "planting.list_presets", "params": {}},
  {"id": "planting.add_brush", "params": {"name": "Woodland", "asset_palette": [{"asset": "flora:quercus-robur", "weight": 1}], "density_per_m2": 0.02,
    "strokes": [{"points": [[-20, -20], [20, -20], [20, -5]], "radius_m": 8, "erase": false}]}},
  {"id": "planting.apply_preset", "params": {"id": "g1", "preset": "broadleaf-woodland"}}
]}
```

The preset's layers (canopy, understorey, floor) replace the brush's palette; `document.inspect`
gives the plant `count`.

### 6. A perennial border filling a face

```json recipe
{"steps": [
  {"id": "shape.add_area", "params": {"name": "Border", "outline": [[0, 0], [12, 0], [12, 2], [0, 2]], "elevation": 0.05, "material": "soil"}},
  {"id": "planting.add_surface_fill", "params": {"name": "Border planting", "owner": "S1", "faces": ["S1:top"], "density_per_m2": 4,
    "asset_palette": [{"asset": "flora:nepeta-faassenii", "weight": 2}, {"asset": "flora:geranium-rozanne", "weight": 2}, {"asset": "flora:echinacea-purpurea", "weight": 1}]}}
]}
```

### 7. A pond: carve the terrain, add water

```json recipe
{"intent": "a pond in the lawn", "steps": [
  {"id": "terrain.create", "params": {"origin": [-20, -20], "width_m": 40, "depth_m": 40, "base_material": "lawn"}},
  {"id": "terrain.carve", "params": {"outline": [[-4, -3], [4, -3], [5, 2], [-3, 4]], "depth_m": 0.8, "bank_m": 1.5}},
  {"id": "shape.add_area", "params": {"name": "Pond water", "outline": [[-4, -3], [4, -3], [5, 2], [-3, 4]], "elevation": -0.3, "drape": false, "material": "water"}},
  {"id": "query.ground_height", "params": {"x": 0, "z": 0}}
]}
```

The water surface sits 0.3 m below the lawn, 0.5 m above the pond bed. It is level
(`drape: false`): an area lies on the terrain by default, and a draped water surface 0.3 m under
the carved bed is no water at all. A carve holds no water by itself: its result's `note` says
so when no water area lies over it. A lawn drawn over the pond is cut out exactly along the
water's edge; on a sloped bank it keeps its grass down to the water.

Areas can have a soft edge: `edge_blend_m` (metres, 0..10, default 0) on `shape.add_area`, or
`shape.set_edge_blend {id, edge_blend_m}` afterwards. Across that band along its outline the area
dithers into whatever lies under it instead of ending on a line, and a lawn's grass carries on
into the band, thinning towards the area's inside. `planting.add_floor` sets 1.5 m by default
(`edge_blend_m` there too; 0 gives the hard edge back). Only the band's triangles are drawn as
cutouts. A render's result (`view.render`, `visioncraft-cli render`) carries `stats`: the frame's
`triangles`, `draws` and `gpu_ms` (null without GPU timestamp queries).

### 8. Lamps, and the garden at dusk

Dusk is the last minutes before sunset: `query.sun` gives `sunset_hour` for the date and site.

```json recipe
{"steps": [
  {"id": "shape.add_area", "params": {"name": "Terrace", "outline": [[-4, 0], [4, 0], [4, 4], [-4, 4]], "material": "paving"}},
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[-5, -8], [5, -8], [5, 0], [-5, 0]], "height": 6, "material": "brick"}},
  {"id": "light.add", "params": {"name": "Wall lamp west", "position": [-3, 2.2, 0.2], "lumens": 1500}},
  {"id": "light.add", "params": {"name": "Wall lamp east", "position": [3, 2.2, 0.2], "lumens": 1500, "color_temperature_k": 3000}},
  {"id": "site.set", "params": {"latitude": 50.85, "longitude": 4.35, "utc_offset_hours": 2}},
  {"id": "time.set", "params": {"month": 9, "day": 21, "hour": 19.6}},
  {"id": "query.sun", "params": {}},
  {"id": "viewpoint.add", "params": {"name": "Terrace", "position": [6, 1.7, 10], "target": [0, 1.5, 0]}},
  {"id": "view.render", "params": {"viewpoint": "Terrace", "width": 960, "height": 540, "path": "dusk.png"}}
]}
```

### 9. Viewpoints and stills

```json recipe
{"steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[-10, -10], [10, -10], [10, 10], [-10, 10]], "material": "lawn"}},
  {"id": "shape.add_extrusion", "params": {"name": "Shed", "outline": [[4, -8], [8, -8], [8, -5], [4, -5]], "height": 2.5, "material": "timber"}},
  {"id": "viewpoint.add", "params": {"name": "Gate", "position": [0, 1.7, 14], "target": [4, 1, -6]}},
  {"id": "viewpoint.add", "params": {"name": "From above", "position": [0, 30, 1], "target": [0, 0, 0], "fov_y_deg": 50}},
  {"id": "view.render", "params": {"viewpoint": "v1", "width": 1280, "height": 720, "path": "gate.png"}},
  {"id": "view.render", "params": {"viewpoint": "v2", "width": 1280, "height": 720, "path": "above.png"}},
  {"id": "file.save", "params": {"path": "stills.visioncraft"}}
]}
```

From the command line, every viewpoint at the export presets:
`visioncraft-cli export stills.visioncraft --out stills --preset HD --preset Square`.

### 10. A sun study across a day

```json recipe
{"intent": "where the shade falls on 21 June", "steps": [
  {"id": "site.set", "params": {"latitude": 51.05, "longitude": 3.72, "utc_offset_hours": 2}},
  {"id": "time.set", "params": {"year": 2026, "month": 6, "day": 21, "hour": 9}},
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[-10, -10], [10, -10], [10, 10], [-10, 10]], "material": "lawn"}},
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[-5, -8], [5, -8], [5, -2], [-5, -2]], "height": 7, "material": "brick"}},
  {"id": "query.sun", "params": {"time": {"hour": 9}}},
  {"id": "query.sun", "params": {"time": {"hour": 13}}},
  {"id": "query.sun", "params": {"time": {"hour": 18}}},
  {"id": "view.render", "params": {"camera": {"position": [0, 35, 12], "target": [0, 0, 0]}, "time": {"hour": 9}, "width": 800, "height": 600, "path": "sun-09.png"}},
  {"id": "view.render", "params": {"camera": {"position": [0, 35, 12], "target": [0, 0, 0]}, "time": {"hour": 13}, "width": 800, "height": 600, "path": "sun-13.png"}},
  {"id": "view.render", "params": {"camera": {"position": [0, 35, 12], "target": [0, 0, 0]}, "time": {"hour": 18}, "width": 800, "height": 600, "path": "sun-18.png"}}
]}
```

`view.render`'s `time` changes the picture only; the project keeps 9:00.

### 11. A house with a roof, windows and a door

```json recipe
{"intent": "a two-storey house", "steps": [
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "height": 6, "material": "plaster", "roof": {"kind": "gable", "pitch_deg": 38, "overhang_m": 0.45}}},
  {"id": "shape.set_plinth", "params": {"id": "S1", "height_m": 0.35}},
  {"id": "shape.add_opening", "params": {"id": "S1", "side": 0, "offset_m": 2.5, "width_m": 2.0, "height_m": 1.6, "sill_m": 0.6, "frame": "split"}},
  {"id": "shape.add_opening", "params": {"id": "S1", "side": 0, "offset_m": 5.0, "kind": "door", "width_m": 1.0, "height_m": 2.2}},
  {"id": "shape.add_opening", "params": {"id": "S1", "side": 0, "offset_m": 7.5, "width_m": 1.2, "height_m": 1.3, "sill_m": 3.8}},
  {"id": "shape.set_roof", "params": {"id": "S1", "kind": "hip", "pitch_deg": 30}},
  {"id": "shape.set_lit_rooms", "params": {"id": "S1", "share": 0.6}},
  {"id": "document.inspect", "params": {"faces": false}}
]}
```

Openings are numbered `o1`, `o2` per block (`shape.update_opening`, `shape.remove_opening`).
The faces are `S1:roof`, `S1:glass`, `S1:frame`, `S1:door`, `S1:sill`, `S1:interior` and so
on, for `material.assign`. Lit rooms show when the lamps are on.

#### Neighbours and other surroundings

Mark a layer as surroundings, then draw the neighbours on it: `layer.add {name: "Surroundings",
context: true}` (or `layer.set_context {id, context: true}` on an existing layer), then closed
`shape.add_extrusion` blocks with a `height` on that layer (or an imported model assigned to it).
Context items cast and receive shadow, so `query.sun_hours` and renders see them, but they draw in
one neutral grey massing look whatever their materials, and `query.quantities` leaves them out
unless asked by `ids` or `layer`. Plantings on the layer, a neighbour's trees, draw normally.

### 12. A pond with a stone coping

```json recipe
{"intent": "a formal square pond", "steps": [
  {"id": "terrain.create", "params": {"origin": [-10, -10], "width_m": 20, "depth_m": 20, "cell_m": 0.5, "base_material": "lawn"}},
  {"id": "terrain.carve", "params": {"outline": [[-2.51, -2.01], [2.51, -2.01], [2.51, 2.01], [-2.51, 2.01]], "depth_m": 0.8, "bank_m": 0, "bed_material": "soil"}},
  {"id": "shape.add_area", "params": {"name": "Pond", "outline": [[-2.5, -2], [2.5, -2], [2.5, 2], [-2.5, 2]], "elevation": -0.05, "drape": false, "material": "water"}},
  {"id": "shape.add_extrusion", "params": {"name": "Pond edge", "outline": [[0, -2.22], [2.72, -2.22], [2.72, 2.22], [-2.72, 2.22], [-2.72, -2.22], [-0.01, -2.22]], "closed": false, "thickness": 0.5, "height": 0.6, "material": "paving"}},
  {"id": "query.ground_height", "params": {"x": 0, "z": 0}}
]}
```

The water is the outline as drawn, the carve a centimetre wider with a straight wall, the coping
an open band (a closed outline would be a solid block) at least one ground cell wide, standing on
the lowest ground under it: give it the height that brings its top to the lawn plus 12 cm.
`terrain.add_pond {"outline": ..., "depth_m": 0.8}` does all this in one step (the Pond tool in
the Landscape panel runs it).

### 13. The picture's look and air

```json recipe
{"steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[-10, -10], [10, -10], [10, 10], [-10, 10]], "material": "lawn"}},
  {"id": "view.set", "params": {"look": "vivid", "haze": 1.5}},
  {"id": "view.set", "params": {"clouds": {"kind": "overcast", "cover": 1.0}}},
  {"id": "view.set", "params": {"look": "natural", "clouds": {"kind": "cumulus", "cover": 0.5}}},
  {"id": "document.inspect", "params": {"faces": false}}
]}
```

`look` is `neutral` (plain tone map), `natural` (default) or `vivid`. `clouds` is a layer over
the analytic sky (an HDRI brings its own): `clear`, `cumulus` (light fair-weather clouds, the
default at cover 0.35) or `overcast` (soft shadows, little direct sun); they drift with the wind
and show in reflections and in the ambient light. The rendering quality and
the texture resolution belong to the window, not the project: set them over the control channel
with `ui.set {"quality": "auto" | "low" | "medium" | "high" | "ultra", "textures": "auto" | "1k"
| "2k" | "4k"}`; `view.render` takes its own `quality`. For a final picture use `view.render
{"quality": "photo"}`: many frames averaged at ULTRA give soft shadows with real penumbrae,
smooth edges and leaves without shimmer (seconds to minutes); recipe 41 has the lens.

Lamps and lit rooms follow the sun: on after sunset at the time shown, off before. `view.set
{"lights": true}` or `false` fixes them for the project (the window's Lights button does the same),
`null` returns to the sun's rule; every picture, from the window, the CLI or `view.render`, uses it.

Wind is in real units: `view.set {"wind_speed_mps": 6.7, "wind_gustiness": 0.5,
"wind_direction_deg": 90}`, or a Beaufort step as a preset, `{"wind_beaufort": 4}` or
`{"wind_beaufort": "gale"}` (0 calm to 10 storm; v = 0.836 B^1.5 m/s). At 0 the air is perfectly
still; plants lean with the drag (growing with the speed squared, saturating as crowns streamline),
leaves flutter faster as the wind rises, and each plant sways at its own natural frequency (tall
trees slowly, shrubs and grasses fast). The old relative `wind_strength` (1 = 6.5 m/s) is still
read from older projects and accepted by `view.set`, as a deprecated alias. `wind_gustiness` is a
slow swell of the whole wind; gusts that roll over lawns and through crowns are `gustiness`
(recipe 39). To see motion in
pictures, `view.render` takes `animation_s` (the animation clock in seconds; the CLI's
`--animation-s`).

Ground clutter is on by default: fallen leaves under deciduous trees (their own leaf shapes, most
in October and November, a few in summer), small stones and twigs on soil and gravel, and grass
blades hanging over the lawn's edge onto borders and paving. All of it fades out within about
25 m and is left out at LOW quality. `view.set {"clutter": false}` switches it off for the
project, `true` back on.

### 14. Your own models

```json recipe
{"steps": [
  {"id": "object.add", "params": {"name": "Garden bench", "file": "objects/bench.glb", "size_m": [1.8, 0.9, 0.6]}},
  {"id": "object.list", "params": {}},
  {"id": "planting.add_single", "params": {"name": "Bench", "asset": "user:uo1", "position": [2, 0, 3]}}
]}
```

`object.add` records the file (relative to the project) and its size; it is placed like any
catalogue asset as `user:<id>`. `object.remove` is refused while a planting uses it.

A model with animations (glTF node animation: parts that turn, slide or tilt on keyframes) lists
its clips with `query.clips {"model": "user:uo1"}`: `{clips: [{name, duration_s, channels}],
warnings}`, the channel count of each and what was left out of the file's animations and why (a
malformed channel: the model still loads); a model that stands still answers `{"clips": [],
"warnings": []}`. A catalogue asset is asked by its id, and its clips are those of its first
level of detail. On the command line the file is read from the project's folder or the library
folder (`--assets`); in a window a library model's clips are known once it is loaded.

A clip plays on the placed objects, not on the model: `model.play_clip {"object": "g4", "clip":
"fire", "start_s": 12.5}` (or `"loop": true`). `object` is a planting (`g4`: every object it
places) or one of its objects (`g4#12`, counted from 0 as `planting.replace_placement` counts
them); `clip` is a clip's name or its index from 0 (an exact name first; of two clips with the
same name the lowest index; a name that is not there is an error that lists them). The clip is
sampled at the session's clock from `start_s` (the clock now: a window's frame time, a film's
time in `film.render`, the `animation_s` of the last picture on the command line), so the same
time always gives the same pose. The same clip on the same object starts again; several clips
may play at once, and where two move the same property the one started later wins; a clip that
ends holds its last pose until `model.stop_clip {"object": "g4", "clip"?: ...}` (every clip
without `clip`; a planting stops the clips on each of its objects too, and one object of a
planting that plays as a whole cannot be stopped alone: stop the planting). `query.part_pose {"object": "g4", "t"?: ...}` reads the pose back: per object
the matrix in model space (column-major, 16 numbers) of every moving part at time `t`, the
clips on it composed as they are drawn. `effect.flash {"position": [x, y, z], "color": [r, g,
b], "intensity": 2000, "range_m": 4, "size_m"?: 0.15, "frames"?: 2, "seconds"?, "start_s"?}` is a
short-lived flash there: a point light (lumens, as a lamp's) and a glowing quad facing the
camera, for `frames` frames of the rate in force (a film's fps, so the same frames show it every
time; 60 a second in a still) or for `seconds`, from `start_s` (the clock now); at most 16 flash
at once (whose times overlap the new one's; `frames` are counted at 60 a second, or the rate
in force if faster), and over it the oldest of those is dropped:
flashes one after another are not counted together. Flashes with `start_s` are kept, at most 256:
over it the one that ends first is dropped, and the answer's `note` says so. A flash without `start_s` starts at the clock now and is
forgotten once over; one with `start_s` is kept, so every render of a film shows it. In a
window the clock is the window's own frame time (seconds since it opened), not a film's: a clip
or a flash meant for `film.render` takes its `start_s` on the film's time explicitly (`"start_s":
1.5` is 1.5 s into the film), or it plays at the window's clock and the film does not see it.
Playing and flashing are session state: not saved with the project, not an undo step, and
gone with it: `file.new` and `file.open` start with nothing playing and no flash.

### 15. The example garden

```json recipe
{"steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "document.inspect", "params": {"faces": false}},
  {"id": "view.render", "params": {"viewpoint": "v1", "width": 1280, "height": 720, "path": "example.png"}}
]}
```

It replaces the project (undoable) with a 20 by 30 m plot built from commands: house, terrace,
lawn, border, path, pond, hedge, trees, lights and six viewpoints. `{"name": "estate"}` is the large one:
a 100 by 60 m plot round an imported 3DCraft villa (its model travels inside the program as
`example:villa.3dcraft`), two lawns over 1,000 m2, a brushed woodland, a border, a pond carved in
the terrain, path lights, a walnut on a hidden layer and five viewpoints. `{"name":
"beach-house"}` is a furnished single-storey house imported from 3DCraft
(`example:beach-house.3dcraft`, one body per element group, from
`examples/models/beach-house/`), with library materials, catalogue furniture, downlights in
every room and ten viewpoints inside and out. For a furnished plan from above, take its roof
off (recipe 19).

### 16. How the garden grows

```json recipe
{"intent": "the garden after ten years", "steps": [
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "height": 6}},
  {"id": "planting.add_single", "params": {"name": "Field maple", "asset": "flora:acer-campestre", "position": [5, 0, 10.5]}},
  {"id": "planting.update", "params": {"id": "g1", "planted_age_years": 6}},
  {"id": "planting.growth", "params": {"id": "g1", "years": 20}},
  {"id": "planting.growth_conflicts", "params": {"years": 30}},
  {"id": "view.set", "params": {"years": 10}},
  {"id": "view.render", "params": {"camera": {"position": [5, 1.7, 25], "target": [5, 3, 8]}, "years": 20, "width": 960, "height": 540, "path": "maple-year-20.png"}}
]}
```

`view.set {years}` shows every plant that many years after planting (null: all full grown, as
projects were before); `view.render` and viewpoints can take their own `years`. Each planting
starts at its `planted_age_years` (default per species: a nursery tree 8, hedge whips 2,
perennials 1). The growth data are estimates, one table per species (`planting.growth` says
which). `planting.canopy_cover {id, years}` (read-only) reports how much of a scatter's or
brush's area its crowns close from above (0..1); woodland presets should reach 0.8 to 0.95 fully
grown. `visioncraft-cli export FILE --out DIR --years 0 --years 5 --years 10` renders a growth
series of every viewpoint.

### 17. The four seasons, and snow

```json recipe
{"intent": "the garden through the year", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[-10, -10], [10, -10], [10, 10], [-10, 10]], "material": "lawn"}},
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[-5, -9], [5, -9], [5, -3], [-5, -3]], "height": 6, "material": "plaster", "roof": {"kind": "gable", "pitch_deg": 38}}},
  {"id": "planting.add_single", "params": {"name": "Field maple", "asset": "flora:acer-campestre", "position": [4, 0, 4]}},
  {"id": "planting.add_spline", "params": {"name": "Yew hedge", "asset_palette": [{"asset": "flora:taxus-hedge", "weight": 1}], "points": [[-10, 10], [10, 10]], "spacing": {"every": 0.6}}},
  {"id": "view.set", "params": {"season": "autumn"}},
  {"id": "viewpoint.add", "params": {"name": "Lawn in spring", "position": [0, 1.7, 18], "target": [0, 1.5, 0], "season": "spring", "time": {"hour": 11}}},
  {"id": "view.set", "params": {"season": "winter", "snow": 0.7}},
  {"id": "document.inspect", "params": {"faces": false}},
  {"id": "view.render", "params": {"camera": {"position": [0, 1.7, 18], "target": [0, 1.5, 0]}, "quality": "high", "width": 960, "height": 540, "path": "snowy.png"}},
  {"id": "view.render", "params": {"viewpoint": "Lawn in spring", "snow": 0, "width": 960, "height": 540, "path": "spring.png"}},
  {"id": "view.set", "params": {"season": "summer", "snow": 0}}
]}
```

`view.set {season}` (`spring`, `summer`, `autumn`, `winter`) shows the plants as they are then
and moves the project's date to the season's day at the site (20 April, 21 June, 20 October,
15 January in the north, six months on in the south), keeping the hour; `document.inspect` reports
`season`. Plants have one state per season: an exact date (`time.set`, for a sun study) shows the
plants of its season. A viewpoint keeps its own season (`season` on `viewpoint.add` and
`viewpoint.update`). `view.set {snow: 0..1}` lays snow on what faces up and lies open to the sky:
ground, lawns, roofs, terraces, hedge tops, evergreen crowns, branches, wall tops and sills, not
under roofs or dense evergreens; from about 0.55 it buries the lawn's blades, from 0.3 the fallen
leaves, and from about 0.5 the pond freezes. About 0.7 is a snowy winter (the window's Snowy).
Snow lies only in winter: `view.set {snow}` in another season is refused (give `season:
"winter"` with it), and a date moved out of winter keeps the value but draws no snow.

A viewpoint shows the project's season and time unless it keeps its own (`time` or `season` on
`viewpoint.add`; the window's "Keep the time of this view"): choosing a season changes every
other view. `view.render` takes `season` and `snow` for one picture (snow alone brings winter),
and so does the CLI: `visioncraft-cli render garden.visioncraft --viewpoint Pond --season winter
--snow 0.7 --out snowy.png`.

### 18. The sea, a beach, and the coast example

```json recipe
{"intent": "a house above a beach, the sea to the horizon", "steps": [
  {"id": "terrain.create", "params": {"origin": [-60, -40], "width_m": 120, "depth_m": 120, "cell_m": 1, "base_material": "lawn"}},
  {"id": "water.set_sea", "params": {"level": -2, "sea_state": 3, "coast": [[-70, 35], [70, 35]]}},
  {"id": "terrain.shore", "params": {"points": [[-70, 10], [70, 10]], "top_m": 2, "bank_m": 5, "berm_m": 0.8, "beach_m": 20, "shelf_m": 40, "depth_m": 5, "material": "sand"}},
  {"id": "view.set", "params": {"ground": {"height": 0, "color": [0.2, 0.18, 0.14]}}},
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[-6, -8], [6, -8], [6, 0], [-6, 0]], "height": 3.2, "roof": {"kind": "flat", "parapet_m": 0}}},
  {"id": "planting.add_area", "params": {"name": "Grass to the water", "asset_palette": [{"asset": "flora:festuca-glauca", "weight": 1}], "outline": [[-20, 5], [20, 5], [20, 60], [-20, 60]], "density_per_m2": 0.3}},
  {"id": "query.ground_height", "params": {"x": 0, "z": 30}},
  {"id": "document.inspect", "params": {"faces": false}},
  {"id": "view.render", "params": {"camera": {"position": [0, -0.2, 30], "target": [0, -1.5, 100]}, "width": 960, "height": 540, "path": "beach.png"}},
  {"id": "project.load_example", "params": {"name": "coast"}},
  {"id": "view.render", "params": {"viewpoint": "Deck over the sea", "width": 1280, "height": 720, "path": "coast.png"}}
]}
```

`water.set_sea` gives the project a sea level: water at `level` out to the horizon, cut by the
terrain and by the endless ground where they rise above it. `coast` makes a mainland: a line
to the horizon (the sea to its right, its first and last stretches running on without end) that
the endless ground follows, land at its own height on the left, under the water on the right,
so the land runs on behind the house and the sea in front of it, both to the horizon (here
along the waterline the shore makes). Without a coast the endless ground lies wholly above the
water (the sea shows only over the terrain) or wholly below it (the sea runs on everywhere). Only the fields given change; `water.remove_sea` takes it away.
`sea_state` is the WMO code, 0 (glassy) to 6 (very rough); `document.inspect` reports `sea`
with the `wave_height_m` and `peak_wavelength_m` it means. `kind` is `sea`, `lake` (shorter,
steeper waves; still water freezes in a snowy winter, the sea never does) or `river` (with
`flow_deg` and `flow_speed`); `outline` limits the water to a polygon. Waves travel along
`direction_deg`, or with the wind (`view.set wind_direction_deg`) when it is null.

`terrain.shore` shapes a coast along a line, the sea to its right (here travelling east, so the
sea lies south): a bank or bluff over `bank_m`, the beach down to the waterline over `beach_m`,
the bed falling to `depth_m` over `shelf_m`, heights measured from the water level; the land
behind the line keeps its ground. Plantings and objects are not placed where the water covers
the ground (the grass stops at the waterline: `document.inspect` counts what stands), and in
the window walking stops where the water is deeper than 0.6 m. `project.load_example {"name":
"coast"}` puts the furnished house of the `beach-house` example on a mainland beach built the
same way, its deck steps leading to the sand, with a coastal garden round it.

### 19. A furnished plan from above: hide the roof

```json recipe
{"intent": "the beach house's rooms and furniture seen from above", "steps": [
  {"id": "project.load_example", "params": {"name": "beach-house"}},
  {"id": "model.set_hidden_bodies", "params": {"id": "m1", "bodies": ["roof", "roof-membrane", "fascia", "ceiling", "ceiling-living"]}},
  {"id": "document.inspect", "params": {"faces": false}},
  {"id": "view.render", "params": {"camera": {"position": [12, 30, -4.49], "target": [12, 0, -4.5], "fov_y_deg": 40}, "time": {"hour": 12.5}, "width": 1280, "height": 720, "path": "plan.png"}}
]}
```

`model.set_hidden_bodies` leaves bodies of an imported model out of the view, by body name or
feature id (`document.inspect` with faces lists them); it is saved with the project, and an
empty list shows them all again. Hidden bodies cast no shadow and block no sky, so by day the
rooms take the sun and sky from above, as in an architect's cut-away plan. The camera here is
given, not a viewpoint, so the recipe works whatever viewpoints the project has: straight down
over the house's middle (a hair off vertical so the picture keeps north up), high enough for the
whole house and its deck in a 40 degree field.

### 20. A fence with a gate

```json recipe
{"intent": "a slatted fence round two sides of the plot, with a gate, and a picket fence", "steps": [
  {"id": "fence.add", "params": {"name": "Side fence", "line": [[19.5, 1], [19.5, 29]], "height": 1.8, "kind": "slats", "gates": [{"offset_m": 10, "width_m": 1.2}]}},
  {"id": "fence.add", "params": {"name": "Front fence", "line": [[0.5, 29.8], [19.5, 29.8]], "height": 0.9, "kind": "picket"}},
  {"id": "fence.add_gate", "params": {"id": "S2", "offset_m": 9.5, "width_m": 1.0}},
  {"id": "material.assign", "params": {"target": {"type": "face", "owner": "S2", "face": "S2:infill"}, "material": "plaster"}},
  {"id": "query.quantities", "params": {}},
  {"id": "document.inspect", "params": {"faces": false}}
]}
```

`fence.add` draws an open line as posts and infill: `kind` is `slats`, `horizontal_slats`,
`panels`, `picket`, `mesh` (a see-through wire grid) or `screen`, and `{kind}` alone gives a good
fence (sizes by kind; `slat_width_m`, `gap_m`, `post_spacing_m`, `post_size_m`, `rails` and
`thickness` change them). Posts stand at every point of the line, at both ends and evenly between;
a gate (`fence.add_gate`, `fence.remove_gate`) leaves a gap with a post on each side and a leaf in
it. The faces are `S1:posts`, `S1:infill`, `S1:rails` and `S1:gate`, for `material.assign`; they
start as timber, with a dark timber for posts and rails (metal for mesh). `fence.set` changes
any field (or turns a plain wall made with `shape.add_extrusion` into a fence), `fence.clear`
turns it back into a wall. `query.quantities` gives a fence's posts, slats (boards, pickets,
panels), gates, length and the real material volume. On terrain a fence stands on the lowest
ground under its line (`slope: "level"`, the default); recipe 33 makes one follow a slope.

### 21. Steps up to a raised terrace

```json recipe
{"intent": "a terrace raised half a metre above the lawn, with steps up to it", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [20, 0], [20, 20], [0, 20]], "material": "lawn"}},
  {"id": "shape.add_area", "params": {"name": "Terrace", "outline": [[0, -4], [20, -4], [20, 0], [0, 0]], "elevation": 0.5, "drape": false, "edge": {"style": "masonry"}, "material": "paving"}},
  {"id": "stairs.add", "params": {"name": "Terrace steps", "from": [10, 1.0], "to": [10, 0], "width_m": 1.6, "tread_m": 0.33}},
  {"id": "query.quantities", "params": {}},
  {"id": "document.inspect", "params": {"faces": false}}
]}
```

The terrace is raised by `elevation` and closed off by its `edge` (`area.set_edge`, `area.clear_edge`
take it on and off): a retaining skirt in a bed style (`plain`, `masonry` with a coping, `sleepers`
or `corten`) along the whole outline, from the top of the area down to the lowest ground under it
(the terrain's ground there when there is one). It sits just outside the outline, so the walkable
top keeps its size, and its top lies 2 mm under the area's so steps that arrive on it do not fight
with the coping. The faces are `S2:edge` and `S2:edge_rim`, brick to begin with; `query.quantities`
adds an `edge` row to the area (`length_m`, `face_area_m2`, `height_m`, and `beams` for sleepers or
`plate_area_m2` for corten). An area that lies on the ground (`drape`, the default of
`shape.add_area` without an `edge`) refuses an edge, so give `"drape": false`.

`stairs.add` draws a flight of solid steps on a footprint made from `from` (the bottom centre),
`to` (the top centre, where you arrive) and `width_m`. Every step is a block from the ground up.
`height` is the rise from the bottom to the top landing; left out it is the height of the ground
just past `to` (terrain and raised areas such as this terrace) over the lowest ground under the
steps, here the terrace at 0.5 m plus its lift. The step count is `round(height / riser_m)`
(`riser_m` 0.17 unless given) and the risers share the height evenly. `tread_m` is the depth of
a tread: a footprint longer than the steps need ends in a landing, a shorter one is warned about
(and the treads shrink to fit); without it the treads share the footprint. The result has
`steps`, `riser_m`, `tread_m` and `warnings`: 2 x riser + tread should lie between 0.60 and 0.65 m
(Blondel's rule). The faces are `S3:treads`, `S3:risers` and `S3:sides`, concrete to begin with.
`stairs.set` changes any field or moves the flight (`from`, `to`, `width_m`), or turns a closed
rectangular block into stairs; `stairs.clear` makes it a block again. `query.quantities` gives
`steps`, `riser_m`, `going_m`, `rise_m`, `run_m` and the real volume. A flight stands on the lowest
ground under its footprint, like a block does, and on uneven terrain each step's block goes down
to the lowest ground under that step, so no step floats over a dip.

### 22. A raised bed with plants

```json recipe
{"intent": "a timber raised bed on the lawn with soil and a planting that stands on it", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [20, 0], [20, 20], [0, 20]], "material": "lawn"}},
  {"id": "bed.add", "params": {"name": "Herb bed", "outline": [[4, 4], [7, 4], [7, 5.2], [4, 5.2]], "height": 0.4, "wall_thickness_m": 0.2}},
  {"id": "planting.add_area", "params": {"name": "Herbs", "asset_palette": [{"asset": "flora:festuca-glauca", "weight": 1}], "outline": [[4.3, 4.3], [6.7, 4.3], [6.7, 4.9], [4.3, 4.9]], "density_per_m2": 6}},
  {"id": "query.ground_height", "params": {"x": 5.5, "z": 4.6}},
  {"id": "query.raycast", "params": {"origin": [5.5, 3, 4.6], "direction": [0, -1, 0]}},
  {"id": "query.quantities", "params": {}},
  {"id": "document.inspect", "params": {"faces": false}},
  {"id": "bed.add", "params": {"name": "Sleeper bed", "outline": [[9, 4], [12, 4], [12, 5.2], [9, 5.2]], "style": "sleepers", "height": 0.6}},
  {"id": "bed.add", "params": {"name": "Steel bed", "outline": [[14, 4], [17, 4], [17, 5.2], [14, 5.2]], "style": "corten"}},
  {"id": "query.quantities", "params": {"ids": ["S4", "S6"]}}
]}
```

`bed.add` makes two shapes in one undo step: the walls (a closed extrusion drawn as a hollow ring
with a rim, faces `S2:wall` and `S2:rim`, timber) and an area `<name> soil` inside the walls
(`S3`, the outline moved in by `wall_thickness_m`, material `soil`) lying 5 cm below the rim. The
result gives both ids. The soil is a real area, so plants and plantings inside the bed stand on
it: `query.ground_height` in the middle is the soil's height, and a ray down the middle hits
`S3:top`, not a wall. `height` is 0.1..2 (default 0.4) and `wall_thickness_m` 0.05..0.3 (default
0.2). `bed.set` changes the height, thickness, outline or name and moves the soil with it (the
walls keep the soil's id; a soil that was removed is made again); `shape.remove` of the bed takes
its soil with it. `query.quantities` gives a bed's `soil_volume_m3` (inner area x soil depth),
`wall_length_m` (along the middle of the walls) and `inner_area_m2`. The soil is flat at the height of
the lowest ground under the bed; it does not follow terrain edited afterwards (run `bed.set`
again).

`style` gives the walls their look: `plain` (a solid timber ring, the default), `masonry` (brick with
a coping slab), `sleepers` (stacked timber beams in 0.2 m courses, long walls laid in beams of
at most 2.4 m with a joint, corners overlapping as in real sleeper beds; `wall_thickness_m` is the
beam's, 0.1..0.2, default 0.15) and `corten` (a thin weathering-steel plate, 0.005..0.02, default
0.01 and drawn at least 1 cm, with a folded top edge). Each style starts with its material (timber,
brick, timber, `corten`); `bed.set` with a new `style` changes both. Sleeper beds report `beams`
and `beam_length_m` in `query.quantities`, steel beds `plate_area_m2`. `fence.add` takes a
`material` too, for one material over the whole fence (a `corten` screen).

### 23. Curves: a round lawn, a curved border and a smooth path

```json recipe
{"intent": "a round lawn, a border with a curved edge, a smooth path, a round steel bed and a curved fence", "steps": [
  {"id": "shape.add_area", "params": {"name": "Garden", "outline": [[0, 0], [24, 0], [24, 24], [0, 24]], "material": "gravel"}},
  {"id": "shape.add_circle", "params": {"name": "Round lawn", "center": [12, 12], "radius_m": 6, "kind": "area", "material": "lawn"}},
  {"id": "shape.add_area", "params": {"name": "Border", "outline": [[0, 0], [8, 0], [8, 6], [0, 6]], "material": "soil"}},
  {"id": "shape.set_curve", "params": {"id": "S3", "edge": 2, "radius_m": 7, "side": "left"}},
  {"id": "shape.add_area", "params": {"name": "Path", "outline": [[1, 21], [8, 19.5], [16, 22], [23, 20.5], [23, 22.5], [16, 24], [8, 21.5], [1, 23]], "smooth": true, "material": "gravel"}},
  {"id": "bed.add", "params": {"name": "Round bed", "outline": [[19, 8], [17, 10], [15, 8], [17, 6]], "style": "corten", "curves": {"0": {"type": "arc", "sagitta_m": 0.5858}, "1": {"type": "arc", "sagitta_m": 0.5858}, "2": {"type": "arc", "sagitta_m": 0.5858}, "3": {"type": "arc", "sagitta_m": 0.5858}}}},
  {"id": "fence.add", "params": {"name": "Curved fence", "line": [[2, 10], [2, 18]], "curves": {"0": {"type": "arc", "sagitta_m": -2}}, "height": 1.5, "kind": "slats"}},
  {"id": "query.quantities", "params": {"ids": ["S2", "S3"]}},
  {"id": "document.inspect", "params": {"faces": false}}
]}
```

A shape stores its **control points** (`outline`, what `shape.move_vertex` and the handles move) and
draws the polygon made from them: `curves` puts an arc on single edges, `smooth` runs a smooth
curve (centripetal Catmull-Rom) through all the points, closed or open as the shape is. Measuring
(`query.quantities`, `query.measure`, `document.inspect`) and drawing use the curved outline, with
the same typed-size precision as straight edges (chords within 3 mm of the true curve, so a circle
of radius 5 has an area within 0.1% of 25 pi).

- **`shape.set_curve {id, edge, radius_m, side}`** curves edge `edge` (it runs from point `edge` to
  point `edge + 1`, wrapping when closed). The radius is the one you type: it must be at least
  half the edge's length, and gives the shorter arc. `side` is `left` (the default) or `right`
  of the edge's direction, seen from above with +X east and -Z north: walking east, left is
  north. For an outline wound clockwise from above (a positive shoelace sum in x, z), left is
  outward. `sagitta_m` instead of `radius_m` is the signed distance from the middle of the
  chord to the arc, positive to the left; `shape.add_area`, `shape.add_extrusion`, `bed.add` and
  `fence.add` take it in `curves` (`{"2": {"type": "arc", "sagitta_m": 1.2}}`). `shape.clear_curve`
  straightens an edge; `shape.set_smooth {id, smooth}` switches the smooth outline. Edges with an
  arc keep it under `smooth`.
- **`shape.add_circle {center, radius_m, kind}`** is an exact circle: 4 points and 4 quarter arcs. `kind`
  `area` takes the arguments of `shape.add_area`, `extrusion` those of `shape.add_extrusion`.
  A round raised bed is `bed.add` with such an outline and `curves` (any style; recipe 35 has it
  in sleepers). `shape.add_ellipse {center, radius_x, radius_z, rotation_deg}` is the same for an
  ellipse (recipe 35).
- **Through a point**: `shape.set_curve {id, edge, through: [x, z]}` is the third way to size an arc,
  next to `radius_m` and `sagitta_m`: it passes through that point. The point must lie between the
  edge's ends (its foot on the chord inside it) and off the chord line, and the arc through it
  may not run past a semicircle; the side follows from where the point is.
- **Editing**: `shape.insert_vertex` on an arc splits it into two arcs on the same circle: the new
  point is projected onto the arc (the answer's `position` is where it went) and each half gets
  the sagitta that keeps it on that circle. `shape.remove_vertex` drops the arcs of the two edges
  it joins; later arcs keep their edges (renumbered). Moving a vertex eases an arc that would
  bulge more than half its new chord to a semicircle. Draw shows the radius of every arc and a
  green handle in the middle of it: drag it to bend the arc (one `shape.set_curve` on release).
- **Faces keep their meaning**: a curved edge `n` of an extrusion is many triangles, all
  `<id>:side(n)`. Houses take curves (recipe 37), and stairs take an arc on their bottom
  edge only (recipe 35); a fence follows them:
  its posts are spread by arc length along the curve (never at every point of it), the slats
  between them are straight and the spans closer together where the curve is tight, and a gate
  needs a straight edge.

### 24. A plan as underlay: calibrate it, trace over it

```json recipe
{"intent": "a scanned site plan under the drawing, scaled on a known 10 m wall, with a lawn traced over it", "steps": [
  {"id": "underlay.add", "params": {"file": "site-plan.png", "name": "Site plan", "opacity": 0.6}},
  {"id": "underlay.calibrate", "params": {"id": "u1", "pixel_a": [40, 30], "pixel_b": [140, 30], "world_a": [0, 0], "world_b": [10, 0]}},
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[-2, -2], [14, -2], [14, 8], [-2, 8]], "material": "lawn"}},
  {"id": "document.inspect", "params": {"faces": false}},
  {"id": "view.render", "params": {"camera": {"position": [6, 25, 2.99], "target": [6, 0, 3], "fov_y_deg": 40}, "width": 960, "height": 600, "path": "underlay-render.png"}}
]}
```

`underlay.add` puts a PNG or JPEG (a floor plan, a site plan, a scan; at most 16384 pixels on a
side and 64 MB) flat under the drawing. `file` is relative to the project file (or the read root),
and the host reads its size into `size_px` itself (a missing, unreadable or other file is an
error that says so; a caller that states `size_px` may name a file this host does not have, which
`document.inspect` then reports with an `error`). The image's top-left lands on `origin`
(`[0, 0]`), one pixel is `metres_per_pixel` (a guess of 0.05 until you set it); at rotation 0 image
+x runs along +X (east) and image +y along +Z (south), and `rotation_deg` turns it
counter-clockwise seen from above, like a model's rotation. It is drawn flat, unlit and see-through
(`opacity`, 0.5; `elevation` 0.02 m above the ground so it does not flicker), casts no shadow, is
not picked, and shows in pictures. It sits on its own layer (`underlay`, made on demand), so `layer.set_visible` hides the plans;
what stands above the ground draws over it.

Set the scale from a dimension you know. **`underlay.calibrate`** takes two image positions in
pixels (top-left is 0, 0, y down) and the two drawing points they must land on: the scale, the
rotation and the origin follow exactly (the image is never mirrored or skewed), so the 10 m wall
above, 100 px long, gives 0.1 m a pixel. **`underlay.scale_by_distance {id, pixel_a, pixel_b,
distance_m}`** only sets the scale and keeps the origin and rotation. `underlay.set` changes any
field (`opacity`, `origin`, `rotation_deg`, `layer`, `file`...), `underlay.remove` takes the plan
out of the project (the file stays). In a bundle the image travels as `underlays/<name>`.
`document.inspect` lists each underlay with its `corners` and `bounds` on the ground. Trace over
it with the usual shape commands, reading drawing coordinates off the pixels with
`origin + R * pixel * metres_per_pixel`, or ask `underlay.calibrate`'s result for the origin.
A PDF page is not read yet: export it as a PNG first.

### 25. A photo of a facade on a wall: straighten it with its four corners

```json recipe
{"intent": "a photographed facade, straightened, as the texture of a house's south wall", "steps": [
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[4, 2], [16, 2], [16, 9], [4, 9]], "height": 6}},
  {"id": "wall_photo.add", "params": {"file": "facade.png", "target": {"type": "face", "owner": "S1", "face": "S1:side(2)"}, "corners_px": [[50, 350], [450, 325], [410, 75], [80, 50]]}},
  {"id": "document.inspect", "params": {"faces": false}},
  {"id": "view.render", "params": {"camera": {"position": [10, 3, 22], "target": [10, 3, 9], "fov_y_deg": 40}, "width": 960, "height": 540, "time": {"hour": 12}, "path": "wall-photo-render.png"}}
]}
```

`wall_photo.add` puts a PNG or JPEG (at most 16384 pixels on a side and 64 MB) on **one wall face**
as its texture. `file` is relative to the project file (or the read root); the host reads its size
into `size_px` itself, with the same errors as `underlay.add` (a missing, unreadable or other file
is refused). `target` names the face: `{"type": "face", "owner": "S1", "face": "S1:side(2)"}`
(a model's faces are named as `document.inspect` lists them). A photo of a building is seen at an
angle, so give `corners_px`: the **facade's four corners as they stand in the photo**, in pixels
(top-left of the image is 0, 0; y runs down), in the order **bottom-left, bottom-right,
top-right, top-left as seen from outside the building**. The photo is warped so that this
four-sided shape fills the wall (a perspective correction), whatever lies outside it in the photo
(sky, ground, neighbours) is cut off. Without `corners_px` the photo is taken as already straight:
its own four corners. A mirrored order is refused with a message that names the right one.

By default the facade fills **the whole face**. `region` `{u0, v0, u1, v1}` (metres) puts it on a
part of the face instead: `u` runs along the face to the right *seen from outside* from its lowest
left point, `v` up (a south wall of `side(2)` has `u` running east); outside the region the face
keeps its material, inside it the photo replaces the material's colour. `document.inspect`
lists each photo with `face_size_m` (the face's `[width, height]`, to type a region against), its
`loaded` state and any `error` or `face_error`. A face that lies flat (a floor, a flat roof) or is
curved takes no photo; a window's reveals in a wall keep the wall's material. The photo is lit like
any surface and casts and receives shadow as the wall does; it is straightened once to at most
2048 px on its long side at the region's proportions. One photo a face: adding another to the
same face replaces it. `wall_photo.set` changes any field (`region: null` makes it the whole face
again), `wall_photo.remove` takes the photo away (the file stays). It follows its side when a
vertex is inserted or removed, and goes with its shape. In a bundle the image travels as
`photos/<name>`. Not yet: several photos on one face, removing what stands in front of the facade
(trees, cars), matching the photo's colour to the sun, and a photo's EXIF rotation (turn it
upright first).

### 26. A sun map of the lawn in winter, seen from above

```json recipe
{"intent": "the hours of winter sun on the example garden's lawn, drawn on the ground", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "view.set", "params": {"season": "winter", "years": 10}},
  {"id": "view.set_sun_map", "params": {"shape": "S3", "season": "winter"}},
  {"id": "query.sun_hours", "params": {"outline": [[3, 12], [19, 12], [19, 29], [3, 29]], "season": "winter", "cell_m": 2}},
  {"id": "view.render", "params": {"camera": {"position": [11, 44, 15.01], "target": [11, 0, 15], "fov_y_deg": 40}, "width": 1000, "height": 1000, "path": "sun-map-render.png"}}
]}
```

`view.set_sun_map` draws the hours of direct sun a day gives over **one area** on the ground, as
a colour map: deep violet for no sun, through blue, teal and green to amber at 8 hours or more
(a ramp that also reads in grey and for colour-blind eyes); outside the area it is clear. Give
the area as `shape` (an area's id, `S3` is the lawn: the map follows the shape when it changes)
or as an `outline` `[[x, z], ...]` (the box round the plot, say). The day is a `season` (its
solstice or equinox: 21 December for winter in the north) or a `date` `{month, day}`, else the
project's date; `years` is the age of the plants (null follows `view.set {years}`). The sun is
cast exactly as `query.sun_hours` casts it (models, shapes, terrain and plant crowns; clouds are
not counted), on cells of `cell_m` (0.5 m, at most 10,000 inside the area; a cell whose centre lies inside an area drawn over the target, such as the pond or a path on the lawn, is clear and not counted, and `query.sun_hours` with an `outline` skips water areas the same way), so ask
`query.sun_hours` with the same outline for the numbers behind the picture (the shares in full
sun, part shade and shade; recipe 10 for a spot through the day). The map is see-through
(`opacity`, 0.6), flat a few centimetres over the area's height (over terrain too: a flat
sheet), and `view.render` draws it, so seen from straight above (eye z a hair over the target's
z, which keeps north up) you can read it: shadows sit **north** of what casts them, because the
sun is in the south. In the example the house, north of the lawn, shades the terrace and the
border but not the lawn; the hedge on the south side and the bare trees shade the strips north
of them. `view.set_sun_map` again changes only the fields you give (`{"opacity": 0.4}`, `{"season":
"summer"}`), `view.clear_sun_map` takes the map off; both are undo steps. In the window the
same is in Sun & sky, with a legend in the corner of the view.

### 27. A 20 second film of the garden, from the terrace to golden hour

```json recipe
{"intent": "a film of the example garden: a walk from the terrace over the lawn, an orbit round the pond and a flyover at golden hour, the sun moving along", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "document.inspect", "params": {"faces": false}},
  {"id": "query.quantities", "params": {}},
  {"id": "film.add", "params": {"name": "Garden at golden hour", "fps": 25}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "walk", "path": [[10, 12.5], [10, 19]], "speed_mps": 1.2, "time_of_day": [15, 16]}}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "orbit", "center": [10.5, 0, 22.5], "radius_m": 9, "height_m": 4, "from_deg": 0, "to_deg": 150, "duration_s": 8, "time_of_day": [16, 19]}}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "flyover", "path": [[2, 3], [10, 14], [18, 28]], "height_m": 14, "pitch_deg": 40, "duration_s": 6.5, "time_of_day": [19, 21.3]}}},
  {"id": "film.preview", "params": {"id": "f1"}},
  {"id": "film.preview", "params": {"id": "f1", "t": 2.5}},
  {"id": "film.preview", "params": {"id": "f1", "t": 12}},
  {"id": "film.preview", "params": {"id": "f1", "t": 19.5}}
]}
```

A film is a list of **shots** played one after the other; each shot has its own camera (the join
is a cut), and `film.add_shot` takes one `shot` object with a `kind`: `walk` (`path` `[[x, z],
...]`, the eye `eye_height_m` = 1.6 above the real ground, which `document.inspect` and
`query.ground_height` describe: terrain, areas and models; it looks ahead along the path), `glide`
(`path` `[[x, y, z], ...]`, a smooth curve through the points), `rise` (`at` `[x, z]`,
`from_height`, `to_height` above the ground, `look_at` `[x, y, z]`), `orbit` (`center` `[x, y, z]`,
`radius_m`, `height_m` above the centre's y, `from_deg`, `to_deg`: the angle is measured from east
of the centre, **positive counter-clockwise seen from above** like `rotation_y_deg`, and may pass
360), `flyover` (`path` `[[x, z], ...]`, `height_m` above the smoothed ground, `pitch_deg` = 35: a
drone looking ahead and down) and `hold` (a `viewpoint` id, or `eye` and `target`, with a
`duration_s`). How long a shot lasts is `duration_s`, or `speed_mps` and the length of the path
decide (a walk is 1.2 m/s, a glide 2, a flyover 8 unless told); not both. `ease` (`in`, `out`,
`in_out` = the default, `none`) shapes the speed along the path, so a shot starts and stops
gently; `look_at` is a point or `"ahead"`; `fov_y_deg` = 60. Along a shot the picture can change:
`time_of_day` `[from, to]` (hours, the sun's course; one number holds it), `date` `[{month, day},
{month, day}]` or `season` (the date's season shows, so a shot from March to September goes
through spring and summer), `years` `[from, to]` (the garden growing), `snow` (winter only), `wetness` and `rain` (each `[from, to]` 0..1, so a
shot can ease from dry to wet and into rain; recipe 36), `haze` (a turbidity, 0 to 50), `mist`, `gustiness` and `shafts`
(0..1; recipes 39 and 40), and the `weather` (`["fair", "storm"]`) and the `clouds`' cover
(recipe 45).

The plan for a garden: ask for its layout first (`document.inspect`: shapes and their outlines;
`query.quantities`: where the plants are and how many), and choose paths from it: a walk from the
terrace (`S2`, z 9 to 12) over the lawn (`S3`, 3 to 19 by 12 to 29) stops before the pond (`S6`,
8 to 13 by 20 to 25, centre `[10.5, 22.5]`), an orbit round the pond, a flyover over everything.
**Check it without drawing**: `film.preview {id}` lists the shots with their start and length (a
walk's length in time depends on the ground), and with `t` it gives the camera (`position`,
`target`), the changes for that picture (`overrides`) and the sun at that moment, so you can see
that the eye is where you meant (here 20 s: the first shot lasts about 5.4 s, so t = 12 is in the
orbit and t = 19.5 in the flyover, where the sun is low). Fix a shot with `film.update_shot {id,
index, shot: {...fields}}`, take one out with `film.remove_shot {id, index}`. Then draw it:
`film.render {id, dir, width, height, quality, from, to, every}` writes `frame_00000.png` and on
into `dir` beneath the write root (the same rules as `view.render`'s `path`), with the film's time
as the animation clock (wind and water move) and each shot's changes applied to that picture only;
at most 10000 frames a call, so render a long film in parts, or use `every` for a quick look (a
test of every 25th frame at 640 x 360). From a shell: `visioncraft-cli film garden.visioncraft
--out frames --size 1280x720`; `--video film.mp4` runs `ffmpeg` on the frames when it is on the
PATH (VisionCraft has no video codec of its own; without ffmpeg the result carries the command to
run). In the window, Views mode has a Film panel to list shots, add a shot from the current
camera (or a walk, an orbit, a flyover), scrub and play the film in the viewport. Not yet: smooth
joins between shots, a film's own cuts and fades, sound.

### 28. Weather the garden: dirt on the house walls, moss on the paving

```json recipe
{"intent": "age the example garden: rain streaks and a dirty foot on the plaster walls, moss and worn patches on the paving, and one slider for everything else", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "view.render", "params": {"camera": {"position": [10, 1.6, 22], "target": [10, 3, 9]}, "width": 960, "height": 540, "path": "new.png"}},
  {"id": "material.update", "params": {"id": "plaster", "weathering": {"amount": 0.7, "streaks": 0.6, "moss": 0.2}}},
  {"id": "material.update", "params": {"id": "paving", "weathering": {"amount": 0.7, "moss": 0.7, "wear": 0.5}}},
  {"id": "view.set", "params": {"weathering": 0.3}},
  {"id": "view.render", "params": {"camera": {"position": [10, 1.6, 22], "target": [10, 3, 9]}, "width": 960, "height": 540, "path": "aged.png"}},
  {"id": "view.render", "params": {"camera": {"position": [11, 0.9, 13.5], "target": [10, 0, 10]}, "width": 960, "height": 540, "path": "terrace.png"}},
  {"id": "material.update", "params": {"id": "paving", "weathering": null}},
  {"id": "view.set", "params": {"weathering": 0}}
]}
```

Materials age with procedural **weathering**, drawn in the renderer on top of the surface: dirt,
moss, wear and rain streaks, no textures, and only the albedo and the roughness change, so the
lighting stays right. `material.update {id, weathering: {amount, dirt?, moss?, wear?, streaks?}}`
(also on `material.add`): each number is 0 to 1, `amount` scales the rest, and without the others
they default to dirt 0.6, moss 0.4, wear 0.4 and streaks 0.5. A later update changes only the keys
given (a first one needs `amount`); `null` takes the weathering off. **Dirt** collects in the first
metre or so above the ground, in crevices (where occlusion darkens) and under overhangs; **moss**
grows on up-facing surfaces and on the wall side away from the sun (true north in the northern
hemisphere, south in the southern: `site.set north_deg` turns it with the drawing), low down and in
patches; **wear** lightens and smooths patches of up-facing paving; **streaks** run down walls,
more the higher the wall. `view.set {weathering: 0..1}` weathers every standard material at once
with the default mix, added to each material's own as layers of age (1 - (1 - own)(1 - view)), so
one slider ages a whole design; the view's `years` does not drive it, it stays explicit. Glass,
water, lawns, plants and the terrain's ground layers are not weathered. The height above the
ground is measured from the view's ground plane (`view.set {ground: {height}}`), so on a hill the
dirt follows that plane, not the slope. Compare the pictures: a wall at 0.7 shows a darker foot, a
few streaks and a hint of moss at its shaded base; paving shows greenish patches in the damp and
paler, smoother patches in the open. In the window: Materials has a Weathering section (Amount,
Dirt, Moss, Wear, Streaks) for the picked material, and Sun & sky an Age slider for the whole view.
`weathering` of the older kind (`kind.weathering`, the "Grime tint" slider: one tint for occluded
and downward faces, green on up-facing ones) still works and adds to it.

### 29. A cottage border in front of the hedge

```json recipe
{"intent": "a mature perennial border along a hornbeam hedge: layered by height, in drifts, closed up after three years", "steps": [
  {"id": "shape.add_area", "params": {"name": "Border bed", "outline": [[1, 1], [3.2, 1], [3.2, 17], [1, 17]], "material": "soil"}},
  {"id": "planting.add_spline", "params": {"name": "Hornbeam hedge", "asset_palette": [{"asset": "flora:carpinus-hedge", "weight": 1}], "points": [[0.2, 0.2], [0.2, 17.8]], "straight": true, "spacing": {"every": 0.8}}},
  {"id": "planting.add_border", "params": {"name": "Cottage border", "area": "S1", "style": "cottage", "seed": 3}},
  {"id": "view.set", "params": {"years": 3}},
  {"id": "view.render", "params": {"camera": {"position": [7, 1.6, 9], "target": [2, 0.8, 9]}, "season": "summer", "width": 960, "height": 540, "path": "border-eye-level.png"}},
  {"id": "view.render", "params": {"camera": {"position": [6, 1.6, 17.5], "target": [2, 0.6, 3]}, "season": "summer", "width": 960, "height": 540, "path": "border-along.png"}},
  {"id": "view.set", "params": {"years": 0}},
  {"id": "view.render", "params": {"camera": {"position": [7, 1.6, 9], "target": [2, 0.8, 9]}, "season": "spring", "width": 960, "height": 540, "path": "border-new.png"}}
]}
```

`planting.add_border {outline | area, style}` plants a border the way a designer does, from a
curated list of built-in perennials, grasses and low shrubs per style (`cottage`, `prairie`,
`shade`, `dry`; every species has growth data, `species.list` shows them and `species` swaps in
your own):

- **Layers by height.** The border has a front, the side you stand on, and a back. Each spot gets
  a species whose mature height is near the height wanted there, from `height_range`
  `[front, back]` (the style's own by default, e.g. 0.4 to 1.6 m for cottage), so delphiniums and
  phlox stand at the back, coneflowers and sages in the middle, catmint and cranesbill along the
  front. The front is `facing` (`[x, z]`, from the back to the front), or the outward side of the
  edges named in `front` (edge `i` runs from point `i` to `i + 1`), or, when neither is given,
  the side away from the nearest wall, building, fence block or hedge within 2.5 m of the outline
  (here the hornbeam hedge, so the front faces +x), else the outward side of the longest edge.
  The result's `facing` says what was used.
- **Drifts.** Species stand in elongated groups along the border, `drift_m` (2.4 m) long, a few to
  a dozen plants each, with a low matrix species (cranesbill, lady's mantle, catmint; grasses and
  sedges in the other styles) between the drifts, thinner towards the back.
- **Closes up.** Each species is planted at `closure / w^2` plants per m2 from its mature width
  `w` (`closure` 1.3 by default: crowns touch and overlap at maturity), and any gap that is left
  between two drifts is filled. The placements are the same at every age: the growth curves make a
  new border open (about 15 % of the ground under a crown at planting) and a three-year-old one
  closed (over 95 %), so `view.set years` shows the border closing up (`planting.growth` has the
  sizes). Flowers follow the season: the picture in spring has the early ones only.

The border is an area scatter with a `border` (`planting.update` changes it: `seed`, `outline`,
or the `border` object whole; `planting.remove` takes it away; all undoable). It is deterministic
by `seed`.

### 30. Garden furniture: a dining set on a terrace, and a parasol

```json recipe
{"intent": "a paved terrace with a dining table for six, a round bistro table with a parasol through it and a sun lounger, each piece sized by its parameters", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [14, 0], [14, 12], [0, 12]], "material": "lawn"}},
  {"id": "shape.add_area", "params": {"name": "Terrace", "outline": [[2, 2], [9, 2], [9, 8], [2, 8]], "elevation": 0.02, "drape": false, "material": "paving"}},
  {"id": "furniture.add_set", "params": {"set": "dining", "position": [4.6, 5], "seats": 6, "base": 0.02}},
  {"id": "furniture.add", "params": {"kind": "table", "name": "Bistro table", "position": [8, 3.2], "round": true, "length": 0.8, "height": 0.72, "base": 0.02}},
  {"id": "furniture.add", "params": {"kind": "parasol", "position": [8, 3.2], "length": 2.6, "height": 2.4, "base": 0.02, "open": true}},
  {"id": "furniture.add", "params": {"kind": "lounger", "position": [11.5, 6], "rotation_deg": 90, "back_deg": 55}},
  {"id": "furniture.set", "params": {"id": "S3", "length": 2.4}},
  {"id": "query.quantities", "params": {}},
  {"id": "view.render", "params": {"camera": {"position": [9, 3.2, 11], "target": [5.5, 0.5, 4.5]}, "width": 960, "height": 540, "path": "furniture-terrace.png"}},
  {"id": "view.render", "params": {"camera": {"position": [1, 4, 9.5], "target": [6, 0.5, 4]}, "width": 960, "height": 540, "path": "furniture-above.png"}}
]}
```

`furniture.add {kind, position, ...}` makes a piece of furniture from parameters, the way
`stairs.add` makes steps: a closed extrusion whose rectangular footprint is its length and depth
and whose height is its overall height, so a table can be given a length. Kinds: `table`
(`round: true` for a circle, `length` its diameter; a round table stands on a pedestal, others on
four legs unless `pedestal` says so), `chair`, `bench` (`back: false` for a plain seat),
`sofa` and `armchair` (seat cushions split by length, about one per 0.7 m between the arms, or
`cushions`), `lounger` (`back_deg` 0 to 80, default 40), `parasol` (`open`; `length` is the
canopy's diameter), `side_table`, `cabinet` (`doors`, `drawers`: separate front panels with
grooves) and `bed`. Each kind has default sizes (a dining table 1.8 by 0.9 by 0.75 m, a chair
0.45 by 0.5 by 0.85, a sofa 2.2 by 0.9 by 0.8, a lounger 1.95 by 0.7, a parasol 3 m across and
2.4 high); `length`, `depth` and `height` change them, and the parts follow: legs keep their
thickness, cushions are added or taken away. `position` is the centre and `rotation_deg` turns the
piece counter-clockwise seen from above (-Z north, +X east): a chair, sofa or cabinet faces +Z
(south) at 0, a lounger or bed has its head at the west end. `base` is the height of the feet.

The faces take materials like any shape's: `<id>:frame` (legs, frames, pole, door fronts),
`<id>:top` (table tops, seat shells, headboards, cabinet bodies), `<id>:cushion` (cushions,
mattress, pillows) and `<id>:canopy`, only those the kind has (the result's `faces`). They start
with teak, black powder coat, sand fabric and white canvas from the palette; `material.assign`
changes them. `furniture.set {id, ...}` changes the kind, the options, the size, the position or the
turn (a new length keeps the centre and the turn; `round` switches between a circle and a
rectangle; `null` puts an option back to its default) and turns a plain rectangular block into
a piece when it is given a `kind`; `furniture.clear {id}` makes it a block again.

`furniture.add_set {set, position, rotation_deg?, seats?}` places a group as separate pieces in
one undo step and returns their ids: `dining` is a table sized for `seats` (2 to 12, default 6)
with the chairs pulled up round it, facing it; `lounge` a sofa, two armchairs and a coffee table
facing each other; `sunbathing` two loungers with their heads north, a side table between and a
parasol behind. The group turns round `position` as a whole. `query.quantities` lists the pieces
per kind under `furniture` with their length, depth and height. Pieces are shapes, so layers,
`shape.set_height`, `shape.remove` and `material.assign` work on them; stacking a piece on a
terrace is `base` (or `shape.set_height`).

### 31. An L stair from a terrace down to the garden

```json recipe
{"intent": "a terrace raised a metre above the lawn, reached by an L stair that turns on a landing", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [20, 0], [20, 12], [0, 12]], "material": "lawn"}},
  {"id": "shape.add_area", "params": {"name": "Terrace", "outline": [[6, 0], [20, 0], [20, 12], [6, 12]], "elevation": 1.0, "drape": false, "edge": {"style": "masonry"}, "material": "paving"}},
  {"id": "stairs.add", "params": {"name": "Garden stair", "from": [3, 7.5], "via": [[3, 6]], "to": [6, 6], "width_m": 1.2, "tread_m": 0.3}},
  {"id": "query.quantities", "params": {}},
  {"id": "view.render", "params": {"camera": {"position": [-2, 5, 13], "target": [5, 0.5, 6.5]}, "width": 960, "height": 540, "path": "stair-l.png"}}
]}
```

`stairs.add` with `via` makes a flight that turns. `via` is one or two corner points between
`from` (the bottom centre) and `to` (the top centre): here the flight leaves the lawn going north,
turns right at [3, 6] and arrives on the terrace's edge going east. At each corner the flight
turns 90 degrees on a square landing of width by width centred on the corner, so the segments must
meet at right angles (within a degree) or the command says so. The height is read off the terrace
(1 m) as for a straight flight; the step count is `round(height / riser_m)` and the steps are
shared over the flights in proportion to each flight's clear run, the segment less half a landing
at each corner end (here 2 steps on the short flight and 4 on the long one). A landing lies at the
height of the last step reaching it, so it is that step with a long tread. The result adds
`flights` (`steps` and `run_m` of each) and `landings` (`size_m` and `height_m`) to the straight
flight's fields; `warnings` are per flight. Two corners make a U (or an S, when the second turn goes
the other way): the middle segment is one landing. With the corners exactly a width apart the
two flights run side by side with no gap and the landing is a half-landing of width by twice the
width; further apart it is a wider landing with a well between the flights; closer than a width
the flights would overlap, so it is refused. The outline is derived from the path and the width
(an L is 6 points, a U with a well 8), so `shape.move_vertex`, `shape.insert_vertex`,
`shape.remove_vertex` and `shape.set_outline` refuse a flight that turns: change it with
`stairs.set`, with `path` (the whole centreline, 2, 3 or 4 points) or `via` (the corners alone;
`null` makes the flight straight again, from its first point to its last). `query.quantities` adds
`flights`, `landings`, `landing_area_m2` and `total_going_m` (the length of the centreline) to a
flight that turns. The Stairs tool in Draw has a Turn option: click the bottom, each corner and
the top, with every segment held at a right angle to the one before.

### 32. A terrace cut into a slope behind the house

```json recipe
{"intent": "a sloping garden with a level cut into the slope behind the house, held by a sleeper wall", "steps": [
  {"id": "terrain.create", "params": {"origin": [0, 0], "width_m": 40, "depth_m": 30, "cell_m": 0.5, "base_material": "lawn"}},
  {"id": "terrain.stroke", "params": {"tool": "raise", "points": [[5, 1], [35, 1]], "radius_m": 12, "strength": 0.12, "falloff": 0.8}},
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[10, 17], [24, 17], [24, 25], [10, 25]], "height": 3.0, "roof": {"kind": "gable"}}},
  {"id": "terrace.add", "params": {"name": "Back terrace", "outline": [[8, 6], [26, 6], [26, 13], [8, 13]], "floor_from": "mean", "style": "sleepers", "parapet_m": 0.45, "material": "paving"}},
  {"id": "query.ground_height", "params": {"x": 17, "z": 9.5}},
  {"id": "query.quantities", "params": {}},
  {"id": "view.render", "params": {"camera": {"position": [30, 7, 30], "target": [16, 0.5, 9]}, "width": 960, "height": 540, "path": "terrace-cut.png"}}
]}
```

`terrace.add` cuts a level into the terrain. It makes a flat area (not draped) whose `floor` is an
absolute height, or `floor_from` the `lowest`, `highest` or `mean` of the ground under the outline,
and the ground inside the outline is that floor from then on. The terrain's height grid is not
edited: its mesh gets the footprint (the outline moved out by the wall's thickness) cut out
exactly, and a retaining wall closes the hole, standing outside the outline so the floor keeps its
size. The wall follows the ground along the edge, sampled at half the terrain's cell size: where the
terrace is cut into the slope it runs from the floor up to the ground plus `coping_m` (0.1 m to
begin with); where the terrace is built up it runs from the ground up to the floor plus
`parapet_m` (0 to begin with: a seat wall when you give it 0.45). The `style` is `plain`,
`masonry` (brick with a coping slab), `sleepers` (courses of timber beams 0.2 m high, the top
stepping with the slope), `corten` or `gabion` (a stone-filled wire cage, 0.2 to 0.6 m thick);
`thickness_m` changes the wall. The faces are `<id>:top` (the floor), `<id>:wall` and
`<id>:wall_rim`, each with the style's material; `material` is for the floor. Inside the outline
everything that asks for the ground gets the floor: plants and scatters, stairs, raised beds,
fences, blocks, `query.ground_height`, `query.raycast`, sun hours and the walking camera.

The answer has `cut_m3` and `fill_m3` (the original ground against the floor over the outline: the
part above the floor is cut, the part below it filled), `wall_length_m`, `max_height_m` and
`warnings`: a wall taller than 1.2 m needs an engineer's design, a terrace with no terrain under it
(the ground is then flat at 0), and a floor above the ground everywhere, which cuts nothing: use
`area.set_edge` on a raised area for that. `query.quantities` adds a `terrace` row to the area
with the same numbers plus the wall's face area on the terrain's side and on the floor's,
`beams` and `beam_length_m` for sleepers or `plate_area_m2` for corten. `terrace.set` changes the
floor, style, thickness, coping or parapet (and turns a plain flat area into a terrace),
`terrace.clear` makes it a plain area again. A terrace replaces `edge`: an area has one or the
other. Draped areas (a lawn on the slope) are cut out under a terrace. Draw has a Terrace tool: draw
the outline, with the floor at the mean ground by default, a wall style and a parapet; the area
panel shows the terrace's fields and its cut and fill.

### 33. Fences down a slope: raked pickets, stepped panels

```json recipe
{"intent": "a picket fence raked down a slope and a stepped panel fence beside it, with a gravel board", "steps": [
  {"id": "terrain.create", "params": {"origin": [0, 0], "width_m": 40, "depth_m": 30, "cell_m": 0.5, "base_material": "lawn"}},
  {"id": "terrain.stroke", "params": {"tool": "raise", "points": [[5, 1], [35, 1]], "radius_m": 12, "strength": 0.12, "falloff": 0.8}},
  {"id": "fence.add", "params": {"name": "Picket fence", "line": [[12, 3], [12, 15]], "height": 1.0, "kind": "picket", "slope": "raked"}},
  {"id": "fence.add", "params": {"name": "Panel fence", "line": [[20, 3], [20, 15]], "height": 1.8, "kind": "panels", "slope": "stepped"}},
  {"id": "fence.set", "params": {"id": "S2", "gravel_board": true}},
  {"id": "fence.add_gate", "params": {"id": "S1", "offset_m": 6, "width_m": 1.0}},
  {"id": "query.quantities", "params": {}},
  {"id": "view.render", "params": {"camera": {"position": [30, 6, 22], "target": [16, 1.5, 9]}, "width": 960, "height": 540, "path": "fence-slope.png"}}
]}
```

`slope` says how a fence follows ground that is not level, and `fence.add` and `fence.set` take it
with `gravel_board`. `level` (the default) stands the whole fence on the lowest ground under its
line. `stepped` and `raked` stand each post on the ground at its own foot, always plumb, reaching
the fence height above it; they differ in what is between the posts. `stepped` makes every span
one level panel whose bottom is the clearance above the higher of its two posts' ground, so the
run steps down the slope; `gravel_board` adds a board under each panel down to the lowest ground
under it (`<id>:rails`), and without it the answer warns when a panel stands more than 0.05 m
above the ground at its low end. `raked` runs the rails, the top and the bottom straight between
the two feet: slats and pickets stay plumb and are cut to those lines (a picket's tip on the top
line), horizontal boards run parallel to the rake, panels and the screen are parallelograms and
the wire mesh is a sheared grid. A gate leaf always stays level, on the higher ground of its two
posts, with the posts on either side keeping their own feet; the answer of `fence.add`, `fence.set`
and `fence.add_gate` has `warnings`, which tell when the ground under a leaf rises above its
bottom (it would not swing). Ground comes from the terrain and from terraces (a fence on a
terrace floor stands on the floor); without terrain every mode is the level fence.
`query.quantities` gives `slope` and `gravel_board` in the fence row, and the volume and face
areas are those of the real mesh. Draw's fence panel has an On a slope list and a Gravel board
checkbox.

### 34. Garden steps set into a lawn slope, up to a cut-in terrace

```json recipe
{"intent": "garden steps with sleeper cheeks climbing a lawn slope to a terrace cut into the top of it", "steps": [
  {"id": "terrain.create", "params": {"origin": [0, 0], "width_m": 40, "depth_m": 30, "cell_m": 0.5, "base_material": "lawn"}},
  {"id": "terrain.stroke", "params": {"tool": "raise", "points": [[5, 1], [35, 1]], "radius_m": 12, "strength": 0.5, "falloff": 0.8}},
  {"id": "terrace.add", "params": {"name": "Upper terrace", "outline": [[12, 0.5], [22, 0.5], [22, 5], [12, 5]], "floor_from": "lowest", "style": "sleepers", "coping_m": 0, "material": "paving"}},
  {"id": "stairs.add", "params": {"name": "Garden steps", "from": [17, 13.5], "to": [17, 5.15], "width_m": 1.2, "set_in": {"cheek_style": "sleepers"}}},
  {"id": "query.ground_height", "params": {"x": 17, "z": 9}},
  {"id": "query.quantities", "params": {}},
  {"id": "view.render", "params": {"camera": {"position": [26, 4, 20], "target": [17, 1.2, 8]}, "width": 960, "height": 540, "path": "steps-set-in.png"}},
  {"id": "view.render", "params": {"camera": {"position": [32, 1.5, 9], "target": [17, 1.2, 9]}, "width": 960, "height": 540, "path": "steps-set-in-side.png"}}
]}
```

`set_in` on `stairs.add` or `stairs.set` sets a flight into the slope it climbs instead of standing
it on the ground. Three things change. The goings follow the ground: the step count is still the
rise over `riser_m`, but each riser stands where the original ground (terrain and terraces, along
the centreline) has climbed to the level of the step before, so a gentle stretch becomes a long
tread and a steep one a short one, each held within 0.25 to 2 m (`tread_m` is not used). The
answer lists `goings_m` and `warnings` for a going held at a limit (the ground is steeper or
flatter than steps can follow) and for goings outside 0.28 to 0.45 m when Blondel's rule fails
too. A flight that turns does this per flight, sharing the steps by the ground each one climbs;
the landings stay as they were. The footprint is cut out of the terrain like a terrace's, widened
by the cheeks' thickness at the sides, and everything that asks for the ground inside it gets the
tread's level: plants never stand on the steps. And cheek walls stand along both sides, in a
`cheek_style` (`plain`, `masonry`, `sleepers`, `corten`, `gabion`, or `none`), `thickness_m` and
`coping_m` as a terrace wall has them: from below the step up to the ground beside it plus the
coping where it holds earth, and just under the tread where the ground is lower, so the top
follows the slope and steps with the treads. Their faces are `<id>:cheeks` and
`<id>:cheeks_rim`. With `none` the terrain is still cut and a bank beside the steps is not held,
which the answer warns about. Without `height` the rise is the ground just past the top (here the
terrace's floor) over the lowest ground under the flight, so end the flight at the terrace's
outline moved out by its wall. The answer's `set_in` and the `stairs.set_in` row of
`query.quantities` give `goings_m`, `cheek_length_m`, the cheeks' face areas, `beams` or
`plate_area_m2`, and `cut_m3` and `fill_m3`, the earth above and below the treads. `stairs.set`
with `"set_in": null` stands the flight on the ground again. Draw's stairs panel has a "Set into
the slope" checkbox and the cheek styles.

### 35. A round sleeper bed, an elliptical pond and a round-fronted step

```json recipe
{"intent": "a round bed of railway sleepers, an elliptical pond and a step with a round front on a lawn", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [24, 0], [24, 18], [0, 18]], "material": "lawn"}},
  {"id": "bed.add", "params": {"name": "Round sleeper bed", "outline": [[9, 5], [6, 8], [3, 5], [6, 2]], "curves": {"0": {"type": "arc", "sagitta_m": 0.8787}, "1": {"type": "arc", "sagitta_m": 0.8787}, "2": {"type": "arc", "sagitta_m": 0.8787}, "3": {"type": "arc", "sagitta_m": 0.8787}}, "style": "sleepers", "height": 0.4}},
  {"id": "shape.add_ellipse", "params": {"name": "Pond", "center": [17, 6], "radius_x": 4, "radius_z": 2.5, "rotation_deg": 20, "kind": "area", "elevation": -0.05, "drape": false, "material": "water"}},
  {"id": "stairs.add", "params": {"name": "Bullnose step", "from": [12, 15], "to": [12, 14.4], "width_m": 1.6, "height": 0.17, "tread_m": 0.3, "front_bow_m": 0.6}},
  {"id": "query.quantities", "params": {}},
  {"id": "view.render", "params": {"camera": {"position": [13, 7, 24], "target": [11, 0, 8]}, "width": 960, "height": 540, "path": "curves-sleepers-pond-step.png"}}
]}
```

Three more things drawn with curves.

- **Sleepers on a curve.** A sleeper bed (`bed.add` with `style: sleepers`), a sleeper edge on a
  raised area (`area.set_edge`) and a sleeper terrace wall (`terrace.add`) take curved outlines.
  Along a curve the sleepers are laid as straight beams, each a chord of it: every course of the
  curve gets the fewest equal chords that keep each within 2 cm of the arc (and none over the
  usual 2.4 m), so a 3 m radius bed has 28 beams a course. The ends are mitred, so two chords meet
  without a gap on the outside, and the joints of one course lie over the middle of the beams
  below. A straight edge keeps its long beams and overlapping corners; a corner of more than
  15 degrees between a straight and a curved edge is a corner, a gentler one is mitred like a
  curve. `query.quantities` counts `beams` and `beam_length_m`, the sum of their lengths (the
  outer face's chords).
- **`shape.add_ellipse {center, radius_x, radius_z, rotation_deg, kind, ...}`** takes the arguments
  of `shape.add_circle`. The outline is control points on the ellipse with an arc between each pair
  (through the ellipse's point halfway between them), as few as measure within 0.1% of the true
  area and perimeter (Ramanujan's formula, for the perimeter): 8 to 96 arcs; the answer has
  `arcs`, `area_m2` and `perimeter_m`. `rotation_deg` turns the x axis towards +Z (clockwise
  seen from above). Draw has an Ellipse tool next to Circle: the centre, then a corner of the box.
- **A round front on steps.** `stairs.add` and `stairs.set` take `front_bow_m`: the bottom edge
  becomes an arc that bulges that far away from the flight (negative: into it), and every riser
  is an arc concentric with it, so the first step is a bullnose and the goings count from the
  arc's apex. `shape.set_curve` on the flight's bottom edge (edge 0 for a flight from
  `stairs.add`) does the same. Only that edge takes an arc, and the arc of the top step must
  stay wider than the flight: a flight of several steps needs a large radius (or a hollow front),
  a single step takes any bow up to a semicircle. A flight that turns or is set into the slope has
  a straight front, and an arc on the top edge is not supported.

### 36. A wet terrace after rain, and a shot easing from dry into a downpour

```json recipe
{"intent": "a paved terrace beside a lawn and a pond after rain, with puddles and dark wet timber, then a film shot that eases from dry into rain", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [24, 0], [24, 18], [0, 18]], "material": "lawn"}},
  {"id": "shape.add_area", "params": {"name": "Terrace", "outline": [[4, 4], [14, 4], [14, 11], [4, 11]], "elevation": 0.04, "drape": false, "material": "paving"}},
  {"id": "shape.add_extrusion", "params": {"name": "Bench", "outline": [[5, 5], [8.5, 5], [8.5, 5.5], [5, 5.5]], "closed": true, "thickness": 0.5, "height": 0.45, "material": "timber"}},
  {"id": "shape.add_ellipse", "params": {"name": "Pond", "center": [19, 7], "radius_x": 3, "radius_z": 2, "kind": "area", "elevation": -0.05, "drape": false, "material": "water"}},
  {"id": "view.set", "params": {"wetness": 0.8}},
  {"id": "view.render", "params": {"camera": {"position": [2, 3, 16], "target": [10, 0, 7]}, "width": 960, "height": 540, "path": "terrace-wet.png"}},
  {"id": "view.render", "params": {"camera": {"position": [2, 3, 16], "target": [10, 0, 7]}, "width": 960, "height": 540, "rain": 0.8, "path": "terrace-rain.png"}},
  {"id": "view.set", "params": {"wetness": 0}},
  {"id": "film.add", "params": {"name": "Rain comes in", "fps": 25}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "hold", "eye": [2, 3, 16], "target": [10, 0, 7], "duration_s": 6, "rain": [0, 0.8]}}},
  {"id": "film.preview", "params": {"id": "f1", "t": 3}},
  {"id": "film.render", "params": {"id": "f1", "dir": "rain-in", "width": 640, "height": 360, "every": 25}}
]}
```

`view.set {wetness: 0..1}` wets every surface at once and `rain: 0..1` makes it rain; both are stored
with the project (0, the default, is not written), are undo steps, and in the window are the Wetness
and Rain sliders under Sun & sky. Rain implies wetness: what is drawn is the larger of the two.
`view.render` takes both for one picture (`wetness`, `rain`; the CLI has `--wetness` and `--rain`),
and a film shot takes `wetness` and `rain` as `[from, to]` (or one number), as it does `snow`.

- **Wet surfaces.** Porous materials (soil, wood, stone, concrete, brick, plaster, fabric) darken, about
  as the square of their albedo when soaked, and turn glossy; leaves take a slight sheen; metal,
  glass and polished paint barely change. How porous a surface is follows from its roughness and
  metalness (no extra setting): rough dielectrics soak, smooth or metallic ones do not. It also
  covers the picture's textures and the terrain's layers. A roof, a pergola or a dense crown keeps
  what is under it damp, not wet.
- **Puddles** form from a wetness of about 0.5 and grow with it: low-frequency patches on flat,
  open, upward-facing paving, terraces, paths and soil, as mirror-like water films that reflect the
  sky and the buildings. Lawns, plants, slopes steeper than about 15 degrees and ground under cover
  stay without.
- **Rain** falls in thin streaks slanted downwind (`wind_speed_mps`, `wind_direction_deg`), thicker
  with the value and fewer under a roof; rings spread over puddles, ponds and lakes; the sky closes
  to a grey overcast layer (from a drizzle of about 0.1, fully at 0.6) and the sun weakens. A plan or
  elevation view (orthographic) shows the wet surfaces, not the streaks. Snow and rain can be set
  together; snow stays dry.

### 37. A garden pavilion with a curved glass front, and a round tower room

```json recipe
{"intent": "a garden pavilion whose front wall is an arc carrying four windows and a door, and a small round tower room with a window and a door", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [24, 0], [24, 18], [0, 18]], "material": "lawn"}},
  {"id": "shape.add_extrusion", "params": {"name": "Pavilion", "outline": [[6, 4], [14, 4], [14, 10], [6, 10]], "curves": {"0": {"type": "arc", "sagitta_m": 1.2}}, "height": 3, "material": "plaster", "roof": {"kind": "flat", "parapet_m": 0.3}}},
  {"id": "shape.add_opening", "params": {"id": "S2", "side": 0, "offset_m": 1.5, "width_m": 1.2, "height_m": 2.2, "sill_m": 0.3, "frame": "split"}},
  {"id": "shape.add_opening", "params": {"id": "S2", "side": 0, "offset_m": 3.0, "width_m": 1.2, "height_m": 2.2, "sill_m": 0.3, "frame": "split"}},
  {"id": "shape.add_opening", "params": {"id": "S2", "side": 0, "offset_m": 4.23, "kind": "door"}},
  {"id": "shape.add_opening", "params": {"id": "S2", "side": 0, "offset_m": 5.5, "width_m": 1.2, "height_m": 2.2, "sill_m": 0.3, "frame": "split"}},
  {"id": "shape.add_opening", "params": {"id": "S2", "side": 0, "offset_m": 7.0, "width_m": 1.2, "height_m": 2.2, "sill_m": 0.3, "frame": "split"}},
  {"id": "shape.add_circle", "params": {"name": "Tower room", "center": [19, 13], "radius_m": 2.5, "kind": "extrusion", "height": 3.2, "material": "plaster", "roof": {"kind": "flat"}}},
  {"id": "shape.add_opening", "params": {"id": "S3", "side": 0, "offset_m": 2.0, "width_m": 1.0}},
  {"id": "shape.add_opening", "params": {"id": "S3", "side": 2, "offset_m": 2.0, "kind": "door"}},
  {"id": "query.quantities", "params": {}},
  {"id": "view.render", "params": {"camera": {"position": [3, 4, -3], "target": [12, 1.5, 6]}, "width": 960, "height": 540, "path": "curved-pavilion.png"}}
]}
```

A house takes curves like any block: `shape.add_extrusion`, `shape.add_circle` (with `kind: extrusion`
and a `roof`), `shape.set_curve` and `shape.set_smooth` all work on one, and the house is rebuilt.

- **Walls.** A curved edge is the tessellated outline, many short straight wall pieces of one solid:
  the faces stay continuous round the curve and every piece is still `<id>:side(n)` for its control
  edge `n`. The parapet of a flat roof is the outline offset by 20 cm and mitred at every vertex, so
  it has the same width all round; the roof deck and the floor follow the curve.
- **Openings.** `side` is the control edge, as on a straight wall. On a curved edge `offset_m` and
  `width_m` are lengths along the arc, from the edge's first corner, so an opening keeps its place
  when the curve changes. The window or door is a straight unit set on the chord of its arc span, as a
  facade takes flat window units: the wall is cut with planar reveals square to the chord, the frame
  and glass stand in the reveal and the sill is flush with the surface. A span whose chord would leave
  the 0.3 m wall (a sagitta over 0.18 m: for example 1.6 m of arc on a 1.5 m radius) is narrowed
  about its centre until it fits, and `shape.add_opening`, `shape.update_opening` and `shape.set_roof`
  answer with a `warnings` note saying so; an opening that no longer fits after a curve change is
  narrowed or left out the same way (with a note in the shape's mesh warnings).
- **Roofs.** Only a flat roof follows a curve. A gable, hip or lean-to needs a rectangular or
  L-shaped outline with straight walls: on a curved one the roof is drawn flat, with a note that
  says to use a flat roof.
- **Quantities.** The house's row in `query.quantities` has `house`: `wall_length_m` (a curved edge by
  its true arc length), `wall_area_gross_m2`, `openings`, `openings_area_m2` (each opening's width
  along the arc by its height) and `wall_area_m2`, the gross area less the openings.

### 38. Check a planting plan against the light, and swap the misfits

```json recipe
{"intent": "a house with two wings that make a shady courtyard on its north side; two lavenders (one in the courtyard, one in the open), found by the light check and swapped for a plant that likes shade, then a shade border chosen by the sun it gets", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [24, 0], [24, 26], [0, 26]], "material": "lawn"}},
  {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[2, 12], [22, 12], [22, 16], [2, 16]], "height": 10}},
  {"id": "shape.add_extrusion", "params": {"name": "East wing", "outline": [[16, 0], [18, 0], [18, 12], [16, 12]], "height": 10}},
  {"id": "shape.add_extrusion", "params": {"name": "West wing", "outline": [[6, 0], [8, 0], [8, 12], [6, 12]], "height": 10}},
  {"id": "planting.add_single", "params": {"name": "Courtyard lavender", "asset": "flora:lavandula-angustifolia", "position": [12, 0, 8]}},
  {"id": "planting.add_single", "params": {"name": "Lawn lavender", "asset": "flora:lavandula-angustifolia", "position": [12, 0, 22]}},
  {"id": "query.plants_for_light", "params": {"outline": [[9, 5], [15, 5], [15, 10], [9, 10]], "grower": "perennial", "limit": 5}},
  {"id": "query.light_check", "params": {"include_ok": true}},
  {"id": "planting.update", "params": {"id": "g1", "asset": "flora:skimmia-japonica"}},
  {"id": "query.light_check", "params": {}},
  {"id": "planting.add_border", "params": {"name": "Courtyard border", "outline": [[9, 6], [15, 6], [15, 10], [9, 10]], "style": "auto", "facing": [0, 1]}},
  {"id": "view.render", "params": {"camera": {"position": [12, 14, -8], "target": [12, 0, 8]}, "width": 960, "height": 540, "path": "courtyard.png"}}
]}
```

Plants carry an estimated light range, the hours of direct sun on a summer day they thrive in (full
sun 6 h and more, part shade 3 to 6 h, full shade under 3 h; many plants span two classes, the
lawn wants 4 h and more). The ranges are general horticultural knowledge, flagged `estimated` in
every answer and not yet checked against open data; a species outside the table gets a guess from
its grower type, flagged `from_table: false`. Three commands use them, all on the sun-hours
computation of `query.sun_hours` (the summer solstice unless `season` or `date` say otherwise, so a
`query.sun_hours` with `season: summer` gives the same hours):

- **`query.plants_for_light {point | outline | hours, season | date, grower?, limit?}`** ranks the
  species for a spot. Over an outline the ranking is for the mean hours, and `spot` also gives the
  min and max. Each entry has `fit` (`full` when the hours lie inside the range, `partial` when
  outside it but within one class; species two classes away are left out), a `reason`, the `light`
  range, `grower`, the mature size and `estimated`. The plants that want just this light come
  before those that take anything. Given `hours` it needs no host and runs in a bare session.
- **`query.light_check {season | date, years?, ids?, layer?, include_ok?}`** measures the sun at every
  placed plant (`flora:` species of plantings and single placements) and holds it against the
  species' range: `summary` has the count by verdict (`ok`, `too_sunny`, `too_shady`; half an hour
  either side of the range still counts as fine), `plants` lists the misfits with their `position`
  `[x, z]`, `hours`, `light` range and `suggestions` (2 or 3 species of the same grower type that
  fit, closest in mature height first), and `by_species` and `by_planting` count them up. A plant
  is measured at half its height (up to 2 m), and its own planting's crowns never shade it, so a
  tree is not judged by its own canopy; other plantings' crowns do. Swap a misfit with
  `planting.update` (a single placement takes `asset`; a scatter takes `asset_palette`).
- **`planting.add_border {style: "auto"}`** takes the mean sun hours over the outline: shade below
  3 h, cottage from 3 to 6 h, prairie above 6 h (`dry: true` makes it dry instead of prairie). The
  answer has the chosen `style` and a `style_reason`, and `light`, the measured hours. An explicit
  style that clearly mismatches the light (shade in 6 h or more, cottage, prairie or dry in under
  3 h) answers with a `light_fit` warning. A bare session without geometry refuses `auto`.

In the window, Sun & sky under the sun-hours map has "Plants for this light" (the species for the
area the map shows, by its mean hours) and a "Check plant light" button; the misfits are ringed on
the plan in the Draw view, amber for too sunny and blue for too shady.

### 39. A hazy autumn morning, and a windy rain shower

```json recipe
{"intent": "the example garden on a misty autumn morning with a low sun glowing through the mist, then a film shot of a windy rain shower with gusts rolling over the lawn and through the trees", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "view.set", "params": {"season": "autumn", "mist": 0.6}},
  {"id": "time.set", "params": {"hour": 8.5}},
  {"id": "view.render", "params": {"camera": {"position": [15, 3, 26], "target": [9, 0.3, 13], "fov_y_deg": 60}, "width": 960, "height": 540, "path": "misty-morning.png"}},
  {"id": "view.set", "params": {"mist": 0, "wind_speed_mps": 12, "wind_direction_deg": 300, "gustiness": 1}},
  {"id": "time.set", "params": {"hour": 15}},
  {"id": "film.add", "params": {"name": "Shower", "fps": 25}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "hold", "eye": [15, 3, 26], "target": [9, 0.3, 13], "duration_s": 6, "rain": [0.2, 0.8], "mist": [0, 0.2]}}},
  {"id": "film.preview", "params": {"id": "f1", "t": 3}},
  {"id": "film.render", "params": {"id": "f1", "dir": "shower", "width": 640, "height": 360, "every": 25}}
]}
```

The air between the camera and the garden has two layers. `haze` is the aerosol of a dry day, as
a turbidity (2.5 a clear day, about 30 km of visibility); it thins over 1.2 km of height and only
the distance feels it. `mist` (0..1) is a low layer of water droplets that thins over 25 m: the
visibility at the ground falls from 1.3 km at 0.3 (a light morning haze) to 330 m at 0.6 (a misty
morning) and 120 m at 1 (fog). Rain brings its own fog on top, about 1.1 km of visibility in a
drizzle (0.4) and 400 m in a downpour (0.8). Both are drawn as exponential height fog with light
scattered in: distant hedges, trees and the horizon fade into the veil while the garden 10 to 30 m
off only softens; the sky near the horizon dissolves into it, and water, glass and the rain's
streaks sit in it like everything else. The droplets scatter all colours alike and mostly forwards,
so the veil is white or grey (never blue), glows towards a low sun, takes the grey of a rain sky,
and dims the sun on the ground (a low sun in a mist is a pale disc and casts faint shadows).

Gusts are a field of stronger wind carried downwind at the wind speed: `view.set {"gustiness":
0..1}` (0, the default, is not written; the window's Gusts slider under Sun & sky). They roll over
a lawn as waves of bending grass whose tips turn paler, sway trunks (slowly), branches and leaves
(faster) of one crown after another, and swing the rain's slant as they pass. The bend scales with
`wind_speed_mps`; `wind_gustiness` stays the slow swell of the whole wind. To see them move in
pictures, give `view.render` an `animation_s`: three pictures half a second apart show a gust
crossing the lawn. `view.render` takes `haze`, `mist` and `gustiness` for one picture (the CLI's
`--haze`, `--mist` and `--gustiness`), and a film shot eases each as `[from, to]`.

### 40. Golden hour towards the sun, with light shafts through a tree

```json recipe
{"intent": "a golden-hour picture of the example garden looking into the low sun through the field maple, in a light mist, with light shafts and the sun's glare through the crown; the same without shafts; and a short hold shot that brings the shafts in", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "time.set", "params": {"month": 9, "day": 5, "hour": 19.0}},
  {"id": "view.set", "params": {"mist": 0.3, "shafts": 1}},
  {"id": "view.render", "params": {"camera": {"position": [18.8, 1.4, 26.4], "target": [6, 4.6, 27.1], "fov_y_deg": 60}, "width": 1280, "height": 720, "path": "golden-hour-shafts.png"}},
  {"id": "view.render", "params": {"camera": {"position": [18.8, 1.4, 26.4], "target": [6, 4.6, 27.1], "fov_y_deg": 60}, "width": 1280, "height": 720, "shafts": 0, "path": "golden-hour-plain.png"}},
  {"id": "film.add", "params": {"name": "Into the light", "fps": 25}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "hold", "eye": [18.8, 1.4, 26.4], "target": [6, 4.6, 27.1], "duration_s": 4, "shafts": [0, 1]}}},
  {"id": "film.preview", "params": {"id": "f1", "t": 2}}
]}
```

`view.set {"shafts": 0..1}` (0, the default, is not written; the Light shafts slider under
Weather) shadows the sun's light in the haze, the mist and the rain fog: the renderer marches
through the four sun shadow cascades along each view ray, at half resolution, with the fog's own
density and phase, and takes the light out where crowns, hedges and houses shade the air. What is
left are beams through the gaps of a crown, a dark lane behind a trunk and a wedge of shade past a
house's edge, strongest looking into a low sun; shadowed, the medium also scatters its full forward
glow, so the air round the sun brightens. Shafts need a medium: in clear air nothing shows (and
nothing is computed); mist 0.3 gives a soft glow with faint beams, 0.6 and more clear ones. The
sun's glare (a halo round the disc, as bright as the share of the disc the leaves leave open) and
the clear air's own colour kilometres off (blue at noon, warm at a low sun: Hillaire's aerial
perspective) are always on. `white_balance` is `"daylight"` (the default: a low sun stays orange),
`"auto"` (from the sun's and the sky's light, as a camera's automatic white balance: most of the
cast is taken out, some of the mood kept, so a golden hour is a little less orange and a rain
sky a little cooler) or a temperature in kelvin (2000 to 12000). `grade` is `neutral` (the
default), `warm`, `cool` or `soft`. `view.render` takes `shafts`, `white_balance` and `grade` for
one picture (the CLI's `--shafts`, `--white-balance` and `--grade`), and a film shot eases
`shafts` as `[from, to]`.

### 41. A photo of the border at f/2.8, with a light vignette and grain

```json recipe
{"intent": "a photo of the example garden's border with a short telephoto at f/2.8, focused on the flowers 4 m away so the hedge behind softens, with a light vignette and fine grain; then the same view stopped down with the autofocus", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "view.set_photo", "params": {"samples": 64, "aperture": 2.8, "focus": 4}},
  {"id": "view.set", "params": {"vignette": 0.3, "grain": 0.2}},
  {"id": "view.render", "params": {"camera": {"position": [5.4, 1.0, 19.6], "target": [2.2, 0.55, 16.4], "fov_y_deg": 24}, "quality": "photo", "width": 1280, "height": 720, "path": "border-f2.8.png"}},
  {"id": "view.render", "params": {"camera": {"position": [5.4, 1.0, 19.6], "target": [2.2, 0.55, 16.4], "fov_y_deg": 24}, "quality": "photo", "samples": 16, "aperture": 8, "focus_m": "auto", "focus_point": [0.5, 0.45], "width": 640, "height": 360, "path": "border-f8.png"}}
]}
```

A photo averages `samples` frames (1 to 1024): each is moved by a fraction of a pixel (Halton
points), the sun's light moves within its disc (soft shadows), and the noise of occlusion, light
shafts and leaf edges turns with the frame. The frames are summed in linear HDR before bloom, the
tone map and grain, which then run once on their mean, so edges and foliage are smooth, not
shimmering, and nothing is blurred that a camera keeps sharp. `view.set_photo` keeps the photo's
settings with the project (the app's photo export uses them; `view.render` at quality `photo`
takes every field it is not given from them): `samples` (64), `aperture` (an f-number, 1 to 32;
null, the default, is a pinhole with everything sharp), `focus` (`"target"`, the default: the
camera's target; `"auto"`: what the picture shows at `focus_point`, read from the first frame's
depth; or metres) and `shutter_s` (0 to 1: plants in the wind, water and rain blur with their
motion from `animation_s` on). The lens is a thin lens on a 36 mm full-frame sensor with the focal
length the field of view asks for: 50 degrees across a 16:9 picture is a 25 mm wide angle, where
f/2.8 still keeps most of a garden sharp; for a soft background use a narrower field (24 degrees
is about 60 mm) as a photographer would. `view.render` takes `samples`, `aperture`, `focus_m`,
`focus_point` and `shutter_s` at any quality (`samples` over 1, an aperture or a shutter averages
frames at that quality; the result carries `focus_m`), the CLI's `--samples`, `--aperture`,
`--focus`, `--focus-point` and `--shutter`, and `film.render` the same per frame (8 samples at
quality `photo` unless told). `view.set {"vignette": 0..1, "grain": 0..1}` (both 0 by default, and
then not written) darken the corners as a lens does (0.3 about half a stop) and add film grain in
the luminance after the tone map, in the live view and every picture; `view.render` takes both for
one picture (`--vignette`, `--grain`).

### 42. Three films by name: a walk round the hedge, a drone reveal and a dusk glide

```json recipe
{"intent": "three short films of the example garden made from names instead of coordinates: a dusk walk from the gate past the border to the pond, planned round the hedge and the trunks, pausing at the corner and turning to the pond and the house without a cut; a drone view that rises from eye level to the whole plot and orbits it; a day into dusk whose lamps fade in; and a plan reveal that ends straight above with north up", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "view.set", "params": {"wind_speed_mps": 3, "gustiness": 0.4, "vignette": 0.3}},
  {"id": "film.add", "params": {"name": "Garden walk", "fps": 24}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "walk", "route": [[16.6, 28.6], [9, 26.1], [4.7, 23.4], [10, 16.2]], "duration_s": 19.6, "pause_at": [{"point": [4.7, 23.4], "s": 3.2}], "look": [{"at_s": 10, "target": "Pond", "hold_s": 2.5}, {"at_s": 16, "target": "House", "hold_s": 3}], "date": {"month": 9, "day": 5}, "time_of_day": [19.3, 19.7], "mist": 0.5, "shafts": 1, "white_balance": 7500, "grade": "warm"}}},
  {"id": "film.reveal", "params": {"name": "Drone view", "target": "plot", "end": "orbit", "from": "south", "seconds": 4.5, "extras": {"fov_y_deg": 55, "ease": "in", "date": {"month": 6, "day": 21}, "time_of_day": [16.5, 16.6]}}},
  {"id": "film.orbit", "params": {"id": "f2", "target": "plot", "from": "south", "sweep_deg": 100, "seconds": 6.5, "extras": {"fov_y_deg": 55, "join": "continue", "ease": "none", "date": {"month": 6, "day": 21}, "time_of_day": [16.6, 16.8]}}},
  {"id": "film.add", "params": {"name": "Day into dusk", "fps": 24}},
  {"id": "film.add_shot", "params": {"id": "f3", "shot": {"kind": "glide", "path": [[18.6, 1.7, 29.2], [18.5, 1.65, 26.0], [18.3, 1.6, 23.2]], "duration_s": 12, "look_at": "Terrace", "fov_y_deg": 55, "date": {"month": 6, "day": 21}, "time_of_day": [21.4, 23.1], "mist": 0.15, "lights": [0, 1]}}},
  {"id": "film.reveal", "params": {"name": "Plan reveal", "target": "plot", "end": "top_down", "north_up": true, "seconds": 6}},
  {"id": "query.clearance", "params": {"from": [16.6, 28.6], "to": "Terrace"}},
  {"id": "query.raycast", "params": {"origin": [5, 11, 22], "direction": [1, 0, 0], "plants": true}},
  {"id": "film.preview", "params": {"id": "f1", "t": 11}},
  {"id": "film.preview", "params": {"id": "f2", "sheet": true}},
  {"id": "film.render", "params": {"id": "f1", "dir": "walk", "width": 480, "height": 270, "quality": "low", "every": 12, "preview": "sheet"}}
]}
```

Every point a command takes is `[x, y, z]` or a reference, so a film is written with what it is
about instead of where it is: a shape id (`"S6"`), a planting (`"g4"`: all its plants) or one
plant of it (`"g4#12"`, counted from 0), a model, a lamp or a viewpoint id, `"plot"` (everything
drawn) or a name matched without regard to case (`"Pond"`). It becomes the centre of its box, and
the answer's `resolved` lists what each reference turned into (centre, height, radius, box).
Plants are as big as the growth model makes them `years` after planting (the shot's `years`, else
the view's, else full grown), so a tree's crown is where its crown is. A name that is also another
item's (the pond is a shape and a viewpoint) takes the shape, and a `warnings` entry says what else
it could mean; an id picks exactly. An unknown reference is refused with the close matches. It
works in `film.add_shot` and `film.update_shot` (`center`, `look_at`, `target`, `eye`, `at`,
waypoints, pause points, looks), in `film.orbit`, `film.flythrough` and `film.reveal`, in
`viewpoint.add` and `viewpoint.update` and in the `camera` of `view.render`.

A shot keeps the name behind a point it was given as one, in its `targets` (`{"target": "S6",
"look_at": "g4#12", "path[2]": "Pond"}`, by the field it fills; a shot reads back with the point
too). The film looks the names up again every time it is played or rendered, so moving the pond
moves the shots that look at it; a name that no longer resolves keeps the point it had, and
`film.preview` answers a `warnings` entry for it. A field given again (`film.update_shot`) lets go
of the name it had. A walk's `route` is planned once, and `film.reveal`'s path is worked out once
from the target's size; only the `look_at` of those follow the name, and `film.orbit` follows its
`target` with its centre (the radius stays). Shots from before `targets` existed have none.

`film.orbit {target, from, sweep_deg, distance, height_m}` circles a target: `from` is a compass
word (`south`, `north-east`, `ssw`) or a bearing in degrees (0 north, 90 east, clockwise from
north; -Z is north, +X is east) and `sweep_deg` goes clockwise seen from above (negative: the
other way). With `distance: "frame"` (the default for a reference) the radius and the height are
set from the target's size and the field of view so it fills about 85 % of a 16:9 frame, the camera
25 degrees above the horizon; a number is the radius in metres. `film.reveal {target, end, from,
north_up}` rises from eye level on the `from` side: `end: "top_down"` ends straight above the
target, framed, a hair off the vertical so the picture's up is north (`north_up`; `false`: away
from `from`); `end: "orbit"` ends exactly where `film.orbit` on the same target starts, so the two
join. Without `end`, `from` and `to` are points as before.

`join: "continue"` on a walk, glide, flyover or orbit starts the shot where the one before ended:
its path gets that point first, the look direction turns from the old to the new over about a
second, and the easing is blended across the boundary: the shot starts at the speed the last one
ended with, and its `ease` only shapes how it ends (`in_out` ends at rest, `none` keeps the
speed), so there is no dip. A walk takes `route` (waypoints, planned round houses, blocks, walls,
fences, water, hedges, shrubs and trunks on a 2D grid with A*; the answer's `route` has the length
and `warnings` say where no clear way exists or a waypoint stands in something and was moved), or
a `path` of your own (which warns when it crosses a hedge, wall or water), `pause_at [{point |
target, s}]` (stands still there; the speed is solved so walking and standing fit `duration_s`)
and `look [{at_s, target, hold_s = 2}]` (from `at_s` into the shot, the pauses counted, the camera
turns to the target over a second, holds, and turns back: no cut). `query.clearance {from, to}`
says whether the line between two places is clear on foot and in sight, with a route round when it
is not, and `query.raycast {plants: true}` also hits crowns (ellipsoids) and trunks (cylinders).

A shot eases `lights` (0..1: the lamps' light, so lamps fade in with the dark instead of popping
on at sunset, and the automatic exposure follows), `white_balance` (kelvin), `grade` (two grades
mix) and `exposure_ev`, besides the weather keys. **`exposure_ev` of a shot is stops relative to
the automatic exposure**: 0 is no correction, +1 one stop brighter. `view.set`'s `exposure_ev` is
another thing, an absolute exposure value where lower is brighter and 0 is a white frame in
daylight; `view.render {exposure_offset_ev}` is the relative one for one picture. `film.preview
{id, sheet: true}` and `film.render {preview: "sheet"}` return a contact sheet of the film as an
image block (over MCP, and `command_run` returns any picture that way), so an agent can see a film
without any other tool; the answer's `sheet` lists the time and the shot of each picture. The
agent's three films were 345 lines of pretty-printed JSON (19 steps, with every coordinate
worked out and the walk cut in three); the same three films here are 214 lines (12 steps), and
the walk is one shot.

### 43. How open is each spot: sky occlusion and the sky view of the garden

```json recipe
{"intent": "the example garden lit by the sky it sees: the ground under the trees and the corners by the hedge and the house gently darker, the backlit crowns green inside, and how open three spots are for a planting plan", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "view.set", "params": {"sky_occlusion": true}},
  {"id": "query.sky_view", "params": {"point": [12, 18]}},
  {"id": "query.sky_view", "params": {"point": [6, 27]}},
  {"id": "query.sky_view", "params": {"point": [1.3, 15]}},
  {"id": "query.sky_view", "params": {"point": [8.3, 1.5, 9.1], "normal": [0, 0, 1]}},
  {"id": "time.set", "params": {"month": 9, "day": 5, "hour": 19.0}},
  {"id": "view.render", "params": {"camera": {"position": [18.8, 1.4, 26.4], "target": [6, 4.6, 27.1], "fov_y_deg": 60}, "width": 1280, "height": 720, "mist": 0.4, "shafts": 1, "white_balance": "auto", "grade": "warm", "path": "golden-hour-sky-occlusion.png"}},
  {"id": "view.render", "params": {"camera": {"position": [18.8, 1.4, 26.4], "target": [6, 4.6, 27.1], "fov_y_deg": 60}, "width": 1280, "height": 720, "mist": 0.4, "shafts": 1, "white_balance": "auto", "grade": "warm", "sky_occlusion": false, "path": "golden-hour-open-sky.png"}}
]}
```

`view.set {"sky_occlusion": true}` lights every surface by the sky it sees, after the sky
visibility of Ghost of Tsushima: once per change of the design a grid of probes over the garden
(a metre apart, twelve layers crowding towards the ground) traces how much of the sky each point
sees through buildings, shapes, models, the terrain and the crowns, which let sky through by their
species' leaf density and the leaves they carry in the month shown; every frame the sky's light,
as it is at that hour and in that weather, is multiplied by it. The ground under a tree and the
foot of a wall go gently darker, the leaves inside a backlit crown get the sky through the leaves
on both sides instead of going black, and wet paving and windows under a tree stop mirroring the
open sky. It is rebaked when the shapes, the plantings, the terrain, the years or the leaves
change (0.4 s for the example garden natively, a few seconds in the browser spread over the
frames), not when
the hour or the weather does. Leave it unset (`null`) and the window and its pictures turn it on;
`view.render {sky_occlusion}` and the CLI's `--sky-occlusion on|off` set it for one picture.

`query.sky_view {point}` answers how open a spot is: the share of the sky it sees,
cosine-weighted as light falls (the sky view factor: 1 on an open field, about 0.75 in the middle
of the example's lawn among its full-grown trees, 0.5 to 0.55 under the field maple and by the
hedge, 0.15 at the bottom of a narrow passage; a wall facing open ground sees at most 0.5). `[x, z]` stands on the
ground there; `[x, y, z]` with a `normal` measures a wall or a window (the third query above is
the house's back wall at window height, facing the garden). Read it with `query.sun_hours`: a spot
with three hours of sun and a sky view of 0.7 is bright part shade, one with three hours under a
dense crown at 0.3 is dark.

### 44. A wooded hillside: rock on the steep faces, scree, moss, boulders and deadwood

```json recipe
{"intent": "a hillside above a small wood that reads as natural ground: bare rock on the steep faces, scree below it, moss in the hollows and on the shaded side, boulders gathered on the rocky ground, fallen logs, stumps and a dead standing tree in the wood, and stones, twigs and moss cushions near the camera", "steps": [
  {"id": "terrain.create", "params": {"origin": [0, 0], "width_m": 80, "depth_m": 60, "cell_m": 0.5, "base_material": "lawn"}},
  {"id": "terrain.stroke", "params": {"tool": "raise", "points": [[22, 14], [58, 12]], "radius_m": 13, "strength": 1.1, "falloff": 0.35}},
  {"id": "terrain.stroke", "params": {"tool": "raise", "points": [[48, 10], [62, 18]], "radius_m": 7, "strength": 0.9, "falloff": 0.3}},
  {"id": "terrain.stroke", "params": {"tool": "noise", "points": [[40, 25]], "radius_m": 45, "strength": 0.35, "falloff": 0.2}},
  {"id": "planting.add_area", "params": {"name": "Wood", "asset_palette": [{"asset": "flora:quercus-robur", "weight": 2}, {"asset": "flora:betula-pendula", "weight": 1}], "outline": [[4, 32], [46, 32], [46, 58], [4, 58]], "density_per_m2": 0.012}},
  {"id": "terrain.set_auto", "params": {}},
  {"id": "terrain.set_tiling", "params": {"hex": true}},
  {"id": "terrain.set_cover", "params": {}},
  {"id": "rock.scatter", "params": {"outline": [[0, 0], [80, 0], [80, 60], [0, 60]], "density_per_100m2": 2.5, "seed": 4}},
  {"id": "rock.add", "params": {"position": [36, 22], "size_m": 3.2, "shape": "outcrop", "yaw_deg": 20, "seed": 2}},
  {"id": "deadwood.scatter", "params": {"planting": "g1", "density_per_100m2": 1.5, "seed": 2}},
  {"id": "deadwood.add", "params": {"kind": "log", "position": [30, 40], "length_m": 6, "diameter_m": 0.45, "yaw_deg": 25, "seed": 3}},
  {"id": "deadwood.add", "params": {"kind": "snag", "position": [22, 36], "length_m": 7, "diameter_m": 0.4, "seed": 4}},
  {"id": "time.set", "params": {"month": 9, "day": 20, "hour": 16.0}},
  {"id": "view.render", "params": {"camera": {"position": [62, 9, 62], "target": [36, 5, 20], "fov_y_deg": 55}, "width": 1280, "height": 720, "path": "wooded-hillside.png"}}
]}
```

`terrain.set_auto` covers the terrain's base layer by the shape of the ground, recomputed whenever
the ground changes: `rock` on slopes past `rock_slope_deg` (38 to begin with) and on convex edges,
`scree` from `scree_slope_deg` (27) down to the foot of the slope, `moss` in hollows and on faces
turned away from the sun (north, after `site.set north_deg`), and `soil` (the forest floor) where
hollows collect. `rock_above_m` puts rock on gentler ground above a height too; `curvature`,
`moisture` and `blend_deg` weigh the rules; low-frequency noise of `macro_scale_m` moves every
boundary, so no two slopes look alike. Each class becomes a terrain layer (added when missing;
the materials `rock`, `scree` and `moss` are CC0 maps from the library, added to the project
when it lacks them). Paint on the other layers stays and wins: paint a path of gravel across the
scree and it stays gravel. Give only the fields to change; `{"enabled": false}` turns it off. The
layers blend by their height maps, so rock pokes up through the grass where its relief is high
instead of fading in. `terrain.set_tiling {"hex": true}` hex-tiles every layer's maps seen from
above, each hexagon shifted and turned at random, so no repeat of a tile shows over a field (three
samples per layer instead of one).

`rock.add` places one rock (`boulder`, `slab`, `outcrop` or `cobble`, `size_m` its width, a
`seed` for its variant), generated with fracture faces and a rough surface, or a CC0 scan with
`model` (`boulder_01`, `rock_07`, `namaqualand_boulder_02`; `catalog.search {"query": "rock"}`),
which the generated rock stands in for while it loads. It sits on the lowest ground under its
footprint, sunk by `sink` of its height (0.25) and tilted towards the slope by `align` (0.7), so
no edge floats. `rock.scatter` scatters them over an outline in clusters, sizes skewed small, and
with `follow_terrain` (the default) gathers them where the terrain shows rock or scree and on
steep ground; the answer's `group` removes them together (`rock.remove {"group": "rs1"}`).
`deadwood.add` places a `log` or `branch` lying along `yaw_deg` (pitched to the ground under its
ends; bowed, tapering to the top, with a root collar, broken slanted ends and the stubs of its
branches), a `stump` on its root flare or a `snag`, a dead standing tree with a broken top;
sizes default by kind, the bark is `bark` and the broken ends weathered `timber`.
`deadwood.scatter` does the same over an outline or a woodland planting's ground (`planting`).
Both scatters keep clear of what stands there already: rocks and deadwood (their own pieces
too; rocks of a cluster may lean into each other a little), the areas drawn on the ground
(paths, terraces, paving, ponds), houses, blocks and walls, and the trunks of trees and palms;
`rock.add` and `deadwood.add` place where they are told. `rock.set` and `deadwood.set` change
any field, both lists are in `document.inspect` (`rocks`, `deadwood`).

`terrain.set_cover` scatters small cover over the terrain from the same layers, within 25 m of
the camera and dissolving towards that edge: stones on scree (fewer on rock and soil), cushions
of moss in the shade, twigs on soil, and leaf litter under deciduous crowns all year. The values
multiply the built-in densities (0..4). It is made again only when the camera moves to another
8 m cell, and left out at LOW quality and under snow.

### 45. Four weathers over the garden, and a fair morning closing into a storm

```json recipe
{"intent": "the example garden at midsummer noon under fair, overcast and stormy skies and on a cloudy sunrise, then a film shot in which the fair sky closes into a storm", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "time.set", "params": {"month": 6, "day": 21, "hour": 13}},
  {"id": "view.set", "params": {"weather": "fair"}},
  {"id": "view.render", "params": {"camera": {"position": [21, 1.7, 31], "target": [6, 6, 13], "fov_y_deg": 65}, "width": 1280, "height": 720, "path": "weather-fair.png"}},
  {"id": "view.render", "params": {"camera": {"position": [21, 1.7, 31], "target": [6, 6, 13], "fov_y_deg": 65}, "width": 1280, "height": 720, "weather": "overcast", "path": "weather-overcast.png"}},
  {"id": "view.render", "params": {"camera": {"position": [21, 1.7, 31], "target": [6, 6, 13], "fov_y_deg": 65}, "width": 1280, "height": 720, "weather": "storm", "path": "weather-storm.png"}},
  {"id": "view.render", "params": {"camera": {"position": [21, 1.7, 31], "target": [6, 6, 13], "fov_y_deg": 65}, "width": 1280, "height": 720, "weather": "cloudy", "time": {"hour": 5.5}, "path": "weather-sunrise.png"}},
  {"id": "view.set", "params": {"weather": "cloudy", "weather_transition_s": 20, "clouds": {"altitude_m": 1200}}},
  {"id": "film.add", "params": {"name": "Front", "fps": 25}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "hold", "eye": [21, 1.7, 31], "target": [6, 6, 13], "fov_y_deg": 65, "duration_s": 8, "weather": ["fair", "storm"]}}},
  {"id": "film.preview", "params": {"id": "f1", "t": 4}},
  {"id": "film.render", "params": {"id": "f1", "dir": "front", "width": 640, "height": 360, "every": 25}}
]}
```

`view.set {"weather": ...}` sets the sky and the air together: `clear` (no clouds, still air),
`fair` (cumulus over a third of the sky, a light breeze), `cloudy` (cumulus over most of it, some
taller, a fresh breeze with gusts), `overcast` (a closed grey layer, soft shadows), `rain` (a low
rain sky, steady rain, everything wet) or `storm` (towering storm clouds, a downpour, strong
gusting wind). It writes the clouds' kind and cover, `rain`, `wetness`, `mist`, `gustiness` and
`wind_speed_mps`; any of them given in the same call wins, and each can be changed on its own
afterwards (the name is not kept). With `weather_transition_s` (0 to 600) the window eases into
every change of those fields instead of switching, so a sky clears or closes in while you watch;
pictures always show the stored weather.

The clouds are volumetric (Medium quality and up): a layer about 2.5 km thick from
`clouds.altitude_m` (1500 m; an overcast or stormy sky lowers it), shaped from a weather map, a
Perlin-Worley noise and a detail noise after the public Nubis talks, lit by the sun through the
cloud and by the sky, with a bright silver lining towards the sun and dark, dense bases under a
storm. The broken part of the sky casts shadows that drift over the ground with the clouds
(`clouds.speed_mps`, by default faster than the wind at the ground); the closed part dims the sun
everywhere, as rain does. `clouds.sun_gap` (true by default) keeps a gap in broken clouds in front
of the sun over the site, so a fair-weather picture keeps the design in the sun; set it to false
for a film in which cloud shadows sweep over the garden. `clouds: {"volumetric": false}` (or Low
quality, or an HDRI sky) draws the flat cloud layer instead. `view.set clouds` merges what it is
given into the current clouds: `{"clouds": {"cover": 0.5}}` changes only the cover.
`view.render` takes `weather` (and `weather_to` with `weather_mix` for a sky between two) and
`clouds` (a cover, or an object as `view.set` takes it) for one picture (the CLI's `--weather`
and `--clouds`); a film shot takes
`weather` `[from, to]`, eased in and out over the shot, and `clouds` (the cover) `[from, to]`. Storm clouds flash with lightning
(recipe 47).

### 46. A flowering meadow beyond a striped lawn, with gusts rolling over both

```json recipe
{"intent": "a mown lawn in stripes by the terrace and a knee-high flowering meadow beyond it, seen at eye level on a June evening with gusts running through the grass", "steps": [
  {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [24, 0], [24, 14], [0, 14]], "material": "lawn"}},
  {"id": "shape.add_area", "params": {"name": "Meadow", "outline": [[0, 14], [24, 14], [24, 44], [0, 44]], "material": "meadow"}},
  {"id": "material.update", "params": {"id": "lawn", "kind": {"type": "lawn3d", "density_per_m2": 2000, "blade_height_m": 0.05, "seed": 1, "stripes_m": 1.5, "stripes_deg": 0}}},
  {"id": "material.update", "params": {"id": "meadow", "kind": {"type": "lawn3d", "density_per_m2": 1600, "blade_height_m": 0.55, "seed": 2, "flowers": 0.5}}},
  {"id": "planting.add_single", "params": {"name": "Oak", "asset": "flora:quercus-robur", "position": [17, 0, 30]}},
  {"id": "time.set", "params": {"month": 6, "day": 12, "hour": 19.5}},
  {"id": "view.render", "params": {"camera": {"position": [12, 1.6, -3], "target": [12, 0.4, 18], "fov_y_deg": 55}, "width": 1280, "height": 720, "gustiness": 0.6, "animation_s": 4, "path": "meadow.png"}}
]}
```

A lawn material (`lawn3d`) grows real blades at MEDIUM quality and up: every frame the GPU
generates them round the camera over every face that carries one, in clumps that share a height
and lean out of their centre, densest near the camera and thinning and widening with the
distance until they sink into the lawn's own texture (about 10 m for a mown lawn at HIGH, three
times as far for a meadow). `blade_height_m` sets the grass from a mown lawn (0.03 to 0.06) to a
meadow (0.3 to 0.8, up to 1.2); taller grass has fewer, broader blades and seed stems over them.
`flowers` (0 to 1) scatters small wild flowers among the blades from April to September, in the
colours of the month (buttercups and daisies in spring, clover, knapweed and cornflowers in
summer). The palette's `meadow` is such a material (0.45 m, flowers 0.35); `shape.add_area`
adds it to an older project that lacks it. The blades take the sun's shadow and the sky they see,
glow when the sun is behind them, are paler at the foot and darker deep in the sward, dry out
in winter (and a meadow from midsummer), lean the way the mower went in `stripes_m` bands, and
bow over in the gusts (`gustiness`) as paler waves. No grass grows under the areas drawn on
top of a lawn (a path, a terrace, a pond), and it thins under tree crowns. At LOW quality a lawn
is drawn as before, with scattered tufts.

A terrain grows the blades too, wherever its drawn layers are a lawn or a meadow: a base of
`lawn` under `terrain.set_auto` has grass on the gentle ground and none where rock, scree, moss
or soil take over, the blades thinning, shortening and fraying into tufts across the blend; paint
a `meadow` layer (`terrain.add_layer`, `terrain.stroke` `paint`) for long grass, painted to
about half its weight for grass in tussocks with the ground between. Nothing grows under the
areas, terraces and models on the terrain, below the sea's level or on slopes past about 45
degrees. On a terrain wider than 128 m the grass is grown in a 128 m square round the camera,
made again as the camera moves on. The library's lawn and meadow ground have a height map of
clumped grass (Grass 005), so scree and soil reach into the grass between its tussocks rather
than meeting it along a line. A lawn material's `blade_color` ([r, g, b]) gives the blades their
own colour where they are not the ground's: grey-green dune grass over sand, as on the `coast`
example's dunes.

### 47. A thunderstorm at dusk: lightning over the garden, and a film shot of flashes

```json recipe
{"intent": "the example garden under a thunderstorm on a July evening, one picture at the moment of a flash with a bolt far off, then a film shot in which the flashes come more and more often", "steps": [
  {"id": "project.load_example", "params": {"name": "garden"}},
  {"id": "time.set", "params": {"month": 7, "day": 15, "hour": 20.5}},
  {"id": "view.set", "params": {"weather": "storm", "clouds": {"lightning": 0.8}}},
  {"id": "view.render", "params": {"camera": {"position": [21, 1.7, 31], "target": [60, 13.7, -61], "fov_y_deg": 65}, "width": 1280, "height": 720, "animation_s": 151.74, "path": "storm-flash.png"}},
  {"id": "film.add", "params": {"name": "Storm", "fps": 25}},
  {"id": "film.add_shot", "params": {"id": "f1", "shot": {"kind": "hold", "eye": [21, 1.7, 31], "target": [60, 13.7, -61], "fov_y_deg": 65, "duration_s": 10, "lightning": [0.3, 1.0]}}},
  {"id": "film.preview", "params": {"id": "f1", "t": 5}},
  {"id": "film.render", "params": {"id": "f1", "dir": "storm", "width": 640, "height": 360, "every": 5}}
]}
```

Storm clouds (`weather: "storm"`, or `clouds.kind: "storm"`) flash with lightning: a flash lights
the clouds from inside for a few frames and the garden as a bright sky over it would, and about
half of the flashes more than 3 km away show a bolt, a jagged channel with branches from the
cloud base to the ground, bright enough to show through the rain. `view.set clouds.lightning`
sets how often, 0 (never) to 1 (every few seconds; 0.5 by default); a sky between two weathers
flashes as much as it is stormy. The flashes follow the animation clock alone: `view.render
animation_s` picks the moment (a picture at another moment shows the storm between flashes),
and a film shows the same flashes every time it is drawn. Raising the frequency keeps every
flash and adds more. A film shot takes `lightning` `[from, to]`, and `view.render` takes
`lightning` for one picture. Undo takes a change of `clouds.lightning` back like any other
view setting.

## Known traps

- **Ids are not names**: `S2`, not `House`; viewpoints are `v1`, lights `li1`. Film commands and
  `viewpoint.add` take either (a name is matched without regard to case; see recipe 42).
- **`exposure_ev` is two things**: `view.set {exposure_ev}` is an absolute EV100 (0 is a white frame
  in daylight; null is automatic); a film shot's `exposure_ev` and `view.render`'s
  `exposure_offset_ev` are stops relative to the automatic exposure (0 is no correction).
- **A batch is not a transaction by default**: after a failed step, the earlier ones are applied.
  Inspect, fix and send only the rest, or use `"atomic": true`. `file.new`, `file.open`,
  `edit.undo` and `edit.redo` cannot be part of an atomic batch.
- **Unknown keys are refused** with the nearest name (`unknown parameter \`lumen\` for
  \`light.add\`; did you mean \`lumens\`?`), except where a command takes any field
  (`openParams: true`, e.g. `planting.update`).
- **Models are files**: `model.add` is refused when its file cannot be read; `source` is relative
  to the project file (or the read root). A model whose file moved is pointed at its new place
  with `model.set_source` (one undo step; materials and placement stay).
- **`file.open` and `file.new` start a new history** (nothing to undo).
- **Pictures need a GPU.** Without one `view.render` is an error that says so; everything else
  works. The first picture takes a few seconds (the device, then the plants).
- **Light markers** (small yellow crosses) are drawn in pictures where lamps are.
- **Commands of an extension live in its window only.** A build made with
  `visioncraft::Shell::new().with(...)` adds its extensions' commands to that window's session:
  reach them with `visioncraft --control` (or `visioncraft-cli mcp --bridge`) against that build,
  and open an extension's mode with `ui.set {"mode": "<extension id>"}`. `visioncraft-cli` on its
  own and `visioncraft --agents-headless` build their session from the core commands and cannot
  know a third-party build's commands, by design (nothing is loaded at run time).
- **Embedding: hide the built-in UI.** An app built on VisionCraft that brings its own chrome
  hides parts of the window with `Shell::new().host_ui(HostUi {..})` or an extension's
  `fn host_ui(&self) -> Option<HostUi>` (the shell's option wins, else the first extension that
  returns `Some`). `HostUi` has `tabs`, `file_bar`, `view_bar`, `mode_panel_frame` (all true by
  default; false draws the open mode's panel without frame and fold button), `start_mode` (a
  built-in mode id or an extension id) and `quiet_missing_assets` (no startup notice when there
  is no asset library), `agent_toggle` (the *Allow AI agents* box on the desktop, the
  *Experimental: browser AI agent (WebMCP)* box in the browser; hiding it only hides the box, it
  does not switch the agent bridge on or off and the browser agent's questions still appear) and
  `replace_prompt: Option<ReplacePrompt {title, body, confirm, cancel}>`, the wording of the
  *Replace the current project?* question the person's own Open or New asks when there are
  unsaved changes (`None`: the built-in text, no title). At run time
  `ui.set {"tabs", "file_bar", "view_bar", "mode_panel_frame", "agent_toggle"}` changes the flags,
  `ui.set {"replace_prompt": {"title", "body", "confirm", "cancel"}}` the question (keys left out
  keep the built-in text; `null` restores it) and `ui.get` reports them under `ui`, so an
  extension's own "view in 3D" button can show the bars and switch back. Hiding only hides buttons: every action stays a command and
  Ctrl+Z, Ctrl+Shift+Z and `ui.set {"mode"}` keep working.
- **Embedding: undo state and unsaved changes.** `edit.state` (`{}`) returns
  `{can_undo, can_redo, dirty, revision}`. `dirty` is true when the project differs from the
  project as last opened, created or saved, so undoing back to that point is clean again and a
  redo makes it dirty; `revision` grows with every change, undo and redo. It works headless
  (`visioncraft-cli`) and in the window, where a save, open or new from the person's own buttons
  counts too, and it is not noted in the agent log. An extension reads the same with
  `ExtensionCx::edit_state()`.
- **Embedding: opening without the question.** The commands never ask: `file.open`, `file.new`
  and `project.load_example` replace the project at once (the browser agent's own confirmation
  is a separate step). Only the person's Open and New ask before dropping unsaved changes. An
  extension that asked already, or has nothing to lose, calls
  `ExtensionCx::open_project(name, bytes)` (a `.visioncraft` or bundle `.zip`; no question) or
  `ExtensionCx::open_example(name, title)`; `ExtensionCx::open_project_asking` asks as the
  person's Open does, with the host's `replace_prompt` wording. The window runs these after the
  frame.
- **Embedding: a generated mesh.** An extension can show a mesh it made, without a server:
  `ExtensionCx::import_object(name, glb_bytes)` checks the `.glb`, keeps its bytes in the window
  and runs `object.add` (one undo step). It returns `user:<id>`, the asset reference that
  `placement.add` and the other commands take; a bad file is an `Err` with a message.
