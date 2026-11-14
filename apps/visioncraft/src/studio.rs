//! Everything that is not a window: the session, the loaded geometry, the scene and the camera
//! over a renderer. The egui app and the headless smoke mode both drive this.

use glam::{Mat4, Vec3};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use visioncraft_core::{
    Asset, Generator, ImportedModel, Material, ModelKind, Project, Session, Shape, Site, SunTime,
    catalog::Category, import_3dcraft, import_obj, sun::sun_position,
};
use visioncraft_render::{LineVertex, Quality, Renderer, Sun};

use crate::camera::{Camera, View, ray_through};
use crate::scene::{Loaded, Pick, Scene, box_outline, light_markers, model_matrix};

/// Source bytes of a model, kept so it can be re-imported and so a project can be saved.
#[derive(Clone)]
pub struct Source {
    pub bytes: Arc<[u8]>,
    pub path: Option<PathBuf>,
}

/// An uncommitted edit shown live: applied on a copy of the project, committed on release.
#[derive(Clone, Debug)]
pub enum Draft {
    Material(Material),
    Shape(Shape),
    Generator(Generator),
    /// A terrain being sculpted, before its stroke is committed.
    Terrain(Arc<visioncraft_core::Terrain>),
}

pub struct Studio {
    pub session: Session,
    pub scene: Scene,
    pub camera: Camera,
    pub renderer: Renderer,
    pub loaded: HashMap<String, Loaded>,
    pub sources: HashMap<String, Source>,
    /// World-aligned bounds `[min, max]` of models whose bytes are missing (from the last session).
    pub bounds_hint: HashMap<String, [[f32; 3]; 2]>,
    pub assets: Vec<Asset>,
    pub quality: usize,
    /// Follow `suggest_quality` instead of the preset in `quality`.
    pub quality_auto: bool,
    pub auto_quality: Quality,
    pub frame_ms: Vec<f32>,
    /// Auto does not step up before this `clock` (seconds): it just stepped down.
    pub auto_hold_until: f64,
    /// Since when (clock) plants or textures have been loading without a break.
    pub auto_loading_since: Option<f64>,
    /// The wait after a step down (s) and when Auto last stepped up.
    pub auto_hold: f64,
    pub auto_rose_at: f64,
    /// What the hardware probe found and chose (`hardware::Measured::line`), for the Quality menu.
    pub hardware_line: Option<String>,
    /// What the GPU affords in lawn tufts over the quality's budget: 1 on integrated GPUs, more
    /// when the hardware probe measured a fast one (set by the window and the CLI).
    pub gpu_lawn_budget: f32,
    /// The Textures choice of the Quality menu: `None` follows the quality, else "1k", "2k" or
    /// "4k".
    pub textures: Option<&'static str>,
    /// The height of a still waiting for the highest texture tier of what is in frame.
    pub still_textures: Option<u32>,
    /// The Quality menu's Re-measure: calibrate again on the next frame.
    pub remeasure: bool,
    pub measuring: bool,
    /// UTC offset follows the EU summer-time rule for the chosen date.
    pub auto_dst: bool,
    pub base_offset: f64,
    /// Exposure offset in stops on top of the automatic exposure: positive is brighter.
    pub exposure: f32,
    pub draft: Option<Draft>,
    /// The species editor's plant: drawn, never part of the session's project.
    pub species_preview: Option<(visioncraft_core::CustomSpecies, Generator)>,
    pub time_draft: Option<SunTime>,
    /// The Compare button: a picture as planted is to be taken (`App::take_compare`), or the
    /// wipe closed.
    pub compare_requested: bool,
    pub compare_close: bool,
    /// Whether the wipe shows (set by the app each frame).
    pub compare_open: bool,
    /// The years after planting while the Years slider is dragged (`set_years_draft`).
    years_draft: Option<f32>,
    pub site_draft: Option<Site>,
    pub highlight: Option<(String, usize)>,
    /// A tool uses the primary drag (a handle, a stroke): the camera does not orbit. Tools set it
    /// every frame; the window reads and clears it.
    pub pointer_captured: bool,
    /// A tool finishes on a double click: the camera does not re-aim.
    pub double_click_captured: bool,
    /// Seconds since start, for the wind animation.
    pub clock: f64,
    /// A 1.8 m figure for scale (positions on the ground).
    pub people: Vec<Vec3>,
    pub show_people: bool,
    pub placing_person: bool,
    /// How many lights the project had last frame (`None` before the first), so the first one
    /// placed switches the lights on.
    pub lights_seen: Option<usize>,
    /// The exposure (EV) and sun elevation of the last frame, for the measurements.
    pub ev_used: (f32, f32),
    /// The lights' markers show while the Lights tool is open.
    pub light_markers: bool,
    /// The sun has set while the lights are off and the user has not been told yet.
    pub suggest_lights: bool,
    suggested: bool,
    pub dropped_lights: usize,
    /// The half-metre cell the ground bounce and indoors were last sampled in (`apply_view`).
    bounce_cell: Option<glam::IVec3>,
    /// Impostor atlases have been made ahead for the current quality.
    impostors_reserved: bool,
    /// The revision (and whether the catalogue was there) the lamp lights were last made for.
    lamps_reconciled: (u64, bool),
    /// Lamps over `lamps::MAX_FIXTURE_LIGHTS` at the last reconcile, and problems to show.
    pub lamps_skipped: usize,
    pub lamp_problems: Vec<String>,
    /// The image sky in use and the sun taken from it.
    pub sky: crate::sky_panel::SkyState,
    /// Library pictures shown in the panels.
    pub thumbs: crate::library_ui::Thumbs,
    last_lights: Vec<visioncraft_render::Light>,
    /// Extra overlay lines from tools: (depth tested, on top).
    pub extra: (Vec<LineVertex>, Vec<LineVertex>),
    /// Pictures (`render_png`) show the editor's overlays (light markers, tool lines, highlight).
    pub overlays_in_pictures: bool,
    /// The renderer's targets were made anew outside the window's frame (`render_png`): the
    /// window must hand the new view to egui.
    pub targets_changed: bool,
    pub size: [u32; 2],
    /// The sun shadows' reach last handed to the renderer (metres).
    shadow_reach: f32,
    draft_serial: u64,
    version: u64,
    synced: Option<(u64, u64)>,
    pub problems: Vec<String>,
}

pub fn make_asset(
    id: &str,
    name: &str,
    category: Category,
    tags: &[&str],
    size: [f32; 3],
) -> Asset {
    Asset {
        id: id.into(),
        name: name.into(),
        category,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        license: "CC0-1.0".into(),
        origin: Default::default(),
        author: "VisionCraft".into(),
        source_url: String::new(),
        size_m: size,
        lods: vec![],
        thumbnail: String::new(),
        plant: None,
        light: None,
    }
}

pub fn demo_assets() -> Vec<Asset> {
    let a = make_asset;
    vec![
        a("demo-tree", "Tree", Category::Tree, &[], [5.0, 7.0, 5.0]),
        a(
            "demo-conifer",
            "Conifer",
            Category::Tree,
            &["conifer"],
            [3.0, 8.0, 3.0],
        ),
        a("demo-shrub", "Shrub", Category::Shrub, &[], [1.4, 1.1, 1.4]),
        a(
            "demo-perennial",
            "Perennial",
            Category::Perennial,
            &[],
            [0.6, 0.6, 0.6],
        ),
        a(
            "demo-grass",
            "Grass tuft",
            Category::Grass,
            &[],
            [0.4, 0.5, 0.4],
        ),
    ]
}

