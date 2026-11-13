//! VisionCraft: a full-bleed 3D viewport under small floating controls. This file is the window:
//! the frame loop, input routing, files and persistence. The scene, the camera and the modes live
//! in their own modules; `studio` is everything that does not need a window.

#[cfg(not(target_arch = "wasm32"))]
pub mod agents;
pub mod assets;
pub mod bundle;
pub mod camera;
#[cfg(test)]
mod command_params;
pub mod draw;
pub mod extension;
pub mod files;
pub mod gltf_load;
pub mod handles;
pub mod import_task;
pub mod lamps;
pub mod library;
pub mod library_ui;
pub mod material_library;
pub mod materials;
pub mod modelui;
pub mod objects;
pub mod placeholder;
pub mod plant_scene;
pub mod planting_panel;
pub mod planting_smoke;
pub mod planting_tools;
pub mod plants;
pub mod scene;
pub mod shell;
pub mod sky_panel;
pub mod smoke;
pub mod species_editor;
pub mod strings;
pub mod studio;
pub mod sun;
pub mod terrain_gpu;
pub mod terrain_tool;
pub mod ui;
pub mod user_objects;
pub mod views;

pub use eframe::egui;
pub use extension::{Extension, ExtensionCx, Overlay};
pub use shell::Shell;

