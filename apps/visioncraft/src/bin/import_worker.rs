//! The web import worker (`import_task`, loaded by `index.html`): receives `[kind, bytes]`, posts
//! back the meshed model as bytes, or an error message; or the sky visibility volume for
//! `["sky", triangles, to_sun, crowns]` (`sky_task`) with its sun bounce, or the baked sky
//! visibility probes for `["skyvis", scene floats, start, end]` (`sky_vis`). Natively it does nothing.

#[cfg(target_arch = "wasm32")]
fn main() {
    use visioncraft::import_task::{encode, kind_of};
    use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let reply_to = scope.clone();
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let msg: js_sys::Array = e.data().unchecked_into();
            // The baked sky visibility (`sky_vis`): `["skyvis", scene floats, start, end]`, the
            // probes `start..end` as floats out (none: nothing blocks the sky, or the job is
            // wrong). Empty scene floats: the scene of the last job of this worker (its binned
            // triangles are kept for the next range).
            if msg.get(0).as_string().as_deref() == Some(visioncraft::sky_vis::SKY_VIS) {
                thread_local! {
                    static BAKE: std::cell::RefCell<Option<visioncraft_automation::SkyVisJob>> =
                        const { std::cell::RefCell::new(None) };
                }
                let f = js_sys::Float32Array::new(&msg.get(1)).to_vec();
                let end = msg.get(3).as_f64().map_or(usize::MAX, |v| v as usize);
                let start = msg.get(2).as_f64().map_or(0, |v| v as usize);
                let out = BAKE.with(|b| {
                    let mut b = b.borrow_mut();
                    if !f.is_empty() {
                        *b = visioncraft_core::SkyScene::from_floats(&f)
                            .map(visioncraft_automation::SkyVisJob::new);
                    }
                    b.as_ref().map_or_else(Vec::new, |job| {
                        let total = job.progress().1;
                        job.probe_floats(start, end.min(total))
                    })
                });
                let _ = reply_to.post_message(&js_sys::Float32Array::from(&out[..]));
                return;
            }
            let bytes = msg.get(1).unchecked_into::<js_sys::Uint8Array>().to_vec();
            // The sky visibility volume and its sun bounce (`sky_task`): empty bytes when nothing
            // blocks the sky.
            if msg.get(0).as_string().as_deref() == Some(visioncraft::sky_task::SKY) {
                let t = visioncraft::sky_task::decode_triangles(&bytes);
                let d = js_sys::Float32Array::new(&msg.get(2)).to_vec();
                let to_sun = [0, 1, 2].map(|i| d.get(i).copied().unwrap_or(0.0));
                let crowns = visioncraft::sky_task::decode_crowns(
                    &js_sys::Float32Array::new(&msg.get(3)).to_vec(),
                );
                let (v, bounce) = visioncraft::sky_task::make(&t, &crowns, to_sun)
                    .map(|(v, b)| (v.to_bytes(), b))
                    .unwrap_or_default();
                let reply = js_sys::Array::of2(
                    &js_sys::Uint8Array::from(&v[..]),
                    &js_sys::Float32Array::from(&bounce[..]),
                );
                let _ = reply_to.post_message(&reply);
                return;
            }
            let reply: JsValue = match msg
                .get(0)
                .as_string()
                .and_then(|k| kind_of(&k))
                .ok_or_else(|| "unknown model kind".to_string())
                .and_then(|k| visioncraft::studio::Studio::import(k, &bytes))
            {
                Ok(m) => js_sys::Uint8Array::from(&encode(&m)[..]).into(),
                Err(e) => format!("error: {e}").into(),
            };
            let _ = reply_to.post_message(&reply);
        });
    scope.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget(); // lives as long as the worker
    let _ = scope.post_message(&visioncraft::import_task::READY.into());
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
