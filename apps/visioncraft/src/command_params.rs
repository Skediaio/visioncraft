//! Every command call written out in the app's source sends only parameters its spec lists (the
//! core refuses unknown keys). Parameters built in code (`Form::params`, `handles`, the lamp
//! reconcile) are covered by the tests that run them through a `Session`.

use std::collections::BTreeSet;
use visioncraft_core::{Project, Session};

const SOURCES: &[(&str, &str)] = &[
    ("draw.rs", include_str!("draw.rs")),
    ("handles.rs", include_str!("handles.rs")),
    ("lamps.rs", include_str!("lamps.rs")),
    ("material_library.rs", include_str!("material_library.rs")),
    ("materials.rs", include_str!("materials.rs")),
    ("modelui.rs", include_str!("modelui.rs")),
    ("planting_panel.rs", include_str!("planting_panel.rs")),
    ("planting_tools.rs", include_str!("planting_tools.rs")),
    ("sky_panel.rs", include_str!("sky_panel.rs")),
    ("studio.rs", include_str!("studio.rs")),
    ("sun.rs", include_str!("sun.rs")),
    ("ui.rs", include_str!("ui.rs")),
    ("views.rs", include_str!("views.rs")),
    ("smoke.rs", include_str!("smoke.rs")),
    ("planting_smoke.rs", include_str!("planting_smoke.rs")),
];

/// The top-level keys of the object literal that starts at `src[0] == '{'`.
fn top_keys(src: &str) -> BTreeSet<String> {
    let b = src.as_bytes();
    let (mut depth, mut i, mut keys) = (0i32, 0usize, BTreeSet::new());
    while i < b.len() {
        match b[i] {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b'"' => {
                let end = i + 1 + src[i + 1..].find('"').expect("closed string");
                let rest = src[end + 1..].trim_start();
                if depth == 1 && rest.starts_with(':') {
                    keys.insert(src[i + 1..end].to_string());
                }
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

/// (file, command, keys) of every `"<command>", json!({...})` in the sources.
fn calls(s: &Session) -> Vec<(&'static str, String, BTreeSet<String>)> {
    let names: Vec<&str> = s.commands().into_iter().map(|(n, _)| n).collect();
    let mut out = Vec::new();
    for (file, src) in SOURCES {
        for (at, _) in src.match_indices("json!(") {
            let body = src[at + 6..].trim_start();
            if !body.starts_with('{') {
                continue;
            }
            // The command is the last string literal shortly before, if it names one.
            let before = &src[at.saturating_sub(160)..at];
            let Some(cmd) = before
                .rsplit('"')
                .skip(1)
                .step_by(2)
                .next()
                .filter(|c| names.contains(c))
            else {
                continue;
            };
            out.push((*file, cmd.to_string(), top_keys(body)));
        }
    }
    out
}

#[test]
fn every_written_out_command_sends_only_listed_parameters() {
    let s = Session::new(Project::default());
    let found = calls(&s);
    assert!(found.len() > 30, "only {} calls found", found.len());
    let mut problems = Vec::new();
    for (file, cmd, keys) in &found {
        let spec = s.command_spec(cmd).expect("registered");
        // Values come from the spec's own example and defaults: this checks names, not values.
        let mut params = serde_json::Map::new();
        for k in keys {
            let p = spec.params.iter().find(|p| &p.name == k);
            let v = spec
                .example
                .get(k)
                .cloned()
                .or_else(|| p.and_then(|p| p.default.clone()))
                .unwrap_or(serde_json::Value::Null);
            if p.is_none() || !v.is_null() {
                params.insert(k.clone(), v);
            }
        }
        for p in spec.params.iter().filter(|p| p.required) {
            if !params.contains_key(&p.name) {
                let v = spec.example.get(&p.name).cloned().unwrap_or_default();
                params.insert(p.name.clone(), v);
            }
        }
        if let Err(e) = spec.check(&serde_json::Value::Object(params)) {
            problems.push(format!("{file}: {cmd}: {e}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_scanner_sees_nested_keys_only_at_the_top() {
    let keys = top_keys(r#"{"id": id, "kind": {"type": "spot"}, "p": [1, 2]}) more"#);
    assert_eq!(keys.into_iter().collect::<Vec<_>>(), ["id", "kind", "p"]);
    let s = Session::new(Project::default());
    let found = calls(&s);
    assert!(
        found
            .iter()
            .any(|(f, c, k)| *f == "lamps.rs" && c == "light.update" && k.contains("kind"))
    );
}
