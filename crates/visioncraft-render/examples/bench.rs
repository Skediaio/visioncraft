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
//! `--custom shadow,cascades,ao,contact,msaa,lod%[,scale%]` (e.g.
//! `1024,2,8,1,0,75,80`): one extra preset named "custom" (the rest as MEDIUM).
//! `--garden`: the beauty-shot garden (house, pond with planar reflection,
//! infinite ground, haze, lamps on) instead of the budget scene, with
//! `--no-haze` `--no-ground` `--no-bloom` `--no-reflection` `--no-fxaa` to
//! price each realism feature. `--forest`: 20 000 trees and 60 000
//! understory plants over 4 ha, meshes near and impostors far.
//! `--woodland`: 23 303 plants of 8 species (3 trees, 5 shrubs, each its
//! own item with two mesh levels and an impostor) over 4 ha, about the
//! app's mixed-woodland planting: many items, many draws.
//! With a device that has timestamp queries, GPU ms per pass are printed.
//! `--pipelined`: wait only for the previous frame's submission instead of
//! this one's, so recording overlaps the GPU (one frame in flight), as an
//! app's presentation loop does. Per frame the bench prints the CPU time of
//! `render()` plus `encoder.finish()` (record) and of `queue.submit`
//! (submit), and the draw calls per pass.

use glam::Vec3;
use visioncraft_render::demo::{self, BudgetOptions};
use visioncraft_render::{Atmosphere, Light, LightKind, Quality, Renderer, Sun};

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
    let pipelined = has("--pipelined");
    let (device, queue) = pollster::block_on(device_with_timestamps());
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
            render_scale: n.get(6).map_or(100, |&p| p) as f32 / 100.0,
            ..Quality::MEDIUM
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
        let quality = Quality {
            water_reflection: if has("--no-reflection") {
                0
            } else {
                quality.water_reflection
            },
            fxaa: quality.fxaa && !has("--no-fxaa"),
            ..quality
        };
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(w, h);
        r.set_quality(quality);
        let garden = has("--garden");
        let forest = has("--forest") || has("--woodland");
        let (tris, instanced) = if has("--woodland") {
            (0, woodland(&mut r))
        } else if forest {
            // 20 000 trees and 60 000 understory plants over 4 ha.
            let per = demo::forest(&mut r, 20_000, 60_000, 100.0);
            (0, per * 20_000)
        } else if garden {
            let lights = demo::garden(&mut r);
            r.set_lights(&lights);
            (0, 0)
        } else {
            demo::budget_with(&mut r, &options)
        };
        if has("--no-ground") {
            r.set_ground(None);
        }
        if has("--no-haze") {
            r.set_atmosphere(&Atmosphere {
                haze: 0.0,
                ..Default::default()
            });
        }
        if has("--no-bloom") {
            r.set_bloom(0.0);
        }
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
        let dropped = if garden { 0 } else { r.set_lights(&list) };
        let elevation = if has("--night") { -10.0 } else { 35.0 };
        r.set_sun(&Sun::from_angles(200.0, elevation));
        r.set_exposure(15.0);
        // Two camera paths: an orbit that sees the whole garden (worst case,
        // nothing to cull) and an eye-level walk through it (the product).
        for path in ["orbit", "eye-level walk"] {
            let encode = std::cell::Cell::new(0.0);
            let submit = std::cell::Cell::new(0.0);
            let finish = std::cell::Cell::new(0.0);
            let mut previous: Option<wgpu::SubmissionIndex> = None;
            let mut frame = |i: u32| {
                let aspect = w as f32 / h as f32;
                let (view, proj, eye) = if forest && path == "orbit" {
                    let yaw = i as f32 * 0.01;
                    let eye = Vec3::new(yaw.sin() * 110.0, 60.0, yaw.cos() * 110.0);
                    demo::camera(eye, Vec3::ZERO, aspect)
                } else if forest {
                    let t = i as f32 * 0.05;
                    let eye = Vec3::new(-20.0 + t, 1.7, 5.0 + 3.0 * (t * 0.1).sin());
                    demo::camera(eye, eye + Vec3::new(1.0, 0.05, -0.4), aspect)
                } else if garden && path == "orbit" {
                    // Around the house and pond at 22 m.
                    let yaw = 0.3 + i as f32 * 0.01;
                    let target = Vec3::new(2.0, 1.0, 4.0);
                    let eye = target + 22.0 * Vec3::new(yaw.sin() * 0.94, 0.33, yaw.cos() * 0.94);
                    demo::camera(eye, target, aspect)
                } else if garden {
                    // Along the terrace and the pond, eye level.
                    let t = i as f32 * 0.08;
                    let eye = Vec3::new(-6.0 + t, 1.7, 15.0 - 0.3 * t);
                    demo::camera(eye, eye + Vec3::new(0.5, -0.12, -1.0), aspect)
                } else if path == "orbit" {
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
                let finishing = std::time::Instant::now();
                let commands = encoder.finish();
                finish.set(finish.get() + finishing.elapsed().as_secs_f64() * 1000.0);
                encode.set(encode.get() + cpu.elapsed().as_secs_f64() * 1000.0);
                let cpu = std::time::Instant::now();
                let index = queue.submit([commands]);
                submit.set(submit.get() + cpu.elapsed().as_secs_f64() * 1000.0);
                let wait = if pipelined {
                    previous.replace(index)
                } else {
                    Some(index)
                };
                if let Some(index) = wait {
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: Some(index),
                            timeout: None,
                        })
                        .expect("poll");
                }
                let stats = r.stats();
                (stats.gpu_passes, stats.pass_draws)
            };
            for i in 0..20 {
                frame(i);
            }
            encode.set(0.0);
            submit.set(0.0);
            finish.set(0.0);
            let mut draws = Vec::new();
            let mut gpu: Vec<(&'static str, f32)> = Vec::new();
            let start = std::time::Instant::now();
            let mut worst = 0f64;
            for i in 0..frames {
                let t = std::time::Instant::now();
                let (timings, pass_draws) = frame(i);
                draws = pass_draws;
                worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
                for (label, ms) in timings {
                    match gpu.iter_mut().find(|(l, _)| *l == label) {
                        Some(e) => e.1 += ms,
                        None => gpu.push((label, ms)),
                    }
                }
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
            println!(
                "{w}x{h} {tris} static + {instanced} instanced (LOD 0) tris, {name}, {path}{}: {ms:.2} ms/frame ({:.1} fps), worst {worst:.1} ms, CPU record {:.2} ms (of which finish {:.2}), submit {:.2} ms{}",
                if pipelined { " pipelined" } else { "" },
                1000.0 / ms,
                encode.get() / f64::from(frames),
                finish.get() / f64::from(frames),
                submit.get() / f64::from(frames),
                if dropped > 0 {
                    format!(", {dropped} lights dropped")
                } else {
                    String::new()
                }
            );
            let parts: Vec<String> = draws.iter().map(|(l, n)| format!("{l} {n}")).collect();
            println!(
                "    draws (last frame): {} (sum {})",
                parts.join(", "),
                draws.iter().map(|d| d.1).sum::<u32>()
            );
            if !gpu.is_empty() {
                let parts: Vec<String> = gpu
                    .iter()
                    .map(|(l, ms)| format!("{l} {:.2}", ms / frames as f32))
                    .collect();
                let total: f32 = gpu.iter().map(|(_, ms)| ms).sum::<f32>() / frames as f32;
                println!("    GPU ms: {} (sum {total:.2})", parts.join(", "));
            }
        }
    }
}

