//! Command documentation: what `engine.commands` and the MCP tool `command_list` give an agent.
//!
//! The shape is [`CommandDoc`]: per parameter its name, type, unit, whether it is required,
//! its default, its allowed values and a one-line doc; the result's shape; an example. The
//! host's own commands (`file.*`, `edit.*`, `document.*`, `view.render`, the queries) are
//! documented exactly in [`crate::commands`]. The core's commands only have a one-line
//! description today, so [`derive`] reads what it can from it (the `{a, b?, c? (default)}`
//! notation the descriptions use) and the naming conventions (`_m` metres, `_deg` degrees).
//!
//! shortcut: derived docs are a guess from the description text; delete `derive` and use the
//! core's own parameter specs once `visioncraft-core` provides them (see the README).

use serde_json::{Value, json};

/// One parameter of a command.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDoc {
    pub name: String,
    /// `string`, `number`, `integer`, `boolean`, `object`, `array`, `[x, y, z]`,
    /// `[[x, z], ...]`, or a notation such as `{count: n} or {every: m}`; `any` when unknown.
    pub ty: String,
    pub unit: Option<String>,
    pub required: bool,
    /// As written in the notation (`0.5`, `true`, `"the open file"`).
    pub default: Option<String>,
    pub allowed: Vec<String>,
    pub doc: String,
}

/// Who runs a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// `visioncraft-core`'s `Session`: undoable, changes the project.
    Core,
    /// The host (this crate): files, history, queries and pictures.
    Host,
}

/// The documentation of one command.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandDoc {
    pub id: String,
    /// A short name, as a menu would show it.
    pub label: String,
    pub summary: String,
    pub params: Vec<ParamDoc>,
    /// The params may hold keys that are not listed (`{id, ...}`): unknown keys pass.
    pub open: bool,
    /// The result's shape in the same notation, `null` for none.
    pub result: Option<String>,
    pub example: Option<Value>,
    pub origin: Origin,
    /// Whether a successful run is one undo step.
    pub undoable: bool,
    /// The docs were read from the description text (a guess).
    pub derived: bool,
}

impl CommandDoc {
    /// The params in 3dcraft's notation: `{"name": type = default, ...}`.
    pub fn notation(&self) -> String {
        let mut parts: Vec<String> = self
            .params
            .iter()
            .map(|p| {
                let ty = if p.allowed.is_empty() {
                    p.ty.clone()
                } else {
                    p.allowed
                        .iter()
                        .map(|a| format!("\"{a}\""))
                        .collect::<Vec<_>>()
                        .join("|")
                };
                let unit = p
                    .unit
                    .as_ref()
                    .map(|u| format!(" ({u})"))
                    .unwrap_or_default();
                let default = match (&p.default, p.required) {
                    (Some(d), _) => format!(" = {d}"),
                    (None, false) => " = not set".into(),
                    (None, true) => String::new(),
                };
                format!("\"{}\": {ty}{unit}{default}", p.name)
            })
            .collect();
        if self.open {
            parts.push("...".into());
        }
        format!("{{{}}}", parts.join(", "))
    }

    /// The entry `command_list` gives: brief or full.
    pub fn to_json(&self, brief: bool, enabled: Result<(), String>) -> Value {
        if brief {
            return json!({
                "id": self.id,
                "params": self.notation(),
                "summary": first_sentence(&self.summary),
                "enabled": enabled.is_ok(),
            });
        }
        let parameters: Vec<Value> = self
            .params
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "type": p.ty,
                    "unit": p.unit,
                    "required": p.required,
                    "default": p.default,
                    "allowed": p.allowed,
                    "doc": p.doc,
                })
            })
            .collect();
        json!({
            "id": self.id,
            "label": self.label,
            "params": self.notation(),
            "summary": self.summary,
            "parameters": parameters,
            "openParams": self.open,
            "result": self.result,
            "example": self.example,
            "undoable": self.undoable,
            "derivedDocs": self.derived,
            "enabled": enabled.is_ok(),
            "disabledReason": enabled.err(),
        })
    }
}

/// The first sentence of `summary`, at most 200 characters (3dcraft's `first_sentence`,
/// simplified).
pub fn first_sentence(summary: &str) -> String {
    let mut depth = 0i32;
    let chars: Vec<char> = summary.chars().collect();
    let mut end = chars.len();
    for (i, c) in chars.iter().enumerate() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            '.' if depth == 0 && chars.get(i + 1).is_none_or(|n| *n == ' ') => {
                end = i + 1;
                break;
            }
            _ => {}
        }
    }
    let s: String = chars[..end].iter().collect();
    if s.chars().count() <= 200 {
        s
    } else {
        format!("{}…", s.chars().take(199).collect::<String>())
    }
}

