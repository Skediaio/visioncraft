//! `query.bloom_calendar` through the host: the bloom months, flower colours and names of the
//! species library, a custom species' from the parameters the project stores for it, and the
//! plants counted as the renderer draws them.

use serde_json::{Value, json};
use visioncraft_automation::{Access, Headless, Host, rpc};
use visioncraft_flora::Species;

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

fn fails(host: &mut Headless, command: &str, params: Value) -> String {
    let r = execute(host, command, params);
    assert_eq!(r["ok"], json!(false), "{command}: {r}");
    r["error"].as_str().unwrap().to_string()
}

fn months(v: &Value) -> Vec<u64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_u64().unwrap())
        .collect()
}

fn ids(v: &Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| e["species"].as_str().unwrap())
        .collect()
}

fn shift(m: u64) -> u64 {
    (m + 5) % 12 + 1
}

/// A cottage border, and the two lamps a path lighting places beside it.
fn garden() -> (Headless, String) {
    let mut h = Headless::new(Access::default());
    let border = ok(
        &mut h,
        "planting.add_border",
        json!({"name": "Cottage border", "outline": [[0, 0], [10, 0], [10, 2], [0, 2]], "style": "cottage", "facing": [0, 1], "seed": 7}),
    );
    let id = border["id"].as_str().unwrap().to_string();
    ok(
        &mut h,
        "planting.add_spline",
        json!({"name": "Path lights", "asset_palette": [{"asset": "security_light", "weight": 1}], "points": [[0, 4], [6, 4]], "spacing": {"count": 2}, "seed": 1}),
    );
    (h, id)
}

