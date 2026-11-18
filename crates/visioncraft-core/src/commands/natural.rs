//! Natural ground (`crate::natural`): automatic terrain materials, hex tiling and ground cover
//! (`terrain.set_auto`, `terrain.set_tiling`, `terrain.set_cover`), rocks (`rock.*`) and deadwood
//! (`deadwood.*`).

use super::{check_layer, fail, merge_update, next_id};
use crate::natural::{
    AutoMaterials, Deadwood, DeadwoodKind, GroundCover, Rock, RockShape, ensure_material,
    rockiness, scatter_points, unit,
};
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use crate::terrain::Terrain;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Most rocks or deadwood pieces one scatter makes.
const MAX_SCATTER: usize = 3000;

const SHAPES: [&str; 4] = ["boulder", "slab", "outcrop", "cobble"];
const KINDS: [&str; 4] = ["log", "stump", "snag", "branch"];

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "terrain.set_auto",
            "Automatic Terrain Materials",
            "Covers the terrain's base layer automatically by the shape of the ground: rock on steep and convex ground (and above `rock_above_m`), scree on the slopes below it and at their foot, moss in hollows and on faces turned away from the sun, soil and litter collecting in hollows. Low-frequency noise moves every boundary over `macro_scale_m`, so no two slopes look alike. Each class gets a terrain layer with its material (added when missing, at most 8 layers in all); paint on other layers stays as painted and wins. Only the given fields change; `enabled: false` turns it off (the layers stay). The materials `rock`, `scree` and `moss` (CC0 maps from the library) are added to the project when it lacks them. North for the shade is the site's.",
            vec![
                param("enabled", T::Boolean, "False removes the automatic materials.").default(json!(true)),
                param("rock", T::Any, "Material id for bare rock, or null for none.").default(json!("rock")),
                param("scree", T::Any, "Material id for scree (loose stones), or null.").default(json!("scree")),
                param("moss", T::Any, "Material id for moss, or null.").default(json!("moss")),
                param("soil", T::Any, "Material id for soil and leaf litter in hollows, or null.").default(json!("forest-floor")),
                param("rock_slope_deg", T::Number, "Slope where rock takes over, 1..89.").unit("deg").default(json!(38)),
                param("scree_slope_deg", T::Number, "Slope where scree starts, 1..89, not above rock_slope_deg.").unit("deg").default(json!(27)),
                param("blend_deg", T::Number, "Half the width of each transition, 0.5..20.").unit("deg").default(json!(5)),
                param("rock_above_m", T::Any, "Above this height rock shows on gentler slopes too (a mountain top); null for none.").unit("m"),
                param("curvature", T::Number, "0..3: how strongly convex edges show rock and hollows collect soil and moss.").default(json!(1)),
                param("moisture", T::Number, "0..3: how readily moss grows in shade and hollows.").default(json!(1)),
                param("macro_scale_m", T::Number, "Size of the noise that moves the boundaries, 2..2000.").unit("m").default(json!(40)),
                param("macro_strength", T::Number, "0..1: how far the noise moves them.").default(json!(0.5)),
                param("seed", T::Integer, "Seed of the noise.").default(json!(1)),
            ],
            "{enabled, layers: [{class, material, index}], added_layers}",
            json!({"rock_slope_deg": 35, "rock_above_m": 12}),
        ),
        set_auto,
    );
    s.register_with_spec(
        spec(
            "terrain.set_tiling",
            "Terrain Hex Tiling",
            "Hex tiling of the terrain layers' maps (seen from above): each texel blends three copies of the map shifted and turned at random per hexagon, weighted by their height, so no repeat shows over a slope or a field (Mikkelsen, \"Practical Real-Time Hex-Tiling\", JCGT 2022). Costs three samples per layer instead of one.",
            vec![param("hex", T::Boolean, "On or off.").required()],
            "null",
            json!({"hex": true}),
        ),
        set_tiling,
    );
    s.register_with_spec(
        spec(
            "terrain.set_cover",
            "Terrain Ground Cover",
            "Scatters small ground cover over the terrain near the camera, from its layers (automatic or painted): stones on scree (and a few on rock and soil), cushions of moss in the shade, twigs on soil and moss, and leaf litter under deciduous crowns all year. Drawn within 25 m of the eye and dissolving towards that edge; left out at LOW quality and under snow. Values are multiples of the built-in densities, 0..4; only the given fields change; `enabled: false` turns it off.",
            vec![
                param("enabled", T::Boolean, "False removes the cover.").default(json!(true)),
                param("stones", T::Number, "Stones, 0..4.").default(json!(1)),
                param("twigs", T::Number, "Twigs, 0..4.").default(json!(1)),
                param("moss", T::Number, "Moss cushions, 0..4.").default(json!(1)),
                param("litter", T::Number, "Leaf litter under crowns, 0..4 (0: only in autumn, as without cover).").default(json!(1)),
                param("seed", T::Integer, "Seed of the scatter.").default(json!(1)),
            ],
            "{enabled}",
            json!({"stones": 1.5, "moss": 2}),
        ),
        set_cover,
    );

    let rock_fields = || {
        vec![
            param(
                "size_m",
                T::Number,
                "Width (the longest side across), 0.05..30.",
            )
            .unit("m")
            .default(json!(1.0)),
            param(
                "shape",
                T::String,
                "boulder (rounded), slab (flat), outcrop (tall, craggy) or cobble (small, round).",
            )
            .allowed(&SHAPES)
            .default(json!("boulder")),
            param("yaw_deg", T::Number, "Turn about the vertical.")
                .unit("deg")
                .default(json!(0)),
            param("seed", T::Integer, "Picks the variant and its proportions.").default(json!(0)),
            param(
                "sink",
                T::Number,
                "0..0.9: share of its height sunk into the ground.",
            )
            .default(json!(0.25)),
            param("align", T::Number, "0..1: how far it tilts with the slope.").default(json!(0.7)),
            param(
                "material",
                T::String,
                "Material id (`rock`, added when missing).",
            )
            .default(json!("rock")),
            param(
                "model",
                T::String,
                "A catalogue asset drawn instead, scaled to size_m: a CC0 scan such as `boulder_01`, `rock_07`, `namaqualand_boulder_02` (catalog.search \"rock\"). The generated rock stands in while it loads.",
            ),
            param("layer", T::String, "Layer id.").default(json!("shapes")),
        ]
    };
    let mut add = vec![
        param(
            "position",
            T::Point2,
            "[x, z] of its centre; it stands on the ground there.",
        )
        .unit("m")
        .required(),
    ];
    add.extend(rock_fields());
    s.register_with_spec(
        spec(
            "rock.add",
            "Add Rock",
            "Places a rock on the ground: generated from its shape and seed (fracture faces and a rough surface), or a CC0 scan (`model`). It sits on the lowest ground under its footprint, sunk by `sink` of its height, tilted towards the slope by `align`, so no edge floats.",
            add,
            "{id}",
            json!({"position": [12, 8], "size_m": 1.4, "shape": "boulder", "yaw_deg": 30}),
        ),
        add_rock,
    );
    let mut set = vec![
        param("id", T::String, "Rock id.").required(),
        param("position", T::Point2, "[x, z] of its centre.").unit("m"),
        param("hidden", T::Boolean, "Hide it without removing it."),
    ];
    set.extend(rock_fields().into_iter().map(no_default));
    s.register_with_spec(
        spec(
            "rock.set",
            "Set Rock",
            "Changes a rock: only the given fields (`model: null` goes back to the generated rock).",
            set,
            "{id}",
            json!({"id": "rk1", "size_m": 2.0, "shape": "outcrop"}),
        ),
        set_rock,
    );
    s.register_with_spec(
        spec(
            "rock.remove",
            "Remove Rocks",
            "Removes rocks by id, or every rock of a scatter (`group`).",
            vec![
                param("ids", T::Array, "Rock ids."),
                param("group", T::String, "The group a rock.scatter answered."),
            ],
            "{removed}",
            json!({"ids": ["rk1"]}),
        ),
        remove_rocks,
    );
    s.register_with_spec(
        spec(
            "rock.scatter",
            "Scatter Rocks",
            "Scatters rocks over an outline in clusters of two to five, sizes skewed towards the small end, at random turns, as one group (one undo step; `rock.remove {group}` takes them away). With `follow_terrain` (the default) most land where the terrain shows rock or scree (automatic or painted) and on steep ground, a few elsewhere.",
            vec![
                param("outline", T::Outline, "Polygon as [x, z] pairs in metres, at least 3.").required(),
                param("density_per_100m2", T::Number, "Rocks per hundred square metres where the ground is rockiest, 0..50.").default(json!(3)),
                param("size_min_m", T::Number, "Smallest width.").unit("m").default(json!(0.4)),
                param("size_max_m", T::Number, "Largest width.").unit("m").default(json!(1.8)),
                param("shapes", T::Array, "Shapes to pick from (repeat one to weight it).").default(json!(["boulder", "boulder", "slab", "outcrop"])),
                param("models", T::Array, "Catalogue assets to pick from instead (CC0 scans); empty: generated rocks.").default(json!([])),
                param("follow_terrain", T::Boolean, "Gather on rock, scree and steep ground.").default(json!(true)),
                param("clustered", T::Boolean, "In clusters (true) or one by one.").default(json!(true)),
                param("sink", T::Number, "0..0.9: share of each rock's height sunk.").default(json!(0.3)),
                param("align", T::Number, "0..1: tilt with the slope.").default(json!(0.7)),
                param("material", T::String, "Material id.").default(json!("rock")),
                param("seed", T::Integer, "Seed.").default(json!(1)),
                param("layer", T::String, "Layer id.").default(json!("shapes")),
            ],
            "{group, count, ids}",
            json!({"outline": [[0, 0], [30, 0], [30, 20], [0, 20]], "density_per_100m2": 4}),
        ),
        scatter_rocks,
    );

    let wood_fields = || {
        vec![
            param("kind", T::String, "log (fallen trunk), stump (on its roots), snag (dead standing tree with a broken top) or branch.").allowed(&KINDS).default(json!("log")),
            param("length_m", T::Number, "A log's or branch's length, a stump's or snag's height, 0.1..40; default by kind (log 4, stump 0.5, snag 6, branch 1.6).").unit("m"),
            param("diameter_m", T::Number, "0.01..3; default by kind (log 0.35, stump 0.45, snag 0.35, branch 0.06).").unit("m"),
            param("yaw_deg", T::Number, "Turn about the vertical (a log's direction).").unit("deg").default(json!(0)),
            param("seed", T::Integer, "Picks the variant: bend, stubs, roots.").default(json!(0)),
            param("material", T::String, "Bark material id (`bark`, added when missing); cut ends are `timber`.").default(json!("bark")),
            param("model", T::String, "A catalogue asset drawn instead, scaled to length_m: a CC0 scan such as `tree_stump_01`, `dead_tree_trunk`."),
            param("layer", T::String, "Layer id.").default(json!("shapes")),
        ]
    };
    let mut add = vec![
        param(
            "position",
            T::Point2,
            "[x, z]: a lying piece's middle, a standing one's foot.",
        )
        .unit("m")
        .required(),
    ];
    add.extend(wood_fields());
    s.register_with_spec(
        spec(
            "deadwood.add",
            "Add Deadwood",
            "Places deadwood: a fallen log or branch along `yaw_deg`, pitched to the ground under its ends and sunk a little; or a stump or snag standing on the lowest ground round its foot. Generated (bark ridges, a bend, branch stubs, root flare, a broken end) or a CC0 scan (`model`).",
            add,
            "{id}",
            json!({"kind": "log", "position": [6, 4], "length_m": 5, "diameter_m": 0.4, "yaw_deg": 20}),
        ),
        add_deadwood,
    );
    let mut set = vec![
        param("id", T::String, "Deadwood id.").required(),
        param("position", T::Point2, "[x, z].").unit("m"),
        param("hidden", T::Boolean, "Hide it without removing it."),
    ];
    set.extend(wood_fields().into_iter().map(no_default));
    s.register_with_spec(
        spec(
            "deadwood.set",
            "Set Deadwood",
            "Changes a deadwood piece: only the given fields.",
            set,
            "{id}",
            json!({"id": "dw1", "length_m": 3}),
        ),
        set_deadwood,
    );
    s.register_with_spec(
        spec(
            "deadwood.remove",
            "Remove Deadwood",
            "Removes deadwood by id, or every piece of a scatter (`group`).",
            vec![
                param("ids", T::Array, "Deadwood ids."),
                param("group", T::String, "The group a deadwood.scatter answered."),
            ],
            "{removed}",
            json!({"ids": ["dw1"]}),
        ),
        remove_deadwood,
    );
    s.register_with_spec(
        spec(
            "deadwood.scatter",
            "Scatter Deadwood",
            "Scatters deadwood over an outline or over a woodland planting's ground (`planting`: an area scatter, brush or cluster): logs, stumps, snags and branches at random sizes and directions, as one group (one undo step).",
            vec![
                param("outline", T::Outline, "Polygon as [x, z] pairs in metres. Give this or `planting`."),
                param("planting", T::String, "A planting id (g1...): its planted ground."),
                param("density_per_100m2", T::Number, "Pieces per hundred square metres, 0..20.").default(json!(1.5)),
                param("kinds", T::Array, "Kinds to pick from (repeat one to weight it).").default(json!(["log", "stump", "branch", "branch", "branch"])),
                param("models", T::Array, "Catalogue assets to pick from instead (CC0 scans); empty: generated.").default(json!([])),
                param("material", T::String, "Bark material id.").default(json!("bark")),
                param("seed", T::Integer, "Seed.").default(json!(1)),
                param("layer", T::String, "Layer id.").default(json!("shapes")),
            ],
            "{group, count, ids}",
            json!({"planting": "g1", "density_per_100m2": 2}),
        ),
        scatter_deadwood,
    );
}

