//! What a design contains, as numbers: areas, lengths, volumes and counts per shape, per planting
//! and in total per material and per asset. Headless; the `query.quantities` command wraps it,
//! and `query.bloom_calendar` counts the plants with the same selection and counting.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::measure;
use crate::model::ImportedModel;
use crate::plant_ground::{placed_shape_mesh, water_areas};
use crate::planting::{Generator, Placement, evaluate, planted_outline};
use crate::project::Project;
use crate::shape::{Shape, area_lifts};

/// Area of every triangle of `model`, in m2.
fn triangle_area(model: &ImportedModel, t: [u32; 3]) -> f64 {
    let p = |i: u32| model.positions[i as usize].map(f64::from);
    let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0
}

/// A plant species (`flora:...`), as opposed to a light or object placed by a generator.
pub(crate) fn is_plant(asset: &str) -> bool {
    asset.starts_with("flora:")
}

/// Which shapes and plantings to list; `None`: all.
#[derive(Default)]
pub struct Filter<'a> {
    pub ids: Option<&'a [String]>,
    pub layer: Option<&'a str>,
}

/// The shapes and plantings a [`Filter`] selects, once checked against the project.
pub(crate) struct Selection<'a> {
    project: &'a Project,
    filter: &'a Filter<'a>,
}

/// What `filter` selects: an error on an unknown layer or id.
pub(crate) fn selection<'a>(
    project: &'a Project,
    filter: &'a Filter<'a>,
) -> Result<Selection<'a>, String> {
    if let Some(layer) = filter.layer
        && !project.layers.iter().any(|l| l.id == layer)
    {
        return Err(format!("no layer `{layer}`"));
    }
    if let Some(ids) = filter.ids {
        for id in ids {
            let known = project.shapes.iter().any(|s| s.id() == id)
                || project.generators.iter().any(|g| g.id() == id)
                || project.models.iter().any(|m| m.id == *id);
            if !known {
                return Err(format!("no shape, planting or model `{id}`"));
            }
        }
    }
    Ok(Selection { project, filter })
}

impl Selection<'_> {
    /// Whether the shape or planting `id` on `layer` is selected. Surroundings are not part of
    /// the design: listed only when named by id or by their layer.
    pub(crate) fn wanted(&self, id: &str, layer: &str) -> bool {
        let (filter, project) = (self.filter, self.project);
        let named = filter.ids.is_some_and(|ids| ids.iter().any(|i| i == id))
            || filter.layer == Some(layer);
        (filter.ids.is_none_or(|ids| ids.iter().any(|i| i == id))
            && filter.layer.is_none_or(|l| l == layer))
            && (named || !project.layer_is_context(layer))
    }
}

/// The plantings a [`Selection`] holds, and their totals.
pub(crate) struct PlantingCounts {
    /// A row per planting, in the project's order.
    pub rows: Vec<Value>,
    /// Every placement per asset: plants, and what a planting else places (lights, objects).
    pub by_asset: BTreeMap<String, usize>,
    /// Placements that are plants.
    pub plants: usize,
    pub placements: usize,
}