use eframe::egui::{Align2, Key, PointerButton, Vec2};
use files::{Inbox, Msg, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use studio::{Source, Studio};
use ui::{Action, Cx, Hud, Input, Mode, Toasts};
use visioncraft_core::{ImportedModel, ModelKind, Project};
use visioncraft_render::{Quality, Renderer};

const KEY_PROJECT: &str = "project";
const KEY_NAME: &str = "project_name";
const KEY_BOUNDS: &str = "model_bounds";
#[cfg(not(target_arch = "wasm32"))]
const KEY_AGENTS: &str = "allow_agents";

/// A model file being imported off the UI thread (`import_task`).
struct Job {
    name: String,
    bytes: Arc<[u8]>,
    path: Option<PathBuf>,
    target: Option<String>,
    task: Option<import_task::ImportTask>,
}

/// Every viewpoint rendered in turn at one size, then saved together.
struct ExportAll {
    size: (u32, u32),
    todo: Vec<visioncraft_core::Viewpoint>,
    current: Option<(String, visioncraft_render::Still)>,
    done: Vec<(String, Vec<u8>)>,
    count: usize,
}

struct ExportJob {
    still: visioncraft_render::Still,
    idx: usize,
}

/// A project replacement waiting for the user to confirm that unsaved work may go.
enum Replace {
    Open {
        name: String,
        bytes: Vec<u8>,
        path: Option<PathBuf>,
    },
    New,
}

pub struct App {
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    texture: Option<egui::TextureId>,
    studio: Studio,
    hud: Hud,
    toasts: Toasts,
    inbox: Inbox,
    watcher: Watcher,
    draw: draw::DrawState,
    materials: materials::MaterialsState,
    objects: objects::ObjectsState,
    terrain: terrain_tool::TerrainTool,
    views: views::ViewsState,
    modelui: modelui::ModelUi,
    extensions: Vec<Box<dyn Extension>>,
    loading: Option<Job>,
    /// Model imports waiting for `loading` (a bundle brings several).
    queued_jobs: Vec<Job>,
    export: Option<ExportJob>,
    export_all: Option<ExportAll>,
    pending_replace: Option<Replace>,
    /// Models whose file changed, with the time to re-import them (debounce).
    changed: HashMap<String, f64>,
    project_name: Option<String>,
    saved_revision: u64,
    saving_revision: u64,
    stored_revision: Option<u64>,
    last_time: f64,
    fps: f64,
    last_title: String,
    settle: u8,
    /// A project restored at start is still settling (its undo history is cleared until then).
    settling_restore: bool,
    /// The last frame asked for the next one: the time between them is a frame, not idle.
    repainting: bool,
    #[cfg(not(target_arch = "wasm32"))]
    agents: agents::Agents,
    #[cfg(not(target_arch = "wasm32"))]
    focused: bool,
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase()
}

fn stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|e| e.to_str())
        .unwrap_or("Model")
        .to_string()
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, extensions: Vec<Box<dyn Extension>>) -> Self {
        ui::apply_theme(&cc.egui_ctx);
        let state = cc
            .wgpu_render_state
            .as_ref()
            .expect("eframe must run on wgpu");
        // egui samples user textures as plain Rgba8Unorm holding sRGB bytes.
        let renderer = Renderer::new(&state.device, &state.queue, wgpu::TextureFormat::Rgba8Unorm);
        let stored = cc.storage.and_then(|s| s.get_string(KEY_PROJECT));
        let restored = stored.and_then(|j| Project::from_json_bytes(j.as_bytes()).ok());
        let was_restored = restored.is_some();
        let inbox = Inbox::default();
        let mut app = Self {
            device: state.device.clone(),
            queue: state.queue.clone(),
            egui_renderer: state.renderer.clone(),
            texture: None,
            studio: Studio::new(renderer, restored.unwrap_or_default()),
            hud: Hud::default(),
            toasts: Toasts::default(),
            watcher: Watcher::new(inbox.clone(), cc.egui_ctx.clone()),
            inbox,
            draw: Default::default(),
            materials: Default::default(),
            objects: Default::default(),
            terrain: Default::default(),
            views: Default::default(),
            modelui: Default::default(),
            extensions,
            loading: None,
            queued_jobs: Vec::new(),
            export: None,
            export_all: None,
            pending_replace: None,
            changed: HashMap::new(),
            project_name: cc.storage.and_then(|s| s.get_string(KEY_NAME)),
            saved_revision: 0,
            saving_revision: 0,
            stored_revision: None,
            last_time: 0.0,
            fps: 0.0,
            last_title: String::new(),
            settle: 0,
            settling_restore: was_restored,
            repainting: false,
            #[cfg(not(target_arch = "wasm32"))]
            agents: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            focused: false,
        };
        for e in &app.extensions {
            e.register(&mut app.studio.session);
            if let Some(label) = e.mode() {
                app.hud.extension_modes.push((e.id(), label));
            }
        }
        // AI agents: the person's switch, or `--control [PORT]` for this run.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let args: Vec<String> = std::env::args().collect();
            let flag = args.iter().position(|a| a == "--control");
            let port = flag
                .and_then(|i| args.get(i + 1))
                .and_then(|p| p.parse().ok());
            let stored = cc.storage.and_then(|s| s.get_string(KEY_AGENTS)).as_deref() == Some("1");
            if flag.is_some() || stored {
                app.agents.allow(true, &cc.egui_ctx, port);
            }
        }
        if was_restored {
            if let Some(b) = cc
                .storage
                .and_then(|s| s.get_string(KEY_BOUNDS))
                .and_then(|j| serde_json::from_str(&j).ok())
            {
                app.studio.bounds_hint = b;
            }
            app.load_sources(None);
            app.studio.sync();
            app.studio.frame_all();
            app.stored_revision = Some(0);
        }
        app
    }

    /// Reads the files of models from the project's `source` paths (native only: a page cannot).
    fn load_sources(&mut self, project_dir: Option<&Path>) {
        #[cfg(not(target_arch = "wasm32"))]
        for m in self.studio.project().models.clone() {
            let p = PathBuf::from(&m.source);
            let candidates = [Some(p.clone()), project_dir.map(|d| d.join(&p))];
            for path in candidates.into_iter().flatten() {
                if let Ok(bytes) = std::fs::read(&path) {
                    let src = Source {
                        bytes: Arc::from(bytes),
                        path: Some(path.clone()),
                    };
                    if let Ok((w, _)) = self.studio.reimport(&m.id, src) {
                        self.watcher.watch(&m.id, path);
                        w.into_iter().for_each(|w| self.toasts.warn(w));
                        break;
                    }
                }
            }
        }
        // Imported objects next to a plain project file (natively; a page sees no folders).
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = project_dir {
            for o in self.studio.project().objects.clone() {
                if let Ok(bytes) = std::fs::read(dir.join(&o.file)) {
                    self.take_bundled(vec![(o.file.clone(), Arc::from(bytes))]);
                }
            }
        }
        let _ = project_dir;
    }

    fn dirty(&self) -> bool {
        self.studio.session.revision() != self.saved_revision
    }

    // ---- files ----

    fn handle_file(
        &mut self,
        ctx: &egui::Context,
        name: String,
        bytes: Vec<u8>,
        path: Option<PathBuf>,
        target: Option<String>,
    ) {
        match extension(&name).as_str() {
            "visioncraft" | "json" | "zip" if self.dirty() => {
                self.pending_replace = Some(Replace::Open { name, bytes, path });
            }
            "visioncraft" | "json" | "zip" => self.open_project(&name, &bytes, path.as_deref()),
            "3dcraft" | "obj" => {
                self.loading = Some(Job {
                    name,
                    bytes: Arc::from(bytes),
                    path,
                    target,
                    task: None,
                })
            }
            "glb" => match self.studio.import_object(&stem(&name), bytes) {
                Ok(_) => {
                    self.toasts
                        .info(format!("{} {}", stem(&name), strings::OBJECT_IMPORTED));
                }
                Err(e) => self.toasts.error(format!("{name}: {e}")),
            },
            "mcmat" => {
                let mut cx = Cx {
                    s: &mut self.studio,
                    toasts: &mut self.toasts,
                    input: &Input::default(),
                    ctx,
                };
                self.materials.add_mcmat(&mut cx, &bytes);
            }
            _ => self
                .toasts
                .error(format!("{name}: not a model, project or material file")),
        }
    }

    fn new_project(&mut self) {
        self.watcher.forget_all();
        self.studio.replace_project(Project::default());
        self.studio.people.clear();
        self.project_name = None;
        self.saved_revision = self.studio.session.revision();
        self.hud.mode = None;
        self.modelui = Default::default();
    }

    /// Renders the current view at preset `idx` (a still at HIGH quality); falls back to the next smaller preset on failure.
    fn start_export(&mut self, idx: usize) {
        let presets = visioncraft_core::export_presets();
        let (name, w, h) = presets[idx];
        self.studio.prepare();
        let (view, proj, eye) = self.studio.camera.matrices(w as f32 / h as f32);
        match self.studio.renderer.render_still(w, h, view, proj, eye) {
            Ok(still) => {
                self.toasts.info(format!("Rendering {name} {w} × {h}…"));
                self.export = Some(ExportJob { still, idx });
            }
            Err(e) => self.export_retry(idx, &e),
        }
    }

    fn export_retry(&mut self, idx: usize, error: &str) {
        let presets = visioncraft_core::export_presets();
        let pixels = |i: usize| u64::from(presets[i].1) * u64::from(presets[i].2);
        let next = (0..presets.len())
            .filter(|&i| pixels(i) < pixels(idx))
            .max_by_key(|&i| pixels(i));
        match next {
            Some(n) => {
                self.toasts
                    .warn(format!("{error}. Trying {} instead.", presets[n].0));
                self.start_export(n);
            }
            None => self.toasts.error(format!("Export failed: {error}")),
        }
    }

    /// One step of exporting every viewpoint; true while it runs.
    fn poll_export_all(&mut self, ctx: &egui::Context) -> bool {
        let Some(job) = &mut self.export_all else {
            return false;
        };
        if let Some((name, still)) = &mut job.current {
            match still.try_take() {
                None => return true,
                Some(Ok(rgba)) => {
                    let (w, h) = still.size();
                    match studio::png_bytes(&rgba, w, h) {
                        Ok(png) => job.done.push((format!("{name}.png"), png)),
                        Err(e) => self.toasts.error(e),
                    }
                }
                Some(Err(e)) => self.toasts.error(format!("{name}: {e}")),
            }
            job.current = None;
        }
        let Some(v) = job.todo.pop() else {
            let job = self.export_all.take().expect("checked");
            self.studio.time_draft = None;
            self.studio.force_sync();
            let stem = files::file_stem(
                self.project_name.as_deref().unwrap_or("visioncraft"),
                "visioncraft",
            );
            files::save_many(&self.inbox, ctx, format!("{stem}-viewpoints.zip"), job.done);
            return false;
        };
        let (w, h) = job.size;
        // The viewpoint's own time of day, its camera, then a still of it.
        self.studio.time_draft = v.time;
        self.studio.prepare();
        let mut cam = camera::Camera::default();
        cam.fov_y = v.fov_y_deg.to_radians();
        cam.look_from(glam::Vec3::from(v.position), glam::Vec3::from(v.target));
        let (view, proj, eye) = cam.matrices(w as f32 / h as f32);
        let n = job.count - job.todo.len();
        let mut name = format!("{n:02}-{}", files::file_stem(&v.name, "view"));
        while job.done.iter().any(|(f, _)| *f == format!("{name}.png")) {
            name.push('x');
        }
        match self.studio.renderer.render_still(w, h, view, proj, eye) {
            Ok(still) => job.current = Some((name, still)),
            Err(e) => self.toasts.error(format!("{}: {e}", v.name)),
        }
        true
    }

    fn poll_export(&mut self, ctx: &egui::Context) -> bool {
        let Some(job) = &mut self.export else {
            return false;
        };
        let Some(result) = job.still.try_take() else {
            return true;
        };
        let (idx, (w, h)) = (job.idx, job.still.size());
        self.export = None;
        match result {
            Ok(rgba) => {
                let mut png_bytes = Vec::new();
                let mut enc = png::Encoder::new(&mut png_bytes, w, h);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                match enc
                    .write_header()
                    .and_then(|mut wr| wr.write_image_data(&rgba))
                {
                    Ok(()) => files::save_as(
                        &self.inbox,
                        ctx,
                        format!("visioncraft-{w}x{h}.png"),
                        png_bytes,
                        "png",
                    ),
                    Err(e) => self.toasts.error(format!("PNG encoding failed: {e}")),
                }
            }
            Err(e) => self.export_retry(idx, &e),
        }
        false
    }

    fn open_project(&mut self, name: &str, bytes: &[u8], path: Option<&Path>) {
        let opened = if extension(name) == "zip" {
            bundle::unpack(bytes).map(|u| (u.project, u.files))
        } else {
            Project::from_json_bytes(bytes)
                .map(|p| (p, Vec::new()))
                .map_err(|e| e.to_string())
        };
        match opened {
            Ok((p, files)) => {
                self.watcher.forget_all();
                self.studio.replace_project(p);
                self.project_name = Some(stem(name.trim_end_matches(".zip")));
                self.load_sources(path.and_then(Path::parent));
                self.take_bundled(files);
                self.studio.sync();
                self.studio.frame_all();
                self.saved_revision = self.studio.session.revision();
                self.hud.mode = None;
                let missing: Vec<String> = self
                    .studio
                    .missing_objects()
                    .iter()
                    .map(|o| format!("{} ({})", o.name, o.file))
                    .collect();
                if !missing.is_empty() {
                    self.toasts.warn(format!(
                        "{} {}",
                        strings::OBJECTS_MISSING,
                        missing.join(", ")
                    ));
                }
            }
            Err(e) => self.toasts.error(format!("{name}: {e}")),
        }
    }

    /// The files of an opened bundle: objects held by the asset store, model sources imported
    /// (in turn, off the UI thread) into the models that name them.
    fn take_bundled(&mut self, files: Vec<(String, Arc<[u8]>)>) {
        for (file, bytes) in files {
            if self.studio.project().objects.iter().any(|o| o.file == file) {
                if let Ok(c) = user_objects::check_glb(&bytes) {
                    let store = &mut self.studio.scene.library.store;
                    if let Some(t) = c.thumbnail {
                        store.provide(&user_objects::thumbnail_path(&file), Arc::from(t));
                    }
                    store.provide(&file, bytes);
                } else {
                    self.toasts
                        .warn(format!("{file}: {}", strings::OBJECT_UNREADABLE));
                }
                continue;
            }
            let models: Vec<String> = self
                .studio
                .project()
                .models
                .iter()
                .filter(|m| m.source == file)
                .map(|m| m.id.clone())
                .collect();
            for id in models {
                self.queued_jobs.push(Job {
                    name: file.clone(),
                    bytes: bytes.clone(),
                    path: None,
                    target: Some(id),
                    task: None,
                });
            }
        }
    }

    fn run_job(&mut self, ctx: &egui::Context, job: Job, model: Result<ImportedModel, String>) {
        let Job {
            name,
            bytes,
            path,
            target,
            ..
        } = job;
        let source_name = path
            .as_ref()
            .map_or_else(|| name.clone(), |p| p.display().to_string());
        let kind = if extension(&name) == "obj" {
            ModelKind::Obj
        } else {
            ModelKind::Craft3d
        };
        let src = Source {
            bytes,
            path: path.clone(),
        };
        let result = model.and_then(|model| match &target {
            Some(id) => self.studio.reimport_imported(id, model, src).map(|(w, d)| {
                if !name.starts_with("models/") {
                    self.toasts.info(strings::MODEL_UPDATED);
                }
                if d > 0 {
                    self.toasts
                        .warn(format!("{d} material assignments no longer match a face"));
                }
                (id.clone(), w)
            }),
            None => self
                .studio
                .add_imported(&stem(&name), kind, model, src, &source_name),
        });
        match result {
            Ok((id, warnings)) => {
                warnings.into_iter().for_each(|w| self.toasts.warn(w));
                if let Some(p) = path {
                    self.watcher.watch(&id, p);
                }
                if target.is_none() {
                    let mut cx = Cx {
                        s: &mut self.studio,
                        toasts: &mut self.toasts,
                        input: &Input::default(),
                        ctx,
                    };
                    self.modelui.after_import(&mut cx, &id);
                }
            }
            Err(e) => self.toasts.error(format!("{name}: {e}")),
        }
    }

    /// Re-imports watched model files that changed on disk (natively; a page sees no files, and
    /// the web build must not link the importer: it lives in the import worker's wasm).
    #[cfg(target_arch = "wasm32")]
    fn reload_changed(&mut self, _now: f64) {}

    #[cfg(not(target_arch = "wasm32"))]
    fn reload_changed(&mut self, now: f64) {
        let due: Vec<String> = self
            .changed
            .iter()
            .filter(|(_, t)| now >= **t)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.changed.remove(&id);
            let Some(path) = self.studio.sources.get(&id).and_then(|s| s.path.clone()) else {
                continue;
            };
            match std::fs::read(&path) {
                Ok(bytes) => {
                    // The assignments are keyed by face name, so they survive; the camera,
                    // sun and everything else are untouched.
                    match self.studio.reimport(
                        &id,
                        Source {
                            bytes: Arc::from(bytes),
                            path: Some(path),
                        },
                    ) {
                        Ok((w, dangling)) => {
                            self.toasts.info(strings::MODEL_UPDATED);
                            w.into_iter().for_each(|w| self.toasts.warn(w));
                            if dangling > 0 {
                                self.toasts.warn(format!(
                                    "{dangling} material assignments no longer match a face"
                                ));
                            }
                        }
                        Err(e) => self
                            .toasts
                            .error(format!("reload failed, keeping the old geometry: {e}")),
                    }
                }
                Err(e) => self
                    .toasts
                    .error(format!("cannot read {}: {e}", path.display())),
            }
        }
    }

    fn set_mode(&mut self, ctx: &egui::Context, mode: Option<Mode>) {
        let mut cx = Cx {
            s: &mut self.studio,
            toasts: &mut self.toasts,
            input: &Input::default(),
            ctx,
        };
        match self.hud.mode {
            Some(Mode::Draw) => self.draw.leave(&mut cx),
            Some(Mode::Materials) => self.materials.leave(&mut cx),
            Some(Mode::Objects) => self.objects.leave(&mut cx),
            Some(Mode::Sun) => self.terrain.leave(&mut cx),
            Some(Mode::Views) => self.views.leave(),
            Some(Mode::Extension(id)) => {
                cx.s.double_click_captured = false;
                if let Some(e) = self.extensions.iter_mut().find(|e| e.id() == id) {
                    e.leave(&mut extension::ExtensionCx {
                        cx: &mut cx,
                        overlay: &mut Default::default(),
                    });
                }
            }
            None => {}
        }
        if mode == Some(Mode::Draw) {
            cx.s.camera.leave_walk();
            cx.s.camera.cancel_fly();
            self.draw.enter(&mut cx);
        }
        self.hud.mode = mode;
    }

    /// Answers AI agents (see `agents`): their commands run on the window's session.
    #[cfg(not(target_arch = "wasm32"))]
    fn serve_agents(&mut self, ctx: &egui::Context, now: f64) -> bool {
        let focused = ctx.input(|i| i.focused);
        if focused && !self.focused {
            self.agents.focused();
        }
        self.focused = focused;
        let mut mode = self.hud.mode;
        let host = agents::WindowHost {
            studio: &mut self.studio,
            device: &self.device,
            queue: &self.queue,
            mode: &mut mode,
            planting: &mut self.objects.tools,
            model_selected: &mut self.modelui.selected,
            extension_modes: &self.hud.extension_modes,
            now,
        };
        let answered = self.agents.poll(host, now);
        if mode != self.hud.mode {
            self.set_mode(ctx, mode);
        }
        answered
    }

    /// The agents switch and what the last agent did, bottom right.
    #[cfg(not(target_arch = "wasm32"))]
    fn agents_pill(&mut self, ctx: &egui::Context, now: f64) {
        let narrow = ui::narrow(ctx);
        // Below the view bar (the bottom right is the plant browser's).
        let offset = if narrow {
            Vec2::new(-10.0, 104.0)
        } else {
            Vec2::new(-10.0, 64.0)
        };
        let mut on = self.agents.allowed();
        ui::floating(ctx, "agents", Align2::RIGHT_TOP, offset, 300.0, |ui| {
            ui.horizontal(|ui| {
                let label = match self.agents.port() {
                    Some(p) if !narrow => format!("{} · {p}", strings::ALLOW_AGENTS),
                    _ => strings::ALLOW_AGENTS.to_string(),
                };
                ui.checkbox(&mut on, label)
                    .on_hover_text(strings::ALLOW_AGENTS_TIP);
                if let Some((cmd, t)) = &self.agents.last
                    && now - t < 120.0
                {
                    ui.small(format!("{} {cmd}", strings::AGENT_LAST));
                }
            });
            if let Some(e) = &self.agents.error {
                ui.small(e.as_str());
            }
        });
        if on != self.agents.allowed() {
            self.agents.allow(on, ctx, None);
        }
    }

    fn toggle_walk(&mut self) {
        if self.hud.mode == Some(Mode::Draw) {
            return;
        }
        let cam = &mut self.studio.camera;
        if cam.view == camera::View::Walk {
            cam.leave_walk();
        } else {
            let eye = cam.eye();
            let ground = self.studio.scene.floor_below(eye.x, 500.0, eye.z);
            self.studio.camera.enter_walk(ground);
        }
    }

    fn act(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::Open => files::pick(&self.inbox, ctx, None, &files::OPEN_EXTENSIONS),
            Action::Save => {
                self.saving_revision = self.studio.session.revision();
                let stem = self
                    .project_name
                    .clone()
                    .unwrap_or_else(|| strings::UNSAVED_PROJECT.to_lowercase());
                // Imported objects (and, on the web, model files) only travel inside a bundle.
                let p = self.studio.project();
                let bundled = !p.objects.is_empty()
                    || (cfg!(target_arch = "wasm32") && !self.studio.sources.is_empty());
                if bundled {
                    let s = &self.studio;
                    let store = &s.scene.library.store;
                    match bundle::pack(p, &|f| store.provided(f), &|id| {
                        s.sources.get(id).map(|src| src.bytes.clone())
                    }) {
                        Ok(bytes) => files::save_as(
                            &self.inbox,
                            ctx,
                            format!("{stem}.visioncraft.zip"),
                            bytes,
                            "zip",
                        ),
                        Err(e) => self.toasts.error(e),
                    }
                } else {
                    files::save(
                        &self.inbox,
                        ctx,
                        format!("{stem}.visioncraft"),
                        p.to_json_bytes(),
                    );
                }
            }
            Action::New if self.dirty() => self.pending_replace = Some(Replace::New),
            Action::New => self.new_project(),
            Action::Export(i) => self.start_export(i),
            Action::ExportAll(i) => {
                let (_, w, h) = visioncraft_core::export_presets()[i];
                let todo: Vec<_> = self
                    .studio
                    .project()
                    .viewpoints
                    .iter()
                    .rev()
                    .cloned()
                    .collect();
                self.toasts
                    .info(format!("{} {} × {}…", strings::EXPORTING_VIEWS, w, h));
                self.export_all = Some(ExportAll {
                    size: (w, h),
                    count: todo.len(),
                    todo,
                    current: None,
                    done: Vec::new(),
                });
            }
            Action::Undo => self.studio.undo(),
            Action::Redo => self.studio.redo(),
            Action::ToggleWalk => self.toggle_walk(),
            Action::Mode(m) => self.set_mode(ctx, m),
            Action::SelectModel(id) => self.modelui.selected = Some(id),
            Action::ReloadModel(id) | Action::Relink(id) => {
                files::pick(&self.inbox, ctx, Some(id), &["3dcraft", "obj"])
            }
        }
    }

    fn title(&mut self, ctx: &egui::Context) {
        let title = format!(
            "{}{} — {}",
            if self.dirty() { "*" } else { "" },
            self.project_name
                .as_deref()
                .unwrap_or(strings::UNSAVED_PROJECT),
            strings::APP_NAME
        );
        if title != self.last_title {
            #[cfg(not(target_arch = "wasm32"))]
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            #[cfg(target_arch = "wasm32")]
            if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                d.set_title(&title);
            }
            let _ = ctx;
            self.last_title = title;
        }
    }

    fn bounds_json(&self) -> String {
        let mut hints = self.studio.bounds_hint.clone();
        for (id, l) in &self.studio.loaded {
            let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for b in &l.model.bodies {
                for k in 0..3 {
                    lo[k] = lo[k].min(b.bounds[0][k]);
                    hi[k] = hi[k].max(b.bounds[1][k]);
                }
            }
            if lo[0].is_finite() {
                hints.insert(id.clone(), [lo, hi]);
            }
        }
        serde_json::to_string(&hints).unwrap_or_default()
    }
}

