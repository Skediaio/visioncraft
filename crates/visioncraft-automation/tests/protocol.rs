//! Golden tests of the JSON-lines protocol: a request line in, the reply line out (the
//! wall-clock `ms` fields taken out, as they differ from run to run).

use serde_json::{Value, json};
use std::path::PathBuf;
use visioncraft_automation::{Access, Headless, rpc};

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("protocol-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn strip_ms(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.remove("ms");
            m.values_mut().for_each(strip_ms);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_ms),
        _ => {}
    }
}

fn ask(host: &mut Headless, request: Value) -> Value {
    let line = rpc::handle_line(host, &request.to_string()).expect("a reply");
    let mut v: Value = serde_json::from_str(&line).expect("JSON");
    strip_ms(&mut v);
    v
}

fn execute(host: &mut Headless, id: u64, command: &str, params: Value) -> Value {
    ask(
        host,
        json!({"id": id, "method": "engine.execute", "params": {"command": command, "params": params}}),
    )
}

#[test]
fn a_command_that_succeeds() {
    let mut h = Headless::new(Access::default());
    assert_eq!(
        execute(&mut h, 1, "time.set", json!({"hour": 20.5})),
        json!({"id": 1, "ok": true, "result": null})
    );
    assert_eq!(
        execute(
            &mut h,
            2,
            "shape.add_area",
            json!({"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]]})
        ),
        json!({"id": 2, "ok": true, "result": {"id": "S1"}})
    );
    assert_eq!(
        execute(
            &mut h,
            3,
            "query.measure",
            json!({"kind": "area", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]]})
        ),
        json!({"id": 3, "ok": true, "result": {"kind": "area", "value": 80.0, "unit": "m2"}})
    );
}

#[test]
fn an_unknown_command_names_the_nearest() {
    let mut h = Headless::new(Access::default());
    assert_eq!(
        execute(&mut h, 1, "time.sett", json!({})),
        json!({"id": 1, "ok": false, "error": "unknown command `time.sett`; did you mean `time.set`?"})
    );
    assert_eq!(
        ask(&mut h, json!({"id": 2, "method": "engine.run"})),
        json!({"id": 2, "ok": false, "error": "unknown method `engine.run`; the methods are: engine.execute, engine.commands, batch, methods"})
    );
}

