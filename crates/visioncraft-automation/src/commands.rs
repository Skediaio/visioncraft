//! What both protocols run: every command of the core's [`Session`], plus the host's own
//! (files, history, inspect, queries, pictures), one registry with one listing, and batches
//! that are all-or-nothing.

use crate::MAX_BATCH_STEPS;
use crate::docs::{CommandDoc, Origin, ParamDoc, derive};
use crate::fields::{Fields, describe, shorten};
use crate::host::{Host, RenderRequest, missing_json};
use crate::inspect::{self, Verbosity};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use visioncraft_core::{ModelKind, Project, Session};

/// Wall-clock milliseconds to a tenth; `null` where there is no clock (the browser).
pub(crate) struct Stopwatch(#[cfg(not(target_arch = "wasm32"))] std::time::Instant);

impl Stopwatch {
    pub fn start() -> Self {
        Self(
            #[cfg(not(target_arch = "wasm32"))]
            std::time::Instant::now(),
        )
    }

    pub fn ms(&self) -> Value {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let ms = self.0.elapsed().as_secs_f64() * 1000.0;
            json!((ms * 10.0).round() / 10.0)
        }
        #[cfg(target_arch = "wasm32")]
        Value::Null
    }
}

// ------------------------------------------------------------------------------- docs

#[allow(clippy::too_many_arguments)]
fn p(
    name: &str,
    ty: &str,
    unit: Option<&str>,
    required: bool,
    default: Option<&str>,
    allowed: &[&str],
    doc: &str,
) -> ParamDoc {
    ParamDoc {
        name: name.into(),
        ty: ty.into(),
        unit: unit.map(Into::into),
        required,
        default: default.map(Into::into),
        allowed: allowed.iter().map(|a| a.to_string()).collect(),
        doc: doc.into(),
    }
}

fn host_doc(
    id: &str,
    label: &str,
    summary: &str,
    params: Vec<ParamDoc>,
    result: &str,
    example: Value,
    undoable: bool,
) -> CommandDoc {
    CommandDoc {
        id: id.into(),
        label: label.into(),
        summary: summary.into(),
        params,
        open: false,
        result: Some(result.into()),
        example: (!example.is_null()).then_some(example),
        origin: Origin::Host,
        undoable,
        derived: false,
    }
}

