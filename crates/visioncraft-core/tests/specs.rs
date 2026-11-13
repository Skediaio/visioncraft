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
