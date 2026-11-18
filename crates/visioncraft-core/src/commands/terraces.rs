//! Terraces cut into the terrain (`terrace.add`, `terrace.set`, `terrace.clear`): a flat area
//! whose floor replaces the ground inside its outline, held by a retaining wall
//! (`crate::terrace`).

use super::levels::find_area;
use super::shapes::add_area;
use super::{fail, house::assign_defaults};
use crate::bed::BedStyle;
use crate::material::{Assignment, Target};
use crate::project::Project;
use crate::session::{CommandError, Session, parse};
use crate::shape::{Shape, shape_mesh};
use crate::spec::{ParamType as T, param, spec};
use crate::terrace::{self, Terrace};
use serde::Deserialize;
use serde_json::{Value, json};

const STYLES: [&str; 5] = ["plain", "masonry", "sleepers", "corten", "gabion"];
const FLOOR_FROM: [&str; 3] = ["lowest", "highest", "mean"];

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FloorFrom {
    Lowest,
    Highest,
    Mean,
}

pub(super) fn register(s: &mut Session) {
    let style = || {
        param("style", T::String, "The wall: plain (a solid timber wall), masonry (brick with a coping slab), sleepers (stacked timber beams, courses 0.2 m high), corten (a steel plate with a folded rim) or gabion (a stone-filled wire cage).").allowed(&STYLES)
    };
    let thickness = || {
        param("thickness_m", T::Number, "Thickness of the wall, outside the outline: plain and masonry 0.05..0.3 (default 0.2), sleepers 0.1..0.2 (default 0.15), corten 0.005..0.02 (default 0.01), gabion 0.2..0.6 (default 0.4).").unit("m")
    };
    let coping = || {
        param("coping_m", T::Number, "How far the wall's top stands above the ground where the terrace is cut into it, 0..1 (default 0.1).").unit("m")
    };
    let parapet = || {
        param("parapet_m", T::Number, "How far the wall's top stands above the floor where the terrace is built up (a seat wall), 0..1 (default 0).").unit("m")
    };
    let floor = || {
        param(
            "floor",
            T::Number,
            "The floor's absolute height, metres (the area's `elevation`).",
        )
        .unit("m")
    };
    let floor_from = || {
        param("floor_from", T::String, "Instead of `floor`: the floor at the lowest, the highest or the mean height of the terrain under the outline.").allowed(&FLOOR_FROM)
    };
    s.register_with_spec(
        spec(
            "terrace.add",
            "Add Terrace",
            "Cuts a level into the terrain: a flat area at an absolute `floor` height that replaces the ground inside its outline, held by a retaining wall along the whole outline. The terrain is not edited: its mesh gets the footprint (the outline moved out by the wall) cut out, and the wall stands in that band from the lower of the floor and the ground up to the higher of the two, plus `coping_m` above the ground where the terrace is cut into the slope, or `parapet_m` above the floor where it is built up, following the ground along the edge. Plants, stairs, beds, fences and the camera stand on the floor inside it. Faces `<id>:top` (the floor), `<id>:wall` and `<id>:wall_rim`, with the style's material (timber, brick, corten, gravel and metal for a gabion). Gives the cut and fill volumes (m3) and the wall length, and warns about a wall over 1.2 m (it needs an engineer), a terrace with no terrain under it, and a floor above the ground everywhere (use area.set_edge for that).",
            vec![
                param("name", T::String, "Display name.").default(json!("Terrace")),
                param("outline", T::Outline, "The floor's outline as [x, z] pairs in metres, either winding, not self-intersecting (the wall stands outside it). Give this or `points`."),
                param("points", T::Outline, "Same as `outline`."),
                param("curves", T::Object, "Arcs on single edges, by edge index, as for shape.add_area."),
                param("smooth", T::Boolean, "A smooth curve through all the points.").default(json!(false)),
                floor(),
                floor_from(),
                style().default(json!("plain")),
                thickness(),
                coping(),
                parapet(),
                param("material", T::String, "Material id for the floor (`<id>:top`); the wall has the style's own."),
                param("layer", T::String, "Layer id.").default(json!("shapes")),
            ],
            "{id, floor, cut_m3, fill_m3, wall_length_m, max_height_m, warnings}",
            json!({"name": "Terrace", "outline": [[11, 1], [16, 1], [16, 6], [11, 6]], "floor": 0.5, "style": "masonry"}),
        ),
        add_terrace,
    );
    s.register_with_spec(
        spec(
            "terrace.set",
            "Set Terrace",
            "Changes a terrace's floor, wall style, thickness, coping or parapet, and gives the new cut and fill. A plain flat area (not draped, no edge) becomes a terrace with it; a draped area needs `floor` or `floor_from`. Changing the style gives the wall that style's material and thickness.",
            vec![
                param("id", T::String, "The area.").required(),
                floor(),
                floor_from(),
                style(),
                thickness(),
                coping(),
                parapet(),
            ],
            "{id, floor, cut_m3, fill_m3, wall_length_m, max_height_m, warnings}",
            json!({"id": "S4", "floor": 0.3, "parapet_m": 0.45}),
        ),
        set_terrace,
    );
    s.register_with_spec(
        spec(
            "terrace.clear",
            "Clear Terrace",
            "Turns a terrace back into a plain flat area at the same height: the terrain is whole again under it and the wall and its materials go.",
            vec![param("id", T::String, "The area.").required()],
            "null",
            json!({"id": "S4"}),
        ),
        clear_terrace,
    );
}

