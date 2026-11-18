//! Block compression off the page's thread on the web: a few Web Workers (`web_workers`, the
//! `texture_worker` binary, the same crate built again, see `index.html`) compress decoded
//! pictures with `visioncraft_render::bc` and send the mip levels back as bytes; a job goes to
//! the worker with the fewest waiting. Natively the work pool compresses (`assets`,
//! `work_pool`), so this is wasm-only. When no worker can start, pictures stay RGBA8 as before.
//!
//! The workers also make a plant species' textures (`["plant", params, scale]` jobs): the leaf
//! atlas, its normal map and the bark, generated and compressed there instead of on the page
//! (natively `plant_compressed` runs on the work pool, see `plants`), and a plant variant's
//! meshes (`["mesh", params, variant, month, detail]` jobs, answered in the disk cache's layout,
//! `plant_cache::encode_levels`): 250 to 500 ms each in wasm, which froze the page for minutes
//! on a large garden when they ran there; and a lawn's tufts (`["lawn", job, shape]` jobs,
//! `scene::scatter_lawn`).

/// Web Workers the page starts (each its own copy of the module): plant textures of several
/// species and maps compress side by side, while the page keeps its thread. The machine's
/// threads (`navigator.hardwareConcurrency`) less two for the page and the browser, from 3 to
/// 8: a fixed three left a desktop's cores idle while a garden's species were made one by one.
pub fn web_workers() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        let n = js_sys::Reflect::get(&js_sys::global(), &"navigator".into())
            .and_then(|nav| js_sys::Reflect::get(&nav, &"hardwareConcurrency".into()))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(4.0) as usize;
        n.saturating_sub(2).clamp(3, 8)
    }
    #[cfg(not(target_arch = "wasm32"))]
    3
}

/// What the worker posts once it can take jobs.
pub const READY: &str = "ready";

pub fn kind_name(k: visioncraft_render::bc::BcKind) -> &'static str {
    use visioncraft_render::bc::BcKind;
    match k {
        BcKind::Rgb => "rgb",
        BcKind::Rgba => "rgba",
        BcKind::Normal => "normal",
        BcKind::Gray => "gray",
    }
}

pub fn kind_of(s: &str) -> Option<visioncraft_render::bc::BcKind> {
    use visioncraft_render::bc::BcKind;
    Some(match s {
        "rgb" => BcKind::Rgb,
        "rgba" => BcKind::Rgba,
        "normal" => BcKind::Normal,
        "gray" => BcKind::Gray,
        _ => return None,
    })
}

/// A species' textures in the order the page uploads them, with how each is compressed and
/// whether it repeats: atlas, its normal map (finer atlases only), bark, bark normal map.
pub fn plant_images(
    t: &visioncraft_flora::PlantTextures,
) -> Vec<(
    &visioncraft_flora::Image,
    visioncraft_render::bc::BcKind,
    bool,
    bool,
)> {
    use visioncraft_render::bc::BcKind;
    let mut v = vec![(&t.atlas, BcKind::Rgba, true, false)];
    if let Some(n) = &t.atlas_normal {
        v.push((n, BcKind::Normal, false, false));
    }
    v.push((&t.bark, BcKind::Rgb, true, true));
    v.push((&t.bark_normal, BcKind::Normal, false, true));
    v
}

/// What a plant job answers before the levels: the leaf colour and, per image, its size,
/// kind, colour space, whether it repeats and how many levels follow.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PlantMeta {
    pub leaf_mean: [f32; 3],
    pub images: Vec<(u32, u32, String, bool, bool, usize)>,
}

/// A species' textures made and compressed: the leaf colour and each image with whether it
/// repeats, in `plant_images` order. Natively on the work pool, on the web in the worker.
pub type PlantCompressed = (
    [f32; 3],
    Vec<(std::sync::Arc<visioncraft_render::bc::Compressed>, bool)>,
);

