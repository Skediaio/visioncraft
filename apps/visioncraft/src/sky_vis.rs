//! The baked sky visibility in the window and its pictures (`view.set sky_occlusion`,
//! `visioncraft_core::sky_visibility`): baked off the UI thread when the design changes, then
//! drawn every frame with the sky as it is.
//!
//! Baking: `visioncraft_automation::SkyVisJob`, the scene `query.sky_view` traces. Natively a
//! thread runs it on all cores (the example garden's 13,104 probes in about 0.4 s on eight cores); on
//! the web in the import worker (a `skyvis` job: the scene and the grid travel as floats,
//! `SkyScene::to_floats`), so the page keeps its frame rate. When the worker cannot start, the
//! window bakes a few probes a frame, within [`SLICE_MS`]. It starts again only when what blocks
//! the sky changed (`Key`: the shapes, models, plantings, terrain, materials and layers, the
//! years and the leaves of the month), never for the hour or the weather; the old grid is drawn
//! until the new one is ready.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(target_arch = "wasm32")]
use visioncraft_automation::SkyVisJob;
use visioncraft_core::{Project, SkyScene, SkyVisibility};

/// Milliseconds a frame the web window bakes for when there is no worker (a fixed count of
/// probes cost a heavy scene 120 to 180 ms a frame).
#[cfg(target_arch = "wasm32")]
pub const SLICE_MS: f64 = 4.0;

/// The worker job's kind (beside the model kinds of `import_task` and `sky_task::SKY`).
pub const SKY_VIS: &str = "skyvis";

/// The grid, once baked; `Err` when the worker failed (the window bakes it then).
type Slot = Arc<Mutex<Option<Result<Option<SkyVisibility>, ()>>>>;

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
    /// A thread natively, the import worker on the web (with the scene, should it fail).
    Off(Slot, Option<SkyScene>),
    #[cfg(target_arch = "wasm32")]
    Sliced(Option<SkyVisJob>),
}

struct Task {
    cancel: Arc<AtomicBool>,
    work: Work,
    #[cfg(target_arch = "wasm32")]
    worker: Option<web_sys::Worker>,
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        #[cfg(target_arch = "wasm32")]
        if let Some(w) = &self.worker {
            w.terminate();
        }
    }
}

fn put(slot: &Slot, v: Result<Option<SkyVisibility>, ()>) {
    if let Ok(mut s) = slot.lock() {
        s.get_or_insert(v);
    }
}

/// Bakes `scene` in the import worker; the grid lands in `slot` (`Err` when the worker fails).
#[cfg(target_arch = "wasm32")]
fn in_worker(scene: &SkyScene, slot: &Slot) -> Option<web_sys::Worker> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    let worker = web_sys::Worker::new("./import_worker_loader.js").ok()?;
    let msg = js_sys::Array::new();
    msg.push(&SKY_VIS.into());
    msg.push(&js_sys::Float32Array::from(&scene.to_floats()[..]));
    let (w, done) = (worker.clone(), slot.clone());
    let t0 = crate::assets::now_ms();
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let data = e.data();
            if data.as_string().as_deref() == Some(crate::import_task::READY) {
                if w.post_message(&msg).is_err() {
                    w.terminate();
                    put(&done, Err(()));
                }
                return;
            }
            // The grid as floats; none: nothing blocks the sky.
            let grid = data
                .dyn_into::<js_sys::Float32Array>()
                .map_err(|_| ())
                .and_then(|a| {
                    let f = a.to_vec();
                    if f.is_empty() {
                        Ok(None)
                    } else {
                        SkyVisibility::from_floats(&f).map(Some).ok_or(())
                    }
                });
            crate::load_stats::add(
                "sky visibility bake (worker, wall)",
                crate::assets::now_ms() - t0,
            );
            put(&done, grid);
            w.terminate();
        });
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    let (w, failed) = (worker.clone(), slot.clone());
    let on_error = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| {
        w.terminate();
        put(&failed, Err(()));
    });
    worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    // shortcut: two small closures leak per bake, fine for a change of the design
    on_message.forget();
    on_error.forget();
    Some(worker)
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

    /// Starts baking `scene`, dropping (cancelling) the bake under way.
    pub fn start(&mut self, scene: SkyScene, now_ms: f64) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.started_ms = now_ms;
        let slot: Slot = Arc::default();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (out, stop) = (slot.clone(), cancel.clone());
            std::thread::spawn(move || {
                let grid = visioncraft_automation::SkyVisJob::new(scene).run();
                if !stop.load(Ordering::Relaxed) {
                    put(&out, Ok(grid));
                }
            });
            self.task = Some(Task {
                cancel,
                work: Work::Off(slot, None),
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let worker = in_worker(&scene, &slot);
            let work = if worker.is_some() {
                Work::Off(slot, Some(scene))
            } else {
                Work::Sliced(Some(SkyVisJob::new(scene)))
            };
            self.task = Some(Task {
                cancel,
                work,
                worker,
            });
        }
    }

    /// Moves the bake along (a slice on the web without a worker) and takes a finished grid
    /// (`Some(None)`: nothing blocks the sky).
    pub fn poll(&mut self, now_ms: f64) -> Option<Option<SkyVisibility>> {
        let task = self.task.as_mut()?;
        let grid = match &mut task.work {
            Work::Off(slot, scene) => {
                let got = slot.lock().ok()?.take()?;
                match got {
                    Ok(grid) => Some(grid),
                    Err(()) => {
                        // The worker failed: the window bakes it, a slice a frame.
                        #[cfg(target_arch = "wasm32")]
                        {
                            let job = scene.take().map(SkyVisJob::new);
                            task.work = Work::Sliced(job);
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        let _ = scene;
                        None
                    }
                }
            }
            #[cfg(target_arch = "wasm32")]
            Work::Sliced(slot) => {
                let t = crate::assets::now_ms();
                let mut done = false;
                while !done && crate::assets::now_ms() - t < SLICE_MS {
                    done = slot.as_mut()?.step(8);
                }
                crate::load_stats::add(
                    "sky visibility bake slice (main thread)",
                    crate::assets::now_ms() - t,
                );
                if done {
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
