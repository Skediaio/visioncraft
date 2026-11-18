//! Films: shots of camera moves (`film.*`). `film.preview` and `film.render` are the host's,
//! as they need the ground the camera walks on and the renderer.

use super::{fail, next_id};
use crate::film::{Film, SHOT_KINDS, Shot};
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

const SHOT_DOC: &str = "A shot {kind, ...}. Kinds: `walk` {path [[x, z]...], eye_height_m = 1.6} (the eye follows the ground, looks ahead; speed_mps = 1.2 unless duration_s); `glide` {path [[x, y, z]...]} (a smooth curve through the points; speed_mps = 2); `rise` {at [x, z], from_height, to_height, look_at [x, y, z] required} (heights above the ground there; duration_s = 4); `orbit` {center [x, y, z], radius_m, height_m (above the centre's y), from_deg, to_deg} (angle from +X, positive counter-clockwise seen from above like rotation_y_deg; may pass 360; duration_s = 4, looks at the centre); `flyover` {path [[x, z]...], height_m (above the smoothed ground), pitch_deg = 35} (a drone looking ahead and down; speed_mps = 8); `hold` {viewpoint id | eye + target, duration_s = 4}. Any shot: duration_s or speed_mps (not both), ease in | out | in_out (default) | none, look_at [x, y, z] | \"ahead\", fov_y_deg = 60, and changes along the shot: time_of_day h or [from, to], date {month, day} or [from, to], season (not with date), years n or [from, to], snow 0..1 or [from, to], wetness 0..1 or [from, to], rain 0..1 or [from, to].";

pub(super) fn register(s: &mut Session) {
    let id = || param("id", T::String, "Film id.").required();
    s.register_with_spec(
        spec(
            "film.add",
            "Add Film",
            "Starts a film: an empty list of shots, played one after the other, each with its own camera. Add shots with film.add_shot, check them with film.preview and make the frames with film.render or `visioncraft-cli film`.",
            vec![
                param("name", T::String, "Display name.").required(),
                param("fps", T::Integer, "Frames per second, 1..120.").default(json!(25)),
            ],
            "{id} (f1, f2...)",
            json!({"name": "Garden tour"}),
        ),
        add,
    );
    s.register_with_spec(
        spec(
            "film.update",
            "Change Film",
            "Changes a film's name or frame rate.",
            vec![
                id(),
                param("name", T::String, "Display name."),
                param("fps", T::Integer, "Frames per second, 1..120."),
            ],
            "null",
            json!({"id": "f1", "fps": 30}),
        ),
        update,
    );
    s.register_with_spec(
        spec(
            "film.add_shot",
            "Add Shot",
            "Adds a shot to a film, at the end or at `index`. A walk needs the ground it walks on, so its length in time is known only to film.preview.",
            vec![
                id(),
                param("shot", T::Object, SHOT_DOC).required(),
                param("index", T::Integer, "Where in the list, from 0 (the end by default)."),
            ],
            "{index, shots}",
            json!({"id": "f1", "shot": {"kind": "orbit", "center": [5, 0, 5], "radius_m": 6, "height_m": 3, "from_deg": 0, "to_deg": 180, "duration_s": 6, "time_of_day": [17, 18.5]}}),
        ),
        add_shot,
    );
    s.register_with_spec(
        spec(
            "film.update_shot",
            "Change Shot",
            "Changes fields of a shot: the fields in `shot` replace those it has (null removes an optional one; a new `duration_s` replaces a `speed_mps` and the other way round); the kind stays, to change it remove the shot and add another.",
            vec![
                id(),
                param("index", T::Integer, "The shot's place in the film, from 0.").required(),
                param("shot", T::Object, "The fields to change, as film.add_shot lists them.").required(),
            ],
            "null",
            json!({"id": "f1", "index": 0, "shot": {"duration_s": 8, "ease": "none"}}),
        ),
        update_shot,
    );
    s.register_with_spec(
        spec(
            "film.remove_shot",
            "Remove Shot",
            "Removes a shot from a film.",
            vec![
                id(),
                param("index", T::Integer, "The shot's place in the film, from 0.").required(),
            ],
            "null",
            json!({"id": "f1", "index": 0}),
        ),
        remove_shot,
    );
    s.register_with_spec(
        spec(
            "film.remove",
            "Remove Film",
            "Removes a film (its rendered frames stay where they are).",
            vec![id()],
            "null",
            json!({"id": "f1"}),
        ),
        remove,
    );
}