pub fn plant_compressed(
    params: &visioncraft_flora::PlantParams,
    scale: usize,
    scans: &visioncraft_flora::scan::Scans,
) -> Option<PlantCompressed> {
    use visioncraft_render::ColorSpace;
    let t = crate::load_stats::time("plant texture composition", || {
        visioncraft_flora::textures_scanned(params, scale, scans)
    });
    // The images compress side by side (natively; each is the same however it is scheduled).
    let compressed = crate::work_pool::map(&plant_images(&t), |&(img, kind, srgb, repeat)| {
        let space = if srgb {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        };
        let c = crate::load_stats::time("BC compression (plant)", || {
            visioncraft_render::bc::compress(&img.rgba, img.width, img.height, kind, space)
        })?;
        Some((std::sync::Arc::new(c), repeat))
    });
    let images = compressed.into_iter().collect::<Option<Vec<_>>>()?;
    Some((crate::plants::opaque_mean(&t.atlas.rgba), images))
}

/// A plant job in the worker: the meta data and every image's levels in turn.
pub fn plant_job(params: &str, scale: usize, scans: &[u8]) -> Option<(PlantMeta, Vec<Vec<u8>>)> {
    let params: visioncraft_flora::PlantParams = serde_json::from_str(params).ok()?;
    let scans = if scans.is_empty() {
        visioncraft_flora::scan::Scans::default()
    } else {
        visioncraft_flora::scan::Scans::unpack(scans)?
    };
    let (leaf_mean, images) = plant_compressed(&params, scale, &scans)?;
    let mut meta = PlantMeta {
        leaf_mean,
        images: Vec::new(),
    };
    let mut levels = Vec::new();
    for (c, repeat) in images {
        meta.images.push((
            c.width,
            c.height,
            kind_name(c.kind).into(),
            c.color_space == visioncraft_render::ColorSpace::Srgb,
            repeat,
            c.levels.len(),
        ));
        levels.extend(c.levels.iter().cloned());
    }
    Some((meta, levels))
}

/// A mesh job in the worker: one variant's levels, encoded as the disk cache stores them.
pub fn mesh_job(params: &str, variant: u32, month: u8, detail: bool) -> Option<Vec<u8>> {
    let params: visioncraft_flora::PlantParams = serde_json::from_str(params).ok()?;
    let g = crate::plants::generate_variant_now("", &params, variant, month, detail);
    Some(crate::plant_cache::encode_levels(g.height, &g.levels))
}

/// A lawn job in the worker (`scene::scatter_lawn`): the tufts' matrices as floats, every
/// variant's in turn, and how many each has.
pub fn lawn_job(job: &str, geom: &[u8]) -> Option<(Vec<f32>, Vec<u32>)> {
    let job: crate::scene::LawnScatter = serde_json::from_str(job).ok()?;
    let geom = crate::import_task::decode(geom).ok()?;
    let lists = crate::scene::scatter_lawn(&geom, &job);
    let counts = lists.iter().map(|l| l.len() as u32).collect();
    let floats = lists
        .iter()
        .flatten()
        .flat_map(|m| m.to_cols_array())
        .collect();
    Some((floats, counts))
}

