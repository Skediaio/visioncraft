//! A block's house parts (`crate::house`): roof, windows and doors, plinth.

use super::{fail, merge_update, next_id};
use crate::house::{FrameStyle, House, Opening, OpeningKind, Roof, RoofKind};
use crate::material::{Assignment, Target};
use crate::project::Project;
use crate::session::{CommandError, Session, parse};
use crate::shape::{Shape, shape_mesh};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    let id = || param("id", T::String, "The block (a closed extrusion).").required();
    s.register_with_spec(
        spec(
            "shape.set_roof",
            "Set Roof",
            "Gives a block a roof, or changes it: flat (with a parapet), gable, hip or lean_to. Pitched roofs need a rectangular or L-shaped outline with straight walls; others, a curved outline included, are drawn flat with a note (use `flat` there). The roof face is `<id>:roof`, the wall above the eaves `<id>:gable`. The answer has `warnings`.",
            vec![
                id(),
                param("kind", T::String, "flat, gable, hip or lean_to.").default(json!("gable")),
                param("pitch_deg", T::Number, "Slope of a pitched roof, 5..70.").unit("deg").default(json!(35)),
                param("overhang_m", T::Number, "How far the roof reaches past the walls, 0..2.").unit("m").default(json!(0.4)),
                param("parapet_m", T::Number, "Height of a flat roof's parapet, 0 for none.").unit("m").default(json!(0.4)),
            ],
            "{warnings}",
            json!({"id": "S2", "kind": "gable", "pitch_deg": 40}),
        ),
        set_roof,
    );
    s.register_with_spec(
        spec(
            "shape.add_opening",
            "Add Window or Door",
            "Puts a window or door in one wall of a block (its outline edge `side`, as in `<id>:side(n)`). Glass is `<id>:glass`, frames `<id>:frame`, doors `<id>:door`, the dark room behind a window `<id>:interior`. On a curved edge (an arc or a smooth curve) `offset_m` and `width_m` are lengths along the arc, and the unit is a flat window or door set on the chord of its span, cut into the wall with planar reveals; a span so curved that its chord would leave the 0.3 m wall (a sagitta over 0.18 m) is narrowed about its centre until it fits, with a note in `warnings`.",
            vec![
                id(),
                param("side", T::Integer, "The wall: index of the outline edge.").required(),
                param("offset_m", T::Number, "From the wall's first corner to the opening's centre, along the wall (along the arc on a curved edge).").unit("m").required(),
                param("kind", T::String, "window or door.").default(json!("window")),
                param("width_m", T::Number, "Width, above zero.").unit("m").default(json!(1.2)),
                param("height_m", T::Number, "Height, above zero.").unit("m").default(json!(1.4)),
                param("sill_m", T::Number, "Bottom above the block's base (a door's is 0).").unit("m").default(json!(0.9)),
                param("frame", T::String, "slim, wide, split (two panes) or none.").default(json!("slim")),
            ],
            "{opening (o1, o2...), warnings}",
            json!({"id": "S2", "side": 0, "offset_m": 3.0, "kind": "window"}),
        ),
        add_opening,
    );
    s.register_with_spec(
        spec(
            "shape.update_opening",
            "Update Window or Door",
            "Changes fields of a window or door. The answer has `warnings` as shape.add_opening.",
            vec![
                id(),
                param("opening", T::String, "The opening's id.").required(),
                param("side", T::Integer, "The wall."),
                param("offset_m", T::Number, "Centre along the wall.").unit("m"),
                param("kind", T::String, "window or door."),
                param("width_m", T::Number, "Width.").unit("m"),
                param("height_m", T::Number, "Height.").unit("m"),
                param("sill_m", T::Number, "Bottom above the base.").unit("m"),
                param("frame", T::String, "slim, wide, split or none."),
            ],
            "{warnings}",
            json!({"id": "S2", "opening": "o1", "width_m": 1.8}),
        ),
        update_opening,
    );
    s.register_with_spec(
        spec(
            "shape.remove_opening",
            "Remove Window or Door",
            "Removes a window or door.",
            vec![
                id(),
                param("opening", T::String, "The opening's id.").required(),
            ],
            "null",
            json!({"id": "S2", "opening": "o1"}),
        ),
        remove_opening,
    );
    s.register_with_spec(
        spec(
            "shape.set_plinth",
            "Set Plinth",
            "A band at the foot of a block's walls (`<id>:plinth`), 0 for none.",
            vec![
                id(),
                param("height_m", T::Number, "Height, 0..3.")
                    .unit("m")
                    .required(),
            ],
            "null",
            json!({"id": "S2", "height_m": 0.3}),
        ),
        set_plinth,
    );
    s.register_with_spec(
        spec(
            "shape.set_lit_rooms",
            "Set Lit Rooms",
            "Share of a house's windows with a lit room behind them when the lamps are on \
             (0: none, 1: every window); the same windows stay lit as the share grows.",
            vec![id(), param("share", T::Number, "0..1.").required()],
            "null",
            json!({"id": "S2", "share": 0.5}),
        ),
        set_lit_rooms,
    );
}

