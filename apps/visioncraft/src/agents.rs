//! AI agents driving the open window (desktop only): the automation crate's control channel
//! (`visioncraft-cli mcp --bridge` finds it), answered every frame on the UI thread through
//! [`WindowHost`] over the window's own `Session`, so agent commands appear live and are undone
//! with Ctrl+Z like the person's own. As in 3dcraft, nothing listens until the person turns on
//! *Allow AI agents* (or starts the window with `--control`); turning it off disconnects.

use crate::camera::{Camera, View};
use crate::planting_tools::PlantingTools;
use crate::studio::Studio;
use crate::ui::Mode;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use visioncraft_automation::control::{ControlHandle, ControlOptions, serve_control};
use visioncraft_automation::rpc::Method;
use visioncraft_automation::{Access, Host, HostState, LoadedModel, RenderRequest, Rendered};
use visioncraft_core::{Project, Session};

/// The window's control channel and what it shows about it.
pub struct Agents {
    control: Option<ControlHandle>,
    pub state: HostState,
    /// The last command an agent ran, and when (seconds of the window's clock).
    pub last: Option<(String, f64)>,
    pub error: Option<String>,
    seen_log: usize,
}

impl Default for Agents {
    fn default() -> Self {
        Self {
            control: None,
            // The person's own files: an agent may read and write where the person can.
            state: HostState::new(Access::any()),
            last: None,
            error: None,
            seen_log: 0,
        }
    }
}

impl Agents {
    pub fn allowed(&self) -> bool {
        self.control.is_some()
    }

    pub fn port(&self) -> Option<u16> {
        self.control.as_ref().map(ControlHandle::port)
    }

    /// Starts or stops listening. Stopping drops every connection and the discovery entry.
    pub fn allow(&mut self, on: bool, ctx: &eframe::egui::Context, port: Option<u16>) {
        self.allow_with(on, ctx, port, None);
    }

    fn allow_with(
        &mut self,
        on: bool,
        ctx: &eframe::egui::Context,
        port: Option<u16>,
        token_file: Option<std::path::PathBuf>,
    ) {
        if on == self.allowed() {
            return;
        }
        if !on {
            self.control = None;
            self.last = None;
            return;
        }
        match serve_control(ControlOptions {
            port,
            token_file,
            ..Default::default()
        }) {
            Ok(c) => {
                let ctx = ctx.clone();
                if let Ok(mut w) = c.waker.lock() {
                    *w = Some(Box::new(move || ctx.request_repaint()));
                }
                self.control = Some(c);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn focused(&mut self) {
        if let Some(c) = &mut self.control {
            c.focused();
        }
    }

    /// Answers waiting requests; true when something was answered (the window repaints).
    pub fn poll(&mut self, mut host: WindowHost, now: f64) -> bool {
        let Some(control) = &self.control else {
            return false;
        };
        host.models_to_agents(&mut self.state);
        let before = host.studio.session.revision();
        let n = control.poll(&mut Bound {
            host: &mut host,
            state: &mut self.state,
        });
        if n == 0 {
            return false;
        }
        host.models_from_agents(&self.state);
        if host.studio.session.revision() != before {
            host.studio.set_draft(None);
        }
        let log = &self.state.log;
        if log.len() != self.seen_log {
            self.seen_log = log.len();
            if let Some(cmd) = log.back().and_then(|e| e["command"].as_str()) {
                self.last = Some((cmd.to_string(), now));
            }
        } else if self.last.is_none() {
            self.last = Some(("connected".into(), now));
        }
        true
    }
}

/// What the window lends the agents for one poll.
pub struct WindowHost<'a> {
    pub studio: &'a mut Studio,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub mode: &'a mut Option<Mode>,
    pub planting: &'a mut PlantingTools,
    pub model_selected: &'a mut Option<String>,
    /// Ids of the extensions' modes, which `ui.set` opens like the built-in ones.
    pub extension_modes: &'a [(&'static str, &'static str)],
    pub now: f64,
}

/// [`WindowHost`] with the state, as the automation crate's `Host`.
struct Bound<'a, 'b> {
    host: &'a mut WindowHost<'b>,
    state: &'a mut HostState,
}

impl WindowHost<'_> {
    /// Models the person loaded are the agents' too (inspect, render).
    fn models_to_agents(&self, state: &mut HostState) {
        for m in &self.studio.project().models {
            let Some(loaded) = self.studio.loaded.get(&m.id) else {
                continue;
            };
            if state
                .models
                .get(&m.id)
                .is_some_and(|l| Arc::ptr_eq(&l.model, &loaded.model))
            {
                continue;
            }
            let src = self.studio.sources.get(&m.id);
            state.models.insert(
                m.id.clone(),
                LoadedModel {
                    source: m.source.clone(),
                    path: src.and_then(|s| s.path.clone()).unwrap_or_default(),
                    bytes: src.map_or_else(|| Arc::from(Vec::new()), |s| s.bytes.clone()),
                    model: loaded.model.clone(),
                },
            );
        }
    }

    /// Models an agent loaded land in the window.
    fn models_from_agents(&mut self, state: &HostState) {
        for (id, m) in &state.models {
            let same = self
                .studio
                .loaded
                .get(id)
                .is_some_and(|l| Arc::ptr_eq(&l.model, &m.model));
            if !same {
                self.studio.install_model(id, m.model.clone());
                self.studio.sources.insert(
                    id.clone(),
                    crate::studio::Source {
                        bytes: m.bytes.clone(),
                        path: Some(m.path.clone()),
                    },
                );
            }
        }
    }

    fn mode_name(&self) -> Value {
        match *self.mode {
            None => Value::Null,
            Some(m) => json!(mode_id(m)),
        }
    }
}

fn mode_id(m: Mode) -> &'static str {
    match m {
        Mode::Draw => "draw",
        Mode::Materials => "materials",
        Mode::Objects => "objects",
        Mode::Sun => "sun",
        Mode::Views => "viewpoints",
        Mode::Extension(id) => id,
    }
}

