//! The experimental WebMCP bridge: in a browser that offers `document.modelContext` (Chrome's
//! WebMCP origin trial), the open app registers a curated set of tools so the browser's own AI
//! agent can drive it, cinematics included. Off by default; the person switches it on (the
//! pill's *Experimental: browser AI agent (WebMCP)* box, or `?webmcp=1` in the URL).
//!
//! Three parts:
//! * this file's catalogue: which commands become tools (grouped by task), their JSON Schemas
//!   from the command specs ([`CommandDoc::input_schema`]), and which are destructive. It runs
//!   everywhere, so it is unit tested natively.
//! * `webmcp.js` (next to index.html): registers the tools with the browser, runs them through
//!   the page's own `vcRpc` (so they pass the app's `Session`, its undo stack and the "agent:"
//!   indicator like any agent command), confirms destructive calls with the person, limits
//!   film renders to one at a time and takes snapshots of the canvas.
//! * the `glue` module (wasm32 only): hands the catalogue to `webmcp.js` and shows its
//!   confirmation questions in egui.

use serde_json::{Value, json};
use visioncraft_automation::docs::CommandDoc;

/// Names tools by task, in the order they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Inspect,
    Design,
    View,
    Film,
    History,
    Meta,
}

impl Group {
    fn name(self) -> &'static str {
        match self {
            Self::Inspect => "Inspect",
            Self::Design => "Design",
            Self::View => "View",
            Self::Film => "Film",
            Self::History => "History",
            Self::Meta => "Meta",
        }
    }
}

