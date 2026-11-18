//! The baked sky visibility in the window and its pictures (`view.set sky_occlusion`,
//! `visioncraft_core::sky_visibility`): baked off the UI thread when the design changes, then
//! drawn every frame with the sky as it is.
//!
//! Baking: `visioncraft_automation::SkyVisJob`, the scene `query.sky_view` traces. Natively a
//! thread runs it on all cores (the example garden's 13,104 probes in about 0.4 s on eight cores); on
//! the web, with no threads, the window does [`PROBES_PER_FRAME`] probes a frame so the page keeps
//! moving, as the sun-hours map does. It starts again only when what blocks the sky changed
//! (`Key`: the shapes, models, plantings, terrain, materials and layers, the years and the
//! leaves of the month), never for the hour or the weather; the old grid is drawn until the new
//! one is ready.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use visioncraft_automation::SkyVisJob;
use visioncraft_core::{Project, SkyVisibility};

/// Probes a frame the web window bakes (no threads there).
#[cfg(target_arch = "wasm32")]
pub const PROBES_PER_FRAME: usize = 160;

/// What decides a bake, before its scene is built: a hash of the parts of the project that block
/// the sky, the years and the leaves, and the loaded models.
pub fn inputs_key(p: &Project, models: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for part in [
        serde_json::to_vec(&p.shapes),
        serde_json::to_vec(&p.models),
        serde_json::to_vec(&p.generators),
        serde_json::to_vec(&p.terrain),
        serde_json::to_vec(&p.layers),
        serde_json::to_vec(&p.materials),
        serde_json::to_vec(&p.assignments),
        serde_json::to_vec(&p.view.ground),
        serde_json::to_vec(&p.species),
    ] {
        part.unwrap_or_default().hash(&mut h);
    }
    p.view.years.map(f32::to_bits).hash(&mut h);
    // The leaves a deciduous crown carries: the same in all of summer.
    visioncraft_core::sun_hours::leaf_share(false, p.time.month, p.site.latitude)
        .to_bits()
        .hash(&mut h);
    models.hash(&mut h);
    h.finish()
}

enum Work {
    #[cfg(not(target_arch = "wasm32"))]
    Thread(Arc<Mutex<Option<Option<SkyVisibility>>>>),
    #[cfg(target_arch = "wasm32")]
    Sliced(Option<SkyVisJob>),
}

struct Task {
    cancel: Arc<AtomicBool>,
    work: Work,
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The bake under way and what the drawn grid was made for.
#[derive(Default)]
pub struct SkyVisState {
    task: Option<Task>,
    /// The inputs (`inputs_key`) of the grid drawn or being baked.
    inputs: Option<u64>,
    /// The scene key (`SkyScene::key`) of the grid drawn or being baked.
    scene: Option<u64>,
    /// How long the last bake took (ms), for the measurements.
    pub last_ms: Option<f64>,
    started_ms: f64,
}

impl SkyVisState {
    pub fn pending(&self) -> bool {
        self.task.is_some()
    }

    /// Whether the inputs `key` are what is drawn or being baked.
    pub fn is_current(&self, key: u64) -> bool {
        self.inputs == Some(key)
    }

    /// Records the inputs; true when their scene differs from the last one's and needs a bake.
    pub fn note(&mut self, inputs: u64, scene: u64) -> bool {
        self.inputs = Some(inputs);
        let changed = self.scene != Some(scene);
        self.scene = Some(scene);
        changed
    }

    /// Starts `job`, dropping (cancelling) the one under way.
    pub fn start(&mut self, job: SkyVisJob, now_ms: f64) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.started_ms = now_ms;
        #[cfg(not(target_arch = "wasm32"))]
        let work = {
            let slot: Arc<Mutex<Option<Option<SkyVisibility>>>> = Arc::default();
            let (out, stop) = (slot.clone(), cancel.clone());
            std::thread::spawn(move || {
                let grid = job.run();
                if !stop.load(Ordering::Relaxed)
                    && let Ok(mut o) = out.lock()
                {
                    *o = Some(grid);
                }
            });
            Work::Thread(slot)
        };
        #[cfg(target_arch = "wasm32")]
        let work = Work::Sliced(Some(job));
        self.task = Some(Task { cancel, work });
    }

    /// Moves the bake along (a slice on the web) and takes a finished grid (`Some(None)`:
    /// nothing blocks the sky).
    pub fn poll(&mut self, now_ms: f64) -> Option<Option<SkyVisibility>> {
        let task = self.task.as_mut()?;
        #[cfg(not(target_arch = "wasm32"))]
        let grid = match &task.work {
            Work::Thread(slot) => slot.lock().ok()?.take(),
        };
        #[cfg(target_arch = "wasm32")]
        let grid = match &mut task.work {
            Work::Sliced(slot) => {
                if slot.as_mut()?.step(PROBES_PER_FRAME) {
                    slot.take().map(SkyVisJob::finish)
                } else {
                    None
                }
            }
        };
        let grid = grid?;
        self.task = None;
        self.last_ms = Some(now_ms - self.started_ms);
        Some(grid)
    }

    /// Forgets the grid and the bake (another project).
    pub fn clear(&mut self) {
        self.task = None;
        self.inputs = None;
        self.scene = None;
    }
}