fn mode_of(s: &str, extensions: &[(&'static str, &'static str)]) -> Option<Mode> {
    let extensions = extensions.iter().map(|(id, _)| Mode::Extension(id));
    crate::ui::MODES
        .into_iter()
        .chain(extensions)
        .find(|m| mode_id(*m) == s)
}

fn vec3(v: &Value) -> Option<glam::Vec3> {
    let a = v.as_array()?;
    Some(glam::Vec3::new(
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    ))
}

const UI_METHODS: &[Method] = &[
    Method {
        name: "ui.get",
        params: "{}",
        summary: "What the window shows: mode, camera (view, position, target, fov_y_deg), the selected planting and model, lights on, size.",
    },
    Method {
        name: "ui.set",
        params: r#"{"mode": "draw"|"materials"|"objects"|"sun"|"viewpoints"|extension id|null, "camera": {"position": [x, y, z], "target": [x, y, z], "fov_y_deg": number}, "lightsOn": boolean, "overlaysInPictures": boolean, "textures": "auto"|"1k"|"2k"|"4k"}"#,
        summary: "Switches the mode, places the camera (orbiting `target`), turns the lights on or off, and whether pictures show the editor's overlays.",
    },
    Method {
        name: "ui.fly",
        params: r#"{"viewpoint": string}"#,
        summary: "Flies the window's camera to a saved viewpoint.",
    },
    Method {
        name: "ui.select",
        params: r#"{"planting": string|null, "model": string|null}"#,
        summary: "Selects a planting (opening Objects mode) or a model in the window; null deselects.",
    },
    Method {
        name: "ui.screenshot",
        params: r#"{"width": integer = window, "height": integer = window}"#,
        summary: "A lossless truecolour PNG (base64) of the 3D view as the window's camera shows it, without the panels; by default at the canvas' physical resolution (what the Screenshot button saves).",
    },
];

impl Host for Bound<'_, '_> {
    fn parts(&mut self) -> (&mut Session, &mut HostState) {
        (&mut self.host.studio.session, self.state)
    }

