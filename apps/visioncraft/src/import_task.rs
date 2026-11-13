//! Model imports off the UI thread: natively on a thread; on the web in a Web Worker that runs
//! the `import_worker` binary (the same crate built a second time, see `index.html`), so the page
//! keeps painting while the geometry kernel meshes a `.3dcraft`. Without SharedArrayBuffer the
//! model travels back as bytes ([`encode`] / [`decode`]). When the worker cannot start, the import
//! fails with a message: the page's own wasm leaves the importer (the geometry kernel) out.

use std::sync::{Arc, Mutex};
use visioncraft_core::{ImportedModel, ModelKind};

type Slot = Arc<Mutex<Option<Result<ImportedModel, String>>>>;

/// One import in flight.
pub struct ImportTask {
    slot: Slot,
}

impl ImportTask {
    /// Starts importing; `wake` repaints the window when the result is there.
    pub fn start(kind: ModelKind, bytes: Arc<[u8]>, wake: eframe::egui::Context) -> Self {
        let slot: Slot = Arc::default();
        run(kind, bytes, slot.clone(), wake);
        Self { slot }
    }

    /// The result, once (then `None` again).
    pub fn take(&self) -> Option<Result<ImportedModel, String>> {
        self.slot.lock().ok()?.take()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(kind: ModelKind, bytes: Arc<[u8]>, slot: Slot, wake: eframe::egui::Context) {
    std::thread::spawn(move || {
        let r = crate::studio::Studio::import(kind, &bytes);
        if let Ok(mut s) = slot.lock() {
            *s = Some(r);
        }
        wake.request_repaint();
    });
}

#[cfg(target_arch = "wasm32")]
fn run(kind: ModelKind, bytes: Arc<[u8]>, slot: Slot, wake: eframe::egui::Context) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    // The page's own wasm does not carry the importer (the geometry kernel is megabytes): only
    // the worker does. Without a worker the import fails with a message.
    let inline = |slot: &Slot| {
        if let Ok(mut s) = slot.lock() {
            *s = Some(Err(
                "the import worker could not start, so this browser cannot open models".into(),
            ));
        }
    };
    let Ok(worker) = web_sys::Worker::new("./import_worker_loader.js") else {
        return inline(&slot);
    };
    let msg = js_sys::Array::new();
    msg.push(&kind_name(kind).into());
    msg.push(&js_sys::Uint8Array::from(&bytes[..]));
    let (w, done, wake2) = (worker.clone(), slot.clone(), wake.clone());
    // The worker says "ready" once its wasm runs (a message sent earlier would be lost), then
    // gets the job and answers with the model's bytes or "error: ...".
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let data = e.data();
            if data.as_string().as_deref() == Some(READY) {
                if w.post_message(&msg).is_err() {
                    w.terminate();
                    if let Ok(mut s) = done.lock() {
                        *s = Some(Err("the import worker refused the file".into()));
                    }
                    wake.request_repaint();
                }
                return;
            }
            let r = match data.dyn_into::<js_sys::Uint8Array>() {
                Ok(a) => decode(&a.to_vec()),
                Err(v) => Err(v
                    .as_string()
                    .map(|s| s.trim_start_matches("error: ").to_string())
                    .unwrap_or_else(|| "import failed".into())),
            };
            if let Ok(mut s) = done.lock() {
                *s = Some(r);
            }
            w.terminate();
            wake.request_repaint();
        });
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    // A worker that cannot load (or panics) imports here instead, blocking as before.
    let (w, failed) = (worker.clone(), slot.clone());
    let on_error = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| {
        w.terminate();
        if failed.lock().is_ok_and(|s| s.is_none()) {
            inline(&failed);
        }
        wake2.request_repaint();
    });
    worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    // shortcut: two small closures leak per import, fine for files a person opens
    on_message.forget();
    on_error.forget();
}

/// What the worker posts when it can take a job.
pub const READY: &str = "ready";

pub fn kind_name(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Craft3d => "3dcraft",
        ModelKind::Obj => "obj",
    }
}

pub fn kind_of(name: &str) -> Option<ModelKind> {
    match name {
        "3dcraft" => Some(ModelKind::Craft3d),
        "obj" => Some(ModelKind::Obj),
        _ => None,
    }
}

// ---- transfer ----

/// The model as bytes for the trip back from the worker (the core's checked codec).
pub fn encode(m: &ImportedModel) -> Vec<u8> {
    m.to_bytes()
}

pub fn decode(b: &[u8]) -> Result<ImportedModel, String> {
    ImportedModel::from_bytes(b)
        .map_err(|e| format!("a damaged model came back from the import worker: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_core::{Body, Face};

    #[test]
    fn a_model_survives_the_trip_through_bytes() {
        let m = ImportedModel {
            positions: vec![[0.0, 1.0, 2.0], [3.5, -4.0, 5.25], [1.0, 1.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]; 3],
            triangles: vec![[0, 1, 2]],
            triangle_faces: vec![0],
            faces: vec![Face {
                name: "F2:side(e7)".into(),
                body: 0,
            }],
            bodies: vec![Body {
                id: "F2".into(),
                name: "Plate".into(),
                color: Some([0.5, 0.25, 1.0, 1.0]),
                triangles: 0..1,
                bounds: [[0.0, -4.0, 1.0], [3.5, 1.0, 5.25]],
            }],
            warnings: vec!["one \"quoted\" warning".into()],
        };
        assert_eq!(decode(&encode(&m)).unwrap(), m);
        assert!(decode(&encode(&m)[..30]).is_err());
    }
}
