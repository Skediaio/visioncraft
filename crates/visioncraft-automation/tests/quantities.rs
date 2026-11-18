//! `query.quantities` through the host: linked models per body and the renderer's building rule.

use serde_json::{Value, json};
use std::path::PathBuf;
use visioncraft_automation::commands::execute;
use visioncraft_automation::{Access, Headless};

/// A cube of 2 m (`box`, closed) and a 2 m by 2 m sheet (`sheet`, open) beside it.
const OBJ: &str = "\
o box
v 0 0 0
v 2 0 0
v 2 2 0
v 0 2 0
v 0 0 2
v 2 0 2
v 2 2 2
v 0 2 2
f 1 2 6 5
f 4 8 7 3
f 1 4 3 2
f 5 6 7 8
f 1 5 8 4
f 2 3 7 6
o sheet
v 5 0 0
v 7 0 0
v 7 0 2
v 5 0 2
f 9 10 11 12
";

fn host(name: &str) -> Headless {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("quantities-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cube.obj"), OBJ).unwrap();
    Headless::new(Access::roots(Some(dir.clone()), Some(dir)))
}

fn run(h: &mut Headless, id: &str, params: Value) -> Value {
    execute(h, id, &params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn near(v: &Value, want: f64) {
    let got = v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"));
    assert!((got - want).abs() < 1e-4, "{got} != {want}");
}

fn body<'a>(model: &'a Value, id: &str) -> &'a Value {
    model["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == id)
        .unwrap()
}

fn add_model(h: &mut Headless) -> String {
    run(
        h,
        "model.add",
        json!({"name": "Cube", "kind": "obj", "source": "cube.obj"}),
    );
    let q = run(h, "query.quantities", json!({}));
    q["models"][0]["id"].as_str().unwrap().to_string()
}

#[test]
fn a_model_is_counted_per_body() {
    let mut h = host("bodies");
    add_model(&mut h);
    let q = run(&mut h, "query.quantities", json!({}));
    assert_eq!(q["counted_with"], "scene");
    let m = &q["models"][0];
    let cube = body(m, "box");
    near(&cube["surface_m2"], 24.0);
    near(&cube["volume_m3"], 8.0);
    let sheet = body(m, "sheet");
    near(&sheet["surface_m2"], 4.0);
    assert!(sheet["volume_m3"].is_null());
    near(&m["surface_m2"], 28.0);
    near(&m["volume_m3"], 8.0);
    assert_eq!(m["open_bodies"], 1);
    // folded into the totals
    near(&q["totals"]["surface_m2"], 28.0);
    near(&q["totals"]["volume_m3"], 8.0);
    near(&q["materials"][0]["area_m2"], 28.0);
}

#[test]
fn the_scale_of_the_transform_applies_squared_and_cubed() {
    let mut h = host("scale");
    let id = add_model(&mut h);
    run(
        &mut h,
        "model.set_transform",
        json!({"id": id, "scale": 2.0}),
    );
    let q = run(&mut h, "query.quantities", json!({}));
    let cube = body(&q["models"][0], "box");
    near(&cube["surface_m2"], 96.0);
    near(&cube["volume_m3"], 64.0);
    near(&q["totals"]["volume_m3"], 64.0);
}

#[test]
fn materials_follow_the_assignments() {
    let mut h = host("materials");
    let id = add_model(&mut h);
    run(
        &mut h,
        "material.assign",
        json!({"target": {"type": "owner", "owner": id}, "material": "brick"}),
    );
    run(
        &mut h,
        "material.assign",
        json!({"target": {"type": "body", "owner": id, "body": "sheet"}, "material": "glass"}),
    );
    let q = run(&mut h, "query.quantities", json!({}));
    let area = |rows: &Value, m: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["material"] == m)
            .map(|r| r["area_m2"].as_f64().unwrap())
    };
    assert_eq!(area(&q["materials"], "brick"), Some(24.0));
    assert_eq!(area(&q["materials"], "glass"), Some(4.0));
    let m = &q["models"][0];
    assert_eq!(area(&body(m, "box")["materials"], "brick"), Some(24.0));
    assert_eq!(area(&body(m, "sheet")["materials"], "glass"), Some(4.0));
}

#[test]
fn the_filters_pick_models() {
    let mut h = host("filters");
    let id = add_model(&mut h);
    run(
        &mut h,
        "shape.add_extrusion",
        json!({"name": "Wall", "outline": [[20, 0], [24, 0], [24, 4], [20, 4]], "height": 2}),
    );
    let all = run(&mut h, "query.quantities", json!({}));
    assert_eq!(all["shapes"].as_array().unwrap().len(), 1);
    let only = run(&mut h, "query.quantities", json!({"ids": [id]}));
    assert_eq!(only["shapes"].as_array().unwrap().len(), 0);
    assert_eq!(only["models"].as_array().unwrap().len(), 1);
    let model_layer = all["models"][0]["layer"].as_str().unwrap().to_string();
    let by_layer = run(&mut h, "query.quantities", json!({"layer": model_layer}));
    assert_eq!(by_layer["models"].as_array().unwrap().len(), 1);
    let other = run(&mut h, "query.quantities", json!({"ids": ["S1"]}));
    assert_eq!(other["models"].as_array().unwrap().len(), 0);
    let err = execute(&mut h, "query.quantities", &json!({"ids": ["nope"]})).unwrap_err();
    assert!(
        err.contains("query.quantities") && err.contains("nope"),
        "{err}"
    );
}

#[test]
fn plants_under_a_building_are_dropped_in_the_scene_but_not_on_the_plan() {
    let mut h = host("building");
    run(
        &mut h,
        "shape.add_extrusion",
        json!({"name": "House", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "height": 6}),
    );
    // Placed singly a plant is where it was put; an area scatter keeps off buildings.
    run(
        &mut h,
        "planting.add_area",
        json!({
            "name": "Trees", "asset_palette": [{"asset": "flora:acer-campestre", "weight": 1}],
            "outline": [[1, 1], [9, 1], [9, 7], [1, 7]], "density_per_m2": 0.5, "seed": 3,
        }),
    );
    let scene = run(&mut h, "query.quantities", json!({}));
    assert_eq!(scene["counted_with"], "scene");
    assert_eq!(scene["totals"]["plants"], 0);
    // The core alone has no building rule.
    let plan = h.session.run("query.quantities", json!({})).unwrap();
    assert_eq!(plan["counted_with"], "plan");
    assert!(plan["totals"]["plants"].as_u64().unwrap() > 0);
    assert!(plan.get("models").is_none());
}