/// The plantings `sel` selects with their plants per asset: from the planting evaluation on
/// the terrain (height 0 without one), or as the host's `scene` draws them.
pub(crate) fn planting_counts(
    project: &Project,
    sel: &Selection<'_>,
    scene: Option<&Scene>,
) -> PlantingCounts {
    let floors = crate::terrace::ground_fn(project);
    let ground = |x: f32, z: f32| floors(x, z).or(Some(0.0));
    let mut out = PlantingCounts {
        rows: Vec::new(),
        by_asset: BTreeMap::new(),
        plants: 0,
        placements: 0,
    };
    for g in project
        .generators
        .iter()
        .filter(|g| sel.wanted(g.id(), g.layer()))
    {
        let e = match scene {
            Some(sc) => {
                let (placements, truncated) = (sc.placements)(g);
                crate::planting::Evaluation {
                    placements,
                    truncated,
                }
            }
            None => evaluate(g, &ground),
        };
        let mut own: BTreeMap<String, usize> = BTreeMap::new();
        for p in &e.placements {
            *own.entry(p.asset.clone()).or_default() += 1;
            *out.by_asset.entry(p.asset.clone()).or_default() += 1;
        }
        // Lights and objects placed along a line are placements too, but not plants.
        let own_plants = e.placements.iter().filter(|p| is_plant(&p.asset)).count();
        out.plants += own_plants;
        out.placements += e.placements.len();
        let kind = serde_json::to_value(g)
            .ok()
            .and_then(|v| v.get("type").cloned())
            .unwrap_or(Value::Null);
        let mut row = json!({
            "id": g.id(), "name": g.name(), "layer": g.layer(), "kind": kind,
            "count": e.placements.len(), "plants": own_plants, "truncated": e.truncated,
            "assets": asset_rows(&own),
        });
        let o = row.as_object_mut().expect("a row is an object");
        if let Some(outline) = planted_outline(g) {
            o.insert(
                "area_m2".into(),
                f64::from(measure::polygon_area(&outline)).into(),
            );
        }
        if let Generator::SplineMass(s) = g {
            o.insert("length_m".into(), path_length(&s.points, s.closed).into());
        }
        out.rows.push(row);
    }
    out
}

fn path_length(points: &[[f32; 2]], closed: bool) -> f64 {
    let mut pts: Vec<[f32; 3]> = points.iter().map(|p| [p[0], 0.0, p[1]]).collect();
    if closed && let Some(first) = pts.first().copied() {
        pts.push(first);
    }
    f64::from(measure::polyline_length(&pts))
}

fn material_rows(project: &Project, by: &BTreeMap<Option<usize>, f64>) -> Vec<Value> {
    let mut rows: Vec<(Option<&str>, Value)> = by
        .iter()
        .map(|(i, area)| {
            let m = i.and_then(|i| project.materials.get(i));
            (
                m.map(|m| m.id.as_str()),
                json!({"material": m.map(|m| &m.id), "name": m.map(|m| &m.name), "area_m2": area}),
            )
        })
        .collect();
    // By id, the unassigned surface last.
    rows.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    rows.into_iter().map(|r| r.1).collect()
}

fn asset_rows(by: &BTreeMap<String, usize>) -> Vec<Value> {
    by.iter()
        .map(|(asset, count)| json!({"asset": asset, "count": count}))
        .collect()
}

/// What the core cannot know alone and a host can tell: the geometry of the linked models, and a
/// generator's placements with the renderer's building rule.
pub struct Scene<'a> {
    /// A model's geometry by its id; `None` when it is not loaded.
    pub models: &'a dyn Fn(&str) -> Option<Arc<ImportedModel>>,
    /// A generator's placements as the renderer draws them, and whether it was cut short.
    pub placements: &'a dyn Fn(&Generator) -> (Vec<Placement>, bool),
}

/// Said in the command's description when only the core answers; a host replaces it with
/// [`HOST_NOTE`].
pub const CORE_NOTE: &str = "Run by the core alone, linked models (3dcraft, OBJ) are left out and plant counts are `counted_with` \"plan\": without the renderer's building rule, so plants under buildings are still counted.";

/// What a host's description of the command says in place of [`CORE_NOTE`].
pub const HOST_NOTE: &str = "Linked models (3dcraft, OBJ) are included in `models` (per body: surface, and volume when the body is closed; materials as the renderer resolves them, the transform's scale applied) and in the material totals and `totals`. Plant counts are `counted_with` \"scene\": the renderer's building rule applies, so plants under buildings are not counted (\"plan\" when the host cannot).";

/// The quantities of the project's drawn shapes and plantings, without linked models and
/// without the building rule: [`quantities_in`] with no scene.
pub fn quantities(project: &Project, filter: &Filter) -> Result<Value, String> {
    quantities_in(project, filter, None)
}