/// What a tool runs.
#[derive(Clone, Copy, Debug)]
enum Route {
    /// A command by id (`engine.execute`); the tool's input is the command's params.
    Command(&'static str),
    /// A method of the window itself (`ui.get`, `ui.set`); with `wrap` the input goes under
    /// that key of the method's params.
    Method {
        name: &'static str,
        wrap: Option<&'static str>,
    },
    /// `document.inspect`, narrowed to one list of the project.
    Section,
    /// Any command by id.
    RunCommand,
    /// `engine.commands`.
    ListCommands,
    /// The canvas as an image.
    Snapshot,
}

struct Def {
    name: &'static str,
    group: Group,
    route: Route,
}

const fn d(name: &'static str, group: Group, route: Route) -> Def {
    Def { name, group, route }
}

const fn cmd(name: &'static str, group: Group, id: &'static str) -> Def {
    d(name, group, Route::Command(id))
}

use Group::{Design, Film, History, Inspect, Meta, View};

/// The default toolset: about 40 tools, named `visioncraft_<task>`. Everything else is
/// reachable through `visioncraft_run_command`.
const DEFS: &[Def] = &[
    // Inspect
    cmd("visioncraft_project_summary", Inspect, "document.inspect"),
    d("visioncraft_list", Inspect, Route::Section),
    cmd("visioncraft_quantities", Inspect, "query.quantities"),
    cmd("visioncraft_sun_hours", Inspect, "query.sun_hours"),
    cmd("visioncraft_light_check", Inspect, "query.light_check"),
    cmd(
        "visioncraft_plants_for_light",
        Inspect,
        "query.plants_for_light",
    ),
    d(
        "visioncraft_window_state",
        Inspect,
        Route::Method {
            name: "ui.get",
            wrap: None,
        },
    ),
    d(
        "visioncraft_select",
        Inspect,
        Route::Method {
            name: "ui.select",
            wrap: None,
        },
    ),
    // Design
    cmd("visioncraft_shape_add_area", Design, "shape.add_area"),
    cmd(
        "visioncraft_shape_add_extrusion",
        Design,
        "shape.add_extrusion",
    ),
    cmd("visioncraft_shape_move_vertex", Design, "shape.move_vertex"),
    cmd("visioncraft_shape_remove", Design, "shape.remove"),
    cmd("visioncraft_terrace_add", Design, "terrace.add"),
    cmd("visioncraft_stairs_add", Design, "stairs.add"),
    cmd("visioncraft_fence_add", Design, "fence.add"),
    cmd("visioncraft_bed_add", Design, "bed.add"),
    cmd("visioncraft_furniture_add", Design, "furniture.add"),
    cmd("visioncraft_furniture_add_set", Design, "furniture.add_set"),
    cmd(
        "visioncraft_planting_add_border",
        Design,
        "planting.add_border",
    ),
    cmd("visioncraft_planting_place", Design, "planting.add_single"),
    cmd("visioncraft_material_update", Design, "material.update"),
    // View
    cmd("visioncraft_view_set", View, "view.set"),
    cmd("visioncraft_time_set", View, "time.set"),
    d(
        "visioncraft_camera_set",
        View,
        Route::Method {
            name: "ui.set",
            wrap: Some("camera"),
        },
    ),
    cmd("visioncraft_viewpoint_add", View, "viewpoint.add"),
    // Film
    cmd("visioncraft_film_add", Film, "film.add"),
    cmd("visioncraft_film_add_shot", Film, "film.add_shot"),
    cmd("visioncraft_film_update_shot", Film, "film.update_shot"),
    cmd("visioncraft_film_remove_shot", Film, "film.remove_shot"),
    cmd("visioncraft_film_remove", Film, "film.remove"),
    cmd("visioncraft_film_orbit", Film, "film.orbit"),
    cmd("visioncraft_film_flythrough", Film, "film.flythrough"),
    cmd("visioncraft_film_reveal", Film, "film.reveal"),
    cmd("visioncraft_film_preview", Film, "film.preview"),
    cmd("visioncraft_film_render", Film, "film.render"),
    // History
    cmd("visioncraft_undo", History, "edit.undo"),
    cmd("visioncraft_redo", History, "edit.redo"),
    // Meta
    d("visioncraft_run_command", Meta, Route::RunCommand),
    d("visioncraft_list_commands", Meta, Route::ListCommands),
    d("visioncraft_snapshot", Meta, Route::Snapshot),
];

/// The lists `visioncraft_list` offers, and the key of `document.inspect` each one is.
const SECTIONS: &[(&str, &str)] = &[
    ("shapes", "shapes"),
    ("plantings", "generators"),
    ("layers", "layers"),
    ("materials", "materials"),
    ("viewpoints", "viewpoints"),
    ("lights", "lights"),
    ("films", "films"),
    ("species", "species"),
    ("models", "models"),
];

/// Commands that ask the person first: removing and clearing things, and replacing the
/// whole project.
pub fn is_destructive(id: &str) -> bool {
    let leaf = id.rsplit('.').next().unwrap_or(id);
    matches!(leaf, "remove" | "clear")
        || matches!(id, "file.new" | "file.open" | "project.load_example")
}

/// One tool, as `webmcp.js` registers it.
#[derive(Clone, Debug)]
pub struct Tool {
    pub name: &'static str,
    pub group: Group,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
    pub route: Value,
}

fn find<'a>(docs: &'a [CommandDoc], id: &str) -> Result<&'a CommandDoc, String> {
    docs.iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("`{id}`"))
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn point3(doc: &str) -> Value {
    json!({"type": "array", "minItems": 3, "maxItems": 3, "items": {"type": "number"}, "description": doc})
}