impl eframe::App for App {
    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(3)
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let rev = self.studio.session.revision();
        if self.stored_revision == Some(rev) {
            return;
        }
        storage.set_string(
            KEY_PROJECT,
            String::from_utf8_lossy(&self.studio.project().to_json_bytes()).into_owned(),
        );
        storage.set_string(KEY_BOUNDS, self.bounds_json());
        #[cfg(not(target_arch = "wasm32"))]
        storage.set_string(
            KEY_AGENTS,
            if self.agents.allowed() { "1" } else { "0" }.into(),
        );
        if let Some(n) = &self.project_name {
            storage.set_string(KEY_NAME, n.clone());
        }
        self.stored_revision = Some(rev);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rect = ctx.content_rect();
        let ppp = ctx.pixels_per_point();
        let now = ctx.input(|i| i.time);
        let raw_dt = now - self.last_time;
        let dt = raw_dt.clamp(0.0, 0.1) as f32;
        self.last_time = now;
        let revision_at_start = self.studio.session.revision();
        let mut busy = false;

        // Work that arrived since last frame.
        if self.loading.is_none() && !self.queued_jobs.is_empty() {
            self.loading = Some(self.queued_jobs.remove(0));
        }
        if let Some(job) = &mut self.loading {
            match &job.task {
                None => {
                    let kind = match &job.target {
                        Some(id) => self.studio.model_kind(id),
                        None if extension(&job.name) == "obj" => Some(ModelKind::Obj),
                        None => Some(ModelKind::Craft3d),
                    };
                    match kind {
                        Some(kind) => {
                            job.task = Some(import_task::ImportTask::start(
                                kind,
                                job.bytes.clone(),
                                ctx.clone(),
                            ));
                        }
                        None => self.loading = None,
                    }
                }
                Some(task) => {
                    if let Some(result) = task.take()
                        && let Some(job) = self.loading.take()
                    {
                        self.run_job(&ctx, job, result);
                    }
                }
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            busy |= self.serve_agents(&ctx, now);
        }
        for m in self.inbox.take() {
            match m {
                Msg::Opened {
                    name,
                    bytes,
                    path,
                    target,
                } => self.handle_file(&ctx, name, bytes, path, target),
                Msg::FileChanged(id) => {
                    self.changed.insert(id, now + 0.35);
                }
                Msg::Saved(name) => {
                    self.toasts.info(format!("{} {name}", strings::SAVED));
                    self.saved_revision = self.saving_revision;
                    self.project_name = Some(stem(&name));
                }
                Msg::Failed(e) => self.toasts.error(e),
            }
        }
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            files::read_dropped(&self.inbox, &ctx, f);
        }
        if !self.changed.is_empty() {
            self.reload_changed(now);
            busy = true;
        }

