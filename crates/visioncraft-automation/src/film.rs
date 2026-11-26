//! `film.preview` and `film.render`: a film's camera at a time, and its frames as numbered
//! PNGs. The camera comes from the core's [`Timeline`] over the host's ground (terrain, areas
//! and models, as `query.ground_height` answers); each frame is a `view.render` with the
//! camera, the film's time as the animation clock and the shot's changes for that picture only.

use crate::commands;
use crate::fields::Fields;
use crate::host::Host;
use crate::inspect;
use crate::sheet;
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

/// The `view.render` parameters of the film at time `t`.
fn frame_params(
    f: &visioncraft_core::film::Frame,
    t: f64,
    width: u64,
    height: u64,
    quality: &str,
) -> serde_json::Map<String, Value> {
    let mut p = f.overrides.render_params();
    p.insert("width".into(), width.into());
    p.insert("height".into(), height.into());
    p.insert("quality".into(), quality.into());
    p.insert("camera".into(), camera_json(f));
    p.insert("animation_s".into(), t.into());
    p
}

/// Most pictures on a contact sheet.
const SHEET_MAX_FRAMES: u64 = 16;

/// Evenly spaced film times for `count` pictures: the first frame to the last.
fn sheet_times(tl: &Timeline, count: usize, fps: f64) -> Vec<f64> {
    let end = (f64::from(tl.duration_s) - 0.5 / fps).max(0.0);
    (0..count)
        .map(|i| {
            if count == 1 {
                end / 2.0
            } else {
                end * i as f64 / (count - 1) as f64
            }
        })
        .collect()
}

/// A contact sheet of `images`, the sheet's PNG as `{mime, base64}` with how it is laid out.
fn sheet_json(
    images: &[sheet::Image],
    times: &[f64],
    shots: &[usize],
    cell_w: u32,
) -> Result<Value, String> {
    let columns = if images.len() <= 3 {
        images.len()
    } else {
        (images.len() as f64).sqrt().ceil() as usize
    };
    let img = sheet::contact_sheet(images, columns, cell_w, 3);
    Ok(json!({
        "mime": "image/png", "base64": crate::base64(&sheet::encode(&img)?),
        "sheet": {"columns": columns, "frames": images.len(), "cell_px": [cell_w, images[0].h * cell_w / images[0].w.max(1)],
            "size_px": [img.w, img.h], "times_s": times, "shots": shots, "order": "row by row, left to right"},
    }))
}

/// `film.preview {sheet: true}`: the film at evenly spaced times, small, on one picture.
fn preview_sheet(
    host: &mut dyn Host,
    film: &Film,
    tl: &Timeline,
    a: &Fields,
    name: &str,
) -> Result<Value, String> {
    let count = a.integer("frames", 1, SHEET_MAX_FRAMES)?.unwrap_or(9) as usize;
    let cell_w = a.integer("width", 64, 640)?.unwrap_or(320) as u32;
    let quality = a.str("quality")?.unwrap_or("low");
    let height = (u64::from(cell_w) * 9 / 16).max(16);
    let fps = f64::from(film.fps);
    let times = sheet_times(tl, count, fps);
    let mut images = Vec::new();
    let mut shots = Vec::new();
    for t in &times {
        let f = tl
            .camera_at(*t as f32)
            .map_err(|e| format!("{name}: {e}"))?;
        let p = frame_params(&f, *t, u64::from(cell_w), height, quality);
        let r = commands::execute(host, "view.render", &Value::Object(p))
            .map_err(|e| format!("{name}: sheet frame at t = {t:.2} s: {e}"))?;
        let bytes = sheet::unbase64(
            r["base64"]
                .as_str()
                .ok_or("the picture came back without data")?,
        )?;
        images.push(sheet::decode(&bytes).map_err(|e| format!("{name}: {e}"))?);
        shots.push(f.shot);
    }
    sheet_json(&images, &times, &shots, cell_w).map_err(|e| format!("{name}: {e}"))
}