fn no_default(mut p: crate::spec::ParamSpec) -> crate::spec::ParamSpec {
    p.default = None;
    p
}

fn terrain_mut(project: &mut Project) -> Result<&mut Terrain, CommandError> {
    project
        .terrain
        .as_mut()
        .map(Arc::make_mut)
        .ok_or_else(|| fail("no terrain: terrain.create first"))
}

fn material(project: &mut Project, id: &str) -> Result<(), CommandError> {
    if ensure_material(project, id) {
        Ok(())
    } else {
        Err(fail(format!("no material `{id}`")))
    }
}

fn set_auto(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let Some(fields) = params.as_object() else {
        return Err(bad("expected an object"));
    };
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if project.terrain.is_none() {
        return Err(fail("no terrain: terrain.create first"));
    }
    if !enabled {
        terrain_mut(project)?.auto = None;
        return Ok(json!({"enabled": false, "layers": [], "added_layers": 0}));
    }
    let current = project
        .terrain
        .as_ref()
        .and_then(|t| t.auto.clone())
        .unwrap_or_default();
    let mut changes = serde_json::Map::new();
    for (k, v) in fields {
        if k != "enabled" {
            changes.insert(k.clone(), v.clone());
        }
    }
    let mut auto: AutoMaterials =
        merge_update(&current, &Value::Object(changes), &["rock_above_m"])?;
    auto.north_deg = project.site.north_deg as f32;
    auto.validate().map_err(fail)?;
    for id in auto.materials() {
        material(project, id)?;
    }
    let t = terrain_mut(project)?;
    let mut added = 0;
    let mut layers = Vec::new();
    for (class, m) in [
        ("rock", &auto.rock),
        ("scree", &auto.scree),
        ("moss", &auto.moss),
        ("soil", &auto.soil),
    ] {
        let Some(m) = m else {
            continue;
        };
        let index = match t.layers.iter().position(|l| l.material == *m) {
            Some(i) => i,
            None => {
                added += 1;
                t.add_layer(m).ok_or_else(|| {
                    fail(
                        "the terrain has 8 layers already: remove one or give fewer classes (null)",
                    )
                })?
            }
        };
        layers.push(json!({"class": class, "material": m, "index": index}));
    }
    t.auto = Some(auto);
    Ok(json!({"enabled": true, "layers": layers, "added_layers": added}))
}

