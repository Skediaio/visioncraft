//! Mist and gusts (`view.set mist`, `gustiness`): stored with the view, not written while 0 so
//! files from before them stay as they were, checked, and undone.

use serde_json::json;
use visioncraft_core::{Project, Session};

fn written(p: &Project) -> serde_json::Value {
    serde_json::from_slice(&p.to_json_bytes()).unwrap()
}

#[test]
fn an_old_project_reads_clear_and_still_and_writes_neither() {
    let p = Project::default();
    let v = written(&p);
    assert!(v["view"].get("mist").is_none() && v["view"].get("gustiness").is_none());
    let back = Project::from_json_bytes(&p.to_json_bytes()).unwrap();
    assert_eq!((back.view.mist, back.view.gustiness), (0.0, 0.0));
    // The slow swell keeps its own default.
    assert_eq!(back.view.wind_gustiness, 0.5);
    assert_eq!(back.to_json_bytes(), p.to_json_bytes());
}

#[test]
fn view_set_stores_mist_and_gusts() {
    let mut s = Session::new(Project::default());
    let before = s.project().to_json_bytes();
    s.run("view.set", json!({"mist": 0.6, "gustiness": 1}))
        .unwrap();
    assert_eq!(
        (s.project().view.mist, s.project().view.gustiness),
        (0.6, 1.0)
    );
    let back = Project::from_json_bytes(&s.project().to_json_bytes()).unwrap();
    assert_eq!((back.view.mist, back.view.gustiness), (0.6, 1.0));
    assert!(s.run("view.set", json!({"mist": 1.5})).is_err());
    assert!(s.run("view.set", json!({"gustiness": -0.1})).is_err());
    s.run("view.set", json!({"mist": 0, "gustiness": 0}))
        .unwrap();
    assert_eq!(s.project().to_json_bytes(), before);
    assert!(s.undo());
    assert_eq!(s.project().view.mist, 0.6);
}