pub(crate) fn preview(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let name = "film.preview";
    let a = Fields::new(
        name,
        "parameter",
        Some(params),
        &["id", "t", "sheet", "frames", "width", "quality"],
    )?;
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
    if !tl.warnings.is_empty() {
        out["warnings"] = json!(tl.warnings);
    }
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
    if a.bool("sheet")?.unwrap_or(false) {
        // The picture: an image block over MCP; the numbers stay beside it.
        let sheet = preview_sheet(host, &film, &tl, &a, name)?;
        if let (Some(o), Value::Object(s)) = (out.as_object_mut(), sheet) {
            o.extend(s);
        }
    } else if ["frames", "width", "quality"]
        .iter()
        .any(|k| a.get(k).is_some())
    {
        return Err(format!(
            "{name}: `frames`, `width` and `quality` go with `sheet: true`"
        ));
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

/// The samples a film frame at quality `photo` averages unless `samples` says otherwise.
pub const FILM_PHOTO_SAMPLES: u32 = 8;

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
            "id",
            "dir",
            "width",
            "height",
            "quality",
            "from",
            "to",
            "every",
            "samples",
            "aperture",
            "focus_m",
            "focus_point",
            "shutter_s",
            "vignette",
            "grain",
            "preview",
        ],
    )?;
    let sheet_wanted = match a.str("preview")? {
        None => false,
        Some("sheet") => true,
        Some(other) => {
            return Err(format!(
                "{name}: `preview` must be \"sheet\" (a contact sheet of the frames drawn), got `{other}`"
            ));
        }
    };
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
    // A flash's `frames` last frames of the film's rate, so the same frames show it every time.
    host.state().film_fps = Some(fps);
    let mut last = Value::Null;
    for (n, i) in frames.iter().enumerate() {
        let t = *i as f64 / fps;
        let f = tl.camera_at(t as f32).map_err(|e| format!("{name}: {e}"))?;
        let mut p = frame_params(&f, t, width, height, quality);
        p.insert(
            "path".into(),
            dir.join(format!("frame_{n:05}.png"))
                .to_string_lossy()
                .into_owned()
                .into(),
        );
        // A film's photo frames average fewer samples than a still's: 8 unless told.
        if quality == "photo" && a.get("samples").is_none() {
            p.insert("samples".into(), FILM_PHOTO_SAMPLES.into());
        }
        for key in [
            "samples",
            "aperture",
            "focus_m",
            "focus_point",
            "shutter_s",
            "vignette",
            "grain",
        ] {
            if let Some(v) = a.get(key) {
                p.insert(key.into(), v.clone());
            }
        }
        last = commands::execute(host, "view.render", &Value::Object(p))
            .map_err(|e| format!("{name}: frame {i} (t = {t:.2} s): {e}"))?;
        progress(n + 1, frames.len());
    }
    let out_fps = fps / every as f64;
    let first_t = frames[0] as f64 / fps;
    let mut sheet_part = Value::Null;
    if sheet_wanted {
        // Up to nine of the frames just drawn, evenly spread, on one picture.
        let count = frames.len().min(9);
        let picks: Vec<usize> = (0..count)
            .map(|i| {
                if count == 1 {
                    0
                } else {
                    i * (frames.len() - 1) / (count - 1)
                }
            })
            .collect();
        let mut images = Vec::new();
        for n in &picks {
            let bytes = std::fs::read(dir.join(format!("frame_{n:05}.png")))
                .map_err(|e| format!("{name}: cannot read frame {n} for the sheet: {e}"))?;
            images.push(sheet::decode(&bytes).map_err(|e| format!("{name}: {e}"))?);
        }
        let times: Vec<f64> = picks.iter().map(|n| frames[*n] as f64 / fps).collect();
        let shots: Vec<usize> = times
            .iter()
            .map(|t| tl.camera_at(*t as f32).map_or(0, |f| f.shot))
            .collect();
        sheet_part = sheet_json(&images, &times, &shots, 320.min(width as u32).max(64))
            .map_err(|e| format!("{name}: {e}"))?;
    }
    host.state().film_fps = None;
    let mut result = json!({
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
    });
    if let (Some(o), Value::Object(s)) = (result.as_object_mut(), sheet_part) {
        o.extend(s);
    }
    Ok(result)
}
