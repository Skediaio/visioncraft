//! The asset store: library files (JSON indexes, GLB models, texture images, HDR skies,
//! thumbnails) loaded asynchronously by path relative to a library base, kept in memory.
//!
//! Where the bytes come from: on the web `fetch` from `./assets/` beside the page (the `?assets=`
//! query parameter or `window.VC_ASSETS` override it); natively a folder from `--assets <dir>`,
//! else `./assets`, else `assets` beside the executable, read on background threads.
//! Decoding happens off the frame: natively on those threads (the `image` crate); on the web the
//! browser decodes JPEG and PNG (`createImageBitmap` and a canvas), HDR is decoded by `image` in an
//! async task. Only the GPU upload runs on the frame, inside a time budget.
//!
//! What is wanted first is decided by a priority queue (a distance in metres, lower first); what
//! stays resident by an LRU over the renderer's `texture_bytes` against a cap.

use crate::gltf_load::{self, Glb};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use visioncraft_render::bc::{BcKind, Compressed};
use visioncraft_render::{ColorSpace, Renderer, TextureDesc, TextureId, UploadJob};

pub struct Rgba {
    pub w: u32,
    pub h: u32,
    /// The pixels; empty once `bc` holds them compressed.
    pub px: Vec<u8>,
    /// Block-compressed with its mips (natively, on a device with BC formats).
    pub bc: Option<Arc<Compressed>>,
}

impl Rgba {
    pub fn new(w: u32, h: u32, px: Vec<u8>) -> Self {
        Self { w, h, px, bc: None }
    }

    /// GPU memory once uploaded.
    pub fn gpu_bytes(&self) -> u64 {
        self.bc
            .as_ref()
            .map_or_else(|| texture_cost(self.w, self.h), |c| c.bytes())
    }
}

/// The BC kind of a library material map, by its file name (`materials/<id>/[<tier>/]<map>.jpg`):
/// colour BC1, packed occlusion-roughness-metallic BC1, normal BC5, height BC4.
pub fn bc_kind_of(path: &str) -> Option<BcKind> {
    if !path.starts_with("materials/") {
        return None;
    }
    let file = path.rsplit('/').next()?;
    Some(match file.split('.').next()? {
        "basecolor" | "orm" => BcKind::Rgb,
        "normal" => BcKind::Normal,
        "height" => BcKind::Gray,
        _ => return None,
    })
}

pub struct HdrData {
    pub w: u32,
    pub h: u32,
    pub rgb: Vec<f32>,
}

pub struct GlbData {
    pub glb: Glb,
    /// Decoded images, parallel to `glb.images`.
    pub images: Vec<Rgba>,
}

pub enum Loaded {
    Bytes(Vec<u8>),
    Image(Rgba),
    Glb(GlbData),
    Hdr(HdrData),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// Raw file (the JSON indexes).
    Bytes(String),
    /// A texture for the renderer; `srgb` and `repeat` are part of the identity.
    Image {
        path: String,
        srgb: bool,
        repeat: bool,
    },
    Glb(String),
    Hdr(String),
    /// A decoded image for the UI (thumbnails); never uploaded to the renderer.
    Thumb(String),
}

impl Key {
    pub fn path(&self) -> &str {
        match self {
            Key::Bytes(p) | Key::Glb(p) | Key::Hdr(p) | Key::Thumb(p) => p,
            Key::Image { path, .. } => path,
        }
    }
}

// ---- time ----

#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[cfg(target_arch = "wasm32")]
pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// A per-frame time allowance for GPU uploads. At least one upload always happens per frame.
pub struct Budget {
    start: f64,
    limit_ms: f64,
}

impl Budget {
    pub fn new(limit_ms: f64) -> Self {
        Self {
            start: now_ms(),
            limit_ms,
        }
    }

    pub fn spent(&self) -> bool {
        now_ms() - self.start >= self.limit_ms
    }
}

// ---- queue and LRU (pure) ----

/// What is wanted, nearest first. A key asked for twice keeps its smaller (more urgent) priority.
#[derive(Default)]
pub struct Queue {
    items: Vec<(Key, f32)>,
}

impl Queue {
    pub fn push(&mut self, key: Key, priority: f32) {
        match self.items.iter_mut().find(|e| e.0 == key) {
            Some(e) => e.1 = e.1.min(priority),
            None => self.items.push((key, priority)),
        }
    }

