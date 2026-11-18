//! Wall photos through the host: the size read from the file, files that cannot be used, and what
//! `document.inspect` says of the face.

use serde_json::{Value, json};
use std::path::PathBuf;
use visioncraft_automation::commands::execute;
use visioncraft_automation::{Access, Headless};

fn host(name: &str) -> (Headless, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("wall-photo-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/facade.png"),
        dir.join("facade.png"),
    )
    .unwrap();
    let mut h = Headless::new(Access::roots(Some(dir.clone()), Some(dir.clone())));
    run(
        &mut h,
        "shape.add_extrusion",
        json!({"name": "House", "outline": [[4, 2], [16, 2], [16, 9], [4, 9]], "height": 6}),
    )
    .unwrap();
    (h, dir)
}

fn run(h: &mut Headless, id: &str, params: Value) -> Result<Value, String> {
    execute(h, id, &params)
}

fn target(side: u32) -> Value {
    json!({"type": "face", "owner": "S1", "face": format!("S1:side({side})")})
}

fn photo(h: &mut Headless) -> Value {
    run(h, "document.inspect", json!({"faces": false})).unwrap()["wall_photos"][0].clone()
}

#[test]
fn the_host_reads_the_size_of_the_photo_and_measures_the_face() {
    let (mut h, _) = host("size");
    run(
        &mut h,
        "wall_photo.add",
        json!({"file": "facade.png", "target": target(2)}),
    )
    .unwrap();
    let p = photo(&mut h);
    assert_eq!(p["size_px"], json!([500, 400]));
    assert_eq!(p["loaded"], true);
    assert_eq!(p["name"], "facade");
    assert_eq!(p["region"], Value::Null);
    // no corners given: the photo's own, bottom-left first
    assert_eq!(
        p["corners_px"],
        json!([[0.0, 400.0], [500.0, 400.0], [500.0, 0.0], [0.0, 0.0]])
    );
    assert_eq!(p["face_size_m"], json!([12.0, 6.0]));
    assert!(p.get("face_error").is_none(), "{p}");
    // a face that is not there is said in `face_error`, not refused: after an edit it may return
    run(
        &mut h,
        "wall_photo.set",
        json!({"id": "p1", "target": target(9)}),
    )
    .unwrap();
    assert!(
        photo(&mut h)["face_error"]
            .as_str()
            .unwrap()
            .contains("no face")
    );
    // the top of the block lies flat
    run(
        &mut h,
        "wall_photo.set",
        json!({"id": "p1", "target": {"type": "face", "owner": "S1", "face": "S1:top"}}),
    )
    .unwrap();
    assert!(
        photo(&mut h)["face_error"]
            .as_str()
            .unwrap()
            .contains("flat")
    );
}

#[test]
fn a_missing_or_unreadable_file_is_a_clear_error_and_changes_nothing() {
    let (mut h, dir) = host("bad");
    let err = run(
        &mut h,
        "wall_photo.add",
        json!({"file": "nope.png", "target": target(2)}),
    )
    .unwrap_err();
    assert!(
        err.contains("wall_photo.add") && err.contains("cannot read the image"),
        "{err}"
    );
    std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
    let err = run(
        &mut h,
        "wall_photo.add",
        json!({"file": "notes.txt", "target": target(2)}),
    )
    .unwrap_err();
    assert!(err.contains("not a PNG or JPEG"), "{err}");
    let png = std::fs::read(dir.join("facade.png")).unwrap();
    std::fs::write(dir.join("cut.png"), &png[..10]).unwrap();
    let err = run(
        &mut h,
        "wall_photo.add",
        json!({"file": "cut.png", "target": target(2)}),
    )
    .unwrap_err();
    assert!(err.contains("cut.png"), "{err}");
    let err = run(
        &mut h,
        "wall_photo.add",
        json!({"file": "../elsewhere.png", "target": target(2)}),
    )
    .unwrap_err();
    assert!(err.contains("wall_photo.add"), "{err}");
    // a refused command (mirrored corners) leaves nothing behind, and says what is right
    let err = run(
        &mut h,
        "wall_photo.add",
        json!({"file": "facade.png", "target": target(2),
               "corners_px": [[450, 325], [50, 350], [80, 50], [410, 75]]}),
    )
    .unwrap_err();
    assert!(err.contains("bottom-left"), "{err}");
    let i = run(&mut h, "document.inspect", json!({"faces": false})).unwrap();
    assert!(i["wall_photos"].as_array().unwrap().is_empty());
    // a stated size lets a file this host does not have be named; inspect says it is missing
    run(
        &mut h,
        "wall_photo.add",
        json!({"file": "later.jpg", "target": target(2), "size_px": [10, 10]}),
    )
    .unwrap();
    let p = photo(&mut h);
    assert_eq!(p["loaded"], false);
    assert!(p["error"].as_str().unwrap().contains("cannot read"), "{p}");
    // a stated size does not excuse a file that is not an image
    let err = run(
        &mut h,
        "wall_photo.add",
        json!({"file": "notes.txt", "target": target(1), "size_px": [10, 10]}),
    )
    .unwrap_err();
    assert!(err.contains("not a PNG or JPEG"), "{err}");
}

#[test]
fn wall_photo_commands_are_listed_with_examples() {
    let (h, _) = host("docs");
    let list = visioncraft_automation::commands::command_list(
        &h.session,
        &visioncraft_automation::commands::ListQuery {
            filter: Some("wall_photo"),
            ..Default::default()
        },
    );
    let ids: Vec<&str> = list["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    for id in ["wall_photo.add", "wall_photo.set", "wall_photo.remove"] {
        assert!(ids.contains(&id), "{ids:?}");
    }
}