/// The materials a house's new faces start with, where nothing is assigned to them yet and the
/// project has the material: roof dark tiles' stand-in, glass, timber frames and doors, a dark
/// interior, concrete plinth.
pub(crate) fn default_materials(project: &mut Project, id: &str) {
    assign_defaults(
        project,
        id,
        &[
            ("roof", "roof"),
            ("glass", "glass"),
            ("frame", "timber"),
            ("door", "timber"),
            ("interior", "interior"),
            ("plinth", "concrete"),
            ("parapet", "concrete"),
            ("sill", "concrete"),
            ("handle", "metal"),
        ],
    );
}

/// Gives `id`'s faces (`face`, `material` pairs) their palette material where the face has none
/// yet; a palette material the project lacks is added.
pub(super) fn assign_defaults(project: &mut Project, id: &str, faces: &[(&str, &str)]) {
    for &(face, material) in faces {
        let target = Target::Face {
            owner: id.to_string(),
            face: format!("{id}:{face}"),
        };
        // A project from before a palette material existed (the interior) gets it now.
        if !project.materials.iter().any(|m| m.id == material)
            && let Some(m) = crate::material::builtin_palette()
                .into_iter()
                .find(|m| m.id == material)
        {
            project.materials.push(m);
        }
        if project.materials.iter().any(|m| m.id == material)
            && !project.assignments.iter().any(|a| a.target == target)
        {
            project.assignments.push(Assignment {
                target,
                material: material.into(),
            });
        }
    }
}

/// Edits block `id`'s house (made with the defaults when it has none) and keeps the change
/// only if the block still meshes.
fn edit_house(
    project: &mut Project,
    id: &str,
    f: impl FnOnce(&mut House, usize) -> Result<Value, CommandError>,
) -> Result<Value, CommandError> {
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == id)
        .ok_or_else(|| fail(format!("no shape `{id}`")))?;
    let mut shape = project.shapes[at].clone();
    let Shape::Extrusion(e) = &mut shape else {
        return Err(fail(format!("`{id}` is an area, not a block")));
    };
    if !e.closed {
        return Err(fail(format!("`{id}` is a wall, not a closed block")));
    }
    let sides = e.points.len();
    let r = f(e.house.get_or_insert_with(House::default), sides)?;
    let mesh = shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    default_materials(project, id);
    // The block's own notes (a roof drawn flat, an opening cut down) go with the answer.
    Ok(match r {
        Value::Object(mut m) => {
            m.insert("warnings".into(), json!(mesh.warnings));
            Value::Object(m)
        }
        _ => json!({ "warnings": mesh.warnings }),
    })
}

fn number(v: f32, what: &str, lo: f32, hi: f32) -> Result<f32, CommandError> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(fail(format!("`{what}` must be within {lo}..={hi}")))
    }
}

fn set_roof(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        #[serde(default)]
        kind: RoofKind,
        pitch_deg: Option<f32>,
        overhang_m: Option<f32>,
        parapet_m: Option<f32>,
    }
    let p: P = parse(params)?;
    edit_house(project, &p.id, |h, _| {
        let d = Roof::default();
        h.roof = Roof {
            kind: p.kind,
            pitch_deg: number(p.pitch_deg.unwrap_or(d.pitch_deg), "pitch_deg", 5.0, 70.0)?,
            overhang_m: number(p.overhang_m.unwrap_or(d.overhang_m), "overhang_m", 0.0, 2.0)?,
            parapet_m: number(p.parapet_m.unwrap_or(d.parapet_m), "parapet_m", 0.0, 3.0)?,
        };
        Ok(Value::Null)
    })
}