#[test]
fn bad_parameters_are_refused_and_named() {
    let mut h = Headless::new(Access::default());
    assert_eq!(
        execute(&mut h, 1, "shape.add_area", json!({"name": "Lawn"})),
        json!({"id": 1, "ok": false, "error": "shape.add_area: bad parameters: `shape.add_area` needs the parameter `outline` ([[x, z], ...])"})
    );
    // A misspelt key is refused, not ignored.
    assert_eq!(
        execute(
            &mut h,
            2,
            "light.add",
            json!({"name": "Lamp", "position": [0, 2, 0], "lumen": 900})
        ),
        json!({"id": 2, "ok": false, "error": "light.add: bad parameters: unknown parameter `lumen` for `light.add`; did you mean `lumens`?"})
    );
    assert_eq!(
        execute(&mut h, 3, "document.inspect", json!({"face": false})),
        json!({"id": 3, "ok": false, "error": "document.inspect: unknown parameter `face`; its parameters are: faces, measures, placements, full"})
    );
    assert_eq!(
        ask(
            &mut h,
            json!({"id": 4, "method": "engine.execute", "params": {"command": "time.set", "params": [1]}})
        ),
        json!({"id": 4, "ok": false, "error": "engine.execute: `params` must be a JSON object, got an array"})
    );
    assert_eq!(
        ask(
            &mut h,
            json!({"id": 5, "method": "engine.execute", "extra": 1})
        ),
        json!({"id": 5, "ok": false, "error": "unknown request field `extra`; a request has id, method and params"})
    );
    let line = rpc::handle_line(&mut h, "{not json").unwrap();
    assert!(
        line.starts_with(r#"{"error":"the request is not valid JSON"#)
            && line.contains(r#""id":null"#),
        "{line}"
    );
    // Nothing above changed the project.
    let inspect = execute(&mut h, 6, "document.inspect", json!({}));
    assert_eq!(inspect["result"]["shapes"], json!([]));
    assert_eq!(
        inspect["result"]["history"],
        json!({"can_undo": false, "can_redo": false})
    );
}

#[test]
fn a_batch_runs_as_in_3dcraft() {
    let mut h = Headless::new(Access::default());
    let steps = json!([
        {"command": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]]}},
        {"command": "light.add", "params": {"name": "Lamp", "position": [0, 2, 0], "layer": "nope"}},
        {"command": "time.set", "params": {"hour": 12}}
    ]);
    // Stops at the first error; what ran stays applied, each its own undo step.
    assert_eq!(
        ask(
            &mut h,
            json!({"id": 1, "method": "batch", "params": {"steps": steps}})
        ),
        json!({"id": 1, "ok": true, "result": {
            "completed": 1, "failed": 1,
            "results": [
                {"ok": true, "result": {"id": "S1"}},
                {"ok": false, "error": "light.add: no layer `nope`"}
            ]
        }})
    );
    let after = execute(&mut h, 2, "document.inspect", json!({"faces": false}));
    assert_eq!(after["result"]["shapes"][0]["id"], json!("S1"));
    assert_eq!(after["result"]["time"]["hour"], json!(14.0));
    // stopOnError: false goes on; terse reports the errors and the last result.
    assert_eq!(
        ask(
            &mut h,
            json!({"id": 3, "method": "batch", "params": {"steps": steps, "stopOnError": false, "stopOnBreak": false, "terse": true}})
        ),
        json!({"id": 3, "ok": true, "result": {
            "completed": 2, "failed": 1,
            "errors": [{"step": 1, "error": "light.add: no layer `nope`"}],
            "last": {"step": 2, "result": null}
        }})
    );
    // Not a transaction: edit.undo takes back the last step only.
    execute(&mut h, 4, "edit.undo", json!({}));
    let undone = execute(&mut h, 5, "document.inspect", json!({"faces": false}));
    assert_eq!(undone["result"]["shapes"].as_array().unwrap().len(), 2);
    assert_eq!(undone["result"]["time"]["hour"], json!(14.0));
    // A malformed step refuses the whole batch before anything runs.
    assert_eq!(
        ask(
            &mut h,
            json!({"id": 6, "method": "batch", "params": {"steps": [{"command": "time.set"}, {"cmd": "x"}]}})
        ),
        json!({"id": 6, "ok": false, "error": "batch: steps[1]: unknown parameter `cmd`; its parameters are: command, params, intent"})
    );
}

#[test]
fn an_atomic_batch_is_all_or_nothing() {
    let mut h = Headless::new(Access::default());
    execute(&mut h, 1, "time.set", json!({"hour": 9}));
    let reply = ask(
        &mut h,
        json!({"id": 2, "method": "batch", "params": {"atomic": true, "stopOnError": false, "steps": [
            {"command": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]]}},
            {"command": "light.add", "params": {"name": "Lamp", "position": [0, 2, 0], "layer": "nope"}},
            {"command": "time.set", "params": {"hour": 12}}
        ]}}),
    );
    assert_eq!(
        reply,
        json!({"id": 2, "ok": true, "result": {
            "completed": 1, "failed": 1, "rolledBack": true,
            "results": [
                {"ok": true, "result": {"id": "S1"}},
                {"ok": false, "error": "light.add: no layer `nope`"}
            ]
        }})
    );
    let after = execute(&mut h, 3, "document.inspect", json!({"faces": false}));
    assert_eq!(after["result"]["shapes"], json!([]));
    assert_eq!(after["result"]["time"]["hour"], json!(9.0));
    assert_eq!(
        after["result"]["history"],
        json!({"can_undo": true, "can_redo": false})
    );
    // An atomic batch that succeeds is one undo step.
    let ok = ask(
        &mut h,
        json!({"id": 4, "method": "batch", "params": {"atomic": true, "terse": true, "steps": [
            {"command": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]]}},
            {"command": "shape.add_extrusion", "params": {"name": "House", "outline": [[0, -8], [6, -8], [6, -1], [0, -1]], "height": 6}}
        ]}}),
    );
    assert_eq!(
        ok,
        json!({"id": 4, "ok": true, "result": {"completed": 2, "failed": 0, "rolledBack": false, "errors": [], "last": {"step": 1, "result": {"id": "S2"}}}})
    );
    execute(&mut h, 5, "edit.undo", json!({}));
    let undone = execute(&mut h, 6, "document.inspect", json!({"faces": false}));
    assert_eq!(undone["result"]["shapes"], json!([]));
    assert_eq!(undone["result"]["time"]["hour"], json!(9.0));
}

