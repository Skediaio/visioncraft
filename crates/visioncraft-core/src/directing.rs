//! What a director's commands do before a shot exists: references in point fields turn into
//! points (`reference`), a walk's `route` becomes a path round hedges, walls, trunks and water
//! (`route`), and an orbit or a reveal is framed from the size of its target.
//!
//! Compass words use the drawing's axes: -Z is north, +X is east. A bearing is degrees clockwise
//! from north (0 north, 90 east, 180 south, 270 west).

use crate::project::Project;
#[cfg(test)]
use crate::reference;
use crate::reference::{Extent, Scene, bearing_deg, bearing_dir, orbit_angle, place, place_xz};
use crate::route::{self, Obstacles};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// How much of the frame the target fills when the distance is `"frame"`.
pub const FRAME_FILL: f32 = 0.85;
/// How far above the horizon an orbit looks down by default, in degrees.
pub const ORBIT_ELEVATION_DEG: f32 = 25.0;
/// The picture's shape that framing is worked out for.
pub const FRAME_ASPECT: f32 = 16.0 / 9.0;
/// The field of view of a shot that gives none.
pub const DEFAULT_FOV_Y: f32 = 60.0;
const EYE_LEVEL_M: f32 = 1.7;

/// What resolving a command's params found.
#[derive(Clone, Debug, Default)]
pub struct Notes {
    /// Each reference with the box it became.
    pub resolved: Vec<Value>,
    pub warnings: Vec<String>,
    /// A planned route: `{waypoints, length_m}`.
    pub route: Option<Value>,
}

impl Notes {
    fn resolved(&mut self, field: &str, e: &Extent) {
        let mut v = e.to_json();
        v["field"] = field.into();
        if let Some(n) = &e.note {
            self.warnings.push(n.clone());
        }
        self.resolved.push(v);
    }

    /// Puts what was found into a command's result.
    pub fn apply(&self, result: &mut Value) {
        let Some(map) = result.as_object_mut() else {
            return;
        };
        if !self.resolved.is_empty() {
            map.insert("resolved".into(), self.resolved.clone().into());
        }
        if let Some(r) = &self.route {
            map.insert("route".into(), r.clone());
        }
        if !self.warnings.is_empty() {
            map.insert("warnings".into(), json!(self.warnings));
        }
    }
}

/// The years after planting plants are measured at for a command: the shot's `years` (its end),
/// else the project's.
pub fn years_of(project: &Project, id: &str, params: &Value) -> Option<f32> {
    let span = |v: &Value| {
        v.as_f64()
            .or_else(|| v.as_array().and_then(|a| a.last()).and_then(Value::as_f64))
            .map(|y| y as f32)
    };
    let own = match id {
        "film.add_shot" | "film.update_shot" => span(&params["shot"]["years"]),
        "film.orbit" | "film.flythrough" | "film.reveal" => span(&params["extras"]["years"]),
        _ => None,
    };
    own.or(project.view.years)
}

fn num_array(v: &[f32]) -> Value {
    json!(v)
}

/// A point field: a reference becomes the centre of its box `[x, y, z]`; a point stays.
fn point3_field(
    scene: &Scene,
    map: &mut Map<String, Value>,
    key: &str,
    notes: &mut Notes,
) -> Result<(), String> {
    let Some(v) = map.get(key) else { return Ok(()) };
    if v.is_array() || v.is_null() {
        return Ok(());
    }
    if v.as_str() == Some("ahead") {
        return Ok(());
    }
    let e = place(scene, v, key)?;
    notes.resolved(key, &e);
    map.insert(key.into(), num_array(&e.look_point()));
    Ok(())
}

/// A ground point field `[x, z]`.
fn point2_field(
    scene: &Scene,
    map: &mut Map<String, Value>,
    key: &str,
    notes: &mut Notes,
) -> Result<(), String> {
    let Some(v) = map.get(key) else { return Ok(()) };
    if v.is_null() || v.as_array().is_some_and(|a| a.len() == 2) {
        return Ok(());
    }
    let p = place_xz(scene, v, key)?;
    if let Value::String(_) | Value::Object(_) = v
        && let Ok(e) = place(scene, v, key)
    {
        notes.resolved(key, &e);
    }
    map.insert(key.into(), num_array(&p));
    Ok(())
}

