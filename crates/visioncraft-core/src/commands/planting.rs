use super::{check_layer, fail, merge_update, next_id};
use crate::planting::{
    AreaScatter, Cluster, Generator, MixedBrush, PlantLayer, Single, Spacing, SplineMass, Stroke,
    SurfaceFill, Weighted,
};
use crate::presets::planting_presets;
use crate::project::{LAYER_PLANTING, Project};
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    let name = || param("name", T::String, "Display name.").required();
    let layer = || param("layer", T::String, "Layer id.").default(json!("planting"));
    let seed = || {
        param(
            "seed",
            T::Integer,
            "Random seed; the same generator and seed always give the same plants. By default the generator's number.",
        )
    };
    let palette = |required: bool| {
        let p = param(
            "asset_palette",
            T::Array,
            "Species to pick from by weight: [{asset, weight >= 0}]. An asset is a catalogue asset id, `flora:<species id>` a custom species id (`sp1`...) or `user:<object id>`.",
        );
        if required {
            p.required()
        } else {
            p.default(json!([]))
        }
    };
    let scale_min =
        || param("scale_min", T::Number, "Smallest scale factor, above zero.").default(json!(1));
    let scale_max = || {
        param(
            "scale_max",
            T::Number,
            "Largest scale factor, at least `scale_min`.",
        )
        .default(json!(1))
    };
    let spin = |d: bool| {
        param(
            "random_rotation",
            T::Boolean,
            "Give each plant a random turn about the vertical.",
        )
        .default(json!(d))
    };
    let min_distance = || {
        param(
            "min_distance_m",
            T::Number,
            "No two plants closer than this; 0 for no limit.",
        )
        .unit("m")
        .default(json!(0))
    };
    let density = |required: bool| {
        let p = param(
            "density_per_m2",
            T::Number,
            "Plants per square metre, above zero (not needed when `layers` are given).",
        )
        .unit("1/m2");
        if required {
            p.required()
        } else {
            p.default(json!(0))
        }
    };
    let layers = || {
        param(
            "layers",
            T::Array,
            "Storeys [{name, asset_palette, density_per_m2, min_distance_m, scale_min, scale_max, random_rotation, keep_clear_m}], evaluated in order on their own random streams; when given they replace the palette, density and scale above. `keep_clear_m` drops plants closer than that to any earlier layer's.",
        )
        .default(json!([]))
    };
    let jitter = || {
        param(
            "position_jitter",
            T::Number,
            "0..1: how far a plant strays from its grid cell's centre, as a fraction of the cell.",
        )
        .default(json!(1))
    };
    s.register_with_spec(
        spec(
            "planting.add_single",
            "Place Plant",
            "Places one plant or asset at a position.",
            vec![
                name(),
                param(
                    "asset",
                    T::String,
                    "Catalogue asset id, `flora:<species id>`, custom species id or `user:<object id>`.",
                )
                .required(),
                param("position", T::Point3, "[x, y, z] of its base.")
                    .unit("m")
                    .required(),
                param(
                    "rotation_y_deg",
                    T::Number,
                    "Turn about +Y, counter-clockwise seen from above.",
                )
                .unit("deg")
                .default(json!(0)),
                param("scale", T::Number, "Scale factor, above zero.").default(json!(1)),
                layer(),
            ],
            "{id} (g1, g2...)",
            json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [4, 0, 3]}),
        ),
        add_single,
    );
    s.register_with_spec(
        spec(
            "planting.add_spline",
            "Plant Along Curve",
            "Plants along a smooth (Catmull-Rom) curve through the points, spaced by arc length: hedges, borders, tree rows.",
            vec![
                name(),
                palette(true),
                param("points", T::Outline, "Control points as [x, z] pairs in metres, at least 2.").required(),
                param("spacing", T::Object, "{count: n} items (ends included on an open line) or {every: metres}.").required(),
                param("closed", T::Boolean, "Join the last point back to the first.").default(json!(false)),
                param("straight", T::Boolean, "Straight segments between the points instead of a smooth curve through them.").default(json!(false)),
                param("spacing_jitter", T::Number, "0..1: how far an item slides along the line, as a fraction of the spacing.").default(json!(0)),
                param("offset_m", T::Number, "Sideways shift, positive to the right of the direction of travel seen from above.").unit("m").default(json!(0)),
                scale_min(),
                scale_max(),
                spin(false),
                seed(),
                param("avoid_buildings", T::Boolean, "Leave out plants that would stand on or in a building: on a roof, under a canopy lower than the plant, inside a block.").default(json!(true)),
                layer(),
            ],
            "{id}",
            json!({"name": "Hedge", "asset_palette": [{"asset": "flora:carpinus-hedge", "weight": 1}], "points": [[0, 0], [5, 0], [10, 2]], "spacing": {"every": 1.0}}),
        ),
        add_spline,
    );
    s.register_with_spec(
        spec(
            "planting.add_area",
            "Scatter in Area",
            "Scatters plants inside a polygon on a jittered grid anchored at the world origin, so editing the outline far away leaves the other plants where they are.",
            vec![
                name(),
                palette(false),
                param("outline", T::Outline, "Polygon as [x, z] pairs in metres, at least 3.").required(),
                density(false),
                layers(),
                jitter(),
                scale_min(),
                scale_max(),
                spin(true),
                min_distance(),
                seed(),
                param("avoid_buildings", T::Boolean, "Leave out plants that would stand on or in a building: on a roof, under a canopy lower than the plant, inside a block.").default(json!(true)),
                param("edge_fade_m", T::Number, "A soft edge in metres: the plants thin out over this width inside the outline along a wavering line, as wild planting ends; 0 a hard edge.").default(json!(0.0)),
                layer(),
            ],
            "{id}",
            json!({"name": "Bed", "asset_palette": [{"asset": "flora:lavandula-angustifolia", "weight": 1}], "outline": [[0, 0], [6, 0], [6, 4], [0, 4]], "density_per_m2": 3.0}),
        ),
        add_area,
    );
    s.register_with_spec(
        spec(
            "planting.add_border",
            "Add Border",
            "Plants a layered perennial border that closes up, from a curated species list per style: tall species at the back, short at the front, in drifts of a few to a dozen with a low matrix species between them, spaced from each species' mature width so the crowns touch at maturity (a young border is open, a three-year-old one closed: see `view.set years`). It is an area scatter with a `border` (change it with `planting.update`, `planting.growth` shows how its species grow). The front is the side the viewer stands: `facing` or `front` says it, else the side away from the nearest wall, building or hedge within 2.5 m of the outline, else the outward side of the outline's longest edge. Low plants of the front row may stand up to 0.2 m over the front edge so their crowns lie over it and no soil strip shows; the back and sides keep their clearance. Deterministic by `seed`.",
            vec![
                param("name", T::String, "Display name.").default(json!("Border")),
                param("outline", T::Outline, "Polygon as [x, z] pairs in metres, at least 3. Give this or `area`."),
                param("area", T::String, "The id of an area shape (`S1`...) to plant over its outline. Give this or `outline`."),
                param("style", T::String, "The species list: cottage (spires, daisies and sages in the sun), prairie (grasses and late daisies), shade (hostas, astilbes, ferns, hellebores) or dry (lavender, sages, stonecrops, fine grasses). `auto` (a host only: the window, CLI or MCP server) measures the mean hours of direct sun over the outline on the summer solstice, as `query.sun_hours` does, and picks shade under 3 h, cottage from 3 to 6 h, and prairie above 6 h (dry above 6 h when `dry` is true); the answer says which and why. A host also adds a `light_fit` warning when an explicit style clearly mismatches the measured light (a shade border in 6+ h, a sun border in under 3 h).")
                    .required()
                    .allowed(&["cottage", "prairie", "shade", "dry", "auto"]),
                param("dry", T::Boolean, "With `style: auto`: choose dry instead of prairie where the border is sunny (lean soil, little water).").default(json!(false)),
                param("front", T::Array, "Indices of the outline's edges that face the viewer (edge `i` runs from point `i` to `i + 1`): the front is their mean outward direction."),
                param("facing", T::Point2, "The front as a direction [x, z] from the back of the border to its front. Wins over `front`."),
                param("height_range", T::Array, "[front, back]: the mature plant height in metres wanted at the front edge and at the back edge, e.g. [0.4, 1.6]; species are chosen by how close their mature height is. By default the style's."),
                param("species", T::Array, "Species ids to use instead of the style's, each `flora:<id>` (built-in, see `species.list`) or a custom species id; low spreading ones (ground cover, or up to 0.5 m and wider than high) are the matrix between drifts."),
                param("closure", T::Number, "Plants per m2 = closure / mature width squared, per species: 1 crowns just touch, more is denser.").default(json!(1.3)),
                param("drift_m", T::Number, "Typical length of a drift along the border in metres (1..10).").unit("m").default(json!(2.4)),
                seed(),
                param("avoid_buildings", T::Boolean, "Leave out plants that would stand on or in a building.").default(json!(true)),
                layer(),
            ],
            "{id, facing: [x, z], height_range: [front, back], species: [asset...], plants}",
            json!({"name": "Cottage border", "outline": [[0, 0], [10, 0], [10, 2], [0, 2]], "style": "cottage", "facing": [0, 1]}),
        ),
        add_border,
    );
    s.register_with_spec(
        spec(
            "planting.add_brush",
            "Start Brush",
            "Starts a brush generator: strokes add and erase coverage in order, plants fill the covered ground. Rotation is always random.",
            vec![
                name(),
                palette(false),
                density(false),
                min_distance(),
                layers(),
                param("strokes", T::Array, "Initial strokes [{points: [[x, z]..], radius_m, erase}].").default(json!([])),
                param("clip_outline", T::Outline, "Plants only grow inside this outline (an area's), when given.").default(json!([])),
                param("clip_holes", T::Array, "Outlines plants never grow in (areas lying on the clip area).").default(json!([])),
                scale_min(),
                scale_max(),
                seed(),
                param("avoid_buildings", T::Boolean, "Leave out plants that would stand on or in a building: on a roof, under a canopy lower than the plant, inside a block.").default(json!(true)),
                layer(),
            ],
            "{id}",
            json!({"name": "Woodland", "asset_palette": [{"asset": "flora:quercus-robur", "weight": 1}], "density_per_m2": 0.02, "strokes": [{"points": [[0, 0], [10, 0]], "radius_m": 4.0, "erase": false}]}),
        ),
        add_brush,
    );
    s.register_with_spec(
        spec(
            "planting.brush_stroke",
            "Brush Stroke",
            "Appends a stroke to a brush generator.",
            vec![
                param("id", T::String, "Brush generator id.").required(),
                param(
                    "points",
                    T::Outline,
                    "Path of the stroke as [x, z] pairs in metres, at least 1.",
                )
                .required(),
                param("radius_m", T::Number, "Brush radius, above zero.")
                    .unit("m")
                    .required(),
                param("erase", T::Boolean, "Remove coverage instead of adding it.")
                    .default(json!(false)),
            ],
            "null",
            json!({"id": "g1", "points": [[2, 2], [8, 3]], "radius_m": 1.5}),
        ),
        brush_stroke,
    );
    s.register_with_spec(
        spec(
            "planting.add_surface_fill",
            "Fill Surface",
            "Fills named faces of a model or shape with plants, by triangle area, on the surface; triangles steeper than `max_slope_deg` are skipped. Stores face names, so it follows a re-imported model.",
            vec![
                name(),
                palette(false),
                param("owner", T::String, "Model or shape id.").required(),
                param("faces", T::Array, "Persistent face names on the owner, e.g. [\"S1:top\"].").required(),
                density(false),
                layers(),
                param("position_jitter", T::Number, "Accepted for uniformity; has no effect on a surface.").default(json!(1)),
                scale_min(),
                scale_max(),
                spin(true),
                min_distance(),
                param("max_slope_deg", T::Number, "Steepest surface planted, 0..90.").unit("deg").default(json!(45)),
                seed(),
                layer(),
            ],
            "{id}",
            json!({"name": "Lawn plants", "asset_palette": [{"asset": "flora:lawn-tuft", "weight": 1}], "owner": "S1", "faces": ["S1:top"], "density_per_m2": 20.0}),
        ),
        add_surface_fill,
    );
    s.register_with_spec(
        spec(
            "planting.add_cluster",
            "Add Cluster",
            "A clump of plants in a disc, denser towards the centre.",
            vec![
                name(),
                palette(true),
                param("center", T::Point2, "Centre [x, z].").unit("m").required(),
                param("radius_m", T::Number, "Radius of the disc, above zero.").unit("m").required(),
                param("count", T::Integer, "Number of plants.").required(),
                scale_min(),
                scale_max(),
                spin(true),
                seed(),
                layer(),
            ],
            "{id}",
            json!({"name": "Clump", "asset_palette": [{"asset": "flora:miscanthus-sinensis", "weight": 1}], "center": [3, 3], "radius_m": 1.5, "count": 9}),
        ),
        add_cluster,
    );
    s.register_with_spec(
        spec(
            "planting.add_floor",
            "Add Forest Floor",
            "Lays an area of ground material (default `forest-floor`: dark humus with moss, from the library's CC0 Ground 045) under a woodland. Its outline follows the crowns of the planting's trees (a ray from their centre to the farthest crown, smoothed), so it is irregular and ends under the outer crowns; it is an ordinary area (the lawn does not grow under it, and the lawn's edge wanders as a mowing edge does). Trees thinned or small in an open edge band leave the lawn showing.",
            vec![
                param("id", T::String, "Planting id (area scatter, brush or cluster).").required(),
                param("material", T::String, "Material id of the ground.").default(json!("forest-floor")),
                param("years", T::Number, "Crowns as they are this many years after planting (0..60); 60 is fully grown, as pictures show them unless the view sets years.").default(json!(60)),
                param("margin_m", T::Number, "Grow the outline outward by this much (0..20).").unit("m").default(json!(0)),
                param("edge_blend_m", T::Number, "Soft edge: the floor dithers into the lawn across a band this wide along its outline instead of ending on a line (0..10; 0 is hard).").unit("m").default(json!(1.5)),
            ],
            "{id}",
            json!({"id": "g1"}),
        ),
        add_floor,
    );
    s.register_with_spec(
        spec(
            "planting.update",
            "Update Planting",
            "Changes any fields of a generator (every field of its add command except `type`, and `planted_age_years`: how old its plants are when planted, null for each species' usual age, see `planting.growth`); fields given replace the old ones whole, the result is validated.",
            vec![param("id", T::String, "Generator id.").required()],
            "null",
            json!({"id": "g1", "density_per_m2": 0.05}),
        )
        .open(),
        update,
    );
    s.register_with_spec(
        spec(
            "planting.apply_preset",
            "Apply Planting Preset",
            "Replaces the layers of a brush, area scatter or surface fill with a ready-made woodland, meadow, border or similar.",
            vec![
                param("id", T::String, "Generator id (brush, area scatter or surface fill).").required(),
                param("preset", T::String, "Preset id.")
                    .required()
                    .allowed(&[
                        "broadleaf-woodland",
                        "conifer-woodland",
                        "mixed-woodland",
                        "woodland-edge",
                        "orchard",
                        "wildflower-meadow",
                        "perennial-border",
                        "hedgerow",
                        "pond-margin",
                        "mediterranean",
                    ]),
                param("edge_band_m", T::Number, "Forest-edge band (0..30): a shrub mantle this deep inside the edge, a herb fringe two fifths of it, trees shrinking towards the edge; deterministic by the planting's seed. Default 8 for the woodland presets (broadleaf, conifer, mixed, woodland-edge), 0 for the others; 0 gives the preset as it is.").unit("m"),
            ],
            "null",
            json!({"id": "g1", "preset": "mixed-woodland", "edge_band_m": 8}),
        ),
        apply_preset,
    );
    s.register_with_spec(
        spec(
            "planting.duplicate",
            "Duplicate Planting",
            "Copies a generator of any kind under a new id (named \"<name> copy\"), shifting its geometry (points, outline, strokes, centre, position) by an offset. A surface fill keeps its faces; only its seed can change.",
            vec![
                param("id", T::String, "Generator id.").required(),
                param("offset", T::Point2, "Shift [x, z] of the copy.").unit("m").default(json!([0, 0])),
                param("new_seed", T::Boolean, "Give the copy its own number as seed, so its plants differ.").default(json!(false)),
            ],
            "{id}",
            json!({"id": "g1", "offset": [12, 0], "new_seed": true}),
        ),
        duplicate,
    );
    s.register_with_spec(
        spec(
            "planting.undo_stroke",
            "Remove Last Stroke",
            "Removes the last stroke of a brush; refused when it has none.",
            vec![param("id", T::String, "Brush generator id.").required()],
            "null",
            json!({"id": "g1"}),
        ),
        undo_stroke,
    );
    s.register_with_spec(
        spec(
            "planting.replace_placement",
            "Change One Plant",
            "Swaps the species of one plant of a planting, or moves or resizes it, leaving the rest of the planting as it is (`index` is the plant's number in the planting: a selected tree's `sub.index` in `ui.get`'s selection). A planting of one plant just changes. In a scatter, border, brush, line or cluster the change is stored as an edit on that plant of the planting, applied after the planting is evaluated: the recipe (density, palette, seed, layers) stays, and the plant keeps its number as long as the recipe does. A change to the recipe that leaves fewer plants drops the edits that no longer have a plant (`planting.update` says which).",
            vec![
                param("planting", T::String, "Generator id.").required(),
                param("index", T::Integer, "The plant's number in the planting, from 0.")
                    .default(json!(0)),
                param(
                    "asset",
                    T::String,
                    "The species or object that takes its place: catalogue asset id, `flora:<species id>`, custom species id or `user:<object id>`.",
                ),
                param("scale", T::Number, "Its scale, above zero."),
                param("offset", T::Point2, "Moves it by [x, z].").unit("m"),
            ],
            "{planting, index, edited (how many plants of the planting are edited)}",
            json!({"planting": "g1", "index": 1, "asset": "flora:betula-pendula"}),
        ),
        replace_placement,
    );
    s.register_with_spec(
        spec(
            "planting.remove_placement",
            "Remove One Plant",
            "Takes one plant out of a planting (`index` as in `planting.replace_placement`), the rest stays. A planting of one plant is removed whole. `planting.reset_placements` puts it back.",
            vec![
                param("planting", T::String, "Generator id.").required(),
                param("index", T::Integer, "The plant's number in the planting, from 0.")
                    .default(json!(0)),
            ],
            "{planting, index, removed: true, edited}",
            json!({"planting": "g1", "index": 1}),
        ),
        remove_placement,
    );
    s.register_with_spec(
        spec(
            "planting.reset_placements",
            "Restore Plants",
            "Takes the edits off one plant (`index`) or off all plants of a planting: swapped species, moves, sizes and plants taken out go back to what the recipe plants.",
            vec![
                param("planting", T::String, "Generator id.").required(),
                param("index", T::Integer, "One plant's number; all plants without it."),
            ],
            "{planting, edited (how many plants are still edited)}",
            json!({"planting": "g1"}),
        ),
        reset_placements,
    );
    s.register_with_spec(
        spec(
            "planting.remove",
            "Remove Planting",
            "Removes a generator.",
            vec![param("id", T::String, "Generator id.").required()],
            "null",
            json!({"id": "g2"}),
        ),
        remove,
    );
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

