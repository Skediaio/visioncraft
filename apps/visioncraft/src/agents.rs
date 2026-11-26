//! AI agents driving the open window: on the desktop the automation crate's control channel
//! (`visioncraft-cli mcp --bridge` finds it), answered every frame on the UI thread through
//! [`WindowHost`] over the window's own `Session`, so agent commands appear live and are undone
//! with Ctrl+Z like the person's own. As in 3dcraft, nothing listens until the person turns on
//! *Allow AI agents* (or starts the window with `--control`); turning it off disconnects.
//!
//! In the browser the same protocol answers scripts in the page itself (`web_rpc`):
//! `await vcRpc("engine.execute", {command: "time.set", params: {hour: 20}})` (index.html).
//! The browser's own AI agent (WebMCP, `webmcp.rs`) uses that same path: its tools call `vcRpc`,
//! so they run on the window's `Session`, undo with Ctrl+Z and show as "agent: <command>".
//! The person's *Experimental: browser AI agent (WebMCP)* box is that agent's consent: the
//! page's scripts were never gated (they are the page), so the box gates only the registration
//! of the tools with the browser, and unticking it takes them away.

use crate::camera::View;
use crate::studio::Studio;
use crate::ui::Mode;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use visioncraft_automation::control::{ControlHandle, ControlOptions, serve_control};
use visioncraft_automation::rpc::Method;
use visioncraft_automation::{Access, Host, HostState, LoadedModel, RenderRequest, Rendered};
use visioncraft_core::{Project, Session};

/// The window's control channel and what it shows about it.
pub struct Agents {
    #[cfg(not(target_arch = "wasm32"))]
    control: Option<ControlHandle>,
    pub state: HostState,
    /// The last command an agent ran, and when (seconds of the window's clock).
    pub last: Option<(String, f64)>,
    pub error: Option<String>,
    seen_log: usize,
    /// The browser's own agent (WebMCP, experimental, off until the person switches it on).
    #[cfg(target_arch = "wasm32")]
    pub webmcp: crate::webmcp::WebMcp,
}

impl Default for Agents {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            control: None,
            // The person's own files: an agent may read and write where the person can. In the
            // browser the window imports an example's model in its worker (`Studio::sync`).
            state: HostState {
                host_imports_examples: cfg!(target_arch = "wasm32"),
                ..HostState::new(Access::any())
            },
            last: None,
            error: None,
            seen_log: 0,
            #[cfg(target_arch = "wasm32")]
            webmcp: crate::webmcp::WebMcp::new(false),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
        host.images_to_agents(&mut self.state);
        let before = host.studio.session.revision();
        let n = control.poll(&mut Bound {
            host: &mut host,
            state: &mut self.state,
        });
        if n == 0 {
            return false;
        }
        self.answered(host, before, now)
    }
}

#[cfg(target_arch = "wasm32")]
impl Agents {
    /// Answers the page's waiting requests (`web_rpc`); true when something was answered.
    pub fn poll(&mut self, mut host: WindowHost, now: f64) -> bool {
        let lines = web_rpc::take();
        if lines.is_empty() {
            return false;
        }
        host.models_to_agents(&mut self.state);
        host.images_to_agents(&mut self.state);
        let before = host.studio.session.revision();
        for line in lines {
            let mut bound = Bound {
                host: &mut host,
                state: &mut self.state,
            };
            let reply = visioncraft_automation::rpc::handle_line(&mut bound, &line);
            web_rpc::reply(&reply.unwrap_or_else(|| "null".into()));
        }
        self.answered(host, before, now)
    }
}