/// The host's own commands, documented exactly.
pub fn host_docs() -> Vec<CommandDoc> {
    let time = "{year?, month?, day?, hour?}: local time at the site";
    let camera = "{position: [x, y, z], target: [x, y, z], fov_y_deg? = 50}";
    vec![
        host_doc(
            "file.new",
            "New Project",
            "Starts an empty project (the built-in materials, the default site in Brussels, 21 June 14:00) with a new history; the open file is forgotten.",
            vec![],
            "{models: 0}",
            json!({}),
            false,
        ),
        host_doc(
            "file.open",
            "Open",
            "Opens a .visioncraft project and loads the models it refers to (paths relative to the project file); a new history starts. Models that cannot be read are listed in missingModels, the project still opens.",
            vec![p(
                "path",
                "string",
                None,
                true,
                None,
                &[],
                "A .visioncraft file beneath the read root",
            )],
            "{path, models, shapes, generators, lights, viewpoints, missingModels}",
            json!({"path": "garden.visioncraft"}),
            false,
        ),
        host_doc(
            "file.save",
            "Save",
            "Writes the project as .visioncraft JSON. Model paths are rewritten relative to the new folder when it changes.",
            vec![
                p(
                    "path",
                    "string",
                    None,
                    false,
                    Some("the open file's path"),
                    &[],
                    "Where to write, beneath the write root",
                ),
                p(
                    "overwrite",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Replace a file that is not the open project's own",
                ),
            ],
            "{path, bytes}",
            json!({"path": "garden.visioncraft"}),
            false,
        ),
        host_doc(
            "edit.undo",
            "Undo",
            "Takes back the last change (one command, or one whole batch).",
            vec![],
            "{can_undo, can_redo}",
            json!({}),
            false,
        ),
        host_doc(
            "edit.redo",
            "Redo",
            "Does the change last undone again.",
            vec![],
            "{can_undo, can_redo}",
            json!({}),
            false,
        ),
        host_doc(
            "document.inspect",
            "Inspect",
            "The project as JSON: site, time and sun, view, layers, models (bounding box, face names), shapes (areas, face names), materials, assignments, planting generators with their evaluated plant counts, species, lights, viewpoints, terrain, history.",
            vec![
                p(
                    "faces",
                    "boolean",
                    None,
                    false,
                    Some("true"),
                    &[],
                    "List the face names of every model and shape",
                ),
                p(
                    "measures",
                    "boolean",
                    None,
                    false,
                    Some("true"),
                    &[],
                    "Evaluate generators (count, truncated) and measure shapes (area_m2, footprint_m2, length_m)",
                ),
                p(
                    "placements",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Every plant's asset, position, rotation and scale (large)",
                ),
                p(
                    "full",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Materials, generators and species with all their fields, as the commands take them",
                ),
            ],
            "{path, dirty, revision, site, time, sun, view, style, layers, models, shapes, materials, assignments, generators, species, lights, viewpoints, terrain, history}",
            json!({"faces": false}),
            false,
        ),
        host_doc(
            "document.measure",
            "Measure",
            "Lengths, areas, heights and slopes. Give one or more of points, outline and path.",
            vec![
                p(
                    "points",
                    "[[x, y, z], ...]",
                    Some("m"),
                    false,
                    None,
                    &[],
                    "A polyline: length_m, and from the first to the last point height_difference_m and slope_percent",
                ),
                p(
                    "outline",
                    "[[x, z], ...]",
                    Some("m"),
                    false,
                    None,
                    &[],
                    "A polygon: area_m2",
                ),
                p(
                    "path",
                    "[[x, z], ...]",
                    Some("m"),
                    false,
                    None,
                    &[],
                    "A walk over the ground (terrain, areas, models): ground_length_m",
                ),
            ],
            "{length_m?, height_difference_m?, slope_percent?, area_m2?, ground_length_m?}",
            json!({"outline": [[0, 0], [4, 0], [4, 2.5], [0, 2.5]]}),
            false,
        ),
        host_doc(
            "query.raycast",
            "Ray Cast",
            "The nearest surface a ray hits: models, shapes and terrain on visible layers. Not a hit is {hit: false}.",
            vec![
                p(
                    "origin",
                    "[x, y, z]",
                    Some("m"),
                    true,
                    None,
                    &[],
                    "Where the ray starts",
                ),
                p(
                    "direction",
                    "[x, y, z]",
                    None,
                    true,
                    None,
                    &[],
                    "Which way it goes; any length but zero",
                ),
            ],
            "{hit, owner, kind: model|shape|terrain, face, body, point, normal, distance}",
            json!({"origin": [0, 20, 0], "direction": [0, -1, 0]}),
            false,
        ),
        host_doc(
            "query.ground_height",
            "Ground Height",
            "The height plants stand on at (x, z): terrain, areas and models, the highest.",
            vec![
                p("x", "number", Some("m"), true, None, &[], "East"),
                p(
                    "z",
                    "number",
                    Some("m"),
                    true,
                    None,
                    &[],
                    "South (-z is north)",
                ),
            ],
            "{x, z, height: number or null}",
            json!({"x": 0, "z": 0}),
            false,
        ),
        host_doc(
            "query.sun",
            "Sun Position",
            "Where the sun stands for the project's site and time (or the time given), with sunrise and sunset that day.",
            vec![p(
                "time",
                "object",
                None,
                false,
                Some("the project's time"),
                &[],
                time,
            )],
            "{azimuth_deg (clockwise from north), elevation_deg, direction, sunrise_hour, sunset_hour, above_horizon}",
            json!({"time": {"hour": 21}}),
            false,
        ),
        host_doc(
            "catalog.search",
            "Search Catalogue",
            "Assets of the built-in catalogue whose name or a tag contains query, by name.",
            vec![
                p(
                    "query",
                    "string",
                    None,
                    false,
                    Some("\"\" (all)"),
                    &[],
                    "Part of a name or tag, any case",
                ),
                p(
                    "category",
                    "string",
                    None,
                    false,
                    None,
                    &[
                        "tree",
                        "shrub",
                        "perennial",
                        "grass",
                        "flower",
                        "hedge",
                        "furniture",
                        "pot",
                        "lighting",
                        "fence",
                        "play",
                        "vehicle",
                        "person",
                        "other",
                    ],
                    "Only this category",
                ),
                p(
                    "exposure",
                    "string",
                    None,
                    false,
                    None,
                    &["sun", "half_shade", "shade"],
                    "Only plants for this exposure",
                ),
                p(
                    "max_height_m",
                    "number",
                    Some("m"),
                    false,
                    None,
                    &[],
                    "Only plants at most this tall when mature",
                ),
                p(
                    "limit",
                    "integer",
                    None,
                    false,
                    Some("50"),
                    &[],
                    "At most this many, 1 to 500",
                ),
            ],
            "{total, assets: [{id, name, category, size_m, license, author}]}",
            json!({"query": "bench"}),
            false,
        ),
        host_doc(
            "species.list",
            "List Species",
            "The procedural plant species: the built-in ones (asset id flora:<id>) and the project's custom ones (their own ids).",
            vec![
                p(
                    "filter",
                    "string",
                    None,
                    false,
                    None,
                    &[],
                    "Part of the id, common or botanical name, any case",
                ),
                p(
                    "category",
                    "string",
                    None,
                    false,
                    None,
                    &[
                        "tree",
                        "conifer",
                        "palm",
                        "shrub",
                        "hedge",
                        "perennial",
                        "fern",
                        "grass",
                        "ground_cover",
                        "climber",
                    ],
                    "Only this category",
                ),
            ],
            "{species: [{asset, common_name, botanical_name, category, mature_height_m, mature_width_m, evergreen, bloom_months, exposure}], custom: [{asset, name, base}]}",
            json!({"category": "hedge"}),
            false,
        ),
        host_doc(
            "planting.list_presets",
            "List Planting Presets",
            "The layered plantings planting.apply_preset takes.",
            vec![],
            "{presets: [{id, name, layers}]}",
            json!({}),
            false,
        ),
        host_doc(
            "view.render",
            "Render",
            "Draws the project with the app's renderer, without a window: PNG to path beneath the write root, else returned as base64. Plants are grown before the picture is taken. The project does not change.",
            vec![
                p(
                    "path",
                    "string",
                    None,
                    false,
                    None,
                    &[],
                    "A .png beneath the write root; without it the PNG comes back as {mime, base64}",
                ),
                p(
                    "width",
                    "integer",
                    Some("px"),
                    false,
                    Some("800"),
                    &[],
                    "16 to 4096",
                ),
                p(
                    "height",
                    "integer",
                    Some("px"),
                    false,
                    Some("600"),
                    &[],
                    "16 to 4096",
                ),
                p(
                    "viewpoint",
                    "string",
                    None,
                    false,
                    None,
                    &[],
                    "A viewpoint's id or name: its camera, and its time when it has one",
                ),
                p(
                    "camera",
                    "object",
                    None,
                    false,
                    Some("the whole scene, framed"),
                    &[],
                    camera,
                ),
                p(
                    "time",
                    "object",
                    None,
                    false,
                    Some("the viewpoint's, else the project's"),
                    &[],
                    time,
                ),
                p(
                    "quality",
                    "string",
                    None,
                    false,
                    Some("\"high\""),
                    &["low", "medium", "high"],
                    "The renderer's quality preset",
                ),
            ],
            "{path? | mime, base64; width, height, camera, time, ...the renderer's info}",
            json!({"width": 960, "height": 540, "viewpoint": "From the gate"}),
            false,
        ),
    ]
}