/// Stores a new generator: `make(id, layer, default seed)`.
fn add(
    project: &mut Project,
    layer: Option<String>,
    make: impl FnOnce(String, String, u64) -> Generator,
) -> Result<Value, CommandError> {
    let layer = layer.unwrap_or_else(|| LAYER_PLANTING.into());
    check_layer(project, &layer)?;
    let id = next_id("g", |id| project.generators.iter().any(|g| g.id() == id));
    // Not every generator the same pattern: the default seed is the generator's number.
    let seed = id[1..].parse().unwrap_or(0);
    let g = make(id.clone(), layer, seed);
    g.validate().map_err(|e| bad(&e))?;
    project.generators.push(g);
    Ok(json!({ "id": id }))
}

fn add_single(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        asset: String,
        position: [f32; 3],
        #[serde(default)]
        rotation_y_deg: f32,
        #[serde(default = "one")]
        scale: f32,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, _| {
        Generator::Single(Single {
            id,
            name: p.name,
            layer,
            planted_age_years: None,
            asset: p.asset,
            position: p.position,
            rotation_y_deg: p.rotation_y_deg,
            scale: p.scale,
        })
    })
}

fn add_spline(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        asset_palette: Vec<Weighted>,
        points: Vec<[f32; 2]>,
        spacing: Spacing,
        #[serde(default)]
        closed: bool,
        #[serde(default)]
        spacing_jitter: f32,
        #[serde(default)]
        offset_m: f32,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        #[serde(default)]
        random_rotation: bool,
        seed: Option<u64>,
        layer: Option<String>,
        #[serde(default)]
        straight: bool,
        #[serde(default = "super::shapes::yes")]
        avoid_buildings: bool,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, seed| {
        Generator::SplineMass(SplineMass {
            overrides: Default::default(),
            id,
            name: p.name,
            layer,
            planted_age_years: None,
            asset_palette: p.asset_palette,
            points: p.points,
            closed: p.closed,
            spacing: p.spacing,
            spacing_jitter: p.spacing_jitter,
            offset_m: p.offset_m,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            random_rotation: p.random_rotation,
            seed: p.seed.unwrap_or(seed),
            straight: p.straight,
            avoid_buildings: p.avoid_buildings,
        })
    })
}

