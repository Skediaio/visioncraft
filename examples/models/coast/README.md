# Coast

The `coast` example (`project.load_example {"name": "coast"}`): the furnished beach house from
[../beach-house](../beach-house) on a mainland beach in southern California, with its coastal
garden, the dunes inland, the sea with its coastline and fifteen viewpoints. Made for this
project, MIT.

| File | What it is |
|---|---|
| `generate.py` | The source: terrain, shore, sea, hard landscape, planting and viewpoints as VisionCraft commands. Python 3, standard library only. |
| `../../../crates/visioncraft-core/src/examples/coast.json` | Generated: the example's own part. It runs after `beach-house-furnished.json` (the house, its materials, furniture and lights). Do not edit by hand. |

## Regenerate

```sh
python3 examples/models/coast/generate.py
cargo test -p visioncraft-core examples
```

The test `the_coast_is_a_mainland_beach_with_the_house_low_by_the_water` checks the levels (the
house plot, the waterline, the endless ground), the hard landscape and that no planting stands
in the water or in the view from the glass wall; update it with the design.