    fn render(
        &mut self,
        project: &Project,
        models: &BTreeMap<String, LoadedModel>,
        request: &RenderRequest,
    ) -> Result<Rendered, String> {
        let h = &mut *self.host;
        // The window draws its own session; a picture of another project (a time of day in
        // the request) goes through a draft-free copy and back.
        let saved = (h.studio.project() != project)
            .then(|| std::mem::replace(&mut h.studio.session, Session::new(project.clone())));
        for (id, m) in models {
            if !h.studio.loaded.contains_key(id) {
                h.studio.install_model(id, m.model.clone());
            }
        }
        let camera = h.studio.camera.clone();
        let (quality, auto) = (h.studio.quality, h.studio.quality_auto);
        if let Some((p, t, fov)) = request.camera {
            h.studio.camera = Camera::default();
            h.studio.camera.fov_y = fov.to_radians();
            h.studio
                .camera
                .look_from(glam::Vec3::from(p), glam::Vec3::from(t));
        } else {
            h.studio.frame_all();
        }
        h.studio.quality_auto = false;
        h.studio.quality = visioncraft_render::Quality::PRESETS
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(&request.quality))
            .unwrap_or(2);
        let png = h
            .studio
            .render_png(h.device, h.queue, request.width, request.height);
        let (eye, target, fov) = (
            h.studio.camera.eye(),
            h.studio.camera.pivot,
            h.studio.camera.fov_y.to_degrees(),
        );
        h.studio.camera = camera;
        (h.studio.quality, h.studio.quality_auto) = (quality, auto);
        if let Some(own) = saved {
            // The window's own session, with its history, comes back; the scene follows it.
            h.studio.session = own;
            h.studio.force_sync();
        }
        Ok(Rendered {
            png: png?,
            info: json!({
                "renderer": "window",
                "camera": {"position": eye.to_array(), "target": target.to_array(), "fov_y_deg": fov},
                "quality": request.quality,
                "lightsDropped": h.studio.dropped_lights,
            }),
        })
    }

    fn methods(&self) -> &[Method] {
        UI_METHODS
    }

    fn call(&mut self, method: &str, params: &Value) -> Option<Result<Value, String>> {
        let h = &mut *self.host;
        Some(match method {
            "ui.get" => Ok(json!({
                "mode": h.mode_name(),
                "camera": {
                    "view": match h.studio.camera.view { View::Orbit => "orbit", View::Walk => "walk", View::Top => "plan" },
                    "position": h.studio.camera.eye().to_array(),
                    "target": h.studio.camera.pivot.to_array(),
                    "fov_y_deg": h.studio.camera.fov_y.to_degrees(),
                },
                "selected": {"planting": h.planting.selected, "model": h.model_selected},
                "lightsOn": h.studio.lights_on,
                "overlaysInPictures": h.studio.overlays_in_pictures,
                "textures": h.studio.textures.unwrap_or("auto"),
                "textureTier": h.studio.scene.library.texture_tier,
                "size": h.studio.size,
            })),
            "ui.set" => (|| {
                if let Some(m) = params.get("mode") {
                    *h.mode = match m {
                        Value::Null => None,
                        v => Some(
                            v.as_str()
                                .and_then(|s| mode_of(s, h.extension_modes))
                                .ok_or_else(|| format!("unknown mode {v}"))?,
                        ),
                    };
                }
                if let Some(c) = params.get("camera") {
                    let p = c
                        .get("position")
                        .and_then(vec3)
                        .ok_or("camera.position: [x, y, z]")?;
                    let t = c
                        .get("target")
                        .and_then(vec3)
                        .ok_or("camera.target: [x, y, z]")?;
                    h.studio.camera.cancel_fly();
                    h.studio.camera.view = View::Orbit;
                    if let Some(f) = c.get("fov_y_deg").and_then(Value::as_f64) {
                        h.studio.camera.fov_y = (f as f32).clamp(5.0, 120.0).to_radians();
                    }
                    h.studio.camera.look_from(p, t);
                }
                if let Some(on) = params.get("lightsOn").and_then(Value::as_bool) {
                    h.studio.lights_on = on;
                }
                if let Some(on) = params.get("overlaysInPictures").and_then(Value::as_bool) {
                    h.studio.overlays_in_pictures = on;
                }
                if let Some(t) = params.get("textures") {
                    h.studio.textures = match t.as_str() {
                        Some("auto") => None,
                        Some("1k") => Some("1k"),
                        Some("2k") => Some("2k"),
                        Some("4k") => Some("4k"),
                        _ => return Err(format!("textures: auto, 1k, 2k or 4k, not {t}")),
                    };
                }
                Ok(Value::Null)
            })(),
            "ui.fly" => (|| {
                let id = params["viewpoint"].as_str().ok_or("`viewpoint`: an id")?;
                let v = h
                    .studio
                    .project()
                    .viewpoints
                    .iter()
                    .find(|v| v.id == id)
                    .cloned()
                    .ok_or_else(|| format!("no viewpoint `{id}`"))?;
                h.studio.camera.start_fly(&v, h.now);
                Ok(Value::Null)
            })(),
            "ui.select" => (|| {
                if let Some(p) = params.get("planting") {
                    let id = p.as_str();
                    let g = id
                        .map(|id| {
                            h.studio
                                .project()
                                .generators
                                .iter()
                                .find(|g| g.id() == id)
                                .cloned()
                                .ok_or_else(|| format!("no planting `{id}`"))
                        })
                        .transpose()?;
                    let rev = h.studio.session.revision();
                    h.planting.select_stored(g.as_ref(), rev);
                    if g.is_some() {
                        *h.mode = Some(Mode::Objects);
                    }
                }
                if let Some(m) = params.get("model") {
                    let id = m.as_str();
                    if let Some(id) = id
                        && !h.studio.project().models.iter().any(|m| m.id == id)
                    {
                        return Err(format!("no model `{id}`"));
                    }
                    *h.model_selected = id.map(str::to_string);
                }
                Ok(Value::Null)
            })(),
            "ui.screenshot" => (|| {
                let [w0, h0] = h.studio.size;
                let dim = |k: &str, d: u32| {
                    params
                        .get(k)
                        .and_then(Value::as_u64)
                        .map_or(d, |v| v as u32)
                };
                let (w, hh) = (dim("width", w0.max(1)), dim("height", h0.max(1)));
                let png = h.studio.render_png(h.device, h.queue, w, hh)?;
                Ok(json!({"width": w, "height": hh, "png_base64": base64(&png)}))
            })(),
            _ => return None,
        })
    }
}