/// Resolves a catalogue against the commands that exist; an `Err` names every dead id.
fn resolve(defs: &[Def], docs: &[CommandDoc]) -> Result<Vec<Tool>, String> {
    let mut dead = Vec::new();
    let mut tools = Vec::new();
    for def in defs {
        let tool = |description: String, input_schema: Value, read_only: bool, route: Value| Tool {
            name: def.name,
            group: def.group,
            description: format!("[{}] {description}", def.group.name()),
            input_schema,
            read_only,
            destructive: false,
            route,
        };
        let t = match def.route {
            Route::Command(id) => match find(docs, id) {
                Ok(doc) => {
                    let mut t = tool(
                        format!("{}: {}", doc.label, doc.summary),
                        doc.input_schema(),
                        !doc.undoable && !matches!(id, "film.render" | "film.preview"),
                        json!({"kind": "command", "id": id}),
                    );
                    t.destructive = is_destructive(id);
                    if def.group == Design {
                        t.description.push_str(" When the person says \"this\", \"here\" or \"the selected one\", check the current selection first with visioncraft_window_state.");
                    }
                    if id == "film.render" {
                        t.description.push_str(" In the browser the frames cannot be written to disk (pictures cannot be read back): check a film with visioncraft_film_preview and visioncraft_snapshot, and render it with visioncraft-cli on a computer. Only one render runs at a time.");
                    }
                    t
                }
                Err(e) => {
                    dead.push(e);
                    continue;
                }
            },
            Route::Method { name, wrap } => {
                let (description, schema, read_only) = match (name, wrap) {
                    ("ui.get", _) => (
                        "What the window shows: mode, the camera (view, position, target, fov_y_deg), the selection (`selected.planting` and `selected.model`: what the person has picked), lights and what is still loading. Call it first when the person says \"this\", \"here\" or \"the selected one\".".to_string(),
                        object(json!({}), &[]),
                        true,
                    ),
                    ("ui.select", _) => (
                        "Selects a planting (opens Objects mode) or a model in the window, as a click would; null deselects. The person sees the selection; it is not part of the project and not an undo step.".to_string(),
                        object(
                            json!({
                                "planting": {"type": ["string", "null"], "description": "A planting's id (see visioncraft_list plantings)."},
                                "model": {"type": ["string", "null"], "description": "A linked model's id (see visioncraft_list models)."},
                            }),
                            &[],
                        ),
                        false,
                    ),
                    _ => (
                        "Places the window's camera: it looks from `position` at `target` (metres, Y up), as a person orbiting. Use it to frame a view before visioncraft_snapshot or visioncraft_viewpoint_add. The camera is not part of the project and not an undo step.".to_string(),
                        object(
                            json!({
                                "position": point3("Where the camera is."),
                                "target": point3("What it looks at."),
                                "fov_y_deg": {"type": "number", "minimum": 5, "maximum": 120, "description": "Vertical field of view in degrees."},
                            }),
                            &["position", "target"],
                        ),
                        false,
                    ),
                };
                tool(
                    description,
                    schema,
                    read_only,
                    json!({"kind": "method", "name": name, "wrap": wrap}),
                )
            }
            Route::Section => {
                let names: Vec<&str> = SECTIONS.iter().map(|(n, _)| *n).collect();
                tool(
                    "Lists one kind of thing in the project (ids, names, measures): shapes, plantings (generators with their evaluated plant counts), layers, materials, viewpoints, lights, films, species or models. Use visioncraft_project_summary for everything at once.".into(),
                    object(
                        json!({"what": {"type": "string", "enum": names, "description": "What to list."}}),
                        &["what"],
                    ),
                    true,
                    json!({"kind": "section", "sections": SECTIONS.iter().map(|(n, k)| (n.to_string(), json!(k))).collect::<serde_json::Map<_, _>>()}),
                )
            }
            Route::RunCommand => tool(
                "Runs any VisionCraft command by id, for what the other tools do not cover (list them with visioncraft_list_commands). Goes through the same undo history as the person's own edits. Destructive commands (remove, clear, file.new, file.open, project.load_example) ask the person to confirm first.".into(),
                object(
                    json!({
                        "command": {"type": "string", "description": "A command id, e.g. \"shape.add_area\"."},
                        "params": {"type": "object", "description": "The command's parameters, as visioncraft_list_commands documents them."},
                        "intent": {"type": "string", "maxLength": 300, "description": "Why you run it, in one short sentence; shown to the person."},
                    }),
                    &["command"],
                ),
                false,
                json!({"kind": "run_command"}),
            ),
            Route::ListCommands => tool(
                "Lists VisionCraft's commands with their parameters (name, type, unit, required, default, doc), result and example. Start with brief: true, then ask for one command by id.".into(),
                object(
                    json!({
                        "filter": {"type": "string", "description": "Case-insensitive part of a command id or label, e.g. \"planting\"."},
                        "id": {"type": "string", "description": "One command by its exact id: its full entry."},
                        "brief": {"type": "boolean", "default": true, "description": "Only id, params and the first sentence of the summary."},
                    }),
                    &[],
                ),
                true,
                json!({"kind": "list_commands"}),
            ),
            Route::Snapshot => tool(
                "A picture of the 3D view as the person sees it now (JPEG or PNG, at most 1024 px wide by default), so you can check your own work. Frame the view first with visioncraft_camera_set or a viewpoint. Plants and effects that are still loading may be missing; visioncraft_window_state reports what is pending.".into(),
                object(
                    json!({
                        "max_width": {"type": "integer", "minimum": 64, "maximum": 2048, "default": 1024, "description": "Widest the picture may be, in pixels."},
                        "format": {"type": "string", "enum": ["jpeg", "png"], "default": "jpeg", "description": "Image format."},
                        "as_text": {"type": "boolean", "default": false, "description": "Return a data URL as text instead of an image, for clients that cannot show images."},
                    }),
                    &[],
                ),
                true,
                json!({"kind": "snapshot"}),
            ),
        };
        tools.push(t);
    }
    if dead.is_empty() {
        Ok(tools)
    } else {
        Err(format!(
            "tools for commands that do not exist: {}",
            dead.join(", ")
        ))
    }
}