fn set_tiling(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        hex: bool,
    }
    let p: P = parse(params)?;
    terrain_mut(project)?.hex_tiling = p.hex;
    Ok(Value::Null)
}

fn set_cover(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let Some(fields) = params.as_object() else {
        return Err(bad("expected an object"));
    };
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let t = terrain_mut(project)?;
    if !enabled {
        t.cover = None;
        return Ok(json!({"enabled": false}));
    }
    let current = t.cover.clone().unwrap_or_default();
    let changes: serde_json::Map<String, Value> = fields
        .iter()
        .filter(|(k, _)| *k != "enabled")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let cover: GroundCover = merge_update(&current, &Value::Object(changes), &[])?;
    cover.validate().map_err(fail)?;
    t.cover = Some(cover);
    Ok(json!({"enabled": true}))
}

// ---- rocks ----

fn shape_of(name: &str) -> Result<RockShape, CommandError> {
    RockShape::ALL
        .into_iter()
        .find(|s| s.name() == name)
        .ok_or_else(|| bad(&format!("unknown shape `{name}`: {}", SHAPES.join(", "))))
}

fn kind_of(name: &str) -> Result<DeadwoodKind, CommandError> {
    DeadwoodKind::ALL
        .into_iter()
        .find(|k| k.name() == name)
        .ok_or_else(|| bad(&format!("unknown kind `{name}`: {}", KINDS.join(", "))))
}