/// Every check on a calendar of built-in species only: the layout, and each species in exactly
/// the months the library gives it (six months on south of the equator). Returns the plants
/// counted, in flower or unknown.
fn check_library(out: &Value, south: bool) -> u64 {
    assert_eq!(out["hemisphere"], if south { "south" } else { "north" });
    let calendar = out["months"].as_array().unwrap();
    assert_eq!(calendar.len(), 12);
    let mut in_flower: std::collections::BTreeMap<String, Vec<u64>> = Default::default();
    for (i, m) in calendar.iter().enumerate() {
        assert_eq!(m["month"], (i + 1) as u64);
        let flowering = m["flowering"].as_array().unwrap();
        let sum: u64 = flowering
            .iter()
            .map(|f| f["plants"].as_u64().unwrap())
            .sum();
        assert_eq!(m["plants"], sum, "{m}");
        assert_eq!(m["species"], flowering.len() as u64);
        let mut sorted: Vec<(u64, &str)> = flowering
            .iter()
            .map(|f| {
                (
                    f["plants"].as_u64().unwrap(),
                    f["species"].as_str().unwrap(),
                )
            })
            .collect();
        sorted.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        let got: Vec<(u64, &str)> = flowering
            .iter()
            .map(|f| {
                (
                    f["plants"].as_u64().unwrap(),
                    f["species"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(got, sorted, "{m}");
        for f in flowering {
            assert!(f["name"].is_string(), "{f}");
            in_flower
                .entry(f["species"].as_str().unwrap().to_string())
                .or_default()
                .push(m["month"].as_u64().unwrap());
        }
    }
    let species = out["species"].as_array().unwrap();
    assert!(!species.is_empty());
    let mut listed = 0;
    for sp in species {
        let asset = sp["species"].as_str().unwrap();
        let s = Species::by_id(asset.strip_prefix("flora:").unwrap()).unwrap();
        // Months that cross the new year come back in calendar order (December before
        // January), as sorted numbers here.
        let mut want: Vec<u64> = s
            .bloom_months
            .iter()
            .map(|&m| {
                if south {
                    shift(u64::from(m))
                } else {
                    u64::from(m)
                }
            })
            .collect();
        want.sort_unstable();
        let mut got = in_flower.remove(asset).unwrap_or_default();
        got.sort_unstable();
        assert_eq!(got, want, "{asset}");
        assert_eq!(months(&sp["bloom_months"]), want, "{asset}");
        assert_eq!(sp["name"], s.common_name, "{asset}");
        // The colour is the flower colour the generator draws, or the key is left out for a
        // species without flowers.
        match s.params.flowers.as_ref() {
            Some(f) => assert_eq!(sp["colour"], json!(f.colour), "{asset}"),
            None => assert!(sp.get("colour").is_none(), "{asset}"),
        }
        listed += sp["plants"].as_u64().unwrap();
    }
    assert!(in_flower.is_empty());
    let gaps: Vec<u64> = calendar
        .iter()
        .filter(|m| m["flowering"].as_array().unwrap().is_empty())
        .map(|m| m["month"].as_u64().unwrap())
        .collect();
    assert_eq!(months(&out["gaps"]), gaps);
    // What the quantities count as plants is in flower or unknown; lamps are not plants.
    listed
        + out["unknown"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["plants"].as_u64().unwrap())
            .sum::<u64>()
}

#[test]
fn a_cottage_border_flowers_as_the_library_says() {
    let (mut h, id) = garden();
    let c = ok(&mut h, "query.bloom_calendar", json!({}));
    let plants = check_library(&c, false);
    let q = ok(&mut h, "query.quantities", json!({"ids": [id]}));
    assert_eq!(plants, q["totals"]["plants"].as_u64().unwrap(), "{c}");
    // The cottage border flowers from May to October: winter is a gap, summer is full, and
    // every species of the style has flowers.
    assert_eq!(months(&c["gaps"]), [1, 2, 3, 4, 11, 12]);
    assert!(c["unknown"].as_array().unwrap().is_empty(), "{c}");
    assert!(
        c["months"][5]["species"].as_u64().unwrap() >= 5,
        "{}",
        c["months"][5]
    );
    // Lavender: June to August, violet.
    let lavender = c["species"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["species"] == "flora:lavandula-angustifolia")
        .expect("lavender in a cottage border");
    assert_eq!(lavender["name"], "English lavender");
    assert_eq!(months(&lavender["bloom_months"]), [6, 7, 8]);
    assert_eq!(
        lavender["colour"],
        json!(
            Species::by_id("lavandula-angustifolia")
                .unwrap()
                .params
                .flowers
                .as_ref()
                .unwrap()
                .colour
        )
    );
    // The description a host gives says where it takes the bloom months from; the core's own
    // still says it knows none.
    let docs = visioncraft_automation::commands::docs(h.session());
    let doc = &docs
        .iter()
        .find(|d| d.id == "query.bloom_calendar")
        .unwrap();
    assert!(doc.summary.contains("species library"), "{}", doc.summary);
    assert!(!doc.undoable);
    let summary = h
        .session()
        .command_spec("query.bloom_calendar")
        .unwrap()
        .summary
        .clone();
    assert!(summary.contains("Run by the core alone"));
}

#[test]
fn a_custom_species_flowers_as_its_parameters_say() {
    let (mut h, _) = garden();
    ok(
        &mut h,
        "species.add",
        json!({"name": "Red geranium", "base": "geranium-rozanne", "params": {"height_m": 0.4, "bloom_months": [3, 4], "flowers": {"colour": [1.0, 0.0, 0.0]}}}),
    );
    ok(
        &mut h,
        "species.add",
        json!({"name": "Leafy cranesbill", "base": "geranium-rozanne", "params": {"height_m": 0.5}}),
    );
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Red", "asset": "sp1", "position": [6, 0, 8]}),
    );
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Spurge", "asset": "sp2", "position": [7, 0, 8]}),
    );
    let c = ok(&mut h, "query.bloom_calendar", json!({}));
    let row = |id: &str| {
        c["species"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["species"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        (
            row("sp1")["name"].clone(),
            months(&row("sp1")["bloom_months"]),
            row("sp1")["colour"].clone(),
            row("sp1")["plants"].clone()
        ),
        (
            json!("Red geranium"),
            vec![3, 4],
            json!([1.0, 0.0, 0.0]),
            json!(1)
        )
    );
    assert_eq!(c["months"][2]["flowering"][0]["species"], "sp1");
    assert_eq!(c["months"][2]["flowering"][0]["name"], "Red geranium");
    // A custom species without bloom months of its own is unknown: it does not inherit the
    // months of the species it derives from (nor does the plant the app draws).
    assert_eq!(
        c["unknown"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["species"] == "sp2")
            .map(|u| u["plants"].clone()),
        Some(json!(1))
    );
    assert!(row("sp2").is_null());
}

#[test]
fn south_of_the_equator_the_months_shift_six() {
    let (mut h, _) = garden();
    let north = ok(&mut h, "query.bloom_calendar", json!({}));
    ok(&mut h, "site.set", json!({"latitude": -33.9}));
    let south = ok(&mut h, "query.bloom_calendar", json!({}));
    check_library(&south, true);
    let lavender = |c: &Value| {
        c["species"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["species"] == "flora:lavandula-angustifolia")
            .unwrap()["bloom_months"]
            .clone()
    };
    assert_eq!(months(&lavender(&north)), [6, 7, 8]);
    // June to August become December, January and February: listed by month number, as the
    // library lists them.
    assert_eq!(months(&lavender(&south)), [1, 2, 12]);
    assert_eq!(
        ids(&north["months"][5]["flowering"]),
        ids(&south["months"][11]["flowering"])
    );
    assert_eq!(months(&south["gaps"]), [5, 6, 7, 8, 9, 10]);
}

#[test]
fn planting_and_layer_filter_and_the_errors() {
    let (mut h, id) = garden();
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [5, 0, 8]}),
    );
    let all = ok(&mut h, "query.bloom_calendar", json!({}));
    // The library gives the oak no bloom months: it is a species without bloom data.
    assert!(ids(&all["unknown"]).contains(&"flora:quercus-robur"));
    assert!(!ids(&all["species"]).contains(&"flora:quercus-robur"));
    let one = ok(&mut h, "query.bloom_calendar", json!({"planting": id}));
    assert!(one["unknown"].as_array().unwrap().is_empty(), "{one}");
    let q = ok(&mut h, "query.quantities", json!({"ids": [id.clone()]}));
    let counted: u64 = one["species"]
        .as_array()
        .unwrap()
        .iter()
        .chain(one["unknown"].as_array().unwrap())
        .map(|s| s["plants"].as_u64().unwrap())
        .sum();
    assert_eq!(counted, q["totals"]["plants"].as_u64().unwrap());
    // Every plant of the design: the border's and the oak's, as the quantities count them.
    let q_all = ok(&mut h, "query.quantities", json!({}));
    let every: u64 = all["species"]
        .as_array()
        .unwrap()
        .iter()
        .chain(all["unknown"].as_array().unwrap())
        .map(|s| s["plants"].as_u64().unwrap())
        .sum();
    assert_eq!(every, q_all["totals"]["plants"].as_u64().unwrap());
    assert_eq!(every, counted + 1);
    let lights = ok(&mut h, "layer.add", json!({"name": "Far bed"}));
    let far = lights["id"].as_str().unwrap().to_string();
    ok(
        &mut h,
        "planting.add_single",
        json!({"name": "Lavender", "asset": "flora:lavandula-angustifolia", "position": [20, 0, 20], "layer": far}),
    );
    let on_far = ok(&mut h, "query.bloom_calendar", json!({"layer": far}));
    assert_eq!(ids(&on_far["species"]), ["flora:lavandula-angustifolia"]);
    assert_eq!(months(&on_far["gaps"]), [1, 2, 3, 4, 5, 9, 10, 11, 12]);
    assert!(on_far["unknown"].as_array().unwrap().is_empty());
    // No lamps anywhere.
    assert!(!all.to_string().contains("security_light"));
    // Errors name the command and what is wrong.
    assert_eq!(
        fails(&mut h, "query.bloom_calendar", json!({"planting": "g9"})),
        "query.bloom_calendar: no planting `g9`"
    );
    let err = fails(&mut h, "query.bloom_calendar", json!({"layer": "nope"}));
    assert_eq!(err, "query.bloom_calendar: no layer `nope`");
    let err = fails(&mut h, "query.bloom_calendar", json!({"month": 6}));
    assert!(
        err.starts_with("query.bloom_calendar: unknown parameter `month`"),
        "{err}"
    );
    let err = fails(&mut h, "query.bloom_calendar", json!({"planting": 6}));
    assert!(err.starts_with("query.bloom_calendar: `planting`"), "{err}");
    // A query is not undoable.
    let state = ok(&mut h, "edit.state", json!({}));
    assert_eq!(state["can_undo"], json!(true));
    ok(&mut h, "query.bloom_calendar", json!({}));
    assert_eq!(
        ok(&mut h, "edit.state", json!({}))["revision"],
        state["revision"]
    );
}
