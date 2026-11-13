//! The built binary: a garden built from a script, saved, reloaded and inspected; exit codes;
//! and (on a machine with a GPU, `--ignored`) a picture that is not blank.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GARDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/visioncraft-automation/docs/examples/garden.json"
);

fn cli(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_visioncraft-cli"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn lines(out: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_garden_is_built_saved_reloaded_and_inspected() {
    let d = dir("garden");
    let out = cli(
        &["run", "--script", GARDEN, "--out", "garden.visioncraft"],
        &d,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let results = lines(&out);
    assert_eq!(results.len(), 7);
    assert_eq!(results[2], json!({"id": "S1"}));
    assert_eq!(results[6], json!({"id": "v1"}));

    let info = cli(&["info", "garden.visioncraft", "--no-faces"], &d);
    assert!(
        info.status.success(),
        "{}",
        String::from_utf8_lossy(&info.stderr)
    );
    let i: Value = serde_json::from_slice(&info.stdout).unwrap();
    assert!(i["path"].as_str().unwrap().ends_with("garden.visioncraft"));
    assert_eq!(i["dirty"], false);
    assert_eq!(i["site"]["latitude"], 51.05);
    assert_eq!(i["time"]["hour"], 15.0);
    let shapes: Vec<(&str, &str)> = i["shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["type"].as_str().unwrap()))
        .collect();
    assert_eq!(shapes, [("Lawn", "area"), ("House", "extrusion")]);
    assert_eq!(i["shapes"][0]["area_m2"], 240.0);
    assert_eq!(i["shapes"][1]["height"], 6.0);
    assert!(i["shapes"][0].get("faces").is_none());
    let assigned: Vec<(&str, &str)> = i["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["target"]["owner"].as_str().unwrap(),
                a["material"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(assigned, [("S1", "lawn"), ("S2", "brick")]);
    assert_eq!(
        i["generators"][0]["assets"],
        json!(["flora:carpinus-hedge"])
    );
    assert!(i["generators"][0]["count"].as_u64().unwrap() > 20);
    assert_eq!(i["lights"][0]["name"], "Garden lamp");
    assert_eq!(i["viewpoints"][0]["name"], "From the gate");

    // Reloaded, changed, saved over itself: the change is there.
    let again = cli(
        &[
            "run",
            "garden.visioncraft",
            "--cmd",
            "time.set",
            "--params",
            r#"{"hour": 21}"#,
            "--out",
            "garden.visioncraft",
        ],
        &d,
    );
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    let i2: Value =
        serde_json::from_slice(&cli(&["info", "garden.visioncraft"], &d).stdout).unwrap();
    assert_eq!(i2["time"]["hour"], 21.0);
    assert_eq!(i2["shapes"][0]["faces"], json!(["S1:top"]));
}

#[test]
fn a_failing_batch_saves_nothing_and_exit_codes_say_why() {
    let d = dir("codes");
    std::fs::write(d.join("bad.json"), r#"[["shape.add_area", {"name": "Lawn", "outline": [[0,0],[1,0],[1,1]]}], ["light.add", {"name": "x"}]]"#).unwrap();
    let out = cli(
        &["batch", "--script", "bad.json", "--out", "x.visioncraft"],
        &d,
    );
    assert_eq!(out.status.code(), Some(1));
    let report = &lines(&out)[0];
    assert_eq!(
        (report["completed"].as_u64(), report["rolledBack"].as_bool()),
        (Some(1), Some(true))
    );
    assert!(!d.join("x.visioncraft").exists());
    let usage = cli(&["render"], &d);
    assert_eq!(usage.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&usage.stderr).starts_with("visioncraft-cli: render: missing FILE")
    );
    assert_eq!(cli(&["dance"], &d).status.code(), Some(2));
    let fail = cli(
        &[
            "run",
            "--cmd",
            "shape.remove",
            "--params",
            r#"{"id": "S1"}"#,
        ],
        &d,
    );
    assert_eq!(fail.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&fail.stderr).trim(),
        "visioncraft-cli: shape.remove: no shape `S1`"
    );
    let listed = cli(&["commands", "--filter", "view.render"], &d);
    assert!(
        String::from_utf8_lossy(&listed.stdout)
            .starts_with("view.render\n  params:  {\"path\": string")
    );
    let new = cli(&["new", "empty.visioncraft"], &d);
    assert!(new.status.success());
    assert_eq!(
        cli(&["new", "empty.visioncraft"], &d).status.code(),
        Some(1)
    );
}

#[test]
fn serve_speaks_json_lines() {
    let d = dir("serve");
    let mut child = Command::new(env!("CARGO_BIN_EXE_visioncraft-cli"))
        .args(["serve", "--automation-write-root", "."])
        .current_dir(&d)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"id": 1, "method": "engine.execute", "params": {{"command": "light.add", "params": {{"name": "Lamp", "position": [0, 2, 0]}}}}}}"#).unwrap();
    writeln!(stdin, r#"{{"id": 2, "method": "engine.execute", "params": {{"command": "file.save", "params": {{"path": "s.visioncraft"}}}}}}"#).unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let r = lines(&out);
    assert_eq!(
        (r[0]["ok"].clone(), r[0]["result"].clone()),
        (json!(true), json!({"id": "li1"}))
    );
    assert_eq!(r[1]["ok"], true, "{}", r[1]);
    assert!(d.join("s.visioncraft").exists());
}

/// Needs a GPU: `cargo test -p visioncraft-cli -- --ignored`. Writes the picture to
/// `$VISIONCRAFT_RENDER_OUT` too when set, to look at.
#[test]
#[ignore]
fn render_draws_a_picture_that_is_not_blank() {
    let d = dir("render");
    assert!(
        cli(
            &["run", "--script", GARDEN, "--out", "garden.visioncraft"],
            &d
        )
        .status
        .success()
    );
    let out = cli(
        &[
            "render",
            "garden.visioncraft",
            "--out",
            "garden.png",
            "--size",
            "960x540",
            "--viewpoint",
            "From the gate",
        ],
        &d,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = &lines(&out)[0];
    assert_eq!(r["renderer"], "gpu");
    assert_eq!(r["plantsPending"], 0);
    let bytes = std::fs::read(d.join("garden.png")).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (960, 540));
    let colours: std::collections::HashSet<[u8; 3]> = buf[..info.buffer_size()]
        .chunks(4)
        .map(|p| [p[0] >> 3, p[1] >> 3, p[2] >> 3])
        .collect();
    assert!(colours.len() > 50, "only {} colours: blank", colours.len());
    if let Ok(target) = std::env::var("VISIONCRAFT_RENDER_OUT") {
        std::fs::write(target, &bytes).unwrap();
    }
}
