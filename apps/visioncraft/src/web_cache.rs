//! The browser's copy of `plant_cache`: generated plant meshes and textures and block-compressed
//! library maps and model pictures, kept in the Cache API (`caches`, per origin), so opening a
//! design a second time loads them instead of generating, decoding and compressing everything
//! again (seconds of worker time per species and map on the web).
//!
//! An entry's name is the `plant_cache::key` of what made it (the same parts as on the desktop),
//! a kind, and this build: the main module's file name, which carries trunk's hash of its
//! contents, so a rebuilt generator never reads an old result. Natively there is nothing here
//! (`plant_cache` keeps them on disk). A browser that refuses the Cache API (not a secure
//! context, storage blocked) simply makes everything every time.

/// The bytes stored under `kind` and `key`, if any.
#[cfg(target_arch = "wasm32")]
pub async fn get(kind: &str, key: u64) -> Option<Vec<u8>> {
    let t = crate::assets::now_ms();
    let got = wasm_bindgen_futures::JsFuture::from(js::vc_cache_get(&name(kind, key)))
        .await
        .ok()
        .filter(|v| !v.is_null() && !v.is_undefined())
        .map(|v| js_sys::Uint8Array::new(&v).to_vec());
    let what = if got.is_some() { "hit (read)" } else { "miss" };
    crate::load_stats::add(
        &format!("browser cache {what} ({kind})"),
        crate::assets::now_ms() - t,
    );
    got
}

/// Stores `bytes` under `kind` and `key` (in the background).
#[cfg(target_arch = "wasm32")]
pub fn put(kind: &str, key: u64, bytes: &[u8]) {
    let _ = js::vc_cache_put(&name(kind, key), &js_sys::Uint8Array::from(bytes));
}

#[cfg(target_arch = "wasm32")]
fn name(kind: &str, key: u64) -> String {
    format!("{kind}-{key:016x}-{}", js::vc_cache_build())
}

#[cfg(target_arch = "wasm32")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(inline_js = r#"
const NAME = 'visioncraft-work-v1';
let build = null;
// The main module's file name carries trunk's hash of its contents.
export function vc_cache_build() {
  if (build === null) {
    const l = document.querySelector('link[rel=preload][type="application/wasm"]');
    build = l ? l.getAttribute('href').replace(/^.*\//, '').replace(/_bg\.wasm$/, '') : 'dev';
  }
  return build;
}
const url = name => new URL('./__vc_cache/' + name, location.href).href;
export async function vc_cache_get(name) {
  try {
    if (typeof caches === 'undefined') return null;
    const c = await caches.open(NAME);
    const r = await c.match(url(name));
    return r ? new Uint8Array(await r.arrayBuffer()) : null;
  } catch (e) {
    return null;
  }
}
// Entries of older builds are dropped once, the first time this build stores something.
let swept = false;
export async function vc_cache_put(name, bytes) {
  try {
    if (typeof caches === 'undefined') return;
    const c = await caches.open(NAME);
    if (!swept) {
      swept = true;
      const mine = '-' + vc_cache_build();
      for (const k of await c.keys()) if (!k.url.endsWith(mine)) await c.delete(k);
    }
    await c.put(url(name), new Response(bytes));
  } catch (e) {}
}
"#)]
    extern "C" {
        pub fn vc_cache_build() -> String;
        pub fn vc_cache_get(name: &str) -> js_sys::Promise;
        pub fn vc_cache_put(name: &str, bytes: &js_sys::Uint8Array) -> js_sys::Promise;
    }
}
