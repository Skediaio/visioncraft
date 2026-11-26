//! What plays beside the project: `model.play_clip`, `model.stop_clip`, `query.part_pose` and
//! `effect.flash`. The state is the host's [`Playback`] (core) and the models' files are the
//! host's ([`Host::animation`]); this reads the parameters and shapes the answers.

use crate::fields::{Fields, describe};
use crate::host::Host;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use visioncraft_core::keyframes::Sampler;
use visioncraft_core::planting::{self, Generator};
use visioncraft_core::{
    Animation, ClipTarget, Flash, Life, MAX_FLASHES, Played, Project, Spot, clip_index,
};

/// Side of a flash's glowing quad unless given (m).
pub const FLASH_SIZE_M: f64 = 0.15;
/// Frames a flash lasts unless given, or `seconds` is.
pub const FLASH_FRAMES: u64 = 2;
/// Most frames one flash may last.
pub const FLASH_MAX_FRAMES: u64 = 1000;
/// Most seconds one flash may last (a day: a film is never longer).
pub const FLASH_MAX_SECONDS: f64 = 86_400.0;

/// `params[key]`, or why it is missing or of the wrong shape.
fn need<T: DeserializeOwned>(id: &str, params: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(params[key].clone())
        .map_err(|e| format!("{id}: `{key}`: {}", missing_or_shape(params, key, &e)))
}

/// "missing `k`" when the key is absent, else serde's message.
fn missing_or_shape(params: &Value, key: &str, e: &serde_json::Error) -> String {
    if params.get(key).is_none_or(Value::is_null) {
        format!("missing `{key}`")
    } else {
        e.to_string()
    }
}

/// A finite number from `min` to `max`.
fn number_in(a: &Fields, id: &str, key: &str, min: f64, max: f64) -> Result<Option<f64>, String> {
    match a.number(key)? {
        None => Ok(None),
        Some(v) if v.is_finite() && (min..=max).contains(&v) => Ok(Some(v)),
        Some(v) => Err(format!(
            "{id}: `{key}` must be a number from {min} to {max}, got {v}"
        )),
    }
}

/// `object` as given: a planting, or one plant of it.
fn target(id: &str, object: &str) -> Result<ClipTarget, String> {
    ClipTarget::parse(object).map_err(|e| format!("{id}: `object`: {e}"))
}

/// A plant a command reaches: its number in its planting and its asset.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Plant {
    number: u32,
    asset: String,
}

/// The plants `target` reaches, in order: a plant taken out is missing, the others keep their
/// numbers (`planting.replace_placement`'s counting).
fn plants(project: &Project, id: &str, target: &ClipTarget) -> Result<Vec<Plant>, String> {
    let ids: Vec<&str> = project.generators.iter().map(Generator::id).collect();
    let g = project
        .generators
        .iter()
        .find(|g| g.id() == target.planting)
        .ok_or_else(|| {
            format!(
                "{id}: `object`: no planting `{}`; the plantings are: {}",
                target.planting,
                if ids.is_empty() {
                    "none".into()
                } else {
                    ids.join(", ")
                }
            )
        })?;
    if matches!(g, Generator::SurfaceFill(_)) {
        return Err(format!(
            "{id}: `object`: `{}` is a surface fill, whose plants follow its owner's faces; name a planting on the ground",
            target.planting
        ));
    }
    let floor = visioncraft_core::terrace::ground_fn(project);
    let ground = |x: f32, z: f32| floor(x, z).or(Some(0.0));
    let numbered = planting::evaluate_numbered(g, &ground).0;
    let count = numbered.len();
    let plants: Vec<Plant> = numbered
        .into_iter()
        .filter(|(n, _)| target.plant.is_none_or(|p| p == *n as u32))
        .map(|(n, p)| Plant {
            number: n as u32,
            asset: p.asset,
        })
        .collect();
    if plants.is_empty() {
        return Err(match target.plant {
            Some(n) => format!(
                "{id}: `object`: planting `{}` has no plant {n}: its plants are numbered from 0 as planting.replace_placement counts them, a plant taken out is missing (it places {count})",
                target.planting
            ),
            None => format!(
                "{id}: `object`: planting `{}` places nothing",
                target.planting
            ),
        });
    }
    Ok(plants)
}

/// The distinct assets of `plants`, sorted.
fn assets_of(plants: &[Plant]) -> Vec<String> {
    let mut assets: Vec<String> = plants.iter().map(|p| p.asset.clone()).collect();
    assets.sort();
    assets.dedup();
    assets
}

