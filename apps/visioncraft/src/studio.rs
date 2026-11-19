//! Everything that is not a window: the session, the loaded geometry, the scene and the camera
//! over a renderer. The egui app and the headless smoke mode both drive this.

use glam::{Mat4, Vec3};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use visioncraft_automation::PhotoRequest;
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

/// What `Studio::load_sources` did.
#[derive(Debug, Default)]
pub struct SourcesLoaded {
    /// Models read from a file, with the file (to watch it).
    pub files: Vec<(String, PathBuf)>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// The file of a model's `source`, natively; the first that reads wins: the stored path as it
/// is, relative to the project's folder, the same file name in the project's folder, the same
/// file name in the folder a model was last relinked from.
#[cfg(not(target_arch = "wasm32"))]
pub fn find_model_file(
    source: &str,
    project_dir: Option<&Path>,
    relink_dir: Option<&Path>,
) -> Option<(PathBuf, Vec<u8>)> {
    let stored = Path::new(source);
    // A path saved on another system may use the other separator.
    let name = source.rsplit(['/', '\\']).next().filter(|n| !n.is_empty());
    [
        Some(stored.to_path_buf()),
        project_dir.map(|d| d.join(stored)),
        project_dir.zip(name).map(|(d, n)| d.join(n)),
        relink_dir.zip(name).map(|(d, n)| d.join(n)),
    ]
    .into_iter()
    .flatten()
    .find_map(|p| std::fs::read(&p).ok().map(|b| (p, b)))
}

/// A page sees no folders.
#[cfg(target_arch = "wasm32")]
pub fn find_model_file(
    _source: &str,
    _project_dir: Option<&Path>,
    _relink_dir: Option<&Path>,
) -> Option<(PathBuf, Vec<u8>)> {
    None
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
    /// How much of the lamps' light is on, 0..1 (a film's `lights`; 1 all of it).
    pub lamp_level: f32,
    /// A second colour grade mixed over the project's: the grade and its share, 0..1 (a film's
    /// `grade` span).
    pub grade_mix: Option<(visioncraft_core::Grade, f32)>,
    /// The weather drawn and the clock it was drawn at: eased towards the project's over its
    /// `weather_transition_s` (`shown_weather`). `None`: the project's at once (a picture).
    pub weather_shown: Option<(Weather, f64)>,
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
    /// What extensions asked the window to do (open a project, an example); the window does
    /// them after the frame.
    pub(crate) requests: Vec<crate::extension::Request>,
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
    bounce_cell: Option<glam::IVec4>,
    /// A hash of the triangles the sky visibility volume was last made from, the volume, and
    /// the lamps' light passed round its rooms (per sample, lux).
    sky_volume_key: u64,
    sky_volume: Option<visioncraft_core::SkyVolume>,
    sky_task: Option<crate::sky_task::SkyTask>,
    /// Examples' models being imported in the import worker (the web: on the page they froze
    /// it for seconds), by model id; `wake` repaints when one is done (the window sets it).
    #[cfg(target_arch = "wasm32")]
    example_imports: Vec<(String, crate::import_task::ImportTask)>,
    #[cfg(target_arch = "wasm32")]
    pub wake: eframe::egui::Context,
    /// Empty when to be made again (`upload_sky_volume`).
    lamp_bounce: Vec<[f32; 3]>,
    /// The sun's light passed round the rooms per unit of its illuminance (empty: to be made
    /// again), the direction it was made for, and the sun's illuminance (lux) it was sent with.
    sun_bounce: Vec<f32>,
    sun_bounce_dir: Vec3,
    sun_lux: Vec3,
    /// The room shift (EV, `meter_shift`) the eye has adapted to and when (`clock`); `None`:
    /// take the metered shift at once (the first frame, a picture).
    adapted_ev: Option<f32>,
    adapted_at: f64,
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
    /// What the renderer draws of the plan underlays, to change it only when it differs.
    underlay_quads: Vec<visioncraft_render::UnderlayQuad>,
    /// The sun-hours map on the ground (`view.set_sun_map`).
    pub sun_map: crate::sun_map::SunMapState,
    /// The baked sky visibility (`view.set sky_occlusion`) and what the last check saw: the
    /// revision, the loaded models, the years and the month.
    pub sky_vis: crate::sky_vis::SkyVisState,
    sky_vis_checked: Option<(u64, u64, Option<u32>, u32)>,
    /// A bake waits for the plants and models still on their way (`sync_sky_visibility`).
    sky_vis_waiting: bool,
    /// The plant light check and its marks on the plan.
    pub light: crate::light_ui::LightState,
    /// An opacity being dragged on a slider, shown before it is committed: (underlay id, value).
    pub underlay_live: Option<(String, f32)>,
    /// The plan panel asked for an image file; the window opens the picker.
    pub pick_underlay: bool,
    /// The materials panel asked for a photo for this face (a `wall_photo.add` target); the
    /// window opens the picker and then the straightening window.
    pub wall_photo_for: Option<serde_json::Value>,
    /// With `wall_photo_for`: the picker is to open now.
    pub pick_wall_photo: bool,
    /// The materials panel asked to straighten this wall photo again (its id).
    pub wall_photo_edit: Option<String>,
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
    /// The distance the last photo was focused at (m): its autofocus' or the one set.
    pub photo_focus_m: Option<f32>,
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

/// The night sky's light on open ground (lux) that the renderer always adds: its night floor
/// (`visioncraft_render::sky::NIGHT`, about 0.018 cd/m2, times pi), plus starlight.
const NIGHT_SKY_LUX: f64 = 0.0575 + visioncraft_core::sun::STARLIGHT_LUX;

/// Automatic exposure (EV100, lower is brighter). By day as a photographer sets it: noon 15,
/// 13.5 with the sun at 10 degrees. Lower, it follows the light on open ground, as an incident
/// light meter reads it (ISO 2720, C = 250: EV = log2(E / 2.5)): the sun and clear sky
/// (`daylight_lux`, USNO Circular 171), `moon_lux`, and the night sky. A picture of dusk or
/// night is kept darker than the meter's mid grey, more so the less light there is, as the
/// eye's adaptation key falls with the light (Krawczyk, Myszkowski & Seidel, "Lightness
/// Perception in Tone Reproduction for High Dynamic Range Images", Eurographics 2005; here a
/// fit by eye): 1 stop at sunset, 2 at the end of civil twilight, 2.6 under a full moon, about
/// 3.4 on a moonless night. So dusk keeps the blue hour's soft light on everything and a moonlit
/// night reads blue-grey. With `lamps` the blue hour is exposed 2 EV darker (the sky keeps a
/// deep blue, windows and lamp pools glow) and the night 0.8 EV. Never brighter than EV -2.5.
pub fn auto_ev(elevation_deg: f32, moon_lux: f32, lamps: bool) -> f32 {
    let e = elevation_deg;
    if e >= 45.0 {
        return 15.0;
    }
    if e >= 10.0 {
        return 13.5 + 1.5 * (e - 10.0) / 35.0;
    }
    let lux = visioncraft_core::sun::daylight_lux(f64::from(e))
        + f64::from(moon_lux.max(0.0))
        + NIGHT_SKY_LUX;
    let metered = (lux / 2.5).log2() as f32;
    // Stops darker than the meter, by log10(lux): sun at 10 degrees (11 klx, joining the day
    // curve), sunset, sun at -4, at -6, full moon, moonless.
    const DARKER: [(f32, f32); 6] = [
        (-1.5, 3.6),
        (-0.5, 2.6),
        (0.48, 2.0),
        (1.39, 1.5),
        (2.88, 1.0),
        (4.045, 1.39),
    ];
    let l = (lux.log10() as f32).clamp(DARKER[0].0, DARKER[5].0);
    let darker = DARKER
        .windows(2)
        .find(|w| l <= w[1].0)
        .map_or(DARKER[5].1, |w| {
            w[0].1 + (w[1].1 - w[0].1) * (l - w[0].0) / (w[1].0 - w[0].0)
        });
    // With lamps a photographer exposes the blue hour a stop and a half under the meter so the
    // sky keeps its colour and the lamps glow; in full night the pools carry the picture anyway.
    let lamp = if lamps {
        let dusk = ((2.0 - e) / 4.0).clamp(0.0, 1.0);
        let night = ((-e - 9.0) / 3.0).clamp(0.0, 1.0);
        dusk * (2.0 - 1.2 * night)
    } else {
        0.0
    };
    (metered + darker + lamp).max(-2.5)
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
            lamp_level: 1.0,
            grade_mix: None,
            weather_shown: None,
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
            requests: Vec::new(),
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
            sky_volume_key: 0,
            sky_volume: None,
            sky_task: None,
            #[cfg(target_arch = "wasm32")]
            example_imports: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            wake: Default::default(),
            lamp_bounce: Vec::new(),
            sun_bounce: Vec::new(),
            sun_bounce_dir: Vec3::ZERO,
            sun_lux: Vec3::ZERO,
            adapted_ev: None,
            adapted_at: 0.0,
            impostors_reserved: false,
            lamps_reconciled: (u64::MAX, false),
            lamps_skipped: 0,
            lamp_problems: Vec::new(),
            sky: Default::default(),
            thumbs: Default::default(),
            last_lights: Vec::new(),
            extra: (Vec::new(), Vec::new()),
            underlay_quads: Vec::new(),
            sun_map: Default::default(),
            sky_vis: Default::default(),
            sky_vis_checked: None,
            sky_vis_waiting: false,
            light: crate::light_ui::LightState::new(),
            underlay_live: None,
            pick_underlay: false,
            wall_photo_for: None,
            pick_wall_photo: false,
            wall_photo_edit: None,
            overlays_in_pictures: false,
            targets_changed: false,
            size: [0, 0],
            shadow_reach: 0.0,
            draft_serial: 0,
            version: 0,
            synced: None,
            problems: Vec::new(),
            photo_focus_m: None,
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
        self.drop_sun_map();
        self.lamps_reconciled = (u64::MAX, false);
        // A new session counts its revisions from zero again: the scene must look at it anew.
        self.synced = None;
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
        self.drop_sun_map();
        self.scene.clear(&mut self.renderer);
        self.scene.flora.sweep();
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
            // A quality step in this frame (to LOW and back: tufts or GPU grass) asks for the
            // scene again (`synced` cleared): that sync starts the lawns' work, so wait for it.
            if self.synced.is_some()
                && self.plants_pending() == 0
                && self.scene.library.idle()
                && !self.sky_volume_pending()
                && !self.sky_visibility_pending()
                && !self.sun_map.pending()
            {
                break;
            }
            if crate::assets::now_ms() - start > PICTURE_WAIT_S * 1000.0 {
                break;
            }
            #[cfg(not(target_arch = "wasm32"))]
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.scene.library.store.stream_bytes = stream;
        // A picture is taken with the eye adapted to it.
        self.adapted_ev = None;
        self.prepare();
        let mut problems = picture_problems(
            self.plants_pending(),
            self.scene.library.idle(),
            &self.scene.flora.waiting_names(5),
            &self.scene.library.store.loading_names(5),
        );
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

    /// A photo of the 3D view at `w` x `h` at `quality`: `photo.samples` frames averaged (soft
    /// shadows, smooth edges, smooth occlusion), with its lens (depth of field) and shutter
    /// (motion blur). RGBA8. Blocks until done.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_photo(
        &mut self,
        w: u32,
        h: u32,
        quality: Quality,
        photo: &PhotoRequest,
    ) -> Result<Vec<u8>, String> {
        let live = self.photo_prepare(w, h);
        let camera = self.camera.matrices(w as f32 / h as f32);
        let lens = self.photo_lens(photo, w, h);
        let result = (|| {
            let mut shot = self.renderer.photo_begin(w, h, quality, camera, lens)?;
            while shot.done < photo.samples.max(1) {
                if !self.renderer.photo_sample(&mut shot)? {
                    self.renderer.wait_gpu()?;
                }
            }
            self.photo_focus_m = shot.focus_m();
            self.renderer.photo_finish(shot)?.wait()
        })();
        self.photo_end(live);
        result
    }

    /// The renderer's lens for `photo` at `w` x `h`: the aperture's radius from the f-number
    /// with the focal length a full-frame sensor needs for the camera's field of view (or the
    /// radius given), the focus (`target`: the distance to the camera's pivot) and the shutter.
    pub fn photo_lens(&self, photo: &PhotoRequest, w: u32, h: u32) -> visioncraft_render::Lens {
        use visioncraft_core::photo::{Focus, aperture_radius_m, focal_length_m};
        let aspect = w as f32 / h.max(1) as f32;
        let radius_m = match photo.f_number {
            _ if photo.aperture_m > 0.0 => photo.aperture_m,
            Some(n) => aperture_radius_m(focal_length_m(self.camera.fov_y, aspect), n),
            None => 0.0,
        };
        let pivot = (self.camera.eye() - self.camera.pivot).length().max(0.1);
        let focus = match photo.focus {
            Focus::Target => visioncraft_render::Focus::Metres(pivot),
            Focus::Metres(m) => visioncraft_render::Focus::Metres(m),
            Focus::Auto => visioncraft_render::Focus::Auto(photo.focus_point),
        };
        visioncraft_render::Lens {
            radius_m,
            focus,
            shutter_s: photo.shutter_s,
        }
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

    /// Opens `project` as the window's Open does: replaces the project and loads its models
    /// (`load_sources`).
    pub fn open(
        &mut self,
        project: Project,
        project_dir: Option<&Path>,
        relink_dir: Option<&Path>,
    ) -> SourcesLoaded {
        crate::load_stats::time("~open: replace project", || self.replace_project(project));
        crate::load_stats::time("~open: import models", || {
            self.load_sources(project_dir, relink_dir)
        })
    }

    /// Loads every model that is not loaded yet: an example's file from inside the program,
    /// else (natively) the first file `find_model_file` finds. Models whose file is not found
    /// stay missing, without a message (the relink bar asks for them).
    pub fn load_sources(
        &mut self,
        project_dir: Option<&Path>,
        relink_dir: Option<&Path>,
    ) -> SourcesLoaded {
        let mut out = SourcesLoaded::default();
        for m in self.project().models.clone() {
            if self.loaded.contains_key(&m.id) {
                continue;
            }
            let found = match visioncraft_core::example_file(&m.source) {
                Some(bytes) => Some((None, Arc::from(bytes))),
                None if visioncraft_core::is_example_source(&m.source) => {
                    out.errors.push(format!(
                        "{}: `{}` is not in this version of VisionCraft (the file was made by a newer one)",
                        m.name, m.source
                    ));
                    None
                }
                None => find_model_file(&m.source, project_dir, relink_dir)
                    .map(|(p, b)| (Some(p), Arc::from(b))),
            };
            let Some((path, bytes)) = found else {
                continue;
            };
            match self.reimport(
                &m.id,
                Source {
                    bytes,
                    path: path.clone(),
                },
            ) {
                Ok((w, _)) => {
                    out.warnings.extend(w);
                    out.files.extend(path.map(|p| (m.id.clone(), p)));
                }
                Err(e) => out.errors.push(format!("{}: {e}", m.name)),
            }
        }
        out
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
        // A new session (an agent's `file.new` or `file.open`; a session's revision only grows):
        // another project, as `replace_project`. Not natively, where an agent's picture of
        // another project swaps the session in for a moment (`agents`, `render`).
        if cfg!(target_arch = "wasm32") && self.synced.is_some_and(|(rev, _)| rev > key.0) {
            self.scene.clear(&mut self.renderer);
            self.scene.flora.sweep();
        }
        // An example's model travels inside the program (`example:<file>`): load it here, on the
        // web too, whoever added it.
        let examples: Vec<(String, &'static [u8])> = self
            .session
            .project()
            .models
            .iter()
            .filter(|m| !self.loaded.contains_key(&m.id))
            .filter_map(|m| Some((m.id.clone(), visioncraft_core::example_file(&m.source)?)))
            .collect();
        #[cfg(target_arch = "wasm32")]
        for (id, bytes) in examples {
            if self.example_imports.iter().any(|(i, _)| *i == id) {
                continue;
            }
            let Some(kind) = self.model_kind(&id) else {
                continue;
            };
            let task = crate::import_task::ImportTask::start(kind, bytes.into(), self.wake.clone());
            self.example_imports.push((id, task));
        }
        #[cfg(not(target_arch = "wasm32"))]
        for (id, bytes) in examples {
            if let Err(e) = self.reimport(
                &id,
                Source {
                    bytes: Arc::from(bytes),
                    path: None,
                },
            ) {
                self.problems.push(format!("{id}: {e}"));
            }
        }
        self.synced = Some(key);
        let project = self.project_view().into_owned();
        let problems = crate::load_stats::time("scene sync (main thread)", || {
            self.scene
                .sync(&mut self.renderer, &project, &self.loaded, &self.assets)
        });
        self.problems.extend(problems);
        crate::load_stats::time("sky volume (main thread)", || {
            self.update_sky_volume(&project)
        });
    }

    /// Remakes the sky visibility volume when the buildings that block the sky changed.
    fn update_sky_volume(&mut self, project: &Project) {
        use std::hash::{Hash, Hasher};
        let triangles = self.scene.sky_occluders(project);
        // With sky occlusion on, the baked sky visibility (`sync_sky_visibility`) has the crowns,
        // each by its species' density: the room volume keeps to the buildings, so a crown is
        // not counted twice.
        let crowns = if project.view.sky_occlusion.unwrap_or(true) {
            Vec::new()
        } else {
            self.scene.sky_crowns()
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for t in &triangles {
            t.as_flattened()
                .iter()
                .for_each(|v| v.to_bits().hash(&mut h));
        }
        crate::sky_task::encode_crowns(&crowns)
            .iter()
            .for_each(|v| v.to_bits().hash(&mut h));
        let key = h.finish() | 1;
        if key == self.sky_volume_key {
            return;
        }
        self.sky_volume_key = key;
        // The old volume stays until the new one is made (`poll_sky_volume`).
        let to_sun = self.sun_bounce_dir.to_array();
        self.sky_task = Some(crate::sky_task::SkyTask::start(
            triangles, crowns, key, to_sun,
        ));
    }

    /// Takes the sky volume once it is made (`update_sky_volume`).
    fn poll_sky_volume(&mut self) {
        let Some(task) = self.sky_task.as_ref() else {
            return;
        };
        if let Some(made) = task.take() {
            // The sun bounce came with it, for the sun when it started: kept if it still stands.
            let same_sun = task.to_sun == self.sun_bounce_dir.to_array();
            self.sky_task = None;
            self.lamp_bounce.clear();
            self.sun_bounce.clear();
            self.sky_volume = made.map(|(v, bounce)| {
                if same_sun {
                    self.sun_bounce = bounce;
                }
                v
            });
            self.upload_sky_volume();
        }
    }

    /// A sky volume is being made.
    pub fn sky_volume_pending(&self) -> bool {
        self.sky_task.is_some()
    }

    /// Examples' models still being imported (the web; natively they load in `sync`).
    pub fn models_pending(&self) -> usize {
        #[cfg(target_arch = "wasm32")]
        return self.example_imports.len();
        #[cfg(not(target_arch = "wasm32"))]
        0
    }

    /// Installs the examples' models the import worker has finished (`sync`); one that is no
    /// longer in the project (another file opened meanwhile) is dropped.
    #[cfg(target_arch = "wasm32")]
    fn poll_example_imports(&mut self) {
        let mut done = Vec::new();
        self.example_imports.retain(|(id, task)| match task.take() {
            Some(r) => {
                done.push((id.clone(), r));
                false
            }
            None => true,
        });
        for (id, result) in done {
            let still = self.project().models.iter().find(|m| m.id == id).cloned();
            let Some(m) = still.filter(|_| !self.loaded.contains_key(&id)) else {
                continue;
            };
            let Some(bytes) = visioncraft_core::example_file(&m.source) else {
                continue;
            };
            let source = Source {
                bytes: Arc::from(bytes),
                path: None,
            };
            if let Err(e) = result.and_then(|model| self.reimport_imported(&id, model, source)) {
                self.problems.push(format!("{id}: {e}"));
            }
        }
    }

    /// Sends the sky volume to the renderer with the light of the lamps now on and of the sun
    /// passed round its rooms (`SkyVolume::lamp_bounce`, `SkyVolume::sun_bounce`), each made
    /// again when emptied.
    fn upload_sky_volume(&mut self) {
        let Some(v) = &self.sky_volume else {
            self.lamp_bounce.clear();
            self.sun_bounce.clear();
            self.renderer.set_sky_volume(None);
            return;
        };
        if self.sun_bounce.len() != v.samples.len() {
            self.sun_bounce = v.sun_bounce(self.sun_bounce_dir.to_array());
        }
        if self.lamp_bounce.len() == v.samples.len() {
            return self.send_sky_volume();
        }
        let lamps: Vec<visioncraft_core::Lamp> = self
            .last_lights
            .iter()
            .map(|l| visioncraft_core::Lamp {
                position: l.position.to_array(),
                color: l.color.to_array(),
                lumens: l.intensity,
                direction: match l.kind {
                    visioncraft_render::LightKind::Spot { direction, .. } => {
                        Some(direction.to_array())
                    }
                    visioncraft_render::LightKind::Point => None,
                },
            })
            .collect();
        self.lamp_bounce = v.lamp_bounce(&lamps);
        self.send_sky_volume();
    }

    /// The volume to the renderer, the room light being the lamps' and the sun's (lux) and the
    /// sky's share (`upload_sky_volume`).
    fn send_sky_volume(&mut self) {
        let Some(v) = &self.sky_volume else { return };
        let sun = self.sun_lux.to_array();
        let (occupancy, voxels) = v.occupancy();
        let texels = v.room_texels();
        let room: Vec<[f32; 4]> = (0..v.samples.len())
            .map(|i| {
                let (l, s) = (self.lamp_bounce[i], self.sun_bounce[i]);
                [
                    l[0] + s * sun[0],
                    l[1] + s * sun[1],
                    l[2] + s * sun[2],
                    texels[i],
                ]
            })
            .collect();
        self.renderer.set_sky_volume(Some((
            v.origin,
            v.cell,
            v.dims,
            &v.samples,
            &room,
            (&occupancy, voxels),
        )));
    }

    /// The illuminance (lux) the lamps now on throw straight onto a surface at `q` facing `n`,
    /// those the sky volume says see it, as the renderer lights it (`local_lights`).
    fn lamp_direct(&self, v: &visioncraft_core::SkyVolume, q: Vec3, n: Vec3) -> f32 {
        let mut e = 0.0;
        for l in &self.last_lights {
            let to = l.position - q;
            let dist2 = to.length_squared().max(1e-4);
            let range = l.range_m.max(0.01);
            let cos = n.dot(to) / dist2.sqrt();
            if cos <= 0.0 || dist2 > range * range || !v.sees(q.to_array(), l.position.to_array()) {
                continue;
            }
            let cone = match l.kind {
                visioncraft_render::LightKind::Spot {
                    direction,
                    inner,
                    outer,
                } => {
                    let (ci, co) = (inner.min(outer).cos(), outer.cos());
                    let c = ((-to.normalize()).dot(direction) - co) / (ci - co).max(1e-4);
                    c.clamp(0.0, 1.0).powi(2)
                }
                visioncraft_render::LightKind::Point => 1.0,
            };
            let window = (1.0 - (dist2 / (range * range)).powi(2))
                .clamp(0.0, 1.0)
                .powi(2);
            e += l.candela() * luminance(l.color.to_array()) * cos * cone * window / dist2;
        }
        e
    }

    /// A light meter over what the camera sees: rays through a grid over the picture, each
    /// to the first occluder of the sky volume, where the light falling on a surface facing
    /// the camera is estimated: the sky's (`open_lux` under the open sky) as far as it sees
    /// the sky, the room's bounced daylight and lamplight for the rest, and the lamps that see
    /// the point. A ray that leaves the volume meets the open air. Returns the mean
    /// illuminance and the mean share of the surfaces' view that is room, not sky (0 out of
    /// doors, near 1 in a room); `None` without a volume.
    fn meter_view(&self, open_lux: f32) -> Option<(f32, f32)> {
        let v = self.sky_volume.as_ref()?;
        let (view, proj, _) = self.camera.matrices(self.aspect());
        let size = glam::Vec2::new(self.size[0] as f32, self.size[1] as f32).max(glam::Vec2::ONE);
        let (mut sum, mut room, mut n) = (0.0f32, 0.0f32, 0.0f32);
        let sun = luminance(self.sun_lux.to_array());
        for j in 0..METER_ROWS {
            for i in 0..METER_COLUMNS {
                let at = glam::Vec2::new(
                    (i as f32 + 0.5) / METER_COLUMNS as f32,
                    (j as f32 + 0.5) / METER_ROWS as f32,
                );
                let (o, d) = ray_through(view, proj, at * size, size);
                n += 1.0;
                let hit = v.trace(o.to_array(), d.to_array(), 60.0);
                let Some((q, k)) = hit.and_then(|t| {
                    let q = o + d * (t - 0.05);
                    Some((q, v.index(q.to_array(), (-d).to_array())?))
                }) else {
                    sum += open_lux;
                    continue;
                };
                let seen = v.seen(k, (-d).to_array());
                let lamp = luminance(self.lamp_bounce.get(k).copied().unwrap_or_default());
                let bounced = lamp + sun * self.sun_bounce.get(k).copied().unwrap_or_default();
                let direct = self.lamp_direct(v, q, -d);
                sum += meter_point(open_lux, seen, v.room[k], bounced, direct);
                room += 1.0 - seen;
            }
        }
        Some((sum / n, room / n))
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
        let site = self.site();
        let pos = sun_position(&site, &self.time());
        (
            Sun::from_angles(
                site.bearing_to_drawing_deg(pos.azimuth_deg) as f32,
                pos.elevation_deg as f32,
            ),
            pos.elevation_deg as f32,
        )
    }

    /// The moon, when it lights the scene more than the sun's twilight does: its light as a sun
    /// (the renderer's one directional light, so it casts shadows) and the sky scale that turns
    /// the daylight sky into the moonlit one (`Renderer::set_sky_scale`). `None` by day, with
    /// the moon down, or under an image sky.
    /// shortcut: the light switches from the twilight sky to the moon in one frame where the two
    /// are equal (sun about 8 degrees down under a full moon): the sky loses half its light and
    /// changes its pattern; blend the two skies if a time-lapse shows the step.
    pub fn moonlight(&self) -> Option<(Sun, f32)> {
        if self.sky.sun_override().is_some() {
            return None;
        }
        let (site, time) = (self.site(), self.time());
        let sun = sun_position(&site, &time);
        let moon = visioncraft_core::sun::moon_position(&site, &time);
        if sun.elevation_deg >= 0.0
            || moon.elevation_deg <= 0.0
            || moon.illuminance_lux <= visioncraft_core::sun::daylight_lux(sun.elevation_deg)
        {
            return None;
        }
        let mut light = Sun::from_angles(
            site.bearing_to_drawing_deg(moon.azimuth_deg) as f32,
            moon.elevation_deg as f32,
        );
        light.illuminance *= moon.sun_ratio as f32;
        Some((light, moon.sun_ratio as f32))
    }

    /// The endless ground for the renderer: the project's plane with its surroundings, which keep
    /// clear of the plot: everything drawn and the terrain.
    /// shortcut: plantings count only through the terrain and the shapes they stand in; a wide
    /// scatter on open ground beyond both can meet the fields.
    pub fn render_ground(&self, height: f32, color: [f32; 3]) -> visioncraft_render::Ground {
        use visioncraft_core::Surroundings as S;
        use visioncraft_render::Surroundings as R;
        let mut lo = glam::Vec2::splat(f32::INFINITY);
        let mut hi = glam::Vec2::splat(f32::NEG_INFINITY);
        if let Some((a, b)) = self.scene.bounds() {
            lo = lo.min(glam::Vec2::new(a.x, a.z));
            hi = hi.max(glam::Vec2::new(b.x, b.z));
        }
        if let Some(t) = &self.project().terrain {
            let [a, b] = t.extent();
            lo = lo.min(a.into());
            hi = hi.max(b.into());
        }
        if lo.x > hi.x {
            (lo, hi) = (glam::Vec2::ZERO, glam::Vec2::ZERO);
        }
        visioncraft_render::Ground {
            height,
            color,
            surroundings: match self.project().view.surroundings {
                S::None => R::None,
                S::Fields => R::Fields,
                S::Woodland => R::Woodland,
                S::Dunes => R::Dunes,
            },
            plot: [lo.x, lo.y, hi.x, hi.y],
            ..Default::default()
        }
    }

    /// The moon's light on open ground (lux), 0 under an image sky.
    fn moon_lux(&self) -> f32 {
        if self.sky.sun_override().is_some() {
            return 0.0;
        }
        visioncraft_core::sun::moon_position(&self.site(), &self.time()).illuminance_lux as f32
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

    /// The weather to draw: the project's, or on the way to it over `weather_transition_s`
    /// (each value approaching it exponentially, 95 % of the way in that time). A jump of the
    /// clock (a picture at its own time, a paused window) takes the project's at once.
    fn shown_weather(&mut self, v: &visioncraft_core::ViewState) -> Weather {
        let target = Weather::of(v);
        let now = self.clock;
        let shown = match self.weather_shown {
            Some((w, at)) if v.weather_transition_s > 0.0 && (0.0..=1.0).contains(&(now - at)) => {
                let k = 1.0 - (-3.0 * (now - at) as f32 / v.weather_transition_s).exp();
                w.toward(&target, k)
            }
            _ => target,
        };
        self.weather_shown = Some((shown, now));
        shown
    }

    /// The weather drawn is still easing towards the project's.
    pub fn weather_easing(&self) -> bool {
        self.weather_shown
            .is_some_and(|(w, _)| w != Weather::of(&self.project().view))
    }

    /// Sky, haze, ground, wind, exposure and lights from the project's view state.
    fn apply_view(&mut self, elevation: f32) {
        let v = self.project().view.clone();
        let weather = self.shown_weather(&v);
        let snow = self.project().snow();
        let lamps = self.lights_on() && self.project().lights.iter().any(|l| l.enabled);
        let (brighter, power, saturation) = look_grade(v.look, elevation);
        let clouds = v.clouds;
        // A camera metering a snowy scene greys the snow; a photographer opens up a little, not
        // so far that sunlit snow clips or the rest of the picture lifts (`SNOW_EV`).
        self.renderer.set_grade(power, saturation);
        self.renderer.set_snow(snow);
        self.renderer.set_wet(weather.wetness, weather.rain);
        self.renderer.set_mist(weather.mist);
        self.renderer.set_shafts(v.shafts);
        self.renderer.set_vignette_grain(v.vignette, v.grain);
        self.renderer.set_white_balance(match v.white_balance {
            visioncraft_core::WhiteBalance::Daylight => visioncraft_render::WhiteBalance::Daylight,
            visioncraft_core::WhiteBalance::Auto => visioncraft_render::WhiteBalance::Auto,
            visioncraft_core::WhiteBalance::Kelvin(k) => {
                visioncraft_render::WhiteBalance::Kelvin(k)
            }
        });
        let render_grade = |g: visioncraft_core::Grade| match g {
            visioncraft_core::Grade::Neutral => visioncraft_render::ColorGrade::Neutral,
            visioncraft_core::Grade::Warm => visioncraft_render::ColorGrade::Warm,
            visioncraft_core::Grade::Cool => visioncraft_render::ColorGrade::Cool,
            visioncraft_core::Grade::Soft => visioncraft_render::ColorGrade::Soft,
        };
        self.renderer.set_color_grade(render_grade(v.grade));
        self.renderer
            .set_color_grade_mix(self.grade_mix.map(|(g, t)| (render_grade(g), t)));
        // A camera sees the sun's glare and the air's own colour far off; both are part of
        // the photographic look, not settings.
        self.renderer.set_sun_glare(1.0);
        self.renderer.set_aerial_perspective(true);
        // The window and its pictures light by the sky each point sees unless told not to.
        self.renderer
            .set_sky_occlusion(v.sky_occlusion.unwrap_or(true));
        self.renderer.set_clouds(&visioncraft_render::Clouds {
            cover: weather.cover,
            overcast: weather.overcast,
            storm: weather.storm,
            volumetric: clouds.volumetric,
            altitude_m: clouds.altitude_m,
            speed_mps: clouds.speed_mps,
            sun_gap: clouds.sun_gap,
            lightning: clouds.lightning,
        });
        // Light bounced up from what lies under the camera (paving under a canopy, lawn in the
        // garden), with most of its colour taken out: a green lawn tints a wall only a little.
        let eye = self.camera.eye();
        // The cell, and the snow in the w (the ground bounces snow's white).
        let cell = (eye * 2.0).round().as_ivec3().extend((snow * 100.0) as i32);
        if self.bounce_cell != Some(cell) {
            self.bounce_cell = Some(cell);
            let albedo = self
                .scene
                .albedo_below(self.project(), eye)
                .or(v.ground.map(|g| g.color))
                .map(|c| {
                    let c: [f32; 3] =
                        std::array::from_fn(|k| c[k] + ([0.8, 0.83, 0.88][k] - c[k]) * snow);
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
        // In a room a photographer meters what the camera sees (`meter_view`) and opens up or
        // stops down towards it: by day a room gets only the sky its windows show, at night
        // the lamps light it. Only part of the way (`METER_SHARE`), so a dim corridor still
        // reads dimmer than a daylit room, and only as far as the view is enclosed. The eye
        // takes a moment to adapt (`adapt_ev`), a picture none.
        // Lamps fading in (a film's `lights`) fade the exposure's dusk allowance in with them.
        let auto = if lamps && self.lamp_level < 1.0 {
            let off = self.sky.auto_ev(elevation, self.moon_lux(), false);
            let on = self.sky.auto_ev(elevation, self.moon_lux(), true);
            off + (on - off) * self.lamp_level
        } else {
            self.sky.auto_ev(elevation, self.moon_lux(), lamps)
        };
        let room = match self.meter_view(METER_LUX * auto.exp2()) {
            Some((lux, enclosed)) => meter_shift(auto, lux, enclosed),
            None => 0.0,
        };
        // The eye adapts to what the view takes in (`room`: walking from the garden into a room),
        // not to the daylight itself: that changes over minutes, and a jump of the clock by
        // command (time.set, a viewpoint's time) is met at once. Adapting it too left a scripted
        // picture at dusk exposed for noon (black) while the clock stood still.
        let dt = (self.clock - self.adapted_at).clamp(0.0, 10.0) as f32;
        let adapted = self.adapted_ev.map_or(room, |a| adapt_ev(a, room, dt));
        self.adapted_ev = Some(adapted);
        self.adapted_at = self.clock;
        // A mist takes the sun's beam and gives it back as a dim, even light, and the camera
        // meters the white veil down to grey: a photographer opens up for fog (`MIST_EV`).
        let target = auto - brighter - snow * SNOW_EV - weather.mist * MIST_EV - adapted;
        let ev = v.exposure_ev.unwrap_or(target) - self.exposure;
        self.ev_used = (ev, elevation);
        self.renderer.set_exposure(ev);
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
                haze: haze_extinction(v.haze, v.view_distance) * (1.0 - 0.7 * night),
                haze_height_m: AEROSOL_HEIGHT_M,
                sun_glow: 0.5 * (1.0 - night),
                clear_m: 0.0,
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
        let ground = v.ground.map(|g| self.render_ground(g.height, g.color));
        self.renderer.set_ground(ground);
        self.renderer.set_sea(
            self.project()
                .sea
                .as_ref()
                .map(|s| render_sea(s, &v, &self.project().site)),
        );
        self.renderer.set_wind(&visioncraft_render::Wind {
            direction: Vec3::from(
                self.project()
                    .site
                    .bearing_to_drawing(f64::from(v.wind_direction_deg)),
            ),
            speed_mps: weather.wind_speed_mps,
            gustiness: v.wind_gustiness,
            gusts: weather.gustiness,
        });
        self.renderer.set_time(self.clock);
        let mut lights = if self.lights_on() {
            render_lights(self.project())
        } else {
            Vec::new()
        };
        for l in &mut lights {
            l.intensity *= self.lamp_level;
        }
        if lights != self.last_lights {
            self.dropped_lights = self.renderer.set_lights(&lights);
            self.last_lights = lights;
            self.lamp_bounce.clear();
            self.upload_sky_volume();
        }
        // The sun's light let into the rooms: made again when the sun has moved a degree, sent
        // again when its illuminance changed by a twentieth.
        let to_sun = self.sun().0.direction.normalize_or_zero();
        let lux = self.renderer.sun_lux();
        let turned = to_sun.dot(self.sun_bounce_dir) < SUN_BOUNCE_COS;
        if turned
            || (lux - self.sun_lux).abs().max_element() > 0.05 * self.sun_lux.max_element() + 1.0
        {
            if turned {
                self.sun_bounce_dir = to_sun;
                self.sun_bounce.clear();
            }
            self.sun_lux = lux;
            self.upload_sky_volume();
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
    /// about the scene; past a grace period they can still step it down) and not for 10 s after a step down (else it climbs back into the tier it
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
        // Down while loading (past the grace), up only once it is done: a rise mid-load (fast
        // frames before the trees arrive) took an integrated GPU to HIGH and 11 fps.
        let can_rise = self.clock >= self.auto_hold_until && !loading;
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

    /// Notes the load's milestones reached by now (`load_stats`), after a frame.
    pub fn track_load(&self) {
        use crate::load_stats::mark;
        if self.scene.flora.rev > 0 {
            mark("first plant generated");
        }
        let plants = self.plants_pending() == 0;
        let models = self.models_pending() == 0;
        let library = self.scene.library.idle();
        let sky = !self.sky_volume_pending() && !self.sky_visibility_pending();
        if plants {
            mark("plants done");
        }
        if models {
            mark("models imported");
        }
        if library {
            mark("library idle");
        }
        if plants && models && library && sky {
            mark("everything loaded");
        }
    }

    /// Plants still being generated.
    pub fn plants_pending(&self) -> usize {
        self.scene.flora.pending() + self.scene.lawns_pending() + self.scene.clutter_pending()
    }

    pub fn animating(&self) -> bool {
        let p = self.project();
        // The sea's waves run all the time, and the rain falls. Gusts also roll over lawns
        // (which are materials, not generators).
        (p.view.wind_speed_mps > 0.0 && (!p.generators.is_empty() || p.view.gustiness > 0.0))
            || p.sea.is_some()
            || p.view.rain > 0.0
            || self.weather_easing()
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
        let t = crate::assets::now_ms();
        self.prepare_now();
        crate::load_stats::add("~frame prepare (main thread)", crate::assets::now_ms() - t);
    }

    fn prepare_now(&mut self) {
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
        let pumped = crate::load_stats::time("~scene pump (main thread)", || {
            self.scene.pump(&mut self.renderer, eye, &px_per_m)
        });
        if pumped {
            self.draft_serial += 1;
        }
        self.scene
            .library
            .set_user_objects(&self.session.project().objects);
        self.take_catalog();
        // A wall photo's straightened picture arrived: the faces that show it are built again.
        if self.scene.photos_changed() {
            self.draft_serial += 1;
        }
        self.sync();
        self.poll_sky_volume();
        #[cfg(target_arch = "wasm32")]
        self.poll_example_imports();
        self.scene
            .sync_terrain_layers(&mut self.renderer, self.session.project());
        self.sync_sun_map();
        self.sync_sky_visibility();
        self.sync_underlays();
        let (view, proj, eye) = self.camera.matrices(self.aspect());
        self.renderer.set_camera(view, proj, eye);
        let (sun, elevation) = self.sun();
        let moon = self.moonlight();
        self.renderer.set_sun(&moon.map_or(sun, |m| m.0));
        self.renderer.set_sky_scale(moon.map_or(1.0, |m| m.1));
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
        // A still takes the highest tier natively. Not in the browser: there an HD photo's step
        // up (two dozen maps, decoded and compressed by the page's worker) had not arrived after
        // the 20 s wait on an integrated GPU and took texture memory from 75 to 576 MB with the
        // photo's own targets; it keeps the live tier.
        let tier = if self.still_textures.is_some() && cfg!(not(target_arch = "wasm32")) {
            "4k"
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
        // MEDIUM and up draw the lawns' blades on the GPU; LOW keeps the tufts.
        if self.scene.set_gpu_grass(&mut self.renderer, q.grass > 0) {
            self.synced = None;
        }
        let budget = self.gpu_lawn_budget * if preset_name(&q) == "ultra" { 2.0 } else { 1.0 };
        if self.scene.set_lawn_budget(&mut self.renderer, budget) {
            self.synced = None;
        }
        if self.scene.set_clutter_low(preset_name(&q) == "low") {
            self.synced = None;
        }
        // Catalogue plants far off as impostors below HIGH (`Library::set_plant_impostors`).
        if self
            .scene
            .library
            .set_plant_impostors(&mut self.renderer, !fine)
        {
            self.synced = None;
        }
        let rebake = q.impostor_resolution != self.renderer.quality().impostor_resolution;
        let (t, builds) = (crate::assets::now_ms(), self.renderer.pipeline_builds());
        self.renderer.set_quality(q);
        let ms = crate::assets::now_ms() - t;
        if self.renderer.pipeline_builds() != builds {
            crate::load_stats::add("quality change: every pipeline made (main thread)", ms);
        } else if ms > 1.0 {
            crate::load_stats::add("quality change: targets (main thread)", ms);
        }
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

    /// Imports a PNG or JPEG as a plan underlay: checked, its bytes held by the asset store and
    /// recorded with `underlay.add`, centred on what the plan view looks at. Returns its id.
    pub fn import_underlay(&mut self, name: &str, bytes: Vec<u8>) -> Result<String, String> {
        if bytes.len() as u64 > visioncraft_core::underlay::MAX_FILE_BYTES {
            return Err(format!(
                "the image is {} MB; at most {} MB can be used",
                bytes.len() / (1024 * 1024),
                visioncraft_core::underlay::MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
        let size = visioncraft_core::underlay::image_size(&bytes)?;
        let file = crate::underlay_ui::file_for(name, &bytes, &self.project().underlays);
        let scale = visioncraft_core::UnderlayPlacement::default().metres_per_pixel;
        let c = self.camera.top_center;
        let origin = [
            c.x - size[0] as f32 * scale / 2.0,
            c.y - size[1] as f32 * scale / 2.0,
        ];
        self.scene.library.store.provide(&file, Arc::from(bytes));
        let r = self
            .run(
                "underlay.add",
                serde_json::json!({"name": name, "file": file, "size_px": size, "origin": origin}),
            )
            .map_err(|e| e.to_string())?;
        Ok(r["id"].as_str().unwrap_or_default().to_string())
    }

    /// Takes a PNG or JPEG for a wall photo: checked, its bytes held by the asset store under a
    /// file name inside `photos/` that no other photo or plan uses, and the decoded picture asked
    /// for (for the straightening window). Returns the file and the image's size.
    pub fn import_wall_photo_file(
        &mut self,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<(String, [u32; 2]), String> {
        if bytes.len() as u64 > visioncraft_core::underlay::MAX_FILE_BYTES {
            return Err(format!(
                "the image is {} MB; at most {} MB can be used",
                bytes.len() / (1024 * 1024),
                visioncraft_core::underlay::MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
        let size = visioncraft_core::underlay::image_size(&bytes)?;
        let file = crate::underlay_ui::photo_file_for(name, &bytes, &self.project().image_files());
        let store = &mut self.scene.library.store;
        store.provide(&file, Arc::from(bytes));
        store.want(crate::assets::Key::Thumb(file.clone()), 1.0);
        Ok((file, size))
    }

    /// Wall photos whose image is not held (a project opened without its bundle).
    pub fn missing_wall_photos(&self) -> Vec<&visioncraft_core::WallPhoto> {
        let store = &self.scene.library.store;
        self.project()
            .wall_photos
            .iter()
            .filter(|w| store.provided(&w.file).is_none())
            .collect()
    }

    /// Underlays whose image is not held (a project opened without its bundle).
    pub fn missing_underlays(&self) -> Vec<&visioncraft_core::Underlay> {
        let store = &self.scene.library.store;
        self.project()
            .underlays
            .iter()
            .filter(|u| store.provided(&u.file).is_none())
            .collect()
    }

    /// Forgets the sun-hours map (another project): its texture is freed, the next frame
    /// starts the new project's, if it has one.
    fn drop_sun_map(&mut self) {
        if let Some(t) = self.sun_map.clear() {
            self.renderer.remove_texture(t);
        }
    }

    /// Keeps the sun-hours map up to date: takes a finished grid up to the GPU as a texture,
    /// and starts the work again when the project, the loaded models or the setting changed.
    fn sync_sun_map(&mut self) {
        use crate::sun_map::{Done, Key};
        let Some(map) = self.session.project().view.sun_map.clone() else {
            self.drop_sun_map();
            return;
        };
        if let Some((_, grid)) = self.sun_map.poll() {
            let rgba = visioncraft_core::sun_map::texture_rgba(&grid.spec, &grid.hours);
            match self.renderer.add_texture(&visioncraft_render::TextureDesc {
                width: grid.spec.cols as u32,
                height: grid.spec.rows as u32,
                rgba8: &rgba,
                color_space: visioncraft_render::ColorSpace::Srgb,
                repeat: false,
            }) {
                Ok(texture) => {
                    if let Some(old) = self.sun_map.done.take() {
                        self.renderer.remove_texture(old.texture);
                    }
                    self.sun_map.done = Some(Done {
                        spec: grid.spec,
                        texture,
                        stats: grid.stats(),
                        years: grid.years,
                    });
                }
                Err(e) => self.problems.push(format!("sun map: {e}")),
            }
        }
        let models = self
            .loaded
            .iter()
            .map(|(id, l)| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (id, l.version).hash(&mut h);
                h.finish()
            })
            .fold(0u64, u64::wrapping_add);
        let key = Key::new(self.session.revision(), models, &map);
        if self.sun_map.is_current(&key) {
            return;
        }
        let p = self.session.project();
        let loaded = |id: &str| self.loaded.get(id).map(|l| l.model.clone());
        match visioncraft_automation::SunGridJob::for_map(p, &loaded, &self.assets, &map) {
            Ok(job) => self.sun_map.start(key, job),
            Err(why) => {
                if let Some(old) = self.sun_map.done.take() {
                    self.renderer.remove_texture(old.texture);
                }
                self.sun_map.fail(key, why);
            }
        }
    }

    /// Keeps the baked sky visibility up to date while sky occlusion is on: takes a finished
    /// grid up to the GPU, and bakes again when what blocks the sky changed (`sky_vis`).
    fn sync_sky_visibility(&mut self) {
        let now = crate::assets::now_ms();
        if let Some(grid) = self.sky_vis.poll(now) {
            match &grid {
                Some(v) => {
                    let texels = v.texels();
                    self.renderer
                        .set_sky_visibility(Some(visioncraft_render::SkyVisibilityGrid {
                            origin: v.origin,
                            cell: v.cell,
                            dims: v.dims,
                            lift: v.lift,
                            height: v.height,
                            texels: &texels,
                        }));
                }
                None => self.renderer.set_sky_visibility(None),
            }
        }
        self.sky_vis_waiting = false;
        if !self.session.project().view.sky_occlusion.unwrap_or(true) {
            return;
        }
        // The plants and models first: a bake shares the cores with what makes them (on the
        // web all the workers' for half a minute on the coast, which held its plants back by
        // as long), and the models it waits for change what it bakes.
        if self.plants_pending() > 0 || self.models_pending() > 0 {
            self.sky_vis_waiting = true;
            return;
        }
        let p = self.session.project();
        let models = self
            .loaded
            .iter()
            .map(|(id, l)| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (id, l.version).hash(&mut h);
                h.finish()
            })
            .fold(0u64, u64::wrapping_add);
        let checked = (
            self.session.revision(),
            models,
            p.view.years.map(f32::to_bits),
            p.time.month,
        );
        if self.sky_vis_checked == Some(checked) {
            return;
        }
        self.sky_vis_checked = Some(checked);
        let inputs = crate::sky_vis::inputs_key(p, models);
        if self.sky_vis.is_current(inputs) {
            return;
        }
        let loaded = |id: &str| self.loaded.get(id).map(|l| l.model.clone());
        let scene = crate::load_stats::time("sky visibility scene (main thread)", || {
            visioncraft_automation::sky_scene(p, &loaded, &self.assets, p.view.years, p.time.month)
        });
        if self.sky_vis.note(inputs, scene.key()) {
            self.sky_vis.start(scene, now);
        }
    }

    /// Whether the sky visibility is being baked, or waits for plants and models to bake.
    pub fn sky_visibility_pending(&self) -> bool {
        self.sky_vis.pending() || self.sky_vis_waiting
    }

    /// Whether the sun-hours map is being worked out.
    pub fn sun_map_pending(&self) -> bool {
        self.sun_map.pending()
    }

    /// Gives the renderer the visible underlays whose image has arrived, flat on the ground.
    fn sync_underlays(&mut self) {
        let p = self.session.project();
        let live = self.underlay_live.clone();
        let wanted: Vec<_> = p
            .underlays
            .iter()
            .filter(|u| {
                p.layers
                    .iter()
                    .find(|l| l.id == u.layer)
                    .is_none_or(|l| l.visible)
            })
            .map(|u| {
                let opacity = match &live {
                    Some((id, v)) if *id == u.id => *v,
                    _ => u.opacity,
                };
                let y = u.elevation;
                (
                    u.file.clone(),
                    u.corners().map(|c| [c[0], y, c[1]]),
                    opacity,
                )
            })
            .filter(|(_, _, opacity)| *opacity > 0.0)
            .collect();
        let mut quads = Vec::new();
        for (file, corners, opacity) in wanted {
            if let Some(texture) = self.scene.library.store.texture(&file, true, false, 0.0) {
                quads.push(visioncraft_render::UnderlayQuad {
                    texture,
                    corners,
                    opacity,
                });
            }
        }
        // The sun-hours map over the plans, flat just over its area.
        if let (Some(map), Some(done)) = (&p.view.sun_map, &self.sun_map.done)
            && map.opacity > 0.0
        {
            let y = map.elevation(p);
            quads.push(visioncraft_render::UnderlayQuad {
                texture: done.texture,
                corners: done.spec.corners().map(|c| [c[0], y, c[1]]),
                opacity: map.opacity,
            });
        }
        if quads != self.underlay_quads {
            self.renderer.set_underlays(&quads);
            self.underlay_quads = quads;
        }
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

/// The photo the project's settings (`view.set_photo`) take: the app's photo export.
pub fn photo_request(settings: &visioncraft_core::PhotoSettings) -> PhotoRequest {
    PhotoRequest {
        samples: settings.samples,
        f_number: settings.aperture,
        aperture_m: 0.0,
        focus: settings.focus,
        focus_point: settings.focus_point,
        shutter_s: settings.shutter_s,
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
        // A picture is one frame: the environment may not lag behind it (a dusk picture after a
        // daytime one kept the day's sky in its haze).
        studio.renderer.finish_environment();
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
            // Luminance 1, so a lamp's lumens are its lumens whatever its colour (a warm lamp
            // scaled to its red channel lost a third of its light).
            let m = luminance(c).max(1e-3);
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

/// Rec. 709 luminance of a linear colour.
fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// A warm room lamp behind each lit window of the visible houses (`shape.set_lit_rooms`).
fn lit_rooms(p: &Project) -> impl Iterator<Item = visioncraft_render::Light> + '_ {
    let c = visioncraft_core::kelvin_to_linear_rgb(2700.0);
    let m = luminance(c).max(1e-3);
    p.shapes
        .iter()
        .filter_map(|s| match s {
            visioncraft_core::Shape::Extrusion(e) if e.closed && p.layer_visible(&e.layer) => e
                .house
                .is_some()
                .then(|| visioncraft_core::house::lit_windows(e)),
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
    fn the_meter_opens_up_for_a_dim_room_and_not_out_of_doors() {
        use super::{METER_LUX, METER_SHARE, meter_point, meter_shift};
        let open = METER_LUX * 15f32.exp2();
        // Under the open sky the meter reads the open light and moves nothing.
        assert_eq!(meter_point(open, 1.0, 0.3, 50.0, 0.0), open);
        assert_eq!(meter_shift(15.0, open, 1.0), 0.0);
        // A room four stops under it: `METER_SHARE` of them.
        let room = meter_point(open, 0.0, 0.0, open / 16.0, 0.0);
        assert!((meter_shift(15.0, room, 1.0) - 4.0 * METER_SHARE).abs() < 1e-4);
        // Half enclosed: half of that; a black room no more than the limit.
        assert!((meter_shift(15.0, room, 0.5) - 2.0 * METER_SHARE).abs() < 1e-4);
        assert_eq!(meter_shift(15.0, 0.0, 1.0), super::METER_UP_EV);
        // A lamp shining straight on the surface counts in full; the inside of a wall (-1) not.
        assert_eq!(meter_point(open, 0.0, -1.0, 0.0, 300.0), 300.0);
    }

    #[test]
    fn the_eye_adapts_in_about_a_second_whatever_the_frame_rate() {
        use super::adapt_ev;
        // 95 % of the way in a second.
        let a = adapt_ev(15.0, 10.0, 1.0);
        assert!((15.0 - a) / 5.0 > 0.94 && (15.0 - a) / 5.0 < 0.97, "{a}");
        // Sixty frames of a sixtieth come out where ten frames of a tenth do.
        let (mut fast, mut slow) = (15.0f32, 15.0f32);
        (0..60).for_each(|_| fast = adapt_ev(fast, 10.0, 1.0 / 60.0));
        (0..10).for_each(|_| slow = adapt_ev(slow, 10.0, 0.1));
        assert!((fast - slow).abs() < 1e-3 && (fast - a).abs() < 1e-3);
        // No time, no change; a long pause, there.
        assert_eq!(adapt_ev(15.0, 10.0, 0.0), 15.0);
        assert!((adapt_ev(15.0, 10.0, 30.0) - 10.0).abs() < 1e-4);
    }

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
            let ev = auto_ev(e as f32, 0.0, false);
            assert!(ev <= last + 1e-6, "{e}");
            last = ev;
        }
        for e in [-10.0, -6.0, -2.0] {
            assert!(auto_ev(e, 0.0, true) > auto_ev(e, 0.0, false));
        }
        assert_eq!(auto_ev(20.0, 0.0, true), auto_ev(20.0, 0.0, false));
        // The day is as before; sunset, blue hour and a moonlit night open up.
        assert!((auto_ev(10.0, 0.0, false) - 13.5).abs() < 0.05);
        assert!((8.8..9.8).contains(&auto_ev(0.0, 0.0, false)));
        assert!((1.8..2.8).contains(&auto_ev(-6.0, 0.0, false)));
        let moonlit = auto_ev(-20.0, 0.3, false);
        assert!((-1.0..0.0).contains(&moonlit), "{moonlit}");
        // A moonless night still shows darker than a moonlit one: six times less light, under
        // 2.6 stops more exposure.
        let moonless = auto_ev(-20.0, 0.0, false);
        assert!(
            moonless < moonlit && moonless > moonlit - (0.358f32 / 0.058).log2(),
            "{moonless}"
        );
    }
}

/// The aerosol's scale height (m): the haze thins by a factor e every 1.2 km up (Hillaire 2020's
/// Earth atmosphere, after Bruneton 2017), so an overview from a few hundred metres looks
/// through nearly the same air as eye level.
const AEROSOL_HEIGHT_M: f32 = 1200.0;

/// The haze's extinction at the ground (per km) for the project's `haze`, a turbidity in the
/// sense of Preetham et al. 1999 (`T = (tau_rayleigh + tau_aerosol) / tau_rayleigh`, vertical
/// optical depths): 1 pure air, 2-3 a clear day, 6 hazy, 20 misty; 0 turns the haze off. The
/// aerosol's vertical optical depth (T - 1) * 0.097 (Rayleigh's at 550 nm, Bucholtz 1995)
/// spread over its scale height, plus Rayleigh's own 0.0116 / km at the ground. So the
/// default 2.5 is 0.13 / km, a visibility (Koschmieder: 3.912 / extinction) of 30 km; 6 gives
/// 9 km, 20 about 2.5 km. A longer `view_distance` clears the air in proportion.
pub fn haze_extinction(turbidity: f32, view_distance: f32) -> f32 {
    if turbidity <= 0.0 {
        return 0.0;
    }
    let aerosol = (turbidity - 1.0).max(0.0) * 0.097 / (AEROSOL_HEIGHT_M / 1000.0);
    (aerosol + 0.0116) / view_distance.max(0.05)
}

/// How far shadows reach and how far small impostors draw, for the camera and the project's
/// view distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    pub shadow_m: f32,
    /// How far the impostors of shrubs and ground plants draw, as a share of their full reach.
    pub small_impostors: f32,
}

/// From an orbit the shadows reach the far side of what the camera looks at (twice its distance
/// to the pivot, at least the 80 m of eye level), in 20 m steps so zooming does not re-render
/// them every frame. Small plants' impostors draw
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
        small_impostors: (height / 40.0).clamp(0.1, 1.0),
    }
}

#[cfg(test)]
mod reach_tests {
    use super::view_reach;
    use crate::camera::{Camera, View};

    #[test]
    fn an_overview_reaches_further_than_eye_level() {
        let mut c = Camera::default();
        c.view = View::Orbit;
        c.dist = 120.0;
        let far = view_reach(&c, 0.0, 1.0);
        assert!(far.shadow_m >= 240.0, "{far:?}");
        assert_eq!(view_reach(&c, 0.0, 2.0).shadow_m, 2.0 * far.shadow_m);
        c.view = View::Walk;
        c.walk.eye.y = 1.7;
        let eye = view_reach(&c, 0.0, 1.0);
        assert_eq!(eye.shadow_m, 80.0);
        // Haze: a clear day sees about 30 km, the default view distance; double it, twice as far.
        let visibility = 3.912 / super::haze_extinction(2.5, 1.0);
        assert!((25.0..35.0).contains(&visibility), "{visibility}");
        assert_eq!(
            super::haze_extinction(2.5, 2.0),
            0.5 * super::haze_extinction(2.5, 1.0)
        );
        assert_eq!(super::haze_extinction(0.0, 1.0), 0.0);
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

/// What the light meter (`meter_view`) reads where one of its rays meets a surface (lux): the
/// open sky's light (`open_lux` under it) as far as the surface sees the sky, and for the rest
/// the room's: the sky's light passed round it (`room`, per unit of the sky's mean light, which
/// is taken as `METER_SKY_SHARE` of the open light) and the lamps' and the sun's (`bounced`,
/// lux); plus the lamps that light it directly (`direct`).
pub fn meter_point(open_lux: f32, seen: f32, room: f32, bounced: f32, direct: f32) -> f32 {
    let room = open_lux * METER_SKY_SHARE * room.max(0.0) + bounced;
    open_lux * seen + (1.0 - seen) * room + direct
}

/// How many stops the exposure opens up (positive) or stops down from the automatic `auto_ev`
/// for a view that meters `metered_lux`: `METER_SHARE` of the way to it, within the limits, as
/// far as the view is enclosed (`enclosed`, 0 out of doors).
pub fn meter_shift(auto_ev: f32, metered_lux: f32, enclosed: f32) -> f32 {
    let metered = (metered_lux.max(1e-3) / METER_LUX).log2();
    enclosed * (METER_SHARE * (auto_ev - metered)).clamp(-METER_DOWN_EV, METER_UP_EV)
}

/// Eye adaptation: the exposure `dt` seconds on from `adapted` towards `target`, an exponential
/// approach with the time constant `ADAPT_S`, the same whatever the frame rate.
pub fn adapt_ev(adapted: f32, target: f32, dt: f32) -> f32 {
    target + (adapted - target) * (-dt.max(0.0) / ADAPT_S).exp()
}

/// The time constant of eye adaptation (s): 95 % of a change in a second.
const ADAPT_S: f32 = 0.33;

/// The light meter (`meter_view`): its grid of rays over the picture; the illuminance (lux) the
/// automatic exposure is made for at EV 0, doubling per stop (a white wall under it reads
/// about a third of white); the share of the metered difference the exposure follows; how far
/// it opens up and stops down; the sky's mean light (half its up- and down-facing illuminance,
/// as scene.wgsl's `room_bounce`) against the light the automatic exposure is made for.
/// The cosine of the angle the sun moves before its light in the rooms is made again (1 deg).
const SUN_BOUNCE_COS: f32 = 0.999_85;

const METER_COLUMNS: usize = 6;
const METER_ROWS: usize = 4;
const METER_LUX: f32 = 1.5;
const METER_SHARE: f32 = 0.85;
const METER_UP_EV: f32 = 5.0;
const METER_DOWN_EV: f32 = 9.0;
const METER_SKY_SHARE: f32 = 0.4;

/// Stops the exposure opens up per unit of snow.
const SNOW_EV: f32 = 1.2;

/// Stops a photographer opens up in a thick mist (scaled by the mist, 0..1): fog meters grey
/// like snow, and the light under it is the sun's scattered, weaker than the direct beam.
const MIST_EV: f32 = 1.0;

/// What a picture taken with `pending` plants still being made, or the library still loading,
/// says about it.
/// `plants` and `files` name some of what was not ready (species, library paths).
pub fn picture_problems(
    pending: usize,
    library_idle: bool,
    plants: &[String],
    files: &[String],
) -> Vec<String> {
    let named = |names: &[String]| {
        if names.is_empty() {
            String::new()
        } else {
            format!(" ({})", names.join(", "))
        }
    };
    let mut out = Vec::new();
    if pending > 0 {
        out.push(format!(
            "{pending} plant generations were not finished after {PICTURE_WAIT_S:.0} s{}: those plants show as stand-ins",
            named(plants)
        ));
    }
    if !library_idle {
        out.push(format!(
            "library files were still loading after {PICTURE_WAIT_S:.0} s{}: some materials or objects show their stand-ins",
            named(files)
        ));
    }
    out
}

#[cfg(test)]
mod picture_tests {
    #[test]
    fn a_picture_with_plants_pending_says_so() {
        assert!(super::picture_problems(0, true, &[], &[]).is_empty());
        let p = super::picture_problems(7, true, &["flora:betula-pendula".into()], &[]);
        assert_eq!(p.len(), 1);
        assert!(p[0].starts_with("7 plant generations"));
        assert!(p[0].contains("(flora:betula-pendula)"), "{}", p[0]);
        let f = super::picture_problems(0, false, &[], &["materials/a/basecolor.jpg".into()]);
        assert_eq!(f.len(), 1);
        assert!(f[0].contains("materials/a/basecolor.jpg"));
    }
}

/// Pictures in one session do not inherit each other's sky: a dusk picture after a noon one
/// matches a dusk picture from a fresh studio (the environment cube, which the haze and
/// reflections read, was only partly redone in a one-frame picture).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod session_leak_tests {
    use super::Studio;

    fn picture(s: &mut Studio, device: &wgpu::Device, queue: &wgpu::Queue, hour: f32) -> Vec<u8> {
        let mut p = s.project().clone();
        p.time.hour = hour;
        s.set_session(visioncraft_core::Session::new(p));
        s.render_pixels(device, queue, 160, 100).unwrap()
    }

    #[test]
    fn a_dusk_picture_after_a_noon_one_matches_a_fresh_dusk_picture() {
        let Ok((device, queue)) = pollster::block_on(visioncraft_render::headless_device()) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let p = visioncraft_core::example("beach-house").unwrap();
        let studio = |p: &visioncraft_core::Project| {
            let r =
                visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
            let mut s = Studio::new(r, p.clone());
            s.camera.look_from(
                glam::Vec3::new(16.0, 2.5, 24.0),
                glam::Vec3::new(11.0, 2.0, -4.0),
            );
            s
        };
        let mut fresh = studio(&p);
        let want = picture(&mut fresh, &device, &queue, 20.3);
        let mut used = studio(&p);
        picture(&mut used, &device, &queue, 13.0);
        let got = picture(&mut used, &device, &queue, 20.3);
        let mean = |px: &[u8]| px.iter().map(|&v| f64::from(v)).sum::<f64>() / px.len() as f64;
        let diff = want
            .iter()
            .zip(&got)
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / want.len() as f64;
        assert!(
            diff < 3.0,
            "dusk after noon differs from a fresh dusk by {diff:.1} per channel (means {:.1} and {:.1})",
            mean(&want),
            mean(&got)
        );
    }
}

/// The project's sea in the renderer's terms: waves along `direction_deg` or the wind, flow for
/// a river only.
pub fn render_sea(
    s: &visioncraft_core::Sea,
    v: &visioncraft_core::ViewState,
    site: &Site,
) -> visioncraft_render::Sea {
    let towards = |deg: f32| {
        let d = site.bearing_to_drawing(f64::from(deg));
        [d[0], d[2]]
    };
    let river = s.kind == visioncraft_core::WaterKind::River;
    let flow = towards(s.flow_deg).map(|c| if river { c * s.flow_speed } else { 0.0 });
    visioncraft_render::Sea {
        level: s.level,
        wave_height_m: s.wave_height_m(),
        peak_wavelength_m: s.peak_wavelength_m(),
        direction: towards(s.direction_deg.unwrap_or(v.wind_direction_deg)),
        color: s.color,
        turbidity: 1.0 - s.clarity,
        flow,
        foam: s.foam,
        freezes: s.freezes(),
        outline: s.outline.clone(),
        coast: s.coast.clone(),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod open_tests {
    use super::{Studio, find_model_file};
    use std::path::{Path, PathBuf};
    use visioncraft_core::{EXAMPLES, Project};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vc-open-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn found(source: &str, project: Option<&Path>, relink: Option<&Path>) -> Option<PathBuf> {
        find_model_file(source, project, relink).map(|(p, _)| p)
    }

    #[test]
    fn a_model_file_is_found_in_the_documented_order() {
        let root = temp("order");
        let (project, relink, elsewhere) = (root.join("p"), root.join("r"), root.join("e"));
        for d in [&project, &relink, &elsewhere, &project.join("sub")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let write = |p: &Path| std::fs::write(p, b"x").unwrap();
        let (p, r) = (Some(project.as_path()), Some(relink.as_path()));
        // 4. The same file name in the folder last relinked from.
        write(&relink.join("h.3dcraft"));
        let gone = elsewhere.join("gone").join("h.3dcraft");
        let gone = gone.to_str().unwrap();
        assert_eq!(found(gone, p, r), Some(relink.join("h.3dcraft")));
        assert_eq!(found(gone, p, None), None);
        // 3. The same file name in the project's folder beats the relink folder.
        write(&project.join("h.3dcraft"));
        assert_eq!(found(gone, p, r), Some(project.join("h.3dcraft")));
        assert_eq!(
            found("C:\\old\\h.3dcraft", p, r),
            Some(project.join("h.3dcraft"))
        );
        // 2. Relative to the project's folder beats the bare file name.
        write(&project.join("sub").join("h.3dcraft"));
        assert_eq!(
            found("sub/h.3dcraft", p, r),
            Some(project.join("sub").join("h.3dcraft"))
        );
        // 1. The stored path as it is beats everything.
        write(&elsewhere.join("h.3dcraft"));
        let there = elsewhere.join("h.3dcraft");
        assert_eq!(found(there.to_str().unwrap(), p, r), Some(there.clone()));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn studio() -> Option<Studio> {
        let (device, queue) = pollster::block_on(visioncraft_render::headless_device()).ok()?;
        let r = visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        Some(Studio::new(r, Project::default()))
    }

    /// Every built-in example, saved to bytes and opened as the window's Open does, has all its
    /// models: an `example:` source never asks for its file.
    #[test]
    fn every_saved_example_opens_with_its_models() {
        let Some(mut s) = studio() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let dir = temp("examples");
        for (name, _) in EXAMPLES {
            let bytes = visioncraft_core::example(name).unwrap().to_json_bytes();
            let p = Project::from_json_bytes(&bytes).unwrap();
            let r = s.open(p, Some(&dir), None);
            assert!(r.errors.is_empty(), "{name}: {:?}", r.errors);
            let missing: Vec<&str> = s
                .missing_models()
                .iter()
                .map(|m| m.source.as_str())
                .collect();
            assert!(missing.is_empty(), "{name}: missing {missing:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The user's window was a build from before the beach house was carried inside the program:
    /// `example:beach-house.3dcraft` was unknown to it, treated as a path, not found, and the
    /// import error was swallowed, so all it showed was the relink bar. Now an unknown example
    /// says why, and an example source an old save turned into a path still loads.
    #[test]
    fn an_unknown_example_says_why_and_an_old_saved_example_path_loads() {
        let Some(mut s) = studio() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let mut p = visioncraft_core::example("beach-house").unwrap();
        p.models[0].source = "/gone/folder/example:beach-house.3dcraft".into();
        let r = s.open(p.clone(), None, None);
        assert!(
            r.errors.is_empty() && s.missing_models().is_empty(),
            "{:?}",
            r.errors
        );
        p.models[0].source = "example:newer-house.3dcraft".into();
        let r = s.open(p, None, None);
        assert_eq!(s.missing_models().len(), 1);
        assert!(
            r.errors.len() == 1 && r.errors[0].contains("example:newer-house.3dcraft"),
            "{:?}",
            r.errors
        );
    }

    /// A model saved with a path that no longer exists opens from the same file name next to
    /// the project file, without asking.
    #[test]
    fn a_moved_model_file_loads_from_next_to_the_project() {
        let Some(mut s) = studio() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let dir = temp("moved");
        std::fs::write(
            dir.join("house.3dcraft"),
            visioncraft_core::example_file("example:beach-house.3dcraft").unwrap(),
        )
        .unwrap();
        let mut p = visioncraft_core::example("beach-house").unwrap();
        p.models[0].source = "/gone/folder/house.3dcraft".into();
        let r = s.open(p, Some(&dir), None);
        assert!(s.missing_models().is_empty(), "{:?}", r.errors);
        assert_eq!(r.files, [("m1".to_string(), dir.join("house.3dcraft"))]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The weather as the renderer draws it (`Studio::shown_weather`): the clouds' shape, the rain,
/// the wetness, the mist, the gusts and the wind speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weather {
    pub cover: f32,
    pub overcast: f32,
    pub storm: f32,
    pub rain: f32,
    pub wetness: f32,
    pub mist: f32,
    pub gustiness: f32,
    pub wind_speed_mps: f32,
}

impl Weather {
    fn of(v: &visioncraft_core::ViewState) -> Weather {
        let c = v.clouds.shape();
        Weather {
            cover: c.cover,
            overcast: c.overcast,
            storm: c.storm,
            rain: v.rain,
            wetness: v.wetness,
            mist: v.mist,
            gustiness: v.gustiness,
            wind_speed_mps: v.wind_speed_mps,
        }
    }

    fn fields(&self) -> [f32; 8] {
        [
            self.cover,
            self.overcast,
            self.storm,
            self.rain,
            self.wetness,
            self.mist,
            self.gustiness,
            self.wind_speed_mps,
        ]
    }

    /// `k` (0..1) of the way to `target`; what is within a thousandth of it arrives.
    fn toward(&self, target: &Weather, k: f32) -> Weather {
        let (a, b) = (self.fields(), target.fields());
        let f: [f32; 8] = std::array::from_fn(|i| {
            let x = a[i] + (b[i] - a[i]) * k;
            if (b[i] - x).abs() < 1e-3 { b[i] } else { x }
        });
        Weather {
            cover: f[0],
            overcast: f[1],
            storm: f[2],
            rain: f[3],
            wetness: f[4],
            mist: f[5],
            gustiness: f[6],
            wind_speed_mps: f[7],
        }
    }
}
