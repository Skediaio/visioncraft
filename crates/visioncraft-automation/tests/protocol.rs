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
}