/// The default toolset, resolved against `docs` (`visioncraft_automation::commands::docs`).
pub fn tools(docs: &[CommandDoc]) -> Result<Vec<Tool>, String> {
    resolve(DEFS, docs)
}

/// What `webmcp.js` registers: the tools, and every destructive command id (for
/// `visioncraft_run_command`).
pub fn payload(docs: &[CommandDoc]) -> Result<Value, String> {
    let tools = tools(docs)?;
    let destructive: Vec<&str> = docs
        .iter()
        .map(|d| d.id.as_str())
        .filter(|id| is_destructive(id))
        .collect();
    Ok(json!({
        "tools": tools.iter().map(|t| json!({
            "name": t.name,
            "title": t.name.trim_start_matches("visioncraft_").replace('_', " "),
            "group": t.group.name(),
            "description": t.description,
            "inputSchema": t.input_schema,
            "annotations": {"readOnlyHint": t.read_only, "consequentialHint": t.destructive},
            "destructive": t.destructive,
            "route": t.route,
        })).collect::<Vec<_>>(),
        "destructiveCommands": destructive,
    }))
}

#[cfg(target_arch = "wasm32")]
pub use glue::WebMcp;

/// The browser side: `webmcp.js` is loaded by index.html as `window.vcWebMcp`.
#[cfg(target_arch = "wasm32")]
mod glue {
    use super::payload;
    use js_sys::{Array, Function, Reflect};
    use visioncraft_core::Session;
    use wasm_bindgen::{JsCast, JsValue};

    /// Seconds between looks for WebMCP in a browser that did not offer it yet.
    const RETRY_S: f64 = 3.0;

    fn call(name: &str, args: &[JsValue]) -> Option<JsValue> {
        let api = Reflect::get(&js_sys::global(), &"vcWebMcp".into()).ok()?;
        if !api.is_object() {
            return None;
        }
        let f: Function = Reflect::get(&api, &name.into()).ok()?.dyn_into().ok()?;
        f.apply(&api, &args.iter().collect::<Array>()).ok()
    }

    /// `?webmcp=1` in the page's URL.
    fn url_flag() -> bool {
        let search = Reflect::get(&js_sys::global(), &"location".into())
            .and_then(|l| Reflect::get(&l, &"search".into()))
            .ok()
            .and_then(|s| s.as_string())
            .unwrap_or_default();
        search
            .trim_start_matches('?')
            .split('&')
            .any(|kv| kv == "webmcp=1")
    }

