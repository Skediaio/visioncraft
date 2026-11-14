//! Block compression off the page's thread on the web: one Web Worker (the `texture_worker`
//! binary, the same crate built again, see `index.html`) compresses decoded pictures with
//! `visioncraft_render::bc` and sends the mip levels back as bytes. Natively the loader threads
//! compress themselves (`assets`), so this is wasm-only. When the worker cannot start, pictures
//! stay RGBA8 as before.

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

/// The worker's side: one job in, its answer out.
#[cfg(target_arch = "wasm32")]
pub fn answer(msg: &js_sys::Array) -> wasm_bindgen::JsValue {
    use visioncraft_render::ColorSpace;
    let out = js_sys::Array::new();
    out.push(&msg.get(0));
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

        type State = Rc<RefCell<(Option<Option<Vec<Vec<u8>>>>, Option<Waker>)>>;
        pub struct Sender(State);
        pub struct Receiver(State);

        pub fn channel() -> (Sender, Receiver) {
            let s: State = Rc::default();
            (Sender(s.clone()), Receiver(s))
        }

        impl Sender {
            pub fn send(self, v: Option<Vec<Vec<u8>>>) {
                let mut s = self.0.borrow_mut();
                s.0 = Some(v);
                if let Some(w) = s.1.take() {
                    w.wake();
                }
            }
        }

        impl Future for Receiver {
            type Output = Option<Vec<Vec<u8>>>;
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
                    let levels: Option<Vec<Vec<u8>>> = (1..a.length())
                        .map(|i| {
                            a.get(i)
                                .dyn_into::<js_sys::Uint8Array>()
                                .ok()
                                .map(|b| b.to_vec())
                        })
                        .collect();
                    if let Some(tx) = w.waiting.remove(&id) {
                        tx.send(levels.filter(|l| !l.is_empty()));
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

    /// Compresses in the worker; `None` when there is none (or it failed): keep RGBA8.
    pub async fn compress(
        px: &[u8],
        w: u32,
        h: u32,
        kind: BcKind,
        space: ColorSpace,
    ) -> Option<Arc<Compressed>> {
        let rx = WORKER.with(|cell| {
            let mut cell = cell.borrow_mut();
            let worker = cell.get_or_insert_with(start).as_mut()?;
            let id = worker.next;
            worker.next = worker.next.wrapping_add(1);
            let msg = js_sys::Array::new();
            msg.push(&id.into());
            msg.push(&w.into());
            msg.push(&h.into());
            msg.push(&kind_name(kind).into());
            msg.push(&(space == ColorSpace::Srgb).into());
            msg.push(&js_sys::Uint8Array::from(px));
            let (tx, rx) = futures_oneshot::channel();
            worker.waiting.insert(id, tx);
            if worker.ready {
                let _ = worker.worker.post_message(&msg);
            } else {
                worker.queued.push(msg);
            }
            Some(rx)
        })?;
        let levels = rx.await?;
        Some(Arc::new(Compressed {
            width: w,
            height: h,
            kind,
            color_space: space,
            levels,
        }))
    }
}

#[cfg(target_arch = "wasm32")]
pub use page::compress;

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
}