/// How much of the ground's colour its bounced light keeps (0 grey, 1 all): enough that a
/// lawn reads faintly in a wall's shade, little enough that a soffit stays neutral.
const BOUNCE_CHROMA: f32 = 0.35;

/// A look's daylight exposure lift (stops, on automatic exposure only), contrast power and
/// saturation (`Renderer::set_grade`). The sky's exposure is saturation-based (luminance
/// 1.2 * 2^EV maps to 1.0); a camera meters an average scene to mid grey instead, about 0.8 stop
/// brighter, and AgX's soft shoulder takes a little more before sunlit paving reads light and
/// a sunlit lawn mid grey. The lift fades out below 20 degrees of sun so dusk and night keep
/// their tuning.
pub fn look_grade(look: visioncraft_core::Look, sun_elevation_deg: f32) -> (f32, f32, f32) {
    use visioncraft_core::Look;
    let day = ((sun_elevation_deg - 0.0) / 20.0).clamp(0.0, 1.0);
    match look {
        Look::Neutral => (0.0, 1.0, 1.0),
        Look::Natural => (1.2 * day, 1.1, 1.2),
        Look::Vivid => (1.4 * day, 1.3, 1.35),
    }
}

/// Automatic exposure (EV100, lower is brighter) from the sun: noon 15, sun at 10 degrees 13.5,
/// sunset 10.8, then down with
/// the twilight sky's light (about 2.5 EV per 3 degrees in the renderer's sky) so the garden
/// stays readable as a photographer's "blue hour" picture would; with `lamps` it stays a little
/// darker (higher EV) after sunset so that the garden lights glow. Linear between rows.
pub fn auto_ev(elevation_deg: f32, lamps: bool) -> f32 {
    const DAY: [(f32, f32); 6] = [
        (-18.0, -3.0),
        (-12.0, -0.5),
        (-8.0, 3.0),
        (-4.0, 6.4),
        (0.0, 10.8),
        (10.0, 13.5),
    ];
    const LAMPS: [(f32, f32); 6] = [
        (-18.0, -2.0),
        (-12.0, 0.3),
        (-8.0, 3.7),
        (-4.0, 6.9),
        (0.0, 10.8),
        (10.0, 13.5),
    ];
    let table: &[(f32, f32)] = if lamps && elevation_deg < 0.0 {
        &LAMPS
    } else {
        &DAY
    };
    let e = elevation_deg;
    if e >= 45.0 {
        return 15.0;
    }
    if e >= 10.0 {
        return 13.5 + 1.5 * (e - 10.0) / 35.0;
    }
    if e <= table[0].0 {
        return table[0].1;
    }
    for w in table.windows(2) {
        if e <= w[1].0 {
            return w[0].1 + (w[1].1 - w[0].1) * (e - w[0].0) / (w[1].0 - w[0].0);
        }
    }
    table[table.len() - 1].1
}

impl Studio {
    pub fn new(mut renderer: Renderer, project: Project) -> Self {
        let quality = 1;
        renderer.set_quality(Quality::PRESETS[quality].1);
        Self {
            session: Session::new(project),
            scene: Scene::default(),
            camera: Camera::default(),
            renderer,
            loaded: HashMap::new(),
            sources: HashMap::new(),
            bounds_hint: HashMap::new(),
            assets: demo_assets(),
            quality,
            quality_auto: true,
            auto_quality: Quality::MEDIUM,
            frame_ms: Vec::new(),
            auto_hold_until: 0.0,
            auto_loading_since: None,
            auto_hold: 10.0,
            auto_rose_at: f64::NEG_INFINITY,
            hardware_line: None,
            gpu_lawn_budget: 1.0,
            textures: None,
            still_textures: None,
            remeasure: false,
            measuring: false,
            auto_dst: true,
            base_offset: 1.0,
            exposure: 0.0,
            draft: None,
            species_preview: None,
            time_draft: None,
            years_draft: None,
            compare_requested: false,
            compare_close: false,
            compare_open: false,
            site_draft: None,
            highlight: None,
            pointer_captured: false,
            double_click_captured: false,
            clock: 0.0,
            people: Vec::new(),
            show_people: true,
            placing_person: false,
            lights_seen: None,
            ev_used: (0.0, 0.0),
            light_markers: false,
            suggest_lights: false,
            suggested: false,
            dropped_lights: 0,
            bounce_cell: None,
            impostors_reserved: false,
            lamps_reconciled: (u64::MAX, false),
            lamps_skipped: 0,
            lamp_problems: Vec::new(),
            sky: Default::default(),
            thumbs: Default::default(),
            last_lights: Vec::new(),
            extra: (Vec::new(), Vec::new()),
            overlays_in_pictures: false,
            targets_changed: false,
            size: [0, 0],
            shadow_reach: 0.0,
            draft_serial: 0,
            version: 0,
            synced: None,
            problems: Vec::new(),
        }
    }

    pub fn project(&self) -> &Project {
        self.session.project()
    }

    /// Undo and redo leave the lights as history has them: a lamp and its light are one step.
    pub fn undo(&mut self) {
        self.set_draft(None);
        self.session.undo();
        self.lamps_reconciled = (self.session.revision(), self.lamps_known());
    }

    pub fn redo(&mut self) {
        self.set_draft(None);
        self.session.redo();
        self.lamps_reconciled = (self.session.revision(), self.lamps_known());
    }

    fn lamps_known(&self) -> bool {
        self.scene.library.catalog.is_some()
    }