/// The material of the wall faces: the style's, over what the faces have when `replace`.
fn wall_materials(project: &mut Project, id: &str, style: BedStyle, replace: bool) {
    if replace {
        drop_wall_materials(project, id);
    }
    assign_defaults(
        project,
        id,
        &[
            ("wall", style.default_material()),
            ("wall_rim", style.rim_material()),
        ],
    );
}

fn drop_wall_materials(project: &mut Project, id: &str) {
    project.assignments.retain(|a| {
        !matches!(&a.target, Target::Face { owner, face }
            if owner == id && terrace::FACES.iter().any(|f| face == &format!("{id}:{f}")))
    });
}

/// The floor asked for: `floor`, or a statistic of the terrain under `outline`.
fn pick_floor(
    project: &Project,
    outline: &[[f32; 2]],
    floor: Option<f32>,
    from: Option<FloorFrom>,
) -> Result<Option<f32>, CommandError> {
    match (floor, from) {
        (Some(_), Some(_)) => Err(fail("give `floor` or `floor_from`, not both")),
        (Some(f), None) if f.is_finite() => Ok(Some(f)),
        (Some(_), None) => Err(fail("`floor` must be a number")),
        (None, Some(from)) => {
            let (ground, _) = terrace::terrain_ground(project);
            let (lo, hi, mean) = terrace::ground_stats(outline, &ground)
                .ok_or_else(|| fail("the outline has no area to take the ground from"))?;
            Ok(Some(match from {
                FloorFrom::Lowest => lo,
                FloorFrom::Highest => hi,
                FloorFrom::Mean => mean,
            }))
        }
        (None, None) => Ok(None),
    }
}

/// The answer of `terrace.add` and `terrace.set`.
fn answer(project: &Project, id: &str) -> Result<Value, CommandError> {
    let Some(Shape::Area(a)) = project.shapes.iter().find(|s| s.id() == id) else {
        return Err(fail(format!("no area `{id}`")));
    };
    let t = a.terrace.as_ref().expect("a terrace");
    let r = terrace::report_for(project, a).expect("a terrace");
    let warnings = terrace::warnings(project, a, &r);
    let mut v = r.json(t);
    let o = v.as_object_mut().expect("an object");
    o.insert("id".into(), id.into());
    o.insert("floor".into(), a.elevation.into());
    o.insert("warnings".into(), warnings.into());
    Ok(v)
}

