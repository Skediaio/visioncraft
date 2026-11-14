# Beach house

A furnished single-storey beach house, modelled as a parametric B-rep in
[3DCraft](https://github.com/johancoppens/3dcraft) (v0.1.1) for VisionCraft's `beach-house`
example (`project.load_example {"name": "beach-house"}`). Made for this project, MIT.
The plan, room schedule and dimensions are in [brief.md](brief.md).

| File | What it is |
|---|---|
| `generate.py` | The source. Parameters at the top (width, depth, ceiling height, overhangs, glass wall bays, open bays, deck). Python 3, standard library only. |
| `beach-house.steps.json` | Generated: the 3DCraft command script (blocks, patterns, booleans, colours). |
| `bodies.json` | Generated: body name -> 3DCraft feature id, the ids VisionCraft assigns materials to. |
| `../../../crates/visioncraft-core/src/examples/beach-house.3dcraft` | Generated from the script by `craft3d-cli`. Do not edit by hand. |
| `../../../crates/visioncraft-core/src/examples/beach-house.json` | Generated: the VisionCraft example (materials per body, furniture, lights, site, viewpoints). |

## Regenerate

```sh
cd examples/models/beach-house
python3 generate.py
craft3d-cli run --new '{"units":"mm"}' --script beach-house.steps.json \
    --out ../../../crates/visioncraft-core/src/examples/beach-house.3dcraft
```

The last step of the script is `document.inspect`: every body should say `"valid": true`, and
its `feature` should match `bodies.json`. A quick look in 3DCraft itself:
`craft3d-cli render ../../../crates/visioncraft-core/src/examples/beach-house.3dcraft --out house.png --look studio`.

Then `cargo test -p visioncraft-core examples` checks the counts. If a change adds or removes
bodies, furniture or lights, update the numbers in `the_beach_house_builds_furnished_and_lit`.

## How it is built

- Axes: 3DCraft millimetres, X east, Y north, Z up; the sea is to the south. VisionCraft turns
  that into metres, Y up, with the sea side along z = 0 and the deck towards +z. Place it with
  `model.set_transform` (a translation, and `rotation_y_deg` to face another sea).
- Everything is axis-aligned `feature.block`s, two `feature.pattern`s (deck boards, path
  pavers) and `feature.boolean`s: openings are cut from the wall rings in one subtraction,
  door openings from the inner walls, basins from the baths and the fire pit.
- Each element group is joined into **one body** (a chain of unions), so a material is one
  `material.assign` per group: 46 bodies, about 1,900 faces, 6,448 triangles in VisionCraft.
  Separate parts stay separate closed shells inside their body: each glass pane is its own
  10 mm shell in the `glass` body.
- Face names are 3DCraft's persistent names (`F674:copy(0)/F177:copy(1)/.../F133:face(4)`,
  at most 64 characters); assign by body unless you need one face.
- The generator asserts its own inputs (positive sizes, heights in range) and that every body
  has a material in the VisionCraft example.