/// The points of a list field, each `[x, z]`, `[x, y, z]` or a reference, as `dims` numbers.
fn list_field(
    scene: &Scene,
    map: &mut Map<String, Value>,
    key: &str,
    dims: usize,
    notes: &mut Notes,
) -> Result<(), String> {
    let Some(Value::Array(items)) = map.get(key) else {
        return Ok(());
    };
    let mut out = Vec::new();
    for (i, v) in items.iter().enumerate() {
        let field = format!("{key}[{i}]");
        let plain = v.as_array().is_some_and(|a| a.len() == dims);
        if plain {
            out.push(v.clone());
            continue;
        }
        let e = place(scene, v, &field)?;
        if matches!(v, Value::String(_)) {
            notes.resolved(&field, &e);
        }
        let c = e.look_point();
        out.push(if dims == 2 {
            num_array(&[c[0], c[2]])
        } else {
            num_array(&c)
        });
    }
    map.insert(key.into(), out.into());
    Ok(())
}

fn xz_list(scene: &Scene, items: &[Value], key: &str) -> Result<Vec<[f32; 2]>, String> {
    items
        .iter()
        .enumerate()
        .map(|(i, v)| place_xz(scene, v, &format!("{key}[{i}]")))
        .collect()
}

fn resolve_walk(
    scene: &Scene,
    map: &mut Map<String, Value>,
    notes: &mut Notes,
) -> Result<(), String> {
    if let Some(route) = map.remove("route").filter(|r| !r.is_null()) {
        if map.get("path").is_some_and(|p| !p.is_null()) {
            return Err("give `path` or `route`, not both".into());
        }
        let items = route
            .as_array()
            .ok_or("`route` is a list of waypoints: [x, z], [x, y, z] or a reference each")?;
        if items.len() < 2 {
            return Err("`route` needs at least 2 waypoints".into());
        }
        let waypoints = xz_list(scene, items, "route")?;
        for (i, v) in items.iter().enumerate() {
            if let Value::String(_) = v
                && let Ok(e) = place(scene, v, "route")
            {
                notes.resolved(&format!("route[{i}]"), &e);
            }
        }
        let planned = route::plan(&Obstacles::from_scene(scene), &waypoints, None);
        let length: f32 = planned
            .path
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
            .sum();
        notes.route = Some(
            json!({"waypoints": waypoints, "path_points": planned.path.len(), "length_m": length}),
        );
        notes.warnings.extend(planned.warnings);
        map.insert("path".into(), json!(planned.path));
    } else if let Some(Value::Array(items)) = map.get("path").cloned() {
        let path = xz_list(scene, &items, "path")?;
        for (i, v) in items.iter().enumerate() {
            if let Value::String(_) = v
                && let Ok(e) = place(scene, v, "path")
            {
                notes.resolved(&format!("path[{i}]"), &e);
            }
        }
        if path.len() >= 2 {
            notes
                .warnings
                .extend(route::check_path(&Obstacles::from_scene(scene), &path));
        }
        map.insert("path".into(), json!(path));
    }
    if let Some(Value::Array(items)) = map.get_mut("pause_at") {
        for (i, item) in items.iter_mut().enumerate() {
            let Some(m) = item.as_object_mut() else {
                return Err(format!("`pause_at[{i}]` is {{point | target, s}}"));
            };
            if let Some(t) = m.remove("target") {
                if m.contains_key("point") {
                    return Err(format!(
                        "`pause_at[{i}]`: give `point` or `target`, not both"
                    ));
                }
                m.insert("point".into(), t);
            }
            point2_field(scene, m, "point", notes)?;
        }
    }
    if let Some(Value::Array(items)) = map.get_mut("look") {
        for item in items.iter_mut() {
            if let Some(m) = item.as_object_mut() {
                point3_field(scene, m, "target", notes)?;
            }
        }
    }
    Ok(())
}

/// A reference given as a name or an id: a string that is not the word `ahead`.
fn named(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| *s != "ahead" && !s.trim().is_empty())
}

