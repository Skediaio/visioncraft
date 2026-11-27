//! What plays beside the project: `model.play_clip`, `model.stop_clip`, `query.part_pose` and
//! `effect.flash`. The state is the host's [`Playback`] (core) and the models' files are the
//! host's ([`Host::animation`]); this reads the parameters and shapes the answers.

use crate::fields::{Fields, describe};
use crate::host::Host;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use visioncraft_core::keyframes::{Matrix, Sampler, mul};
use visioncraft_core::planting::{self, Generator};
use visioncraft_core::{
    Animation, ClipTarget, Flash, FlashDrop, Life, MAX_FLASHES, MAX_STORED_FLASHES, Played, Spot,
    clip_index,
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

/// A plant a command reaches: its number in its planting, its asset, and the matrix the window
/// draws it with (a moving part's world matrix is this times its model-space matrix).
#[derive(Clone, Debug, PartialEq)]
struct Plant {
    number: u32,
    asset: String,
    matrix: Matrix,
}

/// Which plants of a planting a command reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    /// The plants the window draws: what the building rule and the sea leave out is missing.
    Drawn,
    /// Every plant the recipe places: a play is stopped also on a plant hidden since.
    Placed,
}

/// The plants `target` reaches, in order: the others keep their numbers
/// (`planting.replace_placement`'s counting).
fn plants(
    host: &mut dyn Host,
    id: &str,
    target: &ClipTarget,
    reach: Reach,
) -> Result<Vec<Plant>, String> {
    let (session, state) = host.parts();
    let project = session.project();
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
    let assets = &state.catalog.assets;
    let (numbered, _) = match reach {
        Reach::Drawn => crate::inspect::plants_of(session, state, g),
        Reach::Placed => {
            let floor = visioncraft_core::terrace::ground_fn(project);
            let ground = |x: f32, z: f32| floor(x, z).or(Some(0.0));
            (planting::evaluate_numbered(g, &ground).0, false)
        }
    };
    let placed = numbered.len();
    let plants: Vec<Plant> = numbered
        .into_iter()
        .filter(|(n, _)| target.plant.is_none_or(|p| p == *n as u32))
        .map(|(n, p)| Plant {
            number: n as u32,
            matrix: crate::inspect::drawn_matrix(project, assets, g, &p),
            asset: p.asset,
        })
        .collect();
    if plants.is_empty() {
        return Err(match reach {
            Reach::Drawn => not_drawn(id, target, g, session, state),
            Reach::Placed => not_placed(id, target, placed),
        });
    }
    Ok(plants)
}

/// Why `target` reaches no plant among those the recipe places: there is none it names.
fn not_placed(id: &str, target: &ClipTarget, placed: usize) -> String {
    match target.plant {
        Some(n) => format!(
            "{id}: `object`: planting `{}` has no plant {n}: its plants are numbered from 0 as planting.replace_placement counts them, a plant taken out is missing (it places {placed})",
            target.planting
        ),
        None => format!(
            "{id}: `object`: planting `{}` places nothing",
            target.planting
        ),
    }
}

/// Why `target` reaches no plant: the planting places none it names, or the plants it names
/// are not drawn (the building rule, the sea).
fn not_drawn(
    id: &str,
    target: &ClipTarget,
    g: &Generator,
    session: &visioncraft_core::Session,
    state: &crate::host::HostState,
) -> String {
    let stages = crate::inspect::stages(session, state, g);
    let planting = &target.planting;
    match target.plant {
        Some(n) => {
            if !stages.placed.contains(&(n as usize)) {
                return not_placed(id, target, stages.placed.len());
            }
            let why = if !stages.clear.contains(&(n as usize)) {
                "it stands inside a building"
            } else {
                "it is under the sea"
            };
            format!("{id}: `object`: plant {n} of planting `{planting}` is not drawn: {why}")
        }
        None if stages.placed.is_empty() => not_placed(id, target, 0),
        None => {
            let in_building = stages.placed.len() - stages.clear.len();
            let under_sea = stages.clear.len();
            let mut why = Vec::new();
            if in_building > 0 {
                why.push(format!("{in_building} stand inside a building"));
            }
            if under_sea > 0 {
                why.push(format!("{under_sea} are under the sea"));
            }
            format!(
                "{id}: `object`: planting `{planting}` draws none of its {} plants: {}",
                stages.placed.len(),
                why.join(" and ")
            )
        }
    }
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
        Some(v) if v.is_finite() => Some(v),
        Some(v) => {
            return Err(format!(
                "{id}: `start_s` must be a finite number of seconds, got {v}"
            ));
        }
    };
    let looped = a.bool("loop")?.unwrap_or(false);
    let plants = plants(host, id, &target, Reach::Drawn)?;
    let animations = animations(host, id, &assets_of(&plants))?;
    let (clips, name, duration) = resolve(id, &text, &animations)?;
    let playback = playback(host)?;
    // The clock itself, not its `f32` rounding: after a day of uptime that is a frame off.
    let start_s = start_s.unwrap_or(playback.clock);
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
    // Every plant the recipe places: a clip on a plant hidden since is stopped all the same.
    let plants = plants(host, id, &target, Reach::Placed)?;
    let clips = match a.get("clip") {
        None => None,
        Some(_) => {
            let text = clip_text(id, &a)?;
            let animations = animations(host, id, &assets_of(&plants))?;
            Some(resolve(id, &text, &animations)?.0)
        }
    };
    let playback = playback(host)?;
    // One object of a planting that plays as a whole would go on moving with it.
    if playback.plays_on_its_planting(&target, clips.as_ref()) {
        return Err(format!(
            "{id}: `{}` plays as part of its whole planting: stop `{}` (every object of it) instead",
            target.name(),
            target.planting
        ));
    }
    let stopped = playback.stop(&target, clips.as_ref());
    Ok(json!({
        "object": target.name(),
        "stopped": stopped,
        "playing": playback.plays().len(),
    }))
}