/// Every command, sorted by id: the host's and the core's.
pub fn docs(session: &Session) -> Vec<CommandDoc> {
    let mut all = host_docs();
    for (id, description) in session.commands() {
        all.push(derive(id, description));
    }
    all.sort_by(|a, b| a.id.cmp(&b.id));
    all
}

fn enabled(session: &Session, id: &str) -> Result<(), String> {
    match id {
        "edit.undo" if !session.can_undo() => Err("nothing to undo".into()),
        "edit.redo" if !session.can_redo() => Err("nothing to redo".into()),
        _ => Ok(()),
    }
}

/// What a listing lists.
#[derive(Default)]
pub struct ListQuery<'a> {
    /// Case-insensitive part of an id or label.
    pub filter: Option<&'a str>,
    /// One command by its exact id.
    pub id: Option<&'a str>,
    pub enabled_only: bool,
    pub brief: bool,
}

/// `{"commands": [...]}`, as `engine.commands` and `command_list` give it.
pub fn command_list(session: &Session, query: &ListQuery) -> Value {
    let needle = query.filter.map(str::to_lowercase);
    let commands: Vec<Value> = docs(session)
        .iter()
        .filter(|d| query.id.is_none_or(|id| d.id == id))
        .filter(|d| {
            needle.as_deref().is_none_or(|n| {
                d.id.to_lowercase().contains(n) || d.label.to_lowercase().contains(n)
            })
        })
        .filter_map(|d| {
            let on = enabled(session, &d.id);
            (on.is_ok() || !query.enabled_only).then(|| d.to_json(query.brief, on))
        })
        .collect();
    json!({ "commands": commands })
}

