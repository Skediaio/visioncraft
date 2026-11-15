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
            "Lays an area of ground material (default the dark leaf litter `forest-floor`) over the ground a woodland planting stands on, so gaps between the crowns read as forest floor and not as lawn: the outline of an area scatter, a disc for a cluster, the convex hull of a brush's strokes. It is an ordinary area (move, restyle or remove it with the shape commands; the lawn does not grow under it).",
            vec![
                param("id", T::String, "Planting id (area scatter, brush or cluster).").required(),
                param("material", T::String, "Material id of the ground.").default(json!("forest-floor")),
                param("margin_m", T::Number, "Grow the outline outward by this much (0..20).").unit("m").default(json!(0)),
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
        })
    })
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
        #[serde(default)]
        margin_m: f32,
    }
    fn forest_floor() -> String {
        "forest-floor".into()
    }
    let p: P = parse(params)?;
    if !(0.0..=20.0).contains(&p.margin_m) {
        return Err(bad("margin_m: 0..20"));
    }
    let g = project
        .generators
        .iter()
        .find(|g| g.id() == p.id)
        .ok_or_else(|| fail(format!("no generator `{}`", p.id)))?;
    let outline = crate::planting::planted_outline(g)
        .ok_or_else(|| fail("a forest floor needs an area scatter, a brush or a cluster"))?;
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
        &json!({"name": name, "outline": outline, "material": p.material}),
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
    Ok(Value::Null)
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
        ],
    )?;
    updated.validate().map_err(|e| bad(&e))?;
    let layer = updated.layer().to_string();
    *g = updated;
    check_layer(project, &layer)?;
    Ok(Value::Null)
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
    let layers = match find(project, params)? {
        Generator::AreaScatter(g) => &mut g.layers,
        Generator::MixedBrush(g) => &mut g.layers,
        Generator::SurfaceFill(g) => &mut g.layers,
        _ => {
            return Err(fail(
                "only a brush, area scatter or surface fill takes layers",
            ));
        }
    };
    *layers = preset_layers;
    Ok(Value::Null)
}

fn undo_stroke(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let Generator::MixedBrush(b) = find(project, params)? else {
        return Err(fail("only a brush has strokes"));
    };
    b.strokes
        .pop()
        .ok_or_else(|| fail("the brush has no strokes"))?;
    Ok(Value::Null)
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
    project.generators.push(copy);
    Ok(json!({ "id": id }))
}

#[cfg(test)]
mod tests {
    use crate::{Project, Session};
    use serde_json::json;

    #[test]
    fn a_forest_edge_band_puts_shrubs_and_small_trees_at_the_edge() {
        let run = |band: Option<f64>| {
            let mut s = Session::new(Project::default());
            s.run(
                "planting.add_area",
                json!({"name": "Wood", "outline": [[0, 0], [60, 0], [60, 60], [0, 60]], "seed": 4,
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
            v.iter().filter(|p| p.layer == Some(0)).cloned().collect::<Vec<_>>()
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
        // The hull holds every stroke point and reaches the stroke discs' edge.
        for q in [[0.0, 0.0], [20.0, 0.0], [20.0, 20.0]] {
            assert!(crate::planting::point_in_polygon(q, &a.outline));
        }
        let (lo, hi) = a.outline.iter().fold((f32::MAX, f32::MIN), |(l, h), q| {
            (l.min(q[0]), h.max(q[0]))
        });
        assert!((lo + 5.0).abs() < 0.1 && (hi - 25.0).abs() < 0.1, "{lo} {hi}");
        assert!(p.assignments.iter().any(|x| x.material == "forest-floor"));
        s.run(
            "planting.add_single",
            json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [5, 0, 5]}),
        )
        .unwrap();
        assert!(s.run("planting.add_floor", json!({"id": "g2"})).is_err());
    }
}