fn rock_from(project: &Project, id: String, params: &Value) -> Result<Rock, CommandError> {
    let mut v = params.clone();
    v["id"] = json!(id);
    let r: Rock = parse(&v)?;
    r.validate().map_err(fail)?;
    check_layer(project, &r.layer)?;
    Ok(r)
}

fn add_rock(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = next_id("rk", |id| project.rocks.iter().any(|r| r.id == id));
    let r = rock_from(project, id.clone(), params)?;
    material(project, &r.material)?;
    project.rocks.push(r);
    Ok(json!({"id": id}))
}

fn set_rock(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    let at = project
        .rocks
        .iter()
        .position(|r| r.id == id)
        .ok_or_else(|| fail(format!("no rock `{id}`")))?;
    let r: Rock = merge_update(&project.rocks[at], params, &["model", "group", "hidden"])?;
    r.validate().map_err(fail)?;
    check_layer(project, &r.layer)?;
    material(project, &r.material)?;
    project.rocks[at] = r;
    Ok(json!({"id": id}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    #[serde(default)]
    ids: Vec<String>,
    group: Option<String>,
}

fn removes(p: &Remove, id: &str, group: Option<&String>) -> bool {
    p.ids.iter().any(|i| i == id) || (p.group.is_some() && p.group.as_ref() == group)
}

fn remove_rocks(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let p: Remove = parse(params)?;
    if p.ids.is_empty() && p.group.is_none() {
        return Err(bad("give `ids` or `group`"));
    }
    let before = project.rocks.len();
    project
        .rocks
        .retain(|r| !removes(&p, &r.id, r.group.as_ref()));
    let removed = before - project.rocks.len();
    if removed == 0 {
        return Err(fail("no such rocks"));
    }
    Ok(json!({"removed": removed}))
}

fn scatter_rocks(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        outline: Vec<[f32; 2]>,
        #[serde(default = "three")]
        density_per_100m2: f32,
        #[serde(default = "smin")]
        size_min_m: f32,
        #[serde(default = "smax")]
        size_max_m: f32,
        #[serde(default = "shapes")]
        shapes: Vec<String>,
        #[serde(default)]
        models: Vec<String>,
        #[serde(default = "yes")]
        follow_terrain: bool,
        #[serde(default = "yes")]
        clustered: bool,
        #[serde(default = "sink")]
        sink: f32,
        #[serde(default = "align")]
        align: f32,
        #[serde(default = "rock")]
        material: String,
        #[serde(default = "one")]
        seed: u64,
        #[serde(default = "shapes_layer")]
        layer: String,
    }
    fn three() -> f32 {
        3.0
    }
    fn smin() -> f32 {
        0.4
    }
    fn smax() -> f32 {
        1.8
    }
    fn shapes() -> Vec<String> {
        ["boulder", "boulder", "slab", "outcrop"]
            .map(String::from)
            .to_vec()
    }
    fn yes() -> bool {
        true
    }
    fn sink() -> f32 {
        0.3
    }
    fn align() -> f32 {
        0.7
    }
    fn rock() -> String {
        "rock".into()
    }
    fn one() -> u64 {
        1
    }
    fn shapes_layer() -> String {
        crate::project::LAYER_SHAPES.into()
    }
    let p: P = parse(params)?;
    if p.outline.len() < 3 {
        return Err(bad("an outline needs at least 3 points"));
    }
    if !(0.0..=50.0).contains(&p.density_per_100m2) {
        return Err(bad("density_per_100m2 must lie in 0..50"));
    }
    if !(0.05..=30.0).contains(&p.size_min_m) || p.size_max_m < p.size_min_m || p.size_max_m > 30.0
    {
        return Err(bad(
            "sizes must lie in 0.05..30 with size_min_m <= size_max_m",
        ));
    }
    let shapes: Vec<RockShape> = p
        .shapes
        .iter()
        .map(|s| shape_of(s))
        .collect::<Result<_, _>>()?;
    if shapes.is_empty() {
        return Err(bad("give at least one shape"));
    }
    check_layer(project, &p.layer)?;
    material(project, &p.material)?;
    let terrain = project.terrain.clone().filter(|_| p.follow_terrain);
    let keep = |x: f32, z: f32| terrain.as_ref().map_or(1.0, |t| rockiness(t, x, z));
    let pts = scatter_points(&p.outline, p.density_per_100m2, p.seed, p.clustered, &keep);
    if pts.len() > MAX_SCATTER {
        return Err(fail(format!(
            "{} rocks is too many (at most {MAX_SCATTER}): lower the density",
            pts.len()
        )));
    }
    let group = next_id("rs", |g| {
        project.rocks.iter().any(|r| r.group.as_deref() == Some(g))
    });
    let mut ids = Vec::new();
    for (k, pt) in pts.into_iter().enumerate() {
        let k = k as i64;
        let u = unit(p.seed, k * 4);
        // Most small, a few big: sizes skewed towards the small end.
        let size = p.size_min_m + (p.size_max_m - p.size_min_m) * u * u;
        let id = next_id("rk", |id| project.rocks.iter().any(|r| r.id == id));
        let pick = |n: usize, j: i64| ((unit(p.seed, k * 4 + j) * n as f32) as usize).min(n - 1);
        project.rocks.push(Rock {
            id: id.clone(),
            layer: p.layer.clone(),
            position: pt,
            size_m: size,
            shape: shapes[pick(shapes.len(), 1)],
            yaw_deg: unit(p.seed, k * 4 + 2) * 360.0,
            seed: (unit(p.seed, k * 4 + 3) * 1e6) as u64,
            sink: p.sink.clamp(0.0, 0.9),
            align: p.align.clamp(0.0, 1.0),
            material: p.material.clone(),
            model: (!p.models.is_empty()).then(|| p.models[pick(p.models.len(), 1)].clone()),
            group: Some(group.clone()),
            hidden: false,
        });
        ids.push(id);
    }
    Ok(json!({"group": group, "count": ids.len(), "ids": ids}))
}

// ---- deadwood ----

fn deadwood_from(project: &Project, id: String, params: &Value) -> Result<Deadwood, CommandError> {
    let mut v = params.clone();
    v["id"] = json!(id);
    let kind = match v.get("kind").and_then(Value::as_str) {
        Some(k) => kind_of(k)?,
        None => DeadwoodKind::Log,
    };
    let (length, diameter) = kind.default_size();
    if v.get("length_m").is_none_or(Value::is_null) {
        v["length_m"] = json!(length);
    }
    if v.get("diameter_m").is_none_or(Value::is_null) {
        v["diameter_m"] = json!(diameter);
    }
    let d: Deadwood = parse(&v)?;
    d.validate().map_err(fail)?;
    check_layer(project, &d.layer)?;
    Ok(d)
}

fn add_deadwood(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = next_id("dw", |id| project.deadwood.iter().any(|d| d.id == id));
    let d = deadwood_from(project, id.clone(), params)?;
    material(project, &d.material)?;
    material(project, "timber")?;
    project.deadwood.push(d);
    Ok(json!({"id": id}))
}

fn set_deadwood(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    let at = project
        .deadwood
        .iter()
        .position(|d| d.id == id)
        .ok_or_else(|| fail(format!("no deadwood `{id}`")))?;
    if let Some(k) = params.get("kind").and_then(Value::as_str) {
        kind_of(k)?;
    }
    let d: Deadwood = merge_update(&project.deadwood[at], params, &["model", "group", "hidden"])?;
    d.validate().map_err(fail)?;
    check_layer(project, &d.layer)?;
    material(project, &d.material)?;
    project.deadwood[at] = d;
    Ok(json!({"id": id}))
}

fn remove_deadwood(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let p: Remove = parse(params)?;
    if p.ids.is_empty() && p.group.is_none() {
        return Err(bad("give `ids` or `group`"));
    }
    let before = project.deadwood.len();
    project
        .deadwood
        .retain(|d| !removes(&p, &d.id, d.group.as_ref()));
    let removed = before - project.deadwood.len();
    if removed == 0 {
        return Err(fail("no such deadwood"));
    }
    Ok(json!({"removed": removed}))
}

fn scatter_deadwood(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        outline: Option<Vec<[f32; 2]>>,
        planting: Option<String>,
        #[serde(default = "density")]
        density_per_100m2: f32,
        #[serde(default = "kinds")]
        kinds: Vec<String>,
        #[serde(default)]
        models: Vec<String>,
        #[serde(default = "bark")]
        material: String,
        #[serde(default = "one")]
        seed: u64,
        #[serde(default = "shapes_layer")]
        layer: String,
    }
    fn density() -> f32 {
        1.5
    }
    fn kinds() -> Vec<String> {
        ["log", "stump", "branch", "branch", "branch"]
            .map(String::from)
            .to_vec()
    }
    fn bark() -> String {
        "bark".into()
    }
    fn one() -> u64 {
        1
    }
    fn shapes_layer() -> String {
        crate::project::LAYER_SHAPES.into()
    }
    let p: P = parse(params)?;
    let outline = match (&p.outline, &p.planting) {
        (Some(o), None) => o.clone(),
        (None, Some(g)) => {
            let g = project
                .generators
                .iter()
                .find(|x| x.id() == g)
                .ok_or_else(|| fail(format!("no planting `{g}`")))?;
            crate::planting::planted_outline(g).ok_or_else(|| {
                fail("deadwood scatters over an area scatter, a brush or a cluster")
            })?
        }
        _ => return Err(bad("give `outline` or `planting`")),
    };
    if outline.len() < 3 {
        return Err(bad("an outline needs at least 3 points"));
    }
    if !(0.0..=20.0).contains(&p.density_per_100m2) {
        return Err(bad("density_per_100m2 must lie in 0..20"));
    }
    let kinds: Vec<DeadwoodKind> = p
        .kinds
        .iter()
        .map(|k| kind_of(k))
        .collect::<Result<_, _>>()?;
    if kinds.is_empty() {
        return Err(bad("give at least one kind"));
    }
    check_layer(project, &p.layer)?;
    material(project, &p.material)?;
    material(project, "timber")?;
    let pts = scatter_points(&outline, p.density_per_100m2, p.seed, false, &|_, _| 1.0);
    if pts.len() > MAX_SCATTER {
        return Err(fail(format!(
            "{} pieces is too many (at most {MAX_SCATTER}): lower the density",
            pts.len()
        )));
    }
    let group = next_id("ds", |g| {
        project
            .deadwood
            .iter()
            .any(|d| d.group.as_deref() == Some(g))
    });
    let mut ids = Vec::new();
    for (k, pt) in pts.into_iter().enumerate() {
        let k = k as i64;
        let pick = |n: usize, j: i64| ((unit(p.seed, k * 5 + j) * n as f32) as usize).min(n - 1);
        let kind = kinds[pick(kinds.len(), 0)];
        let (length, diameter) = kind.default_size();
        // From two thirds to one and a half times the usual size, thicker with longer.
        let s = 0.65 + 0.85 * unit(p.seed, k * 5 + 1);
        let id = next_id("dw", |id| project.deadwood.iter().any(|d| d.id == id));
        project.deadwood.push(Deadwood {
            id: id.clone(),
            layer: p.layer.clone(),
            kind,
            position: pt,
            length_m: length * s,
            diameter_m: diameter * (0.5 + 0.5 * s),
            yaw_deg: unit(p.seed, k * 5 + 2) * 360.0,
            seed: (unit(p.seed, k * 5 + 3) * 1e6) as u64,
            material: p.material.clone(),
            model: (!p.models.is_empty()).then(|| p.models[pick(p.models.len(), 4)].clone()),
            group: Some(group.clone()),
            hidden: false,
        });
        ids.push(id);
    }
    Ok(json!({"group": group, "count": ids.len(), "ids": ids}))
}