    /// Runs a command, and in the same undo step the light changes of the lamps it placed,
    /// moved or removed.
    pub fn run(
        &mut self,
        name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, visioncraft_core::CommandError> {
        self.with_lamps(|s| s.run(name, params))
    }

    /// Runs `f` and the lamp reconcile (`lamps::reconcile_commands`) as one transaction.
    pub fn with_lamps<T>(
        &mut self,
        f: impl FnOnce(&mut Session) -> Result<T, visioncraft_core::CommandError>,
    ) -> Result<T, visioncraft_core::CommandError> {
        let lamps: Vec<(String, visioncraft_core::LightSpec)> = self
            .scene
            .library
            .catalog
            .as_ref()
            .map(|c| {
                c.assets
                    .iter()
                    .filter_map(|a| a.light.map(|l| (a.id.clone(), l)))
                    .collect()
            })
            .unwrap_or_default();
        let scene = &self.scene;
        let mut problems = Vec::new();
        let mut skipped = None;
        let out = self.session.transaction(|s| {
            let v = f(s)?;
            if !lamps.is_empty() || !s.project().lights.is_empty() {
                let r = crate::lamps::reconcile_commands(s.project(), &lamps, &|g| {
                    scene.evaluate_generator(g).placements
                });
                for (name, params) in r.commands {
                    // a light that cannot be made must not undo the user's change
                    if let Err(e) = s.run(name, params) {
                        problems.push(e.to_string());
                    }
                }
                skipped = Some(r.skipped);
            }
            Ok(v)
        });
        if let Some(n) = skipped {
            if n > 0 && n != self.lamps_skipped {
                problems.push(format!(
                    "{n} {} {}",
                    crate::strings::LAMPS_OVER_CAP,
                    crate::lamps::MAX_FIXTURE_LIGHTS
                ));
            }
            self.lamps_skipped = n;
        }
        self.lamp_problems.extend(problems);
        self.lamps_reconciled = (self.session.revision(), !lamps.is_empty());
        out
    }

    /// Makes the lights of lamps right after a change that did not go through `run` (a loaded
    /// project, the catalogue arriving, a command from automation): as its own undo step.
    pub fn reconcile_lamps(&mut self) -> Vec<String> {
        if self.lamps_reconciled != (self.session.revision(), self.lamps_known()) {
            let _ = self.with_lamps(|_| Ok(()));
        }
        std::mem::take(&mut self.lamp_problems)
    }

    /// Puts `session` in place of the current one without clearing the scene (pictures of
    /// another project state); its lamps get their lights at the next reconcile.
    pub fn set_session(&mut self, session: Session) {
        self.session = session;
        self.lamps_reconciled = (u64::MAX, false);
    }

    /// Makes the next `sync` look at the scene again although the project did not change.
    pub fn force_sync(&mut self) {
        self.draft_serial += 1;
    }

    pub fn set_draft(&mut self, d: Option<Draft>) {
        self.draft = d;
        self.draft_serial += 1;
    }

    /// Replaces the whole project (open, restore): clears history and the scene.
    pub fn replace_project(&mut self, project: Project) {
        self.session.replace_project(project);
        self.scene.clear(&mut self.renderer);
        self.loaded.clear();
        self.sources.clear();
        self.draft = None;
        self.synced = None;
    }

    pub fn time(&self) -> SunTime {
        self.time_draft.unwrap_or(self.project().time)
    }

    /// Lamps and lit rooms: the project's view setting, else on after sunset at the time shown.
    pub fn lights_on(&self) -> bool {
        self.project().lights_on(&self.time())
    }

    pub fn site(&self) -> Site {
        self.site_draft.unwrap_or(self.project().site)
    }

    // ---- models ----

    /// Parses and meshes model bytes. This blocks the caller: the window runs it through
    /// `import_task` (a thread, or a Web Worker).
    pub fn import(kind: ModelKind, bytes: &[u8]) -> Result<ImportedModel, String> {
        match kind {
            ModelKind::Craft3d => import_3dcraft(bytes),
            ModelKind::Obj => import_obj(bytes),
        }
        .map_err(|e| e.to_string())
    }

    fn install(&mut self, id: &str, model: ImportedModel, source: Source) -> Vec<String> {
        self.version += 1;
        self.draft_serial += 1; // forces a scene sync even though the project did not change
        let warnings = model.warnings.clone();
        self.bounds_hint.remove(id);
        self.loaded.insert(
            id.to_string(),
            Loaded {
                model: Arc::new(model),
                version: self.version,
            },
        );
        self.sources.insert(id.to_string(), source);
        warnings
    }

    /// Imports and adds a model with `model.add`. Returns its id and the import warnings.
    pub fn add_model(
        &mut self,
        name: &str,
        kind: ModelKind,
        source: Source,
        source_name: &str,
    ) -> Result<(String, Vec<String>), String> {
        let model = Self::import(kind, &source.bytes)?;
        self.add_imported(name, kind, model, source, source_name)
    }

    /// Adds a model imported elsewhere (`import_task`) with `model.add`.
    pub fn add_imported(
        &mut self,
        name: &str,
        kind: ModelKind,
        model: ImportedModel,
        source: Source,
        source_name: &str,
    ) -> Result<(String, Vec<String>), String> {
        let kind_name = if kind == ModelKind::Obj {
            "obj"
        } else {
            "3dcraft"
        };
        let r = self
            .session
            .run(
                "model.add",
                serde_json::json!({"name": name, "kind": kind_name, "source": source_name}),
            )
            .map_err(|e| e.to_string())?;
        let id = r["id"].as_str().unwrap_or("m1").to_string();
        let warnings = self.install(&id, model, source);
        let first = self.project().models.len() == 1 && self.project().shapes.is_empty();
        if first {
            self.sync();
            self.frame_all();
        }
        Ok((id, warnings))
    }

    /// Installs geometry loaded elsewhere (automation, a loader) for model `id`, replacing what
    /// was there; the next `sync` draws it.
    pub fn install_model(&mut self, id: &str, model: Arc<ImportedModel>) {
        self.version += 1;
        self.draft_serial += 1;
        self.bounds_hint.remove(id);
        self.loaded.insert(
            id.to_string(),
            Loaded {
                model,
                version: self.version,
            },
        );
    }

    /// One picture of `w` x `h` from the current camera at the current quality, as PNG bytes,
    /// with the editor's overlays left out unless `overlays_in_pictures`. Plants still being
    /// generated get frames to finish. The live size is restored afterwards.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_png(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        w: u32,
        h: u32,
    ) -> Result<Vec<u8>, String> {
        let pixels = self.render_pixels(device, queue, w, h)?;
        // Lossless truecolour (what `ui.screenshot` and agents send on).
        png_rgb(&pixels, w, h)
    }