#[test]
fn intents_go_to_the_agent_log() {
    let mut h = Headless::new(Access::default());
    let r = ask(
        &mut h,
        json!({"id": 1, "method": "engine.execute", "params": {"command": "light.add", "params": {"name": "Lamp", "position": [0, 2, 0]}, "intent": "  light the\n terrace\u{202E}  "}}),
    );
    assert_eq!(r["ok"], json!(true));
    ask(
        &mut h,
        json!({"id": 2, "method": "batch", "params": {"intent": "the lawn", "steps": [
            {"command": "shape.add_area", "params": {"name": "Lawn", "outline": [[0, 0], [1, 0], [1, 1]]}},
            {"command": "time.set", "params": {"hour": 99}, "intent": "a wrong hour"}
        ]}}),
    );
    execute(&mut h, 3, "document.inspect", json!({}));
    assert_eq!(
        ask(
            &mut h,
            json!({"id": 4, "method": "engine.execute", "params": {"command": "light.add", "intent": 5}})
        ),
        json!({"id": 4, "ok": false, "error": "engine.execute: `intent` must be a string, got a number"})
    );
    let mut log = execute(&mut h, 5, "agent.log", json!({"limit": 10}))["result"].clone();
    for e in log["entries"].as_array_mut().unwrap() {
        e.as_object_mut().unwrap().remove("timeMs");
    }
    assert_eq!(
        log,
        json!({"total": 3, "entries": [
            {"client": "agent", "command": "time.set", "label": "Set Time", "ok": false, "message": "time.set: bad parameters: not a valid date and local time", "created": [], "changed": [], "intent": "a wrong hour"},
            {"client": "agent", "command": "shape.add_area", "label": "Add Area", "ok": true, "message": "{\"id\":\"S1\"}", "created": ["S1"], "changed": [], "intent": "the lawn"},
            {"client": "agent", "command": "light.add", "label": "Add Light", "ok": true, "message": "{\"id\":\"li1\"}", "created": ["li1"], "changed": [], "intent": "light the terrace"}
        ]})
    );
}

#[test]
fn undo_and_redo() {
    let mut h = Headless::new(Access::default());
    assert_eq!(
        execute(&mut h, 1, "edit.undo", json!({})),
        json!({"id": 1, "ok": false, "error": "edit.undo: nothing to undo"})
    );
    execute(
        &mut h,
        2,
        "light.add",
        json!({"name": "Lamp", "position": [1, 2, 3]}),
    );
    assert_eq!(
        execute(&mut h, 3, "edit.undo", json!({})),
        json!({"id": 3, "ok": true, "result": {"can_undo": false, "can_redo": true}})
    );
    assert_eq!(
        execute(&mut h, 4, "document.inspect", json!({}))["result"]["lights"],
        json!([])
    );
    assert_eq!(
        execute(&mut h, 5, "edit.redo", json!({})),
        json!({"id": 5, "ok": true, "result": {"can_undo": true, "can_redo": false}})
    );
    let lights = &execute(&mut h, 6, "document.inspect", json!({}))["result"]["lights"];
    assert_eq!(
        (lights[0]["id"].as_str(), lights[0]["lumens"].as_f64()),
        (Some("li1"), Some(800.0))
    );
}