#[cfg(test)]
mod tests {
    use crate::Session;
    use serde_json::json;

    fn hill() -> Session {
        let mut s = Session::new(Default::default());
        s.run(
            "terrain.create",
            json!({"origin": [0, 0], "width_m": 40, "depth_m": 30, "cell_m": 0.5, "base_material": "lawn"}),
        )
        .unwrap();
        s.run(
            "terrain.stroke",
            json!({"tool": "raise", "points": [[20, 15]], "radius_m": 10, "strength": 1.5, "falloff": 0.6}),
        )
        .unwrap();
        s
    }

    #[test]
    fn auto_materials_add_their_layers_and_materials_and_undo() {
        let mut s = hill();
        let r = s
            .run("terrain.set_auto", json!({"rock_slope_deg": 30}))
            .unwrap();
        assert_eq!(r["added_layers"], 4, "{r}");
        let p = s.project();
        let t = p.terrain.as_ref().unwrap();
        assert_eq!(t.layers.len(), 5);
        assert_eq!(t.auto.as_ref().unwrap().rock_slope_deg, 30.0);
        for m in ["rock", "scree", "moss", "forest-floor"] {
            assert!(p.materials.iter().any(|x| x.id == m), "{m}");
        }
        // Only the given fields change; a typo is refused.
        s.run("terrain.set_auto", json!({"moisture": 2})).unwrap();
        let t = s.project().terrain.clone().unwrap();
        assert_eq!(t.auto.as_ref().unwrap().rock_slope_deg, 30.0);
        assert!(
            s.run("terrain.set_auto", json!({"rock_slop_deg": 3}))
                .is_err()
        );
        assert!(
            s.run("terrain.set_auto", json!({"scree_slope_deg": 60}))
                .is_err()
        );
        s.run("terrain.set_auto", json!({"enabled": false}))
            .unwrap();
        assert!(s.project().terrain.as_ref().unwrap().auto.is_none());
        assert!(s.undo());
        assert!(s.project().terrain.as_ref().unwrap().auto.is_some());
        s.run("terrain.set_tiling", json!({"hex": true})).unwrap();
        assert!(s.project().terrain.as_ref().unwrap().hex_tiling);
        s.run("terrain.set_cover", json!({"stones": 2})).unwrap();
        assert_eq!(
            s.project()
                .terrain
                .as_ref()
                .unwrap()
                .cover
                .as_ref()
                .unwrap()
                .stones,
            2.0
        );
        assert!(s.run("terrain.set_cover", json!({"stones": 9})).is_err());
    }

