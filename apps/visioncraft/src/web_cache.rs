//! The browser's copy of `plant_cache`: generated plant meshes and textures and block-compressed
//! library maps and model pictures, kept in the Cache API (`caches`, per origin), so opening a
//! design a second time loads them instead of generating, decoding and compressing everything
//! again (seconds of worker time per species and map on the web).
//!
//! An entry's name is the `plant_cache::key` of what made it (the same parts as on the desktop),
//! a kind, and `plant_cache::GENERATOR_FINGERPRINT` (a hash of the generators' source, not of
//! the whole build), so a changed generator never reads an old result while a deploy that leaves
//! them alone keeps the entries. The first store of a fingerprint drops the entries of others. Natively there is nothing here
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
    let _ = js::vc_cache_put(
        &name(kind, key),
        &js_sys::Uint8Array::from(bytes),
        &suffix(),
    );
}

/// The tail of every entry name; the sweep keeps the entries that end in it.
#[cfg(target_arch = "wasm32")]
fn suffix() -> String {
    format!("-{:016x}", crate::plant_cache::GENERATOR_FINGERPRINT)
}

#[cfg(target_arch = "wasm32")]
fn name(kind: &str, key: u64) -> String {
    format!("{kind}-{key:016x}{}", suffix())
}

#[cfg(target_arch = "wasm32")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(inline_js = r#"
const NAME = 'visioncraft-work-v1';
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
// Entries of other generator fingerprints are dropped once, the first time one stores something.
let swept = false;
export async function vc_cache_put(name, bytes, suffix) {
  try {
    if (typeof caches === 'undefined') return;
    const c = await caches.open(NAME);
    if (!swept) {
      swept = true;
      for (const k of await c.keys()) if (!k.url.endsWith(suffix)) await c.delete(k);
    }
    await c.put(url(name), new Response(bytes));
  } catch (e) {}
}
"#)]
    extern "C" {
        pub fn vc_cache_get(name: &str) -> js_sys::Promise;
        pub fn vc_cache_put(
            name: &str,
            bytes: &js_sys::Uint8Array,
            suffix: &str,
        ) -> js_sys::Promise;
    }
}
