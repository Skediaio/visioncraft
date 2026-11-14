//! Block compression off the page's thread on the web: one Web Worker (the `texture_worker`
//! binary, the same crate built again, see `index.html`) compresses decoded pictures with
//! `visioncraft_render::bc` and sends the mip levels back as bytes. Natively the loader threads
//! compress themselves (`assets`), so this is wasm-only. When the worker cannot start, pictures
//! stay RGBA8 as before.
//!
//! The worker also makes a plant species' textures (`["plant", params, scale]` jobs): the leaf
//! atlas, its normal map and the bark, generated and compressed there instead of on the page
//! (natively `plant_compressed` runs on a thread, see `plants`).

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
/// repeats, in `plant_images` order. Natively on a thread of its own, on the web in the worker.
pub type PlantCompressed = (
    [f32; 3],
    Vec<(std::sync::Arc<visioncraft_render::bc::Compressed>, bool)>,
);

pub fn plant_compressed(
    params: &visioncraft_flora::PlantParams,
    scale: usize,
) -> Option<PlantCompressed> {
    use visioncraft_render::ColorSpace;
    let t = visioncraft_flora::textures_at(params, scale);
    let mut images = Vec::new();
    for (img, kind, srgb, repeat) in plant_images(&t) {
        let space = if srgb {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        };
        let c = visioncraft_render::bc::compress(&img.rgba, img.width, img.height, kind, space)?;
        images.push((std::sync::Arc::new(c), repeat));
    }
    Some((crate::plants::opaque_mean(&t.atlas.rgba), images))
}

/// A plant job in the worker: the meta data and every image's levels in turn.
pub fn plant_job(params: &str, scale: usize) -> Option<(PlantMeta, Vec<Vec<u8>>)> {
    let params: visioncraft_flora::PlantParams = serde_json::from_str(params).ok()?;
    let (leaf_mean, images) = plant_compressed(&params, scale)?;
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

/// The worker's side: one job in, its answer out.
#[cfg(target_arch = "wasm32")]
pub fn answer(msg: &js_sys::Array) -> wasm_bindgen::JsValue {
    use visioncraft_render::ColorSpace;
    let out = js_sys::Array::new();
    out.push(&msg.get(0));
    if msg.get(1).as_string().as_deref() == Some("plant") {
        let params = msg.get(2).as_string().unwrap_or_default();
        let scale = msg.get(3).as_f64().unwrap_or(1.0) as usize;
        match plant_job(&params, scale) {
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
        static WORKER: RefCell<Option<Option<Worker>>> = const { RefCell::new(None) };
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

    fn start() -> Option<Worker> {
        let worker = web_sys::Worker::new("./texture_worker_loader.js").ok()?;
        let on_message =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                let data = e.data();
                WORKER.with(|w| {
                    let mut w = w.borrow_mut();
                    let Some(Some(w)) = w.as_mut() else { return };
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
            // A worker that cannot run: everything waiting stays RGBA8, and so does what comes.
            WORKER.with(|w| {
                if let Some(Some(w)) = w.borrow_mut().take() {
                    for (_, tx) in w.waiting {
                        tx.send(None);
                    }
                    w.worker.terminate();
                }
            });
            WORKER.with(|w| *w.borrow_mut() = Some(None));
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

    /// Sends a job (`[id, ...parts]`) to the worker; `None` when there is none.
    fn post(parts: &[wasm_bindgen::JsValue]) -> Option<futures_oneshot::Receiver> {
        WORKER.with(|cell| {
            let mut cell = cell.borrow_mut();
            let worker = cell.get_or_insert_with(start).as_mut()?;
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
    ) -> Option<super::PlantCompressed> {
        let json = serde_json::to_string(params).ok()?;
        let rx = post(&["plant".into(), json.into(), (scale as u32).into()])?;
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
}

#[cfg(target_arch = "wasm32")]
pub use page::{compress, plant};

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
        let (meta, levels) = super::plant_job(&serde_json::to_string(p).unwrap(), 2).unwrap();
        // atlas, its normal map, bark, bark normal map
        assert_eq!(meta.images.len(), 4);
        assert_eq!(meta.images.iter().map(|i| i.5).sum::<usize>(), levels.len());
        assert_eq!(meta.images[2].0, 2 * visioncraft_flora::BARK_SIZE);
        assert!(meta.leaf_mean.iter().all(|c| *c > 0.0));
    }
}
