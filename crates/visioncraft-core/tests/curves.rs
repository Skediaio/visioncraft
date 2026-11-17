//! Curved outlines through `Session`: arcs, the smooth outline, circles, curved fences and beds,
//! the shapes that refuse curves, re-keying and undo.

use serde_json::{Value, json};
use visioncraft_core::curve::{Curve, tessellate};
use visioncraft_core::measure::polygon_area;
use visioncraft_core::{Project, Session, Shape, shape_mesh};

fn session() -> Session {
    Session::new(Project::default())
}

fn id(r: Value) -> String {
    r["id"].as_str().unwrap().to_string()
}

fn shape<'a>(s: &'a Session, id: &str) -> &'a Shape {
    s.project().shapes.iter().find(|x| x.id() == id).unwrap()
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn square() -> Value {
    json!([[0, 0], [10, 0], [10, 10], [0, 10]])
}

fn perimeter(p: &[[f32; 2]]) -> f32 {
    (0..p.len()).map(|i| dist(p[i], p[(i + 1) % p.len()])).sum()
}

#[test]
fn without_curves_the_outline_is_exactly_the_control_points() {
    let mut s = session();
    let a = id(s
        .run(
            "shape.add_area",
            json!({"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8.5], [1.25, 8]]}),
        )
        .unwrap());
    let sh = shape(&s, &a);
    assert_eq!(&*sh.outline(), sh.points());
    assert!(!sh.is_curved());
    // Nothing of curves in the file either.
    let text = String::from_utf8(s.project().to_json_bytes()).unwrap();
    assert!(
        !text.contains("curves") && !text.contains("smooth"),
        "{text}"
    );
    // A straight shape meshes to the same triangles as before: the top is the polygon itself.
    let m = shape_mesh(sh).unwrap();
    assert_eq!(m.positions.len(), m.triangles.len() * 3);
}

#[test]
fn a_circle_of_radius_5_has_the_area_and_perimeter_of_a_circle() {
    let mut s = session();
    let a = id(s
        .run(
            "shape.add_circle",
            json!({"name": "Round lawn", "center": [11, 20], "radius_m": 5.0, "kind": "area"}),
        )
        .unwrap());
    let sh = shape(&s, &a);
    assert_eq!(sh.points().len(), 4);
    assert_eq!(sh.curves().len(), 4);
    let o = sh.outline();
    let (area, per) = (f64::from(polygon_area(&o)), f64::from(perimeter(&o)));
    let pi = std::f64::consts::PI;
    assert!((area / (25.0 * pi) - 1.0).abs() < 0.001, "{area}");
    assert!((per / (10.0 * pi) - 1.0).abs() < 0.001, "{per}");
    // Every outline point lies on the circle.
    assert!(
        o.iter()
            .all(|p| (dist(*p, [11.0, 20.0]) - 5.0).abs() < 0.005)
    );
    // Quantities see the same.
    let q = s.run("query.quantities", json!({})).unwrap();
    let row = &q["shapes"][0];
    assert!((row["plan_area_m2"].as_f64().unwrap() / (25.0 * pi) - 1.0).abs() < 0.001);
    assert!((row["perimeter_m"].as_f64().unwrap() / (10.0 * pi) - 1.0).abs() < 0.001);
    // And as an extrusion: a round bed block, one side per control edge.
    let e = id(s
        .run(
            "shape.add_circle",
            json!({"center": [0, 0], "radius_m": 1.0, "kind": "extrusion", "height": 0.4}),
        )
        .unwrap());
    let m = shape_mesh(shape(&s, &e)).unwrap();
    let names: Vec<_> = m.faces.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            format!("{e}:top"),
            format!("{e}:bottom"),
            format!("{e}:side(0)"),
            format!("{e}:side(1)"),
            format!("{e}:side(2)"),
            format!("{e}:side(3)"),
        ]
    );
    let sides = m
        .triangle_faces
        .iter()
        .filter(|f| m.faces[**f as usize].name.contains("side"))
        .count();
    assert!(
        sides > 4 * 2 * 8,
        "a curved side is many triangles: {sides}"
    );
}