    /// The most urgent entry; equal priorities leave in the order they came.
    pub fn pop(&mut self) -> Option<(Key, f32)> {
        let best = (0..self.items.len())
            .min_by(|&a, &b| self.items[a].1.total_cmp(&self.items[b].1).then(a.cmp(&b)))?;
        Some(self.items.remove(best))
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.items.iter().any(|e| &e.0 == key)
    }

    fn remove_path(&mut self, path: &str) {
        self.items.retain(|e| e.0.path() != path);
    }
}

/// How urgent a model LOD is: its distance to the camera in metres. The coarsest level (what is
/// shown while a finer one loads) gets a head start over finer levels that are further away.
pub fn priority(distance_m: f32, lod: usize, coarsest: usize) -> f32 {
    let rank = if lod == coarsest { 0.0 } else { 20.0 };
    distance_m + rank + 5.0 * lod as f32
}

/// Resident things with their size and the tick they were last used.
#[derive(Default)]
pub struct Lru {
    entries: HashMap<Key, (u64, u64)>,
    pub total: u64,
}

impl Lru {
    pub fn insert(&mut self, key: Key, bytes: u64, tick: u64) {
        if let Some((old, _)) = self.entries.insert(key, (bytes, tick)) {
            self.total -= old;
        }
        self.total += bytes;
    }

    pub fn touch(&mut self, key: &Key, tick: u64) {
        if let Some(e) = self.entries.get_mut(key) {
            e.1 = tick;
        }
    }

    pub fn remove(&mut self, key: &Key) {
        if let Some((bytes, _)) = self.entries.remove(key) {
            self.total -= bytes;
        }
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.entries.contains_key(key)
    }

    fn bytes(&self, key: &Key) -> Option<u64> {
        self.entries.get(key).map(|e| e.0)
    }

    /// Least recently used entries whose path passes `which`, oldest first, until `to_free`
    /// bytes are covered, recent use or not.
    pub fn victims_where(&self, to_free: u64, which: impl Fn(&str) -> bool) -> Vec<Key> {
        let mut old: Vec<(&Key, &(u64, u64))> = self
            .entries
            .iter()
            .filter(|(k, _)| which(k.path()))
            .collect();
        old.sort_by_key(|(k, (_, t))| (*t, k.path().to_string()));
        let (mut freed, mut out) = (0, Vec::new());
        for (k, (bytes, _)) in old {
            if freed >= to_free {
                break;
            }
            freed += bytes;
            out.push(k.clone());
        }
        out
    }

    /// Least recently used entries, oldest first, until `to_free` bytes are covered. Entries used
    /// at `protect_from` or later are never chosen, so what is on screen stays.
    pub fn victims(&self, to_free: u64, protect_from: u64) -> Vec<Key> {
        let mut old: Vec<(&Key, &(u64, u64))> = self
            .entries
            .iter()
            .filter(|(_, (_, t))| *t < protect_from)
            .collect();
        old.sort_by_key(|(k, (_, t))| (*t, k.path().to_string()));
        let (mut freed, mut out) = (0, Vec::new());
        for (k, (bytes, _)) in old {
            if freed >= to_free {
                break;
            }
            freed += bytes;
            out.push(k.clone());
        }
        out
    }
}

/// Whether `path` is `tiers.json` or one of the higher-tier files it lists (`.../2k/...`,
/// `sky_4k.hdr`, `lod0_2k.glb`).
pub fn is_tier_file(path: &str) -> bool {
    path == "tiers.json"
        || ["/2k/", "/4k/", "_2k.", "_4k."]
            .iter()
            .any(|t| path.contains(t))
}

/// Whether `bytes` more fit under the cap with some room to spare (a higher tier is only asked
/// for then: eviction takes it back to the base tier, never below).
impl AssetStore {
    /// Bytes the store holds of files whose path passes `which`.
    pub fn held_bytes(&self, which: impl Fn(&str) -> bool) -> u64 {
        self.lru
            .entries
            .iter()
            .filter(|(k, _)| which(k.path()))
            .map(|(_, v)| v.0)
            .sum()
    }

