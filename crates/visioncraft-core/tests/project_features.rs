//! Layers, shapes, materials, planting, lights, viewpoints and style through `Session`.

use serde_json::{Value, json};
use visioncraft_core::material::Target;
use visioncraft_core::{
    CommandError, Project, Session, dangling_assignments, import_3dcraft, resolve_materials,
    shape_mesh,
};

const PLATE: &[u8] = include_bytes!("data/plate.3dcraft");

fn id(r: Value) -> String {
    r["id"].as_str().unwrap().to_string()
}

fn square() -> Value {
    json!([[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]])
}

fn face(owner: &str, face: &str) -> Value {
    json!({"type": "face", "owner": owner, "face": face})
}

fn assigned(s: &Session, owner: &str) -> Vec<(String, String)> {
    let mut v: Vec<_> = s
        .project()
        .assignments
        .iter()
        .filter(|a| a.target.owner() == owner)
        .map(|a| match &a.target {
            Target::Face { face, .. } => (face.clone(), a.material.clone()),
            _ => unreachable!(),
        })
        .collect();
    v.sort();
    v
}

#[test]
fn version_one_file_without_new_fields_loads() {
    let p = Project::from_json_bytes(
        br#"{"format_version": 1, "models": [{"id": "m1", "name": "H", "kind": "obj", "source": "h.obj"}]}"#,
    )
    .unwrap();
    assert_eq!(p.layers.len(), 4);
    assert_eq!(p.layers[0].name, "Model");
    assert_eq!(p.models[0].layer, "model");
    assert_eq!(p.materials.len(), 12);
    assert!(p.shapes.is_empty() && p.generators.is_empty() && !p.style.clay);
}

#[test]
fn layers_hide_and_refuse_removal_while_used() {
    let mut s = Session::new(Project::default());
    let l = id(s.run("layer.add", json!({"name": "Terrace"})).unwrap());
    s.run("layer.rename", json!({"id": l, "name": "Patio"}))
        .unwrap();
    s.run("layer.set_visible", json!({"id": l, "visible": false}))
        .unwrap();
    assert!(!s.project().layer_visible(&l));
    let shape = id(s
        .run(
            "shape.add_area",
            json!({"name": "Paving", "outline": square(), "layer": l}),
        )
        .unwrap());
    assert!(s.run("layer.remove", json!({"id": l})).is_err());
    s.run("layer.assign", json!({"item": shape, "layer": "shapes"}))
        .unwrap();
    s.run("layer.remove", json!({"id": l})).unwrap();
    assert!(
        s.run(
            "shape.add_area",
            json!({"name": "x", "outline": square(), "layer": l})
        )
        .is_err()
    );
}

#[test]
fn vertex_insert_and_remove_keep_materials_on_their_edge() {
    let mut s = Session::new(Project::default());
    let sh = id(s
        .run(
            "shape.add_extrusion",
            json!({"name": "Block", "outline": square(), "height": 3.0}),
        )
        .unwrap());
    assert_eq!(sh, "S1");
    for (n, m) in [(0, "brick"), (1, "timber"), (3, "glass")] {
        s.run(
            "material.assign",
            json!({"target": face(&sh, &format!("S1:side({n})")), "material": m}),
        )
        .unwrap();
    }
    // Split edge 1: both halves keep timber, side 3 becomes side 4.
    s.run(
        "shape.insert_vertex",
        json!({"id": sh, "edge": 1, "position": [4.0, 2.0]}),
    )
    .unwrap();
    let side = |n: u32, m: &str| (format!("S1:side({n})"), m.to_string());
    assert_eq!(
        assigned(&s, &sh),
        [
            side(0, "brick"),
            side(1, "timber"),
            side(2, "timber"),
            side(4, "glass")
        ]
    );
    // Remove that vertex again: the merged edge keeps edge 1's material, edge 2's goes.
    s.run("shape.remove_vertex", json!({"id": sh, "index": 2}))
        .unwrap();
    assert_eq!(
        assigned(&s, &sh),
        [side(0, "brick"), side(1, "timber"), side(3, "glass")]
    );
    // Removing vertex 0 drops edge 0 (brick) and shifts the rest down.
    s.run("shape.remove_vertex", json!({"id": sh, "index": 0}))
        .unwrap();
    assert_eq!(assigned(&s, &sh), [side(0, "timber"), side(2, "glass")]);
    // Refused when the outline would no longer be a polygon; nothing changes.
    assert!(
        s.run("shape.remove_vertex", json!({"id": sh, "index": 0}))
            .is_err()
    );
    assert_eq!(assigned(&s, &sh).len(), 2);
    s.run(
        "shape.move_vertex",
        json!({"id": sh, "index": 0, "position": [9.0, 9.0]}),
    )
    .unwrap();
    assert_eq!(assigned(&s, &sh).len(), 2);
}