/// Floats as little-endian bytes (the worker's answers are byte arrays).
pub fn bytes_of(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn bytes_of_u32(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn f32s(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

pub fn u32s(b: &[u8]) -> Vec<u32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

/// `lawn_job`'s answer back into one matrix list per variant.
pub fn lawn_lists(floats: &[f32], counts: &[u32]) -> Option<Vec<Vec<glam::Mat4>>> {
    let mut m = floats
        .as_chunks::<16>()
        .0
        .iter()
        .map(glam::Mat4::from_cols_array);
    let lists: Vec<Vec<glam::Mat4>> = counts
        .iter()
        .map(|&n| m.by_ref().take(n as usize).collect())
        .collect();
    let complete = lists
        .iter()
        .zip(counts)
        .all(|(l, &n)| l.len() == n as usize);
    (complete && m.next().is_none()).then_some(lists)
}

/// The worker's side: one job in, its answer out.
#[cfg(target_arch = "wasm32")]
pub fn answer(msg: &js_sys::Array) -> wasm_bindgen::JsValue {
    use visioncraft_render::ColorSpace;
    let out = js_sys::Array::new();
    out.push(&msg.get(0));
    if msg.get(1).as_string().as_deref() == Some("lawn") {
        let job = msg.get(2).as_string().unwrap_or_default();
        let geom = js_sys::Uint8Array::new(&msg.get(3)).to_vec();
        match lawn_job(&job, &geom) {
            Some((floats, counts)) => {
                out.push(&"lawn".into());
                out.push(&js_sys::Uint8Array::from(&bytes_of(&floats)[..]));
                out.push(&js_sys::Uint8Array::from(&bytes_of_u32(&counts)[..]));
            }
            None => {
                out.push(&"error: cannot scatter the lawn".into());
            }
        }
        return out.into();
    }
    if msg.get(1).as_string().as_deref() == Some("mesh") {
        let num = |i: u32| msg.get(i).as_f64().unwrap_or(0.0);
        let params = msg.get(2).as_string().unwrap_or_default();
        match mesh_job(&params, num(3) as u32, num(4) as u8, num(5) != 0.0) {
            Some(b) => {
                out.push(&"mesh".into());
                out.push(&js_sys::Uint8Array::from(&b[..]));
            }
            None => {
                out.push(&"error: cannot make the plant meshes".into());
            }
        }
        return out.into();
    }
    if msg.get(1).as_string().as_deref() == Some("plant") {
        let params = msg.get(2).as_string().unwrap_or_default();
        let scale = msg.get(3).as_f64().unwrap_or(1.0) as usize;
        let scans = js_sys::Uint8Array::new(&msg.get(4)).to_vec();
        match plant_job(&params, scale, &scans) {
            Some((meta, levels)) => {
                out.push(&serde_json::to_string(&meta).unwrap_or_default().into());
                for l in &levels {
                    out.push(&js_sys::Uint8Array::from(&l[..]));
                }
            }
            None => {
                out.push(&"error: cannot make the plant textures".into());
            }
        }
        return out.into();
    }
    let num = |i: u32| msg.get(i).as_f64().unwrap_or(0.0) as u32;
    let (w, h) = (num(1), num(2));
    let kind = msg.get(3).as_string().and_then(|k| kind_of(&k));
    let space = if msg.get(4).as_bool().unwrap_or(true) {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    let px = js_sys::Uint8Array::new(&msg.get(5)).to_vec();
    match kind.and_then(|k| visioncraft_render::bc::compress(&px, w, h, k, space)) {
        Some(c) => {
            for l in &c.levels {
                out.push(&js_sys::Uint8Array::from(&l[..]));
            }
        }
        None => {
            out.push(&"error: not compressible".into());
        }
    }
    out.into()
}

#[cfg(target_arch = "wasm32")]
mod page {
    use super::{READY, kind_name};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;
    use visioncraft_render::ColorSpace;
    use visioncraft_render::bc::{BcKind, Compressed};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    type Waiting = HashMap<u32, futures_oneshot::Sender>;

    struct Worker {
        worker: web_sys::Worker,
        ready: bool,
        queued: Vec<js_sys::Array>,
        waiting: Waiting,
        next: u32,
    }

    thread_local! {
        /// The workers once started (`None` in it: one that failed).
        static WORKERS: RefCell<Option<Vec<Option<Worker>>>> = const { RefCell::new(None) };
    }

    /// A tiny one-shot channel (no dependency): the answer, and the waker of whoever awaits it.
    mod futures_oneshot {
        use std::cell::RefCell;
        use std::future::Future;
        use std::pin::Pin;
        use std::rc::Rc;
        use std::task::{Context, Poll, Waker};

        /// An answer: its leading text (a plant job's meta data) and its byte arrays.
        pub type Answer = Option<(Option<String>, Vec<Vec<u8>>)>;
        type State = Rc<RefCell<(Option<Answer>, Option<Waker>)>>;
        pub struct Sender(State);
        pub struct Receiver(State);

        pub fn channel() -> (Sender, Receiver) {
            let s: State = Rc::default();
            (Sender(s.clone()), Receiver(s))
        }

        impl Sender {
            pub fn send(self, v: Answer) {
                let mut s = self.0.borrow_mut();
                s.0 = Some(v);
                if let Some(w) = s.1.take() {
                    w.wake();
                }
            }
        }

        impl Future for Receiver {
            type Output = Answer;
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let mut s = self.0.borrow_mut();
                match s.0.take() {
                    Some(v) => Poll::Ready(v),
                    None => {
                        s.1 = Some(cx.waker().clone());
                        Poll::Pending
                    }
                }
            }
        }
    }

    fn start(i: usize) -> Option<Worker> {
        let worker = web_sys::Worker::new("./texture_worker_loader.js").ok()?;
        let on_message =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                let data = e.data();
                WORKERS.with(|w| {
                    let mut w = w.borrow_mut();
                    let Some(Some(w)) = w.as_mut().and_then(|v| v.get_mut(i)) else {
                        return;
                    };
                    if data.as_string().as_deref() == Some(READY) {
                        w.ready = true;
                        for m in w.queued.drain(..) {
                            let _ = w.worker.post_message(&m);
                        }
                        return;
                    }
                    let Ok(a) = data.dyn_into::<js_sys::Array>() else {
                        return;
                    };
                    let id = a.get(0).as_f64().unwrap_or(-1.0) as u32;
                    // A plant job's answer leads with its meta data; an error is text too.
                    let meta = a.get(1).as_string().filter(|t| !t.starts_with("error"));
                    let first = if meta.is_some() { 2 } else { 1 };
                    let levels: Option<Vec<Vec<u8>>> = (first..a.length())
                        .map(|i| {
                            a.get(i)
                                .dyn_into::<js_sys::Uint8Array>()
                                .ok()
                                .map(|b| b.to_vec())
                        })
                        .collect();
                    if let Some(tx) = w.waiting.remove(&id) {
                        tx.send(levels.filter(|l| !l.is_empty()).map(|l| (meta, l)));
                    }
                });
            });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget(); // one worker for the page's life
        let on_error = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| {
            // A worker that cannot run: what waits on it stays RGBA8 (or is made on the page),
            // and the others take what comes.
            let gone = WORKERS.with(|w| {
                w.borrow_mut()
                    .as_mut()
                    .and_then(|v| v.get_mut(i))
                    .and_then(Option::take)
            });
            if let Some(w) = gone {
                for (_, tx) in w.waiting {
                    tx.send(None);
                }
                w.worker.terminate();
            }
        });
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();
        Some(Worker {
            worker,
            ready: false,
            queued: Vec::new(),
            waiting: HashMap::new(),
            next: 0,
        })
    }

    /// Sends a job (`[id, ...parts]`) to the worker with the fewest waiting; `None` when there
    /// is none.
    fn post(parts: &[wasm_bindgen::JsValue]) -> Option<futures_oneshot::Receiver> {
        WORKERS.with(|cell| {
            let mut cell = cell.borrow_mut();
            let workers =
                cell.get_or_insert_with(|| (0..super::web_workers()).map(start).collect());
            let worker = workers
                .iter_mut()
                .flatten()
                .min_by_key(|w| w.waiting.len())?;
            let id = worker.next;
            worker.next = worker.next.wrapping_add(1);
            let msg = js_sys::Array::new();
            msg.push(&id.into());
            for p in parts {
                msg.push(p);
            }
            let (tx, rx) = futures_oneshot::channel();
            worker.waiting.insert(id, tx);
            if worker.ready {
                let _ = worker.worker.post_message(&msg);
            } else {
                worker.queued.push(msg);
            }
            Some(rx)
        })
    }

    /// Compresses in the worker; `None` when there is none (or it failed): keep RGBA8.
    pub async fn compress(
        px: &[u8],
        w: u32,
        h: u32,
        kind: BcKind,
        space: ColorSpace,
    ) -> Option<Arc<Compressed>> {
        let rx = post(&[
            w.into(),
            h.into(),
            kind_name(kind).into(),
            (space == ColorSpace::Srgb).into(),
            js_sys::Uint8Array::from(px).into(),
        ])?;
        let (_, levels) = rx.await?;
        Some(Arc::new(Compressed {
            width: w,
            height: h,
            kind,
            color_space: space,
            levels,
        }))
    }

    /// A species' textures made and compressed in the worker (`plant_job`): the leaf colour
    /// and each image with whether it repeats, in `plant_images` order. `None` when there is
    /// no worker or it failed: make them on the page.
    pub async fn plant(
        params: &visioncraft_flora::PlantParams,
        scale: usize,
        scans: &visioncraft_flora::scan::Scans,
    ) -> Option<super::PlantCompressed> {
        let json = serde_json::to_string(params).ok()?;
        let packed = if scans.is_empty() {
            Vec::new()
        } else {
            scans.pack()
        };
        let rx = post(&[
            "plant".into(),
            json.into(),
            (scale as u32).into(),
            js_sys::Uint8Array::from(&packed[..]).into(),
        ])?;
        let (meta, levels) = rx.await?;
        let meta: super::PlantMeta = serde_json::from_str(&meta?).ok()?;
        let mut levels = levels.into_iter();
        let mut images = Vec::new();
        for (width, height, kind, srgb, repeat, n) in meta.images {
            let c = Compressed {
                width,
                height,
                kind: super::kind_of(&kind)?,
                color_space: if srgb {
                    ColorSpace::Srgb
                } else {
                    ColorSpace::Linear
                },
                levels: levels.by_ref().take(n).collect(),
            };
            (c.levels.len() == n).then_some(())?;
            images.push((Arc::new(c), repeat));
        }
        Some((meta.leaf_mean, images))
    }

    thread_local! {
        /// Mesh jobs sent and not answered yet (`meshes_waiting`).
        static MESHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// Mesh jobs on their way: the page hands over more only while this is small, so that the
    /// most urgent plants (the camera moves) are what the workers take next.
    pub fn meshes_waiting() -> usize {
        MESHES.with(std::cell::Cell::get)
    }

    /// One variant's meshes made in a worker (`mesh_job`); `None` when there is no worker or
    /// it failed: make them on the page.
    pub async fn mesh(
        params: &visioncraft_flora::PlantParams,
        variant: u32,
        month: u8,
        detail: bool,
    ) -> Option<(f32, crate::plant_cache::Levels)> {
        let json = serde_json::to_string(params).ok()?;
        let rx = post(&[
            "mesh".into(),
            json.into(),
            variant.into(),
            u32::from(month).into(),
            u32::from(detail).into(),
        ])?;
        MESHES.with(|m| m.set(m.get() + 1));
        let got = rx.await;
        MESHES.with(|m| m.set(m.get().saturating_sub(1)));
        let (_, levels) = got?;
        crate::plant_cache::decode_levels(levels.first()?)
    }

    /// A lawn's tufts scattered in a worker (`lawn_job`); `None` when there is no worker or it
    /// failed: scatter them on the page.
    pub async fn lawn(
        geom: &visioncraft_core::ImportedModel,
        job: &crate::scene::LawnScatter,
    ) -> Option<Vec<Vec<glam::Mat4>>> {
        let json = serde_json::to_string(job).ok()?;
        let geom = crate::import_task::encode(geom);
        let rx = post(&[
            "lawn".into(),
            json.into(),
            js_sys::Uint8Array::from(&geom[..]).into(),
        ])?;
        let (_, parts) = rx.await?;
        let [floats, counts] = &parts[..] else {
            return None;
        };
        super::lawn_lists(&super::f32s(floats), &super::u32s(counts))
    }
}

#[cfg(target_arch = "wasm32")]
pub use page::{compress, lawn, mesh, meshes_waiting, plant};

#[cfg(test)]
mod tests {
    #[test]
    fn kinds_go_through_names() {
        use visioncraft_render::bc::BcKind;
        for k in [BcKind::Rgb, BcKind::Rgba, BcKind::Normal, BcKind::Gray] {
            assert_eq!(super::kind_of(super::kind_name(k)), Some(k));
        }
        assert_eq!(super::kind_of("png"), None);
    }

    #[test]
    fn a_plant_job_answers_every_image_with_its_levels() {
        let p = &visioncraft_flora::species()[0].params;
        let (meta, levels) = super::plant_job(&serde_json::to_string(p).unwrap(), 2, &[]).unwrap();
        // atlas, its normal map, bark, bark normal map
        assert_eq!(meta.images.len(), 4);
        assert_eq!(meta.images.iter().map(|i| i.5).sum::<usize>(), levels.len());
        assert_eq!(meta.images[2].0, 3 * visioncraft_flora::BARK_SIZE);
        assert!(meta.leaf_mean.iter().all(|c| *c > 0.0));
    }

    #[test]
    fn a_lawn_job_answers_what_the_page_would_scatter() {
        let shape = visioncraft_core::Shape::Area(visioncraft_core::shape::Area {
            id: "S1".into(),
            name: "Lawn".into(),
            layer: "l0".into(),
            points: vec![[0.0, 0.0], [12.0, 0.0], [12.0, 8.0], [0.0, 8.0]],
            curves: Default::default(),
            smooth: false,
            elevation: 0.0,
            drape: false,
            edge_blend_m: 0.0,
            edge: None,
            terrace: None,
            extensions: Default::default(),
        });
        let geom = visioncraft_core::shape_mesh(&shape).unwrap();
        let lawn = visioncraft_core::material::Lawn3d {
            density_per_m2: 40.0,
            ..Default::default()
        };
        let faces: Vec<_> = (0..geom.faces.len()).map(|f| (f, lawn)).collect();
        let job = crate::scene::LawnScatter {
            xf: glam::Mat4::IDENTITY.to_cols_array(),
            areas: faces
                .iter()
                .map(|(f, _)| visioncraft_core::lawn_area(&geom, *f))
                .collect(),
            faces,
            max_density: 60.0,
            max_tufts: 5000,
            narrow: 1.0,
            window: None,
            over: Vec::new(),
            over_blend: Vec::new(),
            crowns: Vec::new(),
        };
        let here = crate::scene::scatter_lawn(&geom, &job);
        assert!(here.iter().map(Vec::len).sum::<usize>() > 10);
        let (floats, counts) = super::lawn_job(
            &serde_json::to_string(&job).unwrap(),
            &crate::import_task::encode(&geom),
        )
        .unwrap();
        let back = super::lawn_lists(
            &super::f32s(&super::bytes_of(&floats)),
            &super::u32s(&super::bytes_of_u32(&counts)),
        )
        .unwrap();
        assert_eq!(back, here);
    }

    #[test]
    fn a_mesh_job_answers_what_the_page_would_make() {
        let p = &visioncraft_flora::species()[0].params;
        let bytes = super::mesh_job(&serde_json::to_string(p).unwrap(), 1, 6, false).unwrap();
        let here = crate::plants::generate_variant_now("", p, 1, 6, false);
        assert_eq!(
            bytes,
            crate::plant_cache::encode_levels(here.height, &here.levels)
        );
        let (h, levels) = crate::plant_cache::decode_levels(&bytes).unwrap();
        assert_eq!((h, levels.len()), (here.height, here.levels.len()));
    }
}