    /// The switch, the registration and the confirmations waiting for the person.
    pub struct WebMcp {
        /// The person's choice (the box).
        pub on: bool,
        /// Whether the tools are registered with the browser now.
        registered: bool,
        /// One line for the pill: what is wrong or how many tools are offered.
        pub status: String,
        /// Questions to the person: (id, text).
        pub pending: Vec<(u32, String)>,
        checked_at: f64,
    }

    impl WebMcp {
        /// `stored` is last time's choice; `?webmcp=1` switches it on for this visit.
        pub fn new(stored: bool) -> Self {
            Self {
                on: stored || url_flag(),
                registered: false,
                status: String::new(),
                pending: Vec::new(),
                checked_at: f64::NEG_INFINITY,
            }
        }

        pub fn supported() -> bool {
            call("supported", &[])
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        }

        /// Registers or unregisters the tools to match the switch; collects the questions.
        pub fn sync(&mut self, session: &Session, now: f64) {
            if let Some(json) = call("takeConfirms", &[]).and_then(|v| v.as_string())
                && let Ok(list) = serde_json::from_str::<Vec<(u32, String)>>(&json)
            {
                self.pending.extend(list);
            }
            if self.on && !self.registered && now - self.checked_at >= RETRY_S {
                self.checked_at = now;
                self.status = if Self::supported() {
                    let docs = visioncraft_automation::commands::docs(session);
                    match payload(&docs) {
                        Ok(p) => {
                            let n = call("register", &[p.to_string().into()])
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            self.registered = n > 0.0;
                            if self.registered {
                                format!("{n} tools")
                            } else {
                                crate::strings::WEBMCP_FAILED.into()
                            }
                        }
                        Err(e) => e,
                    }
                } else {
                    crate::strings::WEBMCP_UNSUPPORTED.into()
                };
            } else if !self.on && (self.registered || !self.status.is_empty()) {
                call("unregister", &[]);
                self.registered = false;
                self.status.clear();
                self.pending.clear();
            }
        }

