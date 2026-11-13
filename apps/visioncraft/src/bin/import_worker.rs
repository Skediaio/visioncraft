//! The web import worker (`import_task`, loaded by `index.html`): receives `[kind, bytes]`, posts
//! back the meshed model as bytes, or an error message. Natively it does nothing.

#[cfg(target_arch = "wasm32")]
fn main() {
    use visioncraft::import_task::{encode, kind_of};
    use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let reply_to = scope.clone();
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let msg: js_sys::Array = e.data().unchecked_into();
            let bytes = msg.get(1).unchecked_into::<js_sys::Uint8Array>().to_vec();
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