#[test]
fn remap_holds_for_a_reversed_winding_and_an_open_wall() {
    let mut s = Session::new(Project::default());
    let rev = json!([[0.0, 4.0], [4.0, 4.0], [4.0, 0.0], [0.0, 0.0]]);
    let a = id(s
        .run(
            "shape.add_extrusion",
            json!({"name": "B", "outline": rev, "height": 1.0}),
        )
        .unwrap());
    s.run(
        "material.assign",
        json!({"target": face(&a, "S1:side(2)"), "material": "metal"}),
    )
    .unwrap();
    s.run(
        "shape.insert_vertex",
        json!({"id": a, "edge": 0, "position": [2.0, 4.0]}),
    )
    .unwrap();
    assert_eq!(
        assigned(&s, &a),
        [("S1:side(3)".to_string(), "metal".to_string())]
    );
    let w = id(s.run("shape.add_extrusion", json!({"name": "Fence", "outline": [[0.0, 0.0], [3.0, 0.0], [6.0, 0.0]], "closed": false, "height": 1.8})).unwrap());
    s.run(
        "material.assign",
        json!({"target": face(&w, &format!("{w}:side(1)")), "material": "timber"}),
    )
    .unwrap();
    s.run("shape.remove_vertex", json!({"id": w, "index": 2}))
        .unwrap();
    assert!(assigned(&s, &w).is_empty());
    assert!(
        s.run(
            "shape.insert_vertex",
            json!({"id": w, "edge": 1, "position": [1.0, 1.0]})
        )
        .is_err()
    );
}

#[test]
fn bad_shapes_are_refused() {
    let mut s = Session::new(Project::default());
    let bow = json!([[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0]]);
    assert!(
        s.run("shape.add_area", json!({"name": "x", "outline": bow}))
            .is_err()
    );
    assert!(
        s.run(
            "shape.add_area",
            json!({"name": "x", "outline": [[0.0, 0.0], [1.0, 0.0]]})
        )
        .is_err()
    );
    let sh = id(s
        .run("shape.add_area", json!({"name": "x", "outline": square()}))
        .unwrap());
    assert!(
        s.run("shape.set_height", json!({"id": sh, "height": 2.0}))
            .is_err()
    );
    s.run("shape.set_height", json!({"id": sh, "elevation": 0.3}))
        .unwrap();
    assert!(!s.project().shapes.is_empty() && !s.commands().is_empty());
}