fn find<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut Film, CommandError> {
    let ids: Vec<String> = project.films.iter().map(|f| f.id.clone()).collect();
    project
        .films
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| {
            fail(format!(
                "no film `{id}`; the films are: {}",
                if ids.is_empty() {
                    "none".into()
                } else {
                    ids.join(", ")
                }
            ))
        })
}

fn shot_of(value: &Value, latitude: f64) -> Result<Shot, CommandError> {
    Shot::from_value(value, latitude).map_err(|e| bad(&e))
}

fn add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        fps: Option<u32>,
    }
    let p: P = parse(params)?;
    let id = next_id("f", |id| project.films.iter().any(|f| f.id == id));
    let film = Film::new(id.clone(), p.name, p.fps.unwrap_or(25));
    film.validate().map_err(|e| bad(&e))?;
    project.films.push(film);
    Ok(json!({ "id": id }))
}

fn update(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: Option<String>,
        fps: Option<u32>,
    }
    let p: P = parse(params)?;
    let film = find(project, &p.id)?;
    let mut next = film.clone();
    if let Some(n) = p.name {
        next.name = n;
    }
    if let Some(f) = p.fps {
        next.fps = f;
    }
    next.validate().map_err(|e| bad(&e))?;
    *film = next;
    Ok(Value::Null)
}

fn add_shot(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        shot: Value,
        index: Option<usize>,
    }
    let p: P = parse(params)?;
    let latitude = project.site.latitude;
    let shot = shot_of(&p.shot, latitude)?;
    let film = find(project, &p.id)?;
    let index = p.index.unwrap_or(film.shots.len());
    if index > film.shots.len() {
        return Err(bad(&format!(
            "`index` {index} is past the end: the film has {} shots",
            film.shots.len()
        )));
    }
    film.shots.insert(index, shot);
    Ok(json!({"index": index, "shots": film.shots.len()}))
}

fn shot_index(film: &Film, index: usize) -> Result<usize, CommandError> {
    if index < film.shots.len() {
        Ok(index)
    } else {
        Err(bad(&format!(
            "no shot {index}: the film `{}` has {} shots (from 0)",
            film.id,
            film.shots.len()
        )))
    }
}

fn update_shot(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        index: usize,
        shot: serde_json::Map<String, Value>,
    }
    let p: P = parse(params)?;
    let latitude = project.site.latitude;
    let film = find(project, &p.id)?;
    let i = shot_index(film, p.index)?;
    let mut value = serde_json::to_value(&film.shots[i]).expect("a shot serialises");
    let fields = value.as_object_mut().expect("a shot is an object");
    // A new duration replaces a speed, and the other way round: they are two ways to say one thing.
    if p.shot.get("duration_s").is_some_and(|v| !v.is_null()) && !p.shot.contains_key("speed_mps") {
        fields.remove("speed_mps");
    }
    if p.shot.get("speed_mps").is_some_and(|v| !v.is_null()) && !p.shot.contains_key("duration_s") {
        fields.remove("duration_s");
    }
    for (k, v) in p.shot {
        if k == "kind" && v.as_str() != fields.get("kind").and_then(Value::as_str) {
            return Err(bad(&format!(
                "a shot keeps its kind (`{}`; the kinds are {}); remove it and add another",
                fields["kind"].as_str().unwrap_or_default(),
                SHOT_KINDS.join(", ")
            )));
        }
        if v.is_null() {
            fields.remove(&k);
        } else {
            fields.insert(k, v);
        }
    }
    film.shots[i] = shot_of(&value, latitude)?;
    Ok(Value::Null)
}

fn remove_shot(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        index: usize,
    }
    let p: P = parse(params)?;
    let film = find(project, &p.id)?;
    let i = shot_index(film, p.index)?;
    film.shots.remove(i);
    Ok(Value::Null)
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    find(project, id)?;
    project.films.retain(|f| f.id != id);
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_is_checked() {
        let mut p = Project::default();
        let err = add(
            &mut p,
            &json!({"name": "x", "fps": crate::film::FPS_MAX + 1}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("`fps`"), "{err}");
    }
}