/// The quantities of the project's drawn shapes and plantings and, with a `scene`, of its linked
/// models, with plants counted by the renderer's rules. Errors with a plain message on an
/// unknown id or layer, or a shape that cannot be meshed.
pub fn quantities_in(
    project: &Project,
    filter: &Filter,
    scene: Option<&Scene>,
) -> Result<Value, String> {
    let sel = selection(project, filter)?;

    let lifts: HashMap<String, f32> = area_lifts(&project.shapes).into_iter().collect();
    let water = water_areas(project);

    let mut shapes = Vec::new();
    // Furniture by kind: the pieces with their dimensions.
    let mut furniture: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    let mut by_material: BTreeMap<Option<usize>, f64> = BTreeMap::new();
    let (mut total_plan, mut total_surface, mut total_volume) = (0.0, 0.0, 0.0);
    for shape in project
        .shapes
        .iter()
        .filter(|s| sel.wanted(s.id(), s.layer()))
    {
        let (mesh, _) = placed_shape_mesh(project, shape, &lifts, &water)
            .map_err(|e| format!("shape `{}`: {e}", shape.id()))?;
        let owners = crate::material::resolve_materials(project, shape.id(), &mesh);
        let mut surface = 0.0;
        let mut own: BTreeMap<Option<usize>, f64> = BTreeMap::new();
        for (t, f) in mesh.triangles.iter().zip(&mesh.triangle_faces) {
            let a = triangle_area(&mesh, *t);
            surface += a;
            *own.entry(owners.get(*f as usize).copied().flatten())
                .or_default() += a;
        }
        for (k, a) in &own {
            *by_material.entry(*k).or_default() += a;
        }
        let mut row = json!({
            "id": shape.id(), "name": shape.name(), "layer": shape.layer(),
            "surface_m2": surface, "materials": material_rows(project, &own),
        });
        let o = row.as_object_mut().expect("a row is an object");
        let (plan, volume);
        let tess = shape.tessellate();
        let (outline, ctrl) = (tess.pts.clone(), &tess.ctrl);
        match shape {
            Shape::Area(a) => {
                plan = f64::from(measure::polygon_area(&outline));
                volume = 0.0;
                o.insert("kind".into(), "area".into());
                o.insert("perimeter_m".into(), path_length(&outline, true).into());
                if let Some(edge) = &a.edge {
                    let names = crate::bed::EDGE_FACES.map(|n| format!("{}:{n}", a.id));
                    let (mut face_area, mut lo, mut hi) = (0.0, f32::MAX, f32::MIN);
                    for (t, f) in mesh.triangles.iter().zip(&mesh.triangle_faces) {
                        if names.contains(&mesh.faces[*f as usize].name) {
                            face_area += triangle_area(&mesh, *t);
                            for i in t {
                                let y = mesh.positions[*i as usize][1];
                                lo = lo.min(y);
                                hi = hi.max(y);
                            }
                        }
                    }
                    // An area at or below its ground has no skirt drawn.
                    let drawn = face_area > 0.0;
                    let mut row = json!({
                        "style": edge.style,
                        "thickness_m": edge.wall(),
                        "length_m": path_length(&outline, true),
                        "face_area_m2": face_area,
                        "height_m": if drawn { f64::from(hi - lo) } else { 0.0 },
                    });
                    if drawn {
                        match edge.style {
                            crate::bed::BedStyle::Sleepers => {
                                let beams = crate::bed::skirt_beams(edge, &outline, ctrl, lo, hi);
                                row["beams"] = beams.len().into();
                                row["beam_length_m"] =
                                    f64::from(beams.iter().map(|b| b.u.1 - b.u.0).sum::<f32>())
                                        .into();
                            }
                            // The sheet: the length along the outline by the height.
                            crate::bed::BedStyle::Corten => {
                                row["plate_area_m2"] =
                                    (path_length(&outline, true) * f64::from(hi - lo)).into();
                            }
                            _ => {}
                        }
                    }
                    o.insert("edge".into(), row);
                }
                if let Some(t) = &a.terrace
                    && let Some(r) = crate::terrace::report_for(project, a)
                {
                    o.insert("terrace".into(), r.json(t));
                }
            }
            Shape::Extrusion(e) if e.closed => {
                plan = f64::from(measure::polygon_area(&outline));
                volume = if let Some(st) = &e.stairs {
                    let built = st.set_in.and_then(|_| crate::setin::build_in(project, e));
                    let geometry = match &built {
                        Some(b) => Ok(b.geometry.clone()),
                        None => st.geometry_curved(&e.points, &e.curves, e.height),
                    };
                    if let Ok(g) = geometry {
                        let mut row = json!({
                            "steps": g.steps, "riser_m": g.riser, "going_m": g.tread,
                            "rise_m": e.height, "run_m": g.tread * g.steps as f32,
                            "width_m": g.width,
                        });
                        if let Some(t) = &g.turned {
                            // The landings' count and area, the flights, and the total going:
                            // the length of the centreline walked from the bottom to the top.
                            let area: f32 =
                                t.landings.iter().map(|l| l.size_m[0] * l.size_m[1]).sum();
                            row["flights"] = t.flights.len().into();
                            row["landings"] = t.landings.len().into();
                            row["landing_area_m2"] = area.into();
                            row["total_going_m"] = g.length.into();
                        }
                        if let (Some(b), Some(si)) = (&built, &st.set_in) {
                            row["goings_m"] = json!(g.goings);
                            row["set_in"] = b.report().json(si);
                        }
                        o.insert("stairs".into(), row);
                    }
                    mesh_volume(&mesh)
                } else if e.raised_bed.is_none()
                    && let Some(f) = &e.furniture
                {
                    // Pieces are not solid blocks: no volume is counted.
                    if let Ok(fr) = f.frame(&e.points, shape.is_curved(), e.height) {
                        let dims = json!({
                            "length_m": fr.dims()[0], "depth_m": fr.dims()[1], "height_m": fr.dims()[2],
                        });
                        o.insert(
                            "furniture".into(),
                            json!({
                                "kind": f.kind, "length_m": fr.dims()[0], "depth_m": fr.dims()[1],
                                "height_m": fr.dims()[2],
                            }),
                        );
                        let mut piece = json!({"id": e.id, "name": e.name});
                        piece
                            .as_object_mut()
                            .expect("an object")
                            .extend(dims.as_object().expect("an object").clone());
                        furniture.entry(f.kind.name()).or_default().push(piece);
                    }
                    0.0
                } else if let Some(bed) = &e.raised_bed {
                    if let Ok(inner) = bed.inner(&outline) {
                        let inner_area = f64::from(measure::polygon_area(&inner));
                        let depth = f64::from(crate::bed::RaisedBed::soil_top(e.height));
                        let wall_length =
                            (path_length(&outline, true) + path_length(&inner, true)) / 2.0;
                        let mut row = json!({
                            "style": bed.style,
                            "wall_thickness_m": bed.wall(),
                            "wall_length_m": wall_length,
                            "inner_area_m2": inner_area, "soil_depth_m": depth,
                            "soil_volume_m3": inner_area * depth,
                        });
                        match bed.style {
                            crate::bed::BedStyle::Sleepers => {
                                let beams = crate::bed::beams_curved(
                                    bed,
                                    &outline,
                                    ctrl,
                                    e.base,
                                    e.base + e.height,
                                );
                                row["beams"] = beams.len().into();
                                row["beam_length_m"] =
                                    f64::from(beams.iter().map(|b| b.u.1 - b.u.0).sum::<f32>())
                                        .into();
                            }
                            // The sheet the wall is cut from: its length along the middle by its height.
                            crate::bed::BedStyle::Corten => {
                                row["plate_area_m2"] = (wall_length * f64::from(e.height)).into();
                            }
                            _ => {}
                        }
                        o.insert("raised_bed".into(), row);
                    }
                    mesh_volume(&mesh)
                } else {
                    if e.house.is_some() {
                        o.insert("house".into(), house_row(e));
                    }
                    plan * f64::from(e.height)
                };
                o.insert("kind".into(), "extrusion".into());
                o.insert("perimeter_m".into(), path_length(&outline, true).into());
            }
            Shape::Extrusion(e) => {
                let length = path_length(&outline, false);
                plan = length * f64::from(e.thickness);
                // A fence is posts and boards, not a slab: its volume is the mesh's own.
                volume = match &e.fence {
                    Some(f) => {
                        let c = crate::fence::counts(f, &shape.tessellate().runs(), e.height);
                        o.insert(
                            "fence".into(),
                            json!({
                                "kind": f.kind, "posts": c.posts,
                                f.kind.item_name(): c.items, "gates": c.gates,
                                "slope": f.slope, "gravel_board": f.gravel_board,
                            }),
                        );
                        mesh_volume(&mesh)
                    }
                    None => plan * f64::from(e.height),
                };
                o.insert("kind".into(), "extrusion".into());
                o.insert("length_m".into(), length.into());
            }
        }
        o.insert("plan_area_m2".into(), plan.into());
        if let Shape::Extrusion(e) = shape {
            o.insert("height_m".into(), e.height.into());
            o.insert("volume_m3".into(), volume.into());
        }
        total_plan += plan;
        total_surface += surface;
        total_volume += volume;
        shapes.push(row);
    }

    let mut models = Vec::new();
    let mut models_missing: Vec<&str> = Vec::new();
    if let Some(sc) = scene {
        for m in project
            .models
            .iter()
            .filter(|m| sel.wanted(&m.id, &m.layer))
        {
            let Some(mesh) = (sc.models)(&m.id) else {
                models_missing.push(&m.id);
                continue;
            };
            let (row, own, surface, volume) = model_row(project, m, &mesh);
            for (k, a) in own {
                *by_material.entry(k).or_default() += a;
            }
            total_surface += surface;
            total_volume += volume;
            models.push(row);
        }
    }

    let PlantingCounts {
        rows: plantings,
        by_asset,
        plants,
        placements,
    } = planting_counts(project, &sel, scene);

    let mut out = json!({
        "counted_with": if scene.is_some() { "scene" } else { "plan" },
        "shapes": shapes,
        "furniture": furniture
            .into_iter()
            .map(|(kind, pieces)| json!({"kind": kind, "count": pieces.len(), "pieces": pieces}))
            .collect::<Vec<_>>(),
        "plantings": plantings,
        "materials": material_rows(project, &by_material),
        "assets": asset_rows(&by_asset),
        "totals": {
            "plan_area_m2": total_plan, "surface_m2": total_surface,
            "volume_m3": total_volume, "plants": plants, "placements": placements,
        },
    });
    if scene.is_some() {
        out["models"] = models.into();
        out["models_missing"] = models_missing.into();
    }
    Ok(out)
}

