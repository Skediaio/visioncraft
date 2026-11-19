//! Editing everything that is selected at once (`ui.edit_selection`, the Delete key with several
//! things selected): delete, move by an offset and assign a material. Each runs the ordinary
//! commands (`shape.remove`, `shape.set_outline`, `planting.update`, ...) for every selected
//! thing inside one `Session::transaction`, so the whole edit is one undo step, and a command
//! that fails undoes all of it. What an edit does not make sense for (a material for a planting,
//! a move of a brush) is left alone and reported under `skipped`.

use crate::selection::{Key, Kind};
use serde_json::{Value, json};
use visioncraft_core::planting::Generator;
use visioncraft_core::{CommandError, Project, Session};

/// What to do with the selection.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Delete,
    /// By `[x, y, z]` metres: a shape, a planting and an underlay only move on the ground (x, z).
    Move([f32; 3]),
    /// A material id, for shapes and models.
    Material(String),
}

impl Edit {
    /// The edit `ui.edit_selection`'s params ask for.
    pub fn parse(params: &Value) -> Result<Edit, String> {
        let action = params
            .get("action")
            .and_then(Value::as_str)
            .ok_or("`action` is `delete`, `move` or `material`")?;
        match action {
            "delete" => Ok(Edit::Delete),
            "move" => {
                let offset = params.get("offset").and_then(Value::as_array).ok_or(
                    "`move` needs `offset`: [x, z] or [x, y, z] metres (+X east, -Z north, +Y up)",
                )?;
                let n: Option<Vec<f32>> = offset
                    .iter()
                    .map(|v| v.as_f64().filter(|v| v.is_finite()).map(|v| v as f32))
                    .collect();
                match n.as_deref() {
                    Some([x, z]) => Ok(Edit::Move([*x, 0.0, *z])),
                    Some([x, y, z]) => Ok(Edit::Move([*x, *y, *z])),
                    _ => Err("`offset` is [x, z] or [x, y, z], in metres".into()),
                }
            }
            "material" => Ok(Edit::Material(
                params
                    .get("material")
                    .and_then(Value::as_str)
                    .ok_or("`material` needs `material`: a material id")?
                    .to_string(),
            )),
            other => Err(format!(
                "unknown action `{other}`: `delete`, `move` or `material`"
            )),
        }
    }
}

/// The plants of a planting are one thing each; a whole planting selected covers its plants.
fn covered(k: &Key, items: &[Key]) -> bool {
    k.kind == Kind::Placement
        && items
            .iter()
            .any(|i| i.kind == Kind::Planting && i.id == k.id)
}

/// `g` moved by `d`, if it can be (a brush or a surface fill has no place of its own).
fn moved(g: &Generator, d: [f32; 3]) -> Option<Generator> {
    let mut g = g.clone();
    let shift = |p: &mut [f32; 2]| *p = [p[0] + d[0], p[1] + d[2]];
    match &mut g {
        Generator::Single(s) => {
            s.position = [
                s.position[0] + d[0],
                s.position[1] + d[1],
                s.position[2] + d[2],
            ];
        }
        Generator::SplineMass(s) => s.points.iter_mut().for_each(shift),
        Generator::AreaScatter(a) => a.outline.iter_mut().for_each(shift),
        Generator::Cluster(c) => shift(&mut c.center),
        Generator::MixedBrush(_) | Generator::SurfaceFill(_) => return None,
    }
    Some(g)
}

fn sum3(a: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    [a[0] + d[0], a[1] + d[1], a[2] + d[2]]
}

fn skip(k: &Key, why: &str) -> Value {
    json!({"kind": k.kind.name(), "id": k.id, "reason": why})
}

