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
            "Gives a block a roof, or changes it: flat (with a parapet), gable, hip or lean_to. Pitched roofs need a rectangular or L-shaped outline; others are drawn flat with a note. The roof face is `<id>:roof`, the wall above the eaves `<id>:gable`.",
            vec![
                id(),
                param("kind", T::String, "flat, gable, hip or lean_to.").default(json!("gable")),
                param("pitch_deg", T::Number, "Slope of a pitched roof, 5..70.").unit("deg").default(json!(35)),
                param("overhang_m", T::Number, "How far the roof reaches past the walls, 0..2.").unit("m").default(json!(0.4)),
                param("parapet_m", T::Number, "Height of a flat roof's parapet, 0 for none.").unit("m").default(json!(0.4)),
            ],
            "null",
            json!({"id": "S2", "kind": "gable", "pitch_deg": 40}),
        ),
        set_roof,
    );
    s.register_with_spec(
        spec(
            "shape.add_opening",
            "Add Window or Door",
            "Puts a window or door in one wall of a block (its outline edge `side`, as in `<id>:side(n)`). Glass is `<id>:glass`, frames `<id>:frame`, doors `<id>:door`, the dark room behind a window `<id>:interior`.",
            vec![
                id(),
                param("side", T::Integer, "The wall: index of the outline edge.").required(),
                param("offset_m", T::Number, "From the wall's first corner to the opening's centre.").unit("m").required(),
                param("kind", T::String, "window or door.").default(json!("window")),
                param("width_m", T::Number, "Width, above zero.").unit("m").default(json!(1.2)),
                param("height_m", T::Number, "Height, above zero.").unit("m").default(json!(1.4)),
                param("sill_m", T::Number, "Bottom above the block's base (a door's is 0).").unit("m").default(json!(0.9)),
                param("frame", T::String, "slim, wide, split (two panes) or none.").default(json!("slim")),
            ],
            "{opening} (o1, o2...)",
            json!({"id": "S2", "side": 0, "offset_m": 3.0, "kind": "window"}),
        ),
        add_opening,
    );
    s.register_with_spec(
        spec(
            "shape.update_opening",
            "Update Window or Door",
            "Changes fields of a window or door.",
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
            "null",
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
}

/// The materials a house's new faces start with, where nothing is assigned to them yet and the
/// project has the material: roof dark tiles' stand-in, glass, timber frames and doors, a dark
/// interior, concrete plinth.
pub(crate) fn default_materials(project: &mut Project, id: &str) {
    for (face, material) in [
        ("roof", "roof"),
        ("glass", "glass"),
        ("frame", "timber"),
        ("door", "timber"),
        ("interior", "interior"),
        ("plinth", "concrete"),
        ("parapet", "concrete"),
    ] {
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
    let sides = e.outline.len();
    let r = f(e.house.get_or_insert_with(House::default), sides)?;
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    default_materials(project, id);
    Ok(r)
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
            "roof", "gable", "glass", "frame", "door", "interior", "plinth",
        ] {
            assert!(tris(f) > 0, "{f}");
        }
        assert_eq!(tris("parapet"), 0);
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
}
