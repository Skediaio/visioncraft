#[cfg(not(target_arch = "wasm32"))]
use visioncraft::{grid, view_proj};
use visioncraft::App;
#[cfg(not(target_arch = "wasm32"))]
use visioncraft_render::Renderer;

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

/// Offscreen native bench: 300 frames per instance count through the same
/// renderer, each submitted and waited for. Not a browser measurement.
#[cfg(not(target_arch = "wasm32"))]
fn bench() {
    let (device, queue) = pollster::block_on(visioncraft_render::headless_device()).expect("device");
    let (w, h) = (1280, 720);
    for count in [5000, 50_000] {
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(w, h);
        r.set_instances(&grid(count));
        let mut frame = |i: u32| {
            r.set_camera(view_proj(i as f32 * 0.01, count, w as f32 / h as f32));
            let mut encoder = device.create_command_encoder(&Default::default());
            r.render(&mut encoder);
            queue.submit([encoder.finish()]);
            device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        };
        (0..30).for_each(&mut frame);
        let start = std::time::Instant::now();
        (0..300).for_each(&mut frame);
        let ms = start.elapsed().as_secs_f64() * 1000.0 / 300.0;
        println!("offscreen native {w}x{h}, {count} cubes: {ms:.2} ms/frame ({:.0} fps)", 1000.0 / ms);
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
