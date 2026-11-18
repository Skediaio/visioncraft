//! `film.preview` and `film.render` through the host.

use serde_json::{Value, json};
use std::path::PathBuf;
use visioncraft_automation::commands::execute;
use visioncraft_automation::{Access, Headless};

fn host(name: &str) -> (Headless, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("film-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (
        Headless::new(Access::roots(Some(dir.clone()), Some(dir.clone()))),
        dir,
    )
}

fn run(h: &mut Headless, id: &str, params: Value) -> Result<Value, String> {
    execute(h, id, &params)
}

fn tour(h: &mut Headless) {
    run(
        h,
        "terrain.create",
        json!({"origin": [-10, -10], "width_m": 40, "depth_m": 40, "base_material": "lawn"}),
    )
    .unwrap();
    run(h, "film.add", json!({"name": "Tour"})).unwrap();
    for shot in [
        json!({"kind": "walk", "path": [[0, 0], [0, 12]], "speed_mps": 1.2, "ease": "none", "time_of_day": [17, 18]}),
        json!({"kind": "orbit", "center": [0, 0, 6], "radius_m": 5, "height_m": 3, "from_deg": 0, "to_deg": 90, "duration_s": 4}),
    ] {
        run(h, "film.add_shot", json!({"id": "f1", "shot": shot})).unwrap();
    }
}

#[test]
fn preview_gives_the_shots_and_the_camera_at_a_time() {
    let (mut h, _) = host("preview");
    tour(&mut h);
    let all = run(&mut h, "film.preview", json!({"id": "f1"})).unwrap();
    assert!(
        (all["duration_s"].as_f64().unwrap() - 14.0).abs() < 0.05,
        "{all}"
    );
    assert_eq!(all["fps"], 25);
    assert_eq!(all["shots"][1]["kind"], "orbit");
    assert!((all["shots"][1]["start_s"].as_f64().unwrap() - 10.0).abs() < 0.05);
    assert!(all.get("camera").is_none());

    let at = run(&mut h, "film.preview", json!({"id": "f1", "t": 5})).unwrap();
    let eye = &at["camera"]["position"];
    assert!((eye[2].as_f64().unwrap() - 6.0).abs() < 0.1, "{at}");
    assert!((eye[1].as_f64().unwrap() - 1.6).abs() < 0.05, "{at}");
    assert_eq!(at["shot"], 0);
    assert_eq!(
        at["time"]["hour"].as_f64().map(|h| (h * 100.0).round()),
        Some(1750.0)
    );
    assert!(at["sun"]["elevation_deg"].as_f64().unwrap() > 0.0);
    assert!(at["overrides"]["time_of_day"].as_f64().unwrap() > 17.4);

    // past the end is the last frame; a film without shots or a bad id is an error
    let end = run(&mut h, "film.preview", json!({"id": "f1", "t": 99})).unwrap();
    assert_eq!(end["shot"], 1);
    assert!((end["t"].as_f64().unwrap() - 14.0).abs() < 0.05);
    assert!(
        run(&mut h, "film.preview", json!({"id": "f9"}))
            .unwrap_err()
            .contains("no film `f9`")
    );
    assert!(
        run(&mut h, "film.preview", json!({"id": "f1", "tt": 1}))
            .unwrap_err()
            .contains("unknown parameter")
    );
    run(&mut h, "film.add", json!({"name": "Empty"})).unwrap();
    assert_eq!(
        run(&mut h, "film.preview", json!({"id": "f2"})).unwrap()["duration_s"],
        0.0
    );
    assert!(
        run(&mut h, "film.preview", json!({"id": "f2", "t": 0}))
            .unwrap_err()
            .contains("no shots")
    );
}

#[test]
fn the_walk_follows_the_terrain() {
    let (mut h, _) = host("hill");
    tour(&mut h);
    run(
        &mut h,
        "terrain.stroke",
        json!({"tool": "raise", "points": [[0, 4]], "radius_m": 3, "strength": 1.5, "falloff": 0}),
    )
    .unwrap();
    let g = run(&mut h, "query.ground_height", json!({"x": 0, "z": 4})).unwrap();
    let at = run(&mut h, "film.preview", json!({"id": "f1", "t": 2.8})).unwrap();
    let ground = g["height"].as_f64().unwrap();
    let eye = at["camera"]["position"][1].as_f64().unwrap();
    assert!(eye >= ground + 1.0, "eye {eye} over ground {ground}");
}

#[test]
fn render_is_refused_clearly_without_a_gpu_or_outside_the_root() {
    let (mut h, _) = host("render");
    tour(&mut h);
    let e = run(
        &mut h,
        "film.render",
        json!({"id": "f1", "dir": "../elsewhere"}),
    )
    .unwrap_err();
    assert!(e.contains("`..` is not allowed"), "{e}");
    let e = run(
        &mut h,
        "film.render",
        json!({"id": "f1", "dir": "frames", "every": 25}),
    )
    .unwrap_err();
    assert!(e.contains("no renderer"), "{e}");
    let e = run(
        &mut h,
        "film.render",
        json!({"id": "f1", "dir": "frames", "from": 20}),
    )
    .unwrap_err();
    assert!(e.contains("no frames between"), "{e}");
    let e = run(
        &mut h,
        "film.render",
        json!({"id": "f1", "dir": "frames", "every": 0}),
    )
    .unwrap_err();
    assert!(e.contains("`every`"), "{e}");
}