#[test]
fn listing_and_describing_commands() {
    let mut h = Headless::new(Access::default());
    let list = ask(
        &mut h,
        json!({"id": 1, "method": "engine.commands", "params": {"filter": "viewpoint", "brief": true}}),
    );
    let ids: Vec<&str> = list["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "viewpoint.add",
            "viewpoint.remove",
            "viewpoint.reorder",
            "viewpoint.update"
        ]
    );
    assert_eq!(
        list["result"]["commands"][0],
        json!({
            "id": "viewpoint.add",
            "params": "{\"name\": string, \"position\": [x, y, z] (m), \"target\": [x, y, z] (m), \"fov_y_deg\": number (deg) = 60, \"time\": object = not set, \"season\": string = not set, \"years\": number = not set}",
            "summary": "Saves a camera position and target, optionally with a time of day, a season and years after planting.",
            "enabled": true
        })
    );
    let one = ask(
        &mut h,
        json!({"id": 2, "method": "engine.commands", "params": {"id": "file.save"}}),
    );
    assert_eq!(
        one["result"]["commands"][0]["parameters"],
        json!([
            {"name": "path", "type": "string", "unit": null, "required": false, "default": "the open file's path", "allowed": [], "doc": "Where to write, beneath the write root"},
            {"name": "overwrite", "type": "boolean", "unit": null, "required": false, "default": "false", "allowed": [], "doc": "Replace a file that is not the open project's own"}
        ])
    );
    let undo = ask(
        &mut h,
        json!({"id": 3, "method": "engine.commands", "params": {"id": "edit.undo"}}),
    );
    assert_eq!(undo["result"]["commands"][0]["enabled"], json!(false));
    assert_eq!(
        undo["result"]["commands"][0]["disabledReason"],
        json!("nothing to undo")
    );
    let methods = ask(&mut h, json!({"id": 4, "method": "methods"}));
    assert_eq!(
        methods["result"]["limits"],
        json!({"requestBytes": 1048576, "replyBytes": 8388608, "batchSteps": 256})
    );
}

#[test]
fn every_documented_example_runs() {
    let mut h = Headless::new(Access::default());
    let list = ask(&mut h, json!({"id": 1, "method": "engine.commands"}));
    for c in list["result"]["commands"].as_array().unwrap() {
        let (id, example) = (c["id"].as_str().unwrap(), &c["example"]);
        // Files, history and pictures need a file, a step and a GPU; model.add a model file.
        if example.is_null()
            || id.starts_with("file.")
            || id.starts_with("edit.")
            || id == "view.render"
            || id == "model.add"
        {
            continue;
        }
        let mut h = Headless::new(Access::default());
        h.session = visioncraft_core::Session::new(visioncraft_core::example_project(id));
        let r = execute(&mut h, 2, id, example.clone());
        assert_eq!(r["ok"], json!(true), "{id}: {r}");
    }
}

#[test]
fn files_stay_beneath_the_roots_and_round_trip() {
    let root = dir("roots");
    let mut h = Headless::new(Access::roots(Some(root.clone()), Some(root.clone())));
    execute(
        &mut h,
        1,
        "viewpoint.add",
        json!({"name": "Gate", "position": [0, 1.7, 9], "target": [0, 1, 0]}),
    );
    assert_eq!(
        execute(
            &mut h,
            2,
            "file.save",
            json!({"path": "../out.visioncraft"})
        ),
        json!({"id": 2, "ok": false, "error": "file.save: `../out.visioncraft`: `..` is not allowed in a path"})
    );
    let outside = execute(
        &mut h,
        3,
        "file.save",
        json!({"path": "/etc/out.visioncraft"}),
    );
    assert!(
        outside["error"]
            .as_str()
            .unwrap()
            .contains("is outside the write root"),
        "{outside}"
    );
    let saved = execute(&mut h, 4, "file.save", json!({"path": "g.visioncraft"}));
    assert_eq!(saved["ok"], json!(true), "{saved}");
    execute(&mut h, 5, "file.new", json!({}));
    let opened = execute(&mut h, 6, "file.open", json!({"path": "g.visioncraft"}));
    assert_eq!(opened["result"]["viewpoints"], json!(1));
    let i = execute(&mut h, 7, "document.inspect", json!({}));
    assert_eq!(
        (
            i["result"]["dirty"].clone(),
            i["result"]["viewpoints"][0]["name"].clone()
        ),
        (json!(false), json!("Gate"))
    );
    let none = Headless::new(Access::default());
    let mut none = none;
    let refused = execute(&mut none, 8, "file.save", json!({"path": "x.visioncraft"}));
    assert_eq!(
        refused["error"],
        json!(
            "file.save: `x.visioncraft`: this server may not write files; start it with --automation-write-root DIR"
        )
    );
}

