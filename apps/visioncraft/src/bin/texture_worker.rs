//! The web texture worker (`texture_worker` module, loaded by `index.html`): receives
//! `[id, width, height, kind, srgb, pixels]`, posts back `[id, levels...]` (each level's bytes),
//! or `[id, "error: ..."]`. Block compression of a 4k map takes seconds in wasm: here it does
//! not freeze the page. Natively it does nothing.

#[cfg(target_arch = "wasm32")]
fn main() {
    use visioncraft::texture_worker::{READY, answer};
    use wasm_bindgen::{JsCast, prelude::Closure};
    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let reply_to = scope.clone();
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let reply = answer(&e.data().unchecked_into());
            let _ = reply_to.post_message(&reply);
        });
    scope.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget(); // lives as long as the worker
    let _ = scope.post_message(&READY.into());
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