#[test]
fn material_commands_and_resolution_order_on_the_plate() {
    let model = import_3dcraft(PLATE).unwrap();
    let mut s = Session::new(Project::default());
    let m = id(s
        .run(
            "model.add",
            json!({"name": "Plate", "kind": "3dcraft", "source": "plate.3dcraft"}),
        )
        .unwrap());
    let custom = id(s
        .run(
            "material.add",
            json!({"name": "Red", "base_color": [0.8, 0.0, 0.0, 1.0]}),
        )
        .unwrap());
    let idx = |s: &Session, mat: &str| s.project().materials.iter().position(|x| x.id == mat);
    let target = |t: Value, mat: &str| json!({"target": t, "material": mat});
    s.run(
        "material.assign",
        target(json!({"type": "owner", "owner": m}), "concrete"),
    )
    .unwrap();
    let r = resolve_materials(s.project(), &m, &model);
    assert_eq!(r.len(), model.faces.len());
    assert!(r.iter().all(|x| *x == idx(&s, "concrete")));
    s.run(
        "material.assign",
        target(json!({"type": "body", "owner": m, "body": "F2"}), "timber"),
    )
    .unwrap();
    s.run("material.assign", target(face(&m, "F2:side(e5)"), &custom))
        .unwrap();
    let r = resolve_materials(s.project(), &m, &model);
    for (f, got) in model.faces.iter().zip(&r) {
        let want = if f.name == "F2:side(e5)" {
            idx(&s, &custom)
        } else {
            idx(&s, "timber")
        };
        assert_eq!(*got, want, "{}", f.name);
    }
    // A face that does not exist is kept and reported.
    s.run("material.assign", target(face(&m, "F2:side(e99)"), "glass"))
        .unwrap();
    s.run(
        "material.assign",
        target(json!({"type": "body", "owner": m, "body": "F9"}), "glass"),
    )
    .unwrap();
    assert_eq!(dangling_assignments(s.project(), &m, &model).len(), 2);
    assert_eq!(s.project().assignments.len(), 5);
    // Assigned materials cannot be removed; unassigning frees them.
    assert!(s.run("material.remove", json!({"id": custom})).is_err());
    s.run(
        "material.unassign",
        json!({"target": face(&m, "F2:side(e5)")}),
    )
    .unwrap();
    s.run("material.remove", json!({"id": custom})).unwrap();
    assert!(
        s.run("material.assign", target(face("nope", "x"), "glass"))
            .is_err()
    );
    s.run("material.update", json!({"id": "glass", "roughness": 0.2}))
        .unwrap();
    assert!(
        s.run("material.update", json!({"id": "glass", "roughnes": 0.2}))
            .is_err()
    );
    assert!(
        s.run("material.update", json!({"id": "glass", "roughness": 2.0}))
            .is_err()
    );
    // Removing the model takes its assignments with it.
    s.run("model.remove", json!({"id": m})).unwrap();
    assert!(s.project().assignments.is_empty());
}

fn palette() -> Value {
    json!([{"asset": "oak", "weight": 2.0}, {"asset": "fern", "weight": 1.0}])
}