#[test]
fn the_garden_example_builds_and_is_inspected() {
    let mut h = Headless::new(Access::default());
    let steps: Vec<Value> =
        serde_json::from_str::<Vec<(String, Value)>>(include_str!("../docs/examples/garden.json"))
            .unwrap()
            .into_iter()
            .map(|(command, params)| json!({"command": command, "params": params}))
            .collect();
    let r = ask(
        &mut h,
        json!({"id": 1, "method": "batch", "params": {"steps": steps}}),
    );
    assert_eq!(r["result"]["failed"], json!(0), "{r}");
    let i = execute(&mut h, 2, "document.inspect", json!({}))["result"].clone();
    assert_eq!(i["shapes"][0]["area_m2"], json!(240.0));
    assert_eq!(i["shapes"][0]["faces"], json!(["S1:top"]));
    assert_eq!(i["shapes"][1]["footprint_m2"], json!(70.0));
    assert!(
        i["shapes"][1]["faces"]
            .as_array()
            .unwrap()
            .contains(&json!("S2:side(0)"))
    );
    // 20 m at one every 0.8 m.
    let hedge = &i["generators"][0];
    assert_eq!(
        (hedge["assets"].clone(), hedge["count"].as_u64()),
        (json!(["flora:carpinus-hedge"]), Some(26))
    );
    assert_eq!(i["lights"][0]["lumens"], json!(1200.0));
    assert_eq!(i["viewpoints"][0]["id"], json!("v1"));
    let hit = execute(
        &mut h,
        3,
        "query.raycast",
        json!({"origin": [0, 20, -6], "direction": [0, -1, 0]}),
    )["result"]
        .clone();
    assert_eq!(
        (hit["owner"].clone(), hit["face"].clone()),
        (json!("S2"), json!("S2:top"))
    );
    assert_eq!(hit["point"][1], json!(6.0));
    let ground =
        execute(&mut h, 4, "query.ground_height", json!({"x": 0, "z": 5}))["result"].clone();
    // The lawn lies a hair above 0, lifted as the window lifts areas (`AREA_LIFT_M`).
    let lift = f64::from(visioncraft_core::AREA_LIFT_M);
    assert!(
        (ground["height"].as_f64().unwrap() - lift).abs() < 1e-4,
        "{ground}"
    );
    let sun = execute(&mut h, 5, "query.sun", json!({"time": {"hour": 21.5}}))["result"].clone();
    assert!(
        sun["elevation_deg"].as_f64().unwrap() < 5.0 && sun["sunset_hour"].as_f64().unwrap() > 21.0,
        "{sun}"
    );
    let hedges =
        execute(&mut h, 6, "species.list", json!({"category": "hedge"}))["result"]["species"]
            .clone();
    assert!(
        hedges
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["asset"] == "flora:carpinus-hedge"),
        "{hedges}"
    );
}

