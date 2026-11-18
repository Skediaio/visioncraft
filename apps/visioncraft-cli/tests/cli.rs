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
        &[
            "batch",
            "--script",
            "bad.json",
            "--atomic",
            "--out",
            "x.visioncraft",
        ],
        &d,
    );
    assert_eq!(out.status.code(), Some(1));
    let report = &lines(&out)[0];
    assert_eq!(
        (report["completed"].as_u64(), report["rolledBack"].as_bool()),
        (Some(1), Some(true))
    );
    assert!(!d.join("x.visioncraft").exists());
    // As 3dcraft's batch: not atomic; --keep-going saves what was built.
    let kept = cli(
        &[
            "batch",
            "--script",
            "bad.json",
            "--keep-going",
            "--out",
            "y.visioncraft",
        ],
        &d,
    );
    assert_eq!(kept.status.code(), Some(1));
    let report = &lines(&kept)[0];
    assert_eq!(
        (report["completed"].as_u64(), report["failed"].as_u64()),
        (Some(1), Some(1))
    );
    assert!(report.get("rolledBack").is_none());
    assert!(d.join("y.visioncraft").exists());
    // run --new, and a step noted for an agent.
    std::fs::write(d.join("agent.json"), r#"[{"command": "time.set", "params": {"hour": 8}, "agent": "planner"}, ["agent.log", {}]]"#).unwrap();
    let ran = cli(&["run", "--new", "{}", "--script", "agent.json"], &d);
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let log = &lines(&ran)[1];
    assert_eq!(log["entries"][0]["client"], "planner");
    assert_eq!(log["entries"][0]["command"], "time.set");
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

/// A photo of a skewed facade on the south wall of a block, straightened by its four corners:
/// seen from the front its red corner is top left, green top right, blue bottom left and yellow
/// bottom right, its windows are dark blue and the wall between is beige (lit like any wall).
/// Needs a GPU: `cargo test -p visioncraft-cli -- --ignored`.
#[test]
#[ignore]
fn a_wall_photo_shows_straightened_and_the_right_way_up() {
    let d = dir("wall-photo");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/visioncraft-automation/tests/data/facade.png"),
        d.join("facade.png"),
    )
    .unwrap();
    // a 12 x 6 m wall facing +Z (south) at z = 9, side 2 of the outline
    let made = cli(
        &[
            "run",
            "--cmd",
            "shape.add_extrusion",
            "--params",
            r#"{"name": "Block", "outline": [[4, 2], [16, 2], [16, 9], [4, 9]], "height": 6}"#,
            "--cmd",
            "wall_photo.add",
            "--params",
            r#"{"file": "facade.png", "target": {"type": "face", "owner": "S1", "face": "S1:side(2)"},
                "corners_px": [[50, 350], [450, 325], [410, 75], [80, 50]]}"#,
            "--out",
            "wall.visioncraft",
        ],
        &d,
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let out = cli(
        &[
            "render",
            "wall.visioncraft",
            "--out",
            "wall.png",
            "--size",
            "800x450",
            "--camera",
            "10,3,22,10,3,9",
            "--fov",
            "40",
            "--time",
            "12:00",
        ],
        &d,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(d.join("wall.png")).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (800, 450));
    // The wall is 12 m wide at 13 m from the eye: at 40 degrees the picture holds 16.8 m across
    // and 9.5 m up, so the wall spans 572 x 286 pixels about the picture's centre.
    let at = |s: f32, t: f32| {
        let x = (400.0 + (s - 0.5) * 572.0) as usize;
        let y = (225.0 + (0.5 - t) * 286.0) as usize;
        let i = (y * 800 + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2]].map(i32::from)
    };
    let (red, green, blue, yellow) = (
        at(0.04, 0.93),
        at(0.96, 0.93),
        at(0.04, 0.07),
        at(0.96, 0.07),
    );
    let all = format!("{red:?} {green:?} {blue:?} {yellow:?}");
    assert!(red[0] > 2 * red[1] && red[0] > 2 * red[2], "red: {all}");
    assert!(
        green[1] > green[0] + 40 && green[1] > green[2] + 60,
        "green: {all}"
    );
    assert!(blue[2] > 2 * blue[0] && blue[2] > blue[1], "blue: {all}");
    // Yellow by its hue, with room for the sky's light on the wall (the photo's corner is
    // about 1:0.95:0.45; a clear sky over broken clouds lifts the blue a little).
    assert!(
        yellow[0] > 2 * yellow[2] && yellow[1] * 10 > 18 * yellow[2],
        "yellow: {all}"
    );
    // a window (dark blue) in the upper row and the beige wall beside it; the door below
    let window = at(0.5, 0.66);
    let wall = at(0.38, 0.66);
    assert!(window[2] > window[0] + 20, "window {window:?}");
    assert!(wall[0] > wall[2] + 20 && wall[0] > 100, "wall {wall:?}");
    if let Ok(target) = std::env::var("VISIONCRAFT_RENDER_OUT") {
        std::fs::write(target, &bytes).unwrap();
    }
}