/// Runs `edit` on `items` inside `s`'s open transaction; how many things changed, and what was
/// left alone.
fn run_edit(
    s: &mut Session,
    items: &[Key],
    edit: &Edit,
) -> Result<(usize, Vec<Value>), CommandError> {
    let mut changed = 0;
    let mut skipped = Vec::new();
    // Plants of a planting first: a plant is numbered within its planting, and taking one out
    // leaves the numbers of the others as they are.
    let mut order: Vec<&Key> = items.iter().filter(|k| !covered(k, items)).collect();
    order.sort_by_key(|k| k.kind != Kind::Placement);
    for k in order {
        let p: &Project = s.project();
        let done = match (edit, k.kind) {
            (Edit::Delete, Kind::Shape) => Some(("shape.remove", json!({"id": k.id}))),
            (Edit::Delete, Kind::Planting) => Some(("planting.remove", json!({"id": k.id}))),
            (Edit::Delete, Kind::Placement) => Some((
                "planting.remove_placement",
                json!({"planting": k.id, "index": k.index.unwrap_or(0)}),
            )),
            (Edit::Delete, Kind::Model) => Some(("model.remove", json!({"id": k.id}))),
            (Edit::Delete, Kind::Lamp) => Some(("light.remove", json!({"id": k.id}))),
            (Edit::Delete, Kind::Underlay) => Some(("underlay.remove", json!({"id": k.id}))),
            (Edit::Move(d), Kind::Shape) => p.shapes.iter().find(|x| x.id() == k.id).map(|x| {
                let outline: Vec<[f32; 2]> = x
                    .points()
                    .iter()
                    .map(|q| [q[0] + d[0], q[1] + d[2]])
                    .collect();
                ("shape.set_outline", json!({"id": k.id, "outline": outline}))
            }),
            (Edit::Move(d), Kind::Planting) => {
                let new = p
                    .generators
                    .iter()
                    .find(|g| g.id() == k.id)
                    .and_then(|g| moved(g, *d).map(|n| (g, n)));
                match new {
                    Some((old, new)) => crate::handles::diff_params(old, &new)
                        .map(|params| ("planting.update", params)),
                    None => {
                        skipped.push(skip(k, "this kind of planting has no place of its own"));
                        continue;
                    }
                }
            }
            (Edit::Move(d), Kind::Placement) => Some((
                "planting.replace_placement",
                json!({"planting": k.id, "index": k.index.unwrap_or(0), "offset": [d[0], d[2]]}),
            )),
            (Edit::Move(d), Kind::Model) => p.models.iter().find(|m| m.id == k.id).map(|m| {
                (
                    "model.set_transform",
                    json!({"id": k.id, "translation": sum3(m.transform.translation, *d)}),
                )
            }),
            (Edit::Move(d), Kind::Lamp) => p.lights.iter().find(|l| l.id == k.id).map(|l| {
                (
                    "light.update",
                    json!({"id": k.id, "position": sum3(l.position, *d)}),
                )
            }),
            (Edit::Move(d), Kind::Underlay) => p.underlays.iter().find(|u| u.id == k.id).map(|u| {
                let o = u.placement.origin;
                (
                    "underlay.set",
                    json!({"id": k.id, "origin": [o[0] + d[0], o[1] + d[2]]}),
                )
            }),
            (Edit::Material(m), Kind::Shape | Kind::Model) => Some((
                "material.assign",
                json!({"target": {"type": "owner", "owner": k.id}, "material": m}),
            )),
            (Edit::Material(_), _) => {
                skipped.push(skip(k, "only shapes and models take a material"));
                continue;
            }
        };
        match done {
            Some((command, params)) => {
                s.run(command, params)?;
                changed += 1;
            }
            None => skipped.push(skip(k, "not in the project")),
        }
    }
    Ok((changed, skipped))
}