/// The references a shot is given, by the field they fill (`Shot::targets`), before they turn
/// into points.
fn shot_targets(shot: &Map<String, Value>, kind: Option<&str>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut add = |path: String, v: Option<&Value>| {
        if let Some(r) = v.and_then(named) {
            out.insert(path, r.to_string());
        }
    };
    add("look_at".into(), shot.get("look_at"));
    let list = |key: &str| shot.get(key).and_then(Value::as_array);
    match kind {
        Some("walk") => {
            for (i, v) in list("path").into_iter().flatten().enumerate() {
                add(format!("path[{i}]"), Some(v));
            }
            for (i, v) in list("pause_at").into_iter().flatten().enumerate() {
                add(format!("pause_at[{i}]"), v.get("target").or(v.get("point")));
            }
            for (i, v) in list("look").into_iter().flatten().enumerate() {
                add(format!("look[{i}]"), v.get("target"));
            }
        }
        Some("glide" | "flyover") => {
            for (i, v) in list("path").into_iter().flatten().enumerate() {
                add(format!("path[{i}]"), Some(v));
            }
        }
        Some("rise") => add("at".into(), shot.get("at")),
        Some("orbit") => add("center".into(), shot.get("center")),
        Some("hold") => {
            add("eye".into(), shot.get("eye"));
            add("target".into(), shot.get("target"));
        }
        _ => {}
    }
    out
}

/// Adds `targets` to the `targets` an object already has.
fn merge_targets(map: &mut Map<String, Value>, targets: BTreeMap<String, String>) {
    if targets.is_empty() {
        return;
    }
    let entry = map
        .entry("targets")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(own) = entry.as_object_mut() {
        own.extend(targets.into_iter().map(|(k, v)| (k, Value::String(v))));
    }
}

fn resolve_shot(
    scene: &Scene,
    shot: &mut Map<String, Value>,
    kind: Option<&str>,
    notes: &mut Notes,
) -> Result<(), String> {
    let kind = match kind {
        Some(k) => Some(k.to_string()),
        None if shot.contains_key("route") || shot.contains_key("pause_at") => Some("walk".into()),
        None => None,
    };
    let targets = shot_targets(shot, kind.as_deref());
    point3_field(scene, shot, "look_at", notes)?;
    match kind.as_deref() {
        Some("walk") => resolve_walk(scene, shot, notes)?,
        Some("glide") => list_field(scene, shot, "path", 3, notes)?,
        Some("flyover") => list_field(scene, shot, "path", 2, notes)?,
        Some("rise") => point2_field(scene, shot, "at", notes)?,
        Some("orbit") => point3_field(scene, shot, "center", notes)?,
        Some("hold") => {
            point3_field(scene, shot, "eye", notes)?;
            point3_field(scene, shot, "target", notes)?;
        }
        _ => {}
    }
    merge_targets(shot, targets);
    Ok(())
}

/// What a helper's `extras` (fields merged into the shot it makes) name: its `look_at`, and the
/// `points` of a flythrough as the `path`.
fn extras_targets(map: &mut Map<String, Value>, points: Option<&str>) {
    let mut targets = BTreeMap::new();
    if let Some(r) = map
        .get("extras")
        .and_then(|e| e.get("look_at"))
        .and_then(named)
    {
        targets.insert("look_at".to_string(), r.to_string());
    }
    if let Some(key) = points {
        for (i, v) in map
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if let Some(r) = named(v) {
                targets.insert(format!("path[{i}]"), r.to_string());
            }
        }
    }
    if let Some(extras) = map.get_mut("extras").and_then(Value::as_object_mut) {
        merge_targets(extras, targets);
    } else if !targets.is_empty() {
        let mut extras = Map::new();
        merge_targets(&mut extras, targets);
        map.insert("extras".into(), Value::Object(extras));
    }
}