#[test]
fn set_curve_takes_a_radius_and_a_side_and_undoes() {
    let mut s = session();
    let a = id(s
        .run(
            "shape.add_area",
            json!({"name": "Bed", "outline": square()}),
        )
        .unwrap());
    let before = shape(&s, &a).clone();
    // Edge 0 runs east along +X: left is -Z (north).
    let r = s
        .run(
            "shape.set_curve",
            json!({"id": a, "edge": 0, "radius_m": 8.0, "side": "left"}),
        )
        .unwrap();
    assert!((r["radius_m"].as_f64().unwrap() - 8.0).abs() < 1e-4);
    let sagitta = 8.0 - (64.0f32 - 25.0).sqrt();
    let Curve::Arc { sagitta_m } = shape(&s, &a).curves()[&0];
    assert!((sagitta_m - sagitta).abs() < 1e-4);
    assert!(shape(&s, &a).outline().iter().any(|p| p[1] < -0.5));
    s.run(
        "shape.set_curve",
        json!({"id": a, "edge": 0, "radius_m": 8.0, "side": "right"}),
    )
    .unwrap();
    assert!(shape(&s, &a).curves()[&0].sagitta_m() < 0.0);
    assert!(
        shape(&s, &a)
            .outline()
            .iter()
            .all(|p| p[1] > -1e-6 || p[0] < 1e-6 || p[0] > 10.0 - 1e-6 || p[1] > 9.0)
    );
    // A radius under half the chord is refused, nothing changes.
    let e = s
        .run(
            "shape.set_curve",
            json!({"id": a, "edge": 0, "radius_m": 4.9}),
        )
        .unwrap_err();
    assert!(e.to_string().contains("too small"), "{e}");
    assert!(
        s.run(
            "shape.set_curve",
            json!({"id": a, "edge": 9, "radius_m": 9.0})
        )
        .is_err()
    );
    assert!(
        s.run("shape.set_curve", json!({"id": a, "edge": 0}))
            .is_err()
    );
    // Sagitta, signed: positive left.
    s.run(
        "shape.set_curve",
        json!({"id": a, "edge": 1, "sagitta_m": 1.0}),
    )
    .unwrap();
    // Edge 1 runs south (+Z): left is +X.
    assert!(shape(&s, &a).outline().iter().any(|p| p[0] > 10.5));
    s.run("shape.clear_curve", json!({"id": a, "edge": 1}))
        .unwrap();
    s.run("shape.clear_curve", json!({"id": a, "edge": 1}))
        .unwrap();
    // Undo each step back to the start.
    let mut steps = 0;
    while s.undo() {
        steps += 1;
    }
    assert!(steps >= 4);
    assert!(s.project().shapes.is_empty());
    while s.redo() {}
    assert_eq!(shape(&s, &a).curves().len(), 1);
    let _ = before;
}

#[test]
fn set_smooth_passes_through_the_corners_and_undoes() {
    let mut s = session();
    let a = id(s
        .run(
            "shape.add_area",
            json!({"name": "Path", "outline": [[0, 0], [6, 1], [9, 6], [3, 9]]}),
        )
        .unwrap());
    s.run("shape.set_smooth", json!({"id": a, "smooth": true}))
        .unwrap();
    let sh = shape(&s, &a);
    assert!(sh.is_smooth() && sh.outline().len() > 20);
    let t = sh.tessellate();
    for (i, p) in sh.points().iter().enumerate() {
        assert_eq!(t.pts[t.ctrl[i]], *p);
    }
    // The document shows it.
    let text = String::from_utf8(s.project().to_json_bytes()).unwrap();
    assert!(text.contains("\"smooth\": true") || text.contains("\"smooth\":true"));
    assert!(s.undo());
    assert!(!shape(&s, &a).is_smooth());
    assert!(s.redo());
    assert!(shape(&s, &a).is_smooth());
    // An arc on an edge keeps its arc next to a smooth one.
    s.run(
        "shape.set_curve",
        json!({"id": a, "edge": 0, "radius_m": 20.0}),
    )
    .unwrap();
    let sh = shape(&s, &a);
    let t = sh.tessellate();
    assert!(t.edge_of.iter().filter(|e| **e == 0).count() >= 4);
}

#[test]
fn add_area_and_add_extrusion_take_curves_and_smooth() {
    let mut s = session();
    let a = id(s
        .run(
            "shape.add_area",
            json!({"name": "Lawn", "outline": square(), "curves": {"2": {"type": "arc", "sagitta_m": 2.0}}}),
        )
        .unwrap());
    assert_eq!(shape(&s, &a).curves().len(), 1);
    let e = id(s
        .run(
            "shape.add_extrusion",
            json!({"name": "Hedge", "outline": [[0, 0], [5, 2], [10, 0]], "closed": false, "height": 1.0, "smooth": true}),
        )
        .unwrap());
    assert!(shape(&s, &e).is_smooth());
    // A bad curve refuses the add.
    let bad = s.run(
        "shape.add_area",
        json!({"name": "X", "outline": square(), "curves": {"0": {"type": "arc", "sagitta_m": 6.0}}}),
    );
    assert!(bad.unwrap_err().to_string().contains("semicircle"));
    let bad = s.run(
        "shape.add_area",
        json!({"name": "X", "outline": square(), "curves": {"7": {"type": "arc", "sagitta_m": 1.0}}}),
    );
    assert!(bad.is_err());
}