fn add_terrace(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: Option<String>,
        outline: Option<Vec<[f32; 2]>>,
        points: Option<Vec<[f32; 2]>>,
        #[serde(default)]
        curves: crate::curve::Curves,
        #[serde(default)]
        smooth: bool,
        floor: Option<f32>,
        floor_from: Option<FloorFrom>,
        style: Option<BedStyle>,
        thickness_m: Option<f32>,
        coping_m: Option<f32>,
        parapet_m: Option<f32>,
        material: Option<String>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    let outline = match (p.outline, p.points) {
        (Some(o), None) | (None, Some(o)) => o,
        (None, None) => return Err(fail("give the `outline` of the terrace")),
        (Some(_), Some(_)) => return Err(fail("give `outline` or `points`, not both")),
    };
    if let Some(m) = &p.material
        && project.materials.iter().all(|x| &x.id != m)
    {
        return Err(fail(format!("no material `{m}`")));
    }
    let mut area = json!({
        "name": p.name.unwrap_or_else(|| "Terrace".into()),
        "outline": outline, "curves": p.curves, "smooth": p.smooth,
        "elevation": 0.0, "drape": false,
    });
    if let Some(l) = p.layer {
        area["layer"] = l.into();
    }
    let id = add_area(project, &area)?["id"]
        .as_str()
        .expect("add returns an id")
        .to_string();
    let (at, mut shape) = find_area(project, &id)?;
    let Shape::Area(a) = &mut shape else {
        unreachable!("find_area returns an area")
    };
    let floor = pick_floor(project, &a.outline(), p.floor, p.floor_from)?
        .ok_or_else(|| fail("give the floor: `floor` (a height in metres) or `floor_from`"))?;
    let t = Terrace {
        style: p.style.unwrap_or_default(),
        thickness_m: p.thickness_m,
        coping_m: p.coping_m.unwrap_or(terrace::DEFAULT_COPING_M),
        parapet_m: p.parapet_m.unwrap_or(0.0),
    };
    a.elevation = floor;
    a.terrace = Some(t);
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    wall_materials(project, &id, t.style, false);
    if let Some(material) = p.material {
        project.assignments.push(Assignment {
            target: Target::Face {
                owner: id.clone(),
                face: format!("{id}:top"),
            },
            material,
        });
    }
    answer(project, &id)
}

fn set_terrace(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        floor: Option<f32>,
        floor_from: Option<FloorFrom>,
        style: Option<BedStyle>,
        thickness_m: Option<f32>,
        coping_m: Option<f32>,
        parapet_m: Option<f32>,
    }
    let p: P = parse(params)?;
    let (at, mut shape) = find_area(project, &p.id)?;
    let Shape::Area(a) = &mut shape else {
        unreachable!("find_area returns an area")
    };
    if a.edge.is_some() {
        return Err(fail(
            "this area has an edge, and a terrace replaces it: area.clear_edge first",
        ));
    }
    let floor = pick_floor(project, &a.outline(), p.floor, p.floor_from)?;
    let before = a.terrace;
    let mut t = before.unwrap_or_default();
    if before.is_none() {
        if a.drape && floor.is_none() {
            return Err(fail(
                "this area lies on the ground (`drape`): give the terrace's `floor` or `floor_from`",
            ));
        }
        a.drape = false;
    }
    let restyled = p.style.is_some_and(|s| s != t.style);
    if let Some(style) = p.style {
        t.style = style;
        // The style's own thickness, unless one is given.
        if restyled {
            t.thickness_m = None;
        }
    }
    if p.thickness_m.is_some() {
        t.thickness_m = p.thickness_m;
    }
    if let Some(c) = p.coping_m {
        t.coping_m = c;
    }
    if let Some(v) = p.parapet_m {
        t.parapet_m = v;
    }
    if let Some(f) = floor {
        a.elevation = f;
    }
    a.terrace = Some(t);
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    wall_materials(project, &p.id, t.style, restyled);
    answer(project, &p.id)
}

