//! The starting quality from this device's hardware: on first start the renderer's adapter probe
//! and a short calibration (`visioncraft_render::calibrate`) pick a preset and render scale for
//! the canvas' physical size (`starting_quality`); Auto fine-tunes from there. The result is kept
//! per adapter and canvas size class, so it runs again only when either changes or the person
//! asks (Re-measure in the Quality menu).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use visioncraft_render::{HardwareProbe, Quality, Score, starting_quality};

/// eframe storage key: a JSON map from `key` to `Measured`.
pub const STORAGE_KEY: &str = "visioncraft.hardware";

/// What one device measured and the tier it chose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    /// What was detected, for display ("Intel gen-9").
    pub gpu: String,
    /// The calibration's cost per megapixel; `None` when it could not run (the adapter kind chose).
    pub ms_per_megapixel: Option<f32>,
    pub fixed_ms: f32,
    pub gpu_timed: bool,
    /// The preset's name (`Quality::PRESETS`) and the render scale.
    pub preset: String,
    pub scale: f32,
}

impl Measured {
    pub fn quality(&self) -> Quality {
        let base = Quality::PRESETS
            .iter()
            .find(|(n, _)| *n == self.preset)
            .map_or(Quality::MEDIUM, |p| p.1);
        Quality {
            render_scale: self.scale.clamp(0.5, 1.0),
            ..base
        }
    }

    /// One line for the Quality menu: "Intel gen-9, 10 ms per megapixel: Medium at 75%".
    pub fn line(&self) -> String {
        let score = match self.ms_per_megapixel {
            Some(ms) if ms < 1.0 => format!("{ms:.2} ms per megapixel"),
            Some(ms) => format!("{ms:.1} ms per megapixel"),
            None => "not measured".into(),
        };
        let mut preset = self.preset.clone();
        if let Some(c) = preset.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        format!(
            "{}, {score}: {preset} at {:.0}%",
            self.gpu,
            self.scale * 100.0
        )
    }
}

/// Canvas sizes that start alike: about HD, full HD, QHD, 4K and up (physical pixels).
pub fn size_class((w, h): (u32, u32)) -> &'static str {
    let mp = w as f32 * h as f32 / 1e6;
    match mp {
        m if m < 1.2 => "hd",
        m if m < 2.6 => "fhd",
        m if m < 5.0 => "qhd",
        _ => "uhd",
    }
}

pub fn key(gpu: &str, size: (u32, u32)) -> String {
    format!("{gpu}|{}", size_class(size))
}

/// The tier for this probe, score and size, with what to show and keep.
pub fn choose(
    probe: &HardwareProbe,
    gpu: &str,
    score: Option<Score>,
    size: (u32, u32),
) -> Measured {
    let q = starting_quality(probe, score, size);
    let preset = Quality::PRESETS
        .iter()
        .find(|(_, p)| {
            *p == Quality {
                render_scale: p.render_scale,
                ..q
            }
        })
        .map_or("medium", |p| p.0);
    Measured {
        gpu: gpu.to_string(),
        ms_per_megapixel: score.map(|s| s.ms_per_megapixel),
        fixed_ms: score.map_or(0.0, |s| s.fixed_ms),
        gpu_timed: score.is_some_and(|s| s.gpu_timed),
        preset: preset.into(),
        scale: q.render_scale,
    }
}

type Slot<T> = Arc<Mutex<Option<T>>>;

/// The per-start state: the adapter's display name (from JS on the web, which wgpu leaves
/// empty), a calibration under way, and the key of the tier in use.
pub struct Hardware {
    probe: HardwareProbe,
    gpu: Slot<String>,
    measuring: Option<(String, Slot<Result<Score, String>>)>,
    key: Option<String>,
}