/// The command ids nearest to a misspelt one.
fn suggest(wrong: &str, docs: &[CommandDoc]) -> String {
    fn distance(a: &str, b: &str) -> usize {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut prev = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let next = (prev + usize::from(ca != *cb))
                    .min(row[j] + 1)
                    .min(row[j + 1] + 1);
                prev = row[j + 1];
                row[j + 1] = next;
            }
        }
        row[b.len()]
    }
    let best = docs
        .iter()
        .map(|d| (distance(wrong, &d.id), &d.id))
        .min()
        .filter(|(d, _)| *d <= 3);
    match best {
        Some((_, id)) => format!("unknown command `{}`; did you mean `{id}`?", shorten(wrong)),
        None => format!(
            "unknown command `{}`; engine.commands (or the MCP tool command_list) lists them",
            shorten(wrong)
        ),
    }
}

// ---------------------------------------------------------------------------- running

/// Runs one command by id. Errors start with the command's id and name the parameter.
pub fn execute(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    if !matches!(params, Value::Null | Value::Object(_)) {
        return Err(format!(
            "{id}: params must be a JSON object, got {}",
            describe(params)
        ));
    }
    let empty = Value::Object(Map::new());
    let params = if params.is_null() { &empty } else { params };
    let result = match id {
        "file.new" => file_new(host, params),
        "file.open" => file_open(host, params),
        "file.save" => file_save(host, params),
        "edit.undo" | "edit.redo" => history(host, id, params),
        "document.inspect" => {
            let a = Fields::new(
                id,
                "parameter",
                Some(params),
                &["faces", "measures", "placements", "full"],
            )?;
            let v = Verbosity {
                faces: a.bool("faces")?.unwrap_or(true),
                measures: a.bool("measures")?.unwrap_or(true),
                placements: a.bool("placements")?.unwrap_or(false),
                full: a.bool("full")?.unwrap_or(false),
            };
            let (session, state) = host.parts();
            Ok(inspect::inspect(session, state, &v))
        }
        "document.measure" => {
            Fields::new(
                id,
                "parameter",
                Some(params),
                &["points", "outline", "path"],
            )?;
            let points = take::<Vec<[f32; 3]>>(id, params, "points")?;
            let outline = take::<Vec<[f32; 2]>>(id, params, "outline")?;
            let path = take::<Vec<[f32; 2]>>(id, params, "path")?;
            let (session, state) = host.parts();
            inspect::measure_json(session, state, points, outline, path)
                .map_err(|e| format!("{id}: {e}"))
        }
        "query.raycast" => {
            Fields::new(id, "parameter", Some(params), &["origin", "direction"])?;
            let origin = need::<[f32; 3]>(id, params, "origin")?;
            let dir = need::<[f32; 3]>(id, params, "direction")?;
            if dir.iter().all(|c| *c == 0.0) {
                return Err(format!("{id}: `direction` must not be [0, 0, 0]"));
            }
            let (session, state) = host.parts();
            Ok(inspect::raycast_json(session, state, origin, dir))
        }
        "query.ground_height" => {
            Fields::new(id, "parameter", Some(params), &["x", "z"])?;
            let (x, z) = (need::<f32>(id, params, "x")?, need::<f32>(id, params, "z")?);
            let (session, state) = host.parts();
            Ok(inspect::ground_json(session, state, x, z))
        }
        "query.sun" => {
            let a = Fields::new(id, "parameter", Some(params), &["time"])?;
            let project = host.session().project().clone();
            let time = match a.get("time") {
                Some(t) => Some(
                    with_time(&project, t)
                        .map_err(|e| format!("{id}: {e}"))?
                        .time,
                ),
                None => None,
            };
            Ok(inspect::sun_json(&project, time.as_ref()))
        }
        "catalog.search" => catalog_search(host, params),
        "species.list" => species_list(host, params),
        "planting.list_presets" => {
            Fields::new(id, "parameter", Some(params), &[])?;
            let presets: Vec<Value> = visioncraft_core::planting_presets()
                .iter()
                .map(|p| json!({"id": p.id, "name": p.name, "layers": p.layers.len()}))
                .collect();
            Ok(json!({ "presets": presets }))
        }
        "view.render" => view_render(host, params),
        _ => run_core(host, id, params),
    }?;
    Ok(result)
}