    /// The 3D view at `w` x `h` as RGBA8 once every plant is generated (blocks until then).
    pub fn render_pixels(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        w: u32,
        h: u32,
    ) -> Result<Vec<u8>, String> {
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return Err(format!("picture size {w}x{h} outside 1x1..=8192x8192"));
        }
        let live = self.size;
        self.resize(w, h);
        let (_, mut problems) = self.settle_for_picture();
        self.problems.append(&mut problems);
        if !self.overlays_in_pictures {
            self.renderer.set_overlay_lines(&[]);
            self.renderer.set_overlay_lines_on_top(&[]);
            self.renderer.set_highlight(None);
        }
        let rb = Readback::start(self, device, queue);
        let polled = device.poll(wgpu::PollType::wait_indefinitely());
        let pixels = rb.take(device);
        if live != [0, 0] && live != [w, h] {
            self.resize(live[0], live[1]);
            self.targets_changed = true;
        }
        polled.map_err(|e| format!("the GPU did not finish: {e}"))?;
        pixels.ok_or_else(|| "reading the picture back failed".to_string())
    }

    /// Steps the scene until every plant is generated and the library has nothing on its way,
    /// by the clock (plants are made on threads: frames alone pass faster than they do on a fast
    /// GPU), for at most `PICTURE_WAIT_S`. Returns the steps taken and, in plain words, what was
    /// not ready in time and what the library could not load.
    pub fn settle_for_picture(&mut self) -> (u32, Vec<String>) {
        let start = crate::assets::now_ms();
        let mut steps = 0;
        let mut lamp_problems = Vec::new();
        let stream = self.scene.library.store.stream_bytes;
        self.scene.library.store.stream_bytes = 64 << 20;
        loop {
            self.prepare();
            // The lights of lamps, once the catalogue that says which assets are lamps is in (a
            // project made without a window has none yet).
            lamp_problems.extend(self.reconcile_lamps());
            steps += 1;
            if self.plants_pending() == 0 && self.scene.library.idle() {
                break;
            }
            if crate::assets::now_ms() - start > PICTURE_WAIT_S * 1000.0 {
                break;
            }
            #[cfg(not(target_arch = "wasm32"))]
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.scene.library.store.stream_bytes = stream;
        self.prepare();
        let mut problems = picture_problems(self.plants_pending(), self.scene.library.idle());
        problems.append(&mut lamp_problems);
        problems.append(&mut self.scene.library.messages);
        problems.append(&mut self.scene.library.store.messages);
        (steps, problems)
    }

    /// Gets the scene ready for a photo at `w` x `h`: plants grown, overlays off (when pictures
    /// leave them out). Then `photo_begin`/`photo_sample` on the renderer; `photo_end` after.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn photo_prepare(&mut self, w: u32, h: u32) -> [u32; 2] {
        let live = self.size;
        self.resize(w, h);
        let (_, mut problems) = self.settle_for_picture();
        self.problems.append(&mut problems);
        if !self.overlays_in_pictures {
            self.renderer.set_overlay_lines(&[]);
            self.renderer.set_overlay_lines_on_top(&[]);
            self.renderer.set_highlight(None);
        }
        live
    }

    /// Puts the live size back after `photo_prepare`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn photo_end(&mut self, live: [u32; 2]) {
        if live != [0, 0] && live != self.size {
            self.resize(live[0], live[1]);
            self.targets_changed = true;
        }
    }

    /// A photo of the 3D view at `w` x `h`: `samples` frames at ULTRA averaged (soft shadows,
    /// smooth edges, smooth occlusion); with `lens` `(aperture radius m, focus distance m)`,
    /// depth of field. RGBA8. Blocks until done.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_photo(
        &mut self,
        w: u32,
        h: u32,
        samples: u32,
        lens: Option<(f32, Option<f32>)>,
    ) -> Result<Vec<u8>, String> {
        let live = self.photo_prepare(w, h);
        let (view, proj, eye) = self.camera.matrices(w as f32 / h as f32);
        let (aperture, focus) = self.lens(lens);
        let result = (|| {
            let mut photo = self.renderer.photo_begin(w, h, Quality::ULTRA)?;
            for k in 0..samples.max(1) {
                self.renderer
                    .photo_sample(&mut photo, view, proj, eye, k, aperture, focus)?;
            }
            self.renderer.photo_finish(&photo).wait()
        })();
        self.photo_end(live);
        result
    }

    /// The lens' aperture radius and focus distance (the camera's pivot when not given).
    pub fn lens(&self, lens: Option<(f32, Option<f32>)>) -> (f32, f32) {
        let pivot = (self.camera.eye() - self.camera.pivot).length().max(0.1);
        lens.map_or((0.0, pivot), |(a, f)| (a, f.unwrap_or(pivot)))
    }

    /// Swaps the geometry of an existing model, touching nothing else. Returns warnings and the
    /// number of assignments that no longer match a face.
    pub fn reimport(&mut self, id: &str, source: Source) -> Result<(Vec<String>, usize), String> {
        let kind = self.model_kind(id).ok_or("no such model")?;
        let model = Self::import(kind, &source.bytes)?;
        self.reimport_imported(id, model, source)
    }

    pub fn model_kind(&self, id: &str) -> Option<ModelKind> {
        self.project()
            .models
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.kind)
    }

    /// Swaps in geometry imported elsewhere (`import_task`).
    pub fn reimport_imported(
        &mut self,
        id: &str,
        model: ImportedModel,
        source: Source,
    ) -> Result<(Vec<String>, usize), String> {
        let dangling = visioncraft_core::dangling_assignments(self.project(), id, &model).len();
        Ok((self.install(id, model, source), dangling))
    }

    // ---- rendering ----

    pub fn project_view(&self) -> Cow<'_, Project> {
        let mut p = Cow::Borrowed(self.session.project());
        match &self.draft {
            Some(Draft::Material(m)) => {
                if let Some(slot) = p.to_mut().materials.iter_mut().find(|x| x.id == m.id) {
                    *slot = m.clone();
                }
            }
            Some(Draft::Shape(s)) => {
                if let Some(slot) = p.to_mut().shapes.iter_mut().find(|x| x.id() == s.id()) {
                    *slot = s.clone();
                }
            }
            Some(Draft::Generator(g)) => {
                if let Some(slot) = p.to_mut().generators.iter_mut().find(|x| x.id() == g.id()) {
                    *slot = g.clone();
                }
            }
            Some(Draft::Terrain(t)) => p.to_mut().terrain = Some(t.clone()),
            None => {}
        }
        if let Some(y) = self.years_draft {
            p.to_mut().view.years = Some(y);
        }
        if let Some((sp, g)) = &self.species_preview {
            let p = p.to_mut();
            p.species.push(sp.clone());
            p.generators.push(g.clone());
        }
        p
    }

    /// Shows the plants at `years` after planting without a command (while a slider moves);
    /// `None` returns to the project's setting.
    pub fn set_years_draft(&mut self, years: Option<f32>) {
        if self.years_draft != years {
            self.years_draft = years;
            self.draft_serial += 1;
        }
    }

    pub fn years_draft(&self) -> Option<f32> {
        self.years_draft
    }

    pub fn sync(&mut self) {
        let key = (self.session.revision(), self.draft_serial);
        if self.synced == Some(key) {
            return;
        }
        self.synced = Some(key);
        let project = self.project_view().into_owned();
        let problems = self
            .scene
            .sync(&mut self.renderer, &project, &self.loaded, &self.assets);
        self.problems.extend(problems);
    }

    pub fn frame_all(&mut self) {
        let mut b = self.scene.bounds();
        // Models that are not loaded still occupy their remembered box.
        for m in self.missing_models() {
            if let Some([lo, hi]) = self.bounds_hint.get(&m.id) {
                let xf = model_matrix(m);
                for i in 0..8usize {
                    let p = |k: usize| if (i >> k) & 1 == 0 { lo[k] } else { hi[k] };
                    let w = xf.transform_point3(Vec3::new(p(0), p(1), p(2)));
                    b = Some(b.map_or((w, w), |(a, c)| (a.min(w), c.max(w))));
                }
            }
        }
        let (lo, hi) = b.unwrap_or((Vec3::splat(-5.0), Vec3::splat(5.0)));
        self.camera.frame(lo, hi);
    }

    pub fn sun(&self) -> (Sun, f32) {
        if let Some(s) = self.sky.sun_override() {
            return s;
        }
        let pos = sun_position(&self.site(), &self.time());
        (
            Sun::from_angles(pos.azimuth_deg as f32, pos.elevation_deg as f32),
            pos.elevation_deg as f32,
        )
    }

    pub fn aspect(&self) -> f32 {
        self.size[0] as f32 / self.size[1].max(1) as f32
    }

    pub fn resize(&mut self, w: u32, h: u32) -> bool {
        if self.size == [w, h] {
            return false;
        }
        self.renderer.resize(w, h);
        self.size = [w, h];
        true
    }

    /// Sky, haze, ground, wind, exposure and lights from the project's view state.
    fn apply_view(&mut self, elevation: f32) {
        let v = self.project().view.clone();
        let lamps = self.lights_on() && self.project().lights.iter().any(|l| l.enabled);
        let (brighter, power, saturation) = look_grade(v.look, elevation);
        let ev = v
            .exposure_ev
            .unwrap_or_else(|| self.sky.auto_ev(elevation, lamps) - brighter)
            - self.exposure;
        self.ev_used = (ev, elevation);
        self.renderer.set_exposure(ev);
        self.renderer.set_grade(power, saturation);
        self.renderer.set_clouds(&visioncraft_render::Clouds {
            cover: match v.clouds.kind {
                visioncraft_core::CloudKind::Clear => 0.0,
                _ => v.clouds.cover,
            },
            overcast: v.clouds.kind == visioncraft_core::CloudKind::Overcast,
        });
        // Light bounced up from what lies under the camera (paving under a canopy, lawn in the
        // garden), with most of its colour taken out: a green lawn tints a wall only a little.
        let eye = self.camera.eye();
        let cell = (eye * 2.0).round().as_ivec3();
        if self.bounce_cell != Some(cell) {
            self.bounce_cell = Some(cell);
            let albedo = self
                .scene
                .albedo_below(self.project(), eye)
                .or(v.ground.map(|g| g.color))
                .map(|c| {
                    let lum = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                    c.map(|k| ((lum + BOUNCE_CHROMA * (k - lum)) * 100.0).round() / 100.0)
                });
            self.renderer.set_bounce_albedo(albedo);
            // Indoors: something over the head and walls (glass counts) nearly all round within a
            // room's reach (15 m). Under an open canopy the garden side stays open.
            let hit = |dir: Vec3| {
                self.scene
                    .raycast(eye, dir, false)
                    .is_some_and(|p| p.distance < 15.0)
            };
            let walls = (0..8)
                .filter(|k| {
                    let a = *k as f32 * std::f32::consts::FRAC_PI_4;
                    hit(Vec3::new(a.cos(), 0.0, a.sin()))
                })
                .count();
            // Six of eight: a room open at its ends (a glazed gallery) still counts.
            let indoors = hit(Vec3::Y) && walls >= 6;
            self.renderer.set_indoors(indoors);
        }
        self.renderer.set_clay(self.project().style.clay);
        // Below the horizon the haze only scatters the dim, pink twilight sky back at the camera
        // and washes the garden out: thin it as the sun sinks, and no sun glow.
        let night = ((1.0 - elevation) / 7.0).clamp(0.0, 1.0);
        let reach = view_reach(
            &self.camera,
            v.ground.map_or(0.0, |g| g.height),
            v.view_distance,
        );
        self.renderer
            .set_atmosphere(&visioncraft_render::Atmosphere {
                haze: v.haze * (1.0 - 0.7 * night),
                sun_glow: 0.5 * (1.0 - night),
                clear_m: reach.clear_m,
                ..Default::default()
            });
        self.renderer.set_view_distance(v.view_distance);
        self.renderer.set_impostor_reach(reach.small_impostors);
        if self.shadow_reach != reach.shadow_m {
            self.shadow_reach = reach.shadow_m;
            self.renderer.set_shadow_distance(reach.shadow_m);
        }
        // Lamps after sunset bloom a little more, so they read as lights.
        let q = self.current_quality();
        if q.bloom {
            self.renderer
                .set_bloom(if lamps { 0.025 + 0.035 * night } else { 0.025 });
        }
        self.renderer
            .set_ground(v.ground.map(|g| visioncraft_render::Ground {
                height: g.height,
                color: g.color,
                ..Default::default()
            }));
        let d = v.wind_direction_deg.to_radians();
        self.renderer.set_wind(&visioncraft_render::Wind {
            direction: Vec3::new(d.sin(), 0.0, -d.cos()),
            strength: v.wind_strength,
            ..Default::default()
        });
        self.renderer.set_time(self.clock);
        let lights = if self.lights_on() {
            render_lights(self.project())
        } else {
            Vec::new()
        };
        if lights != self.last_lights {
            self.dropped_lights = self.renderer.set_lights(&lights);
            self.last_lights = lights;
        }
        let people: Vec<glam::Mat4> = if self.show_people {
            self.people
                .iter()
                .map(|p| glam::Mat4::from_translation(*p))
                .collect()
        } else {
            Vec::new()
        };
        self.scene.set_people(&mut self.renderer, &people);
    }

    /// Whether anything on screen moves by itself (wind with plants in view).
    pub fn current_quality(&self) -> Quality {
        if self.quality_auto {
            self.auto_quality
        } else {
            Quality::PRESETS[self.quality].1
        }
    }

    /// Feeds one frame time (ms); Auto may change the quality. Down as soon as 30 frames show
    /// the budget missed (the 75th percentile over 30 fps, so hitches count too), up only after
    /// 90 frames well inside it, never while plants or textures load (those frames say nothing
    /// about the scene) and not for 10 s after a step down (else it climbs back into the tier it
    /// just left and settles below 30 fps).
    pub fn record_frame(&mut self, ms: f32) {
        if !self.quality_auto {
            self.frame_ms.clear();
            return;
        }
        // Frames while plants or textures load say little about the scene, but only for a few
        // seconds: a tier too heavy for the GPU makes loading itself slow (a step to HIGH
        // regenerates the leaf atlases and fetches 2k maps), and Auto must still step down.
        let loading = self.plants_pending() > 0 || !self.scene.library.idle();
        if loading {
            let since = *self.auto_loading_since.get_or_insert(self.clock);
            if self.clock - since < AUTO_LOAD_GRACE_S {
                self.frame_ms.clear();
                return;
            }
        } else {
            self.auto_loading_since = None;
        }
        self.frame_ms.push(ms);
        let can_rise = self.clock >= self.auto_hold_until;
        match auto_step(&self.frame_ms, self.auto_quality, can_rise) {
            AutoStep::Wait => {}
            AutoStep::Keep => {
                let n = self.frame_ms.len();
                self.frame_ms.drain(..n - 30);
            }
            AutoStep::To(next, down) => {
                if down {
                    // Falling back soon after a rise: that rung does not hold, wait longer
                    // before trying it again (10 s doubling up to 2 min), so it does not see-saw.
                    self.auto_hold = if self.clock - self.auto_rose_at < 15.0 {
                        (self.auto_hold * 2.0).min(120.0)
                    } else {
                        10.0
                    };
                    self.auto_hold_until = self.clock + self.auto_hold;
                } else {
                    self.auto_rose_at = self.clock;
                }
                crate::hardware::note(&format!(
                    "auto quality: {} at {:.0}%",
                    preset_name(&next),
                    next.render_scale * 100.0
                ));
                self.auto_quality = next;
                self.frame_ms.clear();
            }
        }
    }

    /// Plants still being generated.
    pub fn plants_pending(&self) -> usize {
        self.scene.flora.pending() + self.scene.lawns_pending() + self.scene.clutter_pending()
    }

    pub fn animating(&self) -> bool {
        let p = self.project();
        p.view.wind_strength > 0.0 && !p.generators.is_empty()
    }

    /// Lowest point of a model in its own space, and its local bounds.
    pub fn model_bounds(&self, id: &str) -> Option<([f32; 3], [f32; 3])> {
        let l = self.loaded.get(id)?;
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for b in &l.model.bodies {
            for k in 0..3 {
                lo[k] = lo[k].min(b.bounds[0][k]);
                hi[k] = hi[k].max(b.bounds[1][k]);
            }
        }
        lo[0].is_finite().then_some((lo, hi))
    }

    /// Puts the figure just in front (+Z side) of a model, on the ground.
    pub fn person_near(&mut self, id: &str) {
        let Some((lo, hi)) = self.model_bounds(id) else {
            return;
        };
        let xf = self.world_matrix(id);
        let c = xf.transform_point3(Vec3::new((lo[0] + hi[0]) / 2.0, lo[1], hi[2]));
        let s = xf.transform_vector3(Vec3::Z).length();
        let ground = self.project().view.ground.map_or(0.0, |g| g.height);
        self.people = vec![Vec3::new(
            c.x,
            ground.max(c.y),
            c.z + 1.0 + 0.5 * s * (hi[2] - lo[2]).min(1.0),
        )];
    }

    /// Sets everything the renderer needs for the next frame.
    pub fn prepare(&mut self) {
        self.scene.library.lamps_on = self.lights_on();
        let eye = self.camera.eye();
        let h = self.still_textures.unwrap_or(self.size[1]).max(1) as f32;
        let cam = &self.camera;
        let (top, half, tan) = (
            cam.view == crate::camera::View::Top,
            cam.top_half_height,
            (cam.fov_y / 2.0).tan(),
        );
        let px_per_m = move |d: f32| {
            if top {
                h / (2.0 * half)
            } else {
                h / (2.0 * d * tan)
            }
        };
        if self.scene.pump(&mut self.renderer, eye, &px_per_m) {
            self.draft_serial += 1;
        }
        self.scene
            .library
            .set_user_objects(&self.session.project().objects);
        self.take_catalog();
        self.sync();
        self.scene
            .sync_terrain_layers(&mut self.renderer, self.session.project());
        let (view, proj, eye) = self.camera.matrices(self.aspect());
        self.renderer.set_camera(view, proj, eye);
        let (sun, elevation) = self.sun();
        self.renderer.set_sun(&sun);
        self.apply_view(elevation);
        crate::sky_panel::apply(self);
        if elevation < 0.0
            && !self.lights_on()
            && !self.suggested
            && !self.project().lights.is_empty()
        {
            self.suggested = true;
            self.suggest_lights = true;
        }
        let q = self.current_quality();
        // Textures follow the quality unless the person chose a tier.
        // A still waiting for its textures takes the highest tier at its own resolution.
        let tier = if let Some(h) = self.still_textures {
            // In the browser the 4k tier of a close-up's materials took over 600 MB and did not
            // arrive within the still's wait: up to QHD there, 2k is what the pixels show.
            if cfg!(target_arch = "wasm32") && h <= 1440 {
                "2k"
            } else {
                "4k"
            }
        } else {
            self.textures.unwrap_or(match preset_name(&q) {
                "ultra" => "4k",
                "high" => "2k",
                _ => "1k",
            })
        };
        self.scene.library.set_texture_tier(tier);
        let fine = matches!(preset_name(&q), "high" | "ultra");
        self.scene.flora.leaf_scale = if fine { 2 } else { 1 };
        if self.scene.set_dense_lawns(&mut self.renderer, fine) {
            self.synced = None; // the lawns are made again on the next sync
        }
        let budget = self.gpu_lawn_budget * if preset_name(&q) == "ultra" { 2.0 } else { 1.0 };
        if self.scene.set_lawn_budget(&mut self.renderer, budget) {
            self.synced = None;
        }
        if self.scene.set_clutter_low(preset_name(&q) == "low") {
            self.synced = None;
        }
        let rebake = q.impostor_resolution != self.renderer.quality().impostor_resolution;
        self.renderer.set_quality(q);
        if rebake || !self.impostors_reserved {
            // The atlases a load bakes, made before it (a few large ones on ULTRA).
            let count = match q.impostor_resolution {
                512.. => 6,
                256.. => 16,
                _ => 24,
            };
            // First the memory the load's textures will take, so none is taken mid-load.
            self.renderer.warm_texture_memory(match preset_name(&q) {
                "ultra" => 256,
                "high" => 128,
                _ => 64,
            });
            self.renderer
                .reserve_impostors(crate::plants::IMPOSTOR_VIEWS, count);
            self.impostors_reserved = true;
        }
        if rebake {
            self.scene.flora.rebake();
        }
        let hl = self
            .highlight
            .as_ref()
            .and_then(|(o, f)| self.scene.highlight(o, *f));
        self.renderer.set_highlight(hl);
        let markers = if std::mem::take(&mut self.light_markers) {
            light_markers(self.project())
        } else {
            Vec::new()
        };
        let (mut depth, top) = (markers, Vec::new());
        depth.extend(self.missing_model_boxes());
        depth.extend(self.extra.0.iter().copied());
        let mut top = top;
        top.extend(self.extra.1.iter().copied());
        self.renderer.set_overlay_lines(&depth);
        self.renderer.set_overlay_lines_on_top(&top);
    }

    /// The catalogue's assets join the demo ones, so placeholders and names work for them.
    fn take_catalog(&mut self) {
        let lib = &mut self.scene.library;
        if !std::mem::take(&mut lib.catalog_arrived) {
            return;
        }
        // Imported objects come and go with the project: theirs are taken afresh.
        self.assets
            .retain(|a| a.origin != visioncraft_core::catalog::Origin::User);
        if let Some(c) = lib.catalog.clone() {
            for a in &c.assets {
                if !self.assets.iter().any(|x| x.id == a.id) {
                    self.assets.push(a.clone());
                }
            }
        }
    }

    /// Imports a `.glb` as a project object: checked, its bytes held by the asset store, and
    /// recorded with `object.add`. Returns the object's asset id (`user:<id>`).
    pub fn import_object(&mut self, name: &str, bytes: Vec<u8>) -> Result<String, String> {
        let checked = crate::user_objects::check_glb(&bytes)?;
        let file = crate::user_objects::file_for(name, &self.project().objects);
        let store = &mut self.scene.library.store;
        store.provide(&file, Arc::from(bytes));
        if let Some(t) = checked.thumbnail {
            store.provide(&crate::user_objects::thumbnail_path(&file), Arc::from(t));
        }
        let r = self
            .run(
                "object.add",
                serde_json::json!({"name": name, "file": file, "size_m": checked.size_m}),
            )
            .map_err(|e| e.to_string())?;
        Ok(format!("user:{}", r["id"].as_str().unwrap_or_default()))
    }

    /// Imported objects whose file is not held (a project opened without its bundle).
    pub fn missing_objects(&self) -> Vec<&visioncraft_core::UserObject> {
        let store = &self.scene.library.store;
        self.project()
            .objects
            .iter()
            .filter(|o| store.provided(&o.file).is_none())
            .collect()
    }

    /// Models in the project whose geometry is not loaded.
    pub fn missing_models(&self) -> Vec<&visioncraft_core::ModelRef> {
        self.project()
            .models
            .iter()
            .filter(|m| !self.loaded.contains_key(&m.id))
            .collect()
    }

    fn missing_model_boxes(&self) -> Vec<LineVertex> {
        let mut out = Vec::new();
        for m in self.missing_models() {
            if let Some([lo, hi]) = self.bounds_hint.get(&m.id) {
                out.extend(box_outline(
                    Vec3::from(*lo),
                    Vec3::from(*hi),
                    &model_matrix(m),
                    [255, 170, 60, 255],
                    2.0,
                ));
            }
        }
        out
    }

    /// The ray under a pointer position (points from the viewport's top-left).
    pub fn pointer_ray(&self, pos: glam::Vec2, size: glam::Vec2) -> (Vec3, Vec3) {
        let (v, p, _) = self.camera.matrices(size.x / size.y.max(1.0));
        ray_through(v, p, pos, size)
    }

    pub fn pick(&self, pos: glam::Vec2, size: glam::Vec2, shapes_only: bool) -> Option<Pick> {
        let (o, d) = self.pointer_ray(pos, size);
        self.scene.raycast(o, d, shapes_only)
    }

    pub fn is_top(&self) -> bool {
        self.camera.view == View::Top
    }

    pub fn world_matrix(&self, model: &str) -> Mat4 {
        self.project()
            .models
            .iter()
            .find(|m| m.id == model)
            .map_or(Mat4::IDENTITY, model_matrix)
    }
}