/// 23 303 plants of 8 species over 200 x 200 m (see `--woodland`).
fn woodland(r: &mut Renderer) -> usize {
    use glam::{Mat4, Quat};
    let per = demo::forest(r, 0, 0, 100.0);
    let mut rng = demo::Rng::new(7);
    for s in 0..8u32 {
        let tree = s < 3;
        let count = if tree { 1_100 } else { 4_000 } + if s == 0 { 3 } else { 0 };
        let at: Vec<Mat4> = (0..count)
            .map(|_| {
                let (x, z) = (rng.unit() * 200.0 - 100.0, rng.unit() * 200.0 - 100.0);
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.7 + rng.unit() * 0.8),
                    Quat::from_rotation_y(rng.unit() * std::f32::consts::TAU),
                    Vec3::new(x, 0.0, z),
                )
            })
            .collect();
        let shape = if tree {
            demo::PlantShape::tree()
        } else {
            demo::PlantShape::shrub()
        };
        let lods = demo::plant_lods(&shape, 42 + s);
        let tint = Vec3::new(0.12 + 0.02 * s as f32, 0.26, 0.06);
        let mats = demo::plant_materials(r, tint, s);
        let distances = if tree {
            [6.0, 12.0, 300.0]
        } else {
            [3.0, 8.0, 80.0]
        };
        demo::add_plants_with_impostor(r, &lods, &mats, distances, &at);
    }
    per * 3_300
}

/// A headless device with timestamp queries when the adapter has them, so
/// the renderer reports per-pass GPU times.
async fn device_with_timestamps() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("adapter");
    eprintln!("adapter: {:?}", adapter.get_info());
    let required_features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features,
            ..Default::default()
        })
        .await
        .expect("device")
}
