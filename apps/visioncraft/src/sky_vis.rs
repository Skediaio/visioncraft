//! The baked sky visibility in the window and its pictures (`view.set sky_occlusion`,
//! `visioncraft_core::sky_visibility`): baked off the UI thread when the design changes, then
//! drawn every frame with the sky as it is.
//!
//! Baking: `visioncraft_automation::SkyVisJob`, the scene `query.sky_view` traces. Natively a
//! thread runs it on all cores (the example garden's 13,104 probes in about 0.4 s on eight cores); on
//! the web in several import workers (`skyvis` jobs for chunks of the probes: the scene and the probes
//! travel as floats, `SkyScene::to_floats`, `SkyBake::probe_floats`), so the page keeps its frame rate. When the worker cannot start, the
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
    /// The workers' run; dropping it stops them.
    #[cfg(target_arch = "wasm32")]
    _run: Option<std::rc::Rc<std::cell::RefCell<Run>>>,
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn put(slot: &Slot, v: Result<Option<SkyVisibility>, ()>) {
    if let Ok(mut s) = slot.lock() {
        s.get_or_insert(v);
    }
}

/// A bake over several import workers: the probe range is cut into chunks, each worker takes the
/// next chunk when it answers (so a slow region does not leave one worker last), and the answers
/// are put together in order.
#[cfg(target_arch = "wasm32")]
struct Run {
    job: Option<SkyVisJob>,
    /// The scene as floats, sent with a worker's first chunk only.
    scene: Vec<f32>,
    workers: Vec<web_sys::Worker>,
    /// Per worker: whether it has the scene, and the chunk it is on.
    has_scene: Vec<bool>,
    on: Vec<usize>,
    chunks: Vec<(usize, usize)>,
    next: usize,
    results: Vec<Option<Vec<f32>>>,
    left: usize,
    done: bool,
    slot: Slot,
}

#[cfg(target_arch = "wasm32")]
impl Run {
    fn stop(&mut self) {
        self.done = true;
        self.workers.iter().for_each(|w| w.terminate());
    }

    fn fail(&mut self) {
        self.stop();
        put(&self.slot, Err(()));
    }

    /// Gives worker `w` the next chunk, if any.
    fn feed(&mut self, w: usize) {
        let Some(&(a, b)) = self.chunks.get(self.next) else {
            return;
        };
        let scene = if self.has_scene[w] {
            js_sys::Float32Array::new_with_length(0)
        } else {
            js_sys::Float32Array::from(&self.scene[..])
        };
        let msg = js_sys::Array::of4(
            &SKY_VIS.into(),
            &scene,
            &(a as f64).into(),
            &(b as f64).into(),
        );
        if self.workers[w].post_message(&msg).is_err() {
            return self.fail();
        }
        self.has_scene[w] = true;
        self.on[w] = self.next;
        self.next += 1;
    }

