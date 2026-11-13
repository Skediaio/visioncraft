//! The app shell: the window, the page and the command-line modes, with the extensions a build
//! compiles in. `src/main.rs` is `Shell::new().run()`; a third-party build adds `.with(...)`.

use crate::App;
use crate::extension::Extension;

#[derive(Default)]
pub struct Shell {
    extensions: Vec<Box<dyn Extension>>,
}

impl Shell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, extension: impl Extension) -> Self {
        self.extensions.push(Box::new(extension));
        self
    }

    /// Runs the window (natively) or the page (wasm32) until it closes.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn run(self) {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--agents-headless") {
            let (Some(port), Some(token), Some(secs)) = (
                args.get(i + 1).and_then(|p| p.parse().ok()),
                args.get(i + 2),
                args.get(i + 3).and_then(|s| s.parse().ok()),
            ) else {
                eprintln!("usage: visioncraft --agents-headless PORT TOKEN_FILE SECONDS");
                std::process::exit(2);
            };
            if let Err(e) = crate::agents::serve_headless(port, token, secs) {
                eprintln!("agents failed: {e}");
                std::process::exit(1);
            }
            return;
        }
        if let Some(i) = args.iter().position(|a| a == "--smoke") {
            let (Some(script), Some(o)) = (
                args.get(i + 1),
                args.iter()
                    .position(|a| a == "--out")
                    .and_then(|o| args.get(o + 1)),
            ) else {
                eprintln!("usage: visioncraft --smoke <script.json> --out <dir>");
                std::process::exit(2);
            };
            if let Err(e) = crate::smoke::run(script.as_ref(), o.as_ref()) {
                eprintln!("smoke failed: {e}");
                std::process::exit(1);
            }
            return;
        }
        let run = eframe::run_native(
            "VisionCraft",
            eframe::NativeOptions {
                viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
                ..Default::default()
            },
            Box::new(|cc| Ok(Box::new(App::new(cc, self.extensions)))),
        );
        if let Err(e) = run {
            eprintln!("VisionCraft could not start: {e}");
            std::process::exit(1);
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn run(self) {
        use eframe::wasm_bindgen::JsCast;
        let extensions = self.extensions;
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("visioncraft_canvas"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("canvas #visioncraft_canvas");
        let mut options = eframe::WebOptions::default();
        // WebGPU only: no WebGL2 fallback.
        let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
        setup.instance_descriptor.backends = wgpu::Backends::BROWSER_WEBGPU;
        options.wgpu_options.wgpu_setup = egui_wgpu::WgpuSetup::CreateNew(setup);
        wasm_bindgen_futures::spawn_local(async move {
            // index.html checks WebGPU and shows the message itself when it is unusable.
            let window = web_sys::window().expect("window");
            if let Ok(ready) = js_sys::Reflect::get(&window, &"__vcReady".into())
                && let Ok(promise) = ready.dyn_into::<js_sys::Promise>()
                && wasm_bindgen_futures::JsFuture::from(promise).await.is_err()
            {
                return;
            }
            let started = eframe::WebRunner::new()
                .start(
                    canvas,
                    options,
                    Box::new(|cc| Ok(Box::new(App::new(cc, extensions)))),
                )
                .await;
            let report = |name: &str, args: &[wasm_bindgen::JsValue]| {
                if let Ok(f) = js_sys::Reflect::get(&window, &name.into())
                    && let Ok(f) = f.dyn_into::<js_sys::Function>()
                {
                    let _ = match args {
                        [a] => f.call1(&window, a),
                        [a, b] => f.call2(&window, a, b),
                        _ => f.call0(&window),
                    };
                }
            };
            match started {
                Ok(()) => report("vcHide", &[]),
                Err(e) => {
                    web_sys::console::error_1(&format!("VisionCraft needs WebGPU: {e:?}").into());
                    report(
                        "vcFail",
                        &[
                            "VisionCraft could not start".into(),
                            format!("{e:?}").into(),
                        ],
                    );
                }
            }
        });
    }
}