fn take<T: serde::de::DeserializeOwned>(
    id: &str,
    params: &Value,
    key: &str,
) -> Result<Option<T>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| format!("{id}: `{key}`: {e}")),
    }
}

fn need<T: serde::de::DeserializeOwned>(id: &str, params: &Value, key: &str) -> Result<T, String> {
    take(id, params, key)?.ok_or_else(|| format!("{id}: missing `{key}`"))
}

/// A core command: unknown keys refused (where the docs list them all), then `Session::run`.
fn run_core(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    let (session, state) = host.parts();
    let all = docs(session);
    let Some(doc) = all.iter().find(|d| d.id == id) else {
        return Err(suggest(id, &all));
    };
    if !doc.open
        && let Some(map) = params.as_object()
        && let Some(unknown) = map
            .keys()
            .find(|k| doc.params.iter().all(|p| &p.name != *k))
    {
        let names: Vec<&str> = doc.params.iter().map(|p| p.name.as_str()).collect();
        return Err(format!(
            "{id}: unknown parameter `{}`; its parameters are: {}",
            shorten(unknown),
            names.join(", ")
        ));
    }
    // A model whose file cannot be read is refused before it is added.
    let preloaded = if id == "model.add" {
        let kind: Option<ModelKind> = take(id, params, "kind")?;
        let source: Option<String> = take(id, params, "source")?;
        match (kind, source) {
            (Some(kind), Some(source)) => Some(
                state
                    .load(kind, &source)
                    .map_err(|e| format!("{id}: {e}"))?,
            ),
            _ => None,
        }
    } else {
        None
    };
    let result = session
        .run(id, params.clone())
        .map_err(|e| format!("{id}: {e}"))?;
    if let (Some(loaded), Some(new)) = (preloaded, result["id"].as_str()) {
        state.missing.remove(new);
        state.models.insert(new.to_string(), loaded);
    }
    state.sync_models(session.project());
    Ok(result)
}

fn history(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    Fields::new(id, "parameter", Some(params), &[])?;
    let (session, state) = host.parts();
    let done = if id == "edit.undo" {
        session.undo()
    } else {
        session.redo()
    };
    if !done {
        return Err(format!("{id}: nothing to {}", &id[5..]));
    }
    state.sync_models(session.project());
    Ok(json!({"can_undo": session.can_undo(), "can_redo": session.can_redo()}))
}

fn file_new(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    Fields::new("file.new", "parameter", Some(params), &[])?;
    let (session, state) = host.parts();
    *session = Session::new(Project::default());
    state.file = None;
    state.models.clear();
    state.missing.clear();
    state.saved = Some(Project::default());
    Ok(json!({"models": 0}))
}