/// `edit` on every one of `items`, as one undo step (an error undoes all of it). The lamp
/// reconcile of [`crate::studio::Studio::with_lamps`] is part of the same step.
pub fn apply(
    studio: &mut crate::studio::Studio,
    items: &[Key],
    edit: &Edit,
) -> Result<Value, String> {
    if items.is_empty() {
        return Err("nothing is selected: `ui.select` first".into());
    }
    let (changed, skipped) = studio
        .with_lamps(|s| run_edit(s, items, edit))
        .map_err(|e| e.to_string())?;
    Ok(json!({"changed": changed, "skipped": skipped}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new(Project::default());
        for (c, p) in [
            (
                "shape.add_area",
                json!({"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 6], [0, 6]]}),
            ),
            (
                "shape.add_area",
                json!({"name": "Bed", "outline": [[20, 0], [24, 0], [24, 3], [20, 3]]}),
            ),
            (
                "planting.add_single",
                json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [4, 0, 3]}),
            ),
            (
                "planting.add_cluster",
                json!({"name": "Grove", "center": [8, 8], "radius_m": 2, "count": 4,
                    "asset_palette": [{"asset": "flora:betula-pendula", "weight": 1}]}),
            ),
            ("light.add", json!({"name": "Lamp", "position": [1, 2, 3]})),
        ] {
            s.run(c, p).unwrap();
        }
        s
    }

    fn keys() -> Vec<Key> {
        let mut plant = Key::new(Kind::Placement, "g2");
        plant.index = Some(1);
        vec![
            Key::new(Kind::Shape, "S1"),
            Key::new(Kind::Shape, "S2"),
            Key::new(Kind::Planting, "g1"),
            plant,
            Key::new(Kind::Lamp, "L1"),
        ]
    }

    fn lamp_id(s: &Session) -> String {
        s.project().lights[0].id.clone()
    }

    #[test]
    fn a_move_shifts_everything_selected_in_one_undo_step() {
        let mut s = session();
        let mut items = keys();
        items[4] = Key::new(Kind::Lamp, lamp_id(&s));
        let before = s.project().clone();
        let (changed, skipped) = s
            .transaction(|s| run_edit(s, &items, &Edit::Move([2.0, 1.0, -3.0])))
            .unwrap();
        assert_eq!((changed, skipped.len()), (5, 0), "{skipped:?}");
        let p = s.project();
        assert_eq!(p.shapes[0].points()[0], [2.0, -3.0]);
        assert_eq!(p.shapes[1].points()[0], [22.0, -3.0]);
        let Generator::Single(oak) = &p.generators[0] else {
            panic!("a single plant")
        };
        assert_eq!(oak.position, [6.0, 1.0, 0.0]);
        assert_eq!(p.lights[0].position, [3.0, 3.0, 0.0]);
        assert!(p.generators[1].overrides().is_some_and(|o| !o.is_empty()));
        // One undo puts everything back.
        assert!(s.undo());
        assert_eq!(*s.project(), before);
    }

    #[test]
    fn a_delete_takes_everything_out_in_one_step_and_a_covered_plant_goes_with_its_planting() {
        let mut s = session();
        let mut items = keys();
        items[4] = Key::new(Kind::Lamp, lamp_id(&s));
        // The grove selected whole and one of its plants: the plant is covered.
        items.push(Key::new(Kind::Planting, "g2"));
        let before = s.project().clone();
        let (changed, _) = s
            .transaction(|s| run_edit(s, &items, &Edit::Delete))
            .unwrap();
        assert_eq!(changed, 5);
        let p = s.project();
        assert!(p.shapes.is_empty() && p.generators.is_empty() && p.lights.is_empty());
        assert!(s.undo());
        assert_eq!(*s.project(), before);
    }

    #[test]
    fn a_failing_edit_undoes_all_of_it() {
        let mut s = session();
        let before = s.project().clone();
        // The shape goes first, then the lamp that is not there fails the lot.
        let items = vec![Key::new(Kind::Shape, "S1"), Key::new(Kind::Lamp, "L99")];
        let r = s.transaction(|s| run_edit(s, &items, &Edit::Delete));
        assert!(r.is_err());
        assert_eq!(*s.project(), before);
        // A material that does not exist fails the whole edit too.
        let items = vec![Key::new(Kind::Shape, "S1"), Key::new(Kind::Shape, "S2")];
        let r = s.transaction(|s| run_edit(s, &items, &Edit::Material("nope".into())));
        assert!(r.is_err());
        assert_eq!(*s.project(), before);
    }

    #[test]
    fn a_material_goes_to_shapes_and_the_rest_is_skipped() {
        let mut s = session();
        let id = s.run("material.add", json!({"name": "Grass"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (changed, skipped) = s
            .transaction(|s| run_edit(s, &keys(), &Edit::Material(id)))
            .unwrap();
        assert_eq!(changed, 2);
        assert_eq!(skipped.len(), 3);
        assert_eq!(s.project().assignments.len(), 2);
    }

    #[test]
    fn the_params_are_checked() {
        assert_eq!(
            Edit::parse(&json!({"action": "move", "offset": [1, 2]})),
            Ok(Edit::Move([1.0, 0.0, 2.0]))
        );
        assert_eq!(Edit::parse(&json!({"action": "delete"})), Ok(Edit::Delete));
        assert!(Edit::parse(&json!({"action": "move"})).is_err());
        assert!(Edit::parse(&json!({"action": "move", "offset": [1]})).is_err());
        assert!(Edit::parse(&json!({"action": "material"})).is_err());
        assert!(Edit::parse(&json!({"action": "spin"})).is_err());
    }
}