    pub fn fits(&self, bytes: u64) -> bool {
        // The renderer's own count includes what the store does not hold (plant atlases,
        // impostors), so a tier never pushes the GPU over the cap.
        self.lru.total.max(self.texture_bytes) + bytes <= self.cap_bytes / 100 * 85
    }
}

/// Memory of an RGBA8 texture with its mip chain.
pub fn texture_cost(w: u32, h: u32) -> u64 {
    u64::from(w) * u64::from(h) * 4 * 4 / 3
}

// ---- input/output per platform ----

fn safe_path(path: &str) -> Result<(), String> {
    let bad = path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.split('/').any(|c| c == "..");
    if bad {
        Err(format!("`{path}` is not a library path"))
    } else {
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod io {
    use super::{Rgba, safe_path};
    use std::path::PathBuf;
    use std::sync::Arc;

    pub type Base = Arc<PathBuf>;

    /// `--assets <dir>`, else `./assets`, else `assets` beside the executable.
    pub fn base() -> Base {
        let args: Vec<String> = std::env::args().collect();
        let given = args
            .iter()
            .position(|a| a == "--assets")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from);
        let beside = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.join("assets")));
        let has_catalog = |p: &PathBuf| p.join("catalog.json").is_file();
        Arc::new(
            given
                .or_else(|| Some(PathBuf::from("assets")).filter(has_catalog))
                .or_else(|| beside.filter(has_catalog))
                .unwrap_or_else(|| PathBuf::from("assets")),
        )
    }

    pub fn describe(b: &Base) -> String {
        b.display().to_string()
    }

    pub async fn read(base: &Base, path: &str) -> Result<Vec<u8>, String> {
        safe_path(path)?;
        std::fs::read(base.join(path)).map_err(|e| format!("{path}: {e}"))
    }

    pub async fn decode(bytes: Vec<u8>) -> Result<Rgba, String> {
        let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
        let rgba = img.into_rgba8();
        Ok(Rgba::new(rgba.width(), rgba.height(), rgba.into_raw()))
    }

    pub fn spawn(job: impl std::future::Future<Output = ()> + Send + 'static) {
        std::thread::spawn(move || pollster::block_on(job));
    }
}