#[test]
fn inserting_and_removing_a_vertex_rekey_the_arcs() {
    let mut s = session();
    let a = id(s
        .run(
            "shape.add_area",
            json!({"name": "Lawn", "outline": square(), "curves": {
                "0": {"type": "arc", "sagitta_m": 1.0},
                "1": {"type": "arc", "sagitta_m": 1.5},
                "3": {"type": "arc", "sagitta_m": -1.0}}}),
        )
        .unwrap());
    let keys = |s: &Session| shape(s, &a).curves().keys().copied().collect::<Vec<_>>();
    assert_eq!(keys(&s), [0, 1, 3]);
    // Splitting edge 1 drops its arc; edge 3 becomes 4.
    s.run(
        "shape.insert_vertex",
        json!({"id": a, "edge": 1, "position": [10, 5]}),
    )
    .unwrap();
    assert_eq!(keys(&s), [0, 4]);
    assert!(s.undo());
    assert_eq!(keys(&s), [0, 1, 3]);
    // Removing vertex 2 joins edges 1 and 2: both lose their arcs; edge 3 becomes 2.
    s.run("shape.remove_vertex", json!({"id": a, "index": 2}))
        .unwrap();
    assert_eq!(keys(&s), [0, 2]);
    assert!(s.undo());
    assert_eq!(keys(&s), [0, 1, 3]);
    // Moving a vertex so a chord gets short eases the arc to a semicircle instead of failing.
    s.run(
        "shape.move_vertex",
        json!({"id": a, "index": 1, "position": [1, 0]}),
    )
    .unwrap();
    let Curve::Arc { sagitta_m } = shape(&s, &a).curves()[&0];
    assert!((sagitta_m - 0.5).abs() < 1e-5);
}

#[test]
fn a_curved_wall_keeps_its_sides_per_control_edge() {
    let mut s = session();
    let w = id(s
        .run(
            "shape.add_extrusion",
            json!({"name": "Wall", "outline": [[0, 0], [6, 0], [6, 6]], "closed": false, "height": 1.0,
                   "thickness": 0.2, "curves": {"0": {"type": "arc", "sagitta_m": -1.0}}}),
        )
        .unwrap());
    let m = shape_mesh(shape(&s, &w)).unwrap();
    let names: Vec<_> = m.faces.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            format!("{w}:top"),
            format!("{w}:bottom"),
            format!("{w}:side(0)"),
            format!("{w}:side(1)"),
            format!("{w}:start"),
            format!("{w}:end"),
        ]
    );
    let count = |n: &str| {
        let f = m
            .faces
            .iter()
            .position(|f| f.name == format!("{w}:{n}"))
            .unwrap() as u32;
        m.triangle_faces.iter().filter(|x| **x == f).count()
    };
    assert!(count("side(0)") > 30 && count("side(1)") == 4);
    assert_eq!(count("start"), 2);
}

#[test]
fn stairs_and_houses_refuse_curves() {
    let mut s = session();
    let st = id(s
        .run(
            "stairs.add",
            json!({"from": [0, 0], "to": [0, 3], "width_m": 1.2, "height": 0.5}),
        )
        .unwrap());
    let e = s
        .run(
            "shape.set_curve",
            json!({"id": st, "edge": 0, "radius_m": 3.0}),
        )
        .unwrap_err();
    assert!(e.to_string().contains("straight"), "{e}");
    assert!(
        s.run("shape.set_smooth", json!({"id": st, "smooth": true}))
            .is_err()
    );
    assert!(shape(&s, &st).curves().is_empty() && !shape(&s, &st).is_smooth());
    let h = id(s
        .run(
            "shape.add_extrusion",
            json!({"name": "House", "outline": [[2, 2], [8, 2], [8, 6], [2, 6]], "height": 3.0, "roof": {"kind": "gable"}}),
        )
        .unwrap());
    let e = s
        .run(
            "shape.set_curve",
            json!({"id": h, "edge": 0, "radius_m": 9.0}),
        )
        .unwrap_err();
    assert!(e.to_string().contains("house"), "{e}");
    // Sleeper beds and sleeper edges refuse too, the other styles take them.
    let b = id(s
        .run(
            "bed.add",
            json!({"outline": [[0, 0], [3, 0], [3, 3], [0, 3]], "style": "sleepers"}),
        )
        .unwrap());
    assert!(
        s.run(
            "shape.set_curve",
            json!({"id": b, "edge": 0, "radius_m": 3.0})
        )
        .is_err()
    );
    s.run("bed.set", json!({"id": b, "style": "corten"}))
        .unwrap();
    s.run(
        "shape.set_curve",
        json!({"id": b, "edge": 0, "radius_m": 3.0}),
    )
    .unwrap();
}