/// The animations of `assets` as the host has them.
fn animations(
    host: &mut dyn Host,
    id: &str,
    assets: &[String],
) -> Result<Vec<(String, Animation)>, String> {
    let mut out = Vec::new();
    for asset in assets {
        let animation = host.animation(asset).ok_or_else(|| {
            format!(
                "{id}: this session has no model files, so the clips of `{asset}` are not known here; the window, `visioncraft-cli` and the MCP server have them"
            )
        })?;
        out.push((asset.clone(), animation?));
    }
    Ok(out)
}

/// `clip` as given: a name, or an index (a whole number, or a numeric string).
fn clip_text(id: &str, a: &Fields) -> Result<String, String> {
    match a.get("clip") {
        None => Err(a.missing("clip")),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|n| n.to_string())
            .ok_or_else(|| format!("{id}: `clip` must be a clip's name or its index from 0")),
        Some(other) => Err(format!(
            "{id}: `clip` must be a clip's name or its index from 0, got {}",
            describe(other)
        )),
    }
}

/// The clip `text` names in every asset's clips (its index per asset), and its name and its
/// longest length.
fn resolve(
    id: &str,
    text: &str,
    animations: &[(String, Animation)],
) -> Result<(BTreeMap<String, usize>, String, f32), String> {
    let mut clips = BTreeMap::new();
    let (mut name, mut duration) = (String::new(), 0.0_f32);
    for (asset, animation) in animations {
        let index = clip_index(&animation.clips, text)
            .map_err(|e| format!("{id}: `clip` for `{asset}`: {e}"))?;
        name = animation.clips[index].name.clone();
        duration = duration.max(animation.clips[index].duration);
        clips.insert(asset.clone(), index);
    }
    Ok((clips, name, duration))
}

/// The host's playback.
fn playback(host: &mut dyn Host) -> Result<&mut visioncraft_core::Playback, String> {
    host.playback()
        .ok_or_else(|| "this host keeps no playback".to_string())
}

/// `model.play_clip {object, clip, start_s?, loop?}`.
pub(crate) fn play_clip(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    let a = Fields::new(
        id,
        "parameter",
        Some(params),
        &["object", "clip", "start_s", "loop"],
    )?;
    let target = target(id, a.required_str("object")?)?;
    let text = clip_text(id, &a)?;
    let start_s = match a.number("start_s")? {
        None => None,
        Some(v) if v.is_finite() => Some(v as f32),
        Some(v) => {
            return Err(format!(
                "{id}: `start_s` must be a finite number of seconds, got {v}"
            ));
        }
    };
    let looped = a.bool("loop")?.unwrap_or(false);
    let plants = plants(host.session().project(), id, &target)?;
    let animations = animations(host, id, &assets_of(&plants))?;
    let (clips, name, duration) = resolve(id, &text, &animations)?;
    let playback = playback(host)?;
    let start_s = start_s.unwrap_or(playback.clock as f32);
    let restarted = playback.play(target.clone(), clips, &name, start_s, looped, duration);
    Ok(json!({
        "object": target.name(),
        "planting": target.planting,
        "plant": target.plant,
        "plants": plants.len(),
        "clip": name,
        "start_s": start_s,
        "loop": looped,
        "duration_s": duration,
        "restarted": restarted,
        "playing": playback.plays().len(),
    }))
}

/// `model.stop_clip {object, clip?}`.
pub(crate) fn stop_clip(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    let a = Fields::new(id, "parameter", Some(params), &["object", "clip"])?;
    let target = target(id, a.required_str("object")?)?;
    let plants = plants(host.session().project(), id, &target)?;
    let clips = match a.get("clip") {
        None => None,
        Some(_) => {
            let text = clip_text(id, &a)?;
            let animations = animations(host, id, &assets_of(&plants))?;
            Some(resolve(id, &text, &animations)?.0)
        }
    };
    let playback = playback(host)?;
    let stopped = playback.stop(&target, clips.as_ref());
    Ok(json!({
        "object": target.name(),
        "stopped": stopped,
        "playing": playback.plays().len(),
    }))
}