#[cfg(target_arch = "wasm32")]
mod io {
    use super::{Rgba, safe_path};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(inline_js = r#"
export function vc_base() {
  let b = new URLSearchParams(location.search).get('assets') || window.VC_ASSETS || './assets/';
  return b.endsWith('/') ? b : b + '/';
}
export async function vc_fetch(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(r.status + ' ' + r.statusText);
  return new Uint8Array(await r.arrayBuffer());
}
// The browser decodes (off the main thread); the pixels come back through a canvas.
export async function vc_decode(bytes) {
  const bmp = await createImageBitmap(new Blob([bytes]), { premultiplyAlpha: 'none', colorSpaceConversion: 'none' });
  const w = bmp.width, h = bmp.height;
  const c = typeof OffscreenCanvas !== 'undefined' ? new OffscreenCanvas(w, h) : Object.assign(document.createElement('canvas'), { width: w, height: h });
  const g = c.getContext('2d', { willReadFrequently: true });
  g.drawImage(bmp, 0, 0);
  bmp.close();
  return { w, h, data: g.getImageData(0, 0, w, h).data };
}
"#)]
    extern "C" {
        fn vc_base() -> String;
        #[wasm_bindgen(catch)]
        async fn vc_fetch(url: &str) -> Result<JsValue, JsValue>;
        #[wasm_bindgen(catch)]
        async fn vc_decode(bytes: js_sys::Uint8Array) -> Result<JsValue, JsValue>;
    }

    pub type Base = String;

    pub fn base() -> Base {
        vc_base()
    }

    pub fn describe(b: &Base) -> String {
        b.clone()
    }

    pub async fn read(base: &Base, path: &str) -> Result<Vec<u8>, String> {
        safe_path(path)?;
        let v = vc_fetch(&format!("{base}{path}"))
            .await
            .map_err(|e| format!("{path}: {}", js_err(&e)))?;
        Ok(js_sys::Uint8Array::new(&v).to_vec())
    }

    fn js_err(e: &JsValue) -> String {
        e.as_string()
            .or_else(|| {
                js_sys::Reflect::get(e, &"message".into())
                    .ok()
                    .and_then(|m| m.as_string())
            })
            .unwrap_or_else(|| "failed".into())
    }

    pub async fn decode(bytes: Vec<u8>) -> Result<Rgba, String> {
        let v = vc_decode(js_sys::Uint8Array::from(&bytes[..]))
            .await
            .map_err(|e| js_err(&e))?;
        let get = |k: &str| js_sys::Reflect::get(&v, &k.into()).map_err(|e| js_err(&e));
        let (w, h) = (
            get("w")?.as_f64().unwrap_or(0.0) as u32,
            get("h")?.as_f64().unwrap_or(0.0) as u32,
        );
        let px = js_sys::Uint8ClampedArray::new(&get("data")?).to_vec();
        if px.len() != (w * h * 4) as usize {
            return Err("the browser returned an image of the wrong size".into());
        }
        Ok(Rgba::new(w, h, px))
    }

    pub fn spawn(job: impl std::future::Future<Output = ()> + 'static) {
        wasm_bindgen_futures::spawn_local(job);
    }
}

/// Files that live in memory rather than under the library folder (objects a person imported,
/// read from a bundle), by path.
pub type MemoryFiles = Arc<Mutex<HashMap<String, Arc<[u8]>>>>;

/// Compresses on one loader thread at a time: four threads compressing 4k maps at once left
/// the frame's own thread waiting for a core (upload steps of 60-100 ms on a 4-core laptop).
fn compress_one(px: &[u8], w: u32, h: u32, kind: BcKind, space: ColorSpace) -> Option<Compressed> {
    static ONE: Mutex<()> = Mutex::new(());
    let _turn = ONE.lock().unwrap_or_else(|e| e.into_inner());
    visioncraft_render::bc::compress(px, w, h, kind, space)
}

/// `compress`: block-compress library material maps (natively, on a device with BC formats; the
/// loader runs on its own thread there, so the frame does not pay for it).
async fn load(
    key: &Key,
    base: &io::Base,
    memory: &MemoryFiles,
    compress: bool,
) -> Result<Loaded, String> {
    let held = memory.lock().ok().and_then(|m| m.get(key.path()).cloned());
    let bytes = match held {
        Some(b) => b.to_vec(),
        None => io::read(base, key.path()).await?,
    };
    Ok(match key {
        Key::Bytes(_) => Loaded::Bytes(bytes),
        Key::Image { path, srgb, .. } => {
            let mut img = io::decode(bytes).await?;
            if compress && let Some(kind) = bc_kind_of(path) {
                let space = if *srgb {
                    ColorSpace::Srgb
                } else {
                    ColorSpace::Linear
                };
                if let Some(c) = compress_one(&img.px, img.w, img.h, kind, space) {
                    img.bc = Some(Arc::new(c));
                    img.px = Vec::new();
                }
            }
            Loaded::Image(img)
        }
        Key::Thumb(_) => Loaded::Image(io::decode(bytes).await?),
        Key::Glb(_) => {
            let glb = gltf_load::parse(&bytes)?;
            let mut images = Vec::new();
            // Each picture compresses as what its material uses it for: colour BC1 (BC3 with
            // alpha), normal map BC5, occlusion-roughness-metallic BC1. One used two ways stays.
            let mut role: HashMap<usize, Option<(BcKind, ColorSpace)>> = HashMap::new();
            for m in &glb.materials {
                let uses = [
                    m.base_image.map(|i| (i, BcKind::Rgb, ColorSpace::Srgb)),
                    m.normal_image
                        .map(|i| (i, BcKind::Normal, ColorSpace::Linear)),
                    m.orm_image.map(|i| (i, BcKind::Rgb, ColorSpace::Linear)),
                ];
                for (i, kind, space) in uses.into_iter().flatten() {
                    let e = role.entry(i).or_insert(Some((kind, space)));
                    if *e != Some((kind, space)) {
                        *e = None;
                    }
                }
            }
            for (i, encoded) in glb.images.iter().enumerate() {
                let mut img = io::decode(encoded.clone()).await?;
                if compress && let Some(Some((mut kind, space))) = role.get(&i).copied() {
                    if kind == BcKind::Rgb
                        && space == ColorSpace::Srgb
                        && !img.px.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
                    {
                        kind = BcKind::Rgba;
                    }
                    if let Some(c) = compress_one(&img.px, img.w, img.h, kind, space) {
                        img.bc = Some(Arc::new(c));
                        img.px = Vec::new();
                    }
                }
                images.push(img);
            }
            Loaded::Glb(GlbData { glb, images })
        }
        Key::Hdr(_) => {
            let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Hdr)
                .map_err(|e| e.to_string())?
                .into_rgb32f();
            Loaded::Hdr(HdrData {
                w: img.width(),
                h: img.height(),
                rgb: img.into_raw(),
            })
        }
    })
}