#[test]
fn one_of_everything_round_trips_and_undoes() {
    let mut s = Session::new(Project::default());
    let start = s.project().clone();
    let m = id(s
        .run(
            "model.add",
            json!({"name": "House", "kind": "obj", "source": "h.obj"}),
        )
        .unwrap());
    let lawn = id(s
        .run(
            "shape.add_area",
            json!({"name": "Lawn", "outline": square()}),
        )
        .unwrap());
    s.run("shape.add_extrusion", json!({"name": "Hedge", "outline": [[0.0, 0.0], [4.0, 0.0]], "closed": false, "height": 1.0})).unwrap();
    s.run(
        "material.assign",
        json!({"target": {"type": "owner", "owner": lawn}, "material": "lawn"}),
    )
    .unwrap();
    s.run(
        "material.assign",
        json!({"target": {"type": "owner", "owner": m}, "material": "plaster"}),
    )
    .unwrap();
    s.run(
        "planting.add_single",
        json!({"name": "Oak", "asset": "oak", "position": [1.0, 0.0, 1.0]}),
    )
    .unwrap();
    let spline = id(s.run("planting.add_spline", json!({"name": "Hedge", "asset_palette": palette(), "points": [[0.0, 0.0], [5.0, 0.0]], "spacing": {"every": 0.5}})).unwrap());
    s.run("planting.add_area", json!({"name": "Beds", "asset_palette": palette(), "outline": square(), "density_per_m2": 2.0})).unwrap();
    let brush = id(s
        .run(
            "planting.add_brush",
            json!({"name": "Brush", "asset_palette": palette(), "density_per_m2": 3.0}),
        )
        .unwrap());
    s.run(
        "planting.brush_stroke",
        json!({"id": brush, "points": [[1.0, 1.0], [3.0, 1.0]], "radius_m": 1.0}),
    )
    .unwrap();
    s.run("planting.add_surface_fill", json!({"name": "Fill", "asset_palette": palette(), "owner": lawn, "faces": [format!("{lawn}:top")], "density_per_m2": 4.0})).unwrap();
    s.run("planting.add_cluster", json!({"name": "Clump", "asset_palette": palette(), "center": [2.0, 2.0], "radius_m": 1.0, "count": 12})).unwrap();
    s.run(
        "planting.update",
        json!({"id": spline, "spacing": {"count": 6}, "closed": true}),
    )
    .unwrap();
    assert!(
        s.run("planting.update", json!({"id": spline, "colour": 1}))
            .is_err()
    );
    assert!(
        s.run("planting.update", json!({"id": spline, "scale_min": 3.0}))
            .is_err()
    );
    assert!(
        s.run(
            "planting.brush_stroke",
            json!({"id": spline, "points": [[0.0, 0.0]], "radius_m": 1.0})
        )
        .is_err()
    );
    let light = id(s.run("light.add", json!({"name": "Lamp", "position": [1.0, 2.0, 1.0], "kind": {"type": "spot", "direction": [0.0, -1.0, 0.0], "inner_cone_deg": 20.0, "outer_cone_deg": 40.0}})).unwrap());
    s.run(
        "light.update",
        json!({"id": light, "lumens": 1200.0, "enabled": false}),
    )
    .unwrap();
    assert!(
        s.run("light.update", json!({"id": light, "range_m": 0.0}))
            .is_err()
    );
    let v1 = id(s.run("viewpoint.add", json!({"name": "Front", "position": [0.0, 1.6, 8.0], "target": [0.0, 1.6, 0.0], "time": {"year": 2026, "month": 6, "day": 21, "hour": 9.0}})).unwrap());
    let v2 = id(s
        .run(
            "viewpoint.add",
            json!({"name": "Back", "position": [0.0, 1.6, -8.0], "target": [0.0, 1.6, 0.0]}),
        )
        .unwrap());
    s.run("viewpoint.reorder", json!({"id": v2, "index": 0}))
        .unwrap();
    s.run(
        "viewpoint.update",
        json!({"id": v1, "fov_y_deg": 75.0, "time": null}),
    )
    .unwrap();
    assert_eq!(s.project().viewpoints[0].id, v2);
    assert!(s.project().viewpoints[1].time.is_none());
    s.run("style.set", json!({"clay": true})).unwrap();
    s.run("layer.add", json!({"name": "Extra"})).unwrap();

    let end = s.project().clone();
    let reloaded = Project::from_json_bytes(&end.to_json_bytes()).unwrap();
    assert_eq!(reloaded, end);
    assert_eq!(end.generators.len(), 6);
    assert!(end.shapes.iter().all(|sh| shape_mesh(sh).is_ok()));

    // Remove one of each family, then undo everything and redo it.
    s.run("light.remove", json!({"id": light})).unwrap();
    s.run("viewpoint.remove", json!({"id": v1})).unwrap();
    s.run("planting.remove", json!({"id": brush})).unwrap();
    s.run("shape.rename", json!({"id": lawn, "name": "Grass"}))
        .unwrap();
    s.run("shape.remove", json!({"id": lawn})).unwrap();
    assert!(
        s.project()
            .assignments
            .iter()
            .all(|a| a.target.owner() != lawn)
    );
    let after = s.project().clone();
    while s.undo() {}
    assert_eq!(*s.project(), start);
    while s.redo() {}
    assert_eq!(*s.project(), after);
    assert!(matches!(
        s.run("light.remove", json!({"id": "nope"})),
        Err(CommandError::Failed(_))
    ));
}