impl Hardware {
    pub fn new(probe: HardwareProbe) -> Self {
        let gpu: Slot<String> = Arc::default();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let name = if probe.name.is_empty() {
                probe.backend.clone()
            } else {
                probe.name.clone()
            };
            *gpu.lock().expect("gpu") = Some(name);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let (slot, fallback) = (gpu.clone(), probe.name.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let name = web_adapter_name().await.unwrap_or_default();
                let name = if name.is_empty() { fallback } else { name };
                let name = if name.is_empty() {
                    "Unknown GPU".to_string()
                } else {
                    name
                };
                *slot.lock().expect("gpu") = Some(name);
            });
        }
        Self {
            probe,
            gpu,
            measuring: None,
            key: None,
        }
    }

    /// Once a frame with the canvas' physical size. Returns the tier to start from when one was
    /// chosen (from storage, or measured now) and what to show; `remeasure` forces a calibration.
    pub fn pump(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        storage: Option<&mut (dyn eframe::Storage + 'static)>,
        size: (u32, u32),
        remeasure: &mut bool,
    ) -> Option<Measured> {
        if size.0 < 16 || size.1 < 16 {
            return None;
        }
        let gpu = self.gpu.lock().ok()?.clone()?;
        if let Some((k, slot)) = &self.measuring {
            let result = slot.lock().ok()?.take()?;
            let k = k.clone();
            self.measuring = None;
            if let Err(e) = &result {
                note(&format!(
                    "calibration failed, the adapter kind chooses: {e}"
                ));
            }
            let m = choose(&self.probe, &gpu, result.ok(), size);
            if let Some(st) = storage {
                let mut all = stored(&*st);
                all.insert(k.clone(), m.clone());
                if let Ok(j) = serde_json::to_string(&all) {
                    st.set_string(STORAGE_KEY, j);
                }
            }
            self.key = Some(k);
            return Some(m);
        }
        let k = key(&gpu, size);
        if !*remeasure && self.key.as_deref() == Some(&k) {
            return None;
        }
        if !*remeasure && let Some(m) = storage.and_then(|st| stored(&*st).remove(&k)) {
            self.key = Some(k);
            return Some(m);
        }
        *remeasure = false;
        let slot: Slot<Result<Score, String>> = Arc::default();
        self.measuring = Some((k, slot.clone()));
        let (device, queue) = (device.clone(), queue.clone());
        let run = async move {
            let clock = clock();
            let r = visioncraft_render::calibrate(&device, &queue, Some(&clock)).await;
            if let Ok(mut s) = slot.lock() {
                *s = Some(r);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        pollster::block_on(run);
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(run);
        None
    }

    /// Measuring now (the menu says so).
    pub fn busy(&self) -> bool {
        self.measuring.is_some()
    }
}

/// `visioncraft --hardware`: the probe, a calibration on a headless device, and the tier each
/// common canvas size starts on.
#[cfg(not(target_arch = "wasm32"))]
pub fn report() -> Result<(), String> {
    let (device, queue) = pollster::block_on(visioncraft_render::headless_device())?;
    let probe = HardwareProbe::from_device(&device);
    let clock = clock();
    let score = pollster::block_on(visioncraft_render::calibrate(&device, &queue, Some(&clock)))?;
    println!(
        "{} ({:?}, {}), timestamp queries: {}",
        probe.name, probe.kind, probe.backend, probe.timestamp_query
    );
    println!(
        "score: {:.2} ms per megapixel + {:.2} ms fixed ({})",
        score.ms_per_megapixel,
        score.fixed_ms,
        if score.gpu_timed {
            "GPU timed"
        } else {
            "clock"
        }
    );
    for size in [(1280, 720), (1920, 1080), (2560, 1300), (3840, 2160)] {
        let m = choose(&probe, &probe.name, Some(score), size);
        println!("{}x{}: {}", size.0, size.1, m.line());
    }
    Ok(())
}

fn stored(st: &dyn eframe::Storage) -> HashMap<String, Measured> {
    st.get_string(STORAGE_KEY)
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

/// Milliseconds from any origin: `performance.now()` on the web.
fn clock() -> impl Fn() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let start = std::time::Instant::now();
        move || start.elapsed().as_secs_f64() * 1000.0
    }
    #[cfg(target_arch = "wasm32")]
    {
        let perf = web_sys::window().and_then(|w| w.performance());
        move || perf.as_ref().map_or_else(js_sys::Date::now, |p| p.now())
    }
}

/// A line on stderr, or the browser console.
pub fn note(msg: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{msg}");
    #[cfg(target_arch = "wasm32")]
    web_sys::console::info_1(&msg.into());
}

/// `GPUAdapterInfo` as the browser gives it ("Intel gen-9"): wgpu leaves vendor and kind empty.
#[cfg(target_arch = "wasm32")]
async fn web_adapter_name() -> Option<String> {
    use wasm_bindgen::JsCast;
    let f = js_sys::Function::new_no_args(
        "const g = navigator.gpu; if (!g) return Promise.resolve(''); \
         return g.requestAdapter().then(a => { if (!a || !a.info) return ''; \
         const i = a.info; const v = i.vendor ? i.vendor[0].toUpperCase() + i.vendor.slice(1) : ''; \
         const n = [v, i.architecture].filter(Boolean).join(' '); \
         return n || i.description || ''; }).catch(() => '');",
    );
    let promise = f.call0(&wasm_bindgen::JsValue::NULL).ok()?;
    let promise = promise.dyn_into::<js_sys::Promise>().ok()?;
    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .ok()?
        .as_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_render::GpuKind;

    fn probe(kind: GpuKind) -> HardwareProbe {
        HardwareProbe {
            name: String::new(),
            vendor: 0,
            device: 0,
            kind,
            backend: "BrowserWebGpu".into(),
            driver: String::new(),
            subgroup_sizes: (0, 0),
            max_texture_dimension_2d: 8192,
            max_bind_groups: 4,
            max_storage_buffers_per_shader_stage: 8,
            max_uniform_buffer_binding_size: 65536,
            max_storage_buffer_binding_size: 128 << 20,
            timestamp_query: true,
            float32_filterable: true,
            compression_bc: true,
            compression_astc: false,
            compression_etc2: false,
        }
    }

    fn score(ms_per_megapixel: f32) -> Option<Score> {
        Some(Score {
            fixed_ms: 0.4,
            ms_per_megapixel,
            gpu_timed: true,
        })
    }

    #[test]
    fn a_strong_desktop_gpu_at_2560_by_1300_starts_on_ultra_at_full_scale() {
        // A desktop card at 170 fps on HIGH: a fraction of a millisecond per megapixel.
        let m = choose(
            &probe(GpuKind::Unknown),
            "Nvidia ampere",
            score(0.3),
            (2560, 1300),
        );
        assert_eq!(m.preset, "ultra");
        assert_eq!(m.scale, 1.0);
        assert_eq!(m.quality(), Quality::ULTRA);
        assert_eq!(
            m.line(),
            "Nvidia ampere, 0.30 ms per megapixel: Ultra at 100%"
        );
        // The laptop this was tuned on lands lower, at a reduced scale.
        let slow = choose(
            &probe(GpuKind::Integrated),
            "Intel gen-9",
            score(10.0),
            (1920, 1080),
        );
        assert_ne!(slow.preset, "ultra");
        assert!(slow.scale < 1.0, "{slow:?}");
        assert!(
            slow.line()
                .starts_with("Intel gen-9, 10.0 ms per megapixel: ")
        );
    }

    #[test]
    fn the_key_follows_the_adapter_and_the_size_class() {
        assert_eq!(key("A", (2560, 1300)), key("A", (2560, 1440)));
        assert_ne!(key("A", (1280, 720)), key("A", (2560, 1300)));
        assert_ne!(key("A", (1920, 1080)), key("B", (1920, 1080)));
        let m = choose(&probe(GpuKind::Discrete), "B", None, (1920, 1080));
        let back: Measured = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.quality(), Quality::HIGH);
    }
}