// ---- the store ----

type Inbox = Arc<Mutex<Vec<(Key, Result<Loaded, String>)>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Absent,
    Pending,
    Ready,
    Failed,
}

/// Counters for the measurements and the debug overlay.
#[derive(Clone, Debug, Default)]
pub struct StoreStats {
    pub uploads: u32,
    pub worst_upload_ms: f64,
    /// What the worst upload was: size and whether compressed.
    pub worst_upload: String,
    pub total_upload_ms: f64,
    pub evictions: u32,
    pub peak_texture_bytes: u64,
}

pub struct AssetStore {
    base: io::Base,
    queue: Queue,
    inflight: HashSet<Key>,
    failed: HashSet<Key>,
    inbox: Inbox,
    pending_tex: Vec<(Key, f32, Rgba)>,
    /// Imported files, read before the library folder.
    pub memory: MemoryFiles,
    done: HashMap<Key, Loaded>,
    textures: HashMap<Key, TextureId>,
    lru: Lru,
    tick: u64,
    evicted: Vec<Key>,
    /// Bumps whenever the set of resident textures changes.
    pub rev: u64,
    /// New problems in plain words, for the toasts.
    pub messages: Vec<String>,
    /// GPU texture memory above which the least recently used things go.
    pub cap_bytes: u64,
    pub max_inflight: usize,
    pub stats: StoreStats,
    /// The GPU holds more texture memory than `cap_bytes` (as of the last frame).
    pub over_cap: bool,
    /// Paths `cancel` dropped, taken out of the queue at the next pump.
    cancelled: std::cell::RefCell<Vec<String>>,
    /// The compressed picture going up over frames: (key, job, its GPU bytes).
    streaming: Option<(Key, UploadJob, u64)>,
    /// Compress library material maps (the device has BC formats).
    pub compress: bool,
    /// The renderer's texture memory as of the last frame.
    pub texture_bytes: u64,
}

/// Bytes of a compressed picture written per frame (a 4k BC1 map with mips is about 11 MB, 44
/// frames): small writes keep each step to a few milliseconds.
const STREAM_BYTES_PER_FRAME: usize = 256 << 10;

/// Entries used within this many ticks (frames) are never evicted.
const PROTECT_TICKS: u64 = 90;

impl Default for AssetStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetStore {
    pub fn new() -> Self {
        Self {
            base: io::base(),
            queue: Queue::default(),
            inflight: HashSet::new(),
            failed: HashSet::new(),
            inbox: Inbox::default(),
            memory: MemoryFiles::default(),
            pending_tex: Vec::new(),
            done: HashMap::new(),
            textures: HashMap::new(),
            lru: Lru::default(),
            tick: 0,
            evicted: Vec::new(),
            rev: 0,
            messages: Vec::new(),
            cap_bytes: if cfg!(target_arch = "wasm32") {
                700 << 20
            } else {
                1536 << 20
            },
            max_inflight: if cfg!(target_arch = "wasm32") { 6 } else { 4 },
            stats: StoreStats::default(),
            over_cap: false,
            compress: false,
            streaming: None,
            cancelled: Default::default(),
            texture_bytes: 0,
        }
    }