fn add_area(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        #[serde(default)]
        asset_palette: Vec<Weighted>,
        outline: Vec<[f32; 2]>,
        #[serde(default)]
        density_per_m2: f32,
        #[serde(default)]
        layers: Vec<PlantLayer>,
        #[serde(default = "one")]
        position_jitter: f32,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        #[serde(default = "yes")]
        random_rotation: bool,
        #[serde(default)]
        min_distance_m: f32,
        seed: Option<u64>,
        layer: Option<String>,
        #[serde(default = "super::shapes::yes")]
        avoid_buildings: bool,
        #[serde(default)]
        edge_fade_m: f32,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, seed| {
        Generator::AreaScatter(AreaScatter {
            overrides: Default::default(),
            id,
            name: p.name,
            layer,
            planted_age_years: None,
            asset_palette: p.asset_palette,
            outline: p.outline,
            density_per_m2: p.density_per_m2,
            position_jitter: p.position_jitter,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            random_rotation: p.random_rotation,
            min_distance_m: p.min_distance_m,
            seed: p.seed.unwrap_or(seed),
            layers: p.layers,
            avoid_buildings: p.avoid_buildings,
            edge_fade_m: p.edge_fade_m,
            border: None,
        })
    })
}

fn add_border(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    use crate::border::{self, Border, BorderSpecies};
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        #[serde(default = "border_name")]
        name: String,
        outline: Option<Vec<[f32; 2]>>,
        area: Option<String>,
        style: String,
        #[serde(default)]
        #[allow(dead_code)]
        dry: bool,
        front: Option<Vec<usize>>,
        facing: Option<[f32; 2]>,
        height_range: Option<[f32; 2]>,
        species: Option<Vec<String>>,
        closure: Option<f32>,
        drift_m: Option<f32>,
        seed: Option<u64>,
        layer: Option<String>,
        #[serde(default = "super::shapes::yes")]
        avoid_buildings: bool,
    }
    fn border_name() -> String {
        "Border".into()
    }
    let p: P = parse(params)?;
    let outline = match (p.outline, p.area) {
        (Some(o), None) => o,
        (None, Some(id)) => match project.shapes.iter().find(|s| s.id() == id) {
            Some(crate::shape::Shape::Area(a)) => a.outline().into_owned(),
            Some(_) => return Err(fail(format!("`{id}` is not an area"))),
            None => return Err(fail(format!("no shape `{id}`"))),
        },
        _ => return Err(bad("give `outline` or `area`, one of them")),
    };
    if outline.len() < 3 || outline.iter().flatten().any(|c| !c.is_finite()) {
        return Err(bad("outline: at least 3 finite points"));
    }
    if p.style == "auto" {
        return Err(bad(
            "style `auto` picks a style from the sun the outline gets, which needs a host that casts shadow (the window, `visioncraft-cli` or the MCP server); a bare session has no geometry: name a style (cottage, prairie, shade, dry) or ask `query.plants_for_light` for the hours you know",
        ));
    }
    let (mut species, default_range) =
        border::style(&p.style).ok_or_else(|| bad(&format!("no style `{}`", p.style)))?;
    if let Some(ids) = p.species {
        if ids.is_empty() {
            return Err(bad("species: at least one"));
        }
        species = ids
            .into_iter()
            .map(|id| {
                let asset = if id.starts_with("sp") || id.starts_with("flora:") {
                    id
                } else {
                    format!("flora:{id}")
                };
                let known = asset
                    .strip_prefix("flora:")
                    .is_some_and(|s| crate::growth::GROWTH.iter().any(|r| r.0 == s))
                    || project.species.iter().any(|s| s.id == asset);
                if !known {
                    return Err(fail(format!(
                        "no species `{asset}`: see species.list for the built-in ones"
                    )));
                }
                Ok(BorderSpecies {
                    matrix: border::is_matrix_by_size(&asset),
                    asset,
                    weight: 1.0,
                })
            })
            .collect::<Result<_, _>>()?;
    }
    let facing = if let Some(f) = p.facing {
        let l = f[0].hypot(f[1]);
        if !l.is_finite() || l < 1e-4 {
            return Err(bad("facing: a direction [x, z], not zero"));
        }
        [f[0] / l, f[1] / l]
    } else if let Some(edges) = &p.front {
        if edges.iter().any(|&e| e >= outline.len()) {
            return Err(bad("front: an edge index is past the outline's last edge"));
        }
        border::facing_of_edges(&outline, edges)
            .ok_or_else(|| bad("front: the edges cancel each other out"))?
    } else {
        border::derive_facing(&outline, &backs(project))
    };
    let height_range = p.height_range.unwrap_or(default_range);
    let b = Border {
        style: p.style,
        facing,
        height_range_m: height_range,
        species,
        closure: p.closure.unwrap_or(1.3),
        drift_m: p.drift_m.unwrap_or(2.4),
    };
    b.validate().map_err(|e| bad(&e))?;
    let used: Vec<String> = b.species.iter().map(|s| s.asset.clone()).collect();
    let r = add(project, p.layer, |id, layer, seed| {
        Generator::AreaScatter(AreaScatter {
            overrides: Default::default(),
            id,
            name: p.name,
            layer,
            planted_age_years: None,
            asset_palette: Vec::new(),
            outline,
            density_per_m2: 0.0,
            position_jitter: 1.0,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: true,
            min_distance_m: 0.0,
            seed: p.seed.unwrap_or(seed),
            layers: Vec::new(),
            avoid_buildings: p.avoid_buildings,
            edge_fade_m: 0.0,
            border: Some(b),
        })
    })?;
    let id = r["id"].as_str().unwrap_or_default().to_string();
    let g = project.generators.iter().find(|g| g.id() == id);
    let floor = crate::terrace::ground_fn(project);
    let ground = |x: f32, z: f32| floor(x, z).or(Some(0.0));
    let plants = g.map_or(0, |g| {
        crate::planting::evaluate(g, &ground).placements.len()
    });
    Ok(json!({
        "id": id,
        "facing": [round2(facing[0]), round2(facing[1])],
        "height_range": height_range,
        "species": used,
        "plants": plants,
    }))
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// What can stand behind a border: the segments of walls, buildings and hedges.
fn backs(project: &Project) -> Vec<Vec<([f32; 2], [f32; 2])>> {
    use crate::growth::{Grower, growth_of};
    let segments = |pts: &[[f32; 2]], closed: bool| -> Vec<([f32; 2], [f32; 2])> {
        let n = pts.len();
        (0..if closed { n } else { n.saturating_sub(1) })
            .map(|i| (pts[i], pts[(i + 1) % n]))
            .collect()
    };
    let mut out = Vec::new();
    for sh in &project.shapes {
        if let crate::shape::Shape::Extrusion(e) = sh {
            out.push(segments(&e.outline(), e.closed));
        }
    }
    for g in &project.generators {
        if let Generator::SplineMass(m) = g {
            let hedge = m.asset_palette.iter().any(|w| {
                matches!(
                    growth_of(&w.asset, None).grower,
                    Grower::Clipped | Grower::Climber
                )
            });
            if hedge {
                out.push(segments(&m.points, m.closed));
            }
        }
    }
    out
}

