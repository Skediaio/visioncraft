//! Files of a newer 3dcraft format open the way the app and the CLI open one: `file.open` loads
//! the models a project refers to, and `model.add` imports a file before the model is added.

use serde_json::{Value, json};
use std::path::PathBuf;
use visioncraft_automation::{Access, Headless, rpc};

/// 3dcraft's own fixture of format version 78 (3dcraft 0.3.0; see
/// `visioncraft-core/tests/import.rs`): a block cut by planes, "Roof", and a kept copy of it
/// above z = 6 mm, "Upper". Copied into the test's folder as `house.3dcraft`.
const HOUSE: &str = "plane-cut-house.3dcraft";

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("formats-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn execute(host: &mut Headless, id: u64, command: &str, params: Value) -> Value {
    let line = rpc::handle_line(
        host,
        &json!({"id": id, "method": "engine.execute", "params": {"command": command, "params": params}})
            .to_string(),
    )
    .expect("a reply");
    serde_json::from_str(&line).expect("JSON")
}

fn close(got: &Value, want: [f32; 3]) {
    let got = got.as_array().unwrap();
    for k in 0..3 {
        let g = got[k].as_f64().unwrap();
        assert!((g - f64::from(want[k])).abs() < 1e-6, "{got:?} != {want:?}");
    }
}

#[test]
fn a_format_78_design_opens_through_the_commands() {
    let root = dir("format-78");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../visioncraft-core/tests/data/",
            "plane-cut-house.3dcraft"
        ),
        root.join(HOUSE),
    )
    .unwrap();
    std::fs::write(
        root.join("p.visioncraft"),
        format!(
            r#"{{"format_version": 1, "models": [{{"id": "m1", "name": "House", "kind": "3dcraft", "source": "{HOUSE}"}}]}}"#
        ),
    )
    .unwrap();
    let mut h = Headless::new(Access::roots(Some(root.clone()), Some(root.clone())));

    let opened = execute(&mut h, 1, "file.open", json!({"path": "p.visioncraft"}));
    assert_eq!(opened["ok"], json!(true), "{opened}");
    assert_eq!(opened["result"]["models"], json!(1));
    assert_eq!(opened["result"]["missingModels"], json!([]), "{opened}");
    let inspected = execute(&mut h, 2, "document.inspect", json!({"faces": true}));
    assert_eq!(inspected["ok"], json!(true), "{inspected}");
    let m = &inspected["result"]["models"][0];
    assert_eq!(m["loaded"], json!(true));
    assert_eq!(m["bodies"], json!(2), "{m}");
    assert_eq!(m["triangles"], json!(32), "{m}");
    assert!(m.get("warnings").is_none(), "{m}");
    // x 2..40 mm, y (3dcraft's z) 0..20 mm, z -30..0 mm: the union of Roof and Upper.
    close(&m["bbox"][0], [0.002, 0.0, -0.03]);
    close(&m["bbox"][1], [0.04, 0.02, 0.0]);
    let faces: Vec<&str> = m["faces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert_eq!(faces.len(), 14, "{faces:?}");
    assert!(
        faces
            .iter()
            .all(|f| f.starts_with("F3:") || f.starts_with("F4:")),
        "{faces:?}"
    );

    // `model.add` imports the file before the model is added, so it is loaded at once.
    let added = execute(
        &mut h,
        3,
        "model.add",
        json!({"name": "House 2", "kind": "3dcraft", "source": HOUSE}),
    );
    assert_eq!(added["ok"], json!(true), "{added}");
    let inspected = execute(&mut h, 4, "document.inspect", json!({}));
    let models = inspected["result"]["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[1]["bodies"], json!(2), "{}", models[1]);
}
