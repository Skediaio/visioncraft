# Driving VisionCraft as an agent

VisionCraft shows a house and garden as they will look: models (from 3DCraft or OBJ), shapes you
draw (lawns, terraces, house blocks, walls), materials, procedural plants, lights and viewpoints,
under the sun of a real site and date. Everything is a command with an id and JSON params, the
same over MCP (`visioncraft-cli mcp`), JSON lines (`visioncraft-cli serve`) and the command line
(`visioncraft-cli run`), in the manner of 3DCraft. The protocol is in
[`control-protocol.md`](control-protocol.md).

## Conventions

- **Metres, Y up, +X east, -Z north** (+Z is south). A house "north of the lawn" has smaller z.
- **Outlines and paths are `[[x, z], ...]`** on the ground; positions are `[x, y, z]`.
- **Ids** are a prefix and the lowest free number: shapes `S1`, models `m1`, generators `g1`,
  lights `li1`, viewpoints `v1`, materials `mat1`, custom species `sp1`. Every `add` returns
  `{id}`. Renders take a viewpoint's id or its name.
- **Face names**: a drawn area has `S1:top`; an extrusion `S2:top`, `S2:bottom`, `S2:side(0)` …
  (side n runs from outline point n to n+1), an open one also `S2:start` and `S2:end`. A 3DCraft
  model keeps its persistent names, `F2:side(e7)`, `F2:endCap(e5,e6,e7,e8)`.
  `document.inspect` lists them under `faces`.
- **Materials**: built in `plaster`, `brick`, `concrete`, `timber`, `roof`, `glass`, `water`,
  `lawn`, `gravel`, `paving`, `metal`, `soil`. A face assignment beats a body one, which beats
  the owner's.
- **Asset ids** in a palette: `flora:<species>` for a built-in procedural plant
  (`species.list {category}`), a custom species of the project (`sp1`), or a catalogue asset id
  (`catalog.search {query}`).
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
   `query.measure`, `query.raycast`, `query.ground_height`, `query.ground_length`, `query.sun`.
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
  {"id": "shape.add_area", "params": {"name": "Pond water", "outline": [[-4, -3], [4, -3], [5, 2], [-3, 4]], "elevation": -0.3, "material": "water"}},
  {"id": "query.ground_height", "params": {"x": 0, "z": 0}}
]}
```

The water surface sits 0.3 m below the lawn, 0.5 m above the pond bed.

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

## Known traps

- **Ids are not names**: `S2`, not `House`; viewpoints are `v1`, lights `li1`.
- **A batch is not a transaction by default**: after a failed step, the earlier ones are applied.
  Inspect, fix and send only the rest, or use `"atomic": true`. `file.new`, `file.open`,
  `edit.undo` and `edit.redo` cannot be part of an atomic batch.
- **Unknown keys are refused** with the nearest name (`unknown parameter \`lumen\` for
  \`light.add\`; did you mean \`lumens\`?`), except where a command takes any field
  (`openParams: true`, e.g. `planting.update`).
- **Models are files**: `model.add` is refused when its file cannot be read; `source` is relative
  to the project file (or the read root).
- **`file.open` and `file.new` start a new history** (nothing to undo).
- **Pictures need a GPU.** Without one `view.render` is an error that says so; everything else
  works. The first picture takes a few seconds (the device, then the plants).
- **Light markers** (small yellow crosses) are drawn in pictures where lamps are.
