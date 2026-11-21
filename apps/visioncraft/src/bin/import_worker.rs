//! The web import worker (`import_task`, loaded by `index.html`): receives `[kind, bytes]`, posts
//! back the meshed model as bytes, or an error message; or the sky visibility volume for
//! `["sky", triangles, to_sun, crowns]` (`sky_task`) with its sun bounce, or the baked sky
//! visibility probes for `["skyvis", scene floats, start, end]` (`sky_vis`). The worker keeps the
//! sky volume it made and relights it: `["skylight", to_sun]` -> `["skylight", sun bounce]`,
//! `["lamplight", lamps]` -> `["lamplight", rgb bounce, flattened]` (`sky_relight`). Natively it
//! does nothing.

#[cfg(target_arch = "wasm32")]
fn main() {
    visioncraft::shell::install_panic_hook();
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
            // The kept sky volume (`sky_task`), relit for the sun or the lamps (`sky_relight`);
            // an empty bounce when this worker has no volume.
            thread_local! {
                static VOLUME: std::cell::RefCell<Option<visioncraft_core::SkyVolume>> =
                    const { std::cell::RefCell::new(None) };
            }
            let kind = msg.get(0).as_string();
            if kind.as_deref() == Some(visioncraft::sky_relight::SKY_LIGHT) {
                let d = js_sys::Float32Array::new(&msg.get(1)).to_vec();
                let to_sun = [0, 1, 2].map(|i| d.get(i).copied().unwrap_or(0.0));
                let out = VOLUME.with(|v| v.borrow().as_ref().map(|v| v.sun_bounce(to_sun)));
                let reply = js_sys::Array::of2(
                    &visioncraft::sky_relight::SKY_LIGHT.into(),
                    &js_sys::Float32Array::from(&out.unwrap_or_default()[..]),
                );
                let _ = reply_to.post_message(&reply);
                return;
            }
            if kind.as_deref() == Some(visioncraft::sky_relight::LAMP_LIGHT) {
                let lamps = visioncraft::sky_relight::decode_lamps(
                    &js_sys::Float32Array::new(&msg.get(1)).to_vec(),
                );
                let out = VOLUME.with(|v| v.borrow().as_ref().map(|v| v.lamp_bounce(&lamps)));
                let reply = js_sys::Array::of2(
                    &visioncraft::sky_relight::LAMP_LIGHT.into(),
                    &js_sys::Float32Array::from(
                        &visioncraft::sky_relight::flatten_rgb(&out.unwrap_or_default())[..],
                    ),
                );
                let _ = reply_to.post_message(&reply);
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
                let made = visioncraft::sky_task::make(&t, &crowns, to_sun);
                let (v, bounce) = made
                    .as_ref()
                    .map_or_else(Default::default, |(v, b)| (v.to_bytes(), b.clone()));
                VOLUME.with(|k| *k.borrow_mut() = made.map(|(v, _)| v));
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
