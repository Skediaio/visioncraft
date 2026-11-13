//! Offscreen native bench of the budget scene (200k static triangles plus
//! 5000 alpha-cut, textured, wind-animated 500-triangle shrubs at mixed
//! levels of detail) at 1920x1080, per quality preset, every frame
//! submitted and waited for. Not a browser measurement.
//!
//! `cargo run --release -p visioncraft-render --example bench -- [flags]`
//! Flags switch features off or add load, to see what each costs:
//! `--untextured` `--opaque-foliage` `--no-lod` `--no-wind` `--no-plants` `--lights N`
//! `--sun-every-frame` (re-renders the environment cube each frame)
//! `--night` (sun below the horizon: no shadow passes)
//! `--presets low,medium,high` `--frames N` `--size 1280x720`
//! `--custom shadow,cascades,ao,contact,msaa,lod%` (e.g. `1024,2,8,1,0,75`): one
//! extra preset named "custom".

use glam::Vec3;
use visioncraft_render::demo::{self, BudgetOptions};
use visioncraft_render::{Light, LightKind, Quality, Renderer, Sun, headless_device};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let options = BudgetOptions {
        textured_static: !has("--untextured"),
        cutout_foliage: !has("--opaque-foliage"),
        lods: !has("--no-lod"),
        wind: !has("--no-wind"),
        plants: !has("--no-plants"),
    };
    let lights: usize = value("--lights").and_then(|v| v.parse().ok()).unwrap_or(0);
    let frames: u32 = value("--frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    let presets = value("--presets").unwrap_or_else(|| "low,medium,high,custom".into());
    let sun_every_frame = has("--sun-every-frame");
    let (device, queue) = pollster::block_on(headless_device()).expect("device");
    let (w, h) = value("--size")
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1920, 1080));
    println!("options {options:?}, {lights} lights, sun every frame: {sun_every_frame}");
    let custom = value("--custom").map(|v| {
        let n: Vec<u32> = v.split(',').map(|x| x.parse().expect("number")).collect();
        Quality {
            shadow_resolution: n[0],
            cascades: n[1],
            ao_samples: n[2],
            contact_shadows: n[3] != 0,
            msaa: n[4] != 0,
            lod_distance_percent: n[5],
        }
    });
    let mut list = Quality::PRESETS.to_vec();
    if let Some(q) = custom {
        list.push(("custom", q));
    }
    for (name, quality) in list {
        if !presets.split(',').any(|p| p == name) {
            continue;
        }
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(w, h);
        r.set_quality(quality);
        let (tris, instanced) = demo::budget_with(&mut r, &options);
        // Lights scattered over the garden, 6 m range.
        let list: Vec<Light> = (0..lights)
            .map(|i| {
                let a = i as f32 * 2.399;
                let d = 3.0 + 22.0 * ((i as f32 + 0.5) / lights as f32).sqrt();
                Light {
                    position: Vec3::new(a.cos() * d, 1.2, a.sin() * d),
                    kind: LightKind::Point,
                    color: demo::WARM,
                    intensity: 600.0,
                    range_m: 6.0,
                }
            })
            .collect();
        let dropped = r.set_lights(&list);
        let elevation = if has("--night") { -10.0 } else { 35.0 };
        r.set_sun(&Sun::from_angles(200.0, elevation));
        r.set_exposure(15.0);
        // Two camera paths: an orbit that sees the whole garden (worst case,
        // nothing to cull) and an eye-level walk through it (the product).
        for path in ["orbit", "eye-level walk"] {
            let encode = std::cell::Cell::new(0.0);
            let mut frame = |i: u32| {
                let aspect = w as f32 / h as f32;
                let (view, proj, eye) = if path == "orbit" {
                    let (yaw, pitch, distance) = (i as f32 * 0.01, 0.35f32, 38.0);
                    let target = Vec3::new(0.0, 0.5, 0.0);
                    let eye = target
                        + distance
                            * Vec3::new(
                                yaw.sin() * pitch.cos(),
                                pitch.sin(),
                                yaw.cos() * pitch.cos(),
                            );
                    demo::camera(eye, target, aspect)
                } else {
                    let t = i as f32 * 0.25;
                    let eye = Vec3::new(-24.0 + t, 1.7, 2.0 * (t * 0.2).sin());
                    demo::camera(
                        eye,
                        eye + Vec3::new(1.0, -0.15, 0.3 * (t * 0.1).cos()),
                        aspect,
                    )
                };
                r.set_camera(view, proj, eye);
                r.set_time(i as f64 / 30.0);
                if sun_every_frame {
                    r.set_sun(&Sun::from_angles(200.0 + i as f32 * 0.1, 35.0));
                }
                let cpu = std::time::Instant::now();
                let mut encoder = device.create_command_encoder(&Default::default());
                r.render(&mut encoder);
                let commands = encoder.finish();
                encode.set(encode.get() + cpu.elapsed().as_secs_f64() * 1000.0);
                queue.submit([commands]);
                device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("poll");
            };
            (0..20).for_each(&mut frame);
            encode.set(0.0);
            let start = std::time::Instant::now();
            let mut worst = 0f64;
            for i in 0..frames {
                let t = std::time::Instant::now();
                frame(i);
                worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
            println!(
                "{w}x{h} {tris} static + {instanced} instanced (LOD 0) tris, {name}, {path}: {ms:.2} ms/frame ({:.1} fps), worst {worst:.1} ms, CPU encode {:.2} ms{}",
                1000.0 / ms,
                encode.get() / f64::from(frames),
                if dropped > 0 {
                    format!(", {dropped} lights dropped")
                } else {
                    String::new()
                }
            );
        }
    }
}