        // Viewport input.
        let response = ui.interact(
            rect,
            egui::Id::new("viewport"),
            egui::Sense::click_and_drag(),
        );
        let typing = ctx.egui_wants_keyboard_input();
        let key = |k: Key| !typing && ctx.input(|i| i.key_pressed(k));
        let alt = ctx.input(|i| i.modifiers.alt);
        let input = Input {
            rect,
            hover: response.hovered().then(|| response.hover_pos()).flatten(),
            clicked: response.clicked_by(PointerButton::Primary),
            double_clicked: response.double_clicked(),
            drag_started: response.drag_started_by(PointerButton::Primary),
            press_origin: ctx.input(|i| i.pointer.press_origin()),
            dragging: response.dragged_by(PointerButton::Primary),
            drag_stopped: response.drag_stopped_by(PointerButton::Primary),
            alt,
            enter: key(Key::Enter),
            escape: key(Key::Escape),
            backspace: key(Key::Backspace),
            delete: key(Key::Delete),
        };

        // Shortcuts.
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        if !typing {
            if cmd && key(Key::Z) {
                self.act(&ctx, if shift { Action::Redo } else { Action::Undo });
            } else if cmd && key(Key::Y) {
                self.act(&ctx, Action::Redo);
            } else if !cmd && key(Key::Space) {
                self.toggle_walk();
            } else if !cmd && key(Key::F) {
                self.studio.frame_all();
            }
        }