        /// The person's answer to a question.
        pub fn answer(&mut self, id: u32, allow: bool) {
            self.pending.retain(|(i, _)| *i != id);
            call("answer", &[id.into(), allow.into()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_automation::commands::docs;
    use visioncraft_automation::{Access, Headless};
    use visioncraft_core::Session;

    fn all_docs() -> Vec<CommandDoc> {
        docs(&Session::new(Default::default()))
    }

    #[test]
    fn the_curated_tools_resolve_to_real_commands() {
        let tools = tools(&all_docs()).expect("no dead names");
        assert_eq!(tools.len(), DEFS.len());
        assert!(
            (25..=40).contains(&tools.len()),
            "{} tools is not a sensible default set",
            tools.len()
        );
        let mut names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), tools.len(), "tool names are unique");
        for t in &tools {
            assert!(
                !t.name.is_empty()
                    && t.name.len() <= 64
                    && t.name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "`{}` is not a valid tool identifier",
                t.name
            );
            assert!(t.description.starts_with('['), "{}", t.name);
        }
        for must in [
            "visioncraft_run_command",
            "visioncraft_list_commands",
            "visioncraft_snapshot",
            "visioncraft_film_orbit",
            "visioncraft_film_flythrough",
            "visioncraft_film_reveal",
            "visioncraft_undo",
            "visioncraft_redo",
            "visioncraft_window_state",
            "visioncraft_select",
        ] {
            assert!(names.contains(&must), "{must}");
        }
    }

    #[test]
    fn a_dead_name_is_reported() {
        let defs = [
            cmd("visioncraft_a", Design, "shape.add_area"),
            cmd("visioncraft_b", Design, "shape.nonexistent"),
            cmd("visioncraft_c", Film, "film.vanished"),
        ];
        let err = resolve(&defs, &all_docs()).unwrap_err();
        assert!(err.contains("shape.nonexistent") && err.contains("film.vanished"));
        assert!(!err.contains("shape.add_area"));
    }

    /// The part of JSON Schema the generated schemas use: checks `value` against `schema`.
    fn validate(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
        let types: Vec<&str> = match schema.get("type") {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty() {
            let ok = types.iter().any(|ty| match *ty {
                "null" => value.is_null(),
                "string" => value.is_string(),
                "number" => value.is_number(),
                "integer" => value.is_i64() || value.is_u64(),
                "boolean" => value.is_boolean(),
                "object" => value.is_object(),
                "array" => value.is_array(),
                other => panic!("{at}: unknown type {other}"),
            });
            if !ok {
                return Err(format!("{at}: expected {types:?}, got {value}"));
            }
        }
        if let (Some(allowed), true) = (schema.get("enum").and_then(Value::as_array), true)
            && !allowed.contains(value)
        {
            return Err(format!("{at}: {value} is not one of {allowed:?}"));
        }
        if let Some(items) = value.as_array() {
            let n = items.len() as u64;
            if schema
                .get("minItems")
                .and_then(Value::as_u64)
                .is_some_and(|m| n < m)
                || schema
                    .get("maxItems")
                    .and_then(Value::as_u64)
                    .is_some_and(|m| n > m)
            {
                return Err(format!("{at}: {n} items"));
            }
            if let Some(item) = schema.get("items") {
                for (i, v) in items.iter().enumerate() {
                    validate(item, v, &format!("{at}[{i}]"))?;
                }
            }
        }
        if let Some(map) = value.as_object() {
            let props = schema.get("properties").and_then(Value::as_object);
            for r in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let r = r.as_str().unwrap();
                if !map.contains_key(r) {
                    return Err(format!("{at}: missing `{r}`"));
                }
            }
            for (k, v) in map {
                match props.and_then(|p| p.get(k)) {
                    Some(s) => validate(s, v, &format!("{at}.{k}"))?,
                    None if schema["additionalProperties"] == json!(false) => {
                        return Err(format!("{at}: unexpected `{k}`"));
                    }
                    None => {}
                }
            }
        }
        Ok(())
    }

    /// A schema is itself well formed: known keywords only, `required` names listed
    /// properties, nested schemas likewise.
    fn well_formed(schema: &Value, at: &str) -> Result<(), String> {
        const KEYWORDS: &[&str] = &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "items",
            "minItems",
            "maxItems",
            "minimum",
            "maximum",
            "maxLength",
            "enum",
            "default",
            "description",
        ];
        let map = schema.as_object().ok_or(format!("{at}: not an object"))?;
        for k in map.keys() {
            if !KEYWORDS.contains(&k.as_str()) {
                return Err(format!("{at}: unknown keyword `{k}`"));
            }
        }
        let types: Vec<&str> = match map.get("type") {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for ty in types {
            if ![
                "string", "number", "integer", "boolean", "object", "array", "null",
            ]
            .contains(&ty)
            {
                return Err(format!("{at}: bad type {ty}"));
            }
        }
        let props = map.get("properties").and_then(Value::as_object);
        for r in map
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let r = r
                .as_str()
                .ok_or(format!("{at}: required holds a non-string"))?;
            if !props.is_some_and(|p| p.contains_key(r)) {
                return Err(format!("{at}: required `{r}` is not a property"));
            }
        }
        for (k, v) in props.into_iter().flatten() {
            well_formed(v, &format!("{at}.{k}"))?;
        }
        if let Some(items) = map.get("items") {
            well_formed(items, &format!("{at}[]"))?;
        }
        Ok(())
    }

