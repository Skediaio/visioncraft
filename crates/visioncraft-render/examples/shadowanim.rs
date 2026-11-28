//! Measurement of the shadow caches while instances move (`Renderer::stale_moved`): a
//! village of 300 boxes, each its own mesh, material and item, and one door that swings on a
//! looped "clip" (`set_instance` a frame, which is what a playing glTF clip comes down to),
//! at a still camera. Per frame it records the shadow passes' draws and calls
//! (`RenderStats::pass_draws`, `pass_calls`: every cascade and the snow cover map are one
//! group), their GPU time and the CPU time of `render()`; per run it prints how often each
//! shadow draw count came up.
//!
//! `cargo run --release -p visioncraft-render --example shadowanim --`
//! `--size 640x360` `--frames 120` `--presets medium,high` `--boxes 300` `--per-frame`
//! (every measured frame's shadow draws and calls).
//! Three runs per preset and snow setting: the door near the camera (4 m), far (153 m: outside
//! the cached far cascade and the snow cover map), and a still scene (the door closed).

use std::collections::BTreeMap;

use glam::{Mat4, Vec3};
use visioncraft_render::demo::{self, MeshData};
use visioncraft_render::{Ground, Material, Quality, Renderer, Sun};

/// Where the door's hinge stands in the swinging runs (m).
const NEAR: Vec3 = Vec3::new(1.2, 0.0, 4.0);
const FAR: Vec3 = Vec3::new(30.0, 0.0, 150.0);
const WARMUP: u32 = 20;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let frames: u32 = value("--frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let boxes: usize = value("--boxes").and_then(|v| v.parse().ok()).unwrap_or(300);
    let presets = value("--presets").unwrap_or_else(|| "medium,high".into());
    let per_frame = args.iter().any(|a| a == "--per-frame");
    let (w, h) = value("--size")
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((640, 360));
    let (device, queue) = pollster::block_on(device_with_timestamps());
    for (name, quality) in Quality::PRESETS {
        if !presets.split(',').any(|p| p == name) {
            continue;
        }
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(w, h);
        r.set_quality(quality);
        let items = village(&mut r, boxes);
        let door = swinging_door(&mut r);
        let scene = Scene {
            door,
            items: items + 1,
            frames,
            preset: name,
        };
        r.set_sun(&Sun::from_angles(200.0, 35.0));
        let eye = Vec3::new(0.0, 1.7, 0.0);
        let view = Mat4::look_at_rh(eye, eye + Vec3::new(0.2, -0.07, 1.0), Vec3::Y);
        let proj = Mat4::perspective_rh(50f32.to_radians(), w as f32 / h as f32, 0.1, 2000.0);
        r.set_camera(view, proj, eye);
        for snow in [0.0, 0.5] {
            r.set_snow(snow);
            let (near, far, still) = (
                Run {
                    name: "near",
                    hinge: NEAR,
                    swing: true,
                },
                Run {
                    name: "far",
                    hinge: FAR,
                    swing: true,
                },
                Run {
                    name: "still",
                    hinge: NEAR,
                    swing: false,
                },
            );
            run(&mut r, &device, &queue, &scene, snow, &near, per_frame);
            run(&mut r, &device, &queue, &scene, snow, &far, per_frame);
            run(&mut r, &device, &queue, &scene, snow, &still, per_frame);
        }
    }
}

/// `boxes` boxes of 5 to 7 m footprint and 2.5 to 7 m height on a 10 m grid, each its own mesh,
/// material (one of eight colour families, varied) and item with one instance.
fn village(r: &mut Renderer, boxes: usize) -> usize {
    r.set_ground(Some(Ground::default()));
    let mut rng = demo::Rng::new(11);
    let cols = 20;
    let mut items = 0;
    for i in 0..boxes {
        let (x, z) = (
            ((i % cols) * 10) as f32 - 95.0,
            ((i / cols) * 10) as f32 - 70.0,
        );
        // Not where the camera and the near door stand.
        if x.abs() < 6.0 && z > -6.0 && z < 12.0 {
            continue;
        }
        let (sx, sz, h) = (
            5.0 + 2.0 * rng.unit(),
            5.0 + 2.0 * rng.unit(),
            2.5 + 4.5 * rng.unit(),
        );
        let mut m = MeshData::default();
        m.add_box(
            Vec3::new(-sx / 2.0, 0.0, -sz / 2.0),
            Vec3::new(sx / 2.0, h, sz / 2.0),
        );
        let mesh = r.add_mesh(&m.mesh()).expect("mesh");
        let family = (i % 8) as f32 / 8.0;
        let light = 0.3 + 0.5 * rng.unit();
        let material = r.add_material(&Material {
            base_color: [
                light * (0.9 - 0.6 * family),
                light * (0.5 + 0.3 * (family - 0.5).abs()),
                light * (0.2 + 0.6 * family),
                1.0,
            ],
            roughness: 0.85,
            ..Default::default()
        });
        r.add_item(
            mesh,
            material,
            &[Mat4::from_translation(Vec3::new(x, 0.0, z))],
        );
        items += 1;
    }
    items
}

