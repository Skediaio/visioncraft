//! The sky visibility volume (`visioncraft_core::sky_volume`) off the UI thread: seconds for a
//! furnished house in the browser. Natively on a thread; on the web in the import worker (the
//! same worker binary as `import_task`, a `sky` job), the volume travelling back as bytes
//! (`SkyVolume::to_bytes`). The sun's bounce through its rooms for the sun at the start
//! (`SkyVolume::sun_bounce`, seconds too) is made with it. When the worker cannot start, both are
//! made here, blocking.

use std::sync::{Arc, Mutex};
use visioncraft_core::{Crown, SkyVolume};

type Triangles = Vec<[[f32; 3]; 3]>;
/// A volume with its sun bounce (`None`: nothing blocks the sky).
pub type Made = Option<(SkyVolume, Vec<f32>)>;
type Slot = Arc<Mutex<Option<Made>>>;

/// One volume in the making, for the occluders hashed to `key`, its sun bounce for `to_sun`.
pub struct SkyTask {
    pub key: u64,
    pub to_sun: [f32; 3],
    slot: Slot,
}

impl SkyTask {
    pub fn start(triangles: Triangles, crowns: Vec<Crown>, key: u64, to_sun: [f32; 3]) -> Self {
        let slot: Slot = Arc::default();
        run(triangles, crowns, to_sun, slot.clone());
        Self { key, to_sun, slot }
    }

    /// The volume once made, then `None` again.
    pub fn take(&self) -> Option<Made> {
        self.slot.lock().ok()?.take()
    }
}

/// The volume and its sun bounce, made here.
pub fn make(triangles: &[[[f32; 3]; 3]], crowns: &[Crown], to_sun: [f32; 3]) -> Made {
    let v = visioncraft_core::sky_volume_with_canopy(triangles, crowns)?;
    let bounce = v.sun_bounce(to_sun);
    Some((v, bounce))
}

fn put(slot: &Slot, v: Made) {
    if let Ok(mut s) = slot.lock() {
        *s = Some(v);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(triangles: Triangles, crowns: Vec<Crown>, to_sun: [f32; 3], slot: Slot) {
    std::thread::spawn(move || put(&slot, make(&triangles, &crowns, to_sun)));
}

#[cfg(target_arch = "wasm32")]
fn run(triangles: Triangles, crowns: Vec<Crown>, to_sun: [f32; 3], slot: Slot) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    let Ok(worker) = web_sys::Worker::new("./import_worker_loader.js") else {
        return put(&slot, make(&triangles, &crowns, to_sun));
    };
    let msg = js_sys::Array::new();
    msg.push(&SKY.into());
    msg.push(&js_sys::Uint8Array::from(&encode_triangles(&triangles)[..]));
    msg.push(&js_sys::Float32Array::from(&to_sun[..]));
    msg.push(&js_sys::Float32Array::from(&encode_crowns(&crowns)[..]));
    let (w, done) = (worker.clone(), slot.clone());
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let data = e.data();
            if data.as_string().as_deref() == Some(crate::import_task::READY) {
                let _ = w.post_message(&msg);
                return;
            }
            // [volume, sun bounce]; empty volume bytes: nothing blocks the sky.
            let parts = data.dyn_into::<js_sys::Array>().ok();
            let v = parts.and_then(|a| {
                let b = a.get(0).dyn_into::<js_sys::Uint8Array>().ok()?.to_vec();
                let bounce = a.get(1).dyn_into::<js_sys::Float32Array>().ok()?.to_vec();
                let v = SkyVolume::from_bytes(&b).ok()?;
                Some((v, bounce))
            });
            put(&done, v);
            w.terminate();
        });
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    let (w, failed) = (worker.clone(), slot.clone());
    let on_error = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| {
        w.terminate();
        if failed.lock().is_ok_and(|s| s.is_none()) {
            put(&failed, make(&triangles, &crowns, to_sun));
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

/// The crowns as seven floats each: centre, radii, ground height.
pub fn encode_crowns(c: &[Crown]) -> Vec<f32> {
    c.iter()
        .flat_map(|c| c.center.into_iter().chain(c.radii).chain([c.ground_y]))
        .collect()
}

pub fn decode_crowns(f: &[f32]) -> Vec<Crown> {
    f.as_chunks::<7>()
        .0
        .iter()
        .map(|c| Crown {
            center: [c[0], c[1], c[2]],
            radii: [c[3], c[4], c[5]],
            ground_y: c[6],
        })
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
    fn crowns_survive_the_trip_through_floats() {
        let c = vec![Crown {
            center: [1.0, 7.5, -3.0],
            radii: [2.0, 3.0, 2.5],
            ground_y: 0.25,
        }];
        assert_eq!(decode_crowns(&encode_crowns(&c)), c);
    }

    #[test]
    fn a_volume_is_made_off_the_thread() {
        let t = vec![[[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 4.0]]];
        let task = SkyTask::start(t, Vec::new(), 7, [0.3, 0.8, 0.2]);
        let v = loop {
            if let Some(v) = task.take() {
                break v;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let (v, bounce) = v.expect("a volume");
        assert!(!v.samples.is_empty());
        assert_eq!(bounce, v.sun_bounce([0.3, 0.8, 0.2]));
    }
}
