//! Command specs, strict parameters, transactions, no-ops and queries.

use serde_json::{Value, json};
use visioncraft_core::{CommandError, Project, Session, example_project};

fn example_session(command: &str) -> Session {
    Session::new(example_project(command))
}

fn specs() -> Vec<visioncraft_core::CommandSpec> {
    Session::new(Project::default())
        .command_specs()
        .into_iter()
        .cloned()
        .collect()
}

#[test]
fn every_example_runs() {
    let all = specs();
    assert!(all.len() >= 55, "{}", all.len());
    for spec in &all {
        let mut s = example_session(&spec.name);
        let revision = s.revision();
        let result = s.run(&spec.name, spec.example.clone());
        assert!(result.is_ok(), "{}: {:?}", spec.name, result);
        if !spec.undoable {
            assert_eq!(s.revision(), revision, "{} touched history", spec.name);
            assert!(!s.can_undo());
        }
        assert!(
            !spec.summary.is_empty() && !spec.label.is_empty() && !spec.result.is_empty(),
            "{}",
            spec.name
        );
    }
}

#[test]
fn a_bogus_key_fails_before_anything_changes() {
    for spec in specs() {
        let mut s = example_session(&spec.name);
        let before = s.project().clone();
        let mut params = spec.example.clone();
        params["lenght_bogus"] = json!(1);
        let err = s.run(&spec.name, params).unwrap_err();
        assert!(
            matches!(err, CommandError::BadParams(_)),
            "{}: {err:?}",
            spec.name
        );
        assert_eq!(*s.project(), before, "{}", spec.name);
        assert!(!s.can_undo(), "{}", spec.name);
    }
}

#[test]
fn the_error_names_the_nearest_key() {
    let mut s = example_session("shape.add_area");
    let err = s
        .run(
            "shape.add_area",
            json!({"name": "x", "outlin": [[0, 0], [1, 0], [1, 1]]}),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("unknown parameter `outlin`")
            && err.to_string().contains("did you mean `outline`"),
        "{err}"
    );
    let err = s
        .run("shape.add_area", json!({"name": "x", "outline": "square"}))
        .unwrap_err();
    assert!(
        err.to_string().contains("`outline` must be [[x, z], ...]"),
        "{err}"
    );
    let err = s.run("shape.add_area", json!({"outline": []})).unwrap_err();
    assert!(
        err.to_string().contains("needs the parameter `name`"),
        "{err}"
    );
    let err = s
        .run(
            "planting.apply_preset",
            json!({"id": "g1", "preset": "nope"}),
        )
        .unwrap_err();
    assert!(err.to_string().contains("one of"), "{err}");
    let err = s
        .run("planting.update", json!({"id": "g1", "densty_per_m2": 1.0}))
        .unwrap_err();
    assert!(
        err.to_string().contains("did you mean `density_per_m2`"),
        "{err}"
    );
}

#[test]
fn handlers_accept_every_declared_parameter() {
    for spec in specs() {
        for p in &spec.params {
            // Each parameter with a plain default, given explicitly, must be accepted.
            if let Some(default) = &p.default
                && spec.example.get(&p.name).is_none()
            {
                let mut s = example_session(&spec.name);
                let mut params = spec.example.clone();
                params[&p.name] = default.clone();
                let r = s.run(&spec.name, params);
                assert!(r.is_ok(), "{} with {}={default}: {r:?}", spec.name, p.name);
            }
            // Each allowed value is accepted by the spec and the handler (it may fail for its own
            // reasons, such as a missing layer, but never as an unknown key or value).
            for value in p.allowed.iter().flatten() {
                let mut s = example_session(&spec.name);
                let mut params = spec.example.clone();
                params[&p.name] = value.clone();
                if let Err(e) = s.run(&spec.name, params) {
                    let text = e.to_string();
                    assert!(
                        !text.contains("unknown parameter")
                            && !text.contains("must be one of")
                            && !text.contains("unknown field"),
                        "{} {}={value}: {text}",
                        spec.name,
                        p.name
                    );
                }
            }
        }
    }
}