    #[test]
    fn rocks_are_added_set_scattered_and_removed_by_group() {
        let mut s = hill();
        s.run("terrain.set_auto", json!({})).unwrap();
        let r = s
            .run("rock.add", json!({"position": [5, 5], "size_m": 1.2}))
            .unwrap();
        assert_eq!(r["id"], "rk1");
        s.run(
            "rock.set",
            json!({"id": "rk1", "shape": "outcrop", "model": "boulder_01"}),
        )
        .unwrap();
        let rock = &s.project().rocks[0];
        assert_eq!(
            (rock.shape, rock.model.as_deref()),
            (crate::natural::RockShape::Outcrop, Some("boulder_01"))
        );
        s.run("rock.set", json!({"id": "rk1", "model": null}))
            .unwrap();
        assert!(s.project().rocks[0].model.is_none());
        assert!(
            s.run("rock.set", json!({"id": "rk1", "size_m": 99}))
                .is_err()
        );
        assert!(
            s.run("rock.add", json!({"position": [5, 5], "shape": "pebble"}))
                .is_err()
        );
        let sc = s
            .run("rock.scatter", json!({"outline": [[0, 0], [40, 0], [40, 30], [0, 30]], "density_per_100m2": 4, "seed": 3}))
            .unwrap();
        let n = sc["count"].as_u64().unwrap() as usize;
        assert!(n > 5, "{sc}");
        assert_eq!(s.project().rocks.len(), n + 1);
        // Deterministic.
        let again = s
            .run("rock.scatter", json!({"outline": [[0, 0], [40, 0], [40, 30], [0, 30]], "density_per_100m2": 4, "seed": 3}))
            .unwrap();
        assert_eq!(again["count"], sc["count"]);
        let removed = s.run("rock.remove", json!({"group": sc["group"]})).unwrap();
        assert_eq!(removed["removed"].as_u64().unwrap() as usize, n);
        assert!(s.undo());
        assert_eq!(s.project().rocks.len(), 2 * n + 1);
        // Rocks survive the project file.
        let bytes = s.project().to_json_bytes();
        let back = crate::Project::from_json_bytes(&bytes).unwrap();
        assert_eq!(back.rocks, s.project().rocks);
    }

