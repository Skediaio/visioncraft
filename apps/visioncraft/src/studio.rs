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
    auto_quality: Quality,
    pub frame_ms: Vec<f32>,
    /// UTC offset follows the EU summer-time rule for the chosen date.
    pub auto_dst: bool,
    pub base_offset: f64,
    /// Exposure offset in stops on top of the automatic exposure: positive is brighter.
    pub exposure: f32,
    pub draft: Option<Draft>,
    pub time_draft: Option<SunTime>,
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
    pub lights_on: bool,
    /// The exposure (EV) and sun elevation of the last frame, for the measurements.
    pub ev_used: (f32, f32),
    /// The lights' markers show while the Lights tool is open.
    pub light_markers: bool,
    /// The sun has set while the lights are off and the user has not been told yet.
    pub suggest_lights: bool,
    suggested: bool,
    pub dropped_lights: usize,
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
    pub size: [u32; 2],
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

/// Automatic exposure from the sun: noon 15, sun at 10 degrees 13.5, sunset 10.5, then down with
/// the twilight sky's light (about 2.5 EV per 3 degrees in the renderer's sky) so the garden
/// stays readable as a photographer's "blue hour" picture would; `lamps` keeps it about one stop
/// lower after sunset so that garden lights glow instead of washing out. Linear between rows.
pub fn auto_ev(elevation_deg: f32, lamps: bool) -> f32 {
    const DAY: [(f32, f32); 5] = [
        (-12.0, 1.5),
        (-8.0, 2.8),
        (-4.0, 6.0),
        (0.0, 10.5),
        (10.0, 13.5),
    ];
    const LAMPS: [(f32, f32); 5] = [
        (-12.0, 1.0),
        (-8.0, 2.0),
        (-4.0, 5.0),
        (0.0, 10.0),
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
            auto_dst: true,
            base_offset: 1.0,
            exposure: 0.0,
            draft: None,
            time_draft: None,
            site_draft: None,
            highlight: None,
            pointer_captured: false,
            double_click_captured: false,
            clock: 0.0,
            people: Vec::new(),
            show_people: true,
            placing_person: false,
            lights_on: false,
            ev_used: (0.0, 0.0),
            light_markers: false,
            suggest_lights: false,
            suggested: false,
            dropped_lights: 0,
            lamps_reconciled: (u64::MAX, false),
            lamps_skipped: 0,
            lamp_problems: Vec::new(),
            sky: Default::default(),
            thumbs: Default::default(),
            last_lights: Vec::new(),
            extra: (Vec::new(), Vec::new()),
            size: [0, 0],
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
            None => {}
        }
        p
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
        let lamps = self.lights_on && self.project().lights.iter().any(|l| l.enabled);
        let ev = v
            .exposure_ev
            .unwrap_or_else(|| self.sky.auto_ev(elevation, lamps))
            - self.exposure;
        self.ev_used = (ev, elevation);
        self.renderer.set_exposure(ev);
        self.renderer.set_clay(self.project().style.clay);
        // Below the horizon the haze only scatters the dim, pink twilight sky back at the camera
        // and washes the garden out: thin it as the sun sinks, and no sun glow.
        let night = (-elevation / 6.0).clamp(0.0, 1.0);
        self.renderer
            .set_atmosphere(&visioncraft_render::Atmosphere {
                haze: v.haze * (1.0 - 0.7 * night),
                sun_glow: 0.5 * (1.0 - night),
                ..Default::default()
            });
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
        let lights = if self.lights_on {
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

    /// Feeds one frame time (ms); every 45 frames of continuous drawing Auto may change the quality.
    pub fn record_frame(&mut self, ms: f32) {
        if !self.quality_auto {
            self.frame_ms.clear();
            return;
        }
        self.frame_ms.push(ms);
        if self.frame_ms.len() >= 45 {
            let next = visioncraft_render::suggest_quality(&self.frame_ms, self.auto_quality);
            if next != self.auto_quality {
                self.auto_quality = next;
            }
            self.frame_ms.clear();
        }
    }

    /// Plants still being generated.
    pub fn plants_pending(&self) -> usize {
        self.scene.flora.pending()
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
        self.scene.library.lamps_on = self.lights_on;
        let eye = self.camera.eye();
        if self.scene.pump(&mut self.renderer, eye) {
            self.draft_serial += 1;
        }
        self.take_catalog();
        self.sync();
        let (view, proj, eye) = self.camera.matrices(self.aspect());
        self.renderer.set_camera(view, proj, eye);
        let (sun, elevation) = self.sun();
        self.renderer.set_sun(&sun);
        self.apply_view(elevation);
        crate::sky_panel::apply(self);
        if elevation < 0.0
            && !self.lights_on
            && !self.suggested
            && !self.project().lights.is_empty()
        {
            self.suggested = true;
            self.suggest_lights = true;
        }
        let q = self.current_quality();
        self.renderer.set_quality(q);
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
        if let Some(c) = lib.catalog.clone() {
            for a in &c.assets {
                if !self.assets.iter().any(|x| x.id == a.id) {
                    self.assets.push(a.clone());
                }
            }
        }
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::auto_ev;

    #[test]
    fn exposure_falls_with_the_sun_and_lamps_keep_it_lower_after_sunset() {
        let mut last = f32::INFINITY;
        for e in (-15..=60).rev() {
            let ev = auto_ev(e as f32, false);
            assert!(ev <= last + 1e-6, "{e}");
            last = ev;
        }
        for e in [-10.0, -6.0, -2.0] {
            assert!(auto_ev(e, true) < auto_ev(e, false));
        }
        assert_eq!(auto_ev(20.0, true), auto_ev(20.0, false));
    }
}