#[test]
fn old_style_register_gives_an_open_minimal_spec() {
    let mut s = Session::new(Project::default());
    s.register("ext.tag", "Store a tag", |p, params| {
        p.extensions.insert("test.tag".into(), params.clone());
        Ok(json!("done"))
    });
    let spec = s.command_spec("ext.tag").unwrap();
    assert!(spec.open && spec.params.is_empty() && spec.summary == "Store a tag");
    assert!(s.commands().contains(&("ext.tag", "Store a tag")));
    s.run("ext.tag", json!({"anything": [1, 2]})).unwrap();
    let names: Vec<_> = s.command_specs().iter().map(|c| c.name.clone()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

fn time(s: &mut Session, hour: f32) -> Result<Value, CommandError> {
    s.run("time.set", json!({ "hour": hour }))
}

#[test]
fn a_transaction_is_one_undo_step_and_rolls_back_whole() {
    let mut s = example_session("x");
    s.register("ext.tag", "Store a tag", |p, params| {
        p.extensions.insert("test.tag".into(), params.clone());
        Ok(Value::Null)
    });
    let start = s.project().clone();
    let revision = s.revision();
    s.transaction(|s| {
        time(s, 9.0)?;
        s.run("shape.set_height", json!({"id": "S2", "height": 4.0}))?;
        s.run("ext.tag", json!({"n": 1}))?;
        // A nested transaction joins this one.
        s.transaction(|s| time(s, 10.0).map(|_| ()))?;
        assert!(!s.undo());
        Ok(())
    })
    .unwrap();
    assert_eq!(s.revision(), revision + 1);
    assert_eq!(s.project().time.hour, 10.0);
    assert!(s.undo());
    assert_eq!(*s.project(), start);
    assert!(!s.can_undo());
    assert!(s.redo());
    assert_eq!(s.project().time.hour, 10.0);

    // A failure takes everything back, including the stacks and the revision.
    s.undo();
    time(&mut s, 7.0).unwrap();
    s.undo();
    let (project, revision) = (s.project().clone(), s.revision());
    let (can_undo, can_redo) = (s.can_undo(), s.can_redo());
    let mut done = 0;
    let err = s.transaction(|s| {
        time(s, 11.0)?;
        done += 1;
        s.run("shape.set_height", json!({"id": "S2", "height": 9.0}))?;
        done += 1;
        s.run("shape.remove", json!({"id": "nope"}))?;
        done += 1;
        Ok(())
    });
    assert!(err.is_err() && done == 2);
    assert_eq!(*s.project(), project);
    assert_eq!(
        (s.revision(), s.can_undo(), s.can_redo()),
        (revision, can_undo, can_redo)
    );
}

#[test]
fn commands_that_change_nothing_leave_history_alone() {
    let mut s = example_session("x");
    time(&mut s, 12.0).unwrap();
    let revision = s.revision();
    time(&mut s, 12.0).unwrap();
    assert_eq!(s.revision(), revision);
    // A terrain stroke outside the grid copies the terrain but changes nothing.
    s.run(
        "terrain.stroke",
        json!({"tool": "raise", "points": [[500.0, 500.0]], "radius_m": 2, "strength": 0.5}),
    )
    .unwrap();
    assert_eq!(s.revision(), revision);
    s.run(
        "terrain.stroke",
        json!({"tool": "raise", "points": [[10.0, 10.0]], "radius_m": 2, "strength": 0.5}),
    )
    .unwrap();
    assert_eq!(s.revision(), revision + 1);
    // An empty transaction is a no-op too, and queries never count.
    s.transaction(|_| Ok(())).unwrap();
    s.run("query.sun", json!({})).unwrap();
    assert_eq!(s.revision(), revision + 1);
    // Two steps remain: the time change and the stroke inside the grid.
    assert_eq!((0..).take_while(|_| s.undo()).count(), 2);
}

#[test]
fn queries_answer_from_the_project() {
    let mut s = example_session("x");
    let sun = s.run("query.sun", json!({})).unwrap();
    // Brussels, 21 June, 14:00 local: high in the south-west.
    assert!(sun["elevation_deg"].as_f64().unwrap() > 50.0);
    assert!(sun["azimuth_deg"].as_f64().unwrap() > 180.0);
    assert!(sun["sunrise_hour"].as_f64().unwrap() < 6.0 && sun["above_horizon"] == true);
    let night = s.run("query.sun", json!({"time": {"hour": 1.0}})).unwrap();
    assert_eq!(night["above_horizon"], false);
    assert!(s.run("query.sun", json!({"time": {"minute": 1}})).is_err());
    let m = |s: &mut Session, p: Value| s.run("query.measure", p).unwrap();
    assert_eq!(
        m(
            &mut s,
            json!({"kind": "length", "points": [[0, 0, 0], [3, 0, 4], [3, 0, 10]]})
        )["value"],
        11.0
    );
    assert_eq!(
        m(
            &mut s,
            json!({"kind": "area", "outline": [[0, 0], [4, 0], [4, 2.5], [0, 2.5]]})
        )["value"],
        10.0
    );
    assert_eq!(
        m(
            &mut s,
            json!({"kind": "height_difference", "points": [[0, 1, 0], [5, 3.5, 0]]})
        )["value"],
        2.5
    );
    assert_eq!(
        m(
            &mut s,
            json!({"kind": "slope", "points": [[0, 0, 0], [10, 1, 0]]})
        )["value"],
        10.0
    );
    assert_eq!(
        m(
            &mut s,
            json!({"kind": "slope", "points": [[0, 0, 0], [0, 1, 0]]})
        )["value"],
        Value::Null
    );
    assert!(
        s.run(
            "query.measure",
            json!({"kind": "area", "points": [[0, 0, 0], [1, 0, 0]]})
        )
        .is_err()
    );
    assert!(!s.can_undo());
}

#[test]
fn replace_project_still_works_inside_the_new_run() {
    let mut s = example_session("x");
    let other = Project::default();
    s.replace_project(other.clone());
    assert_eq!(*s.project(), other);
    assert!(s.undo());
}

#[test]
fn materials_take_orm_variation_and_extensions() {
    let mut s = example_session("x");
    let id = s
        .run("material.add", json!({"name": "Brick", "maps": {"orm": "brick_orm.png", "normal": "brick_n.png"}, "variation": 0.4, "extensions": {"com.example.lib": {"id": 7}}}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let m = s
        .project()
        .materials
        .iter()
        .find(|m| m.id == id)
        .unwrap()
        .clone();
    assert_eq!(m.maps.orm.as_deref(), Some("brick_orm.png"));
    assert_eq!(
        (m.variation, m.extensions["com.example.lib"]["id"].clone()),
        (0.4, json!(7))
    );
    // `maps` merges key by key; null clears; extensions are replaced.
    s.run("material.update", json!({"id": id, "maps": {"roughness": "r.png", "normal": null}, "variation": 0.1, "extensions": {"com.example.other": 1}})).unwrap();
    let m = s
        .project()
        .materials
        .iter()
        .find(|m| m.id == id)
        .unwrap()
        .clone();
    assert_eq!(
        (
            m.maps.orm.as_deref(),
            m.maps.roughness.as_deref(),
            m.maps.normal.clone()
        ),
        (Some("brick_orm.png"), Some("r.png"), None)
    );
    assert_eq!((m.variation, m.extensions.len()), (0.1, 1));
    assert!(m.extensions.contains_key("com.example.other"));
    let err = s
        .run("material.update", json!({"id": id, "maps": {"orn": "x"}}))
        .unwrap_err();
    assert!(err.to_string().contains("did you mean `orm`"), "{err}");
    assert!(
        s.run("material.update", json!({"id": id, "variation": 2.0}))
            .is_err()
    );
    let again = Project::from_json_bytes(&s.project().to_json_bytes()).unwrap();
    assert_eq!(&again, s.project());
}

#[test]
fn placements_know_their_layer() {
    use visioncraft_core::{Generator, evaluate};
    let mut s = example_session("x");
    s.run(
        "planting.apply_preset",
        json!({"id": "g1", "preset": "broadleaf-woodland"}),
    )
    .unwrap();
    s.run(
        "planting.brush_stroke",
        json!({"id": "g1", "points": [[0, 0], [30, 0]], "radius_m": 10}),
    )
    .unwrap();
    let e = evaluate(&s.project().generators[0], &|_, _| None);
    let layers: std::collections::BTreeSet<_> = e.placements.iter().map(|p| p.layer).collect();
    assert_eq!(layers, [Some(0), Some(1), Some(2)].into());
    let plain = evaluate(&s.project().generators[1], &|_, _| None);
    assert!(plain.placements.iter().all(|p| p.layer.is_none()));
    assert!(matches!(
        &s.project().generators[0],
        Generator::MixedBrush(_)
    ));
}

#[test]
fn duplicate_shifts_geometry_and_undo_stroke_pops() {
    use visioncraft_core::Generator;
    let mut s = example_session("x");
    s.run("planting.add_area", json!({"name": "A", "asset_palette": [{"asset": "a", "weight": 1}], "outline": [[0, 0], [4, 0], [4, 4]], "density_per_m2": 1.0, "seed": 5})).unwrap();
    s.run("planting.add_cluster", json!({"name": "C", "asset_palette": [{"asset": "a", "weight": 1}], "center": [1, 2], "radius_m": 1, "count": 3, "seed": 5})).unwrap();
    s.run("planting.add_spline", json!({"name": "S", "asset_palette": [{"asset": "a", "weight": 1}], "points": [[0, 0], [2, 0]], "spacing": {"count": 3}, "seed": 5})).unwrap();
    s.run("planting.add_surface_fill", json!({"name": "F", "asset_palette": [{"asset": "a", "weight": 1}], "owner": "S1", "faces": ["S1:top"], "density_per_m2": 1.0, "seed": 5})).unwrap();
    let ids: Vec<String> = s
        .project()
        .generators
        .iter()
        .map(|g| g.id().to_string())
        .collect();
    assert_eq!(ids.len(), 6);
    for id in &ids {
        let r = s
            .run(
                "planting.duplicate",
                json!({"id": id, "offset": [10, -2], "new_seed": true}),
            )
            .unwrap();
        assert!(!ids.contains(&r["id"].as_str().unwrap().to_string()));
    }
    let gens = &s.project().generators;
    assert_eq!(gens.len(), 12);
    let (a, b) = (&gens[0], &gens[6]);
    let (Generator::MixedBrush(a), Generator::MixedBrush(b)) = (a, b) else {
        panic!()
    };
    assert_eq!(b.strokes[0].points[0], [10.0, -2.0]);
    assert_eq!((b.name.as_str(), b.seed != a.seed), ("Woodland copy", true));
    let Generator::Single(c) = &gens[7] else {
        panic!()
    };
    assert_eq!(c.position, [14.0, 0.0, 1.0]);
    let Generator::AreaScatter(c) = &gens[8] else {
        panic!()
    };
    assert_eq!(c.outline[1], [14.0, -2.0]);
    let Generator::Cluster(c) = &gens[9] else {
        panic!()
    };
    assert_eq!(c.center, [11.0, 0.0]);
    let Generator::SplineMass(c) = &gens[10] else {
        panic!()
    };
    assert_eq!(c.points[1], [12.0, -2.0]);
    let (Generator::SurfaceFill(f), Generator::SurfaceFill(g)) = (&gens[5], &gens[11]) else {
        panic!()
    };
    assert_eq!((&f.faces, f.seed != g.seed), (&g.faces, true));
    // Without new_seed the seed stays.
    s.run("planting.duplicate", json!({"id": "g3"})).unwrap();
    let (Generator::AreaScatter(x), Generator::AreaScatter(y)) = (
        &s.project().generators[2],
        s.project().generators.last().unwrap(),
    ) else {
        panic!()
    };
    assert_eq!((x.seed, y.seed, x.outline == y.outline), (5, 5, true));
    // Strokes can be taken back one by one.
    s.run("planting.undo_stroke", json!({"id": "g1"})).unwrap();
    assert!(s.run("planting.undo_stroke", json!({"id": "g1"})).is_err());
    assert!(s.run("planting.undo_stroke", json!({"id": "g2"})).is_err());
}

#[test]
fn the_session_reports_what_the_last_terrain_command_touched() {
    use visioncraft_core::DirtyRect;
    let mut s = example_session("x");
    assert_eq!(s.last_terrain_dirty(), None);
    s.run("terrain.stroke", json!({"tool": "raise", "points": [[10.0, 10.0]], "radius_m": 2.0, "strength": 0.5, "falloff": 0.0})).unwrap();
    let d = s.last_terrain_dirty().unwrap();
    assert_eq!(
        d,
        DirtyRect {
            x0: 16,
            z0: 16,
            x1: 24,
            z1: 24
        }
    );
    time(&mut s, 8.0).unwrap();
    assert_eq!(s.last_terrain_dirty(), None);
    s.run(
        "terrain.stroke",
        json!({"tool": "raise", "points": [[500.0, 5.0]], "radius_m": 2.0, "strength": 0.5}),
    )
    .unwrap();
    assert_eq!(s.last_terrain_dirty(), None);
    assert!(s.undo());
    let whole = s.last_terrain_dirty().unwrap();
    assert_eq!((whole.x0, whole.z0, whole.x1, whole.z1), (0, 0, 80, 60));
    // A transaction reports the union of its commands.
    s.transaction(|s| {
        s.run("terrain.stroke", json!({"tool": "raise", "points": [[2.0, 2.0]], "radius_m": 1.0, "strength": 0.5, "falloff": 0.0}))?;
        s.run("terrain.stroke", json!({"tool": "raise", "points": [[30.0, 20.0]], "radius_m": 1.0, "strength": 0.5, "falloff": 0.0}))?;
        time(s, 9.0).map(|_| ())
    })
    .unwrap();
    let u = s.last_terrain_dirty().unwrap();
    assert!(u.x0 <= 4 && u.z0 <= 4 && u.x1 >= 60 && u.z1 >= 40, "{u:?}");
    let _ = s.run("terrain.remove", json!({}));
    assert_eq!(s.last_terrain_dirty(), None);
}

#[test]
fn many_lamps_make_capped_lights() {
    use visioncraft_core::{LightKind, LightSpec, Placement, lights_for_placements};
    let spec = LightSpec {
        kind: LightKind::Point,
        color_temperature_k: 2700.0,
        lumens: 300.0,
        range_m: 5.0,
        offset_m: [0.0, 1.0, 0.0],
    };
    let at = |x: f32| Placement {
        asset: "lamp".into(),
        position: [x, 0.0, 0.0],
        rotation_y_deg: 0.0,
        scale: 1.0,
        variant: 0,
        layer: None,
    };
    let placements: Vec<_> = (0..5).map(|i| at(i as f32)).collect();
    let (lights, skipped) = lights_for_placements(&spec, &placements, "li-g4", "g4", "lighting", 3);
    assert_eq!((lights.len(), skipped), (3, 2));
    assert_eq!(lights[2].id, "li-g4-2");
    assert_eq!(
        (
            lights[2].position,
            lights[2].source.as_deref(),
            lights[2].layer.as_str()
        ),
        ([2.0, 1.0, 0.0], Some("g4"), "lighting")
    );
    assert_eq!(
        lights_for_placements(&spec, &placements, "x", "g4", "lighting", 10).1,
        0
    );
}