impl Agents {
    fn answered(&mut self, mut host: WindowHost, before: u64, now: f64) -> bool {
        host.models_from_agents(&self.state);
        host.images_from_agents(&self.state);
        if host.studio.session.revision() != before {
            host.studio.set_draft(None);
        }
        let log = &self.state.log;
        if log.len() != self.seen_log {
            // An agent opened another project: the load timeline starts again.
            for e in log.iter().skip(self.seen_log) {
                let cmd = e["command"].as_str().unwrap_or_default();
                if matches!(cmd, "project.load_example" | "file.open" | "file.new") {
                    let label = e["label"].as_str().unwrap_or(cmd);
                    crate::load_stats::reset(&format!("agent: {label}"));
                }
            }
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
    /// What is selected, and what an agent selects (`selection`).
    pub selection: &'a mut crate::selection::SelectionState,
    /// Ids of the extensions' modes, which `ui.set` opens like the built-in ones.
    pub extension_modes: &'a [(&'static str, &'static str)],
    /// Which parts of the built-in UI are drawn (`ui.set` / `ui.get`).
    pub host_ui: &'a mut crate::host_ui::HostUi,
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

    /// Images the person added (plans, wall photos) are the agents' too (`document.inspect` says
    /// they are loaded).
    fn images_to_agents(&self, state: &mut HostState) {
        let store = &self.studio.scene.library.store;
        for file in self.studio.project().image_files() {
            if state.images.contains_key(file) {
                continue;
            }
            if let Some(bytes) = store.provided(file) {
                state.image_missing.remove(file);
                state.images.insert(file.to_string(), bytes);
            }
        }
    }

    /// Images an agent read land in the window's asset store.
    fn images_from_agents(&mut self, state: &HostState) {
        let store = &mut self.studio.scene.library.store;
        for (file, bytes) in &state.images {
            if store.provided(file).is_none() {
                store.provide(file, bytes.clone());
            }
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

pub(crate) fn mode_of(s: &str, extensions: &[(&'static str, &'static str)]) -> Option<Mode> {
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
        summary: "What the window shows: mode, ui (tabs, file_bar, view_bar, mode_panel_frame: which built-in bars are drawn), camera (view, position, target, fov_y_deg), the selection, lights on, size, and what is still on its way (pending: plants to generate, library downloads, the sky volume). `selection` is what the person has picked, in any mode: {items: [{kind: shape|placement|planting|model|lamp|underlay, id, name, summary (one line: what it is, size, material or species, position), sub?: {edge?, vertex?, index?}}], point?: [x, y, z] (the last ground or surface point the person clicked, metres), revision (bumps on every change)}. A tree picked in a planting is kind `placement`: id is its planting, sub.index its plant number (for planting.replace_placement and planting.remove_placement). `selected` ({planting, model}) is the older form of the same. Poll it and compare `revision` to notice a new selection.",
    },
    Method {
        name: "ui.load",
        params: r#"{"reset": boolean}"#,
        summary: "The load timeline (docs/performance.md): `startup` (the program's start: renderer and pipelines, calibration, first frame on screen, ms since the program started), `load` (since the last project or example was opened: `milestones` such as first frame, first frame on screen, plants done, library idle, everything loaded; `stages` by total time with count and longest step, `breakdown` rows detail others; `frames` drawn, over 25 ms, over 50 ms and the longest; `hitches`: the last 64 frames over 25 ms, each {at_s (seconds since the program started), ms, stages: {main-thread stage: ms, for stages of 0.5 ms or more that frame}}; `reset: true` returns this reply and then clears the hitches and the frame counters so a measurement starts clean; `timeline`: the longer steps with start and duration, ms since the load began), `pending` (what is still on its way), `textures` (the store's cap and held bytes) and `lods`: per placed catalogue asset, nearest first, the level of detail its nearest instance needs, the levels resident, the level shown and why.",
    },
    Method {
        name: "ui.set",
        params: r#"{"mode": "draw"|"materials"|"objects"|"sun"|"viewpoints"|extension id|null, "camera": {"position": [x, y, z], "target": [x, y, z], "fov_y_deg": number}, "lightsOn": boolean, "overlaysInPictures": boolean, "tabs": boolean, "file_bar": boolean, "view_bar": boolean, "mode_panel_frame": boolean, "textures": "auto"|"1k"|"2k"|"4k", "quality": "auto"|"low"|"medium"|"high"|"ultra", "walk": boolean}"#,
        summary: "Switches the mode, places the camera (orbiting `target`), walks (as the Walk button: on foot near the camera's target; the arrow keys or WASD move) or orbits again, turns the lights on or off (the project's view.set lights), whether pictures show the editor's overlays, the texture resolution and the rendering quality (the Quality menu). `tabs`, `file_bar`, `view_bar` and `mode_panel_frame` show or hide the built-in mode tabs, the Open/Save/Undo/Redo bar, the view bar (Layers, Lights, Quality, Export, Walk, fps) and the frame and fold button of the mode panel (an embedding app hides what it replaces; `ui.get` reports them under `ui`). Hidden controls stay reachable as commands and shortcuts (Ctrl+Z, `ui.set mode`).",
    },
    Method {
        name: "ui.fly",
        params: r#"{"viewpoint": string}"#,
        summary: "Flies the window's camera to a saved viewpoint and, as a click on it, sets its time of day and years.",
    },
    Method {
        name: "ui.select",
        params: r#"{"items": [{"kind": "shape"|"placement"|"planting"|"model"|"lamp"|"underlay", "id": string, "sub": {"edge": integer, "vertex": integer, "index": integer}}]|null, "planting": string|null, "model": string|null, "point": [x, y, z]|null}"#,
        summary: "Selects things in the window as a click would: opens the mode of the first (shape and underlay: Draw with the handles; planting, placement and lamp: Objects; model: the plain view); the others are selected with it, as Shift+click does. A plant of a planting is `placement` (id: the planting, sub.index: its number). `items: null` or [] clears; an unknown id is an error. `planting` and `model` are the older form (one thing). `point` sets the picked point alone. The selection is not part of the project and not an undo step.",
    },
    Method {
        name: "ui.edit_selection",
        params: r#"{"action": "delete"|"move"|"material", "offset": [x, z]|[x, y, z], "material": string}"#,
        summary: "Edits everything selected (`ui.get` -> `selection.items`) at once, as one undo step: `delete` removes it all; `move` shifts it by `offset` metres (+X east, -Z north; a shape, a planting and a plan image only move on the ground, and a brush or a surface fill has no place to move; a plant of a planting moves as `planting.replace_placement` offset; a plant whose whole planting is selected goes with it); `material` assigns a material id to every shape and model. What an action does not apply to is left alone and listed under `skipped`; an error undoes the whole edit. Answers {changed, skipped: [{kind, id, reason}]}.",
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

    fn extra_assets(&self) -> Vec<String> {
        let s = &self.host.studio;
        let library = s.scene.library.catalog.iter().flat_map(|c| c.assets.iter());
        library
            .chain(&s.assets)
            .map(|a| a.id.clone())
            .chain(crate::fixtures::assets().into_iter().map(|a| a.id))
            .collect()
    }

    #[cfg(target_arch = "wasm32")]
    fn render(
        &mut self,
        _: &Project,
        _: &BTreeMap<String, LoadedModel>,
        _: &RenderRequest,
    ) -> Result<Rendered, String> {
        Err(NO_READBACK.into())
    }

    #[cfg(not(target_arch = "wasm32"))]
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
        if saved.is_some() {
            h.studio.growth_conflicts = Default::default();
        }
        for (id, m) in models {
            if !h.studio.loaded.contains_key(id) {
                h.studio.install_model(id, m.model.clone());
            }
        }
        let camera = h.studio.camera.clone();
        let (quality, auto) = (h.studio.quality, h.studio.quality_auto);
        let shadow_filter = h.studio.shadow_filter;
        if let Some((p, t, fov)) = request.camera {
            h.studio.camera = crate::camera::Camera::default();
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
            .unwrap_or(if request.photo.is_some() { 3 } else { 2 });
        h.studio
            .set_shadow_filter(request.shadow_filter, request.shadow_radius_m);
        let before = h.studio.problems.len();
        let png = match &request.photo {
            Some(p) => {
                let (w, hh) = (request.width, request.height);
                let quality = h.studio.current_quality();
                h.studio
                    .render_photo(w, hh, quality, p)
                    .and_then(|px| crate::studio::png_rgb(&px, w, hh))
            }
            None => h
                .studio
                .render_png(h.device, h.queue, request.width, request.height),
        };
        let (eye, target, fov) = (
            h.studio.camera.eye(),
            h.studio.camera.pivot,
            h.studio.camera.fov_y.to_degrees(),
        );
        h.studio.camera = camera;
        (h.studio.quality, h.studio.quality_auto) = (quality, auto);
        h.studio.shadow_filter = shadow_filter;
        if let Some(own) = saved {
            // The window's own session, with its history, comes back; the scene follows it.
            h.studio.session = own;
            h.studio.growth_conflicts = Default::default();
            h.studio.force_sync();
        }
        Ok(Rendered {
            png: png?,
            info: json!({
                "renderer": "window",
                "problems": h.studio.problems.get(before..).unwrap_or_default(),
                "camera": {"position": eye.to_array(), "target": target.to_array(), "fov_y_deg": fov},
                "quality": request.quality,
                "lightsDropped": h.studio.dropped_lights,
                "samples": request.photo.as_ref().map(|p| p.samples),
                "focus_m": request
                    .photo
                    .as_ref()
                    .filter(|p| p.f_number.is_some() || p.aperture_m > 0.0)
                    .and(h.studio.photo_focus_m),
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
                "ui": h.host_ui.to_json(),
                "camera": {
                    "view": match h.studio.camera.view { View::Orbit => "orbit", View::Walk => "walk", View::Top => "plan" },
                    "position": h.studio.camera.eye().to_array(),
                    "target": h.studio.camera.pivot.to_array(),
                    "fov_y_deg": h.studio.camera.fov_y.to_degrees(),
                },
                "selection": h.selection.to_json(h.studio.project(), &h.studio.assets),
                "selected": h.selection.legacy(),
                "lightsOn": h.studio.lights_on(),
                "overlaysInPictures": h.studio.overlays_in_pictures,
                "textures": h.studio.textures.unwrap_or("auto"),
                "textureTier": h.studio.scene.library.texture_tier,
                "size": h.studio.size,
                "quality": {
                    "auto": h.studio.quality_auto,
                    "preset": crate::studio::preset_name(&h.studio.current_quality()),
                    "renderScale": h.studio.current_quality().render_scale,
                },
                "pending": {
                    "plants": h.studio.plants_pending(),
                    "models": h.studio.models_pending(),
                    "library": !h.studio.scene.library.idle(),
                    "skyVolume": h.studio.sky_volume_pending(),
                    "skyVisibility": h.studio.sky_visibility_pending(),
                    "sunMap": h.studio.sun_map_pending(),
                },
            })),
            "ui.load" => {
                let s = &h.studio;
                let lib = &s.scene.library;
                let mut out = crate::load_stats::json();
                out["pending"] = json!({
                    "plants": s.plants_pending(),
                    "models": s.models_pending(),
                    "library": !lib.idle(),
                    "skyVolume": s.sky_volume_pending(),
                    "skyVisibility": s.sky_visibility_pending(),
                });
                out["textures"] = json!({
                    "tier": lib.texture_tier,
                    "cap_mb": lib.store.cap_bytes >> 20,
                    "content_mb": lib.store.texture_bytes >> 20,
                    "over_cap": lib.store.over_cap,
                    "library_mb": lib.store.held_bytes(|_| true) >> 20,
                });
                out["lods"] = lib.lod_report();
                if params.get("reset").and_then(Value::as_bool) == Some(true) {
                    crate::load_stats::reset_hitches();
                }
                Ok(out)
            }
            "ui.set" => (|| {
                h.host_ui.apply(params)?;
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
                    // The project's view setting (view.set lights), so pictures agree.
                    h.studio
                        .run("view.set", json!({"lights": on}))
                        .map_err(|e| e.to_string())?;
                }
                if let Some(on) = params.get("walk").and_then(Value::as_bool) {
                    let walking = h.studio.camera.view == View::Walk;
                    if on && !walking {
                        h.studio.camera.view = View::Orbit;
                        let target = h.studio.camera.pivot;
                        crate::enter_walk_facing(h.studio, target);
                    } else if !on && walking {
                        h.studio.camera.leave_walk();
                    }
                }
                if let Some(on) = params.get("overlaysInPictures").and_then(Value::as_bool) {
                    h.studio.overlays_in_pictures = on;
                }
                if let Some(q) = params.get("quality") {
                    let name = q.as_str().unwrap_or_default();
                    if name == "auto" {
                        h.studio.quality_auto = true;
                    } else {
                        h.studio.quality = visioncraft_render::Quality::PRESETS
                            .iter()
                            .position(|(n, _)| n.eq_ignore_ascii_case(name))
                            .ok_or_else(|| {
                                format!("quality: auto, low, medium, high or ultra, not {q}")
                            })?;
                        h.studio.quality_auto = false;
                    }
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
                // As a click on the viewpoint (views.rs): its time of day and years come along.
                if let Some(t) = v.time
                    && t != h.studio.project().time
                {
                    let p = json!({"year": t.year, "month": t.month, "day": t.day, "hour": t.hour});
                    h.studio.run("time.set", p).map_err(|e| e.to_string())?;
                }
                if let Some(years) = v.years
                    && Some(years) != h.studio.project().view.years
                {
                    let p = json!({"years": years});
                    h.studio.run("view.set", p).map_err(|e| e.to_string())?;
                }
                Ok(Value::Null)
            })(),
            "ui.select" => (|| {
                let items = ["items", "planting", "model"]
                    .iter()
                    .any(|k| params.get(*k).is_some());
                if items || params.get("point").is_none() {
                    let keys = crate::selection::parse(h.studio.project(), params)?;
                    // The mode a click on it would be in; the window opens it after this call.
                    if let Some(k) = keys.first() {
                        *h.mode = k.mode();
                    }
                    h.selection.set(keys);
                }
                if let Some(p) = params.get("point") {
                    h.selection.set_point(vec3(p).map(|p| p.to_array()));
                }
                Ok(Value::Null)
            })(),
            "ui.edit_selection" => (|| {
                let edit = crate::selection_edit::Edit::parse(params)?;
                let items = h.selection.items.clone();
                let done = crate::selection_edit::apply(h.studio, &items, &edit);
                h.selection.prune(h.studio.project());
                done
            })(),
            #[cfg(target_arch = "wasm32")]
            "ui.screenshot" => Err(NO_READBACK.into()),
            #[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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
    let (mut mode, mut selection) = (None, crate::selection::SelectionState::default());
    let mut host_ui = crate::host_ui::HostUi::default();
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        let now = start.elapsed().as_secs_f64();
        let host = WindowHost {
            studio: &mut studio,
            device: &device,
            queue: &queue,
            mode: &mut mode,
            selection: &mut selection,
            extension_modes: &[],
            host_ui: &mut host_ui,
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

/// WebGPU reads pictures back asynchronously; the bridge answers within one frame.
#[cfg(target_arch = "wasm32")]
const NO_READBACK: &str =
    "pictures cannot be read back in the browser; take them from the page (the canvas)";

/// The page's request queue: `vcRpc` (index.html) pushes JSON-lines requests onto
/// `__vcRpcIn` and gets each reply through `__vcRpcReply`.
#[cfg(target_arch = "wasm32")]
pub mod web_rpc {
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};

    /// The waiting request lines, emptying the queue.
    pub fn take() -> Vec<String> {
        let Ok(queue) = js_sys::Reflect::get(&js_sys::global(), &"__vcRpcIn".into()) else {
            return Vec::new();
        };
        let Some(queue) = queue.dyn_ref::<js_sys::Array>() else {
            return Vec::new();
        };
        let lines = queue.iter().filter_map(|v| v.as_string()).collect();
        queue.set_length(0);
        lines
    }

    pub fn reply(line: &str) {
        if let Ok(f) = js_sys::Reflect::get(&js_sys::global(), &"__vcRpcReply".into())
            && let Some(f) = f.dyn_ref::<js_sys::Function>()
        {
            let _ = f.call1(&JsValue::NULL, &line.into());
        }
    }

    /// `__vcRpcWake()`: the page asks for a frame, so a request is answered while nothing moves.
    pub fn install_waker(ctx: &eframe::egui::Context) {
        let ctx = ctx.clone();
        let wake = Closure::<dyn Fn()>::new(move || ctx.request_repaint());
        let _ = js_sys::Reflect::set(
            &js_sys::global(),
            &"__vcRpcWake".into(),
            wake.as_ref().unchecked_ref(),
        );
        wake.forget();
    }
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
    use serde_json::{Value, json};
    use visioncraft_core::{Project, Session};

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

    /// A window host over a headless device, or `None` where there is no adapter.
    fn with_host(project: Project, f: impl FnOnce(&mut super::Bound<'_, '_>)) {
        let Ok((device, queue)) = pollster::block_on(visioncraft_render::headless_device()) else {
            return;
        };
        let renderer =
            visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut studio = super::Studio::new(renderer, project);
        let (mut mode, mut selection) = (None, crate::selection::SelectionState::default());
        let mut host_ui = crate::host_ui::HostUi::default();
        let mut host = super::WindowHost {
            studio: &mut studio,
            device: &device,
            queue: &queue,
            mode: &mut mode,
            selection: &mut selection,
            extension_modes: &[],
            host_ui: &mut host_ui,
            now: 0.0,
        };
        let mut state = super::HostState::new(visioncraft_automation::Access::any());
        f(&mut super::Bound {
            host: &mut host,
            state: &mut state,
        });
    }

    fn garden() -> Project {
        let mut s = Session::new(Project::default());
        for (c, p) in [
            (
                "terrace.add",
                json!({"name": "Patio", "outline": [[0, 0], [4, 0], [4, 3], [0, 3]], "floor": -0.3}),
            ),
            (
                "planting.add_cluster",
                json!({"name": "Grove", "center": [8, 8], "radius_m": 2, "count": 4,
                    "asset_palette": [{"asset": "flora:betula-pendula", "weight": 1}]}),
            ),
            (
                "light.add",
                json!({"name": "Lamp", "position": [1, 2, 1], "kind": {"type": "point"}}),
            ),
        ] {
            s.run(c, p).unwrap();
        }
        s.project().clone()
    }

    #[test]
    fn ui_set_and_get_carry_the_host_ui_flags() {
        use super::Host;
        with_host(Project::default(), |h| {
            let got = h.call("ui.get", &json!({})).unwrap().unwrap();
            assert_eq!(got["ui"]["tabs"], true);
            h.call("ui.set", &json!({"tabs": false, "view_bar": false}))
                .unwrap()
                .unwrap();
            assert!(!h.host.host_ui.tabs && !h.host.host_ui.view_bar && h.host.host_ui.file_bar);
            let got = h.call("ui.get", &json!({})).unwrap().unwrap();
            assert_eq!(got["ui"]["tabs"], false);
            assert!(h.call("ui.set", &json!({"file_bar": 3})).unwrap().is_err());
        });
    }

    #[test]
    fn import_object_returns_a_reference_the_project_knows() {
        with_host(Project::default(), |h| {
            let id = h
                .host
                .studio
                .import_object("Shed", crate::user_objects::tests::triangle_glb())
                .unwrap();
            assert!(id.starts_with("user:"));
            assert_eq!(h.host.studio.project().objects.len(), 1);
            assert!(
                h.host
                    .studio
                    .import_object("Bad", b"nope".to_vec())
                    .is_err()
            );
        });
    }

    #[test]
    fn what_ui_select_selects_ui_get_reads_back() {
        use super::Host;
        with_host(garden(), |h| {
            let lamp = h.host.studio.project().lights[0].id.clone();
            let cases = [
                (
                    json!({"kind": "shape", "id": "S1", "sub": {"vertex": 1}}),
                    "terrace, floor at -0.3 m",
                    Some("draw"),
                ),
                (
                    json!({"kind": "planting", "id": "g1"}),
                    "cluster",
                    Some("objects"),
                ),
                (
                    json!({"kind": "placement", "id": "g1", "sub": {"index": 2}}),
                    "plant number 2 of 4",
                    Some("objects"),
                ),
                (
                    json!({"kind": "lamp", "id": lamp}),
                    "point light",
                    Some("objects"),
                ),
            ];
            let mut last = 0;
            for (item, summary, mode) in cases {
                let r = h.call("ui.select", &json!({"items": [item.clone()]}));
                assert_eq!(r, Some(Ok(Value::Null)), "{item}");
                let got = h.call("ui.get", &json!({})).unwrap().unwrap();
                let sel = &got["selection"];
                assert_eq!(sel["items"].as_array().unwrap().len(), 1, "{sel}");
                let it = &sel["items"][0];
                assert_eq!(it["kind"], item["kind"]);
                assert_eq!(it["id"], item["id"]);
                assert_eq!(it.get("sub"), item.get("sub"));
                assert!(it["summary"].as_str().unwrap().contains(summary), "{it}");
                assert!(!it["name"].as_str().unwrap().is_empty());
                assert_eq!(got["mode"], json!(mode));
                let rev = sel["revision"].as_u64().unwrap();
                assert!(rev > last, "the revision bumps: {rev} after {last}");
                last = rev;
            }
            // The older fields follow.
            let r = h.call("ui.select", &json!({"planting": "g1"}));
            assert_eq!(r, Some(Ok(Value::Null)));
            let got = h.call("ui.get", &json!({})).unwrap().unwrap();
            assert_eq!(got["selected"]["planting"], "g1");
            assert_eq!(got["selection"]["items"][0]["kind"], "planting");
            // null and [] clear.
            for clear in [json!({"items": null}), json!({"items": []})] {
                h.call("ui.select", &json!({"planting": "g1"}));
                h.call("ui.select", &clear);
                let got = h.call("ui.get", &json!({})).unwrap().unwrap();
                assert_eq!(got["selection"]["items"], json!([]));
                assert_eq!(got["selected"]["planting"], Value::Null);
            }
        });
    }

    #[test]
    fn ui_select_refuses_unknown_ids_and_keeps_the_selection() {
        use super::Host;
        with_host(garden(), |h| {
            h.call(
                "ui.select",
                &json!({"items": [{"kind": "planting", "id": "g1"}]}),
            );
            let before = h.call("ui.get", &json!({})).unwrap().unwrap()["selection"].clone();
            let e = h
                .call(
                    "ui.select",
                    &json!({"items": [{"kind": "shape", "id": "S9"}]}),
                )
                .unwrap()
                .unwrap_err();
            assert!(e.contains("no shape `S9`"), "{e}");
            assert_eq!(
                h.call("ui.get", &json!({})).unwrap().unwrap()["selection"],
                before
            );
        });
    }

    #[test]
    fn the_picked_point_is_set_beside_the_items() {
        use super::Host;
        with_host(garden(), |h| {
            h.call(
                "ui.select",
                &json!({"items": [{"kind": "planting", "id": "g1"}]}),
            );
            h.call("ui.select", &json!({"point": [3.5, 0.0, -2.0]}));
            let sel = h.call("ui.get", &json!({})).unwrap().unwrap()["selection"].clone();
            assert_eq!(sel["point"], json!([3.5, 0.0, -2.0]));
            assert_eq!(sel["items"][0]["id"], "g1", "the items stay");
        });
    }

    #[test]
    fn several_things_are_selected_and_edited_together() {
        use super::Host;
        with_host(garden(), |h| {
            let lamp = h.host.studio.project().lights[0].id.clone();
            let r = h.call(
                "ui.select",
                &json!({"items": [{"kind": "shape", "id": "S1"}, {"kind": "planting", "id": "g1"},
                    {"kind": "lamp", "id": lamp}]}),
            );
            assert!(r.unwrap().is_ok());
            let got = h.call("ui.get", &json!({})).unwrap().unwrap();
            assert_eq!(got["selection"]["items"].as_array().unwrap().len(), 3);
            let moved = h
                .call(
                    "ui.edit_selection",
                    &json!({"action": "move", "offset": [1, 2]}),
                )
                .unwrap()
                .unwrap();
            assert_eq!(moved["changed"], 3);
            let removed = h
                .call("ui.edit_selection", &json!({"action": "delete"}))
                .unwrap()
                .unwrap();
            assert_eq!(removed["changed"], 3);
            let p = h.host.studio.project();
            assert!(p.shapes.is_empty() && p.generators.is_empty() && p.lights.is_empty());
            // Gone things leave the selection; one undo brings all three back.
            let got = h.call("ui.get", &json!({})).unwrap().unwrap();
            assert!(got["selection"]["items"].as_array().unwrap().is_empty());
            assert!(h.host.studio.session.undo());
            assert_eq!(h.host.studio.project().shapes.len(), 1);
            let e = h
                .call("ui.edit_selection", &json!({"action": "delete"}))
                .unwrap()
                .unwrap_err();
            assert!(e.contains("nothing is selected"), "{e}");
        });
    }
}
