//! Underlays through the host: the size read from the file, and files that cannot be used.

use serde_json::{Value, json};
use std::path::PathBuf;
use visioncraft_automation::commands::execute;
use visioncraft_automation::{Access, Headless};

fn host(name: &str) -> (Headless, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("underlay-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/site-plan.png"),
        dir.join("plan.png"),
    )
    .unwrap();
    (
        Headless::new(Access::roots(Some(dir.clone()), Some(dir.clone()))),
        dir,
    )
}

fn run(h: &mut Headless, id: &str, params: Value) -> Result<Value, String> {
    execute(h, id, &params)
}

#[test]
fn the_host_reads_the_size_of_the_image() {
    let (mut h, _) = host("size");
    run(&mut h, "underlay.add", json!({"file": "plan.png"})).unwrap();
    let u =
        run(&mut h, "document.inspect", json!({"faces": false})).unwrap()["underlays"][0].clone();
    assert_eq!(u["size_px"], json!([200, 120]));
    assert_eq!(u["loaded"], true);
    assert_eq!(u["name"], "plan");
}

#[test]
fn a_missing_or_unreadable_file_is_a_clear_error_and_changes_nothing() {
    let (mut h, dir) = host("bad");
    let err = run(&mut h, "underlay.add", json!({"file": "nope.png"})).unwrap_err();
    assert!(
        err.contains("underlay.add") && err.contains("cannot read the image"),
        "{err}"
    );
    std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
    let err = run(&mut h, "underlay.add", json!({"file": "notes.txt"})).unwrap_err();
    assert!(err.contains("not a PNG or JPEG"), "{err}");
    // cut short, and lying about its type
    let png = std::fs::read(dir.join("plan.png")).unwrap();
    std::fs::write(dir.join("cut.png"), &png[..10]).unwrap();
    let err = run(&mut h, "underlay.add", json!({"file": "cut.png"})).unwrap_err();
    assert!(err.contains("cut.png"), "{err}");
    // outside the read root
    let err = run(&mut h, "underlay.add", json!({"file": "../elsewhere.png"})).unwrap_err();
    assert!(err.contains("underlay.add"), "{err}");
    // a stated size lets a file this host does not have be named; inspect says it is missing
    run(
        &mut h,
        "underlay.add",
        json!({"file": "later.png", "size_px": [10, 10]}),
    )
    .unwrap();
    let u =
        run(&mut h, "document.inspect", json!({"faces": false})).unwrap()["underlays"][0].clone();
    assert_eq!(u["loaded"], false);
    assert!(u["error"].as_str().unwrap().contains("cannot read"), "{u}");
    let i = run(&mut h, "document.inspect", json!({"faces": false})).unwrap();
    assert_eq!(i["underlays"].as_array().unwrap().len(), 1);
    // a stated size does not excuse a file that is not an image
    let err = run(
        &mut h,
        "underlay.add",
        json!({"file": "notes.txt", "size_px": [10, 10]}),
    )
    .unwrap_err();
    assert!(err.contains("not a PNG or JPEG"), "{err}");
}

#[test]
fn underlay_commands_are_listed_with_examples() {
    let (h, _) = host("docs");
    let list = visioncraft_automation::commands::command_list(
        &h.session,
        &visioncraft_automation::commands::ListQuery {
            filter: Some("underlay"),
            ..Default::default()
        },
    );
    let ids: Vec<&str> = list["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    for id in [
        "underlay.add",
        "underlay.set",
        "underlay.calibrate",
        "underlay.scale_by_distance",
        "underlay.remove",
    ] {
        assert!(ids.contains(&id), "{ids:?}");
    }
}
