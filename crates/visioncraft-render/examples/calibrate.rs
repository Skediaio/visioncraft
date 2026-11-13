//! Prints the hardware probe, three calibration scores and the starting
//! quality for a few output sizes, on a headless device (with timestamp
//! queries when the adapter has them) and on one without (timed with the
//! wall clock).
//!
//! `cargo run --release -p visioncraft-render --example calibrate`

use visioncraft_render::{HardwareProbe, calibrate, headless_device, starting_quality};

fn main() {
    let (device, queue) = pollster::block_on(headless_device()).expect("device");
    let probe = HardwareProbe::from_device(&device);
    println!("{probe:#?}");
    let start = std::time::Instant::now();
    let clock = move || start.elapsed().as_secs_f64() * 1000.0;
    for run in 0..3 {
        let t = std::time::Instant::now();
        let score = pollster::block_on(calibrate(&device, &queue, Some(&clock))).expect("score");
        println!(
            "run {run}: {score:?} in {:.0} ms",
            t.elapsed().as_secs_f64() * 1000.0
        );
        for size in [(1280, 720), (1920, 1080), (2560, 1300), (3840, 2160)] {
            let q = starting_quality(&probe, Some(score), size);
            let name = visioncraft_render::Quality::PRESETS
                .iter()
                .find(|(_, p)| {
                    p.render_scale == q.render_scale
                        && p.shadow_resolution == q.shadow_resolution
                        && p.msaa == q.msaa
                        && p.lod_distance_percent == q.lod_distance_percent
                })
                .map_or("(scaled)", |(n, _)| n);
            println!(
                "    {size:?}: {name} {:?} render scale {:.2}",
                (q.shadow_resolution, q.msaa, q.lod_distance_percent),
                q.render_scale
            );
        }
    }
    // The same without timestamp queries: the wall clock around submit-and-wait.
    let (device, queue) = pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("adapter");
        adapter
            .request_device(&Default::default())
            .await
            .expect("device")
    });
    for run in 0..3 {
        let t = std::time::Instant::now();
        let score = pollster::block_on(calibrate(&device, &queue, Some(&clock))).expect("score");
        println!(
            "clock run {run}: {score:?} in {:.0} ms",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}
