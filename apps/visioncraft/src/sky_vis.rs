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

/// The layout of a stored grid; bump it when `SkyVisibility` or this encoding changes.
const CACHE_FORMAT: u8 = 1;

/// The cache key of the bake of the scene with key `scene` (`SkyScene::key`).
pub fn cache_key(scene: u64) -> u64 {
    crate::plant_cache::key(&[b"sky visibility", &scene.to_le_bytes()])
}

/// A grid as bytes: the format, a checksum and length of the body, then the body (the header
/// numbers, then the arrays, little-endian floats, so the round trip is bit-exact).
pub fn encode(v: &SkyVisibility) -> Vec<u8> {
    let mut body = Vec::new();
    let f = |b: &mut Vec<u8>, xs: &[f32]| xs.iter().for_each(|x| b.extend(x.to_le_bytes()));
    f(&mut body, &v.origin);
    f(&mut body, &[v.cell]);
    v.dims.iter().for_each(|d| body.extend(d.to_le_bytes()));
    f(&mut body, &[v.lift, v.height]);
    f(&mut body, &v.ground);
    v.sh.iter().for_each(|p| f(&mut body, p));
    f(&mut body, &v.valid);
    f(&mut body, &v.albedo);
    let mut out = vec![CACHE_FORMAT];
    out.extend((body.len() as u64).to_le_bytes());
    out.extend(crate::plant_cache::digest(&body));
    out.extend(body);
    out
}

/// The grid in `bytes`; `None` for another format, a wrong length or checksum, or sizes that
/// do not fit the grid's dimensions.
pub fn decode(bytes: &[u8]) -> Option<SkyVisibility> {
    let (&format, rest) = bytes.split_first()?;
    (format == CACHE_FORMAT).then_some(())?;
    let len = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?);
    let sum = rest.get(8..16)?;
    let body = rest.get(16..)?;
    (body.len() as u64 == len && crate::plant_cache::digest(body)[..] == *sum).then_some(())?;
    let mut at = 0usize;
    let mut take = |n: usize| {
        let s = body.get(at..at.checked_add(n.checked_mul(4)?)?)?;
        at += n * 4;
        Some(s)
    };
    let floats = |s: &[u8]| -> Vec<f32> {
        s.as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect()
    };
    // The header: origin x, z, cell, dims (three u32), lift, height.
    let words = take(8)?.as_chunks::<4>().0;
    let fl = |i: usize| f32::from_le_bytes(words[i]);
    let un = |i: usize| u32::from_le_bytes(words[i]);
    let dims = [un(3), un(4), un(5)];
    let columns = (dims[0] as usize).checked_mul(dims[2] as usize)?;
    let probes = columns.checked_mul(dims[1] as usize)?;
    let ground = floats(take(columns)?);
    let sh: Vec<[f32; 9]> = floats(take(probes.checked_mul(9)?)?)
        .as_chunks::<9>()
        .0
        .to_vec();
    let valid = floats(take(probes)?);
    let albedo = floats(take(probes)?);
    (at == body.len()).then_some(())?;
    Some(SkyVisibility {
        origin: [fl(0), fl(1)],
        cell: fl(2),
        dims,
        lift: fl(6),
        height: fl(7),
        ground,
        sh,
        valid,
        albedo,
    })
}

enum Work {
    /// A thread natively, the import worker on the web (with the scene, should it fail).
    Off(Slot, Option<SkyScene>),
    #[cfg(target_arch = "wasm32")]
    Sliced(Option<SkyVisJob>),
    /// The browser's cache is being asked; the bake follows when it has no entry.
    #[cfg(target_arch = "wasm32")]
    Lookup(LookupSlot, Option<SkyScene>),
    /// No cache entry: the bake waits for `SkyVisState::hold` to end.
    Parked(Option<SkyScene>),
}