/// The command's params with references turned into numbers (a target of `film.orbit` or
/// `film.reveal` into the box it stands for) and a walk's `route` planned into its `path`.
/// Params that are numbers already come back as they are.
pub fn resolve_command(id: &str, params: &Value, scene: &Scene) -> Result<(Value, Notes), String> {
    let mut p = params.clone();
    let mut notes = Notes::default();
    let fail = |e: String| e;
    match id {
        "film.add_shot" => {
            if let Some(shot) = p.get_mut("shot").and_then(Value::as_object_mut) {
                let kind = shot.get("kind").and_then(Value::as_str).map(str::to_string);
                resolve_shot(scene, shot, kind.as_deref(), &mut notes).map_err(fail)?;
            }
        }
        "film.update_shot" => {
            let kind = scene
                .project
                .films
                .iter()
                .find(|f| p["id"].as_str() == Some(&f.id))
                .and_then(|f| {
                    f.shots
                        .get(p["index"].as_u64().unwrap_or(u64::MAX) as usize)
                })
                .map(|s| s.kind.name().to_string());
            if let Some(shot) = p.get_mut("shot").and_then(Value::as_object_mut) {
                resolve_shot(scene, shot, kind.as_deref(), &mut notes).map_err(fail)?;
            }
        }
        "film.orbit" | "film.reveal" => {
            let Some(map) = p.as_object_mut() else {
                return Ok((p, notes));
            };
            extras_targets(map, None);
            for key in ["target", "center"] {
                if let Some(v) = map.get(key).filter(|v| v.is_string()).cloned() {
                    let e = place(scene, &v, key).map_err(fail)?;
                    notes.resolved(key, &e);
                    map.insert(key.into(), e.to_json());
                }
            }
            if id == "film.reveal" {
                point3_field(scene, map, "to", &mut notes).map_err(fail)?;
                // `from` is a point, a compass word or a bearing: only a point is left alone.
            }
            if let Some(extras) = map.get_mut("extras").and_then(Value::as_object_mut) {
                point3_field(scene, extras, "look_at", &mut notes).map_err(fail)?;
            }
        }
        "film.flythrough" => {
            let Some(map) = p.as_object_mut() else {
                return Ok((p, notes));
            };
            extras_targets(map, Some("points"));
            list_field(scene, map, "points", 2, &mut notes).map_err(fail)?;
            if let Some(extras) = map.get_mut("extras").and_then(Value::as_object_mut) {
                point3_field(scene, extras, "look_at", &mut notes).map_err(fail)?;
            }
        }
        "viewpoint.add" | "viewpoint.update" => {
            if let Some(map) = p.as_object_mut() {
                point3_field(scene, map, "position", &mut notes).map_err(fail)?;
                point3_field(scene, map, "target", &mut notes).map_err(fail)?;
            }
        }
        "view.render" => {
            if let Some(cam) = p.get_mut("camera").and_then(Value::as_object_mut) {
                point3_field(scene, cam, "position", &mut notes).map_err(fail)?;
                point3_field(scene, cam, "target", &mut notes).map_err(fail)?;
            }
        }
        _ => {}
    }
    Ok((p, notes))
}

// ----------------------------------------------------------------------------- framing

/// The radius of the sphere that stands for a box when framing it: a plot, a house or a tree
/// seen from a few degrees above the horizon does not fill its bounding sphere, so it is a
/// little smaller than the box's own (three quarters of half its diagonal, the height counted
/// half).
pub fn framing_radius(e: &Extent) -> f32 {
    let [w, d] = e.footprint();
    let h = e.height() / 2.0;
    0.75 * 0.5 * (w * w + d * d + h * h).sqrt()
}

/// How far a camera with field of view `fov_y_deg` must stand from the box for it to fill
/// [`FRAME_FILL`] of the frame.
pub fn frame_distance(e: &Extent, fov_y_deg: f32) -> f32 {
    let half = (fov_y_deg.clamp(1.0, 170.0) / 2.0).to_radians().tan();
    (framing_radius(e).max(0.5) / (FRAME_FILL * half)).max(2.5)
}

/// An orbit's radius on the ground and height above the box's centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitFrame {
    pub radius_m: f32,
    pub height_m: f32,
    /// From the camera to the centre.
    pub distance_m: f32,
}