fn add_brush(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        #[serde(default)]
        asset_palette: Vec<Weighted>,
        #[serde(default)]
        density_per_m2: f32,
        #[serde(default)]
        min_distance_m: f32,
        #[serde(default)]
        layers: Vec<PlantLayer>,
        #[serde(default)]
        strokes: Vec<Stroke>,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        seed: Option<u64>,
        layer: Option<String>,
        #[serde(default)]
        clip_outline: Vec<[f32; 2]>,
        #[serde(default)]
        clip_holes: Vec<Vec<[f32; 2]>>,
        #[serde(default = "super::shapes::yes")]
        avoid_buildings: bool,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, seed| {
        Generator::MixedBrush(MixedBrush {
            overrides: Default::default(),
            id,
            name: p.name,
            layer,
            planted_age_years: None,
            asset_palette: p.asset_palette,
            strokes: p.strokes,
            density_per_m2: p.density_per_m2,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            min_distance_m: p.min_distance_m,
            seed: p.seed.unwrap_or(seed),
            layers: p.layers,
            clip_outline: p.clip_outline,
            clip_holes: p.clip_holes,
            avoid_buildings: p.avoid_buildings,
        })
    })
}

fn add_surface_fill(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        #[serde(default)]
        asset_palette: Vec<Weighted>,
        owner: String,
        faces: Vec<String>,
        #[serde(default)]
        density_per_m2: f32,
        #[serde(default)]
        layers: Vec<PlantLayer>,
        #[serde(default = "one")]
        position_jitter: f32,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        #[serde(default = "yes")]
        random_rotation: bool,
        #[serde(default)]
        min_distance_m: f32,
        #[serde(default = "slope")]
        max_slope_deg: f32,
        seed: Option<u64>,
        layer: Option<String>,
    }
    fn slope() -> f32 {
        45.0
    }
    let p: P = parse(params)?;
    if !project.models.iter().any(|m| m.id == p.owner)
        && !project.shapes.iter().any(|s| s.id() == p.owner)
    {
        return Err(fail(format!("no model or shape `{}`", p.owner)));
    }
    add(project, p.layer, |id, layer, seed| {
        Generator::SurfaceFill(SurfaceFill {
            id,
            name: p.name,
            layer,
            planted_age_years: None,
            asset_palette: p.asset_palette,
            owner: p.owner,
            faces: p.faces,
            density_per_m2: p.density_per_m2,
            position_jitter: p.position_jitter,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            random_rotation: p.random_rotation,
            min_distance_m: p.min_distance_m,
            max_slope_deg: p.max_slope_deg,
            seed: p.seed.unwrap_or(seed),
            layers: p.layers,
        })
    })
}