/// An asynchronous copy of the renderer's output to the CPU.
pub struct Readback {
    buffer: wgpu::Buffer,
    ready: Arc<Mutex<bool>>,
    pub size: [u32; 2],
    stride: u32,
}

impl Readback {
    /// Renders one frame and starts copying it out. Needs `prepare` to have been called.
    pub fn start(studio: &mut Studio, device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let [w, h] = studio.size;
        let mut encoder = device.create_command_encoder(&Default::default());
        studio.renderer.render(&mut encoder);
        let stride = (w * 4).next_multiple_of(256);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(stride * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            studio.renderer.output_texture().as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let ready = Arc::new(Mutex::new(false));
        let flag = ready.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |res| {
            if res.is_ok() {
                *flag.lock().expect("flag") = true;
            }
        });
        Self {
            buffer,
            ready,
            size: [w, h],
            stride,
        }
    }

    /// Tightly packed RGBA8, once the GPU is done.
    pub fn take(&self, device: &wgpu::Device) -> Option<Vec<u8>> {
        let _ = device.poll(wgpu::PollType::Poll);
        if !*self.ready.lock().expect("flag") {
            return None;
        }
        let mapped = self.buffer.slice(..).get_mapped_range().ok()?;
        let row = (self.size[0] * 4) as usize;
        Some(
            mapped
                .chunks(self.stride as usize)
                .flat_map(|r| &r[..row])
                .copied()
                .collect(),
        )
    }
}

