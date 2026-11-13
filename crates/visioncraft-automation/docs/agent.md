# Driving VisionCraft as an agent

VisionCraft shows a house and garden as they will look: models (from 3DCraft or OBJ), shapes you
draw (lawns, terraces, house blocks, walls), materials, procedural plants, lights and viewpoints,
under the sun of a real site and date. Everything is a command with an id and JSON params, the
same over MCP (`visioncraft-cli mcp`), JSON lines (`visioncraft-cli serve`) and the command line
(`visioncraft-cli run`). The protocol is in [`control-protocol.md`](control-protocol.md).

## Conventions

- **Metres, Y up, +X east, -Z north** (so +Z is south). A house "north of the lawn" has smaller z.
- **Outlines and paths are `[[x, z], ...]`** on the ground; positions are `[x, y, z]`.
- **Ids** are a prefix and the lowest free number: shapes `S1`, models `m1`, generators `g1`,
  lights `li1`, viewpoints `v1`. Every `add` returns `{id}`. Commands that take a viewpoint also
  take its name.
- **Face names**: a drawn area has `S1:top`; an extrusion `S2:top`, `S2:bottom`, `S2:side(0)` …
  (side n runs from outline point n to n+1), an open one also `S2:start`, `S2:end`. A 3DCraft
  model keeps its persistent names, `F2:side(e7)`, `F2:endCap(e5,e6,e7,e8)`.
  `document.inspect` lists them under `faces`.
- **Materials**: built in `plaster`, `brick`, `concrete`, `timber`, `roof`, `glass`, `water`,
  `lawn`, `gravel`, `paving`, `metal`, `soil`. `material.assign {target: {type: owner|body|face,
  owner, body?, face?}, material}`; a face assignment beats a body one, which beats the owner's.
  `shape.add_area` and `shape.add_extrusion` take `material` to assign it at once.
- **Plants** (an `asset` in a palette): `flora:<species>` for the built-in procedural species
  (`species.list {category: "hedge"}` lists them: trees, conifers, palms, shrubs, hedges,
  perennials, ferns, grasses, ground cover, climbers), the id of a custom species of the project
  (`species.add`), or a catalogue asset id (`catalog.search {query: "bench"}`).
- **Sun**: `site.set {latitude, longitude, utc_offset_hours}` and `time.set {year?, month?, day?,
  hour?}`, local time; the offset is fixed, so give summer time yourself (Belgium in June: 2).
  `query.sun` says where the sun is and when it rises and sets.

## The working loop

1. **Look first.** `doc_inspect {"faces": false}` (or `info FILE`). `command_list {"brief":
   true}`, then `command_list {"id": ...}` for each command you will use: its parameters, units,
   defaults and an example.
2. **Plan in numbers.** Every outline, height, plant spacing and light position, written down
   before the first command, in metres in the axes above.
3. **Build in batches.** `command_batch` is all or nothing: if step 4 fails, steps 0 to 3 are
   taken back and the report says `rolledBack: true` and `step 4: <why>`. Fix that step and send
   the whole batch again. A batch that succeeds is one undo step (`edit.undo`).
4. **Check without a picture.** `doc_inspect`: `area_m2` of areas, `footprint_m2` of blocks,
   `count` of plants per generator, positions. `document.measure` for lengths and areas,
   `query.raycast` and `query.ground_height` for what is where.
5. **Look.** `doc_render_preview {"viewpoint": "From the gate"}`, or a `camera {position,
   target}`. The picture's JSON says the camera, the sun and `plantsPending` (0: all plants
   drawn). `time` in the preview changes the picture only; `time.set` changes the project.
6. **Save.** `command_run file.save {"path": "garden.visioncraft"}` (beneath the write root).

## A worked example

Every line below was run (`docs/examples/mcp_demo.py`, and `docs/examples/garden.json` in the
tests). Requests are MCP `tools/call`s, shown as `tool {arguments}` → result.