/// The space `query.part_pose` answers in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Space {
    /// The world, as the window draws it: a part's matrix is its plant's `placement` times the
    /// part's matrix in model space.
    World,
    /// The model's own space: a part's matrix on its own.
    Model,
}

impl Space {
    fn name(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Model => "model",
        }
    }
}

/// `space` as given: `world` (the default) or `model`.
fn space(id: &str, a: &Fields) -> Result<Space, String> {
    match a.get("space") {
        None => Ok(Space::World),
        Some(Value::String(s)) if s == "world" => Ok(Space::World),
        Some(Value::String(s)) if s == "model" => Ok(Space::Model),
        Some(other) => Err(format!(
            "{id}: `space` must be `world` or `model`, got {}",
            describe(other)
        )),
    }
}

/// `query.part_pose {object, t?, space?}`: for each plant the object names, the matrix of every
/// moving part at clock time `t` (the session's clock), column-major, 16 numbers. In `world`
/// (the default) as the window draws it: the plant's `placement` (scale, then Y rotation, then
/// translation, under the growth of the years shown) times the part's matrix in model space; in
/// `model` that matrix on its own. Each plant tells its `placement`, to compose or invert.
/// Only the plants the window draws are reached (the building rule, the sea).
pub(crate) fn part_pose(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    let a = Fields::new(id, "parameter", Some(params), &["object", "t", "space"])?;
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
    let space = space(id, &a)?;
    let plants = plants(host, id, &target, Reach::Drawn)?;
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
        let parts: Vec<Matrix> = match space {
            Space::World => pose.iter().map(|m| mul(&plant.matrix, m)).collect(),
            Space::Model => pose.to_vec(),
        };
        out.push(json!({
            "plant": plant.number,
            "asset": plant.asset,
            "placement": plant.matrix,
            "posed": !played.is_empty(),
            "clips": played.iter().map(|p| rig.clips[p.clip].name.clone()).collect::<Vec<_>>(),
            "parts": parts,
        }));
    }
    Ok(json!({ "object": target.name(), "t": t, "space": space.name(), "plants": out }))
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
    // Without a time of its own it starts at the clock now, and is forgotten once over; with
    // one it is on a film's time, and stays for every render of the film.
    let given = a.number("start_s")?;
    let start_s = match given {
        None => playback.clock,
        Some(v) if v.is_finite() => v,
        Some(v) => {
            return Err(format!(
                "{id}: `start_s` must be a finite number of seconds, got {v}"
            ));
        }
    };
    // Live, what is over on the session's clock goes first (a flash on a film's time stays).
    playback.prune_live(playback.clock);
    let flash = Flash {
        id: 0,
        start_s,
        position,
        color,
        intensity: intensity as f32,
        range_m: range_m as f32,
        size_m: size_m as f32,
        life,
    };
    let (flash_id, dropped) = playback.add_flash(flash, given.is_none());
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
    match dropped {
        Some(FlashDrop::AtOnce) => {
            out["note"] = json!(format!(
                "the oldest flash at the same time was dropped: at most {MAX_FLASHES} play at once"
            ));
        }
        Some(FlashDrop::Stored) => {
            out["note"] = json!(format!(
                "the flash that ends first was dropped: at most {MAX_STORED_FLASHES} flashes with a `start_s` are kept"
            ));
        }
        None => {}
    }
    Ok(out)
}
