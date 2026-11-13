//! A project being edited: every change is a named command, which gives undo/redo now and
//! scripting later.

use crate::project::{ModelKind, ModelRef, Project, Transform};
use crate::spec::{CommandSpec, ParamType, param, spec};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

const UNDO_LIMIT: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub enum CommandError {
    Unknown(String),
    BadParams(String),
    Failed(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(f, "unknown command `{name}`"),
            Self::BadParams(why) => write!(f, "bad parameters: {why}"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for CommandError {}

pub type Handler = Box<dyn Fn(&mut Project, &Value) -> Result<Value, CommandError>>;

pub struct Command {
    pub spec: CommandSpec,
    pub handler: Handler,
}

pub struct Session {
    project: Project,
    commands: BTreeMap<String, Command>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    revision: u64,
    /// Inside a transaction: nothing is pushed on the undo stack until it ends.
    in_transaction: bool,
}

/// Whether two projects are equal; the terrain is compared by pointer first, since a command that
/// did not touch it leaves the very same `Arc`.
fn same_project(a: &mut Project, b: &mut Project) -> bool {
    let (ta, tb) = (a.terrain.take(), b.terrain.take());
    let terrain = match (&ta, &tb) {
        (Some(x), Some(y)) => Arc::ptr_eq(x, y) || x == y,
        (None, None) => true,
        _ => false,
    };
    let same = terrain && a == b;
    a.terrain = ta;
    b.terrain = tb;
    same
}

impl Session {
    /// A session with the built-in commands registered.
    pub fn new(project: Project) -> Self {
        let mut s = Self {
            project,
            commands: BTreeMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            in_transaction: false,
        };
        s.register_with_spec(
            spec(
                "model.add",
                "Add Model",
                "Adds a model (a CAD file or OBJ the app loads from `source`) to the project.",
                vec![
                    param("name", ParamType::String, "Display name.").required(),
                    param("kind", ParamType::String, "File format of the source.")
                        .required()
                        .allowed(&["3dcraft", "obj"]),
                    param("source", ParamType::String, "A path relative to the project file, or a URL.").required(),
                    param("id", ParamType::String, "Id to use; by default the next free `m1`, `m2`..."),
                    param("transform", ParamType::Object, "Placement {translation: [x, y, z] m, rotation_y_deg, scale}; any part may be left out. Scale first, then rotation about +Y, then translation.")
                        .default(json!({"translation": [0, 0, 0], "rotation_y_deg": 0, "scale": 1})),
                    param("layer", ParamType::String, "Layer id.").default(json!("model")),
                ],
                "{id}",
                json!({"name": "House", "kind": "obj", "source": "house.obj"}),
            ),
            model_add,
        );
        s.register_with_spec(
            spec(
                "model.remove",
                "Remove Model",
                "Removes a model and the material assignments made to it.",
                vec![param("id", ParamType::String, "Model id.").required()],
                "null",
                json!({"id": "m1"}),
            ),
            model_remove,
        );
        s.register_with_spec(
            spec(
                "model.set_transform",
                "Move Model",
                "Moves, turns or scales a model; only the parts given change.",
                vec![
                    param("id", ParamType::String, "Model id.").required(),
                    param("translation", ParamType::Point3, "Position of the model's origin, [x, y, z] (+X east, +Y up, -Z north).").unit("m"),
                    param("rotation_y_deg", ParamType::Number, "Turn about +Y, counter-clockwise seen from above (east turns towards north).").unit("deg"),
                    param("scale", ParamType::Number, "Uniform scale, above zero.").default(json!(1)),
                ],
                "null",
                json!({"id": "m1", "translation": [2, 0, -3], "rotation_y_deg": 90}),
            ),
            model_set_transform,
        );
        s.register_with_spec(
            spec(
                "site.set",
                "Set Site",
                "Sets where the project stands on Earth, which decides the sun.",
                vec![
                    param("latitude", ParamType::Number, "North positive, -90..90.").unit("deg"),
                    param("longitude", ParamType::Number, "East positive, -180..180.").unit("deg"),
                    param(
                        "utc_offset_hours",
                        ParamType::Number,
                        "Hours east of UTC that the project's time is given in, -14..14.",
                    )
                    .unit("h"),
                ],
                "null",
                json!({"latitude": 50.85, "longitude": 4.35, "utc_offset_hours": 1}),
            ),
            site_set,
        );
        s.register_with_spec(
            spec(
                "time.set",
                "Set Time",
                "Sets the local date and time at the site, for the sun and the seasons (the month drives plant foliage).",
                vec![
                    param("year", ParamType::Integer, "Gregorian year."),
                    param("month", ParamType::Integer, "1..12."),
                    param("day", ParamType::Integer, "Day of the month, valid for the month."),
                    param("hour", ParamType::Number, "Local time of day, 0 <= hour < 24.").unit("h"),
                ],
                "null",
                json!({"month": 6, "day": 21, "hour": 20.5}),
            ),
            time_set,
        );
        crate::commands::register(&mut s);
        s
    }

    /// Adds or replaces a command that has no spec of its own: it gets a minimal open one (any
    /// parameters pass, nothing is documented but the description).
    pub fn register(
        &mut self,
        name: &str,
        description: &str,
        handler: impl Fn(&mut Project, &Value) -> Result<Value, CommandError> + 'static,
    ) {
        self.register_with_spec(CommandSpec::minimal(name, description), handler);
    }

    /// Adds or replaces a command with its full spec; parameters are checked against it before
    /// the handler runs.
    pub fn register_with_spec(
        &mut self,
        spec: CommandSpec,
        handler: impl Fn(&mut Project, &Value) -> Result<Value, CommandError> + 'static,
    ) {
        self.commands.insert(
            spec.name.clone(),
            Command {
                spec,
                handler: Box::new(handler),
            },
        );
    }

    /// Names and one-line descriptions, sorted by name.
    pub fn commands(&self) -> Vec<(&str, &str)> {
        self.commands
            .values()
            .map(|c| (c.spec.name.as_str(), c.spec.summary.as_str()))
            .collect()
    }

    /// The specs of all commands, sorted by name.
    pub fn command_specs(&self) -> Vec<&CommandSpec> {
        self.commands.values().map(|c| &c.spec).collect()
    }

    pub fn command_spec(&self, name: &str) -> Option<&CommandSpec> {
        self.commands.get(name).map(|c| &c.spec)
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Increases on every change of the project (run, undo, redo): compare it to know when to redraw.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Runs a command. Parameters are checked against its spec first. On success that changed the
    /// project it is one undo step; a command that left the project as it was adds no step and
    /// does not bump the revision. On failure the project and both stacks are as before. A query
    /// (`undoable: false`) never touches the project or the history.
    pub fn run(&mut self, name: &str, params: Value) -> Result<Value, CommandError> {
        let command = self
            .commands
            .get(name)
            .ok_or_else(|| CommandError::Unknown(name.to_string()))?;
        command
            .spec
            .check(&params)
            .map_err(CommandError::BadParams)?;
        if !command.spec.undoable {
            return (command.handler)(&mut self.project.clone(), &params);
        }
        // shortcut: snapshot undo clones the whole project (the terrain is shared), switch to inverse commands if projects grow past a few MB
        let mut before = self.project.clone();
        match (command.handler)(&mut self.project, &params) {
            Ok(result) => {
                if same_project(&mut before, &mut self.project) {
                    return Ok(result);
                }
                if !self.in_transaction {
                    self.push_undo(before);
                    self.redo.clear();
                    self.revision += 1;
                }
                Ok(result)
            }
            Err(e) => {
                self.project = before; // the handler may have changed it before failing
                Err(e)
            }
        }
    }

    fn push_undo(&mut self, before: Project) {
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Runs `f`, which may run any commands (including ones registered by extensions); all of
    /// them together are one undo step, or none if they changed nothing. If `f` fails, the
    /// project, both stacks and the revision are exactly as they were. A transaction inside a
    /// transaction joins the outer one. `undo` and `redo` do nothing while one runs.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut Session) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        if self.in_transaction {
            return f(self);
        }
        let mut before = self.project.clone();
        self.in_transaction = true;
        let outcome = f(self);
        self.in_transaction = false;
        match outcome {
            Ok(value) => {
                if !same_project(&mut before, &mut self.project) {
                    self.push_undo(before);
                    self.redo.clear();
                    self.revision += 1;
                }
                Ok(value)
            }
            Err(e) => {
                self.project = before;
                Err(e)
            }
        }
    }

    /// Swaps in another project ("Open", "New") as one undoable step.
    pub fn replace_project(&mut self, project: Project) {
        let old = std::mem::replace(&mut self.project, project);
        if !self.in_transaction {
            self.push_undo(old);
            self.redo.clear();
            self.revision += 1;
        }
    }

    /// Forgets undo and redo (the project stays): what the app did by itself while opening a
    /// project is not something to undo.
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Returns false when there is nothing to undo (or a transaction is running).
    pub fn undo(&mut self) -> bool {
        if self.in_transaction {
            return false;
        }
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.project, previous));
        self.revision += 1;
        true
    }

    /// Returns false when there is nothing to redo (or a transaction is running).
    pub fn redo(&mut self) -> bool {
        if self.in_transaction {
            return false;
        }
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.project, next));
        self.revision += 1;
        true
    }
}