fn file_open(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "file.open";
    let a = Fields::new(id, "parameter", Some(params), &["path"])?;
    let given = a.required_str("path")?;
    let (session, state) = host.parts();
    let path = state
        .access
        .resolve(given, false, None)
        .map_err(|e| format!("{id}: {e}"))?;
    let bytes =
        std::fs::read(&path).map_err(|e| format!("{id}: cannot read `{}`: {e}", path.display()))?;
    let project = Project::from_json_bytes(&bytes).map_err(|e| format!("{id}: `{given}`: {e}"))?;
    *session = Session::new(project.clone());
    state.file = Some(std::path::absolute(&path).unwrap_or(path));
    state.models.clear();
    state.missing.clear();
    state.saved = Some(project.clone());
    state.sync_models(&project);
    Ok(json!({
        "path": state.file.as_ref().map(|f| f.display().to_string()),
        "models": project.models.len(), "shapes": project.shapes.len(),
        "generators": project.generators.len(), "lights": project.lights.len(),
        "viewpoints": project.viewpoints.len(),
        "missingModels": missing_json(state, &project),
    }))
}

/// `target` relative to `folder` when it lies beneath it, else absolute.
fn source_from(folder: &Path, target: &Path) -> String {
    let target = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    target
        .strip_prefix(&folder)
        .unwrap_or(&target)
        .to_string_lossy()
        .replace('\\', "/")
}

fn file_save(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "file.save";
    let a = Fields::new(id, "parameter", Some(params), &["path", "overwrite"])?;
    let overwrite = a.bool("overwrite")?.unwrap_or(false);
    let (session, state) = host.parts();
    let target: PathBuf = match a.str("path")? {
        Some(p) => state
            .access
            .resolve(p, true, None)
            .map_err(|e| format!("{id}: {e}"))?,
        None => {
            let own = state
                .file
                .clone()
                .ok_or_else(|| format!("{id}: the project has no file yet; give `path`"))?;
            state
                .access
                .resolve(&own.to_string_lossy(), true, None)
                .map_err(|e| format!("{id}: {e}"))?
        }
    };
    let target = std::path::absolute(&target).unwrap_or(target);
    if target.exists() && state.file.as_ref() != Some(&target) && !overwrite {
        return Err(format!(
            "{id}: `{}` exists and is not this project's file; give overwrite: true to replace it",
            target.display()
        ));
    }
    let folder = target.parent().map(Path::to_path_buf).unwrap_or_default();
    // Model paths follow the file to its new folder.
    let mut project = session.project().clone();
    let mut moved = false;
    for m in &mut project.models {
        if let Some(l) = state.models.get_mut(&m.id) {
            let source = source_from(&folder, &l.path);
            if source != m.source {
                m.source = source.clone();
                l.source = source;
                moved = true;
            }
        }
    }
    let bytes = project.to_json_bytes();
    std::fs::write(&target, &bytes)
        .map_err(|e| format!("{id}: cannot write `{}`: {e}", target.display()))?;
    if moved {
        session.replace_project(project.clone());
    }
    state.file = Some(target.clone());
    state.saved = Some(project);
    Ok(json!({"path": target.display().to_string(), "bytes": bytes.len()}))
}

fn catalog_search(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "catalog.search";
    let a = Fields::new(
        id,
        "parameter",
        Some(params),
        &["query", "category", "exposure", "max_height_m", "limit"],
    )?;
    let category = take(id, params, "category")?;
    let exposure = take(id, params, "exposure")?;
    let max_h = a.number("max_height_m")?.map(|h| h as f32);
    let limit = a.integer("limit", 1, 500)?.unwrap_or(50) as usize;
    let state = host.state();
    let found = state
        .catalog
        .search(a.str("query")?.unwrap_or(""), category, exposure, max_h);
    let assets: Vec<Value> = found
        .iter()
        .take(limit)
        .map(|x| {
            json!({"id": x.id, "name": x.name, "category": x.category, "size_m": x.size_m,
                   "license": x.license, "author": x.author, "plant": x.plant})
        })
        .collect();
    Ok(json!({"total": found.len(), "assets": assets}))
}