/// The sun-hours map on a lawn with a 3 m wall along its south side, seen from straight above
/// (north up): the strip just north of the wall gets no sun (violet), the far north end most of
/// the day (amber). Needs a GPU: `cargo test -p visioncraft-cli -- --ignored`.
#[test]
#[ignore]
fn a_sun_map_is_drawn_on_the_lawn_with_shade_north_of_the_wall() {
    let d = dir("sun-map");
    let made = cli(
        &[
            "run",
            "--cmd",
            "shape.add_area",
            "--params",
            r#"{"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 10], [0, 10]], "material": "lawn"}"#,
            "--cmd",
            "shape.add_extrusion",
            "--params",
            r#"{"name": "Wall", "outline": [[0, 9.4], [10, 9.4], [10, 9.7], [0, 9.7]], "height": 3}"#,
            "--cmd",
            "view.set_sun_map",
            "--params",
            r#"{"shape": "S1", "season": "spring", "opacity": 1}"#,
            "--out",
            "map.visioncraft",
        ],
        &d,
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let out = cli(
        &[
            "render",
            "map.visioncraft",
            "--out",
            "map.png",
            "--size",
            "600x600",
            "--camera",
            "5,30,5.01,5,0,5",
            "--fov",
            "40",
        ],
        &d,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(d.join("map.png")).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (600, 600));
    // 30 m up at 40 degrees: 21.8 m over 600 px, 27.5 px a metre about the centre (5, 5).
    let at = |z: f32| {
        let y = (300.0 + (z - 5.0) * 27.5) as usize;
        let i = (y * 600 + 300) * 4;
        [buf[i], buf[i + 1], buf[i + 2]].map(i32::from)
    };
    let (shade, sun) = (at(8.8), at(1.0));
    assert!(
        shade[2] > shade[0] + 20 && shade[2] > shade[1],
        "shade {shade:?}"
    );
    assert!(sun[0] > sun[2] + 60 && sun[1] > sun[2] + 40, "sun {sun:?}");
    if let Ok(target) = std::env::var("VISIONCRAFT_RENDER_OUT") {
        std::fs::write(target, &bytes).unwrap();
    }
}

/// Every recipe of the agent guide through `visioncraft-cli mcp`, pictures included. Needs a
/// GPU: `cargo test -p visioncraft-cli -- --ignored`.
#[test]
#[ignore]
fn recipes_render_through_the_mcp_server() {
    use std::io::{BufRead, BufReader, Write};
    let guide = include_str!("../../../crates/visioncraft-automation/docs/agent.md");
    let recipes: Vec<Value> = guide
        .split("```json recipe\n")
        .skip(1)
        .map(|b| serde_json::from_str(&b[..b.find("```").unwrap()]).unwrap())
        .collect();
    for (n, recipe) in recipes.iter().enumerate() {
        let d = dir(&format!("recipe-{n}"));
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/visioncraft-core/tests/data/plate.3dcraft"
            ),
            d.join("plate.3dcraft"),
        )
        .unwrap();
        for image in ["site-plan.png", "facade.png"] {
            std::fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../crates/visioncraft-automation/tests/data")
                    .join(image),
                d.join(image),
            )
            .unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_visioncraft-cli"))
            .args([
                "mcp",
                "--automation-read-root",
                ".",
                "--automation-write-root",
                ".",
            ])
            .current_dir(&d)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "command_batch", "arguments": recipe}})).unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        drop(stdin);
        child.wait().unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            reply["result"]["isError"],
            false,
            "recipe {}: {line}",
            n + 1
        );
        for step in recipe["steps"].as_array().unwrap() {
            if let Some(path) = step["params"]["path"].as_str() {
                assert!(d.join(path).exists(), "recipe {}: {path}", n + 1);
                if let Ok(to) = std::env::var("VISIONCRAFT_RECIPES_OUT") {
                    let _ = std::fs::copy(
                        d.join(path),
                        Path::new(&to).join(format!("recipe{}-{path}", n + 1)),
                    );
                }
            }
        }
    }
}

