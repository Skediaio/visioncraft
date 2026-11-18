//! Plants and the light a spot gets: `query.plants_for_light`, `query.light_check` and
//! `planting.add_border` with `style: auto`, with the shadow of a house and two walls.

use serde_json::{Value, json};
use visioncraft_automation::{Access, Headless, rpc};

fn execute(host: &mut Headless, command: &str, params: Value) -> Value {
    let request = json!({"id": 1, "method": "engine.execute", "params": {"command": command, "params": params}});
    let line = rpc::handle_line(host, &request.to_string()).expect("a reply");
    serde_json::from_str(&line).expect("JSON")
}

fn ok(host: &mut Headless, command: &str, params: Value) -> Value {
    let r = execute(host, command, params);
    assert_eq!(r["ok"], json!(true), "{command}: {r}");
    r["result"].clone()
}

/// A house block with a courtyard on its north side (-Z) between two tall walls: no direct sun
/// there on the summer solstice. Open lawn lies far to the south.
fn courtyard() -> Headless {
    let mut h = Headless::new(Access::default());
    ok(
        &mut h,
        "shape.add_area",
        json!({"name": "Lawn", "outline": [[-30, -30], [30, -30], [30, 40], [-30, 40]], "material": "lawn"}),
    );
    for (name, outline, height) in [
        ("House", json!([[-10, -2], [10, -2], [10, 2], [-10, 2]]), 12),
        (
            "East wall",
            json!([[4, -14], [6, -14], [6, -2], [4, -2]]),
            12,
        ),
        (
            "West wall",
            json!([[-6, -14], [-4, -14], [-4, -2], [-6, -2]]),
            12,
        ),
    ] {
        ok(
            &mut h,
            "shape.add_extrusion",
            json!({"name": name, "outline": outline, "height": height}),
        );
    }
    h
}

const NORTH: [[i32; 2]; 4] = [[-3, -9], [3, -9], [3, -4], [-3, -4]];

#[test]
fn a_lavender_in_the_courtyard_is_too_shady_and_one_on_the_open_lawn_is_fine() {
    let mut h = courtyard();
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Shaded lavender", "asset": "flora:lavandula-angustifolia", "position": [0, 0, -6]}),
    );
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Sunny lavender", "asset": "flora:lavandula-angustifolia", "position": [0, 0, 20]}),
    );
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Shaded hosta", "asset": "flora:hosta", "position": [1, 0, -6]}),
    );
    let r = ok(&mut h, "query.light_check", json!({"include_ok": true}));
    assert_eq!(r["summary"]["checked"], 3, "{r}");
    assert_eq!(r["summary"]["ok"], 2, "{r}");
    assert_eq!(r["summary"]["too_shady"], 1, "{r}");
    assert_eq!(r["summary"]["too_sunny"], 0, "{r}");
    let plants = r["plants"].as_array().unwrap();
    let shaded = plants
        .iter()
        .find(|p| p["planting"] == "g1")
        .or_else(|| plants.iter().find(|p| p["verdict"] == "too_shady"))
        .unwrap();
    assert_eq!(shaded["species"], "flora:lavandula-angustifolia");
    assert!(shaded["hours"].as_f64().unwrap() < 3.0, "{shaded}");
    assert_eq!(
        shaded["light"],
        json!({"min_hours": 6.0, "max_hours": 24.0})
    );
    // Swaps: shrubs that thrive in that much shade.
    let s = shaded["suggestions"].as_array().unwrap();
    assert!((2..=3).contains(&s.len()), "{shaded}");
    assert!(s.iter().all(|x| x["fit"] == "full"));
    let open = plants.iter().find(|p| p["position"][1] == 20.0).unwrap();
    assert_eq!(open["verdict"], "ok");
    assert!(open["hours"].as_f64().unwrap() > 12.0, "{open}");
    assert!(r["note"].as_str().unwrap().contains("estimated"));
}

#[test]
fn a_planting_is_not_shaded_by_its_own_crown() {
    let mut h = Headless::new(Access::default());
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [0, 0, 0]}),
    );
    let r = ok(
        &mut h,
        "query.light_check",
        json!({"years": 60, "include_ok": true}),
    );
    assert!(r["crowns"].as_u64().unwrap() >= 1, "{r}");
    assert_eq!(r["summary"]["ok"], 1, "{r}");
    assert!(r["plants"][0]["hours"].as_f64().unwrap() > 12.0, "{r}");
    // Another planting's crown does shade it.
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Lavender", "asset": "flora:lavandula-angustifolia", "position": [0, 0, 0.5]}),
    );
    let r = ok(
        &mut h,
        "query.light_check",
        json!({"years": 60, "include_ok": true, "ids": ["g2"]}),
    );
    assert_eq!(r["summary"]["checked"], 1, "{r}");
    // The oak's crown takes the middle of the day from it (16.5 h of daylight).
    assert!(r["plants"][0]["hours"].as_f64().unwrap() < 12.0, "{r}");
}