/// One linked model: its row, its area per material (scale applied), surface and volume.
#[allow(clippy::type_complexity)]
fn model_row(
    project: &Project,
    m: &crate::project::ModelRef,
    mesh: &ImportedModel,
) -> (Value, BTreeMap<Option<usize>, f64>, f64, f64) {
    let (k2, k3) = {
        let k = f64::from(m.transform.scale);
        (k * k, k * k * k)
    };
    let owners = crate::material::resolve_materials(project, &m.id, mesh);
    let closed = closed_bodies(mesh);
    let mut all: BTreeMap<Option<usize>, f64> = BTreeMap::new();
    let (mut surface, mut volume) = (0.0, 0.0);
    let mut bodies = Vec::new();
    for (bi, b) in mesh.bodies.iter().enumerate() {
        let range = b.triangles.start as usize..b.triangles.end as usize;
        let mut own: BTreeMap<Option<usize>, f64> = BTreeMap::new();
        let mut area = 0.0;
        for i in range.clone() {
            let a = triangle_area(mesh, mesh.triangles[i]) * k2;
            area += a;
            let face = mesh.triangle_faces[i] as usize;
            *own.entry(owners.get(face).copied().flatten()).or_default() += a;
        }
        for (k, a) in &own {
            *all.entry(*k).or_default() += a;
        }
        let vol = closed[bi].then(|| tris_volume(mesh, &mesh.triangles[range]) * k3);
        surface += area;
        volume += vol.unwrap_or(0.0);
        bodies.push(json!({
            "id": b.id, "name": b.name, "hidden": m.hides(b),
            "surface_m2": area, "volume_m3": vol,
            "materials": material_rows(project, &own),
        }));
    }
    let row = json!({
        "id": m.id, "name": m.name, "layer": m.layer, "source": m.source,
        "scale": m.transform.scale,
        "bodies": bodies, "surface_m2": surface, "volume_m3": volume,
        "open_bodies": closed.iter().filter(|c| !**c).count(),
        "materials": material_rows(project, &all),
    });
    (row, all, surface, volume)
}