/// RGBA8 rows as a PNG file.
pub fn png_bytes(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(rgba))
        .map_err(|e| format!("cannot encode the PNG: {e}"))?;
    Ok(out)
}

/// Seconds of loading Auto waits out before it judges the frames anyway.
const AUTO_LOAD_GRACE_S: f64 = 3.0;

#[derive(Debug, PartialEq)]
enum AutoStep {
    /// Too few frames yet.
    Wait,
    /// Inside the budget: keep the last 30 frames and look again.
    Keep,
    /// Change to this quality; true when it is a step down.
    To(Quality, bool),
}

/// Auto's decision from frame times (ms, oldest first): one rung down the renderer's ladder as
/// soon as 30 frames have their 75th percentile over the 30 fps budget, one up (the renderer's
/// `suggest_quality`, with its ULTRA headroom) after 90 frames with it under 65 % of it, when
/// `can_rise`.
fn auto_step(frames: &[f32], current: Quality, can_rise: bool) -> AutoStep {
    let n = frames.len();
    if n < 30 {
        return AutoStep::Wait;
    }
    let mut recent = frames[n - 30..].to_vec();
    recent.sort_by(f32::total_cmp);
    let p75 = recent[22];
    let budget = visioncraft_render::TARGET_MS;
    let next = if p75 > budget {
        // A history that misses the budget asks the ladder for the next rung down.
        visioncraft_render::suggest_quality(&[budget * 1.5; 30], current)
    } else if n >= 90 && can_rise && p75 < budget * 0.65 {
        visioncraft_render::suggest_quality(frames, current)
    } else if n >= 90 {
        return AutoStep::Keep;
    } else {
        return AutoStep::Wait;
    };
    if next == current {
        return AutoStep::Keep;
    }
    // A step up only when it is predicted to fit: the measured frames scaled by the renderer's
    // cost of the next quality over this one (preset cost table times pixels), with 15 % of the
    // budget to spare; a render scale step down within one preset only when 10 % over (a dead
    // band of its own). Without the prediction Auto rose to HIGH from MEDIUM's 60 fps, missed,
    // and went back, again and again.
    let cost =
        visioncraft_render::relative_cost(&next) / visioncraft_render::relative_cost(&current);
    let up = cost > 1.0;
    if up && p75 * cost > budget * 0.85 {
        return AutoStep::Keep;
    }
    if !up && preset_name(&next) == preset_name(&current) && p75 < budget * 1.1 {
        return AutoStep::Keep;
    }
    AutoStep::To(next, p75 > budget)
}

