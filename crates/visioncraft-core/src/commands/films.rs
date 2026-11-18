//! Films: shots of camera moves (`film.*`). `film.preview` and `film.render` are the host's,
//! as they need the ground the camera walks on and the renderer.

use super::{fail, next_id};
#[cfg(test)]
use crate::film::ShotKind;
use crate::film::{Film, SHOT_KINDS, Shot};
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Map, Value, json};

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
    s.register_with_spec(
        spec(
            "film.orbit",
            "Orbit Film",
            "Builds a film of one orbit shot: the camera circles `center` at `radius` metres, `height` metres above the centre's y, looking at it. Starts a new film, or with `id` adds the shot to that film. Time, weather and ease go along as in film.add_shot.",
            vec![
                param("center", T::Point3, "The point the camera circles and looks at.").required().unit("m"),
                param("radius", T::Number, "Distance from the centre on the ground plane.").required().unit("m"),
                param("height", T::Number, "Eye height above the centre's y.").unit("m").default(json!(3)),
                param("seconds", T::Number, "Length of the shot.").unit("s").default(json!(8)),
                param("from_deg", T::Number, "Start angle from +X, counter-clockwise seen from above.").unit("deg").default(json!(0)),
                param("to_deg", T::Number, "End angle; 360 more than from_deg is one full turn.").unit("deg").default(json!(360)),
                param("name", T::String, "Name of a new film.").default(json!("Orbit")),
                param("id", T::String, "Add the shot to this film instead of starting one."),
                param("fps", T::Integer, "Frames per second of a new film, 1..120.").default(json!(25)),
                param("extras", T::Object, "More shot fields: ease, fov_y_deg, time_of_day, date, season, rain, mist, shafts...."),
            ],
            "{id, shot, shots}",
            json!({"center": [5, 0, 5], "radius": 8, "height": 3, "seconds": 8}),
        ),
        orbit,
    );
    s.register_with_spec(
        spec(
            "film.flythrough",
            "Flythrough Film",
            "Builds a film of one flyover shot: a drone flies along `points` (a path on the ground, at least two [x, z]) at `height` metres above the ground, looking ahead and down. Starts a new film, or with `id` adds the shot to that film.",
            vec![
                param("points", T::Outline, "The path, [[x, z], ...] with at least two points.").required().unit("m"),
                param("seconds", T::Number, "Length of the shot.").unit("s").default(json!(10)),
                param("height", T::Number, "Height above the ground.").unit("m").default(json!(12)),
                param("pitch_deg", T::Number, "How far the camera looks down.").unit("deg").default(json!(35)),
                param("name", T::String, "Name of a new film.").default(json!("Flythrough")),
                param("id", T::String, "Add the shot to this film instead of starting one."),
                param("fps", T::Integer, "Frames per second of a new film, 1..120.").default(json!(25)),
                param("extras", T::Object, "More shot fields: ease, fov_y_deg, time_of_day, date, season, rain, mist, shafts...."),
            ],
            "{id, shot, shots}",
            json!({"points": [[0, 0], [10, 5], [20, 0]], "seconds": 12, "height": 10}),
        ),
        flythrough,
    );
    s.register_with_spec(
        spec(
            "film.reveal",
            "Reveal Film",
            "Builds a film of one glide shot that moves from a distant, high viewpoint `from` towards `target` and keeps looking at it: the target is revealed. `to` is where the move ends (by default a third of the way from the target to `from`, at least 2.5 m above it). Starts a new film, or with `id` adds the shot to that film.",
            vec![
                param("target", T::Point3, "The point revealed and looked at.").required().unit("m"),
                param("from", T::Point3, "Where the camera starts (default: 25 m away on both ground axes, 15 m up).").unit("m"),
                param("to", T::Point3, "Where the camera ends.").unit("m"),
                param("seconds", T::Number, "Length of the shot.").unit("s").default(json!(8)),
                param("name", T::String, "Name of a new film.").default(json!("Reveal")),
                param("id", T::String, "Add the shot to this film instead of starting one."),
                param("fps", T::Integer, "Frames per second of a new film, 1..120.").default(json!(25)),
                param("extras", T::Object, "More shot fields: ease, fov_y_deg, time_of_day, date, season, rain, mist, shafts...."),
            ],
            "{id, shot, shots}",
            json!({"target": [5, 1, 5], "from": [30, 15, 30], "seconds": 8}),
        ),
        reveal,
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

/// Starts a film (or finds `id`) and adds one shot built from JSON: the shared end of the
/// film helpers.
fn build_film(
    project: &mut Project,
    id: Option<String>,
    name: Option<String>,
    fps: Option<u32>,
    default_name: &str,
    shot: Value,
) -> Result<Value, CommandError> {
    let shot = shot_of(&shot, project.site.latitude)?;
    let shown = serde_json::to_value(&shot).map_err(|e| bad(&e.to_string()))?;
    let film_id = match id {
        Some(id) => {
            let film = find(project, &id)?;
            film.shots.push(shot);
            let shots = film.shots.len();
            return Ok(json!({"id": id, "shot": shown, "shots": shots}));
        }
        None => next_id("f", |id| project.films.iter().any(|f| f.id == id)),
    };
    let mut film = Film::new(
        film_id.clone(),
        name.unwrap_or_else(|| default_name.into()),
        fps.unwrap_or(25),
    );
    film.validate().map_err(|e| bad(&e))?;
    film.shots.push(shot);
    project.films.push(film);
    Ok(json!({"id": film_id, "shot": shown, "shots": 1}))
}

/// Merges the `extras` fields into a shot, the helper's own fields winning.
fn with_extras(mut shot: Value, extras: Option<Map<String, Value>>) -> Result<Value, CommandError> {
    if let Some(extras) = extras {
        for (k, v) in extras {
            if k == "kind" || shot.get(&k).is_some() {
                return Err(bad(&format!(
                    "`extras` cannot set `{k}`; the command has it"
                )));
            }
            shot[k] = v;
        }
    }
    Ok(shot)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Common {
    name: Option<String>,
    id: Option<String>,
    fps: Option<u32>,
    extras: Option<Map<String, Value>>,
}

fn orbit(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        center: [f32; 3],
        radius: f32,
        height: Option<f32>,
        seconds: Option<f32>,
        from_deg: Option<f32>,
        to_deg: Option<f32>,
        #[serde(flatten)]
        common: Common,
    }
    let p: P = parse(params)?;
    let from = p.from_deg.unwrap_or(0.0);
    let shot = json!({
        "kind": "orbit", "center": p.center, "radius_m": p.radius,
        "height_m": p.height.unwrap_or(3.0), "from_deg": from,
        "to_deg": p.to_deg.unwrap_or(from + 360.0), "duration_s": p.seconds.unwrap_or(8.0),
    });
    let shot = with_extras(shot, p.common.extras)?;
    build_film(
        project,
        p.common.id,
        p.common.name,
        p.common.fps,
        "Orbit",
        shot,
    )
}

fn flythrough(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        points: Vec<[f32; 2]>,
        seconds: Option<f32>,
        height: Option<f32>,
        pitch_deg: Option<f32>,
        #[serde(flatten)]
        common: Common,
    }
    let p: P = parse(params)?;
    let shot = json!({
        "kind": "flyover", "path": p.points, "height_m": p.height.unwrap_or(12.0),
        "pitch_deg": p.pitch_deg.unwrap_or(35.0), "duration_s": p.seconds.unwrap_or(10.0),
    });
    let shot = with_extras(shot, p.common.extras)?;
    build_film(
        project,
        p.common.id,
        p.common.name,
        p.common.fps,
        "Flythrough",
        shot,
    )
}

