//! The sky visibility volume (`visioncraft_core::sky_volume`) off the UI thread: seconds for a
//! furnished house in the browser. Natively on a thread; on the web in the import worker (the
//! same worker binary as `import_task`, a `sky` job), the volume travelling back as bytes
//! (`SkyVolume::to_bytes`). When the worker cannot start, it is made here, blocking.

use std::sync::{Arc, Mutex};
use visioncraft_core::SkyVolume;

type Triangles = Vec<[[f32; 3]; 3]>;
type Slot = Arc<Mutex<Option<Option<SkyVolume>>>>;

/// One volume in the making, for the occluders hashed to `key`.
pub struct SkyTask {
    pub key: u64,
    slot: Slot,
}

impl SkyTask {
    pub fn start(triangles: Triangles, key: u64) -> Self {
        let slot: Slot = Arc::default();
        run(triangles, slot.clone());
        Self { key, slot }
    }

    /// The volume once made (`Some(None)`: nothing blocks the sky), then `None` again.
    pub fn take(&self) -> Option<Option<SkyVolume>> {
        self.slot.lock().ok()?.take()
    }
}

fn put(slot: &Slot, v: Option<SkyVolume>) {
    if let Ok(mut s) = slot.lock() {
        *s = Some(v);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(triangles: Triangles, slot: Slot) {
    std::thread::spawn(move || put(&slot, visioncraft_core::sky_volume(&triangles)));
}

#[cfg(target_arch = "wasm32")]
fn run(triangles: Triangles, slot: Slot) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    let Ok(worker) = web_sys::Worker::new("./import_worker_loader.js") else {
        return put(&slot, visioncraft_core::sky_volume(&triangles));
    };
    let msg = js_sys::Array::new();
    msg.push(&SKY.into());
    msg.push(&js_sys::Uint8Array::from(&encode_triangles(&triangles)[..]));
    let (w, done) = (worker.clone(), slot.clone());
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let data = e.data();
            if data.as_string().as_deref() == Some(crate::import_task::READY) {
                let _ = w.post_message(&msg);
                return;
            }
            // Empty bytes: nothing blocks the sky.
            let v = data
                .dyn_into::<js_sys::Uint8Array>()
                .ok()
                .map(|a| a.to_vec())
                .filter(|b| !b.is_empty())
                .and_then(|b| SkyVolume::from_bytes(&b).ok());
            put(&done, v);
            w.terminate();
        });
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    let (w, failed) = (worker.clone(), slot.clone());
    let on_error = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| {
        w.terminate();
        if failed.lock().is_ok_and(|s| s.is_none()) {
            put(&failed, visioncraft_core::sky_volume(&triangles));
        }
    });
    worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    // shortcut: two small closures leak per volume, fine for a change of the buildings
    on_message.forget();
    on_error.forget();
}

/// The worker job's kind (beside the model kinds of `import_task`).
pub const SKY: &str = "sky";

/// The triangles as little-endian floats, nine per triangle.
pub fn encode_triangles(t: &[[[f32; 3]; 3]]) -> Vec<u8> {
    t.as_flattened()
        .as_flattened()
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

pub fn decode_triangles(b: &[u8]) -> Triangles {
    b.as_chunks::<36>()
        .0
        .iter()
        .map(|c| {
            let f =
                |i: usize| f32::from_le_bytes([c[4 * i], c[4 * i + 1], c[4 * i + 2], c[4 * i + 3]]);
            std::array::from_fn(|v| std::array::from_fn(|k| f(3 * v + k)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangles_survive_the_trip_through_bytes() {
        let t = vec![[[0.0, 1.0, 2.0], [3.5, -4.0, 5.25], [1e-3, 7.0, -0.5]]; 2];
        assert_eq!(decode_triangles(&encode_triangles(&t)), t);
    }

    #[test]
    fn a_volume_is_made_off_the_thread() {
        let t = vec![[[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 4.0]]];
        let task = SkyTask::start(t, 7);
        let v = loop {
            if let Some(v) = task.take() {
                break v;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(v.is_some_and(|v| !v.samples.is_empty()));
    }
}
