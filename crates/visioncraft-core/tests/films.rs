//! Film commands: undo, validation, and the project file (old files unchanged).

use serde_json::{Value, json};
use visioncraft_core::{Project, Session};

fn walk() -> Value {
    json!({"kind": "walk", "path": [[0, 0], [0, 10]], "speed_mps": 1.25})
}

#[test]
fn films_and_shots_are_commands_with_undo() {
    let mut s = Session::new(Project::default());
    assert_eq!(
        s.run("film.add", json!({"name": "Tour"})).unwrap(),
        json!({"id": "f1"})
    );
    s.run("film.add_shot", json!({"id": "f1", "shot": walk()}))
        .unwrap();
    let hold = json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 2});
    let r = s
        .run(
            "film.add_shot",
            json!({"id": "f1", "shot": hold, "index": 0}),
        )
        .unwrap();
    assert_eq!(r, json!({"index": 0, "shots": 2}));
    assert_eq!(s.project().films[0].shots[0].kind.name(), "hold");
    // a new duration replaces the speed; the kind stays
    s.run(
        "film.update_shot",
        json!({"id": "f1", "index": 1, "shot": {"duration_s": 8, "ease": "none"}}),
    )
    .unwrap();
    let saved: Value = serde_json::to_value(&s.project().films[0].shots[1]).unwrap();
    assert_eq!(saved["duration_s"], 8.0);
    assert!(saved.get("speed_mps").is_none(), "{saved}");
    let err = s
        .run(
            "film.update_shot",
            json!({"id": "f1", "index": 1, "shot": {"kind": "orbit"}}),
        )
        .unwrap_err();
    assert!(err.to_string().contains("keeps its kind"), "{err}");
    s.run(
        "film.update",
        json!({"id": "f1", "fps": 30, "name": "Tour 2"}),
    )
    .unwrap();
    s.run("film.remove_shot", json!({"id": "f1", "index": 0}))
        .unwrap();
    assert_eq!(s.project().films[0].shots.len(), 1);

    // every step is one undo step, back to the start
    while s.can_undo() {
        s.undo();
    }
    assert!(s.project().films.is_empty());
    s.redo();
    assert_eq!(s.project().films[0].name, "Tour");
}

#[test]
fn bad_input_is_refused_and_changes_nothing() {
    let mut s = Session::new(Project::default());
    s.run("film.add", json!({"name": "Tour"})).unwrap();
    let before = s.project().clone();
    let why = |s: &mut Session, id: &str, p: Value| s.run(id, p).unwrap_err().to_string();
    let add = |shot: Value| json!({"id": "f1", "shot": shot});
    assert!(
        why(
            &mut s,
            "film.add_shot",
            add(json!({"kind": "walk", "path": [], "speed_mps": 1}))
        )
        .contains("at least 2")
    );
    assert!(
        why(
            &mut s,
            "film.add_shot",
            add(json!({"kind": "walk", "path": [[0, 0], [1, 1]], "speed_mps": 0}))
        )
        .contains("`speed_mps` must be above 0")
    );
    assert!(
        why(
            &mut s,
            "film.add_shot",
            add(json!({"kind": "hold", "eye": [0, 1, 0], "target": [0, 1, 1], "duration_s": -1}))
        )
        .contains("`duration_s`")
    );
    assert!(
        why(
            &mut s,
            "film.add_shot",
            add(json!({"kind": "flyover", "path": [[0, 0], [5, 5]], "height_m": 10, "pitch_degg": 3}))
        )
        .contains("did you mean `pitch_deg`")
    );
    assert!(
        why(&mut s, "film.add_shot", json!({"id": "f9", "shot": walk()}))
            .contains("no film `f9`; the films are: f1")
    );
    assert!(
        why(
            &mut s,
            "film.add_shot",
            json!({"id": "f1", "shot": walk(), "index": 3})
        )
        .contains("past the end")
    );
    assert!(why(&mut s, "film.remove_shot", json!({"id": "f1", "index": 0})).contains("no shot 0"));
    assert!(why(&mut s, "film.add", json!({"name": "x", "fps": 0})).contains("`fps`"));
    assert_eq!(*s.project(), before);
    assert!(s.can_undo()); // only the film.add above
}

#[test]
fn a_project_without_films_is_written_as_before() {
    let p = Project::default();
    let saved: Value = serde_json::from_slice(&p.to_json_bytes()).unwrap();
    assert!(saved.get("films").is_none());
    // an old file has no films and loads, and writes none
    let mut old = saved.clone();
    old.as_object_mut().unwrap().remove("films");
    let loaded = Project::from_json_bytes(&serde_json::to_vec(&old).unwrap()).unwrap();
    assert!(loaded.films.is_empty());
    assert_eq!(loaded.to_json_bytes(), p.to_json_bytes());
}

#[test]
fn a_film_survives_the_project_file() {
    let mut s = Session::new(Project::default());
    s.run("film.add", json!({"name": "Tour", "fps": 24}))
        .unwrap();
    let shot = json!({"kind": "orbit", "center": [1.0, 0.0, 2.0], "radius_m": 6.0, "height_m": 3.0,
        "from_deg": 0.0, "to_deg": 720.0, "duration_s": 8.0, "ease": "in_out",
        "time_of_day": [17.0, 19.5], "date": {"month": 6, "day": 21}, "years": [0.0, 10.0]});
    s.run("film.add_shot", json!({"id": "f1", "shot": shot}))
        .unwrap();
    let back = Project::from_json_bytes(&s.project().to_json_bytes()).unwrap();
    assert_eq!(&back, s.project());
    let saved: Value = serde_json::to_value(&back.films[0].shots[0]).unwrap();
    assert_eq!(saved, shot);
}