    /// Worker `w` answered with the probes of its chunk.
    fn answer(&mut self, w: usize, floats: Vec<f32>) {
        let c = self.on[w];
        let (a, b) = self.chunks[c];
        // A chunk is never empty: an empty answer is a worker that could not bake it.
        if floats.len() != (b - a) * visioncraft_core::sky_visibility::PROBE_FLOATS {
            return self.fail();
        }
        self.results[c] = Some(floats);
        self.left -= 1;
        if self.left > 0 {
            return self.feed(w);
        }
        self.stop();
        let grid = self.job.take().and_then(|mut job| {
            let all = self.results.drain(..).flatten().all(|f| job.fill(&f));
            all.then(|| job.finish())
        });
        put(&self.slot, grid.ok_or(()));
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for Run {
    fn drop(&mut self) {
        self.workers.iter().for_each(|w| w.terminate());
    }
}

/// Workers that bake the sky visibility: the cores less two for the page and the GPU, two to
/// eight (the texture workers run beside them).
#[cfg(target_arch = "wasm32")]
fn bake_workers() -> usize {
    let n = js_sys::Reflect::get(&js_sys::global(), &"navigator".into())
        .and_then(|nav| js_sys::Reflect::get(&nav, &"hardwareConcurrency".into()))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(4.0) as usize;
    n.saturating_sub(2).clamp(2, 8)
}

/// Bakes `scene` over import workers; the grid lands in `slot` (`Err` when a worker fails). `None`
/// when no worker starts. The bake stops when the returned run is dropped.
#[cfg(target_arch = "wasm32")]
fn in_workers(scene: &SkyScene, slot: &Slot) -> Option<std::rc::Rc<std::cell::RefCell<Run>>> {
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    let job = SkyVisJob::new(scene.clone());
    let total = job.progress().1;
    if total == 0 {
        // Nothing blocks the sky.
        put(slot, Ok(None));
        return Some(Rc::new(RefCell::new(Run {
            job: None,
            scene: Vec::new(),
            workers: Vec::new(),
            has_scene: Vec::new(),
            on: Vec::new(),
            chunks: Vec::new(),
            next: 0,
            results: Vec::new(),
            left: 0,
            done: true,
            slot: slot.clone(),
        })));
    }
    let k = bake_workers();
    let size = total.div_ceil(4 * k).max(1);
    let chunks: Vec<(usize, usize)> = (0..total)
        .step_by(size)
        .map(|a| (a, (a + size).min(total)))
        .collect();
    let workers: Vec<web_sys::Worker> = (0..k.min(chunks.len()))
        .map(|_| web_sys::Worker::new("./import_worker_loader.js").ok())
        .collect::<Option<_>>()?;
    let n = workers.len();
    let run = Rc::new(RefCell::new(Run {
        job: Some(job),
        scene: scene.to_floats(),
        workers: workers.clone(),
        has_scene: vec![false; n],
        on: vec![0; n],
        results: vec![None; chunks.len()],
        left: chunks.len(),
        chunks,
        next: 0,
        done: false,
        slot: slot.clone(),
    }));
    for (w, worker) in workers.iter().enumerate() {
        // The closures hold the run weakly: dropping it (the bake cancelled) ends the bake.
        let weak = Rc::downgrade(&run);
        let on_message =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                let Some(run) = weak.upgrade() else { return };
                let mut run = run.borrow_mut();
                if run.done {
                    return;
                }
                let data = e.data();
                if data.as_string().as_deref() == Some(crate::import_task::READY) {
                    return run.feed(w);
                }
                match data.dyn_into::<js_sys::Float32Array>() {
                    Ok(a) => run.answer(w, a.to_vec()),
                    Err(_) => run.fail(),
                }
            });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        let weak = Rc::downgrade(&run);
        let on_error = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| {
            if let Some(run) = weak.upgrade() {
                let mut run = run.borrow_mut();
                if !run.done {
                    run.fail();
                }
            }
        });
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        // shortcut: two small closures leak per worker per bake, fine for a change of the design
        on_message.forget();
        on_error.forget();
    }
    Some(run)
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
                // The work pool's share of the cores: the frame's thread keeps one (on all of
                // them a long bake, such as the coast's, starved the frames).
                let grid =
                    visioncraft_automation::SkyVisJob::new(scene).run_on(crate::work_pool::size());
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
            let run = in_workers(&scene, &slot);
            let work = if run.is_some() {
                Work::Off(slot, Some(scene))
            } else {
                Work::Sliced(Some(SkyVisJob::new(scene)))
            };
            self.task = Some(Task {
                cancel,
                work,
                _run: run,
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
        crate::load_stats::add(
            "sky visibility bake (off the frame, wall)",
            now_ms - self.started_ms,
        );
        Some(grid)
    }

    /// Forgets the grid and the bake (another project).
    pub fn clear(&mut self) {
        self.task = None;
        self.inputs = None;
        self.scene = None;
    }
}