fn add_cluster(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        asset_palette: Vec<Weighted>,
        center: [f32; 2],
        radius_m: f32,
        count: u32,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        #[serde(default = "yes")]
        random_rotation: bool,
        seed: Option<u64>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, seed| {
        Generator::Cluster(Cluster {
            overrides: Default::default(),
            id,
            name: p.name,
            layer,
            planted_age_years: None,
            asset_palette: p.asset_palette,
            center: p.center,
            radius_m: p.radius_m,
            count: p.count,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            random_rotation: p.random_rotation,
            seed: p.seed.unwrap_or(seed),
        })
    })
}

fn add_floor(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        #[serde(default = "forest_floor")]
        material: String,
        #[serde(default = "full_grown")]
        years: f32,
        #[serde(default)]
        margin_m: f32,
        // guess: about 1.5 m reads as a woodland edge fading out under the last crowns; no source.
        #[serde(default = "floor_blend")]
        edge_blend_m: f32,
    }
    fn floor_blend() -> f32 {
        1.5
    }
    fn forest_floor() -> String {
        "forest-floor".into()
    }
    fn full_grown() -> f32 {
        60.0
    }
    let p: P = parse(params)?;
    if !(0.0..=20.0).contains(&p.margin_m) || !(0.0..=60.0).contains(&p.years) {
        return Err(bad("margin_m: 0..20, years: 0..60"));
    }
    let g = project
        .generators
        .iter()
        .find(|g| g.id() == p.id)
        .ok_or_else(|| fail(format!("no generator `{}`", p.id)))?;
    if crate::planting::planted_area(g).is_none() {
        return Err(fail(
            "a forest floor needs an area scatter, a brush or a cluster",
        ));
    }
    let crowns = super::growth::crowns(project, g, p.years, false);
    let outline = crate::canopy::crown_outline(&crowns)
        .ok_or_else(|| fail("the planting has no trees to lay a floor under"))?;
    let outline = crate::shape::offset_outline(&outline, p.margin_m);
    let name = format!("{} floor", g.name());
    // A project from before the palette had it gets the library's leaf litter now.
    if !project.materials.iter().any(|m| m.id == p.material)
        && let Some(m) = crate::material::builtin_palette()
            .into_iter()
            .find(|m| m.id == p.material)
    {
        project.materials.push(m);
    }
    super::shapes::add_area(
        project,
        &json!({"name": name, "outline": outline, "material": p.material, "edge_blend_m": p.edge_blend_m}),
    )
}

fn find<'a>(project: &'a mut Project, params: &Value) -> Result<&'a mut Generator, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    project
        .generators
        .iter_mut()
        .find(|g| g.id() == id)
        .ok_or_else(|| fail(format!("no generator `{id}`")))
}

fn brush_stroke(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        #[serde(rename = "id")]
        _id: String,
        points: Vec<[f32; 2]>,
        radius_m: f32,
        #[serde(default)]
        erase: bool,
    }
    let p: P = parse(params)?;
    let Generator::MixedBrush(b) = find(project, params)? else {
        return Err(fail("only a brush takes strokes"));
    };
    b.strokes.push(Stroke {
        points: p.points,
        radius_m: p.radius_m,
        erase: p.erase,
    });
    let g = Generator::MixedBrush(b.clone());
    g.validate().map_err(|e| bad(&e))?;
    let id = params["id"].as_str().unwrap_or_default().to_string();
    Ok(pruned(project, &id))
}

fn update(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let g = find(project, params)?;
    // Fields that are not written while empty can still be given.
    let updated: Generator = merge_update(
        g,
        params,
        &[
            "clip_outline",
            "clip_holes",
            "straight",
            "avoid_buildings",
            "planted_age_years",
            "border",
        ],
    )?;
    updated.validate().map_err(|e| bad(&e))?;
    let layer = updated.layer().to_string();
    *g = updated;
    check_layer(project, &layer)?;
    let id = params["id"].as_str().unwrap_or_default().to_string();
    Ok(pruned(project, &id))
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = find(project, params)?.id().to_string();
    project.generators.retain(|g| g.id() != id);
    Ok(Value::Null)
}

fn apply_preset(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let name = params["preset"]
        .as_str()
        .ok_or_else(|| bad("missing preset"))?;
    let preset = planting_presets()
        .into_iter()
        .find(|p| p.id == name)
        .ok_or_else(|| fail(format!("no preset `{name}`")))?;
    let band = match params.get("edge_band_m") {
        None | Some(Value::Null) => crate::presets::default_edge_band_m(name),
        Some(v) => v
            .as_f64()
            .filter(|b| (0.0..=30.0).contains(b))
            .ok_or_else(|| bad("edge_band_m: 0..30"))? as f32,
    };
    let preset_layers = crate::presets::with_edge_band(preset.layers, band);
    // A woodland is a grown stand: its plants start at the preset's stand age unless the
    // planting has an age of its own (nursery age 8 would show a young plantation).
    let stand = crate::presets::stand_age_years(name);
    let (layers, age) = match find(project, params)? {
        Generator::AreaScatter(g) => (&mut g.layers, &mut g.planted_age_years),
        Generator::MixedBrush(g) => (&mut g.layers, &mut g.planted_age_years),
        Generator::SurfaceFill(g) => (&mut g.layers, &mut g.planted_age_years),
        _ => {
            return Err(fail(
                "only a brush, area scatter or surface fill takes layers",
            ));
        }
    };
    *layers = preset_layers;
    if age.is_none() {
        *age = stand;
    }
    let id = params["id"].as_str().unwrap_or_default().to_string();
    Ok(pruned(project, &id))
}