fn snake(debug: impl std::fmt::Debug) -> String {
    let mut out = String::new();
    for (i, c) in format!("{debug:?}").chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn species_list(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "species.list";
    let a = Fields::new(id, "parameter", Some(params), &["filter", "category"])?;
    let filter = a.str("filter")?.map(str::to_lowercase);
    let category = a.str("category")?;
    let matches = |texts: &[&str]| {
        filter
            .as_deref()
            .is_none_or(|f| texts.iter().any(|t| t.to_lowercase().contains(f)))
    };
    let species: Vec<Value> = visioncraft_flora::species()
        .iter()
        .filter(|s| category.is_none_or(|c| snake(s.category) == c))
        .filter(|s| matches(&[s.id, s.common_name, s.botanical_name]))
        .map(|s| {
            json!({"asset": format!("flora:{}", s.id), "common_name": s.common_name,
                   "botanical_name": s.botanical_name, "category": snake(s.category),
                   "mature_height_m": s.mature_height_m, "mature_width_m": s.mature_width_m,
                   "evergreen": s.evergreen, "bloom_months": s.bloom_months, "exposure": snake(s.exposure)})
        })
        .collect();
    let custom: Vec<Value> = host
        .session()
        .project()
        .species
        .iter()
        .filter(|s| matches(&[&s.id, &s.name]))
        .map(|s| json!({"asset": s.id, "name": s.name, "base": s.base}))
        .collect();
    Ok(json!({"species": species, "custom": custom}))
}

/// A copy of `project` with the fields of `time` changed, checked as `time.set` checks them.
fn with_time(project: &Project, time: &Value) -> Result<Project, String> {
    let mut scratch = Session::new(project.clone());
    scratch
        .run("time.set", time.clone())
        .map_err(|e| format!("`time`: {e}"))?;
    Ok(scratch.project().clone())
}

fn view_render(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "view.render";
    let a = Fields::new(
        id,
        "parameter",
        Some(params),
        &[
            "path",
            "width",
            "height",
            "viewpoint",
            "camera",
            "time",
            "quality",
        ],
    )?;
    let width = a.integer("width", 16, 4096)?.unwrap_or(800) as u32;
    let height = a.integer("height", 16, 4096)?.unwrap_or(600) as u32;
    let quality = a.str("quality")?.unwrap_or("high");
    if !["low", "medium", "high"].contains(&quality) {
        return Err(format!(
            "{id}: `quality` must be low, medium or high, got `{}`",
            shorten(quality)
        ));
    }
    let mut project = host.session().project().clone();
    let mut camera = None;
    if let Some(v) = a.str("viewpoint")? {
        let vp = project
            .viewpoints
            .iter()
            .find(|x| x.id == v || x.name == v)
            .ok_or_else(|| {
                let names: Vec<String> = project
                    .viewpoints
                    .iter()
                    .map(|x| format!("{} ({})", x.id, x.name))
                    .collect();
                format!(
                    "{id}: no viewpoint `{}`; the viewpoints are: {}",
                    shorten(v),
                    if names.is_empty() {
                        "none".into()
                    } else {
                        names.join(", ")
                    }
                )
            })?
            .clone();
        camera = Some((vp.position, vp.target, vp.fov_y_deg));
        if let Some(t) = vp.time {
            project.time = t;
        }
    }
    if let Some(c) = a.get("camera") {
        let pos = need::<[f32; 3]>(id, c, "position")?;
        let target = need::<[f32; 3]>(id, c, "target")?;
        let fov = take::<f32>(id, c, "fov_y_deg")?.unwrap_or(50.0);
        if pos == target || !(1.0..=179.0).contains(&fov) {
            return Err(format!(
                "{id}: `camera` needs position and target apart and fov_y_deg from 1 to 179"
            ));
        }
        camera = Some((pos, target, fov));
    }
    if let Some(t) = a.get("time") {
        project = with_time(&project, t).map_err(|e| format!("{id}: {e}"))?;
    }
    let path = match a.str("path")? {
        Some(p) => Some(
            host.state()
                .access
                .resolve(p, true, None)
                .map_err(|e| format!("{id}: {e}"))?,
        ),
        None => None,
    };
    let request = RenderRequest {
        width,
        height,
        camera,
        quality: quality.to_string(),
    };
    let models = host.state().models.clone();
    let rendered = host
        .render(&project, &models, &request)
        .map_err(|e| format!("{id}: {e}"))?;
    let mut out = match rendered.info {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    out.insert("width".into(), width.into());
    out.insert("height".into(), height.into());
    out.insert("time".into(), json!(project.time));
    out.insert("sun".into(), inspect::sun_json(&project, None));
    match path {
        Some(path) => {
            std::fs::write(&path, &rendered.png)
                .map_err(|e| format!("{id}: cannot write `{}`: {e}", path.display()))?;
            out.insert("path".into(), path.display().to_string().into());
            out.insert("bytes".into(), rendered.png.len().into());
        }
        None => {
            out.insert("mime".into(), "image/png".into());
            out.insert("base64".into(), crate::base64(&rendered.png).into());
        }
    }
    Ok(Value::Object(out))
}

// ---------------------------------------------------------------------------- batches

/// One step of a batch.
pub struct Step {
    pub command: String,
    pub params: Value,
}

/// Reads and checks every step before any runs. `id_key` is what the protocol calls the
/// command in a step (`command` in JSON lines, `id` in MCP).
pub(crate) fn parse_steps(
    owner: &str,
    noun: &str,
    fields: &Fields,
    id_key: &str,
) -> Result<Vec<Step>, String> {
    let items = fields
        .array("steps")?
        .ok_or_else(|| fields.missing("steps"))?;
    if items.len() > MAX_BATCH_STEPS {
        return Err(format!(
            "{owner}: `steps` has {} steps, and a batch takes at most {MAX_BATCH_STEPS}; split it into several batches",
            items.len()
        ));
    }
    let mut steps = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let step_owner = format!("{owner}: steps[{index}]");
        if !item.is_object() {
            return Err(format!(
                "{step_owner} must be an object {{\"{id_key}\": string, \"params\": object}}, got {}",
                describe(item)
            ));
        }
        let step = Fields::new(&step_owner, noun, Some(item), &[id_key, "params"])?;
        steps.push(Step {
            command: step.required_str(id_key)?.to_string(),
            params: step.object("params")?,
        });
    }
    Ok(steps)
}

/// The outcome of a batch: the report both protocols return, and whether it failed.
pub struct BatchOutcome {
    pub report: Value,
    pub failed: bool,
}

/// Runs the steps in order, all or nothing: on a scratch copy of the project, so that the
/// first step that fails leaves the project, its history and the open file as they were
/// (`rolledBack: true`). A batch that succeeds and changes the project is one undo step. Files
/// written by a step (`file.save`, `view.render` with a path) stay written.
pub fn run_batch(host: &mut dyn Host, steps: &[Step], terse: bool) -> BatchOutcome {
    let whole = Stopwatch::start();
    let (session, state) = host.parts();
    let scratch = Session::new(session.project().clone());
    let original = std::mem::replace(session, scratch);
    let saved_state = (
        state.file.clone(),
        state.models.clone(),
        state.missing.clone(),
        state.saved.clone(),
    );
    let (mut results, mut errors, mut last) = (Vec::new(), Vec::new(), Value::Null);
    let mut completed = 0usize;
    let mut failed = false;
    for (index, step) in steps.iter().enumerate() {
        let clock = Stopwatch::start();
        // A panic must not leave the scratch session in place of the real one.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute(host, &step.command, &step.params)
        }))
        .unwrap_or_else(|payload| {
            Err(format!(
                "{}: internal error (a bug): {}",
                step.command,
                crate::rpc::panic_text(payload.as_ref())
            ))
        });
        let ms = clock.ms();
        match outcome {
            Ok(result) => {
                completed += 1;
                if terse {
                    last = json!({"step": index, "result": result, "ms": ms});
                } else {
                    results.push(json!({"ok": true, "result": result, "ms": ms}));
                }
            }
            Err(error) => {
                failed = true;
                let error = format!("step {index}: {error}");
                if terse {
                    errors.push(json!({"step": index, "error": error, "ms": ms}));
                } else {
                    results.push(json!({"ok": false, "error": error, "ms": ms}));
                }
                break;
            }
        }
    }
    let (session, state) = host.parts();
    let done = std::mem::replace(session, original);
    if failed {
        (state.file, state.models, state.missing, state.saved) = saved_state;
    } else if done.project() != session.project() {
        session.replace_project(done.project().clone());
    }
    let mut report =
        json!({"completed": completed, "failed": usize::from(failed), "rolledBack": failed});
    if terse {
        report["errors"] = errors.into();
        report["last"] = last;
    } else {
        report["results"] = results.into();
    }
    report["ms"] = whole.ms();
    BatchOutcome { report, failed }
}