fn clear_terrace(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
    }
    let p: P = parse(params)?;
    let (at, mut shape) = find_area(project, &p.id)?;
    let Shape::Area(a) = &mut shape else {
        unreachable!("find_area returns an area")
    };
    if a.terrace.is_none() {
        return Err(fail(format!("`{}` is not a terrace", p.id)));
    }
    a.terrace = None;
    project.shapes[at] = shape;
    drop_wall_materials(project, &p.id);
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    /// A 40 x 30 m terrain sloping 1 in 5 down towards +x: the ground is `2 - 0.2 x`.
    fn sloped() -> Session {
        let mut t = crate::Terrain::flat([0.0, 0.0], 40.0, 30.0, 0.5, "lawn").unwrap();
        let w = t.width as usize + 1;
        for (i, h) in t.heights.iter_mut().enumerate() {
            *h = 2.0 - 0.2 * (i % w) as f32 * t.cell_m;
        }
        Session::new(Project {
            terrain: Some(std::sync::Arc::new(t)),
            ..Project::default()
        })
    }

    fn outline() -> Value {
        json!([[5, 5], [11, 5], [11, 11], [5, 11]])
    }

    fn area(s: &Session, id: &str) -> crate::shape::Area {
        match s.project().shapes.iter().find(|x| x.id() == id) {
            Some(Shape::Area(a)) => a.clone(),
            _ => panic!("no area {id}"),
        }
    }

    #[test]
    fn add_makes_the_area_the_terrace_and_the_numbers_and_can_be_undone() {
        let mut s = sloped();
        let before = s.project().shapes.len();
        let r = s
            .run(
                "terrace.add",
                json!({"name": "Level", "outline": outline(), "floor": 0.5, "style": "masonry"}),
            )
            .unwrap();
        let id = r["id"].as_str().unwrap();
        let a = area(&s, id);
        assert!(!a.drape && a.elevation == 0.5);
        assert_eq!(a.terrace.unwrap().style, BedStyle::Masonry);
        // The ground is 1.0 at x = 5 and -0.2 at x = 11. Cut: x in 5..7.5, depth 0.5 - 0.2 (x - 5),
        // 6 m across.
        let cut: f64 = 0.5 * 2.5 * 0.5 * 6.0;
        // fill: x in 7.5..11, depth 0.2 (x - 7.5), 6 m across.
        let fill: f64 = 0.5 * 3.5 * 0.7 * 6.0;
        assert!((r["cut_m3"].as_f64().unwrap() - cut).abs() < 0.02 * cut);
        assert!((r["fill_m3"].as_f64().unwrap() - fill).abs() < 0.02 * fill);
        assert!(r["wall_length_m"].as_f64().unwrap() > 24.0);
        assert!(r["warnings"].as_array().unwrap().is_empty(), "{r}");
        // The wall faces start as brick.
        let m = crate::shape_mesh(&s.project().shapes[before]).unwrap();
        assert_eq!(m.faces.len(), 3);
        assert!(s.project().assignments.iter().any(|x| matches!(&x.target,
            Target::Face { face, .. } if face == &format!("{id}:wall"))
            && x.material == "brick"));
        s.undo();
        assert_eq!(s.project().shapes.len(), before);
    }

    #[test]
    fn floor_from_takes_a_statistic_of_the_ground_under_the_outline() {
        let mut s = sloped();
        let mut floors = Vec::new();
        for from in ["lowest", "highest", "mean"] {
            let r = s
                .run(
                    "terrace.add",
                    json!({"outline": outline(), "floor_from": from}),
                )
                .unwrap();
            floors.push(r["floor"].as_f64().unwrap());
        }
        // 2 - 0.2 x over x = 5..11: 1.0 down to -0.2, mean 0.4.
        assert!((floors[0] + 0.2).abs() < 0.02, "{floors:?}");
        assert!((floors[1] - 1.0).abs() < 0.02, "{floors:?}");
        assert!((floors[2] - 0.4).abs() < 0.02, "{floors:?}");
        // The mean cuts and fills the same.
        let r = s
            .run(
                "terrace.add",
                json!({"outline": [[20, 5], [26, 5], [26, 11], [20, 11]], "floor_from": "mean"}),
            )
            .unwrap();
        let (cut, fill) = (
            r["cut_m3"].as_f64().unwrap(),
            r["fill_m3"].as_f64().unwrap(),
        );
        assert!((cut - fill).abs() < 0.03 * cut.max(fill), "{r}");
        // Both, or neither, are refused.
        assert!(
            s.run(
                "terrace.add",
                json!({"outline": outline(), "floor": 1, "floor_from": "mean"})
            )
            .is_err()
        );
        assert!(s.run("terrace.add", json!({"outline": outline()})).is_err());
        assert!(s.run("terrace.add", json!({"floor": 1})).is_err());
    }

    #[test]
    fn warnings_for_a_tall_wall_no_terrain_and_a_floor_above_the_ground() {
        let mut s = sloped();
        // 2.0 m of cut at the high edge on a floor of -1.
        let r = s
            .run(
                "terrace.add",
                json!({"outline": [[2, 5], [8, 5], [8, 11], [2, 11]], "floor": -1.0}),
            )
            .unwrap();
        let w = r["warnings"].as_array().unwrap();
        assert!(
            w.iter().any(|w| w.as_str().unwrap().contains("engineer")),
            "{r}"
        );
        // Above the ground everywhere (the ground is at most 2).
        let r = s
            .run(
                "terrace.add",
                json!({"outline": [[30, 5], [36, 5], [36, 11], [30, 11]], "floor": 5.0}),
            )
            .unwrap();
        let w = r["warnings"].as_array().unwrap();
        assert!(
            w.iter()
                .any(|w| w.as_str().unwrap().contains("area.set_edge")),
            "{r}"
        );
        // Without a terrain.
        let mut bare = Session::new(Project::default());
        let r = bare
            .run("terrace.add", json!({"outline": outline(), "floor": 0.0}))
            .unwrap();
        let w = r["warnings"].as_array().unwrap();
        assert!(
            w.iter().any(|w| w.as_str().unwrap().contains("no terrain")),
            "{r}"
        );
    }

    #[test]
    fn set_changes_the_terrace_and_turns_a_flat_area_into_one() {
        let mut s = sloped();
        let r = s
            .run("terrace.add", json!({"outline": outline(), "floor": 0.5}))
            .unwrap();
        let id = r["id"].as_str().unwrap().to_string();
        let r = s
            .run(
                "terrace.set",
                json!({"id": id, "floor": 0.8, "style": "sleepers", "parapet_m": 0.45}),
            )
            .unwrap();
        assert_eq!(r["floor"], 0.8f32);
        assert!(r["beams"].as_u64().unwrap() > 10, "{r}");
        let a = area(&s, &id);
        let t = a.terrace.unwrap();
        assert_eq!(
            (t.style, t.parapet_m, a.elevation),
            (BedStyle::Sleepers, 0.45, 0.8)
        );
        assert!(s.project().assignments.iter().any(|x| matches!(&x.target,
            Target::Face { face, .. } if face == &format!("{id}:wall"))
            && x.material == "timber"));
        // A thickness the style cannot take is refused and changes nothing.
        let was = area(&s, &id);
        assert!(
            s.run("terrace.set", json!({"id": id, "thickness_m": 0.5}))
                .is_err()
        );
        assert_eq!(area(&s, &id), was);
        // A plain flat area becomes a terrace at its own height.
        let flat = s
            .run(
                "shape.add_area",
                json!({"name": "Deck", "outline": [[20, 5], [24, 5], [24, 9], [20, 9]], "elevation": 0.3, "drape": false}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        s.run("terrace.set", json!({"id": flat, "style": "gabion"}))
            .unwrap();
        let a = area(&s, &flat);
        assert_eq!(
            (a.elevation, a.terrace.unwrap().style),
            (0.3, BedStyle::Gabion)
        );
        // A draped area needs a floor.
        let lawn = s
            .run(
                "shape.add_area",
                json!({"name": "Lawn", "outline": [[28, 5], [32, 5], [32, 9], [28, 9]]}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            s.run("terrace.set", json!({"id": lawn, "style": "plain"}))
                .is_err()
        );
        s.run("terrace.set", json!({"id": lawn, "floor_from": "lowest"}))
            .unwrap();
        assert!(!area(&s, &lawn).drape);
        assert!(
            s.run("terrace.set", json!({"id": "S99", "floor": 1}))
                .is_err()
        );
    }

    #[test]
    fn clear_turns_it_back_into_a_plain_area_and_a_terrace_and_an_edge_exclude_each_other() {
        let mut s = sloped();
        let id = s
            .run("terrace.add", json!({"outline": outline(), "floor": 0.5}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        // An edge on a terrace is refused, and a terrace on an edged area.
        assert!(
            s.run("area.set_edge", json!({"id": id, "style": "plain"}))
                .is_err()
        );
        let raised = s
            .run(
                "shape.add_area",
                json!({"name": "Raised", "outline": [[20, 5], [24, 5], [24, 9], [20, 9]], "elevation": 1.0, "drape": false, "edge": {"style": "plain"}}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            s.run("terrace.set", json!({"id": raised, "floor": 1.0}))
                .is_err()
        );
        assert!(s.run("terrace.clear", json!({"id": raised})).is_err());
        s.run("terrace.clear", json!({"id": id})).unwrap();
        let a = area(&s, &id);
        assert!(a.terrace.is_none() && a.elevation == 0.5);
        assert_eq!(
            crate::shape_mesh(&s.project().shapes[0])
                .unwrap()
                .faces
                .len(),
            1
        );
        assert!(!s.project().assignments.iter().any(|x| matches!(&x.target,
            Target::Face { face, .. } if face.contains(":wall"))));
        s.undo();
        assert!(area(&s, &id).terrace.is_some());
    }

    #[test]
    fn a_terrace_survives_a_round_trip_and_an_old_project_loads_without_the_field() {
        let mut s = sloped();
        let id = s
            .run(
                "terrace.add",
                json!({"outline": outline(), "floor": 0.5, "style": "gabion", "parapet_m": 0.3, "coping_m": 0.2, "thickness_m": 0.5}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let text = serde_json::to_string(&s.project().shapes).unwrap();
        let back: Vec<Shape> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s.project().shapes);
        // The default wall writes almost nothing, and an area without the field loads.
        let plain = Terrace::default();
        assert_eq!(serde_json::to_value(plain).unwrap(), json!({}));
        let mut v = serde_json::to_value(&s.project().shapes[0]).unwrap();
        v.as_object_mut().unwrap().remove("terrace");
        let old: Shape = serde_json::from_value(v).unwrap();
        let Shape::Area(old) = old else { panic!() };
        assert!(old.terrace.is_none() && old.id == id);
        let t: Terrace = serde_json::from_value(json!({"style": "masonry"})).unwrap();
        assert_eq!(t.coping_m, terrace::DEFAULT_COPING_M);
    }

    #[test]
    fn quantities_report_the_terrace() {
        let mut s = sloped();
        let id = s
            .run(
                "terrace.add",
                json!({"outline": outline(), "floor": 0.5, "style": "corten"}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let q = s.run("query.quantities", json!({})).unwrap();
        let row = q["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id.as_str())
            .unwrap();
        let t = &row["terrace"];
        assert_eq!(t["style"], "corten");
        assert!(t["cut_m3"].as_f64().unwrap() > 1.0 && t["fill_m3"].as_f64().unwrap() > 1.0);
        assert!(t["wall_length_m"].as_f64().unwrap() > 24.0);
        assert!(t["outer_face_area_m2"].as_f64().unwrap() > 1.0);
        assert!(t["plate_area_m2"].as_f64().unwrap() > 1.0);
    }
}
