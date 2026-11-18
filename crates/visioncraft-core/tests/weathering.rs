//! Weathering: a material's own (`material.add` / `material.update`) and the view's global one
//! (`view.set`), with undo, and files from before it existed unchanged.

use serde_json::json;
use visioncraft_core::{Project, Session, Weathering};

fn written(p: &Project) -> serde_json::Value {
    serde_json::from_slice(&p.to_json_bytes()).unwrap()
}

/// Whether the file has weathering of the new kind: on a material or on the view (a palette
/// kind's own `weathering` number is the older, simpler one and stays where it is).
fn has_weathering(p: &Project) -> bool {
    let v = written(p);
    v["view"].get("weathering").is_some()
        || v["materials"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m.get("weathering").is_some())
}

fn session() -> Session {
    Session::new(Project::default())
}

#[test]
fn a_project_without_weathering_writes_none_of_it() {
    let p = Project::default();
    assert!(!has_weathering(&p));
    assert!(p.materials.iter().all(|m| m.weathering.is_none()));
    // An old file (no field) reads back the same, and writes the same.
    let back = Project::from_json_bytes(&p.to_json_bytes()).unwrap();
    assert_eq!(back.view.weathering, 0.0);
    assert_eq!(back.to_json_bytes(), p.to_json_bytes());
}

#[test]
fn a_material_takes_weathering_with_defaults_merges_and_undoes() {
    let mut s = session();
    let before = s.project().to_json_bytes();
    s.run(
        "material.update",
        json!({"id": "plaster", "weathering": {"amount": 0.7}}),
    )
    .unwrap();
    let w = s
        .project()
        .materials
        .iter()
        .find(|m| m.id == "plaster")
        .unwrap()
        .weathering;
    assert_eq!(w, Some(Weathering::new(0.7)));
    // Keys given change, the rest stay.
    s.run(
        "material.update",
        json!({"id": "plaster", "weathering": {"moss": 0.9}}),
    )
    .unwrap();
    let w = s
        .project()
        .materials
        .iter()
        .find(|m| m.id == "plaster")
        .unwrap()
        .weathering
        .unwrap();
    assert_eq!(
        (w.amount, w.moss, w.dirt),
        (0.7, 0.9, Weathering::DEFAULT_DIRT)
    );
    let file = written(s.project());
    let m = file["materials"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "plaster")
        .unwrap();
    assert_eq!(m["weathering"]["moss"], 0.9);
    // A first one needs `amount`; numbers out of range and typos are refused.
    assert!(
        s.run(
            "material.update",
            json!({"id": "brick", "weathering": {"moss": 0.9}})
        )
        .is_err()
    );
    assert!(
        s.run(
            "material.update",
            json!({"id": "brick", "weathering": {"amount": 1.5}})
        )
        .is_err()
    );
    assert!(
        s.run(
            "material.update",
            json!({"id": "brick", "weathering": {"amount": 0.5, "mosss": 1}})
        )
        .is_err()
    );
    // null clears it; undo brings it back, and undoing everything restores the file.
    s.run(
        "material.update",
        json!({"id": "plaster", "weathering": null}),
    )
    .unwrap();
    assert!(
        s.project()
            .materials
            .iter()
            .find(|m| m.id == "plaster")
            .unwrap()
            .weathering
            .is_none()
    );
    assert!(s.undo());
    assert!(
        s.project()
            .materials
            .iter()
            .find(|m| m.id == "plaster")
            .unwrap()
            .weathering
            .is_some()
    );
    while s.undo() {}
    assert_eq!(s.project().to_json_bytes(), before);
}

#[test]
fn material_add_takes_weathering() {
    let mut s = session();
    let id = s
        .run(
            "material.add",
            json!({"name": "Old brick", "weathering": {"amount": 0.5, "streaks": 0}}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let m = s.project().materials.iter().find(|m| m.id == id).unwrap();
    assert_eq!(
        m.weathering.map(|w| (w.amount, w.streaks, w.dirt)),
        Some((0.5, 0.0, 0.6))
    );
}

#[test]
fn view_set_weathering_is_stored_validated_and_undoable() {
    let mut s = session();
    s.run("view.set", json!({"weathering": 0.7})).unwrap();
    assert_eq!(s.project().view.weathering, 0.7);
    assert_eq!(written(s.project())["view"]["weathering"], 0.7);
    assert!(s.run("view.set", json!({"weathering": 1.2})).is_err());
    assert_eq!(s.project().view.weathering, 0.7);
    assert!(s.undo());
    assert_eq!(s.project().view.weathering, 0.0);
    // Off again: not written.
    s.run("view.set", json!({"weathering": 0.3})).unwrap();
    s.run("view.set", json!({"weathering": 0})).unwrap();
    assert!(!has_weathering(s.project()));
}

#[test]
fn strengths_add_the_material_and_the_view_as_layers() {
    assert_eq!(Weathering::strengths(None, 0.0), [0.0; 4]);
    let g = Weathering::strengths(None, 0.5);
    assert_eq!(g, [0.3, 0.2, 0.2, 0.25]);
    let own = Weathering {
        amount: 0.5,
        dirt: 1.0,
        moss: 0.0,
        wear: 0.0,
        streaks: 0.0,
    };
    let both = Weathering::strengths(Some(own), 0.5);
    assert!((both[0] - 0.75).abs() < 1e-6 && both[1] == 0.0);
}