    pub fn base_description(&self) -> String {
        io::describe(&self.base)
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Holds `bytes` as the file at `path` (an imported object): it loads from memory from now
    /// on, and a failed earlier attempt (the file was missing) is forgotten.
    pub fn provide(&mut self, path: &str, bytes: Arc<[u8]>) {
        if let Ok(mut m) = self.memory.lock() {
            m.insert(path.to_string(), bytes);
        }
        self.failed.retain(|k| k.path() != path);
    }

    /// The bytes of a file held in memory.
    pub fn provided(&self, path: &str) -> Option<Arc<[u8]>> {
        self.memory.lock().ok()?.get(path).cloned()
    }

    /// Drops a file asked for and not yet being fetched (a fetch under way finishes).
    pub fn cancel(&self, path: &str) {
        self.cancelled.borrow_mut().push(path.to_string());
    }

    /// Asks for a file; nothing happens when it is loaded, loading or has failed.
    pub fn want(&mut self, key: Key, priority: f32) {
        let have = self.done.contains_key(&key)
            || self.textures.contains_key(&key)
            || self.inflight.contains(&key)
            || self.failed.contains(&key)
            || self.lru.contains(&key);
        if !have {
            self.queue.push(key, priority);
        }
    }

    pub fn state(&self, key: &Key) -> State {
        if self.done.contains_key(key) || self.textures.contains_key(key) {
            State::Ready
        } else if self.failed.contains(key) {
            State::Failed
        } else if self.inflight.contains(key) || self.queue.contains(key) {
            State::Pending
        } else {
            State::Absent
        }
    }

    /// The renderer texture of a library image when it is resident (marking it used); else asks.
    pub fn texture(
        &mut self,
        path: &str,
        srgb: bool,
        repeat: bool,
        priority: f32,
    ) -> Option<TextureId> {
        let key = Key::Image {
            path: path.to_string(),
            srgb,
            repeat,
        };
        if let Some(id) = self.textures.get(&key) {
            self.lru.touch(&key, self.tick);
            return Some(*id);
        }
        self.want(key, priority);
        None
    }

    /// Takes a loaded file that is not a renderer texture (an index, a GLB, an HDR, a thumbnail).
    pub fn take(&mut self, key: &Key) -> Option<Loaded> {
        self.done.remove(key)
    }

    /// Whether a taken GLB or other owner-held resource is still counted (see `track`).
    pub fn track(&mut self, key: Key, bytes: u64) {
        self.lru.insert(key, bytes, self.tick);
    }

    pub fn touch(&mut self, key: &Key) {
        self.lru.touch(key, self.tick);
    }

    pub fn untrack(&mut self, key: &Key) {
        self.lru.remove(key);
    }

    /// Things the owner must release (GLB models evicted for memory).
    pub fn take_evicted(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.evicted)
    }

    pub fn busy(&self) -> bool {
        !self.queue.is_empty()
            || !self.inflight.is_empty()
            || !self.pending_tex.is_empty()
            || self.streaming.is_some()
    }

    pub fn queued(&self) -> usize {
        self.queue.len() + self.inflight.len() + self.pending_tex.len()
    }

    /// Uploads decoded pixels as a renderer texture; times the call into the statistics.
    pub fn upload(
        &mut self,
        r: &mut Renderer,
        img: &Rgba,
        srgb: bool,
        repeat: bool,
    ) -> Option<TextureId> {
        let t = now_ms();
        let id = match &img.bc {
            Some(c) => r.add_compressed(c.clone(), repeat).ok(),
            None => r
                .add_texture(&TextureDesc {
                    width: img.w,
                    height: img.h,
                    rgba8: &img.px,
                    color_space: if srgb {
                        ColorSpace::Srgb
                    } else {
                        ColorSpace::Linear
                    },
                    repeat,
                })
                .ok(),
        };
        let ms = now_ms() - t;
        self.stats.uploads += 1;
        self.stats.total_upload_ms += ms;
        if ms > self.stats.worst_upload_ms {
            self.stats.worst_upload = format!(
                "{}x{} {}",
                img.w,
                img.h,
                if img.bc.is_some() { "BC" } else { "RGBA8" }
            );
        }
        self.stats.worst_upload_ms = self.stats.worst_upload_ms.max(ms);
        id
    }

    /// Marks a key as failed for good and raises one message (none for thumbnails; the other
    /// indexes stay quiet when the catalogue itself is missing; at most four detailed ones).
    pub fn fail_key(&mut self, key: Key, why: &str) {
        let catalog_failed = self.failed.iter().any(|k| k.path() == "catalog.json");
        if key.path() == "catalog.json" {
            self.messages.push(format!(
                "No asset library found at {} ({why}). Objects and library materials are unavailable; start with --assets <dir> or open the page with ?assets=<url>.",
                self.base_description()
            ));
        } else if matches!(key, Key::Thumb(_))
            || (matches!(key, Key::Bytes(_)) && catalog_failed)
            || is_tier_file(key.path())
        {
            // A missing higher tier (or tiers.json) quietly falls back to the base files.
        } else if self.failed.len() < 4 {
            self.messages
                .push(format!("Could not load {}: {why}", key.path()));
        } else if self.failed.len() == 4 {
            self.messages
                .push("More library files could not be loaded".into());
        }
        self.failed.insert(key);
    }

