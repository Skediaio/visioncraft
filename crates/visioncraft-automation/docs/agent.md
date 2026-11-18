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
   `query.measure`, `query.quantities {ids?, layer?}` (areas, lengths, volumes and plant counts per object, per material and per asset), `query.raycast`, `query.sun_hours {point | outline, season | date, cell_m?, years?, plants?}` (hours of direct sun at a spot or as a grid over an area, with shadow from models, shapes, terrain and plant crowns), `query.ground_height`, `query.ground_length`, `query.sun`.
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
{"quality": "photo", "samples": 64}`: many frames averaged at ULTRA give soft shadows with real
penumbrae and smooth edges (seconds to minutes); `aperture_m` (lens radius, e.g. 0.02) and
`focus_m` add depth of field.

Lamps and lit rooms follow the sun: on after sunset at the time shown, off before. `view.set
{"lights": true}` or `false` fixes them for the project (the window's Lights button does the same),
`null` returns to the sun's rule; every picture, from the window, the CLI or `view.render`, uses it.

Wind is in real units: `view.set {"wind_speed_mps": 6.7, "wind_gustiness": 0.5,
"wind_direction_deg": 90}`, or a Beaufort step as a preset, `{"wind_beaufort": 4}` or
`{"wind_beaufort": "gale"}` (0 calm to 10 storm; v = 0.836 B^1.5 m/s). At 0 the air is perfectly
still; plants lean with the drag (growing with the speed squared, saturating as crowns streamline),
leaves flutter faster as the wind rises, and each plant sways at its own natural frequency (tall
trees slowly, shrubs and grasses fast). The old relative `wind_strength` (1 = 6.5 m/s) is still
read from older projects and accepted by `view.set`, as a deprecated alias. To see motion in
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
ground under its line, not following the slope.

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
ground under its footprint, like a block does.

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
  A round raised bed is `bed.add` with such an outline and `curves` (plain, masonry and corten
  styles; sleepers need straight edges).
- **Editing**: `shape.insert_vertex` drops the arc of the edge it splits; `shape.remove_vertex` drops
  the arcs of the two edges it joins; later arcs keep their edges (renumbered). Moving a vertex
  eases an arc that would bulge more than half its new chord to a semicircle.
- **Faces keep their meaning**: a curved edge `n` of an extrusion is many triangles, all
  `<id>:side(n)`. Stairs and houses refuse curves (the message says so); a fence follows them:
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

## Known traps

- **Ids are not names**: `S2`, not `House`; viewpoints are `v1`, lights `li1`.
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