/// One of the three runs per preset and snow setting.
struct Run {
    /// `near` or `far`: where `hinge` puts the door.
    name: &'static str,
    hinge: Vec3,
    /// The door swings from closed to 80 degrees open and back, or stays closed (a still scene).
    swing: bool,
}

/// What every run of one renderer shares.
struct Scene {
    door: visioncraft_render::ItemId,
    items: usize,
    frames: u32,
    preset: &'static str,
}

/// A door 1 x 2.1 m, 8 cm thick, its hinge on the instance's origin: rotating the instance
/// about y swings it.
fn swinging_door(r: &mut Renderer) -> visioncraft_render::ItemId {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(0.0, 0.0, -0.04), Vec3::new(1.0, 2.1, 0.04));
    let mesh = r.add_mesh(&m.mesh()).expect("mesh");
    let material = r.add_material(&Material {
        base_color: [0.35, 0.2, 0.08, 1.0],
        roughness: 0.6,
        ..Default::default()
    });
    r.add_item(mesh, material, &[Mat4::from_translation(NEAR)])
}

struct Frame {
    shadow_draws: u32,
    shadow_calls: u32,
    draws: u32,
    gpu_shadow_ms: f32,
    render_ms: f64,
    finish_ms: f64,
}

/// One run: `WARMUP` frames with the door closed where its hinge puts it, then `frames`
/// measured ones, the door swinging from closed to 80 degrees open and back (a cosine ease over
/// the run, as a looped clip) or staying closed.
fn run(
    r: &mut Renderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
    snow: f32,
    run: &Run,
    per_frame: bool,
) {
    let closed = Mat4::from_translation(run.hinge);
    r.set_instance(scene.door, 0, closed);
    let frame = |r: &mut Renderer, angle: f32| -> Frame {
        if angle != 0.0 {
            r.set_instance(scene.door, 0, closed * Mat4::from_rotation_y(angle));
        }
        let start = std::time::Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        r.render(&mut encoder);
        let rendered = std::time::Instant::now();
        let commands = encoder.finish();
        let finished = std::time::Instant::now();
        let index = queue.submit([commands]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: None,
            })
            .expect("poll");
        let stats = r.stats();
        let of = |list: &[(&'static str, u32)], label: &str| {
            list.iter()
                .find(|(l, _)| *l == label)
                .map_or(0, |(_, n)| *n)
        };
        Frame {
            shadow_draws: of(&stats.pass_draws, "shadow"),
            shadow_calls: of(&stats.pass_calls, "shadow"),
            draws: stats.draw_calls,
            gpu_shadow_ms: stats
                .gpu_passes
                .iter()
                .find(|(l, _)| *l == "shadow")
                .map_or(0.0, |(_, ms)| *ms),
            render_ms: rendered.duration_since(start).as_secs_f64() * 1000.0,
            finish_ms: finished.duration_since(rendered).as_secs_f64() * 1000.0,
        }
    };
    for _ in 0..WARMUP {
        frame(r, 0.0);
    }
    let mut histogram: BTreeMap<u32, u32> = BTreeMap::new();
    let (mut calls, mut draws, mut gpu, mut render, mut finish) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..scene.frames {
        let t = i as f32 / scene.frames as f32;
        let angle = if run.swing {
            (0.5 - 0.5 * (std::f32::consts::TAU * t).cos()) * 80f32.to_radians()
        } else {
            0.0
        };
        let f = frame(r, angle);
        if per_frame {
            println!(
                "    frame {i}: shadow draws {}, calls {}",
                f.shadow_draws, f.shadow_calls
            );
        }
        *histogram.entry(f.shadow_draws).or_default() += 1;
        calls += f64::from(f.shadow_calls);
        draws += f64::from(f.draws);
        gpu += f64::from(f.gpu_shadow_ms);
        render += f.render_ms;
        finish += f.finish_ms;
    }
    let n = f64::from(scene.frames);
    let hist: Vec<String> = histogram
        .iter()
        .map(|(d, c)| format!("{d} draws x {c} frames"))
        .collect();
    println!(
        "{:>6}, snow {snow}: door {}{}: shadow draws per frame [{}]; shadow calls {:.2}/frame, all draws {:.0}/frame, GPU shadow {:.2} ms, CPU render {:.3} ms + finish {:.3} ms",
        scene.preset,
        run.name,
        if run.swing { ", swinging" } else { ", closed" },
        hist.join("; "),
        calls / n,
        draws / n,
        gpu / n,
        render / n,
        finish / n,
    );
    println!(
        "    ({} items, {} measured frames after {WARMUP})",
        scene.items, scene.frames
    );
}

/// A headless device with timestamp queries when the adapter has them (as `bench`).
async fn device_with_timestamps() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("adapter");
    eprintln!("adapter: {:?}", adapter.get_info());
    let required_features = adapter.features()
        & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::INDIRECT_FIRST_INSTANCE);
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features,
            ..Default::default()
        })
        .await
        .expect("device")
}
