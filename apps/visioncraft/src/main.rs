use visioncraft::App;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
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
        if let Err(e) = visioncraft::agents::serve_headless(port, token, secs) {
            eprintln!("agents failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
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
        if let Err(e) = visioncraft::smoke::run(script.as_ref(), o.as_ref()) {
            eprintln!("smoke failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    eframe::run_native(
        "VisionCraft",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast;
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
            .start(canvas, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
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