pub(crate) fn parse<T: DeserializeOwned>(params: &Value) -> Result<T, CommandError> {
    serde_json::from_value(params.clone()).map_err(|e| CommandError::BadParams(e.to_string()))
}

pub(crate) fn bad(why: &str) -> CommandError {
    CommandError::BadParams(why.to_string())
}

fn check_transform(t: &Transform) -> Result<(), CommandError> {
    let finite = t.translation.iter().all(|c| c.is_finite()) && t.rotation_y_deg.is_finite();
    if !finite || !(t.scale.is_finite() && t.scale > 0.0) {
        return Err(bad("transform must be finite with scale above zero"));
    }
    Ok(())
}

fn model_add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: Option<String>,
        name: String,
        kind: ModelKind,
        source: String,
        #[serde(default)]
        transform: Transform,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    let layer = p
        .layer
        .unwrap_or_else(|| crate::project::LAYER_MODEL.into());
    if project.layers.iter().all(|l| l.id != layer) {
        return Err(CommandError::Failed(format!("no layer `{layer}`")));
    }
    check_transform(&p.transform)?;
    let id = match p.id {
        Some(id) => id,
        None => (1..)
            .map(|n| format!("m{n}"))
            .find(|id| project.models.iter().all(|m| &m.id != id))
            .expect("unbounded range"),
    };
    if project.models.iter().any(|m| m.id == id) {
        return Err(CommandError::Failed(format!("model `{id}` already exists")));
    }
    project.models.push(ModelRef {
        id: id.clone(),
        name: p.name,
        kind: p.kind,
        source: p.source,
        transform: p.transform,
        layer,
        extensions: BTreeMap::new(),
    });
    Ok(json!({ "id": id }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdOnly {
    id: String,
}

fn find<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut ModelRef, CommandError> {
    project
        .models
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| CommandError::Failed(format!("no model `{id}`")))
}

fn model_remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let IdOnly { id } = parse(params)?;
    find(project, &id)?;
    project.models.retain(|m| m.id != id);
    project.assignments.retain(|a| a.target.owner() != id);
    Ok(Value::Null)
}