#[test]
fn plants_for_light_on_the_north_side_ranks_shade_plants_first() {
    let mut h = courtyard();
    let r = ok(
        &mut h,
        "query.plants_for_light",
        json!({"outline": NORTH, "cell_m": 1}),
    );
    assert!(r["spot"]["mean"].as_f64().unwrap() < 3.0, "{r}");
    assert_eq!(r["spot"]["date"], json!({"month": 6, "day": 21}));
    let list = r["species"].as_array().unwrap();
    assert_eq!(list[0]["fit"], "full");
    assert!(
        list[0]["light"]["max_hours"].as_f64().unwrap() <= 6.0,
        "{r}"
    );
    assert!(list[0]["name"].is_string());
    assert!(
        list.iter().take(5).all(|e| e["light"]["min_hours"] == 0.0),
        "{r}"
    );
    // Agrees with `query.sun_hours` on the same day.
    let s = ok(
        &mut h,
        "query.sun_hours",
        json!({"outline": NORTH, "season": "summer", "cell_m": 1}),
    );
    assert_eq!(r["spot"]["mean"], s["mean"]);
    // A grower filter.
    let trees = ok(
        &mut h,
        "query.plants_for_light",
        json!({"point": [0, -6], "grower": "tree", "limit": 3}),
    );
    assert!(
        trees["species"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["grower"] == "tree")
    );
    // The open lawn ranks sun lovers first.
    let sunny = ok(&mut h, "query.plants_for_light", json!({"point": [0, 20]}));
    assert!(
        sunny["species"][0]["light"]["min_hours"].as_f64().unwrap() >= 3.0,
        "{sunny}"
    );
}

#[test]
fn plants_for_light_runs_without_geometry_on_hours_and_refuses_the_rest() {
    let mut h = Headless::new(Access::default());
    let r = ok(&mut h, "query.plants_for_light", json!({"hours": 8}));
    assert_eq!(r["hours"], 8.0);
    assert_eq!(r["species"][0]["fit"], "full");
    let both = execute(
        &mut h,
        "query.plants_for_light",
        json!({"hours": 8, "point": [0, 0]}),
    );
    assert_eq!(both["ok"], json!(false), "{both}");
    let none = execute(&mut h, "query.plants_for_light", json!({}));
    assert_eq!(none["ok"], json!(false), "{none}");
    let bad = execute(
        &mut h,
        "query.plants_for_light",
        json!({"hours": 8, "grower": "fern"}),
    );
    assert_eq!(bad["ok"], json!(false), "{bad}");
}

#[test]
fn an_auto_border_picks_shade_in_the_courtyard_and_a_sun_style_there_is_warned() {
    let mut h = courtyard();
    let outline: Vec<Value> = NORTH.iter().map(|p| json!(p)).collect();
    let r = ok(
        &mut h,
        "planting.add_border",
        json!({"outline": outline, "style": "auto", "facing": [0, 1]}),
    );
    assert_eq!(r["style"], "shade", "{r}");
    assert!(r["style_reason"].as_str().unwrap().contains("auto"));
    assert!(r["warnings"].as_array().unwrap().is_empty(), "{r}");
    let g = h.session.project().generators.last().unwrap().clone();
    let border = match g {
        visioncraft_core::Generator::AreaScatter(a) => a.border.unwrap(),
        _ => panic!("an area scatter"),
    };
    assert_eq!(border.style, "shade");
    let warned = ok(
        &mut h,
        "planting.add_border",
        json!({"outline": outline, "style": "cottage", "facing": [0, 1]}),
    );
    assert!(
        warned["warnings"][0]
            .as_str()
            .unwrap()
            .starts_with("light_fit"),
        "{warned}"
    );
    // In the open: auto picks a sunny style, dry on request.
    let open = json!([[-3, 12], [3, 12], [3, 17], [-3, 17]]);
    let p = ok(
        &mut h,
        "planting.add_border",
        json!({"outline": open, "style": "auto", "facing": [0, 1]}),
    );
    assert_eq!(p["style"], "prairie", "{p}");
    let d = ok(
        &mut h,
        "planting.add_border",
        json!({"outline": open, "style": "auto", "dry": true, "facing": [0, 1]}),
    );
    assert_eq!(d["style"], "dry", "{d}");
    let shade_in_sun = ok(
        &mut h,
        "planting.add_border",
        json!({"outline": open, "style": "shade", "facing": [0, 1]}),
    );
    assert!(
        shade_in_sun["warnings"][0]
            .as_str()
            .unwrap()
            .contains("light_fit")
    );
}

#[test]
fn auto_needs_an_outline_or_area_and_a_core_session_refuses_it() {
    let mut h = Headless::new(Access::default());
    let r = execute(&mut h, "planting.add_border", json!({"style": "auto"}));
    assert_eq!(r["ok"], json!(false), "{r}");
    let mut s = visioncraft_core::Session::new(visioncraft_core::Project::default());
    let e = s
        .run(
            "planting.add_border",
            json!({"outline": [[0, 0], [4, 0], [4, 2], [0, 2]], "style": "auto"}),
        )
        .unwrap_err();
    assert!(e.to_string().contains("needs a host"), "{e}");
}
