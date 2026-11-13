//! Model imports off the UI thread: natively on a thread; on the web in a Web Worker that runs
//! the `import_worker` binary (the same crate built a second time, see `index.html`), so the page
//! keeps painting while the geometry kernel meshes a `.3dcraft`. Without SharedArrayBuffer the
//! model travels back as bytes ([`encode`] / [`decode`]). When the worker cannot start, the import
//! fails with a message: the page's own wasm leaves the importer (the geometry kernel) out.

use std::sync::{Arc, Mutex};
use visioncraft_core::{Body, Face, ImportedModel, ModelKind};

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

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// The model as bytes: counts, the vertex and triangle arrays little-endian, then the faces,
/// bodies and warnings as JSON.
pub fn encode(m: &ImportedModel) -> Vec<u8> {
    let meta = serde_json::json!({
        "faces": m.faces.iter().map(|f| (&f.name, f.body)).collect::<Vec<_>>(),
        "bodies": m.bodies.iter().map(|b| (&b.id, &b.name, b.color, b.triangles.start, b.triangles.end, b.bounds)).collect::<Vec<_>>(),
        "warnings": m.warnings,
    })
    .to_string();
    let mut out = Vec::new();
    for n in [
        m.positions.len(),
        m.normals.len(),
        m.triangles.len(),
        m.triangle_faces.len(),
        meta.len(),
    ] {
        put_u32(&mut out, n as u32);
    }
    let floats = m.positions.iter().chain(&m.normals).flatten();
    floats.for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    let ints = m.triangles.iter().flatten().chain(&m.triangle_faces);
    ints.for_each(|v| put_u32(&mut out, *v));
    out.extend_from_slice(meta.as_bytes());
    out
}

pub fn decode(b: &[u8]) -> Result<ImportedModel, String> {
    let bad = || "a damaged model came back from the import worker".to_string();
    let mut at = 0usize;
    let mut word = || -> Result<[u8; 4], String> {
        let w = b.get(at..at + 4).ok_or_else(bad)?;
        at += 4;
        Ok([w[0], w[1], w[2], w[3]])
    };
    let mut counts = [0usize; 5];
    for c in &mut counts {
        *c = u32::from_le_bytes(word()?) as usize;
    }
    let [np, nn, nt, nf, nmeta] = counts;
    let mut f3 = |n: usize| -> Result<Vec<[f32; 3]>, String> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let mut p = [0.0f32; 3];
            for c in &mut p {
                *c = f32::from_le_bytes(word()?);
            }
            v.push(p);
        }
        Ok(v)
    };
    let positions = f3(np)?;
    let normals = f3(nn)?;
    let mut triangles = Vec::with_capacity(nt);
    for _ in 0..nt {
        let mut t = [0u32; 3];
        for c in &mut t {
            *c = u32::from_le_bytes(word()?);
        }
        triangles.push(t);
    }
    let triangle_faces = (0..nf)
        .map(|_| word().map(u32::from_le_bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let meta = b.get(at..at + nmeta).ok_or_else(bad)?;
    #[derive(serde::Deserialize)]
    struct Meta {
        faces: Vec<(String, u32)>,
        #[allow(clippy::type_complexity)]
        bodies: Vec<(String, String, Option<[f32; 4]>, u32, u32, [[f32; 3]; 2])>,
        warnings: Vec<String>,
    }
    let meta: Meta = serde_json::from_slice(meta).map_err(|e| e.to_string())?;
    Ok(ImportedModel {
        positions,
        normals,
        triangles,
        triangle_faces,
        faces: meta
            .faces
            .into_iter()
            .map(|(name, body)| Face { name, body })
            .collect(),
        bodies: meta
            .bodies
            .into_iter()
            .map(|(id, name, color, s, e, bounds)| Body {
                id,
                name,
                color,
                triangles: s..e,
                bounds,
            })
            .collect(),
        warnings: meta.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