/// The browser cache's answer: `Some(None)` is a miss.
#[cfg(target_arch = "wasm32")]
type LookupSlot = Arc<Mutex<Option<Option<SkyVisibility>>>>;

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
    /// Set by the window while plants are still being made: a bake that missed the cache waits
    /// (a cache hit needs no cores, so it is never held).
    pub hold: bool,
    /// The cache key to store the running bake's grid under.
    store: Option<u64>,
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

    /// Gets the grid of `scene`: from the cache when it has it (the disk natively, the browser's
    /// cache on the web; no workers), else baked (`hold` delays that), dropping (cancelling) the
    /// work under way. A baked grid is stored when it is done.
    pub fn start(&mut self, scene: SkyScene, now_ms: f64) {
        self.started_ms = now_ms;
        self.store = None;
        let key = cache_key(scene.key());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let got = crate::plant_cache::disk()
                .and_then(|d| d.read("skyvis", key))
                .and_then(|b| decode(&b));
            let what = if got.is_some() { "hit" } else { "miss" };
            crate::load_stats::add(&format!("disk cache {what} (skyvis)"), 0.0);
            if let Some(v) = got {
                let slot: Slot = Arc::new(Mutex::new(Some(Ok(Some(v)))));
                self.task = Some(Task {
                    cancel: Arc::default(),
                    work: Work::Off(slot, None),
                });
                return;
            }
            self.after_miss(scene, key);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let slot: LookupSlot = Arc::default();
            let out = slot.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let got = crate::web_cache::get("skyvis", key)
                    .await
                    .and_then(|b| decode(&b));
                if let Ok(mut s) = out.lock() {
                    *s = Some(got);
                }
            });
            self.store = Some(key);
            self.task = Some(Task {
                cancel: Arc::default(),
                work: Work::Lookup(slot, Some(scene)),
                _run: None,
            });
        }
    }

    /// No cache entry: bakes now, or parks the scene while plants are made.
    fn after_miss(&mut self, scene: SkyScene, key: u64) {
        self.store = Some(key);
        if self.hold {
            self.task = Some(Task {
                cancel: Arc::default(),
                work: Work::Parked(Some(scene)),
                #[cfg(target_arch = "wasm32")]
                _run: None,
            });
        } else {
            self.bake(scene);
        }
    }

    /// Starts baking `scene`.
    fn bake(&mut self, scene: SkyScene) {
        let cancel = Arc::new(AtomicBool::new(false));
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
            Work::Parked(scene) => {
                if self.hold {
                    return None;
                }
                let scene = scene.take()?;
                // `store` still holds the key `after_miss` kept.
                self.bake(scene);
                return None;
            }
            #[cfg(target_arch = "wasm32")]
            Work::Lookup(slot, scene) => {
                let got = slot.lock().ok()?.take()?;
                match got {
                    Some(v) => {
                        self.store = None;
                        Some(Some(v))
                    }
                    None => {
                        let scene = scene.take()?;
                        let key = self.store.take()?;
                        self.after_miss(scene, key);
                        return None;
                    }
                }
            }
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
        if let (Some(v), Some(key)) = (&grid, self.store.take()) {
            let bytes = encode(v);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(d) = crate::plant_cache::disk() {
                d.write("skyvis", key, &bytes);
            }
            #[cfg(target_arch = "wasm32")]
            crate::web_cache::put("skyvis", key, &bytes);
        }
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
        self.store = None;
        self.inputs = None;
        self.scene = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> SkyVisibility {
        let (cols, probes) = (6usize, 12usize);
        let f = |i: usize, k: f32| (i as f32 * 0.37 + k).sin();
        SkyVisibility {
            origin: [-3.5, 1.25],
            cell: 0.75,
            dims: [3, 2, 2],
            lift: 0.4,
            height: 9.0,
            ground: (0..cols).map(|i| f(i, 1.0)).collect(),
            sh: (0..probes)
                .map(|i| std::array::from_fn(|j| f(i * 9 + j, 2.0)))
                .collect(),
            valid: (0..probes).map(|i| (i % 2) as f32).collect(),
            albedo: (0..probes).map(|i| f(i, 3.0).abs()).collect(),
        }
    }

    #[test]
    fn round_trip_is_bit_exact() {
        let g = grid();
        assert_eq!(decode(&encode(&g)), Some(g));
    }

    #[test]
    fn truncated_or_corrupt_entries_are_misses() {
        let bytes = encode(&grid());
        for n in [0, 1, 8, 16, 17, bytes.len() / 2, bytes.len() - 1] {
            assert_eq!(decode(&bytes[..n]), None, "truncated to {n}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(decode(&longer), None);
        for at in [0, 3, 9, 20, bytes.len() / 2, bytes.len() - 1] {
            let mut b = bytes.clone();
            b[at] ^= 0x55;
            assert_eq!(decode(&b), None, "byte {at} damaged");
        }
    }

    #[test]
    fn the_key_follows_the_scene_key() {
        assert_ne!(cache_key(1), cache_key(2));
        assert_eq!(cache_key(7), cache_key(7));
    }
}