/// The same host without a window, for checking it where no display is available: a headless
/// device and `Studio`, answering agents for `seconds` (`visioncraft --agents-headless PORT
/// TOKEN_FILE SECONDS --assets DIR`).
pub fn serve_headless(port: u16, token_file: &str, seconds: f64) -> Result<(), String> {
    let (device, queue) = pollster::block_on(visioncraft_render::headless_device())?;
    let renderer =
        visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut studio = Studio::new(renderer, Project::default());
    studio.resize(1280, 720);
    let mut agents = Agents::default();
    let ctx = eframe::egui::Context::default();
    agents.allow_with(true, &ctx, Some(port), Some(token_file.into()));
    if let Some(e) = &agents.error {
        return Err(e.clone());
    }
    eprintln!("agents: listening on 127.0.0.1:{port}");
    let (mut mode, mut planting, mut model) = (None, PlantingTools::default(), None);
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        let now = start.elapsed().as_secs_f64();
        let host = WindowHost {
            studio: &mut studio,
            device: &device,
            queue: &queue,
            mode: &mut mode,
            planting: &mut planting,
            model_selected: &mut model,
            extension_modes: &[],
            now,
        };
        if agents.poll(host, now)
            && let Some((cmd, _)) = &agents.last
        {
            eprintln!("agents: {cmd} (revision {})", studio.session.revision());
        }
        for e in studio.reconcile_lamps() {
            eprintln!("problem: {e}");
        }
        studio.prepare();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    Ok(())
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() {
                A[(n >> shift) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn ui_set_knows_extension_modes_by_id() {
        use super::{Mode, mode_id, mode_of};
        let ext = [("example.pipe-length", "Pipe")];
        let m = mode_of("example.pipe-length", &ext);
        assert_eq!(m, Some(Mode::Extension("example.pipe-length")));
        assert_eq!(mode_id(m.unwrap()), "example.pipe-length");
        assert_eq!(mode_of("draw", &ext), Some(Mode::Draw));
        assert_eq!(mode_of("example.other", &ext), None);
    }
}
