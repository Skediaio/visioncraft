//! Wetness and rain (`view.set`): stored with the view, not written while 0 so files from before
//! them stay as they were, with undo, and rain implying wetness.

use serde_json::json;
use visioncraft_core::{Project, Session};

fn written(p: &Project) -> serde_json::Value {
    serde_json::from_slice(&p.to_json_bytes()).unwrap()
}

#[test]
fn an_old_project_reads_dry_and_writes_no_wetness_or_rain() {
    let p = Project::default();
    let v = written(&p);
    assert!(v["view"].get("wetness").is_none() && v["view"].get("rain").is_none());
    // A file from before them (no such keys) reads dry and writes the same bytes back.
    let back = Project::from_json_bytes(&p.to_json_bytes()).unwrap();
    assert_eq!((back.view.wetness, back.view.rain), (0.0, 0.0));
    assert_eq!(back.view.wet(), 0.0);
    assert_eq!(back.to_json_bytes(), p.to_json_bytes());
}

#[test]
fn view_set_stores_wetness_and_rain_and_rain_implies_wetness() {
    let mut s = Session::new(Project::default());
    let before = s.project().to_json_bytes();
    s.run("view.set", json!({"wetness": 0.6})).unwrap();
    assert_eq!(s.project().view.wetness, 0.6);
    assert_eq!(s.project().view.wet(), 0.6);
    s.run("view.set", json!({"rain": 0.8})).unwrap();
    assert_eq!(
        (s.project().view.wetness, s.project().view.rain),
        (0.6, 0.8)
    );
    // The wetness drawn is the larger of the two.
    assert_eq!(s.project().view.wet(), 0.8);
    // Written, and read back.
    let v = written(s.project());
    assert!((v["view"]["wetness"].as_f64().unwrap() - 0.6).abs() < 1e-6);
    let back = Project::from_json_bytes(&s.project().to_json_bytes()).unwrap();
    assert_eq!((back.view.wetness, back.view.rain), (0.6, 0.8));
    // Out of range is refused, 0 clears (and is not written), undo restores.
    assert!(s.run("view.set", json!({"rain": 1.5})).is_err());
    assert!(s.run("view.set", json!({"wetness": -0.1})).is_err());
    s.run("view.set", json!({"wetness": 0, "rain": 0})).unwrap();
    assert_eq!(s.project().to_json_bytes(), before);
    assert!(s.undo());
    assert_eq!(s.project().view.rain, 0.8);
}

#[test]
fn rain_may_fall_in_any_season() {
    let mut s = Session::new(Project::default());
    s.run("view.set", json!({"season": "summer", "rain": 0.5}))
        .unwrap();
    assert_eq!(s.project().view.rain, 0.5);
}