/// The preset a quality is (at any render scale), for messages.
pub fn preset_name(q: &Quality) -> &'static str {
    Quality::PRESETS
        .iter()
        .find(|(_, p)| {
            *p == Quality {
                render_scale: p.render_scale,
                ..*q
            }
        })
        .map_or("custom", |p| p.0)
}

/// Truecolour RGB, lossless: what a screenshot is sent as (no alpha, no palette).
pub fn png_rgb(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&rgb))
        .map_err(|e| format!("cannot encode the PNG: {e}"))?;
    Ok(out)
}

/// The project's lights as the renderer wants them.
pub fn render_lights(p: &Project) -> Vec<visioncraft_render::Light> {
    use visioncraft_core::{LightKind, kelvin_to_linear_rgb};
    p.lights
        .iter()
        .filter(|l| l.enabled && p.layer_visible(&l.layer))
        .map(|l| {
            let c = kelvin_to_linear_rgb(l.color_temperature_k);
            let m = c[0].max(c[1]).max(c[2]).max(1e-3);
            visioncraft_render::Light {
                position: Vec3::from(l.position),
                kind: match l.kind {
                    LightKind::Point => visioncraft_render::LightKind::Point,
                    LightKind::Spot {
                        direction,
                        inner_cone_deg,
                        outer_cone_deg,
                    } => visioncraft_render::LightKind::Spot {
                        direction: Vec3::from(direction).normalize_or_zero(),
                        inner: inner_cone_deg.to_radians(),
                        outer: outer_cone_deg.to_radians(),
                    },
                },
                color: Vec3::new(c[0] / m, c[1] / m, c[2] / m),
                intensity: l.lumens,
                range_m: l.range_m,
            }
        })
        .chain(lit_rooms(p))
        .collect()
}

/// A warm room lamp behind each lit window of the visible houses (`shape.set_lit_rooms`).
fn lit_rooms(p: &Project) -> impl Iterator<Item = visioncraft_render::Light> + '_ {
    let c = visioncraft_core::kelvin_to_linear_rgb(2700.0);
    let m = c[0].max(c[1]).max(c[2]).max(1e-3);
    p.shapes
        .iter()
        .filter_map(|s| match s {
            visioncraft_core::Shape::Extrusion(e) if e.closed && p.layer_visible(&e.layer) => e
                .house
                .as_ref()
                .map(|h| visioncraft_core::house::lit_windows(&e.outline, h, e.base)),
            _ => None,
        })
        .flatten()
        .map(move |at| visioncraft_render::Light {
            position: Vec3::from(at),
            kind: visioncraft_render::LightKind::Point,
            color: Vec3::new(c[0] / m, c[1] / m, c[2] / m),
            // A living room's lamps together (2000-3000 lm): at 1200 lm the windows read dim
            // against the dusk sky, where lit rooms are what a dusk picture is about.
            intensity: 2500.0,
            range_m: 3.5,
        })
}

#[cfg(test)]
mod tests {
    use super::auto_ev;

