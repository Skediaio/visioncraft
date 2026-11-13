use visioncraft::App;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--bench") {
        bench();
        return Ok(());
    }
    eframe::run_native(
        "VisionCraft",
        eframe::NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// Offscreen native bench of the budget scene (200k static triangles plus
/// 5000 instances of a 500-triangle mesh) at 1920x1080, per quality preset,
/// every frame submitted and waited for. Not a browser measurement.
#[cfg(not(target_arch = "wasm32"))]
fn bench() {
    use visioncraft_render::{Quality, Renderer, Sun, demo};
    let (device, queue) =
        pollster::block_on(visioncraft_render::headless_device()).expect("device");
    let (w, h) = (1920, 1080);
    for (name, quality) in Quality::PRESETS {
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(w, h);
        r.set_quality(quality);
        let (tris, instanced) = demo::budget(&mut r);
        r.set_sun(&Sun::from_angles(200.0, 35.0));
        r.set_exposure(15.0);
        // Two camera paths: an orbit that sees the whole garden (worst case,
        // nothing to cull) and an eye-level walk through it (the product).
        for path in ["orbit", "eye-level walk"] {
            let mut frame = |i: u32| {
                let aspect = w as f32 / h as f32;
                let (view, proj, eye) = if path == "orbit" {
                    visioncraft::orbit_camera(i as f32 * 0.01, 0.35, 38.0, aspect)
                } else {
                    let t = i as f32 * 0.25;
                    let eye = glam::Vec3::new(-24.0 + t, 1.7, 2.0 * (t * 0.2).sin());
                    demo::camera(
                        eye,
                        eye + glam::Vec3::new(1.0, -0.15, 0.3 * (t * 0.1).cos()),
                        aspect,
                    )
                };
                r.set_camera(view, proj, eye);
                let mut encoder = device.create_command_encoder(&Default::default());
                r.render(&mut encoder);
                queue.submit([encoder.finish()]);
                device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("poll");
            };
            (0..20).for_each(&mut frame);
            let frames = 150;
            let start = std::time::Instant::now();
            let mut worst = 0f64;
            for i in 0..frames {
                let t = std::time::Instant::now();
                frame(i);
                worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
            println!(
                "offscreen native {w}x{h}, {tris} static + {instanced} instanced triangles, {name}, {path}: {ms:.2} ms/frame ({:.1} fps), worst {worst:.1} ms",
                1000.0 / ms
            );
        }
    }
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
        let started = eframe::WebRunner::new()
            .start(canvas, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
            .await;
        if let Err(e) = started {
            web_sys::console::error_1(&format!("VisionCraft needs WebGPU: {e:?}").into());
        }
    });
}