#[test]
fn photo_parameters_are_checked_before_rendering() {
    let mut h = Headless::new(Access::default());
    let error = |h: &mut Headless, params: Value| {
        execute(h, 1, "view.render", params)["error"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        error(&mut h, json!({"samples": 8})),
        "view.render: `samples`, `aperture_m` and `focus_m` need quality `photo`"
    );
    assert!(error(&mut h, json!({"quality": "photo", "samples": 0})).contains("`samples`"));
    assert_eq!(
        error(&mut h, json!({"quality": "photo", "aperture_m": 2})),
        "view.render: `aperture_m` must be from 0 to 0.5"
    );
    // Valid: on to the renderer, which this host has not.
    assert!(
        error(
            &mut h,
            json!({"quality": "photo", "samples": 16, "aperture_m": 0.02})
        )
        .contains("no renderer")
    );
    // Snow lies only in winter: on its own it brings winter, with summer it is refused.
    assert!(error(&mut h, json!({"season": "summer", "snow": 0.5})).contains("only in winter"));
    assert!(error(&mut h, json!({"snow": 0.5})).contains("no renderer"));
    assert!(error(&mut h, json!({"season": "monsoon"})).contains("`season`"));
}

/// A saved example keeps its model as the program's own (`example:<file>`), so `render FILE`
/// and `file.open` load it as `run` does; files saved before, with the example's name made a
/// path in their folder, still open.
#[test]
fn a_saved_example_keeps_its_embedded_model() {
    let root = dir("example-save");
    let mut h = Headless::new(Access::roots(Some(root.clone()), Some(root.clone())));
    let r = execute(&mut h, 1, "project.load_example", json!({"name": "estate"}));
    assert_eq!(r["ok"], json!(true), "{r}");
    let saved = execute(&mut h, 2, "file.save", json!({"path": "e.visioncraft"}));
    assert_eq!(saved["ok"], json!(true), "{saved}");
    let text = std::fs::read_to_string(root.join("e.visioncraft")).unwrap();
    assert!(text.contains("\"example:villa.3dcraft\""));
    let opened = execute(&mut h, 3, "file.open", json!({"path": "e.visioncraft"}));
    assert_eq!(opened["result"]["missingModels"], json!([]), "{opened}");
    // The old form.
    let path = root.join("example:villa.3dcraft").display().to_string();
    let old = text.replace(
        "\"example:villa.3dcraft\"",
        &serde_json::to_string(&path).unwrap(),
    );
    std::fs::write(root.join("old.visioncraft"), old).unwrap();
    let opened = execute(&mut h, 4, "file.open", json!({"path": "old.visioncraft"}));
    assert_eq!(opened["result"]["missingModels"], json!([]), "{opened}");
}

#[test]
fn planting_refuses_an_unknown_asset_id_but_a_file_with_one_still_opens() {
    let root = dir("unknown-asset");
    let mut h = Headless::new(Access::roots(Some(root.clone()), Some(root.clone())));
    // An app fixture (put in the catalogue by hand here) and a species are accepted.
    let mut fixture = h.state.catalog.assets[0].clone();
    fixture.id = "vc-path-light".into();
    h.state.catalog.assets.push(fixture);
    let single = |asset: &str| json!({"name": "T", "asset": asset, "position": [1, 0, 1]});
    for asset in ["vc-path-light", "flora:quercus-robur"] {
        let r = execute(&mut h, 1, "planting.add_single", single(asset));
        assert_eq!(r["ok"], true, "{asset}: {r}");
    }
    // The wrong id is refused with the right one among the suggestions.
    let r = execute(
        &mut h,
        2,
        "planting.add_single",
        single("flora:vc-path-light"),
    );
    assert_eq!(r["ok"], false, "{r}");
    let e = r["error"].as_str().unwrap();
    assert!(
        e.contains("`flora:vc-path-light`") && e.contains("`vc-path-light`"),
        "{e}"
    );
    // A palette is checked too; a refusal changes nothing.
    let area = json!({"name": "A", "asset_palette": [{"asset": "nope", "weight": 1}],
                      "outline": [[0, 0], [4, 0], [4, 4]], "density_per_m2": 1.0});
    assert_eq!(execute(&mut h, 3, "planting.add_area", area)["ok"], false);
    assert_eq!(h.session.project().generators.len(), 2);
    // A saved file that names an unknown asset opens all the same.
    execute(&mut h, 4, "file.save", json!({"path": "u.visioncraft"}));
    let path = root.join("u.visioncraft");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("vc-path-light", "gone-asset")).unwrap();
    let opened = execute(&mut h, 5, "file.open", json!({"path": "u.visioncraft"}));
    assert_eq!(opened["ok"], true, "{opened}");
    assert!(
        h.session
            .project()
            .generators
            .iter()
            .any(|g| g.assets().contains(&"gone-asset"))
    );
}

#[test]
fn sun_hours_of_an_empty_project_is_the_daylight() {
    let mut h = Headless::new(Access::default());
    let r = execute(
        &mut h,
        1,
        "query.sun_hours",
        json!({"point": [0, 0], "season": "summer"}),
    );
    assert_eq!(r["ok"], json!(true), "{r}");
    let r = &r["result"];
    // Brussels in June: a long day, all of it sunny.
    assert!(r["daylight_hours"].as_f64().unwrap() > 15.5, "{r}");
    assert_eq!(r["sun_hours"], r["daylight_hours"]);
    assert_eq!(r["date"], json!({"month": 6, "day": 21}));
    assert_eq!(r["intervals"].as_array().unwrap().len(), 1);
}