    #[test]
    fn looks_lift_the_day_and_leave_the_night() {
        use visioncraft_core::Look;
        assert_eq!(super::look_grade(Look::Neutral, 60.0), (0.0, 1.0, 1.0));
        let (lift, power, sat) = super::look_grade(Look::Natural, 60.0);
        assert!(lift > 0.5 && power > 1.0 && sat > 1.0);
        assert_eq!(super::look_grade(Look::Vivid, -5.0).0, 0.0);
    }

    #[test]
    fn exposure_falls_with_the_sun_and_lamps_keep_the_night_darker() {
        let mut last = f32::INFINITY;
        for e in (-15..=60).rev() {
            let ev = auto_ev(e as f32, false);
            assert!(ev <= last + 1e-6, "{e}");
            last = ev;
        }
        for e in [-10.0, -6.0, -2.0] {
            assert!(auto_ev(e, true) > auto_ev(e, false));
        }
        assert_eq!(auto_ev(20.0, true), auto_ev(20.0, false));
    }
}

/// How far shadows reach and how far the air stays clear, for the camera and the project's view
/// distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    pub shadow_m: f32,
    pub clear_m: f32,
    /// How far the impostors of shrubs and ground plants draw, as a share of their full reach.
    pub small_impostors: f32,
}

/// From an orbit the shadows reach the far side of what the camera looks at (twice its distance
/// to the pivot, at least the 80 m of eye level), in 20 m steps so zooming does not re-render
/// them every frame. The air is clear for 5 times the camera's height over the ground: an
/// overview from 50-100 m up sees the ground crisp for several hundred metres and the haze only
/// towards the horizon, eye level keeps its haze. Small plants' impostors draw
/// their full reach from 40 m up and a tenth of it at eye level, where the ground, trunks and
/// hedges hide them (a 0.6 m perennial still reaches about 45 m at MEDIUM, four times its old cull).
pub fn view_reach(camera: &crate::camera::Camera, ground_y: f32, view_distance: f32) -> Reach {
    let looked_at = match camera.view {
        crate::camera::View::Orbit => 2.0 * camera.dist,
        _ => 0.0,
    };
    let shadow = (looked_at.max(80.0) * view_distance).clamp(40.0, 1200.0);
    let height = (camera.eye().y - ground_y).max(0.0);
    Reach {
        shadow_m: (shadow / 20.0).round() * 20.0,
        clear_m: (5.0 * height).min(800.0) * view_distance,
        small_impostors: (height / 40.0).clamp(0.1, 1.0),
    }
}

#[cfg(test)]
mod reach_tests {
    use super::view_reach;
    use crate::camera::{Camera, View};

    #[test]
    fn an_overview_reaches_further_and_stays_clear_eye_level_keeps_its_haze() {
        let mut c = Camera::default();
        c.view = View::Orbit;
        c.dist = 120.0;
        let far = view_reach(&c, 0.0, 1.0);
        assert!(far.shadow_m >= 240.0, "{far:?}");
        assert!(far.clear_m > 100.0, "{far:?}");
        assert_eq!(view_reach(&c, 0.0, 2.0).shadow_m, 2.0 * far.shadow_m);
        c.view = View::Walk;
        c.walk.eye.y = 1.7;
        let eye = view_reach(&c, 0.0, 1.0);
        assert_eq!(eye.shadow_m, 80.0);
        assert!(eye.clear_m < 10.0, "{eye:?}");
        assert!(eye.small_impostors < far.small_impostors);
    }

    #[test]
    fn screenshots_are_lossless_truecolour() {
        // Every one of 4096 colours survives (a palette PNG would keep 256 at most).
        let rgba: Vec<u8> = (0..64u32 * 64)
            .flat_map(|i| {
                [
                    (i % 64 * 4) as u8,
                    (i / 64 * 4) as u8,
                    (i % 7 * 30) as u8,
                    255,
                ]
            })
            .collect();
        let png = super::png_rgb(&rgba, 64, 64).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        assert_eq!(reader.info().color_type, png::ColorType::Rgb);
        assert_eq!(reader.info().bit_depth, png::BitDepth::Eight);
        let mut out = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut out).unwrap();
        let back: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        assert_eq!(out, back);
    }

    #[test]
    fn auto_steps_down_at_once_and_up_only_when_well_inside_the_budget() {
        use super::{AutoStep, auto_step};
        use visioncraft_render::Quality;
        let high = Quality::HIGH;
        // 28 fps with hitches: under the old 110 % median rule this stayed; now it steps down.
        let mut frames = vec![34.0; 20];
        frames.extend([45.0; 10]);
        assert!(matches!(
            auto_step(&frames, high, true),
            AutoStep::To(_, true)
        ));
        // Fast but too few frames, or held after a step down: no rise.
        assert_eq!(
            auto_step(&[10.0; 60], Quality::MEDIUM, true),
            AutoStep::Wait
        );
        assert_eq!(
            auto_step(&[10.0; 90], Quality::MEDIUM, false),
            AutoStep::Keep
        );
        assert!(matches!(
            auto_step(&[10.0; 90], Quality::MEDIUM, true),
            AutoStep::To(_, false)
        ));
        // Inside the band: keep.
        assert_eq!(
            auto_step(&[28.0; 90], Quality::MEDIUM, true),
            AutoStep::Keep
        );
        // Render scale within LOW: 21 ms at 55 % would be 29 ms at 65 % (over 85 % of the
        // budget): no step up; 35 ms (5 % over) at 65 %: no step down to 55 % either.
        let low = |s: f32| Quality {
            render_scale: s,
            ..Quality::LOW
        };
        assert_eq!(auto_step(&[21.0; 90], low(0.55), true), AutoStep::Keep);
        // MEDIUM at 60 fps (16.7 ms): HIGH at 80 % costs about 2.1 times as much, 35 ms, over
        // the budget: stay (the woodland's see-saw).
        assert_eq!(
            auto_step(&[16.7; 90], Quality::MEDIUM, true),
            AutoStep::Keep
        );
        assert_eq!(auto_step(&[35.0; 90], low(0.65), true), AutoStep::Keep);
        assert!(matches!(
            auto_step(&[40.0; 30], low(0.65), true),
            AutoStep::To(_, true)
        ));
    }
}

/// How long a picture waits for plants and library files before it is taken anyway.
pub const PICTURE_WAIT_S: f64 = 120.0;

/// What a picture taken with `pending` plants still being made, or the library still loading,
/// says about it.
pub fn picture_problems(pending: usize, library_idle: bool) -> Vec<String> {
    let mut out = Vec::new();
    if pending > 0 {
        out.push(format!(
            "{pending} plant generations were not finished after {PICTURE_WAIT_S:.0} s: those plants show as stand-ins"
        ));
    }
    if !library_idle {
        out.push(format!(
            "library files were still loading after {PICTURE_WAIT_S:.0} s: some materials or objects show their stand-ins"
        ));
    }
    out
}

#[cfg(test)]
mod picture_tests {
    #[test]
    fn a_picture_with_plants_pending_says_so() {
        assert!(super::picture_problems(0, true).is_empty());
        let p = super::picture_problems(7, true);
        assert_eq!(p.len(), 1);
        assert!(p[0].starts_with("7 plant generations"));
        assert_eq!(super::picture_problems(0, false).len(), 1);
    }
}