/// A red 1 m panel as a GLB (two triangles in the XY plane, no textures).
fn red_panel_glb() -> Vec<u8> {
    let mut bin = Vec::new();
    for v in [
        [-0.5f32, 0.0, 0.0],
        [0.5, 0.0, 0.0],
        [0.5, 1.0, 0.0],
        [-0.5, 1.0, 0.0],
    ] {
        for c in v {
            bin.extend(c.to_le_bytes());
        }
    }
    for i in [0u32, 1, 2, 0, 2, 3] {
        bin.extend(i.to_le_bytes());
    }
    let j = json!({
        "asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
        "materials": [{"doubleSided": true, "pbrMetallicRoughness":
            {"baseColorFactor": [0.9, 0.02, 0.02, 1.0], "metallicFactor": 0.0}}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
             "min": [-0.5, 0.0, 0.0], "max": [0.5, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5125, "count": 6, "type": "SCALAR"}],
        "bufferViews": [{"buffer": 0, "byteLength": 48},
                        {"buffer": 0, "byteOffset": 48, "byteLength": 24}],
        "buffers": [{"byteLength": 72}]
    });
    let mut j = serde_json::to_vec(&j).unwrap();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let mut out = Vec::new();
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend(((12 + 8 + j.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend((j.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(&j);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(&bin);
    out
}

/// A small catalogue object at 2.4 times its size seen from 25 m, beyond its finer levels,
/// draws its model and not the stand-in box. Its planting is built before the catalogue has
/// arrived, and its model must still be asked for once it has.
#[test]
fn a_small_scaled_object_far_away_draws_its_model() {
    let d = dir("far-object");
    let lib = d.join("lib");
    std::fs::create_dir_all(lib.join("models/panel")).unwrap();
    let glb = red_panel_glb();
    for k in 0..3 {
        std::fs::write(lib.join(format!("models/panel/lod{k}.glb")), &glb).unwrap();
    }
    let lods: Vec<Value> = [0.27 * 12.0, 0.27 * 40.0, f32::MAX]
        .iter()
        .enumerate()
        .map(|(k, m)| json!({"file": format!("models/panel/lod{k}.glb"), "max_distance_m": m}))
        .collect();
    let catalog = json!({"assets": [{"id": "panel", "name": "Panel", "category": "pot",
        "tags": [], "license": "CC0-1.0", "origin": "BuiltIn", "author": "a", "source_url": "u",
        "size_m": [1.0, 1.0, 0.01], "lods": lods, "thumbnail": "t.png", "plant": null,
        "light": null}]});
    std::fs::write(lib.join("catalog.json"), catalog.to_string()).unwrap();
    let params =
        json!({"name": "Panel", "asset": "panel", "position": [0.0, 0.0, 0.0], "scale": 2.4});
    let made = cli(
        &[
            "--assets",
            "lib",
            "run",
            "--cmd",
            "planting.add_single",
            "--params",
            &params.to_string(),
            "--out",
            "p.visioncraft",
        ],
        &d,
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let out = cli(
        &[
            "render",
            "p.visioncraft",
            "--out",
            "p.png",
            "--size",
            "320x180",
            "--camera",
            "0,1.2,25,0,1.2,0",
            "--fov",
            "20",
            "--assets",
            "lib",
        ],
        &d,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(d.join("p.png")).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let red = buf[..info.buffer_size()]
        .chunks(4)
        .filter(|p| {
            let [r, g, b] = [p[0], p[1], p[2]].map(i32::from);
            r > 2 * g + 30 && r > 2 * b + 30
        })
        .count();
    // 2.4 m of a 8.8 m high field of view: about 2400 of the panel's pixels.
    assert!(red > 1000, "only {red} red pixels: the model is not drawn");
}

/// A film of three frames: they exist, are pictures, and differ as the camera moves.
/// Needs a GPU: `cargo test -p visioncraft-cli -- --ignored`.
#[test]
#[ignore]
fn a_film_renders_numbered_frames_that_differ() {
    let d = dir("film");
    let made = cli(
        &[
            "run",
            "--script",
            GARDEN,
            "--cmd",
            "film.add",
            "--params",
            r#"{"name": "Tiny", "fps": 2}"#,
            "--cmd",
            "film.add_shot",
            "--params",
            r#"{"id": "f1", "shot": {"kind": "orbit", "center": [5, 0, 5], "radius_m": 8, "height_m": 3,
                "from_deg": 0, "to_deg": 180, "duration_s": 1.5, "ease": "none", "time_of_day": [10, 18]}}"#,
            "--out",
            "garden.visioncraft",
        ],
        &d,
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let out = cli(
        &[
            "film",
            "garden.visioncraft",
            "--out",
            "frames",
            "--size",
            "96x54",
            "--quality",
            "low",
        ],
        &d,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = &lines(&out)[0];
    assert_eq!(r["frames"], 3);
    assert_eq!(r["framePattern"], "frame_%05d.png");
    let frames: Vec<Vec<u8>> = (0..3)
        .map(|i| std::fs::read(d.join(format!("frames/frame_{i:05}.png"))).unwrap())
        .collect();
    for f in &frames {
        assert!(f.starts_with(b"\x89PNG"));
    }
    assert!(frames[0] != frames[1] && frames[1] != frames[2] && frames[0] != frames[2]);
    // without ffmpeg, --video says so and keeps the frames; with it, the file is made
    let video = cli(
        &[
            "film",
            "garden.visioncraft",
            "--out",
            "frames",
            "--size",
            "96x54",
            "--every",
            "2",
            "--video",
            "film.mp4",
        ],
        &d,
    );
    let err = String::from_utf8_lossy(&video.stderr);
    if video.status.success() {
        assert!(d.join("film.mp4").exists());
    } else {
        assert!(err.contains("ffmpeg"), "{err}");
    }
}

/// The pixel of world point `p` in a `w` x `h` picture from `eye` looking at `target` with a
/// vertical field of view of `fov_deg`.
fn project(
    eye: [f32; 3],
    target: [f32; 3],
    fov_deg: f32,
    w: f32,
    h: f32,
    p: [f32; 3],
) -> (f32, f32) {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let norm = |a: [f32; 3]| {
        let l = dot(a, a).sqrt();
        a.map(|v| v / l)
    };
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let f = norm(sub(target, eye));
    let r = norm(cross(f, [0.0, 1.0, 0.0]));
    let u = cross(r, f);
    let d = sub(p, eye);
    let t = (fov_deg.to_radians() / 2.0).tan();
    let (x, y) = (
        dot(d, r) / (dot(d, f) * t * w / h),
        dot(d, u) / (dot(d, f) * t),
    );
    ((x * 0.5 + 0.5) * w, (0.5 - y * 0.5) * h)
}

/// A PNG's pixels (RGBA8) and width.
fn read_rgba(path: &Path) -> (Vec<u8>, usize) {
    let bytes = std::fs::read(path).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    (buf, info.width as usize)
}

/// The mean luminance (linear, Rec. 709) of the pixels within `r` of `(x, y)`.
fn luminance_at(px: &(Vec<u8>, usize), (x, y): (f32, f32), r: i32) -> f32 {
    let lin = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (buf, w) = px;
    let h = buf.len() / 4 / w;
    assert!(
        x as i32 - r >= 0
            && y as i32 - r >= 0
            && (x as i32 + r) < *w as i32
            && (y as i32 + r) < h as i32,
        "({x}, {y}) is off the picture"
    );
    let (mut sum, mut n) = (0.0, 0.0);
    for dy in -r..=r {
        for dx in -r..=r {
            let i = ((y as i32 + dy) as usize * w + (x as i32 + dx) as usize) * 4;
            sum += 0.2126 * lin(buf[i]) + 0.7152 * lin(buf[i + 1]) + 0.0722 * lin(buf[i + 2]);
            n += 1.0;
        }
    }
    sum / n
}

/// Sky occlusion on a tree and an L of garden wall (GPU: `--ignored`): just after sunset
/// the ground under the crown and in the wall's inner corner are darker than the open ground by
/// a plausible share of the sky they lose, and looking into a low sun through the crown, its
/// leaves are brighter with it on than off (the sky through the leaves, not black).
#[test]
#[ignore]
fn sky_occlusion_darkens_the_shade_and_lights_the_backlit_crown() {
    let d = dir("sky-occlusion");
    let made = cli(
        &[
            "run",
            "--cmd",
            "planting.add_single",
            "--params",
            r#"{"name": "Maple", "asset": "flora:acer-campestre", "position": [0, 0, 0]}"#,
            "--cmd",
            "shape.add_extrusion",
            "--params",
            r#"{"name": "Wall", "outline": [[-2, -8], [8.3, -8], [8.3, -2], [8, -2], [8, -7.7], [-2, -7.7]], "height": 2.5, "material": "plaster"}"#,
            "--out",
            "tree.visioncraft",
        ],
        &d,
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let render = |name: &str, eye: [f32; 3], target: [f32; 3], extra: &[&str]| {
        let camera = format!(
            "{},{},{},{},{},{}",
            eye[0], eye[1], eye[2], target[0], target[1], target[2]
        );
        let mut args = vec![
            "render",
            "tree.visioncraft",
            "--out",
            name,
            "--size",
            "640x360",
            "--camera",
            &camera,
            "--fov",
            "60",
        ];
        args.extend_from_slice(extra);
        let out = cli(&args, &d);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        read_rgba(&d.join(name))
    };
    // The shade: just after sunset, so the sky's light is all there is; seen from 6 m up on
    // the open lawn, under the crown (its lowest leaves about 2.5 m up) and into the corner.
    let (eye, target) = ([5.0, 6.0, 7.0], [4.0, 0.0, -4.0]);
    let dusk = ["--date", "2026-07-15", "--time", "21:50"];
    let grey = |on: &str, name: &str| {
        let mut extra = dusk.to_vec();
        extra.extend_from_slice(&["--sky-occlusion", on]);
        render(name, eye, target, &extra)
    };
    let (on, off) = (grey("on", "shade-on.png"), grey("off", "shade-off.png"));
    let at = |p: [f32; 3]| project(eye, target, 60.0, 640.0, 360.0, p);
    let (under, corner, open) = (
        at([1.5, 0.0, 1.5]),
        at([7.3, 0.0, -6.8]),
        at([7.5, 0.0, 2.0]),
    );
    let ratio = |px: &(Vec<u8>, usize), p| luminance_at(px, p, 3) / luminance_at(px, open, 3);
    let (under_on, under_off) = (ratio(&on, under), ratio(&off, under));
    let (corner_on, corner_off) = (ratio(&on, corner), ratio(&off, corner));
    // Under a field maple about half the sky is left; the inner corner of a 2.5 m wall about a
    // third to two thirds, the screen-space occlusion's share not counted twice.
    assert!(
        (0.3..0.85).contains(&under_on) && under_on < under_off,
        "under the crown {under_on} (off {under_off})"
    );
    assert!(
        (0.25..0.9).contains(&corner_on) && corner_on < corner_off,
        "in the corner {corner_on} (off {corner_off})"
    );
    // Into a low sun (an early September evening, from the west, 12 degrees up) through the
    // crown.
    let (eye, target) = ([14.0, 1.5, -0.9], [0.0, 5.5, 0.0]);
    assert!(
        cli(
            &[
                "run",
                "tree.visioncraft",
                "--cmd",
                "view.set",
                "--params",
                r#"{"clouds": {"kind": "clear", "cover": 0}}"#,
                "--out",
                "tree.visioncraft"
            ],
            &d
        )
        .status
        .success()
    );
    // A photo (eight samples): its sky light is gathered by bounce rays from each leaf's near
    // side, the light through the leaves from their far side only with sky occlusion.
    let low = [
        "--date",
        "2026-09-05",
        "--time",
        "19:00",
        "--quality",
        "photo",
        "--samples",
        "8",
    ];
    let backlit = |on: &str, name: &str| {
        let mut extra = low.to_vec();
        extra.extend_from_slice(&["--sky-occlusion", on]);
        render(name, eye, target, &extra)
    };
    let (on, off) = (
        backlit("on", "backlit-on.png"),
        backlit("off", "backlit-off.png"),
    );
    // The crown's upper half, off the sun's disc behind its lower middle.
    let crown = project(eye, target, 60.0, 640.0, 360.0, [1.0, 7.0, 1.0]);
    let (lit_on, lit_off) = (luminance_at(&on, crown, 12), luminance_at(&off, crown, 12));
    assert!(
        lit_on > lit_off * 1.05,
        "the backlit crown: {lit_on} on, {lit_off} off"
    );
}
