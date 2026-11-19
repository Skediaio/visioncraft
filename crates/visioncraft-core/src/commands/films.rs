//! Films: shots of camera moves (`film.*`). `film.preview` and `film.render` are the host's,
//! as they need the ground the camera walks on and the renderer.

use super::{fail, next_id};
use crate::directing::{self, Notes, RevealEnd};
#[cfg(test)]
use crate::film::ShotKind;
use crate::film::{Film, SHOT_KINDS, Shot};
use crate::project::Project;
use crate::reference::{Extent, Scene, bearing_deg, place};
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const SHOT_DOC: &str = "A shot {kind, ...}. Kinds: `walk` {path [[x, z]...] | route [waypoints], eye_height_m = 1.6, pause_at [{point | target, s}], look [{at_s, target, hold_s = 2}]} (the eye follows the ground, looks ahead; speed_mps = 1.2 unless duration_s; `route` plans a path round hedges, walls, trunks and water between the waypoints and answers `warnings` where no clear way exists, and a given `path` that crosses one warns too; `pause_at` stands still s seconds at the nearest point of the path, the speed solved so walking and standing fit duration_s; `look` turns to `target` from at_s seconds into the shot (counting the pauses), holds hold_s and turns back, no cut); `glide` {path [[x, y, z]...]} (a smooth curve through the points; speed_mps = 2); `rise` {at [x, z], from_height, to_height, look_at required} (heights above the ground there; duration_s = 4); `orbit` {center [x, y, z], radius_m, height_m (above the centre's y), from_deg, to_deg} (angle from +X, positive counter-clockwise seen from above like rotation_y_deg; may pass 360; duration_s = 4, looks at the centre; film.orbit sets these from a target); `flyover` {path [[x, z]...], height_m (above the smoothed ground), pitch_deg = 35} (a drone looking ahead and down; speed_mps = 8); `hold` {viewpoint id | eye + target, duration_s = 4}. Any point (center, look_at, target, eye, at, path and route waypoints, pause points, looks) is [x, y, z] ([x, z] on the ground) or a reference: a shape id (\"S6\"), a planting (\"g4\") or one plant of it (\"g4#12\", crown size at the shot's years), a model, lamp or viewpoint id, \"plot\" (everything drawn) or a name matched without regard to case (\"Pond\"); it becomes the centre of its box, and the answer lists what each became under `resolved`. Any shot: duration_s or speed_mps (not both), ease in | out | in_out (default) | none, look_at [x, y, z] | reference | \"ahead\", fov_y_deg = 60, join \"continue\" (walk, glide, flyover, orbit: the shot starts where the one before ended, in position, look direction and speed, its ease only shaping how it ends, so there is no dip in speed and no cut; `cut` is the default), and changes along the shot, each one value or [from, to]: time_of_day h, date {month, day}, season (not with date), years n, snow 0..1, wetness 0..1, rain 0..1, haze 0..50 (turbidity), mist 0..1, gustiness 0..1, shafts 0..1, weather clear | fair | cloudy | overcast | rain | storm (as view.set's `weather`: [\"fair\", \"storm\"] eases the clouds, rain, wetness, mist, gusts and wind from one to the other over the shot; rain, mist and the others given on the shot win), clouds 0..1 (the clouds' cover), lightning 0..1 (how often lightning flashes in a storm sky, as view.set's `clouds.lightning`), lights 0..1 (the lamps' light: fades lamps in and out, any value forces the lamps on or off), white_balance kelvin 2000..12000 (6500 daylight), grade neutral | warm | cool | soft (two grades are mixed along the shot), exposure_ev stops -8..8 relative to the automatic exposure (0: no correction, +1 one stop brighter; not view.set's exposure_ev, which is an absolute value where 0 is white daylight).";

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
            "Adds a shot to a film, at the end or at `index`. Points may be references (\"Pond\", \"S6\", \"g4#12\", \"plot\"); a walk's `route` is planned round the obstacles; the answer says what each reference became (`resolved`) and `warnings` where a walk would cross a hedge, wall or water. A walk needs the ground it walks on, so its length in time is known only to film.preview.",
            vec![
                id(),
                param("shot", T::Object, SHOT_DOC).required(),
                param("index", T::Integer, "Where in the list, from 0 (the end by default)."),
            ],
            "{index, shots, resolved?, route?, warnings?}",
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
    let extras = || {
        param(
            "extras",
            T::Object,
            "More shot fields as film.add_shot lists them: ease, fov_y_deg, join, time_of_day, date, season, years, rain, mist, shafts, lightning, lights, white_balance, grade, exposure_ev, ....",
        )
    };
    s.register_with_spec(
        spec(
            "film.orbit",
            "Orbit Film",
            "Builds a film of one orbit shot: the camera circles the target and looks at its centre. `target` (or `center`) is [x, y, z] or a reference (a shape id, a planting, `g4#12`, a model, a lamp, a viewpoint, `plot` or a name such as \"Pond\"). With `distance` \"frame\" (the default for a reference) the radius and height are set from the target's size and the field of view so it fills about 85 % of the frame at 16:9; a number is the radius on the ground in metres. Start from a compass point (`from`: south, north-east, ... or a bearing in degrees, 0 north, 90 east; -Z is north, +X is east) and `sweep_deg` degrees clockwise seen from above (negative: counter-clockwise); or give the angles themselves with `from_deg` and `to_deg`. Starts a new film, or with `id` adds the shot to that film (`extras` {join: \"continue\"} takes over from the shot before). Time, weather and ease go along as in film.add_shot.",
            vec![
                param("target", T::Place, "The thing the camera circles and looks at: [x, y, z] or a reference. Give this or `center`."),
                param("center", T::Place, "The point the camera circles and looks at (as `target`)."),
                param("distance", T::Any, "\"frame\": radius and height from the target's size and the field of view; or the radius in metres on the ground. Default \"frame\" (a `radius` instead is the same as a number).").default(json!("frame")),
                param("radius", T::Number, "Radius on the ground plane in metres (the same as a numeric `distance`).").unit("m"),
                param("height_m", T::Number, "Eye height above the target's centre (the framed height by default, 3 m with a radius in metres).").unit("m"),
                param("height", T::Number, "The same as `height_m`.").unit("m"),
                param("from", T::Any, "Where the orbit starts: a compass word (south, north-east, nne, ...) or a bearing in degrees, 0 north, 90 east, 180 south (clockwise from north). Default: from_deg 0, east.").default(json!("east")),
                param("sweep_deg", T::Number, "How far it goes round, positive clockwise seen from above, 360 a full turn.").unit("deg").default(json!(360)),
                param("seconds", T::Number, "Length of the shot.").unit("s").default(json!(8)),
                param("duration_s", T::Number, "The same as `seconds`.").unit("s"),
                param("from_deg", T::Number, "Start angle from +X, counter-clockwise seen from above (instead of `from`).").unit("deg"),
                param("to_deg", T::Number, "End angle (instead of `sweep_deg`); 360 more than from_deg is one full turn.").unit("deg"),
                param("name", T::String, "Name of a new film.").default(json!("Orbit")),
                param("id", T::String, "Add the shot to this film instead of starting one."),
                param("fps", T::Integer, "Frames per second of a new film, 1..120.").default(json!(25)),
                extras(),
            ],
            "{id, shot, shots, framing: {radius_m, height_m, distance_m}, resolved?, warnings?}",
            json!({"target": "House", "from": "south", "sweep_deg": 120, "seconds": 8}),
        ),
        orbit,
    );
    s.register_with_spec(
        spec(
            "film.flythrough",
            "Flythrough Film",
            "Builds a film of one flyover shot: a drone flies along `points` (a path on the ground, at least two [x, z] or references) at `height` metres above the ground, looking ahead and down. Starts a new film, or with `id` adds the shot to that film.",
            vec![
                param("points", T::Places, "The path: at least two of [x, z] or a reference (its centre).").required().unit("m"),
                param("seconds", T::Number, "Length of the shot.").unit("s").default(json!(10)),
                param("height", T::Number, "Height above the ground.").unit("m").default(json!(12)),
                param("pitch_deg", T::Number, "How far the camera looks down.").unit("deg").default(json!(35)),
                param("name", T::String, "Name of a new film.").default(json!("Flythrough")),
                param("id", T::String, "Add the shot to this film instead of starting one."),
                param("fps", T::Integer, "Frames per second of a new film, 1..120.").default(json!(25)),
                extras(),
            ],
            "{id, shot, shots, resolved?, warnings?}",
            json!({"points": [[0, 0], [10, 5], [20, 0]], "seconds": 12, "height": 10}),
        ),
        flythrough,
    );
    s.register_with_spec(
        spec(
            "film.reveal",
            "Reveal Film",
            "Builds a film of one glide shot that reveals the target while looking at its centre. With `end` (the standard drone opening: `top_down`) it rises from eye level on the `from` side (default south) and ends straight above the target, framed to fill the picture, with north up (`north_up`; with false, up is the way away from `from`); `end` \"orbit\" ends where film.orbit on the same target starts (follow it with film.orbit {id, target, extras: {join: \"continue\"}}). Without `end`, `from` is a point [x, y, z] (default 25 m away on both ground axes, 15 m up) and `to` where the move ends (by default a third of the way from the target to `from`, at least 2.5 m above it). `target` is [x, y, z] or a reference (`plot`, a shape id, a planting, a name). Starts a new film, or with `id` adds the shot to that film.",
            vec![
                param("target", T::Place, "The thing revealed and looked at: [x, y, z] or a reference.").required(),
                param("end", T::String, "Where the reveal ends: top_down (straight above the target) or orbit (where film.orbit starts).").allowed(&["top_down", "orbit"]),
                param("north_up", T::Boolean, "With `end`: the picture's up at the top-down end is north (default); false: away from `from`.").default(json!(true)),
                param("from", T::Any, "Where the camera starts: a point [x, y, z], or with `end` a compass word or bearing in degrees (south by default).").unit("m"),
                param("to", T::Place, "Without `end`: where the camera ends.").unit("m"),
                param("seconds", T::Number, "Length of the shot.").unit("s").default(json!(8)),
                param("duration_s", T::Number, "The same as `seconds`.").unit("s"),
                param("name", T::String, "Name of a new film.").default(json!("Reveal")),
                param("id", T::String, "Add the shot to this film instead of starting one."),
                param("fps", T::Integer, "Frames per second of a new film, 1..120.").default(json!(25)),
                extras(),
            ],
            "{id, shot, shots, resolved?, warnings?}",
            json!({"target": "plot", "end": "top_down", "north_up": true, "seconds": 8}),
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

/// The command's params with references turned into numbers and a route planned, over what
/// the core knows of the project (a host that knows more does this before the command runs;
/// what it has resolved is numbers by then).
fn resolved(project: &Project, id: &str, params: &Value) -> Result<(Value, Notes), CommandError> {
    let scene = Scene::new(project, directing::years_of(project, id, params));
    directing::resolve_command(id, params, &scene).map_err(|e| bad(&e))
}

fn answer(mut result: Value, notes: &Notes) -> Value {
    notes.apply(&mut result);
    result
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
    let (given, notes) = resolved(project, "film.add_shot", params)?;
    let shot = shot_of(&given["shot"], latitude)?;
    let _ = p.shot;
    let film = find(project, &p.id)?;
    let index = p.index.unwrap_or(film.shots.len());
    if index > film.shots.len() {
        return Err(bad(&format!(
            "`index` {index} is past the end: the film has {} shots",
            film.shots.len()
        )));
    }
    film.shots.insert(index, shot);
    Ok(answer(
        json!({"index": index, "shots": film.shots.len()}),
        &notes,
    ))
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
    shot_index(find(project, &p.id)?, p.index)?;
    let (given, notes) = resolved(project, "film.update_shot", params)?;
    let p: P = parse(&given)?;
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
    Ok(
        if notes.warnings.is_empty() && notes.resolved.is_empty() && notes.route.is_none() {
            Value::Null
        } else {
            answer(json!({}), &notes)
        },
    )
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
    answer: (&Notes, Value),
) -> Result<Value, CommandError> {
    let (notes, extra) = answer;
    let shot = shot_of(&shot, project.site.latitude)?;
    let shown = serde_json::to_value(&shot).map_err(|e| bad(&e.to_string()))?;
    let film_id = match id {
        Some(id) => {
            let film = find(project, &id)?;
            film.shots.push(shot);
            let shots = film.shots.len();
            return Ok(with_extra(
                json!({"id": id, "shot": shown, "shots": shots}),
                extra,
                notes,
            ));
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
    Ok(with_extra(
        json!({"id": film_id, "shot": shown, "shots": 1}),
        extra,
        notes,
    ))
}

fn with_extra(mut result: Value, extra: Value, notes: &Notes) -> Value {
    if let (Some(r), Value::Object(x)) = (result.as_object_mut(), extra) {
        r.extend(x);
    }
    answer(result, notes)
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

fn seconds(a: Option<f32>, b: Option<f32>, default: f32) -> Result<f32, CommandError> {
    if a.is_some() && b.is_some() {
        return Err(bad("give `seconds` or `duration_s`, not both"));
    }
    Ok(a.or(b).unwrap_or(default))
}

fn fov_of(extras: &Option<Map<String, Value>>) -> f32 {
    extras
        .as_ref()
        .and_then(|e| e.get("fov_y_deg"))
        .and_then(Value::as_f64)
        .map_or(directing::DEFAULT_FOV_Y, |f| f as f32)
}

fn orbit(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        target: Option<Value>,
        center: Option<Value>,
        distance: Option<Value>,
        radius: Option<f32>,
        height_m: Option<f32>,
        height: Option<f32>,
        from: Option<Value>,
        sweep_deg: Option<f32>,
        seconds: Option<f32>,
        duration_s: Option<f32>,
        from_deg: Option<f32>,
        to_deg: Option<f32>,
        #[serde(flatten)]
        common: Common,
    }
    let (given, notes) = resolved(project, "film.orbit", params)?;
    let p: P = parse(&given)?;
    let scene = Scene::new(project, directing::years_of(project, "film.orbit", params));
    let target = match (&p.target, &p.center) {
        (Some(_), Some(_)) => return Err(bad("give `target` or `center`, not both")),
        (Some(t), None) | (None, Some(t)) => place(&scene, t, "target").map_err(|e| bad(&e))?,
        (None, None) => return Err(bad("give `target` (a point or a reference) to orbit")),
    };
    // The radius on the ground: `radius`, a numeric `distance`, or "frame".
    let radius = match (&p.distance, p.radius) {
        (Some(_), Some(_)) => return Err(bad("give `distance` or `radius`, not both")),
        (Some(Value::String(w)), None) if w == "frame" => None,
        (Some(d), None) => Some(
            d.as_f64()
                .filter(|d| *d > 0.0)
                .ok_or_else(|| bad("`distance` is \"frame\" or a number of metres above 0"))?
                as f32,
        ),
        (None, r) => r,
    };
    let height = match (p.height_m, p.height) {
        (Some(_), Some(_)) => return Err(bad("give `height_m` or `height`, not both")),
        (a, b) => a.or(b),
    };
    let fov = fov_of(&p.common.extras);
    let frame = directing::orbit_frame(&target, fov, radius, height);
    let (from_deg, to_deg) = match (&p.from, p.sweep_deg, p.from_deg, p.to_deg) {
        (None, None, from, to) => {
            let from = from.unwrap_or(0.0);
            (from, to.unwrap_or(from + 360.0))
        }
        (from, sweep, None, None) => directing::orbit_angles(
            from.as_ref().unwrap_or(&json!("south")),
            sweep.unwrap_or(360.0),
        )
        .map_err(|e| bad(&e))?,
        _ => {
            return Err(bad(
                "give `from` and `sweep_deg` (a compass point and a sweep) or `from_deg` and `to_deg` (angles), not both",
            ));
        }
    };
    let centre = target.look_point();
    let shot = json!({
        "kind": "orbit", "center": centre, "radius_m": frame.radius_m,
        "height_m": frame.height_m, "from_deg": from_deg, "to_deg": to_deg,
        "duration_s": seconds(p.seconds, p.duration_s, 8.0)?,
    });
    let shot = with_extras(shot, p.common.extras)?;
    build_film(
        project,
        p.common.id,
        p.common.name,
        p.common.fps,
        "Orbit",
        shot,
        (
            &notes,
            json!({"framing": {"radius_m": frame.radius_m, "height_m": frame.height_m, "distance_m": frame.distance_m}}),
        ),
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
    let (given, notes) = resolved(project, "film.flythrough", params)?;
    let p: P = parse(&given)?;
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
        (&notes, json!({})),
    )
}

fn reveal(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        target: Value,
        end: Option<String>,
        north_up: Option<bool>,
        from: Option<Value>,
        to: Option<[f32; 3]>,
        seconds: Option<f32>,
        duration_s: Option<f32>,
        #[serde(flatten)]
        common: Common,
    }
    let (given, notes) = resolved(project, "film.reveal", params)?;
    let p: P = parse(&given)?;
    let scene = Scene::new(project, directing::years_of(project, "film.reveal", params));
    let target: Extent = place(&scene, &p.target, "target").map_err(|e| bad(&e))?;
    let t = target.look_point();
    let compass_from = p
        .from
        .as_ref()
        .is_some_and(|f| f.is_string() || f.is_number());
    let path: Vec<[f32; 3]> = if p.end.is_some() || compass_from {
        if p.to.is_some() {
            return Err(bad(
                "`to` is for the point form (without `end`); `end` decides where the reveal ends",
            ));
        }
        let end = RevealEnd::parse(p.end.as_deref().unwrap_or("top_down")).map_err(|e| bad(&e))?;
        let bearing = match &p.from {
            Some(f) => bearing_deg(f).map_err(|e| bad(&format!("`from`: {e}")))?,
            None => 180.0,
        };
        directing::reveal_path(
            &target,
            fov_of(&p.common.extras),
            bearing,
            end,
            p.north_up.unwrap_or(true),
        )
    } else {
        if p.north_up.is_some() {
            return Err(bad("`north_up` goes with `end`"));
        }
        let from = match &p.from {
            Some(f) => serde_json::from_value::<[f32; 3]>(f.clone())
                .map_err(|_| bad("`from` is a point [x, y, z], or with `end` a compass word"))?,
            None if target.kind == "point" => [t[0] + 25.0, t[1] + 15.0, t[2] + 25.0],
            None => {
                let f = directing::orbit_frame(&target, fov_of(&p.common.extras), None, None);
                [t[0], t[1] + f.height_m, t[2] + f.radius_m]
            }
        };
        let to = p.to.unwrap_or_else(|| {
            let near = |i: usize| t[i] + (from[i] - t[i]) / 3.0;
            [near(0), near(1).max(t[1] + 2.5), near(2)]
        });
        vec![from, to]
    };
    let shot = json!({
        "kind": "glide", "path": path, "look_at": t,
        "duration_s": seconds(p.seconds, p.duration_s, 8.0)?,
    });
    let shot = with_extras(shot, p.common.extras)?;
    build_film(
        project,
        p.common.id,
        p.common.name,
        p.common.fps,
        "Reveal",
        shot,
        (&notes, json!({})),
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

    fn garden() -> Project {
        crate::example("garden").unwrap()
    }

    #[test]
    fn orbit_of_a_target_frames_it_and_starts_from_a_compass_point() {
        let mut p = garden();
        let out = orbit(
            &mut p,
            &json!({"target": "plot", "from": "south", "sweep_deg": 100, "seconds": 10}),
        )
        .unwrap();
        let film = p.films.iter().find(|f| f.id == out["id"]).unwrap();
        let ShotKind::Orbit {
            center,
            radius_m,
            height_m,
            from_deg,
            to_deg,
        } = film.shots[0].kind
        else {
            panic!("not an orbit");
        };
        // South is +Z of the centre; 100 degrees clockwise ends past west.
        assert_eq!((from_deg, to_deg), (-90.0, -190.0));
        assert!(radius_m > 20.0 && height_m > 8.0, "{radius_m} {height_m}");
        assert_eq!(
            out["framing"]["radius_m"].as_f64().unwrap() as f32,
            radius_m
        );
        assert!(
            (center[0] - 9.9).abs() < 0.2 && center[2] > 14.0,
            "{center:?}"
        );
        assert_eq!(out["resolved"][0]["ref"], "plot");
        // A smaller target is closer; metres radius and height are taken as they are.
        let near = orbit(&mut p, &json!({"target": "Pond"})).unwrap();
        assert!(near["framing"]["radius_m"].as_f64().unwrap() < f64::from(radius_m));
        let own = orbit(
            &mut p,
            &json!({"target": "g4", "distance": 9, "height_m": 4}),
        )
        .unwrap();
        assert_eq!(own["framing"]["radius_m"], 9.0);
        assert!(
            orbit(
                &mut p,
                &json!({"target": "plot", "from": "north", "from_deg": 3})
            )
            .is_err()
        );
        assert!(orbit(&mut p, &json!({})).is_err());
        let err = orbit(&mut p, &json!({"target": "Pnd"})).unwrap_err();
        assert!(err.to_string().contains("close matches"), "{err}");
    }

    #[test]
    fn reveal_with_an_end_rises_and_ends_above_the_target() {
        let mut p = garden();
        let out = reveal(
            &mut p,
            &json!({"target": "plot", "end": "top_down", "north_up": true}),
        )
        .unwrap();
        let path = out["shot"]["path"].as_array().unwrap();
        assert_eq!(path.len(), 5);
        let (first, last) = (&path[0], &path[4]);
        assert!(first[1].as_f64().unwrap() < 2.0 && last[1].as_f64().unwrap() > 20.0);
        assert!(reveal(&mut p, &json!({"target": "plot", "end": "sideways"})).is_err());
        assert!(
            reveal(
                &mut p,
                &json!({"target": "plot", "end": "top_down", "to": [1, 2, 3]})
            )
            .is_err()
        );
        // The orbit that follows starts where an `orbit` end stops.
        let r = reveal(
            &mut p,
            &json!({"target": "plot", "end": "orbit", "from": "south"}),
        )
        .unwrap();
        let o = orbit(
            &mut p,
            &json!({"target": "plot", "from": "south", "id": r["id"]}),
        )
        .unwrap();
        let last = r["shot"]["path"][4].clone();
        let ShotKind::Orbit {
            center,
            radius_m,
            height_m,
            ..
        } = p.films[1].shots[1].kind
        else {
            panic!("not an orbit")
        };
        let eye = [center[0], center[1] + height_m, center[2] + radius_m];
        for k in 0..3 {
            assert!(
                (last[k].as_f64().unwrap() as f32 - eye[k]).abs() < 0.01,
                "{last} {eye:?}"
            );
        }
        assert_eq!(o["shots"], 2);
    }

    #[test]
    fn walk_routes_pauses_and_references_go_through_add_shot() {
        let mut p = garden();
        let id = add(&mut p, &json!({"name": "w"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let out = add_shot(
            &mut p,
            &json!({"id": id, "shot": {"kind": "walk", "route": [[5, 22.5], "S5"], "pause_at": [{"target": "Pond", "s": 2}], "duration_s": 20}}),
        )
        .unwrap();
        assert_eq!(out["shots"], 1);
        assert!(out["route"]["length_m"].as_f64().unwrap() > 10.0);
        assert!(
            out["resolved"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["ref"] == "S5")
        );
        // The same walk by `path` through the pond warns, and update_shot warns too.
        let out = add_shot(
            &mut p,
            &json!({"id": id, "shot": {"kind": "walk", "path": [[5, 22.5], [16, 22.5]]}}),
        )
        .unwrap();
        assert!(
            out["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w.as_str().unwrap().contains("water S6")),
            "{out}"
        );
        let up = update_shot(
            &mut p,
            &json!({"id": id, "index": 1, "shot": {"path": [[5, 22.5], [16, 22.5]]}}),
        )
        .unwrap();
        assert!(up["warnings"].is_array());
        let err = add_shot(
            &mut p,
            &json!({"id": id, "shot": {"kind": "walk", "route": [[0, 0]]}}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("at least 2"), "{err}");
        let err = add_shot(&mut p, &json!({"id": id, "shot": {"kind": "walk", "path": [[0, 0], [3, 3]], "route": [[0, 0], [3, 3]]}})).unwrap_err();
        assert!(err.to_string().contains("not both"), "{err}");
        // A viewpoint by name target.
        let mut s = crate::Session::new(garden());
        s.run(
            "viewpoint.add",
            json!({"name": "to pond", "position": [5, 2, 30], "target": "Pond"}),
        )
        .unwrap();
        assert_eq!(s.project().viewpoints.last().unwrap().target[0], 10.5);
    }
}