```text
command_run {"id": "site.set", "params": {"latitude": 51.05, "longitude": 3.72, "utc_offset_hours": 2}}  → null
command_run {"id": "time.set", "params": {"year": 2026, "month": 6, "day": 21, "hour": 15}}             → null
command_run {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[-10, -2], [10, -2], [10, 10], [-10, 10]], "material": "lawn"}}
  → {"id": "S1"}
command_run {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[-5, -10], [5, -10], [5, -3], [-5, -3]], "height": 6, "material": "brick"}}
  → {"id": "S2"}
command_run {"id": "species.list", "params": {"category": "hedge", "filter": "carpinus"}}
  → {"species": [{"asset": "flora:carpinus-hedge", "common_name": "Hornbeam hedge (1 m segment)", "mature_height_m": 1.8, ...}], "custom": []}
command_run {"id": "planting.add_spline", "params": {"name": "Hedge", "asset_palette": [{"asset": "flora:carpinus-hedge", "weight": 1}],
             "points": [[-10, 10], [10, 10]], "spacing": {"every": 0.8}}}
  → {"id": "g1"}
command_batch {"steps": [
  {"id": "light.add", "params": {"name": "Garden lamp", "position": [3, 2.2, -1], "lumens": 1200}},
  {"id": "viewpoint.add", "params": {"name": "From the gate", "position": [12, 2.2, 16], "target": [0, 2, -4]}}]}
  → {"completed": 2, "failed": 0, "rolledBack": false, "results": [{"ok": true, "result": {"id": "li1"}}, {"ok": true, "result": {"id": "v1"}}]}
doc_inspect {"faces": false}
  → ... "shapes": [{"id": "S1", "area_m2": 240.0, ...}, {"id": "S2", "footprint_m2": 70.0, ...}],
        "generators": [{"id": "g1", "type": "spline_mass", "assets": ["flora:carpinus-hedge"], "count": 26, "truncated": false}] ...
doc_render_preview {"viewpoint": "From the gate", "max_side": 960}
  → [image] {"camera": {"position": [12, 2.2, 16], "target": [0, 2, -4], "fov_y_deg": 60}, "plantsPending": 0, "renderer": "gpu",
             "sun": {"azimuth_deg": 213.9, "elevation_deg": 59.0, "sunset_hour": 22.06, ...}}
command_run {"id": "time.set", "params": {"hour": 21.75}}   → null      (dusk: about 20 minutes before sunset)
doc_render_preview {"viewpoint": "From the gate", "max_side": 960}   → [image] {"sun": {"elevation_deg": 1.8, ...}}
command_run {"id": "file.save", "params": {"path": "garden.visioncraft"}}   → {"path": ".../garden.visioncraft", "bytes": 7717}
```

The same garden from the command line, and a picture of it:

```sh
visioncraft-cli run --script docs/examples/garden.json --out garden.visioncraft
visioncraft-cli render garden.visioncraft --out garden.png --viewpoint "From the gate" --time 21:45
```

## Known traps

- **Ids are not names.** `S2`, not `House`; viewpoints are `v1` (not `vp1`), lights `li1`.
  `view.render` and `doc_render_preview` also take a viewpoint's name.
- **`spacing` is an object**: `{"every": 0.8}` (metres) or `{"count": 20}`.
- **Unknown keys are refused** (`light.add: unknown parameter \`lumen\`; its parameters are: ...`),
  except where a command takes any field (`planting.update {id, ...}`).
- **Updates change named fields only**: `light.update {id, lumens}`; `planting.update {id, ...}`
  takes any field of the generator as `doc_inspect {"full": true}` shows it.
- **A batch makes ids in order**: the second `shape.add_area` of a batch on a project with `S1` is
  `S2`; you can use `S2` in a later step of the same batch.
- **Undo after a batch takes the whole batch back.** `file.open` and `file.new` start a new
  history (nothing to undo).
- **Models are files**: `model.add {name, kind: "3dcraft"|"obj", source}` is refused when the
  file cannot be read; `source` is relative to the project file (or the read root). Over MCP the
  server reads beneath `--automation-read-root` only.
- **Pictures need a GPU.** Without one, `doc_render_preview` is a tool error that says so;
  everything else works. The first picture takes a few seconds (the device, then the plants).
- **Light markers** (small yellow crosses) are drawn in pictures where lamps are.
- **Core parameter docs are derived** from one-line descriptions (`derivedDocs: true`): when a
  type says `any`, follow the `summary` and the `example`.
