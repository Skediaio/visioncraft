//! A project being edited: every change is a named command, which gives undo/redo now and
//! scripting later.

use crate::project::{ModelKind, ModelRef, Project, Transform};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;

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
    pub name: String,
    /// One line.
    pub description: String,
    pub handler: Handler,
}

pub struct Session {
    project: Project,
    commands: BTreeMap<String, Command>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    revision: u64,
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
        };
        s.register(
            "model.add",
            "Add a model: {name, kind: 3dcraft|obj, source, id?, transform?, layer?}; returns {id}",
            model_add,
        );
        s.register("model.remove", "Remove a model: {id}", model_remove);
        s.register(
            "model.set_transform",
            "Move, turn or scale a model: {id, translation?, rotation_y_deg?, scale?}",
            model_set_transform,
        );
        s.register(
            "site.set",
            "Set the site: {latitude?, longitude?, utc_offset_hours?}",
            site_set,
        );
        s.register(
            "time.set",
            "Set the local date and time: {year?, month?, day?, hour?}",
            time_set,
        );
        crate::commands::register(&mut s);
        s
    }

    /// Adds or replaces a command; extensions use this to add their own.
    pub fn register(
        &mut self,
        name: &str,
        description: &str,
        handler: impl Fn(&mut Project, &Value) -> Result<Value, CommandError> + 'static,
    ) {
        self.commands.insert(
            name.to_string(),
            Command {
                name: name.to_string(),
                description: description.to_string(),
                handler: Box::new(handler),
            },
        );
    }

    /// Names and one-line descriptions, sorted by name.
    pub fn commands(&self) -> Vec<(&str, &str)> {
        self.commands
            .values()
            .map(|c| (c.name.as_str(), c.description.as_str()))
            .collect()
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Increases on every change of the project (run, undo, redo): compare it to know when to redraw.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Runs a command. On success it is undoable; on failure the project and both stacks are as before.
    pub fn run(&mut self, name: &str, params: Value) -> Result<Value, CommandError> {
        let command = self
            .commands
            .get(name)
            .ok_or_else(|| CommandError::Unknown(name.to_string()))?;
        // shortcut: snapshot undo clones the whole project, switch to inverse commands if projects grow past a few MB
        let before = self.project.clone();
        match (command.handler)(&mut self.project, &params) {
            Ok(result) => {
                self.undo.push(before);
                if self.undo.len() > UNDO_LIMIT {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.revision += 1;
                Ok(result)
            }
            Err(e) => {
                self.project = before; // the handler may have changed it before failing
                Err(e)
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Returns false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.project, previous));
        self.revision += 1;
        true
    }

    /// Returns false when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
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