/// Splits `text` at `sep` outside brackets and parentheses.
fn split_top(text: &str, sep: char) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in text.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// The text inside the bracket that opens at byte `open`, and the byte after its closing one.
fn bracketed(text: &str, open: usize) -> Option<(&str, usize)> {
    let mut depth = 0i32;
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[open + 1..open + i], open + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Type and unit by the core's naming conventions, for a parameter whose notation gives none.
fn guess(command: &str, name: &str) -> (&'static str, Option<&'static str>) {
    let shape = command.starts_with("shape.");
    match name {
        n if n.ends_with("_m2") => ("number", Some("1/m2")),
        n if n.ends_with("_m") => ("number", Some("m")),
        n if n.ends_with("_deg") => ("number", Some("deg")),
        n if n.ends_with("_k") => ("number", Some("K")),
        n if n.ends_with("_ev") => ("number", Some("EV")),
        "utc_offset_hours" | "hour" => ("number", Some("h")),
        "latitude" | "longitude" => ("number", Some("deg")),
        "lumens" => ("number", Some("lm")),
        "elevation" | "height" | "thickness" => ("number", Some("m")),
        "base" if shape => ("number", Some("m")),
        "year" | "month" | "day" | "index" | "edge" | "seed" | "count" => ("integer", None),
        "scale" | "scale_min" | "scale_max" | "haze" | "wind_strength" | "strength" | "falloff"
        | "metallic" | "roughness" | "position_jitter" | "spacing_jitter" | "intensity" => {
            ("number", None)
        }
        "position" | "target" | "translation" => ("[x, y, z]", Some("m")),
        "outline" | "points" => ("[[x, z], ...]", Some("m")),
        "center" | "origin" => ("[x, z]", Some("m")),
        "visible" | "closed" | "drape" | "enabled" | "erase" | "random_rotation" | "clay" => {
            ("boolean", None)
        }
        "asset_palette" => ("[{asset, weight}]", None),
        "layers" => ("array", None),
        "time" | "transform" | "params" | "maps" | "sky" | "ground" => ("object", None),
        "id" | "name" | "layer" | "material" | "asset" | "owner" | "shape" | "item" | "preset"
        | "source" | "base_material" | "kind" | "base" | "tool" => ("string", None),
        _ => ("any", None),
    }
}

/// Reads a command's docs from its one-line description, written as the core writes them:
/// `Label: {name, kind: a|b, size? (default), note? (or other)}; returns {id}`.
pub fn derive(id: &str, description: &str) -> CommandDoc {
    let mut doc = CommandDoc {
        id: id.to_string(),
        label: String::new(),
        summary: description.to_string(),
        params: Vec::new(),
        open: false,
        result: None,
        example: example(id),
        origin: Origin::Core,
        undoable: true,
        derived: true,
    };
    let Some(open) = description.find('{') else {
        doc.label = description.trim_end_matches('.').to_string();
        return doc;
    };
    doc.label = description[..open]
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_string();
    let Some((inner, after)) = bracketed(description, open) else {
        return doc;
    };
    if let Some(r) = description[after..].find("returns ") {
        doc.result = Some(description[after + r + 8..].trim().to_string());
    }
    for item in split_top(inner, ',') {
        let item = item.trim();
        if item.starts_with("...") {
            doc.open = true;
            continue;
        }
        let end = item
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(item.len());
        let name = &item[..end];
        if name.is_empty() {
            continue;
        }
        let mut rest = item[end..].trim();
        let required = !rest.starts_with('?');
        rest = rest.trim_start_matches('?').trim();
        // `(note)` at the end: a default when its first part is a plain value.
        let (mut ty_text, mut default, mut note) = (String::new(), None, String::new());
        let paren = split_top_last_paren(rest);
        if let Some(p) = paren {
            if let Some((n, _)) = bracketed(rest, p) {
                let n = n.trim();
                let (head, tail) = n.split_once(", ").unwrap_or((n, ""));
                let plain =
                    head.parse::<f64>().is_ok() || matches!(head, "true" | "false" | "null");
                if plain {
                    default = Some(head.to_string());
                    note = tail.to_string();
                } else {
                    note = n.to_string();
                }
            }
            rest = rest[..p].trim();
        }
        if let Some(t) = rest.strip_prefix(':') {
            ty_text = t.trim().to_string();
        }
        let (guessed, unit) = guess(id, name);
        let mut allowed = Vec::new();
        let ty = if ty_text.is_empty() {
            guessed.to_string()
        } else if ty_text.contains('|')
            && ty_text
                .split('|')
                .all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            allowed = ty_text.split('|').map(str::to_string).collect();
            "string".to_string()
        } else {
            ty_text
        };
        doc.params.push(ParamDoc {
            name: name.to_string(),
            ty,
            unit: unit.map(str::to_string),
            required,
            default,
            allowed,
            doc: note,
        });
    }
    doc
}