    /// One frame of work: start loads (most urgent first), collect finished ones, evict above the
    /// cap, upload decoded textures within `budget`. Returns true when the resident textures
    /// changed (anything holding texture ids must look again).
    pub fn pump(&mut self, r: &mut Renderer, budget: &Budget) -> bool {
        self.tick += 1;
        for p in self.cancelled.get_mut().drain(..) {
            self.queue.remove_path(&p);
        }
        while self.inflight.len() < self.max_inflight {
            let Some((key, _)) = self.queue.pop() else {
                break;
            };
            self.inflight.insert(key.clone());
            let (inbox, base, memory) =
                (self.inbox.clone(), self.base.clone(), self.memory.clone());
            // shortcut: in the browser the loader runs on the page's thread, where a 4k map
            // takes seconds to compress, so the web keeps RGBA8; a worker would lift that
            let compress = self.compress && !cfg!(target_arch = "wasm32");
            io::spawn(async move {
                let result = load(&key, &base, &memory, compress).await;
                inbox.lock().expect("asset inbox").push((key, result));
            });
        }
        let arrived = std::mem::take(&mut *self.inbox.lock().expect("asset inbox"));
        for (key, result) in arrived {
            self.inflight.remove(&key);
            match result {
                Err(e) => self.fail_key(key, &e),
                Ok(Loaded::Image(img)) if matches!(key, Key::Image { .. }) => {
                    self.pending_tex.push((key, 0.0, img));
                }
                Ok(loaded) => {
                    self.done.insert(key, loaded);
                }
            }
        }
        let mut changed = false;
        let texture_bytes = r.stats().texture_bytes;
        self.texture_bytes = texture_bytes;
        self.stats.peak_texture_bytes = self.stats.peak_texture_bytes.max(texture_bytes);
        let over = texture_bytes.saturating_sub(self.cap_bytes);
        self.over_cap = over > 0;
        if over > 0 {
            // Higher tiers go first, even in view: the base files under them stay, so the
            // picture only gets coarser, never blank.
            let mut victims = self.lru.victims_where(over, is_tier_file);
            let freed: u64 = victims.iter().filter_map(|k| self.lru.bytes(k)).sum();
            if freed < over {
                let more: Vec<Key> = self
                    .lru
                    .victims(over - freed, self.tick.saturating_sub(PROTECT_TICKS))
                    .into_iter()
                    .filter(|k| !victims.contains(k))
                    .collect();
                victims.extend(more);
            }
            for key in victims {
                self.lru.remove(&key);
                self.stats.evictions += 1;
                if let Some(id) = self.textures.remove(&key) {
                    r.remove_texture(id);
                    changed = true;
                } else {
                    self.evicted.push(key);
                }
            }
        }
        // Nothing new goes up while over the cap and nothing could be freed: no thrashing.
        let room = over == 0 || changed || !self.evicted.is_empty();
        // A compressed picture goes up a few megabytes per frame (a 4k map in one write cost
        // the frame up to 100 ms); only once complete does anything use it.
        if let Some((key, job, cost)) = &mut self.streaming {
            let t = now_ms();
            let done = r.upload_step(job, STREAM_BYTES_PER_FRAME);
            let ms = now_ms() - t;
            self.stats.worst_upload_ms = self.stats.worst_upload_ms.max(ms);
            match done {
                None => {}
                Some(result) => {
                    let (key, cost) = (key.clone(), *cost);
                    self.streaming = None;
                    self.stats.uploads += 1;
                    match result {
                        Ok(id) => {
                            self.lru.insert(key.clone(), cost, self.tick);
                            self.texture_bytes += cost;
                            self.textures.insert(key, id);
                            changed = true;
                        }
                        Err(e) => self.fail_key(key, &e),
                    }
                }
            }
        }
        while room && self.streaming.is_none() && !self.pending_tex.is_empty() {
            let (key, _, img) = self.pending_tex.swap_remove(0);
            let Key::Image { srgb, repeat, .. } = key else {
                continue;
            };
            // A higher tier that no longer fits (the base files filled the budget meanwhile)
            // is dropped, not uploaded: the base stays, it may be asked for again later.
            if is_tier_file(key.path()) && !self.fits(img.gpu_bytes()) {
                continue;
            }
            if let Some(c) = &img.bc {
                self.streaming = Some((key, UploadJob::new(c.clone(), repeat), img.gpu_bytes()));
                break;
            }
            match self.upload(r, &img, srgb, repeat) {
                Some(id) => {
                    let cost = img.gpu_bytes();
                    self.lru.insert(key.clone(), cost, self.tick);
                    self.texture_bytes += cost;
                    self.textures.insert(key, id);
                    changed = true;
                }
                None => self.fail_key(key, "the GPU refused the texture"),
            }
            if budget.spent() {
                break;
            }
        }
        if changed {
            self.rev += 1;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(p: &str) -> Key {
        Key::Glb(p.into())
    }

    #[test]
    fn tier_files_are_told_apart_from_base_files() {
        for p in [
            "tiers.json",
            "materials/bricks028/2k/basecolor.jpg",
            "materials/bricks028/4k/orm.jpg",
            "skies/noon/sky_4k.hdr",
            "models/bench/lod0_2k.glb",
        ] {
            assert!(super::is_tier_file(p), "{p}");
        }
        for p in [
            "materials/bricks028/basecolor.jpg",
            "skies/noon/sky_1k.hdr",
            "catalog.json",
        ] {
            assert!(!super::is_tier_file(p), "{p}");
        }
    }

    #[test]
    fn queue_serves_the_nearest_first_and_keeps_the_smaller_priority() {
        let mut q = Queue::default();
        q.push(k("far"), 90.0);
        q.push(k("near"), 3.0);
        q.push(k("mid"), 20.0);
        q.push(k("far"), 1.0); // asked again, now urgent
        q.push(k("mid"), 50.0); // asked again, less urgent: ignored
        assert_eq!(q.len(), 3);
        let order: Vec<String> = std::iter::from_fn(|| q.pop())
            .map(|(k, _)| k.path().to_string())
            .collect();
        assert_eq!(order, ["far", "near", "mid"]);
    }

    #[test]
    fn queue_keeps_arrival_order_among_equals() {
        let mut q = Queue::default();
        for p in ["a", "b", "c"] {
            q.push(k(p), 1.0);
        }
        assert_eq!(q.pop().unwrap().0.path(), "a");
        assert_eq!(q.pop().unwrap().0.path(), "b");
    }

    #[test]
    fn coarse_levels_beat_finer_ones_at_the_same_distance() {
        assert!(priority(10.0, 2, 2) < priority(10.0, 0, 2));
        assert!(priority(2.0, 0, 2) < priority(60.0, 0, 2));
    }

    #[test]
    fn lru_evicts_oldest_first_and_spares_the_recent() {
        let mut l = Lru::default();
        l.insert(k("a"), 100, 1);
        l.insert(k("b"), 100, 2);
        l.insert(k("c"), 100, 50);
        l.touch(&k("a"), 3);
        assert_eq!(l.total, 300);
        // free 150 bytes, protecting everything used at tick 50 or later
        assert_eq!(l.victims(150, 50), vec![k("b"), k("a")]);
        // 100 is enough with one
        assert_eq!(l.victims(100, 50), vec![k("b")]);
        // nothing is evictable when everything is recent
        assert!(l.victims(100, 0).is_empty());
        l.remove(&k("b"));
        assert_eq!(l.total, 200);
        l.insert(k("a"), 10, 4); // re-inserting replaces the size
        assert_eq!(l.total, 110);
    }

    #[test]
    fn library_paths_cannot_escape() {
        assert!(safe_path("models/a/lod0.glb").is_ok());
        for bad in ["../x", "/etc/passwd", "a/../../b", "C:/x", "a\\b"] {
            assert!(safe_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn texture_cost_counts_the_mip_chain() {
        assert_eq!(texture_cost(1024, 1024), 1024 * 1024 * 4 * 4 / 3);
    }
}
