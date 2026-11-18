//! The documentation cannot drift from the commands: the parameters the protocol's command table
//! lists are the parameters the specs have, the `shot` doc names every key a shot takes, and the
//! quality names are the ones the renderer accepts.

use visioncraft_automation::{Access, Headless, Host, commands};
use visioncraft_core::film::{COMMON_FIELDS, SHOT_KINDS, ShotKind};

/// The cells of a markdown table row (an escaped `\|` is part of a cell).
fn cells(row: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = row.trim().trim_start_matches('|').chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                out.last_mut().unwrap().push_str("\\|");
                chars.next();
            }
            '|' => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out.into_iter().map(|c| c.trim().to_string()).collect()
}

/// The parameter names in a cell like `` `{point \| outline, season \| date?, cell_m? = 1}` ``:
/// the leading word of each top-level item and of each of its `\|` alternatives.
fn names(cell: &str) -> Option<Vec<String>> {
    let body = cell.strip_prefix("`{")?;
    let body = &body[..body.find("}`")?];
    let mut items = Vec::new();
    let (mut depth, mut cur) = (0i32, String::new());
    for c in body.chars() {
        match c {
            '[' | '{' | '(' => {
                depth += 1;
                cur.push(c);
            }
            ']' | '}' | ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => items.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    items.push(cur);
    let mut out = Vec::new();
    for item in items {
        for alt in item.split("\\|") {
            let word: String = alt
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            if !word.is_empty()
                && alt
                    .trim()
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase())
            {
                out.push(word);
            }
        }
    }
    out.sort();
    out.dedup();
    Some(out)
}

#[test]
fn the_protocol_table_lists_the_params_of_the_specs() {
    let mut host = Headless::new(Access::any());
    let docs = commands::docs(host.session());
    let table = include_str!("../docs/control-protocol.md");
    let (mut checked, mut drift) = (0, Vec::new());
    for row in table.lines().filter(|l| l.starts_with("| `")) {
        let c = cells(row);
        // `edit.undo`, `edit.redo` share a row; the id cells are `id` or `a`, `b`.
        let ids: Vec<&str> = c[0].split(", ").map(|i| i.trim_matches('`')).collect();
        let Some(listed) = c.get(1).and_then(|p| names(p)) else {
            continue;
        };
        for id in ids {
            let Some(doc) = docs.iter().find(|d| d.id == id) else {
                continue;
            };
            let mut have: Vec<String> = doc.params.iter().map(|p| p.name.clone()).collect();
            have.sort();
            if listed != have {
                let missing: Vec<&String> = have.iter().filter(|h| !listed.contains(h)).collect();
                let extra: Vec<&String> = listed.iter().filter(|l| !have.contains(l)).collect();
                drift.push(format!(
                    "`{id}`: the row lacks {missing:?}, the spec lacks {extra:?}"
                ));
            }
            checked += 1;
        }
    }
    assert!(
        drift.is_empty(),
        "docs/control-protocol.md drifted from the specs:\n{}",
        drift.join("\n")
    );
    assert!(checked > 20, "only {checked} rows were compared");
}

#[test]
fn the_shot_doc_names_every_key_a_shot_takes() {
    let mut host = Headless::new(Access::any());
    let docs = commands::docs(host.session());
    let doc = &docs
        .iter()
        .find(|d| d.id == "film.add_shot")
        .unwrap()
        .params[1]
        .doc;
    let mut keys: Vec<&str> = COMMON_FIELDS.to_vec();
    for kind in SHOT_KINDS {
        assert!(
            doc.contains(&format!("`{kind}`")),
            "the shot doc lacks the kind `{kind}`"
        );
        keys.extend(ShotKind::fields(kind));
    }
    for key in keys {
        assert!(doc.contains(key), "the shot doc does not name `{key}`");
    }
    for key in [
        "haze",
        "mist",
        "gustiness",
        "shafts",
        "lights",
        "white_balance",
        "grade",
        "exposure_ev",
        "join",
    ] {
        assert!(doc.contains(key), "{key}");
    }
}

#[test]
fn the_quality_names_include_photo_where_pictures_are_drawn() {
    let mut host = Headless::new(Access::any());
    let docs = commands::docs(host.session());
    for id in ["film.render", "film.preview"] {
        let d = docs.iter().find(|d| d.id == id).unwrap();
        let q = d.params.iter().find(|p| p.name == "quality").unwrap();
        assert!(
            q.allowed.iter().any(|a| a == "photo"),
            "{id}: {:?}",
            q.allowed
        );
    }
    let table = include_str!("../docs/control-protocol.md");
    let row = table
        .lines()
        .find(|l| l.starts_with("| `film.render`"))
        .unwrap();
    for key in ["samples", "shutter_s", "vignette", "grain", "photo"] {
        assert!(row.contains(key), "the film.render row lacks `{key}`");
    }
}