/// The orbit round `e`: `radius` (a ground distance) and `height` when given, else what frames
/// the box: the camera [`ORBIT_ELEVATION_DEG`] above the horizon at [`frame_distance`].
pub fn orbit_frame(
    e: &Extent,
    fov_y_deg: f32,
    radius: Option<f32>,
    height: Option<f32>,
) -> OrbitFrame {
    let d = frame_distance(e, fov_y_deg);
    let (r, h) = match (radius, height) {
        (Some(r), Some(h)) => (r, h),
        (Some(r), None) => (r, 3.0),
        (None, Some(h)) => ((d * d - h * h).max((0.3 * d).powi(2)).sqrt(), h),
        (None, None) => {
            let a = ORBIT_ELEVATION_DEG.to_radians();
            (d * a.cos(), d * a.sin())
        }
    };
    OrbitFrame {
        radius_m: r,
        height_m: h,
        distance_m: r.hypot(h),
    }
}

/// An orbit's start and end angles for a compass `from` and a `sweep` in degrees (positive
/// clockwise seen from above, like a compass).
pub fn orbit_angles(from: &Value, sweep_deg: f32) -> Result<(f32, f32), String> {
    let a = orbit_angle(bearing_deg(from).map_err(|e| format!("`from`: {e}"))?);
    Ok((a, a - sweep_deg))
}

/// Where a reveal ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealEnd {
    /// Straight above the target.
    TopDown,
    /// Where `film.orbit` on the same target starts.
    Orbit,
}

impl RevealEnd {
    pub fn parse(word: &str) -> Result<RevealEnd, String> {
        match word.to_lowercase().replace('-', "_").as_str() {
            "top_down" | "topdown" | "plan" | "overhead" => Ok(RevealEnd::TopDown),
            "orbit" | "oblique" | "high" => Ok(RevealEnd::Orbit),
            _ => Err(format!(
                "`end` is `top_down` (straight above, north up) or `orbit` (where film.orbit on the same target starts), got `{word}`"
            )),
        }
    }
}

