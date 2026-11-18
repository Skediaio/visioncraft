//! Command documentation: what `engine.commands` and the MCP tool `command_list` give an agent.
//!
//! One shape, [`CommandDoc`], for the core's commands (from `Session::command_specs()`, mapped
//! one to one by [`from_spec`]) and the host's own (written in [`crate::commands::host_docs`]):
//! per parameter its name, type, unit, whether it is required, its default, its allowed values
//! and a one-line doc; the result's shape; an example that runs.

use serde_json::{Value, json};
use visioncraft_core::CommandSpec;

/// One parameter of a command.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDoc {
    pub name: String,
    /// `string`, `number`, `integer`, `boolean`, `[x, z]`, `[x, y, z]`, `[[x, z], ...]`, ...
    pub ty: String,
    pub unit: Option<String>,
    pub required: bool,
    /// As JSON text (`0.5`, `true`, `"point"`), or in words where it is not a constant.
    pub default: Option<String>,
    pub allowed: Vec<String>,
    pub doc: String,
}

/// The documentation of one command.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandDoc {
    pub id: String,
    /// A short name, as a menu would show it.
    pub label: String,
    pub summary: String,
    pub params: Vec<ParamDoc>,
    /// The params may hold keys that are not listed.
    pub open: bool,
    /// The result's shape, `null` for none.
    pub result: Option<String>,
    pub example: Option<Value>,
    /// Whether a successful run that changes the project is one undo step.
    pub undoable: bool,
}

/// A core command's spec, as documentation.
pub fn from_spec(s: &CommandSpec) -> CommandDoc {
    let text = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_string);
    CommandDoc {
        id: s.name.clone(),
        label: s.label.clone(),
        summary: s.summary.clone(),
        params: s
            .params
            .iter()
            .map(|p| ParamDoc {
                name: p.name.clone(),
                ty: p.ty.describe().to_string(),
                unit: p.unit.clone(),
                required: p.required,
                default: p.default.as_ref().map(Value::to_string),
                allowed: p.allowed.iter().flatten().map(text).collect(),
                doc: p.doc.clone(),
            })
            .collect(),
        open: s.open,
        result: (s.result != "null").then(|| s.result.clone()),
        example: Some(s.example.clone()),
        undoable: s.undoable,
    }
}

/// The JSON Schema (draft 2020-12) of a parameter type as [`ParamDoc::ty`] spells it; an
/// unknown spelling (a host command's own words) leaves the value unconstrained.
fn type_schema(ty: &str) -> Value {
    let points = |n: usize| json!({"type": "array", "minItems": n, "maxItems": n, "items": {"type": "number"}});
    match ty {
        "string" | "number" | "integer" | "boolean" | "object" | "array" => json!({"type": ty}),
        "[x, z]" => points(2),
        "[x, y, z]" => points(3),
        "[r, g, b] or [r, g, b, a]" => {
            json!({"type": "array", "minItems": 3, "maxItems": 4, "items": {"type": "number"}})
        }
        "[[x, z], ...]" => json!({"type": "array", "items": points(2)}),
        "[[x, y, z], ...]" => json!({"type": "array", "items": points(3)}),
        _ => json!({}),
    }
}

impl CommandDoc {
    /// The command's params as a JSON Schema object: each parameter with its type, a
    /// description (doc, unit, default) and its allowed values; `required` as documented.
    /// Open commands accept more keys; the others refuse them, as the command does.
    pub fn input_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        for p in &self.params {
            let mut schema = type_schema(&p.ty);
            let mut doc = p.doc.clone();
            if let Some(unit) = &p.unit {
                doc.push_str(&format!(" ({unit})"));
            }
            if p.ty == "string" && !p.allowed.is_empty() {
                schema["enum"] = json!(p.allowed);
            }
            // An optional parameter may be null (null clears it in many commands).
            if !p.required {
                if let Some(ty) = schema["type"].as_str().map(str::to_string) {
                    schema["type"] = json!([ty, "null"]);
                }
                if let Some(values) = schema["enum"].as_array_mut() {
                    values.push(Value::Null);
                }
            }
            if let Some(d) = &p.default {
                match serde_json::from_str::<Value>(d) {
                    Ok(v) if !p.required => schema["default"] = v,
                    _ => doc.push_str(&format!(" Default: {d}.")),
                }
            }
            schema["description"] = json!(doc.trim());
            properties.insert(p.name.clone(), schema);
        }
        let required: Vec<&str> = self
            .params
            .iter()
            .filter(|p| p.required)
            .map(|p| p.name.as_str())
            .collect();
        let mut schema = json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": self.open,
        });
        if !required.is_empty() {
            schema["required"] = json!(required);
        }
        schema
    }

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
                    "name": p.name, "type": p.ty, "unit": p.unit, "required": p.required,
                    "default": p.default, "allowed": p.allowed, "doc": p.doc,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_core_spec_maps_one_to_one() {
        let s = visioncraft_core::Session::new(Default::default());
        let d = from_spec(s.command_spec("shape.add_extrusion").unwrap());
        assert_eq!(d.id, "shape.add_extrusion");
        let outline = d.params.iter().find(|p| p.name == "outline").unwrap();
        assert_eq!(
            (outline.ty.as_str(), outline.required),
            ("[[x, z], ...]", true)
        );
        assert!(d.notation().starts_with('{'));
        assert_eq!(first_sentence("One. Two."), "One.");
    }

    #[test]
    fn the_input_schema_follows_the_spec() {
        let s = visioncraft_core::Session::new(Default::default());
        let d = from_spec(s.command_spec("shape.add_extrusion").unwrap());
        let schema = d.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["outline"]["type"], "array");
        let optional = d.params.iter().find(|p| !p.required).unwrap();
        let ty = &schema["properties"][optional.name.as_str()]["type"];
        assert!(
            ty.as_array().is_none_or(|t| t.contains(&json!("null"))),
            "{ty}"
        );
        assert_eq!(schema["properties"]["outline"]["items"]["minItems"], 2);
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("outline"))
        );
    }
}