#[test]
fn a_wall_shades_the_north_side_and_not_the_south() {
    let mut h = Headless::new(Access::default());
    let added = execute(
        &mut h,
        1,
        "shape.add_extrusion",
        json!({"name": "Block", "outline": [[-10, -2], [10, -2], [10, 2], [-10, 2]], "height": 12}),
    );
    assert_eq!(added["ok"], json!(true), "{added}");
    let hours = |h: &mut Headless, z: i32| {
        let r = execute(
            h,
            2,
            "query.sun_hours",
            json!({"point": [0, z], "season": "winter", "step_minutes": 5}),
        );
        assert_eq!(r["ok"], json!(true), "{r}");
        r["result"]["sun_hours"].as_f64().unwrap()
    };
    // -Z is north: the block is in the sun's way for the point just north of it.
    let (north, south) = (hours(&mut h, -4), hours(&mut h, 4));
    // In winter the sun stays in the south: the north side is in the block's shadow all day.
    assert!(north < 0.5 && south > 5.0, "north {north}, south {south}");
}

#[test]
fn a_context_block_shades_like_any_other() {
    let mut h = Headless::new(Access::default());
    let layer = execute(
        &mut h,
        1,
        "layer.add",
        json!({"name": "Surroundings", "context": true}),
    );
    assert_eq!(layer["ok"], json!(true), "{layer}");
    let l = layer["result"]["id"].as_str().unwrap().to_string();
    let added = execute(
        &mut h,
        2,
        "shape.add_extrusion",
        json!({"name": "Neighbour", "outline": [[-10, -2], [10, -2], [10, 2], [-10, 2]], "height": 12, "layer": l}),
    );
    assert_eq!(added["ok"], json!(true), "{added}");
    let r = execute(
        &mut h,
        3,
        "query.sun_hours",
        json!({"point": [0, -4], "season": "winter", "step_minutes": 5}),
    );
    assert_eq!(r["ok"], json!(true), "{r}");
    assert!(r["result"]["sun_hours"].as_f64().unwrap() < 0.5, "{r}");
    let inspect = execute(&mut h, 4, "document.inspect", json!({}));
    let layers = inspect["result"]["layers"].as_array().unwrap();
    assert!(
        layers
            .iter()
            .any(|x| x["id"] == l && x["context"] == json!(true))
    );
}

#[test]
fn sun_hours_over_an_l_shaped_outline_leaves_nulls_outside() {
    let mut h = Headless::new(Access::default());
    let r = execute(
        &mut h,
        1,
        "query.sun_hours",
        json!({"outline": [[0, 0], [6, 0], [6, 2], [2, 2], [2, 6], [0, 6]], "season": "summer", "cell_m": 1}),
    );
    assert_eq!(r["ok"], json!(true), "{r}");
    let r = &r["result"];
    assert_eq!((r["cols"].clone(), r["rows"].clone()), (json!(6), json!(6)));
    let hours = r["hours"].as_array().unwrap();
    assert_eq!(hours.len(), 36);
    // Row 0 is z 0..1 (all six cells inside), row 5 only the first cell, (5, 5) is outside.
    assert!(hours[0].is_f64() && hours[5].is_f64() && hours[30].is_f64());
    assert!(hours[35].is_null() && hours[2 * 6 + 3].is_null());
    assert_eq!(r["cells"], json!(20));
    assert_eq!(r["full_sun_share"], json!(1.0));
    assert_eq!(r["histogram"]["6+"], json!(20));
}

#[test]
fn sun_hours_refuses_both_neither_and_too_many_cells() {
    let mut h = Headless::new(Access::default());
    let both = execute(
        &mut h,
        1,
        "query.sun_hours",
        json!({"point": [0, 0], "outline": [[0, 0], [1, 0], [1, 1]]}),
    );
    assert_eq!(both["ok"], json!(false));
    assert!(
        both["error"].as_str().unwrap().contains("exactly one"),
        "{both}"
    );
    let neither = execute(&mut h, 2, "query.sun_hours", json!({}));
    assert_eq!(neither["ok"], json!(false), "{neither}");
    let many = execute(
        &mut h,
        3,
        "query.sun_hours",
        json!({"outline": [[0, 0], [200, 0], [200, 200], [0, 200]], "cell_m": 1}),
    );
    assert_eq!(many["ok"], json!(false));
    assert!(
        many["error"].as_str().unwrap().contains("larger `cell_m`"),
        "{many}"
    );
}