/// Where the last top-level `(` of `text` starts, when the text ends with its `)`.
fn split_top_last_paren(text: &str) -> Option<usize> {
    if !text.ends_with(')') {
        return None;
    }
    let mut depth = 0i32;
    for (i, c) in text.char_indices().rev() {
        match c {
            ')' | ']' | '}' => depth += 1,
            '(' | '[' | '{' => {
                depth -= 1;
                if depth == 0 {
                    return (c == '(').then_some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Worked examples of the core commands a garden is built with, tested in tests/protocol.rs.
fn example(id: &str) -> Option<Value> {
    Some(match id {
        "shape.add_area" => {
            json!({"name": "Lawn", "outline": [[-10, -8], [10, -8], [10, 8], [-10, 8]], "material": "lawn"})
        }
        "shape.add_extrusion" => {
            json!({"name": "House", "outline": [[-4, -6], [4, -6], [4, 0], [-4, 0]], "height": 6})
        }
        "planting.add_spline" => {
            json!({"name": "Hedge", "asset_palette": [{"asset": "flora:carpinus-hedge", "weight": 1}], "points": [[-10, 8], [10, 8]], "spacing": {"every": 0.6}})
        }
        "planting.add_single" => {
            json!({"name": "Cherry", "asset": "flora:prunus-serrulata", "position": [5, 0, 4]})
        }
        "light.add" => json!({"name": "Lamp", "position": [2, 2.2, 1], "lumens": 800}),
        "viewpoint.add" => {
            json!({"name": "From the gate", "position": [0, 1.7, 14], "target": [0, 1.5, 0]})
        }
        "material.assign" => {
            json!({"target": {"type": "owner", "owner": "S1"}, "material": "brick"})
        }
        "time.set" => json!({"hour": 20.5}),
        "site.set" => json!({"latitude": 51.05, "longitude": 3.72}),
        "model.add" => json!({"name": "Villa", "kind": "3dcraft", "source": "villa.3dcraft"}),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_names_types_defaults_and_results() {
        let d = derive(
            "shape.add_extrusion",
            "Add an extruded outline: {name, outline, closed? (true), base? (0), height, thickness? (0.2, open only), layer?, material?}; returns {id}",
        );
        assert_eq!(d.label, "Add an extruded outline");
        assert_eq!(d.result.as_deref(), Some("{id}"));
        let names: Vec<&str> = d.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "name",
                "outline",
                "closed",
                "base",
                "height",
                "thickness",
                "layer",
                "material"
            ]
        );
        let t = &d.params[5];
        assert_eq!((t.ty.as_str(), t.unit.as_deref()), ("number", Some("m")));
        assert_eq!(
            (t.default.as_deref(), t.doc.as_str()),
            (Some("0.2"), "open only")
        );
        assert!(d.params[0].required && !d.params[2].required);
        assert!(!d.open);
        let k = derive(
            "model.add",
            "Add a model: {name, kind: 3dcraft|obj, source}",
        );
        assert_eq!(k.params[1].allowed, ["3dcraft", "obj"]);
        let o = derive("planting.update", "Change fields: {id, ...}");
        assert!(o.open && o.params.len() == 1);
        let v = derive(
            "view.set",
            "Set: {exposure_ev? (null = auto), sky? ({type: physical} or {type: hdri, id}), haze?}",
        );
        assert_eq!(v.params.len(), 3);
        assert_eq!(v.params[0].doc, "null = auto");
        assert_eq!(v.params[1].doc, "{type: physical} or {type: hdri, id}");
        assert!(
            derive("terrain.remove", "Remove the terrain")
                .params
                .is_empty()
        );
        assert!(d.notation().contains("\"thickness\": number (m) = 0.2"));
    }
}
