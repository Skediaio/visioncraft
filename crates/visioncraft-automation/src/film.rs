//! `film.preview` and `film.render`: a film's camera at a time, and its frames as numbered
//! PNGs. The camera comes from the core's [`Timeline`] over the host's ground (terrain, areas
//! and models, as `query.ground_height` answers); each frame is a `view.render` with the
//! camera, the film's time as the animation clock and the shot's changes for that picture only.

use crate::commands;
use crate::fields::Fields;
use crate::host::Host;
use crate::inspect;
use serde_json::{Value, json};
use visioncraft_core::film::{Film, Timeline, sun_time};

/// Most frames one `film.render` makes; render a long film in parts (`from`, `to`).
pub const MAX_FRAMES: usize = 10_000;

fn film_of(host: &mut dyn Host, id: &str, name: &str) -> Result<Film, String> {
    let films = &host.session().project().films;
    films.iter().find(|f| f.id == id).cloned().ok_or_else(|| {
        let ids: Vec<&str> = films.iter().map(|f| f.id.as_str()).collect();
        format!(
            "{name}: no film `{id}`; the films are: {}",
            if ids.is_empty() {
                "none".into()
            } else {
                ids.join(", ")
            }
        )
    })
}

fn timeline(host: &mut dyn Host, film: &Film, name: &str) -> Result<Timeline, String> {
    let (session, state) = host.parts();
    inspect::film_timeline(session, state, film).map_err(|e| format!("{name}: {e}"))
}

fn camera_json(f: &visioncraft_core::film::Frame) -> Value {
    json!({"position": f.eye, "target": f.target, "fov_y_deg": f.fov_y_deg})
}

pub(crate) fn preview(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let name = "film.preview";
    let a = Fields::new(name, "parameter", Some(params), &["id", "t"])?;
    let film = film_of(host, a.required_str("id")?, name)?;
    let t = a.number("t")?;
    let tl = timeline(host, &film, name)?;
    let shots: Vec<Value> = film
        .shots
        .iter()
        .zip(tl.spans())
        .enumerate()
        .map(|(i, (s, (start, d)))| {
            json!({"index": i, "kind": s.kind.name(), "start_s": start, "duration_s": d})
        })
        .collect();
    let mut out = json!({
        "id": film.id, "name": film.name, "fps": film.fps,
        "duration_s": tl.duration_s, "frame_count": tl.frame_count(), "shots": shots,
    });
    if let Some(t) = t {
        if !t.is_finite() || t < 0.0 {
            return Err(format!("{name}: `t` must be a time in seconds, 0 or more"));
        }
        let f = tl.camera_at(t as f32).map_err(|e| format!("{name}: {e}"))?;
        let project = host.session().project().clone();
        let mut at = project.clone();
        at.time = sun_time(project.time, &f.overrides, project.site.latitude);
        out["t"] = json!(t.min(f64::from(tl.duration_s)));
        out["shot"] = json!(f.shot);
        out["progress"] = json!(f.progress);
        out["camera"] = camera_json(&f);
        out["overrides"] = json!(f.overrides);
        out["time"] = json!(at.time);
        out["sun"] = inspect::sun_json(&at);
    }
    Ok(out)
}

/// The frames `film.render` makes: indices into the film's frames.
fn frame_indices(
    tl: &Timeline,
    from: Option<f64>,
    to: Option<f64>,
    every: usize,
    name: &str,
) -> Result<Vec<usize>, String> {
    let total = tl.frame_count();
    let fps = f64::from(tl.fps);
    let first = from.map_or(0, |s| (s * fps - 1e-6).ceil().max(0.0) as usize);
    let last = to.map_or(total.saturating_sub(1), |s| {
        ((s * fps + 1e-6).floor().max(0.0) as usize).min(total.saturating_sub(1))
    });
    let frames: Vec<usize> = (first..=last).step_by(every).collect();
    if total == 0 || first > last || frames.is_empty() {
        return Err(format!(
            "{name}: no frames between `from` and `to` (the film lasts {} s)",
            tl.duration_s
        ));
    }
    if frames.len() > MAX_FRAMES {
        return Err(format!(
            "{name}: {} frames is over the limit of {MAX_FRAMES} for one call; render a part (`from`, `to`) or every Nth frame (`every`)",
            frames.len()
        ));
    }
    Ok(frames)
}

pub(crate) fn render(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    render_with_progress(host, params, &mut |_, _| {})
}

/// `film.render`, telling `progress(done, total)` after each frame.
pub fn render_with_progress(
    host: &mut dyn Host,
    params: &Value,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Value, String> {
    let name = "film.render";
    let a = Fields::new(
        name,
        "parameter",
        Some(params),
        &[
            "id", "dir", "width", "height", "quality", "from", "to", "every",
        ],
    )?;
    let film = film_of(host, a.required_str("id")?, name)?;
    let dir = a.required_str("dir")?;
    let width = a.integer("width", 16, 4096)?.unwrap_or(960);
    let height = a.integer("height", 16, 4096)?.unwrap_or(540);
    let every = a.integer("every", 1, 10_000)?.unwrap_or(1) as usize;
    let quality = a.str("quality")?.unwrap_or("high");
    let (from, to) = (a.number("from")?, a.number("to")?);
    let tl = timeline(host, &film, name)?;
    let frames = frame_indices(&tl, from, to, every, name)?;
    // The folder is resolved as view.render resolves a picture's path: beneath the write root.
    let dir = host
        .state()
        .access
        .resolve(dir, true, None)
        .map_err(|e| format!("{name}: {e}"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("{name}: cannot make `{}`: {e}", dir.display()))?;
    let fps = f64::from(film.fps);
    let mut last = Value::Null;
    for (n, i) in frames.iter().enumerate() {
        let t = *i as f64 / fps;
        let f = tl.camera_at(t as f32).map_err(|e| format!("{name}: {e}"))?;
        let mut p = f.overrides.render_params();
        p.insert(
            "path".into(),
            dir.join(format!("frame_{n:05}.png"))
                .to_string_lossy()
                .into_owned()
                .into(),
        );
        p.insert("width".into(), width.into());
        p.insert("height".into(), height.into());
        p.insert("quality".into(), quality.into());
        p.insert("camera".into(), camera_json(&f));
        p.insert("animation_s".into(), t.into());
        last = commands::execute(host, "view.render", &Value::Object(p))
            .map_err(|e| format!("{name}: frame {i} (t = {t:.2} s): {e}"))?;
        progress(n + 1, frames.len());
    }
    let out_fps = fps / every as f64;
    let first_t = frames[0] as f64 / fps;
    Ok(json!({
        "dir": dir.display().to_string(), "frames": frames.len(),
        "framePattern": "frame_%05d.png", "fps": out_fps,
        "width": width, "height": height,
        "duration_s": tl.duration_s, "first_t": first_t,
        "last_t": frames[frames.len() - 1] as f64 / fps,
        "renderer": last.get("renderer").cloned().unwrap_or(Value::Null),
        "ffmpeg": format!(
            "ffmpeg -framerate {out_fps} -i {}/frame_%05d.png -c:v libx264 -pix_fmt yuv420p film.mp4",
            dir.display()
        ),
    }))
}