        // Camera.
        // Tools claim the pointer each frame (handles, brush strokes): no orbit, no Shift-zoom.
        let captured = std::mem::take(&mut self.studio.pointer_captured);
        let cam = &mut self.studio.camera;
        let walking = cam.view == camera::View::Walk;
        if cam.is_flying() && (response.dragged() || response.double_clicked()) {
            cam.cancel_fly();
        }
        if response.hovered() && !(shift && captured) {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                cam.zoom(scroll);
            }
        }
        let d = response.drag_delta();
        if response.dragged_by(PointerButton::Secondary)
            || response.dragged_by(PointerButton::Middle)
        {
            cam.pan(d.x, d.y, rect.height());
        } else if response.dragged_by(PointerButton::Primary)
            && (walking || cam.view == camera::View::Orbit)
            && !captured
        {
            cam.orbit(d.x, d.y);
        }
        if input.double_clicked
            && !self.studio.double_click_captured
            && cam.view == camera::View::Orbit
            && let Some(p) = input.hover.and_then(|p| {
                self.studio.pick(
                    glam::Vec2::new(p.x, p.y),
                    glam::Vec2::new(rect.width(), rect.height()),
                    false,
                )
            })
        {
            self.studio.camera.set_pivot(p.point);
        }
        if walking {
            let down = |ks: &[Key]| ctx.input(|i| ks.iter().any(|k| i.key_down(*k)));
            let (fwd, right) = if typing {
                (0.0, 0.0)
            } else {
                (
                    f32::from(u8::from(down(&[Key::W, Key::ArrowUp])))
                        - f32::from(u8::from(down(&[Key::S, Key::ArrowDown]))),
                    f32::from(u8::from(down(&[Key::D, Key::ArrowRight])))
                        - f32::from(u8::from(down(&[Key::A, Key::ArrowLeft]))),
                )
            };
            let scene = &self.studio.scene;
            let before = self.studio.camera.walk.eye;
            self.studio
                .camera
                .walk_update(dt, fwd, right, shift, &|x, y, z| scene.floor_below(x, y, z));
            busy |= fwd != 0.0
                || right != 0.0
                || (self.studio.camera.walk.eye - before).length() > 1e-4;
        }
        if self.studio.camera.fly_update(now) {
            busy = true;
        }

        // Modes: pointer tools, overlays, highlights.
        self.studio.highlight = None;
        self.studio.extra = Default::default();
        let mut cx = Cx {
            s: &mut self.studio,
            toasts: &mut self.toasts,
            input: &input,
            ctx: &ctx,
        };
        if cx.s.placing_person && input.clicked {
            if let Some(h) = input.hover {
                let (o, d) = cx.s.pointer_ray(glam::Vec2::new(h.x, h.y), cx.size());
                let at =
                    cx.s.scene
                        .raycast(o, d, false)
                        .map(|p| p.point)
                        .or_else(|| camera::ray_plane(o, d, 0.0));
                if let Some(at) = at {
                    cx.s.people = vec![at];
                    cx.s.placing_person = false;
                }
            }
        } else if self.hud.mode.is_none()
            && input.clicked
            && let Some(h) = input.hover
            && let Some(p) = cx.s.pick(glam::Vec2::new(h.x, h.y), cx.size(), false)
            && !p.is_shape
        {
            self.modelui.selected = Some(p.owner);
        }
        match self.hud.mode {
            Some(Mode::Draw) => self.draw.update(&mut cx),
            Some(Mode::Materials) => self.materials.update(&mut cx),
            Some(Mode::Objects) => self.objects.update(&mut cx),
            Some(Mode::Sun) => self.terrain.update(&mut cx),
            _ => {}
        }
        let mut overlay = extension::Overlay::default();
        for e in &mut self.extensions {
            if self.hud.mode == Some(Mode::Extension(e.id())) {
                cx.s.double_click_captured = false;
                e.update(&mut extension::ExtensionCx {
                    cx: &mut cx,
                    overlay: &mut overlay,
                });
            }
            e.overlay(cx.s.project(), &mut overlay);
        }
        overlay.show(&mut cx);
        self.views.pump(&mut cx, &self.device, &self.queue);
        lamps::reconcile(&mut cx);
        // What the app does by itself while a restored project settles (lamp lights, the
        // catalogue arriving) is not the person's to undo.
        // shortcut: an edit made in those first seconds loses its undo step, rarely anyone's
        if self.settling_restore {
            let settled = cx.s.plants_pending() == 0
                && cx.s.scene.library.idle()
                && self.loading.is_none()
                && self.queued_jobs.is_empty();
            cx.s.session.clear_history();
            self.settling_restore = !settled;
        }
        busy |= self.views.busy();

        // Render.
        let size = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let quality_changed =
            Quality::PRESETS[self.studio.quality].1 != self.studio.renderer.quality();
        let rebuilt = self.studio.resize(size[0], size[1])
            || quality_changed
            || std::mem::take(&mut self.studio.targets_changed);
        self.studio.clock = now;
        busy |= self.studio.animating()
            || self.studio.plants_pending() > 0
            || !self.studio.scene.library.idle();
        self.studio.prepare();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let view = self.studio.renderer.render(&mut encoder);
        self.queue.submit([encoder.finish()]);
        let mut egui_renderer = self.egui_renderer.write();
        let id = match self.texture {
            Some(id) => {
                if rebuilt {
                    egui_renderer.update_egui_texture_from_wgpu_texture(
                        &self.device,
                        view,
                        wgpu::FilterMode::Linear,
                        id,
                    );
                }
                id
            }
            None => {
                egui_renderer.register_native_texture(&self.device, view, wgpu::FilterMode::Linear)
            }
        };
        drop(egui_renderer);
        self.texture = Some(id);
        ui.painter().image(
            id,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        // Smoothed rate of consecutive frames; a pause between repaints (idle) does not count, but
        // a slow frame after a repaint request does (else Auto never sees a scene it cannot draw).
        let consecutive = raw_dt > 0.0 && (raw_dt < 0.1 || (self.repainting && raw_dt < 2.0));
        if consecutive {
            self.studio.record_frame(raw_dt as f32 * 1000.0);
        } else {
            self.studio.frame_ms.clear();
        }
        if consecutive {
            self.fps = if self.fps == 0.0 {
                1.0 / raw_dt
            } else {
                self.fps * 0.9 + 0.1 / raw_dt
            };
        }

        // HUD.
        let mut cx = Cx {
            s: &mut self.studio,
            toasts: &mut self.toasts,
            input: &input,
            ctx: &ctx,
        };
        for m in std::mem::take(&mut cx.s.scene.library.messages) {
            cx.toasts.warn(m);
        }
        if std::mem::take(&mut cx.s.suggest_lights) {
            cx.toasts.info(strings::LIGHTS_SUGGEST);
        }
        let mut actions = self.hud.show(&mut cx, self.fps, walking);
        self.modelui.show(&mut cx, &mut actions);
        let (w, _h) = (rect.width(), rect.height());
        let panel_width = (w - 20.0).min(380.0);
        match self.hud.mode {
            Some(Mode::Draw) => ui::floating(
                &ctx,
                "modepanel",
                Align2::LEFT_BOTTOM,
                Vec2::new(10.0, -58.0),
                panel_width,
                |ui| {
                    self.draw.panel(ui, &mut cx);
                },
            ),
            Some(Mode::Objects) => ui::floating_own_scroll(
                &ctx,
                "modepanel",
                Align2::LEFT_BOTTOM,
                Vec2::new(10.0, -58.0),
                panel_width,
                |ui| {
                    self.objects.panel(ui, &mut cx);
                },
            ),
            Some(Mode::Sun) => ui::floating(
                &ctx,
                "modepanel",
                Align2::LEFT_BOTTOM,
                Vec2::new(10.0, -58.0),
                panel_width.max(300.0).min(w - 20.0),
                |ui| {
                    sun::panel(ui, &mut cx, &mut self.terrain);
                },
            ),
            Some(Mode::Views) => {
                ui::floating(
                    &ctx,
                    "modepanel",
                    Align2::LEFT_BOTTOM,
                    Vec2::new(10.0, -58.0),
                    w - 20.0,
                    |ui| self.views.panel(ui, &mut cx),
                );
                self.views.menu(&mut cx);
            }
            Some(Mode::Materials) => {
                self.materials.panel(&mut cx);
                ui::floating(
                    &ctx,
                    "modepanel",
                    Align2::LEFT_BOTTOM,
                    Vec2::new(10.0, -58.0),
                    panel_width,
                    |ui| {
                        ui.small(strings::MATERIALS_HINT);
                    },
                );
            }
            Some(Mode::Extension(id)) => {
                if let Some(e) = self.extensions.iter_mut().find(|e| e.id() == id) {
                    ui::floating(
                        &ctx,
                        "modepanel",
                        Align2::LEFT_BOTTOM,
                        Vec2::new(10.0, -58.0),
                        panel_width,
                        |ui| {
                            e.panel(
                                ui,
                                &mut extension::ExtensionCx {
                                    cx: &mut cx,
                                    overlay: &mut Default::default(),
                                },
                            )
                        },
                    );
                }
            }
            None => {}
        }
        // Models that need their file.
        let missing: Vec<(String, String)> =
            cx.s.missing_models()
                .iter()
                .map(|m| (m.id.clone(), m.name.clone()))
                .collect();
        let narrow = ui::narrow(&ctx);
        let mut relink = None;
        for (i, (id, name)) in missing.iter().enumerate() {
            ui::floating(
                &ctx,
                &format!("relink{i}"),
                Align2::CENTER_TOP,
                Vec2::new(0.0, (if narrow { 104.0 } else { 10.0 }) + 44.0 * i as f32),
                w - 20.0,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("{name} {}", strings::RELINK_PROMPT));
                        if ui.button(strings::RELINK).clicked() {
                            relink = Some(id.clone());
                        }
                    });
                },
            );
        }
        let toast_top = (if narrow { 150.0 } else { 56.0 })
            + 44.0 * missing.len() as f32 * f32::from(u8::from(narrow));
        busy |= cx.toasts.show(&ctx, toast_top);
        // Clay is saved with the project (and restored on the next visit): say so, with the
        // way back, so a white garden is never a mystery.
        if ui::clay_notice(cx.s.project()) {
            let mut off = false;
            ui::floating(
                &ctx,
                "clay-notice",
                Align2::CENTER_BOTTOM,
                Vec2::new(0.0, -64.0),
                360.0,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(strings::CLAY_NOTICE);
                        off = ui.button(strings::CLAY_OFF).clicked();
                    });
                },
            );
            if off {
                cx.run("style.set", serde_json::json!({"clay": false}));
            }
        }
        if cx.s.project().models.is_empty()
            && cx.s.project().shapes.is_empty()
            && cx.s.project().generators.is_empty()
            && cx.s.project().terrain.is_none()
            && self.hud.mode.is_none()
        {
            egui::Area::new(egui::Id::new("empty"))
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .order(egui::Order::Foreground)
                .interactable(false)
                .show(&ctx, |ui| {
                    ui.label(
                        egui::RichText::new(strings::DROP_HINT)
                            .size(15.0)
                            .color(egui::Color32::WHITE)
                            .background_color(egui::Color32::from_black_alpha(140)),
                    );
                });
        }
        if let Some(job) = &mut self.loading {
            egui::Area::new(egui::Id::new("loading"))
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .order(egui::Order::Tooltip)
                .show(&ctx, |ui| {
                    ui::panel_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(egui::RichText::new(strings::LOADING).size(18.0));
                            ui.weak(&job.name);
                        })
                    });
                });
            busy = true;
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.agents_pill(&ctx, now);
        busy |= self.poll_export(&ctx);
        busy |= self.poll_export_all(&ctx);
        let mut confirm = None;
        if self.pending_replace.is_some() {
            ui::floating(
                &ctx,
                "confirm",
                Align2::CENTER_CENTER,
                Vec2::ZERO,
                340.0,
                |ui| {
                    ui.label(strings::REPLACE_PROJECT);
                    ui.horizontal(|ui| {
                        if ui.button(strings::REPLACE_ANYWAY).clicked() {
                            confirm = Some(true);
                        }
                        if ui.button(strings::CANCEL).clicked() {
                            confirm = Some(false);
                        }
                    });
                },
            );
        }
        match (confirm, self.pending_replace.take()) {
            (Some(true), Some(Replace::Open { name, bytes, path })) => {
                self.open_project(&name, &bytes, path.as_deref())
            }
            (Some(true), Some(Replace::New)) => self.new_project(),
            (None, pending) => self.pending_replace = pending,
            _ => {}
        }
        for a in actions {
            self.act(&ctx, a);
        }
        if let Some(id) = relink {
            self.act(&ctx, Action::Relink(id));
        }
        self.title(&ctx);
        // New panels size themselves over two passes: keep painting briefly after any click.
        if ctx.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
            self.settle = 3;
        }
        if self.settle > 0 {
            self.settle -= 1;
            busy = true;
        }
        self.repainting = busy
            || self.studio.session.revision() != revision_at_start
            || self.studio.draft.is_some();
        if self.repainting {
            ctx.request_repaint();
        }
    }
}