    #[test]
    fn deadwood_takes_sizes_by_kind_and_scatters_over_a_planting() {
        let mut s = hill();
        s.run("deadwood.add", json!({"kind": "stump", "position": [3, 3]}))
            .unwrap();
        let d = &s.project().deadwood[0];
        assert_eq!((d.length_m, d.diameter_m), (0.5, 0.45));
        assert!(s.project().materials.iter().any(|m| m.id == "bark"));
        assert!(
            s.run(
                "deadwood.add",
                json!({"kind": "log", "position": [3, 3], "length_m": 0.2, "diameter_m": 0.5})
            )
            .is_err()
        );
        s.run(
            "planting.add_area",
            json!({"name": "Wood", "asset_palette": [{"asset": "flora:quercus-robur", "weight": 1}], "outline": [[0, 0], [30, 0], [30, 20], [0, 20]], "density_per_m2": 0.02}),
        )
        .unwrap();
        let g = s.project().generators[0].id().to_string();
        let r = s
            .run(
                "deadwood.scatter",
                json!({"planting": g, "density_per_100m2": 3}),
            )
            .unwrap();
        assert!(r["count"].as_u64().unwrap() > 5, "{r}");
        assert!(
            s.project()
                .deadwood
                .iter()
                .skip(1)
                .all(|d| d.position[0] <= 30.0 && d.position[1] <= 20.0)
        );
        s.run(
            "deadwood.set",
            json!({"id": "dw1", "kind": "snag", "length_m": 5}),
        )
        .unwrap();
        assert_eq!(
            s.project().deadwood[0].kind,
            crate::natural::DeadwoodKind::Snag
        );
        s.run("deadwood.remove", json!({"ids": ["dw1"]})).unwrap();
        assert!(s.project().deadwood.iter().all(|d| d.id != "dw1"));
    }
}