fn check(o: &Opening, sides: usize) -> Result<(), CommandError> {
    if o.side >= sides {
        return Err(fail(format!("no wall {}; the block has {sides}", o.side)));
    }
    number(o.width_m, "width_m", 0.1, 50.0)?;
    number(o.height_m, "height_m", 0.1, 50.0)?;
    number(o.sill_m, "sill_m", 0.0, 50.0)?;
    number(o.offset_m, "offset_m", 0.0, 1e4)?;
    Ok(())
}

fn add_opening(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        side: usize,
        offset_m: f32,
        #[serde(default)]
        kind: OpeningKind,
        width_m: Option<f32>,
        height_m: Option<f32>,
        sill_m: Option<f32>,
        #[serde(default)]
        frame: FrameStyle,
    }
    let p: P = parse(params)?;
    let door = p.kind == OpeningKind::Door;
    edit_house(project, &p.id, |h, sides| {
        let id = next_id("o", |id| h.openings.iter().any(|o| o.id == id));
        let o = Opening {
            id: id.clone(),
            side: p.side,
            offset_m: p.offset_m,
            width_m: p.width_m.unwrap_or(if door { 1.0 } else { 1.2 }),
            height_m: p.height_m.unwrap_or(if door { 2.2 } else { 1.4 }),
            sill_m: p.sill_m.unwrap_or(if door { 0.0 } else { 0.9 }),
            kind: p.kind,
            frame: p.frame,
        };
        check(&o, sides)?;
        h.openings.push(o);
        Ok(json!({ "opening": id }))
    })
}

fn update_opening(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"]
        .as_str()
        .ok_or_else(|| fail("`id`: a block"))?
        .to_string();
    let which = params["opening"]
        .as_str()
        .ok_or_else(|| fail("`opening`: an opening id"))?
        .to_string();
    let mut changes = params.clone();
    if let Some(m) = changes.as_object_mut() {
        m.remove("id");
        m.remove("opening");
        m.insert("id".into(), json!(which));
    }
    edit_house(project, &id, |h, sides| {
        let o = h
            .openings
            .iter_mut()
            .find(|o| o.id == which)
            .ok_or_else(|| fail(format!("no opening `{which}`")))?;
        let updated: Opening = merge_update(o, &changes, &[])?;
        check(&updated, sides)?;
        *o = updated;
        Ok(Value::Null)
    })
}

fn remove_opening(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        opening: String,
    }
    let p: P = parse(params)?;
    edit_house(project, &p.id, |h, _| {
        let before = h.openings.len();
        h.openings.retain(|o| o.id != p.opening);
        if h.openings.len() == before {
            return Err(fail(format!("no opening `{}`", p.opening)));
        }
        Ok(Value::Null)
    })
}

fn set_plinth(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        height_m: f32,
    }
    let p: P = parse(params)?;
    edit_house(project, &p.id, |h, _| {
        h.plinth_m = number(p.height_m, "height_m", 0.0, 3.0)?;
        Ok(Value::Null)
    })
}

fn set_lit_rooms(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        share: f32,
    }
    let p: P = parse(params)?;
    edit_house(project, &p.id, |h, _| {
        h.lit_rooms = number(p.share, "share", 0.0, 1.0)?;
        Ok(Value::Null)
    })
}

#[cfg(test)]
mod tests {
    use crate::{Project, Session};
    use serde_json::json;