/// Per body: whether its triangles close up, every edge shared by exactly two of them running
/// in opposite directions (vertices at the same place are one, as an OBJ splits them for
/// normals).
fn closed_bodies(mesh: &ImportedModel) -> Vec<bool> {
    let mut weld: HashMap<[u32; 3], u32> = HashMap::new();
    let ids: Vec<u32> = mesh
        .positions
        .iter()
        .map(|p| {
            let n = weld.len() as u32;
            *weld.entry(p.map(f32::to_bits)).or_insert(n)
        })
        .collect();
    mesh.bodies
        .iter()
        .map(|b| {
            let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
            let tris = &mesh.triangles[b.triangles.start as usize..b.triangles.end as usize];
            for t in tris {
                for k in 0..3 {
                    let (a, c) = (ids[t[k] as usize], ids[t[(k + 1) % 3] as usize]);
                    if a != c {
                        *edges.entry((a, c)).or_default() += 1;
                    }
                }
            }
            !tris.is_empty()
                && edges
                    .iter()
                    .all(|(&(a, c), &n)| n == 1 && edges.get(&(c, a)) == Some(&1))
        })
        .collect()
}

/// The volume a closed mesh with outward-facing triangles encloses (divergence theorem); parts
/// that overlap, like a rail across a board, count twice.
fn mesh_volume(mesh: &crate::model::ImportedModel) -> f64 {
    tris_volume(mesh, &mesh.triangles)
}