/// `query.part_pose {object, t?}`: for each plant the object names, the matrix in model space
/// of every moving part at clock time `t` (the session's clock), column-major.
pub(crate) fn part_pose(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    let a = Fields::new(id, "parameter", Some(params), &["object", "t"])?;
    let target = target(id, a.required_str("object")?)?;
    let t = match a.number("t")? {
        None => None,
        Some(v) if v.is_finite() && v >= 0.0 => Some(v),
        Some(v) => {
            return Err(format!(
                "{id}: `t` must be a time in seconds, 0 or more, got {v}"
            ));
        }
    };
    let plants = plants(host.session().project(), id, &target)?;
    let animations = animations(host, id, &assets_of(&plants))?;
    let playback = playback(host)?;
    let t = t.unwrap_or(playback.clock);
    let mut out = Vec::new();
    let (mut sampler, mut played) = (Sampler::default(), Vec::<Played>::new());
    for plant in &plants {
        let animation = &animations
            .iter()
            .find(|(asset, _)| asset == &plant.asset)
            .expect("each asset was resolved")
            .1;
        let Some(rig) = &animation.rig else {
            return Err(format!(
                "{id}: `{}` (plant {} of `{}`) stands still: it has no moving parts",
                plant.asset, plant.number, target.planting
            ));
        };
        played.clear();
        played.extend(playback.played(&Spot {
            planting: &target.planting,
            plant: Some(plant.number),
            asset: &plant.asset,
        }));
        let pose = sampler.pose(rig, &played, t);
        out.push(json!({
            "plant": plant.number,
            "asset": plant.asset,
            "posed": !played.is_empty(),
            "clips": played.iter().map(|p| rig.clips[p.clip].name.clone()).collect::<Vec<_>>(),
            "parts": pose.iter().collect::<Vec<_>>(),
        }));
    }
    Ok(json!({ "object": target.name(), "t": t, "plants": out }))
}

/// `effect.flash {position, color, intensity, range_m, size_m?, frames? | seconds?, start_s?}`.
pub(crate) fn flash(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    let a = Fields::new(
        id,
        "parameter",
        Some(params),
        &[
            "position",
            "color",
            "intensity",
            "range_m",
            "size_m",
            "frames",
            "seconds",
            "start_s",
        ],
    )?;
    let position: [f32; 3] = need(id, params, "position")?;
    if !position.iter().all(|c| c.is_finite()) {
        return Err(format!(
            "{id}: `position` must be three finite numbers [x, y, z]"
        ));
    }
    let color: [f32; 3] = need(id, params, "color")?;
    if !color
        .iter()
        .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
    {
        return Err(format!(
            "{id}: `color` must be three numbers from 0 to 1 (linear red, green, blue)"
        ));
    }
    let intensity =
        number_in(&a, id, "intensity", 0.0, 1e9)?.ok_or_else(|| a.missing("intensity"))?;
    let range_m =
        number_in(&a, id, "range_m", 0.01, 10_000.0)?.ok_or_else(|| a.missing("range_m"))?;
    let size_m = number_in(&a, id, "size_m", 0.0, 100.0)?.unwrap_or(FLASH_SIZE_M);
    let frames = a.integer("frames", 1, FLASH_MAX_FRAMES)?;
    let seconds = match a.number("seconds")? {
        None => None,
        Some(v) if v.is_finite() && v > 0.0 && v <= FLASH_MAX_SECONDS => Some(v),
        Some(v) => {
            return Err(format!(
                "{id}: `seconds` must be a number of seconds above 0 and at most {FLASH_MAX_SECONDS}, got {v}"
            ));
        }
    };
    let life = match (frames, seconds) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{id}: `frames` and `seconds` each say how long the flash lasts; give one"
            ));
        }
        (Some(n), None) => Life::Frames(n as u32),
        (None, Some(s)) => Life::Seconds(s as f32),
        (None, None) => Life::Frames(FLASH_FRAMES as u32),
    };
    let playback = playback(host)?;
    let start_s = match a.number("start_s")? {
        None => playback.clock as f32,
        Some(v) if v.is_finite() => v as f32,
        Some(v) => {
            return Err(format!(
                "{id}: `start_s` must be a finite number of seconds, got {v}"
            ));
        }
    };
    let full = playback.flashes().len() >= MAX_FLASHES;
    let flash_id = playback.flash(Flash {
        id: 0,
        start_s,
        position,
        color,
        intensity: intensity as f32,
        range_m: range_m as f32,
        size_m: size_m as f32,
        life,
    });
    let mut out = json!({
        "id": flash_id,
        "start_s": start_s,
        "size_m": size_m,
        "flashes": playback.flashes().len(),
    });
    match life {
        Life::Frames(n) => out["frames"] = json!(n),
        Life::Seconds(s) => out["seconds"] = json!(s),
    }
    if full {
        out["note"] = json!(format!(
            "the oldest flash was dropped: at most {MAX_FLASHES} play at once"
        ));
    }
    Ok(out)
}