fn reveal(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        target: [f32; 3],
        from: Option<[f32; 3]>,
        to: Option<[f32; 3]>,
        seconds: Option<f32>,
        #[serde(flatten)]
        common: Common,
    }
    let p: P = parse(params)?;
    let t = p.target;
    let from = p.from.unwrap_or([t[0] + 25.0, t[1] + 15.0, t[2] + 25.0]);
    let to = p.to.unwrap_or_else(|| {
        let near = |i: usize| t[i] + (from[i] - t[i]) / 3.0;
        [near(0), near(1).max(t[1] + 2.5), near(2)]
    });
    let shot = json!({
        "kind": "glide", "path": [from, to], "look_at": t,
        "duration_s": p.seconds.unwrap_or(8.0),
    });
    let shot = with_extras(shot, p.common.extras)?;
    build_film(
        project,
        p.common.id,
        p.common.name,
        p.common.fps,
        "Reveal",
        shot,
    )
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

    fn built(f: fn(&mut Project, &Value) -> Result<Value, CommandError>, params: Value) -> Film {
        let mut p = Project::default();
        let out = f(&mut p, &params).unwrap();
        let film = p.films.iter().find(|f| f.id == out["id"]).unwrap().clone();
        film.validate().unwrap();
        for shot in &film.shots {
            shot.validate(p.site.latitude).unwrap();
        }
        film
    }

    #[test]
    fn orbit_builds_one_full_turn_by_default() {
        let film = built(orbit, json!({"center": [1, 0, 2], "radius": 6}));
        assert_eq!(film.shots.len(), 1);
        let ShotKind::Orbit {
            from_deg,
            to_deg,
            radius_m,
            ..
        } = film.shots[0].kind
        else {
            panic!("not an orbit");
        };
        assert_eq!((from_deg, to_deg, radius_m), (0.0, 360.0, 6.0));
        assert_eq!(film.shots[0].duration_s, Some(8.0));
    }

    #[test]
    fn flythrough_and_reveal_build_valid_films() {
        let film = built(
            flythrough,
            json!({"points": [[0, 0], [10, 5]], "seconds": 5, "extras": {"time_of_day": [17, 19]}}),
        );
        assert_eq!(film.shots[0].kind.name(), "flyover");
        assert!(film.shots[0].time_of_day.is_some());
        let film = built(reveal, json!({"target": [5, 1, 5]}));
        assert_eq!(film.shots[0].kind.name(), "glide");
    }

    #[test]
    fn helpers_add_to_an_existing_film_and_refuse_bad_input() {
        let mut p = Project::default();
        let id = add(&mut p, &json!({"name": "x"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let out = orbit(&mut p, &json!({"center": [0, 0, 0], "radius": 4, "id": id})).unwrap();
        assert_eq!(out["shots"], 1);
        let out = reveal(&mut p, &json!({"target": [0, 0, 0], "id": id})).unwrap();
        assert_eq!(out["shots"], 2);
        assert!(flythrough(&mut p, &json!({"points": [[0, 0]]})).is_err());
        let clash = orbit(
            &mut p,
            &json!({"center": [0, 0, 0], "radius": 4, "extras": {"duration_s": 3}}),
        );
        assert!(clash.is_err());
        assert!(reveal(&mut p, &json!({"target": [0, 0, 0], "id": "nope"})).is_err());
        assert_eq!(p.films.len(), 1);
    }
}