/// [`mesh_volume`] of some of the mesh's triangles.
fn tris_volume(mesh: &crate::model::ImportedModel, triangles: &[[u32; 3]]) -> f64 {
    let p = |i: u32| mesh.positions[i as usize].map(f64::from);
    triangles
        .iter()
        .map(|t| {
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let cross = [
                b[1] * c[2] - b[2] * c[1],
                b[2] * c[0] - b[0] * c[2],
                b[0] * c[1] - b[1] * c[0],
            ];
            (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0
        })
        .sum::<f64>()
        .abs()
}

/// A house's walls: their length along the outline (a curved edge by its true arc length), the
/// gross wall area, the openings (cut to what the wall holds, measured along the arc) and the net
/// area.
fn house_row(e: &crate::shape::Extrusion) -> Value {
    let tess = crate::curve::tessellate(&e.points, true, &e.curves, e.smooth);
    let n = e.points.len();
    let length: f64 =
        tess.runs()
            .iter()
            .enumerate()
            .map(|(i, run)| match e.curves.get(&i) {
                Some(crate::curve::Curve::Arc { sagitta_m }) => f64::from(
                    crate::curve::arc_length(e.points[i], e.points[(i + 1) % n], *sagitta_m),
                ),
                _ => path_length(run, false),
            })
            .sum();
    let Some(house) = &e.house else {
        return Value::Null;
    };
    let sign = if crate::shape::shoelace_f32(&tess.pts) > 0.0 {
        1.0
    } else {
        -1.0
    };
    let (placed, _) = crate::house::place(house, &tess, sign, (e.base, e.base + e.height));
    let holes: f64 = placed
        .iter()
        .map(|p| f64::from((p.s1 - p.s0) * (p.v1 - p.v0)))
        .sum();
    let gross = length * f64::from(e.height);
    json!({
        "wall_length_m": length,
        "wall_height_m": e.height,
        "wall_area_gross_m2": gross,
        "openings": placed.len(),
        "openings_area_m2": holes,
        "wall_area_m2": gross - holes,
    })
}