fn model_set_transform(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        translation: Option<[f32; 3]>,
        rotation_y_deg: Option<f32>,
        scale: Option<f32>,
    }
    let p: P = parse(params)?;
    let model = find(project, &p.id)?;
    let mut t = model.transform;
    t.translation = p.translation.unwrap_or(t.translation);
    t.rotation_y_deg = p.rotation_y_deg.unwrap_or(t.rotation_y_deg);
    t.scale = p.scale.unwrap_or(t.scale);
    check_transform(&t)?;
    model.transform = t;
    Ok(Value::Null)
}

fn site_set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        latitude: Option<f64>,
        longitude: Option<f64>,
        utc_offset_hours: Option<f64>,
    }
    let p: P = parse(params)?;
    let mut s = project.site;
    s.latitude = p.latitude.unwrap_or(s.latitude);
    s.longitude = p.longitude.unwrap_or(s.longitude);
    s.utc_offset_hours = p.utc_offset_hours.unwrap_or(s.utc_offset_hours);
    if !(-90.0..=90.0).contains(&s.latitude)
        || !(-180.0..=180.0).contains(&s.longitude)
        || !(-14.0..=14.0).contains(&s.utc_offset_hours)
    {
        return Err(bad("latitude, longitude or UTC offset out of range"));
    }
    project.site = s;
    Ok(Value::Null)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn time_set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        year: Option<i32>,
        month: Option<u32>,
        day: Option<u32>,
        hour: Option<f32>,
    }
    let p: P = parse(params)?;
    let mut t = project.time;
    t.year = p.year.unwrap_or(t.year);
    t.month = p.month.unwrap_or(t.month);
    t.day = p.day.unwrap_or(t.day);
    t.hour = p.hour.unwrap_or(t.hour);
    if !(1..=12).contains(&t.month)
        || t.day < 1
        || t.day > days_in_month(t.year, t.month)
        || !(0.0..24.0).contains(&t.hour)
    {
        return Err(bad("not a valid date and local time"));
    }
    project.time = t;
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(s: &mut Session) -> String {
        let r = s
            .run(
                "model.add",
                json!({"name": "House", "kind": "3dcraft", "source": "h.3dcraft"}),
            )
            .unwrap();
        r["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn run_undo_redo_give_equal_projects() {
        let mut s = Session::new(Project::default());
        let start = s.project().clone();
        let id = add(&mut s);
        s.run(
            "model.set_transform",
            json!({"id": id, "scale": 3.0, "translation": [1,2,3]}),
        )
        .unwrap();
        s.run("time.set", json!({"hour": 9.5})).unwrap();
        let end = s.project().clone();
        assert_ne!(start, end);
        let rev = s.revision();
        while s.undo() {}
        assert_eq!(*s.project(), start);
        assert!(s.can_redo() && !s.can_undo());
        while s.redo() {}
        assert_eq!(*s.project(), end);
        assert!(s.revision() > rev);
    }

    #[test]
    fn failing_command_changes_nothing() {
        let mut s = Session::new(Project::default());
        let id = add(&mut s);
        s.undo();
        s.redo();
        let (project, rev) = (s.project().clone(), s.revision());
        assert!(s.run("model.remove", json!({"id": "nope"})).is_err());
        assert!(
            s.run("model.set_transform", json!({"id": id, "scale": -1.0}))
                .is_err()
        );
        assert!(s.run("time.set", json!({"month": 2, "day": 30})).is_err());
        assert!(matches!(
            s.run("nope", json!({})),
            Err(CommandError::Unknown(_))
        ));
        assert_eq!(*s.project(), project);
        assert_eq!(s.revision(), rev);
        assert!(s.can_undo() && !s.can_redo());
    }

    #[test]
    fn custom_command_works_and_undoes() {
        let mut s = Session::new(Project::default());
        s.register("ext.tag", "Store a tag", |p, params| {
            p.extensions.insert("test.tag".into(), params.clone());
            Ok(json!("done"))
        });
        assert!(
            s.commands()
                .iter()
                .any(|(n, d)| *n == "ext.tag" && *d == "Store a tag")
        );
        assert_eq!(s.run("ext.tag", json!({"a": 1})).unwrap(), json!("done"));
        assert_eq!(s.project().extensions["test.tag"], json!({"a": 1}));
        assert!(s.undo());
        assert!(s.project().extensions.is_empty());
    }

    #[test]
    fn undo_stack_is_capped() {
        let mut s = Session::new(Project::default());
        for h in 0..150 {
            s.run("time.set", json!({"hour": (h % 24) as f32})).unwrap();
        }
        let mut n = 0;
        while s.undo() {
            n += 1;
        }
        assert_eq!(n, UNDO_LIMIT);
    }
}