/// The eye points of a reveal: from eye level `from_bearing` of the target, rising and coming in
/// along an arc to the end. A top-down end is a hair off the vertical, on the `from` side, so
/// the camera's up is the way away from `from` (north up when `from` is south).
pub fn reveal_path(
    e: &Extent,
    fov_y_deg: f32,
    from_bearing: f32,
    end: RevealEnd,
    north_up: bool,
) -> Vec<[f32; 3]> {
    let c = e.look_point();
    let dir = bearing_dir(from_bearing);
    let orbit = orbit_frame(e, fov_y_deg, None, None);
    let [w, d] = e.footprint();
    let half = (fov_y_deg / 2.0).to_radians().tan();
    let top = (d / 2.0).max(w / 2.0 / FRAME_ASPECT) / (FRAME_FILL * half) + e.height() / 2.0;
    let start_d = (orbit.distance_m * 0.9).max(4.0);
    let eye_y = e.min[1] + EYE_LEVEL_M;
    let start = [c[0] + dir[0] * start_d, eye_y, c[2] + dir[1] * start_d];
    let (end_eye, end_elev, end_d) = match end {
        RevealEnd::TopDown => {
            let hair = (top * 0.004).max(0.05);
            let side = if north_up { [0.0, 1.0] } else { dir };
            (
                [c[0] + side[0] * hair, c[1] + top, c[2] + side[1] * hair],
                90.0f32,
                top,
            )
        }
        RevealEnd::Orbit => (
            [
                c[0] + dir[0] * orbit.radius_m,
                c[1] + orbit.height_m,
                c[2] + dir[1] * orbit.radius_m,
            ],
            orbit.height_m.atan2(orbit.radius_m).to_degrees(),
            orbit.distance_m,
        ),
    };
    let start_elev = ((eye_y - c[1]) / start_d)
        .clamp(-1.0, 1.0)
        .asin()
        .to_degrees();
    let mut pts = vec![start];
    for k in 1..4 {
        let s = k as f32 / 4.0;
        // Rise first, close in later: the elevation leads the distance.
        let elev = (start_elev + (end_elev - start_elev) * s.powf(0.8)).to_radians();
        let dist = start_d + (end_d - start_d) * s;
        pts.push([
            c[0] + dir[0] * dist * elev.cos(),
            c[1] + dist * elev.sin(),
            c[2] + dir[1] * dist * elev.cos(),
        ]);
    }
    pts.push(end_eye);
    pts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::film::{Env, Film, Shot, Timeline};

    fn garden() -> Project {
        crate::example("garden").unwrap()
    }

    #[test]
    fn the_orbit_radius_comes_from_the_frame_and_the_field_of_view() {
        let p = garden();
        let plot = reference::resolve(&Scene::new(&p, None), "plot").unwrap();
        let tight = frame_distance(&plot, 60.0);
        let wide = frame_distance(&plot, 90.0);
        assert!(wide < tight, "{wide} {tight}");
        // The target fills the vertical field of view by the fill share, to a bounding sphere.
        let seen = tight * (30.0f32.to_radians().tan()) * FRAME_FILL;
        assert!((seen - framing_radius(&plot)).abs() < 1e-3, "{seen}");
        // The plot is about 20 by 31 m: framed from a few tens of metres, not hundreds.
        assert!((25.0..45.0).contains(&tight), "{tight}");
        let f = orbit_frame(&plot, 60.0, None, None);
        assert!((f.distance_m - tight).abs() < 1e-3);
        assert!((f.height_m / f.distance_m - ORBIT_ELEVATION_DEG.to_radians().sin()).abs() < 1e-4);
        // A small thing is closer than the plot; a metres radius and height win.
        let pond = reference::resolve(&Scene::new(&p, None), "Pond").unwrap();
        assert!(orbit_frame(&pond, 60.0, None, None).radius_m < f.radius_m);
        let own = orbit_frame(&pond, 60.0, Some(9.0), Some(4.0));
        assert_eq!((own.radius_m, own.height_m), (9.0, 4.0));
        // A height alone keeps the framing distance.
        let h = orbit_frame(&plot, 60.0, None, Some(10.0));
        assert!((h.distance_m - tight).abs() < 0.01 && h.height_m == 10.0);
    }

    #[test]
    fn an_orbit_from_a_compass_point_starts_there_and_sweeps_clockwise() {
        let (a, b) = orbit_angles(&json!("south"), 90.0).unwrap();
        assert_eq!((a, b), (-90.0, -180.0));
        // Starting south of the centre: +Z of it.
        let eye = [a.to_radians().cos(), -a.to_radians().sin()];
        assert!(eye[0].abs() < 1e-5 && (eye[1] - 1.0).abs() < 1e-5);
        // 90 degrees clockwise from south is west: -X.
        let end = [b.to_radians().cos(), -b.to_radians().sin()];
        assert!((end[0] + 1.0).abs() < 1e-5 && end[1].abs() < 1e-5);
        let (a, _) = orbit_angles(&json!("north-east"), 360.0).unwrap();
        assert_eq!(a, 45.0);
        assert!(orbit_angles(&json!("sideways"), 90.0).is_err());
    }

    #[test]
    fn a_reveal_rises_from_eye_level_and_ends_top_down_with_north_up() {
        let p = garden();
        let plot = reference::resolve(&Scene::new(&p, None), "plot").unwrap();
        let pts = reveal_path(&plot, 60.0, 180.0, RevealEnd::TopDown, true);
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        assert!((first[1] - (plot.min[1] + 1.7)).abs() < 1e-4);
        assert!(
            first[2] > plot.center()[2],
            "the start is south of the target"
        );
        let c = plot.look_point();
        assert!((last[0] - c[0]).abs() < 0.5 && last[1] > 20.0, "{last:?}");
        // Rising all the way.
        assert!(pts.windows(2).all(|w| w[1][1] > w[0][1]), "{pts:?}");
        // Looking at the centre from the last point: nearly straight down, a hair to the north,
        // so the camera's up is north.
        let look = [c[0] - last[0], c[1] - last[1], c[2] - last[2]];
        let h = look[0].hypot(look[2]);
        assert!(
            look[1] < 0.0 && h / look[1].abs() < 0.02 && look[2] < 0.0,
            "{look:?}"
        );
        // As a film: the camera at the end looks at the plot.
        let shot = Shot::from_value(
            &json!({"kind": "glide", "path": pts, "look_at": c, "duration_s": 8}),
            50.8,
        )
        .unwrap();
        let film = Film {
            id: "f1".into(),
            name: "x".into(),
            fps: 25,
            shots: vec![shot],
        };
        let ground = |_: f32, _: f32| 0.0f32;
        let env = Env {
            ground: &ground,
            viewpoints: &[],
            latitude: 50.8,
            project: None,
            resolve: None,
        };
        let end = Timeline::new(&film, &env).unwrap().camera_at(8.0).unwrap();
        assert!((end.eye[1] - last[1]).abs() < 0.01 && end.target == c);
        // `orbit` ends where film.orbit starts: the same eye.
        let o = reveal_path(&plot, 60.0, 180.0, RevealEnd::Orbit, true);
        let f = orbit_frame(&plot, 60.0, None, None);
        let e = o[o.len() - 1];
        assert!(
            (e[1] - (c[1] + f.height_m)).abs() < 1e-3 && (e[2] - (c[2] + f.radius_m)).abs() < 1e-3
        );
    }

    #[test]
    fn references_in_shots_become_points_and_a_route_goes_round_the_pond() {
        let p = garden();
        let scene = Scene::new(&p, None);
        let (out, notes) = resolve_command(
            "film.add_shot",
            &json!({"id": "f1", "shot": {
                "kind": "hold", "eye": "v1", "target": "g4", "duration_s": 3}}),
            &scene,
        )
        .unwrap();
        assert_eq!(out["shot"]["eye"], json!([30.0, 22.0, 42.0]));
        let t = out["shot"]["target"].as_array().unwrap();
        assert!(
            t[1].as_f64().unwrap() > 3.0,
            "the birch's crown centre is up: {t:?}"
        );
        assert_eq!(notes.resolved.len(), 2);
        // A walk by route avoids the pond and the hedge; the same by path warns.
        let (out, notes) = resolve_command(
            "film.add_shot",
            &json!({"id": "f1", "shot": {"kind": "walk", "route": [[5, 22.5], [16, 22.5]]}}),
            &scene,
        )
        .unwrap();
        assert!(out["shot"].get("route").is_none());
        assert!(out["shot"]["path"].as_array().unwrap().len() > 2);
        assert!(notes.warnings.is_empty(), "{:?}", notes.warnings);
        assert!(notes.route.unwrap()["length_m"].as_f64().unwrap() > 13.0);
        let (_, notes) = resolve_command(
            "film.add_shot",
            &json!({"id": "f1", "shot": {"kind": "walk", "path": [[5, 22.5], [16, 22.5]]}}),
            &scene,
        )
        .unwrap();
        assert!(
            notes.warnings.iter().any(|w| w.contains("water S6")),
            "{:?}",
            notes.warnings
        );
        // Names that do not exist are refused with what is close.
        let err = resolve_command(
            "film.add_shot",
            &json!({"id": "f1", "shot": {"kind": "hold", "eye": [0, 1, 0], "target": "Pnd", "duration_s": 3}}),
            &scene,
        )
        .unwrap_err();
        assert!(
            err.contains("`target`") && err.contains("close matches"),
            "{err}"
        );
        // `look_at: "ahead"` is left alone; a pause by target and a look by name resolve.
        let (out, _) = resolve_command(
            "film.add_shot",
            &json!({"id": "f1", "shot": {"kind": "walk", "path": [[1, 14], [18, 14]], "look_at": "ahead",
                "pause_at": [{"target": "Pond", "s": 2}], "look": [{"at_s": 1, "target": "S6"}]}}),
            &scene,
        )
        .unwrap();
        assert_eq!(out["shot"]["look_at"], "ahead");
        assert!(out["shot"]["pause_at"][0]["point"].is_array());
        assert!(out["shot"]["look"][0]["target"].is_array());
        // Orbit targets become boxes; viewpoints and cameras get points.
        let (out, _) = resolve_command("film.orbit", &json!({"target": "plot"}), &scene).unwrap();
        assert!(out["target"]["max"].is_array());
        let (out, _) = resolve_command(
            "viewpoint.add",
            &json!({"name": "x", "position": [1, 2, 3], "target": "S6"}),
            &scene,
        )
        .unwrap();
        assert_eq!(out["position"], json!([1, 2, 3]));
        assert_eq!(out["target"][0].as_f64().unwrap(), 10.5);
    }
}