    #[test]
    fn every_input_schema_is_valid_and_accepts_the_commands_example() {
        let docs = all_docs();
        for t in tools(&docs).unwrap() {
            well_formed(&t.input_schema, t.name).unwrap();
            assert_eq!(t.input_schema["type"], "object", "{}", t.name);
            assert!(
                t.input_schema["additionalProperties"].is_boolean(),
                "{}",
                t.name
            );
            // MCP clients refuse a tool whose schema is not valid; check what a command's own
            // example does against the schema made from its spec.
            if let Some(id) = t.route["id"].as_str() {
                let doc = docs.iter().find(|d| d.id == id).unwrap();
                if let Some(example) = &doc.example {
                    validate(&t.input_schema, example, t.name)
                        .unwrap_or_else(|e| panic!("example of {id}: {e}"));
                }
            }
        }
    }

    #[test]
    fn destructive_commands_are_flagged() {
        let docs = all_docs();
        let p = payload(&docs).unwrap();
        let flagged: Vec<&str> = p["destructiveCommands"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for id in [
            "film.remove",
            "shape.remove",
            "planting.remove",
            "layer.remove",
            "fence.clear",
            "terrace.clear",
            "file.new",
            "file.open",
            "project.load_example",
        ] {
            assert!(flagged.contains(&id), "{id} must ask first");
        }
        for id in [
            "film.add",
            "film.remove_shot",
            "view.set",
            "edit.undo",
            "shape.add_area",
        ] {
            assert!(!flagged.contains(&id), "{id} needs no confirmation");
        }
        for t in p["tools"].as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            let expected = [
                "visioncraft_shape_remove",
                "visioncraft_planting_remove",
                "visioncraft_film_remove",
            ]
            .contains(&name);
            assert_eq!(t["destructive"], json!(expected), "{name}");
            assert_eq!(
                t["annotations"]["consequentialHint"],
                json!(expected),
                "{name}"
            );
        }
        // Read-only queries say so.
        let by = |n: &str| {
            p["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == n)
                .unwrap()
                .clone()
        };
        assert_eq!(
            by("visioncraft_quantities")["annotations"]["readOnlyHint"],
            true
        );
        assert_eq!(
            by("visioncraft_view_set")["annotations"]["readOnlyHint"],
            false
        );
        assert_eq!(
            by("visioncraft_film_render")["annotations"]["readOnlyHint"],
            false
        );
    }

    /// What `webmcp.js` sends for a command tool: `engine.execute` through the page's RPC.
    fn rpc(host: &mut Headless, command: &str, params: Value) -> Value {
        let line = json!({"id": 1, "method": "engine.execute", "params": {"command": command, "params": params}});
        let reply = visioncraft_automation::rpc::handle_line(host, &line.to_string()).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    #[test]
    fn tools_round_trip_through_the_page_protocol_and_undo() {
        let mut host = Headless::new(Access::default());
        let r = rpc(&mut host, "view.set", json!({"rain": 0.5}));
        assert_eq!(r["ok"], true, "{r}");
        let r = rpc(
            &mut host,
            "film.orbit",
            json!({"center": [0, 0, 0], "radius": 8, "seconds": 6}),
        );
        assert_eq!(r["ok"], true, "{r}");
        let id = r["result"]["id"].as_str().unwrap().to_string();
        let r = rpc(&mut host, "film.preview", json!({"id": id}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["result"]["duration_s"], 6.0);
        // The helper is one undo step.
        assert_eq!(rpc(&mut host, "edit.undo", json!({}))["ok"], true);
        assert!(host.session.project().films.is_empty());
        // A wrong call fails with a message for the agent, not a panic.
        assert_eq!(
            rpc(&mut host, "film.orbit", json!({"radius": 8}))["ok"],
            false
        );
    }

    #[test]
    fn the_list_tool_names_real_inspect_keys() {
        let mut host = Headless::new(Access::default());
        let r = rpc(
            &mut host,
            "document.inspect",
            json!({"faces": false, "measures": false}),
        );
        for (what, key) in SECTIONS {
            assert!(
                r["result"].get(key).is_some(),
                "{what}: no `{key}` in document.inspect"
            );
        }
    }
}