#[test]
fn a_round_corten_bed_follows_its_curve_with_soil_inside() {
    let mut s = session();
    let b = s
        .run(
            "bed.add",
            json!({"name": "Round bed", "outline": [[5, 0], [0, 5], [-5, 0], [0, -5]],
                   "style": "corten", "curves": {
                     "0": {"type": "arc", "sagitta_m": 1.4645}, "1": {"type": "arc", "sagitta_m": 1.4645},
                     "2": {"type": "arc", "sagitta_m": 1.4645}, "3": {"type": "arc", "sagitta_m": 1.4645}}}),
        )
        .unwrap();
    let soil = b["soil"].as_str().unwrap();
    let Shape::Area(a) = shape(&s, soil) else {
        panic!()
    };
    // The soil is the circle moved in by the wall (1.5 mm thick corten: about r 4.998).
    let area = f64::from(polygon_area(&a.points));
    assert!(
        (area / (std::f64::consts::PI * 25.0) - 1.0).abs() < 0.01,
        "{area}"
    );
    let q = s.run("query.quantities", json!({})).unwrap();
    let bed = q["shapes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == b["id"])
        .unwrap();
    assert!(bed["raised_bed"]["wall_length_m"].as_f64().unwrap() > 31.0);
}

#[test]
fn undo_of_every_curve_command_restores_the_shape() {
    let mut s = session();
    let c = id(s
        .run(
            "shape.add_circle",
            json!({"center": [3, 3], "radius_m": 2.0}),
        )
        .unwrap());
    let base = shape(&s, &c).clone();
    for (cmd, params) in [
        (
            "shape.set_curve",
            json!({"id": c, "edge": 0, "radius_m": 5.0}),
        ),
        ("shape.clear_curve", json!({"id": c, "edge": 1})),
        ("shape.set_smooth", json!({"id": c, "smooth": true})),
    ] {
        let before = shape(&s, &c).clone();
        s.run(cmd, params).unwrap();
        assert_ne!(shape(&s, &c), &before, "{cmd}");
        assert!(s.undo(), "{cmd}");
        assert_eq!(shape(&s, &c), &before, "{cmd}");
    }
    assert_eq!(shape(&s, &c), &base);
}

#[test]
fn the_arc_helpers_agree_with_the_commands() {
    let pts = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    let curves = [(0usize, Curve::Arc { sagitta_m: 2.0 })].into();
    let t = tessellate(&pts, true, &curves, false);
    // Left of the eastward edge is north (-Z).
    assert!(t.pts[1..t.ctrl[1]].iter().all(|p| p[1] < 0.0));
    assert!(
        t.pts[1..t.ctrl[1]]
            .iter()
            .any(|p| (p[1] + 2.0).abs() < 0.01)
    );
}

#[test]
fn curves_and_smooth_survive_a_save_and_load() {
    let mut s = session();
    s.run(
        "shape.add_circle",
        json!({"name": "Round", "center": [3, 3], "radius_m": 2.0, "material": "lawn"}),
    )
    .unwrap();
    s.run(
        "shape.add_extrusion",
        json!({"name": "Hedge", "outline": [[0, 0], [4, 1], [8, 0]], "closed": false, "height": 1.0, "smooth": true}),
    )
    .unwrap();
    let bytes = s.project().to_json_bytes();
    let back = Project::from_json_bytes(&bytes).unwrap();
    assert_eq!(back.shapes, s.project().shapes);
    assert_eq!(back.shapes[0].curves().len(), 4);
    assert!(back.shapes[1].is_smooth());
    // Files from before curves existed load with none.
    let old = br#"{"format_version": 1, "shapes": [{"type": "area", "id": "S1", "name": "L", "layer": "shapes", "outline": [[0,0],[1,0],[1,1]], "elevation": 0.0}]}"#;
    let p = Project::from_json_bytes(old).unwrap();
    assert!(!p.shapes[0].is_curved());
}