fn undo_stroke(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let Generator::MixedBrush(b) = find(project, params)? else {
        return Err(fail("only a brush has strokes"));
    };
    b.strokes
        .pop()
        .ok_or_else(|| fail("the brush has no strokes"))?;
    let id = params["id"].as_str().unwrap_or_default().to_string();
    Ok(pruned(project, &id))
}

fn duplicate(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        #[serde(rename = "id")]
        _id: String,
        #[serde(default)]
        offset: [f32; 2],
        #[serde(default)]
        new_seed: bool,
    }
    let p: P = parse(params)?;
    if !p.offset.iter().all(|c| c.is_finite()) {
        return Err(bad("offset must be finite"));
    }
    let mut copy = find(project, params)?.clone();
    let id = next_id("g", |id| project.generators.iter().any(|g| g.id() == id));
    let seed = id[1..].parse().unwrap_or(0);
    let shift = |pts: &mut Vec<[f32; 2]>| {
        for q in pts {
            *q = [q[0] + p.offset[0], q[1] + p.offset[1]];
        }
    };
    match &mut copy {
        Generator::Single(g) => {
            g.id = id.clone();
            g.name += " copy";
            g.position = [
                g.position[0] + p.offset[0],
                g.position[1],
                g.position[2] + p.offset[1],
            ];
        }
        Generator::SplineMass(g) => {
            g.id = id.clone();
            g.name += " copy";
            shift(&mut g.points);
            if p.new_seed {
                g.seed = seed;
            }
        }
        Generator::AreaScatter(g) => {
            g.id = id.clone();
            g.name += " copy";
            shift(&mut g.outline);
            if p.new_seed {
                g.seed = seed;
            }
        }
        Generator::MixedBrush(g) => {
            g.id = id.clone();
            g.name += " copy";
            g.strokes.iter_mut().for_each(|s| shift(&mut s.points));
            if p.new_seed {
                g.seed = seed;
            }
        }
        Generator::SurfaceFill(g) => {
            g.id = id.clone();
            g.name += " copy";
            if p.new_seed {
                g.seed = seed;
            }
        }
        Generator::Cluster(g) => {
            g.id = id.clone();
            g.name += " copy";
            g.center = [g.center[0] + p.offset[0], g.center[1] + p.offset[1]];
            if p.new_seed {
                g.seed = seed;
            }
        }
    }
    if p.new_seed
        && let Some(o) = copy.overrides_mut()
    {
        o.clear();
    }
    project.generators.push(copy);
    Ok(json!({ "id": id }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementParams {
    planting: String,
    index: Option<usize>,
    asset: Option<String>,
    scale: Option<f32>,
    offset: Option<[f32; 2]>,
}

/// The plants of a planting as the commands number them: before its edits.
fn plant_count(project: &Project, g: &Generator) -> Result<usize, CommandError> {
    match g {
        Generator::Single(_) => Ok(1),
        Generator::SurfaceFill(_) => Err(fail(format!(
            "`{}` is planted on a surface: its plants cannot be edited one by one",
            g.id()
        ))),
        _ => {
            let floor = crate::terrace::ground_fn(project);
            let ground = |x: f32, z: f32| floor(x, z).or(Some(0.0));
            Ok(crate::planting::evaluate_raw(g, &ground).placements.len())
        }
    }
}

/// The planting's place in the project and a checked plant number.
fn plant_of(project: &Project, p: &PlacementParams) -> Result<(usize, u32), CommandError> {
    let at = project
        .generators
        .iter()
        .position(|g| g.id() == p.planting)
        .ok_or_else(|| fail(format!("no generator `{}`", p.planting)))?;
    let n = plant_count(project, &project.generators[at])?;
    let i = p.index.unwrap_or(0);
    if i >= n {
        return Err(fail(format!(
            "`{}` has {n} plant(s): index {i} is out of range",
            p.planting
        )));
    }
    Ok((at, i as u32))
}

/// Drops the edits of `id` that have no plant any more (after the recipe changed); the numbers
/// dropped.
fn prune_overrides(project: &mut Project, id: &str) -> Vec<u32> {
    let Some(at) = project.generators.iter().position(|g| g.id() == id) else {
        return Vec::new();
    };
    if project.generators[at]
        .overrides()
        .is_none_or(|o| o.is_empty())
    {
        return Vec::new();
    }
    let n = plant_count(project, &project.generators[at]).unwrap_or(0) as u32;
    let Some(o) = project.generators[at].overrides_mut() else {
        return Vec::new();
    };
    let gone: Vec<u32> = o.keys().copied().filter(|i| *i >= n).collect();
    for i in &gone {
        o.remove(i);
    }
    gone
}

/// What a recipe change answers: null, or the warning about the edits it dropped.
fn pruned(project: &mut Project, id: &str) -> Value {
    let gone = prune_overrides(project, id);
    if gone.is_empty() {
        return Value::Null;
    }
    let list: Vec<String> = gone.iter().map(u32::to_string).collect();
    json!({"warnings": [format!(
        "`{id}` has fewer plants now: the edits of plant(s) {} were dropped",
        list.join(", ")
    )]})
}

fn replace_placement(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let p: PlacementParams = parse(params)?;
    let asset = p.asset.clone();
    if asset.as_deref().is_some_and(|a| a.trim().is_empty()) {
        return Err(bad("asset is empty"));
    }
    if asset.is_none() && p.scale.is_none() && p.offset.is_none() {
        return Err(bad("give an asset, a scale or an offset"));
    }
    if p.scale.is_some_and(|s| !(s.is_finite() && s > 0.0)) {
        return Err(bad("scale: above zero"));
    }
    let (at, i) = plant_of(project, &p)?;
    let g = &mut project.generators[at];
    if let Generator::Single(s) = g {
        if let Some(a) = asset {
            s.asset = a;
        }
        if let Some(k) = p.scale {
            s.scale = k;
        }
        if let Some(d) = p.offset {
            s.position[0] += d[0];
            s.position[2] += d[1];
        }
        return Ok(json!({"planting": p.planting, "index": 0, "edited": 0}));
    }
    let o = g.overrides_mut().ok_or_else(|| fail("not editable"))?;
    let e = o.entry(i).or_default();
    e.removed = false;
    if asset.is_some() {
        e.asset = asset;
    }
    if p.scale.is_some() {
        e.scale = p.scale;
    }
    if let Some(d) = p.offset {
        // Moves add up.
        let old = e.offset.unwrap_or([0.0, 0.0]);
        e.offset = Some([old[0] + d[0], old[1] + d[1]]);
    }
    Ok(json!({"planting": p.planting, "index": i, "edited": o.len()}))
}

fn remove_placement(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let p: PlacementParams = parse(params)?;
    let (at, i) = plant_of(project, &p)?;
    if matches!(project.generators[at], Generator::Single(_)) {
        project.generators.remove(at);
        return Ok(json!({"planting": p.planting, "index": 0, "removed": true, "edited": 0}));
    }
    let o = project.generators[at]
        .overrides_mut()
        .ok_or_else(|| fail("not editable"))?;
    o.entry(i).or_default().removed = true;
    Ok(json!({"planting": p.planting, "index": i, "removed": true, "edited": o.len()}))
}

fn reset_placements(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let p: PlacementParams = parse(params)?;
    let at = project
        .generators
        .iter()
        .position(|g| g.id() == p.planting)
        .ok_or_else(|| fail(format!("no generator `{}`", p.planting)))?;
    let Some(o) = project.generators[at].overrides_mut() else {
        return Ok(json!({"planting": p.planting, "edited": 0}));
    };
    match p.index {
        Some(i) => {
            o.remove(&(i as u32));
        }
        None => o.clear(),
    }
    Ok(json!({"planting": p.planting, "edited": o.len()}))
}

#[cfg(test)]
mod tests {
    use crate::{Project, Session};
    use serde_json::json;

    #[test]
    fn woodland_presets_start_grown_and_close_from_above() {
        for (preset, min) in [("mixed-woodland", 0.8), ("broadleaf-woodland", 0.8)] {
            let mut s = Session::new(Project::default());
            s.run(
                "planting.add_area",
                json!({"name": "Wood", "outline": [[0, 0], [100, 0], [100, 100], [0, 100]],
                    "seed": 3, "density_per_m2": 0.01,
                    "asset_palette": [{"asset": "flora:betula-pendula", "weight": 1}]}),
            )
            .unwrap();
            s.run(
                "planting.apply_preset",
                json!({"id": "g1", "preset": preset, "edge_band_m": 0}),
            )
            .unwrap();
            let c = s.run("planting.canopy_cover", json!({"id": "g1"})).unwrap();
            assert!(c["cover"].as_f64().unwrap() >= min, "{preset}: {c}");
        }
    }

    #[test]
    fn a_forest_edge_band_puts_shrubs_and_small_trees_at_the_edge() {
        let run = |band: Option<f64>| {
            let mut s = Session::new(Project::default());
            s.run(
                "planting.add_area",
                json!({"name": "Wood", "outline": [[0, 0], [60, 0], [60, 60], [0, 60]], "seed": 4, "density_per_m2": 0.01,
                    "asset_palette": [{"asset": "flora:betula-pendula", "weight": 1}]}),
            )
            .unwrap();
            let mut p = json!({"id": "g1", "preset": "mixed-woodland"});
            if let Some(b) = band {
                p["edge_band_m"] = json!(b);
            }
            s.run("planting.apply_preset", p).unwrap();
            let g = &s.project().generators[0];
            crate::planting::evaluate(g, &|_, _| Some(0.0)).placements
        };
        let (plain, banded) = (run(Some(0.0)), run(None));
        let depth = |p: &crate::planting::Placement| {
            let (x, z) = (p.position[0], p.position[2]);
            x.min(60.0 - x).min(z).min(60.0 - z)
        };
        // Layer 3 and 4 are the band's: nothing deeper in than the band.
        let band: Vec<_> = banded.iter().filter(|p| p.layer >= Some(3)).collect();
        assert!(!band.is_empty());
        assert!(band.iter().all(|p| depth(p) <= 8.0 + 1e-3));
        assert!(plain.iter().all(|p| p.layer < Some(3)));
        // The canopy keeps its places but is smaller at the edge; deeper in it is unchanged.
        let canopy = |v: &[crate::planting::Placement]| {
            v.iter()
                .filter(|p| p.layer == Some(0))
                .cloned()
                .collect::<Vec<_>>()
        };
        let (a, b) = (canopy(&plain), canopy(&banded));
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.position, y.position);
            if depth(x) >= 8.0 {
                assert_eq!(x.scale, y.scale);
            } else {
                assert!(y.scale < x.scale + 1e-6);
            }
        }
        assert!(a.iter().zip(&b).any(|(x, y)| y.scale < 0.8 * x.scale));
        assert_eq!(banded, run(None), "deterministic");
    }

    #[test]
    fn a_forest_floor_follows_the_planted_ground() {
        let mut s = Session::new(Project::default());
        s.run(
            "planting.add_brush",
            json!({"name": "Wood", "asset_palette": [{"asset": "flora:betula-pendula", "weight": 1}],
                "density_per_m2": 0.02, "strokes": [{"points": [[0, 0], [20, 0], [20, 20]], "radius_m": 5, "erase": false}]}),
        )
        .unwrap();
        s.run("planting.add_floor", json!({"id": "g1"})).unwrap();
        let p = s.project();
        let crate::shape::Shape::Area(a) = &p.shapes[0] else {
            panic!("an area");
        };
        // The outline follows the crowns: a polygon of rays round their centre, ending under
        // the outer crowns (inside the brush's reach), not a ruler-straight hull.
        assert_eq!(a.points.len(), 72);
        assert!(
            a.points
                .iter()
                .all(|q| (-10.0..=30.0).contains(&q[0]) && (-10.0..=30.0).contains(&q[1]))
        );
        assert!(p.assignments.iter().any(|x| x.material == "forest-floor"));
        // Its edge is soft by default (a guess, 1.5 m), and settable.
        assert_eq!(a.edge_blend_m, 1.5);
        s.run(
            "shape.set_edge_blend",
            json!({"id": "S1", "edge_blend_m": 0.0}),
        )
        .unwrap();
        let crate::shape::Shape::Area(a) = &s.project().shapes[0] else {
            panic!("an area");
        };
        assert_eq!(a.edge_blend_m, 0.0);
        assert!(
            s.run(
                "shape.set_edge_blend",
                json!({"id": "S1", "edge_blend_m": 11})
            )
            .is_err()
        );
        s.run(
            "planting.add_single",
            json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [5, 0, 5]}),
        )
        .unwrap();
        assert!(s.run("planting.add_floor", json!({"id": "g2"})).is_err());
    }

    fn grove_session() -> Session {
        let mut s = Session::new(Project::default());
        s.run(
            "planting.add_cluster",
            json!({"name": "Grove", "center": [0, 0], "radius_m": 3, "count": 5,
                "asset_palette": [{"asset": "flora:quercus-robur", "weight": 1}]}),
        )
        .unwrap();
        s
    }

    fn plants(s: &Session) -> Vec<crate::planting::Placement> {
        let g = &s.project().generators[0];
        crate::planting::evaluate(g, &|_, _| Some(0.0)).placements
    }

    #[test]
    fn replacing_a_single_plant_changes_its_species() {
        let mut s = Session::new(Project::default());
        s.run(
            "planting.add_single",
            json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [4, 0, 3]}),
        )
        .unwrap();
        let r = s
            .run(
                "planting.replace_placement",
                json!({"planting": "g1", "asset": "flora:betula-pendula"}),
            )
            .unwrap();
        assert_eq!(r["planting"], "g1");
        let g = &s.project().generators[0];
        assert!(g.uses_asset("flora:betula-pendula"));
        assert!(!g.uses_asset("flora:quercus-robur"));
        s.undo();
        assert!(s.project().generators[0].uses_asset("flora:quercus-robur"));
    }

    #[test]
    fn replacing_one_plant_of_a_cluster_keeps_the_planting_and_its_recipe() {
        let mut s = grove_session();
        let before = plants(&s);
        let r = s
            .run(
                "planting.replace_placement",
                json!({"planting": "g1", "index": 2, "asset": "flora:betula-pendula"}),
            )
            .unwrap();
        assert_eq!(r["edited"], 1);
        assert_eq!(s.project().generators.len(), 1, "still one planting");
        let after = plants(&s);
        assert_eq!(after.len(), 5);
        for (i, (a, b)) in after.iter().zip(&before).enumerate() {
            assert_eq!(a.position, b.position, "plant {i} stays where it was");
            assert_eq!(
                a.asset == "flora:betula-pendula",
                i == 2,
                "only plant 2 is a birch"
            );
        }
        // The recipe is untouched: the palette still says oak.
        assert!(s.project().generators[0].uses_asset("flora:quercus-robur"));
        // Moving and resizing add to the edit.
        s.run(
            "planting.replace_placement",
            json!({"planting": "g1", "index": 2, "scale": 2, "offset": [1, 0]}),
        )
        .unwrap();
        let moved = &plants(&s)[2];
        assert_eq!(moved.asset, "flora:betula-pendula");
        assert_eq!(moved.scale, 2.0);
        assert!((moved.position[0] - before[2].position[0] - 1.0).abs() < 1e-5);
        s.undo();
        s.undo();
        assert_eq!(plants(&s), before);
    }

    #[test]
    fn removing_one_plant_leaves_the_others_where_they_were_and_numbered() {
        let mut s = grove_session();
        let before = plants(&s);
        let r = s
            .run(
                "planting.remove_placement",
                json!({"planting": "g1", "index": 1}),
            )
            .unwrap();
        assert_eq!(r["removed"], true);
        let after: Vec<[f32; 3]> = plants(&s).iter().map(|p| p.position).collect();
        let mut want: Vec<[f32; 3]> = before.iter().map(|p| p.position).collect();
        want.remove(1);
        assert_eq!(after, want);
        // The numbers hold: plant 3 is still plant 3 for the next edit.
        s.run(
            "planting.replace_placement",
            json!({"planting": "g1", "index": 3, "asset": "flora:betula-pendula"}),
        )
        .unwrap();
        let g = &s.project().generators[0];
        let (numbered, _) = crate::planting::evaluate_numbered(g, &|_, _| Some(0.0));
        let birch: Vec<usize> = numbered
            .iter()
            .filter(|(_, p)| p.asset == "flora:betula-pendula")
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(birch, vec![3]);
        // And it comes back.
        s.run("planting.reset_placements", json!({"planting": "g1"}))
            .unwrap();
        assert_eq!(plants(&s), before);
    }

    #[test]
    fn removing_the_only_plant_removes_the_planting() {
        let mut s = Session::new(Project::default());
        s.run(
            "planting.add_single",
            json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [4, 0, 3]}),
        )
        .unwrap();
        s.run("planting.remove_placement", json!({"planting": "g1"}))
            .unwrap();
        assert!(s.project().generators.is_empty());
    }

    #[test]
    fn placement_commands_refuse_bad_indices_and_ids() {
        let mut s = grove_session();
        let e = s
            .run(
                "planting.replace_placement",
                json!({"planting": "g1", "index": 9, "asset": "flora:betula-pendula"}),
            )
            .unwrap_err();
        assert!(e.to_string().contains("out of range"), "{e}");
        assert!(
            s.run("planting.remove_placement", json!({"planting": "nope"}))
                .is_err()
        );
        assert!(
            s.run(
                "planting.replace_placement",
                json!({"planting": "g1", "index": 0})
            )
            .is_err()
        );
        assert!(s.project().generators[0].overrides().unwrap().is_empty());
    }

    #[test]
    fn an_edited_plant_survives_saving_and_loading() {
        let mut s = grove_session();
        s.run(
            "planting.replace_placement",
            json!({"planting": "g1", "index": 4, "asset": "flora:betula-pendula"}),
        )
        .unwrap();
        s.run(
            "planting.remove_placement",
            json!({"planting": "g1", "index": 0}),
        )
        .unwrap();
        let json = serde_json::to_string(s.project()).unwrap();
        let loaded: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(&loaded, s.project());
        let g = &loaded.generators[0];
        assert_eq!(g.overrides().unwrap().len(), 2);
        let want = plants(&s);
        assert_eq!(
            crate::planting::evaluate(g, &|_, _| Some(0.0)).placements,
            want
        );
        // A project without edits writes no `overrides`, and an old one still loads.
        let plain = serde_json::to_string(grove_session().project()).unwrap();
        assert!(!plain.contains("overrides"));
    }

    #[test]
    fn a_recipe_change_drops_the_edits_that_lost_their_plant() {
        let mut s = Session::new(Project::default());
        s.run(
            "planting.add_area",
            json!({"name": "Meadow", "outline": [[0, 0], [20, 0], [20, 20], [0, 20]],
                "seed": 3, "density_per_m2": 0.1,
                "asset_palette": [{"asset": "flora:quercus-robur", "weight": 1}]}),
        )
        .unwrap();
        let n = crate::planting::evaluate(&s.project().generators[0], &|_, _| Some(0.0))
            .placements
            .len();
        assert!(n > 20, "{n}");
        for i in [1, n - 1] {
            s.run(
                "planting.replace_placement",
                json!({"planting": "g1", "index": i, "asset": "flora:betula-pendula"}),
            )
            .unwrap();
        }
        let r = s
            .run(
                "planting.update",
                json!({"id": "g1", "density_per_m2": 0.02}),
            )
            .unwrap();
        let warning = r["warnings"][0].as_str().expect("a warning");
        assert!(warning.contains(&(n - 1).to_string()), "{warning}");
        let g = &s.project().generators[0];
        let kept: Vec<u32> = g.overrides().unwrap().keys().copied().collect();
        assert_eq!(kept, vec![1], "the one that still has a plant stays");
        // A change that loses nothing has nothing to say.
        let r = s
            .run(
                "planting.update",
                json!({"id": "g1", "name": "Wild meadow"}),
            )
            .unwrap();
        assert_eq!(r, serde_json::Value::Null);
    }

    #[test]
    fn quantities_count_the_replaced_species() {
        let mut s = grove_session();
        let count = |s: &mut Session, asset: &str| -> u64 {
            let q = s.run("query.quantities", json!({})).unwrap();
            let species = q["assets"].as_array().cloned().unwrap_or_default();
            species
                .iter()
                .find(|e| e["asset"] == asset)
                .map_or(0, |e| e["count"].as_u64().unwrap())
        };
        assert_eq!(count(&mut s, "flora:quercus-robur"), 5);
        s.run(
            "planting.replace_placement",
            json!({"planting": "g1", "index": 2, "asset": "flora:betula-pendula"}),
        )
        .unwrap();
        s.run(
            "planting.remove_placement",
            json!({"planting": "g1", "index": 0}),
        )
        .unwrap();
        assert_eq!(count(&mut s, "flora:quercus-robur"), 3);
        assert_eq!(count(&mut s, "flora:betula-pendula"), 1);
    }
}