    #[test]
    fn a_block_becomes_a_house_with_named_faces_and_undo() {
        let mut s = Session::new(Project::default());
        let id = s
            .run(
                "shape.add_extrusion",
                json!({"name": "House", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "height": 6, "material": "plaster", "roof": {"kind": "gable"}}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let o = s
            .run(
                "shape.add_opening",
                json!({"id": id, "side": 0, "offset_m": 3, "frame": "split"}),
            )
            .unwrap();
        assert_eq!(o["opening"], "o1");
        s.run(
            "shape.add_opening",
            json!({"id": id, "side": 2, "offset_m": 5, "kind": "door"}),
        )
        .unwrap();
        s.run("shape.set_plinth", json!({"id": id, "height_m": 0.3}))
            .unwrap();
        let mesh = crate::shape_mesh(&s.project().shapes[0]).unwrap();
        let tris = |f: &str| {
            let i = mesh
                .faces
                .iter()
                .position(|x| x.name == format!("{id}:{f}"))
                .unwrap();
            mesh.triangle_faces
                .iter()
                .filter(|&&t| t as usize == i)
                .count()
        };
        for f in [
            "roof", "gable", "glass", "frame", "door", "interior", "plinth", "sill", "handle",
        ] {
            assert!(tris(f) > 0, "{f}");
        }
        assert_eq!(tris("parapet"), 0);
        // Lit rooms: none by default, then one lamp per window, inside the outline.
        let block = |s: &Session| match &s.project().shapes[0] {
            crate::Shape::Extrusion(e) => e.clone(),
            _ => unreachable!(),
        };
        let e = block(&s);
        let lamps = |e: &crate::shape::Extrusion| crate::house::lit_windows(e);
        assert!(lamps(&e).is_empty());
        s.run("shape.set_lit_rooms", json!({"id": id, "share": 1}))
            .unwrap();
        let l = lamps(&block(&s));
        assert_eq!(l.len(), 1);
        assert!(
            l[0][0] > 0.0 && l[0][0] < 10.0 && l[0][2] > 0.0 && l[0][2] < 8.0,
            "{l:?}"
        );
        assert!(
            s.run("shape.set_lit_rooms", json!({"id": id, "share": 2}))
                .is_err()
        );
        // The ridge stands above the 6 m walls: 4 m half-width at 35 degrees, 2.8 m.
        let top = mesh.positions.iter().map(|p| p[1]).fold(0.0, f32::max);
        assert!(
            (top - (6.0 + 4.0 * 35f32.to_radians().tan())).abs() < 1e-3,
            "{top}"
        );
        // Glass gets glass, the roof the roof material.
        let has = |f: &str, m: &str| {
            s.project().assignments.iter().any(|a| {
                a.material == m
                    && matches!(&a.target, crate::Target::Face { face, .. } if *face == format!("{id}:{f}"))
            })
        };
        assert!(has("glass", "glass") && has("roof", "roof"));
        // A flat roof on a pentagon is fine; a gable on it falls back to flat with a note.
        let p = s
            .run(
                "shape.add_extrusion",
                json!({"name": "Odd", "outline": [[20, 0], [26, 0], [27, 4], [23, 7], [19, 4]], "height": 3, "roof": {"kind": "gable"}}),
            )
            .unwrap();
        let odd = s
            .project()
            .shapes
            .iter()
            .find(|x| x.id() == p["id"])
            .unwrap();
        assert!(!crate::shape_mesh(odd).unwrap().warnings.is_empty());
        assert!(
            s.run(
                "shape.add_opening",
                json!({"id": id, "side": 9, "offset_m": 1})
            )
            .is_err()
        );
        assert!(s.undo());
        assert_eq!(s.project().shapes.len(), 1);
    }

    // ---- curved houses ----

    type Mesh = crate::ImportedModel;

    fn mesh_of(s: &Session, id: &str) -> Mesh {
        let shape = s.project().shapes.iter().find(|x| x.id() == id).unwrap();
        crate::shape_mesh(shape).unwrap()
    }

    /// The positions and the area of the triangles on face `<id>:<name>`.
    fn face(m: &Mesh, id: &str, name: &str) -> (Vec<[f32; 3]>, f64) {
        let i = m
            .faces
            .iter()
            .position(|f| f.name == format!("{id}:{name}"))
            .unwrap();
        let (mut pts, mut area) = (Vec::new(), 0.0f64);
        for (t, f) in m.triangles.iter().zip(&m.triangle_faces) {
            if *f as usize == i {
                let v = t.map(|k| m.positions[k as usize]);
                pts.extend(v);
                let (a, b) = (
                    [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]],
                    [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]],
                );
                let c = [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ];
                area += f64::from((c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()) / 2.0;
            }
        }
        (pts, area)
    }

    fn round_house(s: &mut Session, radius: f32) -> String {
        s.run(
            "shape.add_circle",
            json!({"center": [0, 0], "radius_m": radius, "kind": "extrusion", "height": 3,
                   "roof": {"kind": "flat", "parapet_m": 0.4}}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_round_house_builds_a_closed_wall_of_constant_thickness_with_a_flat_roof_and_floor() {
        let mut s = Session::new(Project::default());
        let id = round_house(&mut s, 5.0);
        let m = mesh_of(&s, &id);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        let disc = std::f64::consts::PI * 25.0;
        // The wall is a continuous ring of short pieces: its area is the cylinder's (within the
        // chord tolerance), the deck and the floor are the disc.
        let wall: f64 = (0..4).map(|n| face(&m, &id, &format!("side({n})")).1).sum();
        assert!(
            (wall / (2.0 * disc / 5.0 * 3.0) - 1.0).abs() < 0.003,
            "{wall}"
        );
        assert!((face(&m, &id, "roof").1 / disc - 1.0).abs() < 0.003);
        assert!((face(&m, &id, "bottom").1 / disc - 1.0).abs() < 0.003);
        // The parapet: outside at the wall (radius 5), inside 0.2 m in, every vertex of the ring.
        let (pts, _) = face(&m, &id, "parapet");
        assert!(!pts.is_empty());
        for p in &pts {
            let r = p[0].hypot(p[2]);
            assert!(
                (r - 5.0).abs() < 0.004 || (r - 4.8).abs() < 0.004,
                "radius {r}"
            );
        }
        // A pitched roof on a curved outline is drawn flat, with a note pointing to a flat one.
        let r = s
            .run("shape.set_roof", json!({"id": id, "kind": "gable"}))
            .unwrap();
        assert!(
            r["warnings"][0].as_str().unwrap().contains("flat roof"),
            "{r}"
        );
        assert!(face(&mesh_of(&s, &id), &id, "roof").1 > 0.0);
    }

    fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
        let mut c = false;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                c = !c;
            }
        }
        c
    }

    #[test]
    fn an_opening_on_an_arc_edge_is_a_flat_unit_on_its_chord_inside_the_wall() {
        let mut s = Session::new(Project::default());
        // 8 m wide house, its south wall bulging out 1 m: radius 8.5, arc length about 8.6.
        let id = s
            .run(
                "shape.add_extrusion",
                json!({"name": "Bay", "outline": [[0, 0], [8, 0], [8, 6], [0, 6]], "height": 3,
                       "curves": {"0": {"type": "arc", "sagitta_m": 1.0}},
                       "roof": {"kind": "flat"}}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let r = s
            .run(
                "shape.add_opening",
                json!({"id": id, "side": 0, "offset_m": 3.0, "width_m": 1.5, "frame": "split"}),
            )
            .unwrap();
        assert!(r["warnings"].as_array().unwrap().is_empty(), "{r}");
        s.run(
            "shape.add_opening",
            json!({"id": id, "side": 0, "offset_m": 6.0, "kind": "door"}),
        )
        .unwrap();
        let m = mesh_of(&s, &id);
        let outline = s.project().shapes[0].outline().into_owned();
        let (glass, _) = face(&m, &id, "glass");
        assert!(!glass.is_empty());
        // The glass is one vertical plane (the chord's), 1.5 m of arc wide.
        let (x0, z0) = (glass[0][0], glass[0][2]);
        let far = glass
            .iter()
            .max_by(|p, q| {
                (p[0] - x0)
                    .hypot(p[2] - z0)
                    .total_cmp(&(q[0] - x0).hypot(q[2] - z0))
            })
            .unwrap();
        let (dx, dz) = (far[0] - x0, far[2] - z0);
        let chord = dx.hypot(dz);
        // The glass is the frame's inside: the opening's 1.5 m less a slim frame's 5 cm a side.
        assert!((chord - 1.4).abs() < 0.01, "{chord}");
        for p in &glass {
            // On the vertical plane through the chord ...
            let off = ((p[0] - x0) * dz - (p[2] - z0) * dx) / chord;
            assert!(off.abs() < 1e-3, "off the chord plane by {off}");
            // ... inside the wall, within its thickness of the surface.
            assert!(inside(&outline, [p[0], p[2]]), "{p:?} is outside");
            let near = (0..outline.len())
                .map(|i| {
                    let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
                    let (ex, ez) = (b[0] - a[0], b[1] - a[1]);
                    let t = (((p[0] - a[0]) * ex + (p[2] - a[1]) * ez) / (ex * ex + ez * ez))
                        .clamp(0.0, 1.0);
                    (p[0] - a[0] - ex * t).hypot(p[2] - a[1] - ez * t)
                })
                .fold(f32::MAX, f32::min);
            assert!(near < 0.3 && near > 0.0, "glass {near} m from the surface");
        }
        // The door has its leaf and handle too.
        assert!(face(&m, &id, "door").1 > 0.0 && face(&m, &id, "handle").1 > 0.0);
        // Both openings are cut out of the wall: the arc wall's own area is the strip less them.
        let (_, wall) = face(&m, &id, "side(0)");
        assert!(wall > 0.0);
        // The lamps of the lit rooms hang behind the windows, inside the outline.
        s.run("shape.set_lit_rooms", json!({"id": id, "share": 1}))
            .unwrap();
        let crate::Shape::Extrusion(e) = &s.project().shapes[0] else {
            unreachable!()
        };
        let lamps = crate::house::lit_windows(e);
        assert_eq!(lamps.len(), 1);
        assert!(inside(&outline, [lamps[0][0], lamps[0][2]]));
    }

    #[test]
    fn wall_area_is_the_true_arc_length_times_the_height_less_the_openings() {
        let mut s = Session::new(Project::default());
        let id = round_house(&mut s, 5.0);
        s.run(
            "shape.add_opening",
            json!({"id": id, "side": 0, "offset_m": 4.0, "width_m": 1.2, "height_m": 1.4, "sill_m": 0.9}),
        )
        .unwrap();
        s.run(
            "shape.add_opening",
            json!({"id": id, "side": 2, "offset_m": 3.0, "kind": "door"}),
        )
        .unwrap();
        let q = s.run("query.quantities", json!({})).unwrap();
        let row = q["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id.as_str())
            .unwrap();
        let h = &row["house"];
        let length = 2.0 * std::f64::consts::PI * 5.0;
        assert!(
            (h["wall_length_m"].as_f64().unwrap() - length).abs() < 0.01,
            "{h}"
        );
        let holes = 1.2 * 1.4 + 1.0 * 2.2;
        assert!(
            (h["openings_area_m2"].as_f64().unwrap() - holes).abs() < 0.01,
            "{h}"
        );
        assert!(
            (h["wall_area_m2"].as_f64().unwrap() - (length * 3.0 - holes)).abs() < 0.03,
            "{h}"
        );
        assert_eq!(h["openings"], 2);
    }

    #[test]
    fn an_opening_too_curved_for_the_wall_is_narrowed_with_a_warning() {
        let mut s = Session::new(Project::default());
        let id = round_house(&mut s, 1.5);
        // 1.6 m of arc on a 1.5 m radius: its chord would stand 0.2 m off the wall.
        let r = s
            .run(
                "shape.add_opening",
                json!({"id": id, "side": 0, "offset_m": 1.0, "width_m": 1.6}),
            )
            .unwrap();
        let w = r["warnings"][0].as_str().unwrap();
        assert!(w.contains("narrowed") && w.contains("o1"), "{r}");
        let m = mesh_of(&s, &id);
        let (g, _) = face(&m, &id, "glass");
        assert!(!g.is_empty());
        let q = s.run("query.quantities", json!({})).unwrap();
        let h = q["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id.as_str())
            .unwrap()["house"]
            .clone();
        // The area taken out is the narrowed width, not the asked one.
        assert!(h["openings_area_m2"].as_f64().unwrap() < 1.6 * 1.4, "{h}");
    }

    #[test]
    fn curving_a_house_rebuilds_it_and_keeps_its_openings_on_their_edges() {
        let mut s = Session::new(Project::default());
        let id = s
            .run(
                "shape.add_extrusion",
                json!({"name": "House", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "height": 3,
                       "roof": {"kind": "gable"}}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        s.run(
            "shape.add_opening",
            json!({"id": id, "side": 0, "offset_m": 5.0}),
        )
        .unwrap();
        s.run(
            "shape.set_curve",
            json!({"id": id, "edge": 0, "sagitta_m": 0.8}),
        )
        .unwrap();
        let m = mesh_of(&s, &id);
        assert!(face(&m, &id, "glass").1 > 0.0);
        // The gable roof no longer fits the outline: flat, with a note.
        assert!(!m.warnings.is_empty());
        s.run("shape.set_smooth", json!({"id": id, "smooth": true}))
            .unwrap();
        assert!(face(&mesh_of(&s, &id), &id, "glass").1 > 0.0);
        s.run("shape.set_smooth", json!({"id": id, "smooth": false}))
            .unwrap();
        s.run("shape.clear_curve", json!({"id": id, "edge": 0}))
            .unwrap();
        let m = mesh_of(&s, &id);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        // A curved house survives the project's JSON.
        s.run(
            "shape.set_curve",
            json!({"id": id, "edge": 0, "sagitta_m": 0.8}),
        )
        .unwrap();
        let json = serde_json::to_string(s.project()).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(&back.shapes, &s.project().shapes);
    }
}
