//! The volumetric clouds' temporal upsampling (clouds.rs) leaves no dither: at a quarter and a
//! third of the render size, where the march samples one texel of each 2x2 block of the
//! history a frame, the clouds stay soft right after the history starts over.
//!
//! Each frame is measured against the clouds converged at the same view and moment (a picture,
//! eight jitters a texel): the fine part of their difference (`dither`). Every run prints the
//! measures right after a restart, settled and panning, and writes the frames, the truth and
//! their difference (eight times as strong) to target/tmp/clouds/.
//!
//! Runs on any adapter, a software one included (slow but small).

use std::path::PathBuf;
use std::sync::OnceLock;

use glam::{Mat4, Vec3};
use visioncraft_render::demo;
use visioncraft_render::{Clouds, Quality, Renderer, Sky, headless_device};

const W: u32 = 480;
const H: u32 = 272;

fn gpu() -> &'static (wgpu::Device, wgpu::Queue) {
    static GPU: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();
    GPU.get_or_init(|| {
        pollster::block_on(headless_device()).expect("headless wgpu device (no adapter?)")
    })
}

/// Renders one frame and reads the output back as tightly packed RGBA8.
fn capture(r: &mut Renderer) -> Vec<u8> {
    let (device, queue) = gpu();
    let mut encoder = device.create_command_encoder(&Default::default());
    r.render(&mut encoder);
    let stride = (W * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (stride * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        r.output_texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |res| res.expect("map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let padded = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped range")
        .to_vec();
    padded
        .chunks(stride as usize)
        .flat_map(|row| &row[..(W * 4) as usize])
        .copied()
        .collect()
}

/// Writes the picture, and a corner of the sky four times as large (`<name>_zoom`).
fn write_png(name: &str, pixels: &[u8]) {
    save(name, pixels, W, H);
    let (x0, y0, w, h) = (240, 0, 120, 68);
    let zoom: Vec<u8> = (0..h * 4)
        .flat_map(|y| (0..w * 4).map(move |x| (x0 + x / 4, y0 + y / 4)))
        .flat_map(|(x, y)| {
            let i = ((y * W + x) * 4) as usize;
            pixels[i..i + 4].to_vec()
        })
        .collect();
    save(&format!("{name}_zoom"), &zoom, w * 4, h * 4);
}

fn save(name: &str, pixels: &[u8], w: u32, h: u32) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("clouds");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).expect("png file");
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(pixels)
        .expect("png data");
}

fn sky_renderer(div: u32) -> Renderer {
    let (device, queue) = gpu();
    let mut r = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    r.resize(W, H);
    r.set_quality(Quality {
        render_scale: 1.0,
        msaa: false,
        fxaa: false,
        clouds: div,
        ..Quality::MEDIUM
    });
    demo::spheres(&mut r);
    r.set_sun(&demo::noon_sun());
    r.set_exposure(15.0);
    r.set_sky(&Sky::Physical).unwrap();
    r.set_clouds(&Clouds {
        cover: 0.6,
        volumetric: true,
        speed_mps: Some(12.0),
        ..Default::default()
    });
    r
}

/// Looking up at the clouds, turned `yaw` degrees.
fn look(r: &mut Renderer, yaw: f32) {
    let eye = Vec3::new(0.0, 1.5, 6.0);
    let a = yaw.to_radians();
    let target = eye + Vec3::new(a.sin() * 20.0, 9.0, -a.cos() * 20.0);
    let (view, proj, eye): (Mat4, Mat4, Vec3) = demo::camera(eye, target, W as f32 / H as f32);
    r.set_camera(view, proj, eye);
}

/// The dither of a frame against the clouds converged at the same moment (`truth`, a picture):
/// over the clouds in the upper half (grey pixels of the truth, not the blue sky), the RMS of
/// their difference less its `p` x `p` box average, `p` the march grid's period in pixels. A
/// lattice (or any dither) at the march's or the history's texels shows here; a slow, smooth
/// lag behind the truth does not. And the part of it fixed to the march's grid: the RMS of its
/// means per phase (x mod p, y mod p).
fn dither(frame: &[u8], truth: &[u8], p: u32) -> (f32, f32) {
    let at = |x: u32, y: u32| ((y * W + x) * 4) as usize;
    let diff = |x: u32, y: u32| {
        let i = at(x, y);
        (0..3)
            .map(|c| f32::from(frame[i + c]) - f32::from(truth[i + c]))
            .sum::<f32>()
            / 3.0
    };
    let mut sq = 0.0f64;
    let mut count = 0u32;
    let mut phase = vec![(0.0f64, 0u32); (p * p) as usize];
    for y in p..H / 2 {
        for x in p..W - p {
            let i = at(x, y);
            if i32::from(truth[i + 2]) - i32::from(truth[i]) >= 25 {
                continue;
            }
            let mut box_mean = 0.0;
            for dy in 0..p {
                for dx in 0..p {
                    box_mean += diff(x + dx - p / 2, y + dy - p / 2);
                }
            }
            let h = f64::from(diff(x, y) - box_mean / (p * p) as f32);
            sq += h * h;
            count += 1;
            let k = &mut phase[((y % p) * p + x % p) as usize];
            k.0 += h;
            k.1 += 1;
        }
    }
    assert!(count > 2000, "too little cloud to measure: {count} pixels");
    let grid = phase
        .iter()
        .map(|(s, n)| (s / f64::from((*n).max(1))).powi(2))
        .sum::<f64>()
        / phase.len() as f64;
    ((sq / f64::from(count)).sqrt() as f32, grid.sqrt() as f32)
}

/// The frames are 1/60 s apart; the clouds drift.
fn at_frame(r: &mut Renderer, frame: u32) -> Vec<u8> {
    r.set_time(f64::from(frame) / 60.0);
    capture(r)
}

/// The clouds converged at a view and a moment: a picture (eight jitters of every texel), on a
/// renderer of its own.
fn truth(r: &mut Renderer, yaw: f32, frame: u32) -> Vec<u8> {
    look(r, yaw);
    r.set_time(f64::from(frame) / 60.0);
    r.finish_environment();
    capture(r)
}

/// Writes a frame, the truth and their difference eight times as strong round mid grey.
fn write_compared(name: &str, frame: &[u8], truth: &[u8]) {
    write_png(name, frame);
    write_png(&format!("{name}_truth"), truth);
    let diff: Vec<u8> = frame
        .iter()
        .zip(truth)
        .enumerate()
        .map(|(i, (a, b))| {
            if i % 4 == 3 {
                255
            } else {
                (128.0 + 8.0 * (f32::from(*a) - f32::from(*b))).clamp(0.0, 255.0) as u8
            }
        })
        .collect();
    write_png(&format!("{name}_error"), &diff);
}

fn worst(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0.max(b.0), a.1.max(b.1))
}

/// The dither (`dither`) of the clouds at a march divisor: right after the history starts over
/// (a cut), once settled, and while the view pans half a degree a frame; each the worst of the
/// frames looked at.
fn run(div: u32) -> [(f32, f32); 3] {
    let mut r = sky_renderer(div);
    let mut still = sky_renderer(div);
    let p = div;
    let mut frame = 0;
    let mut measure = |r: &mut Renderer, frame: u32, yaw: f32, name: Option<&str>| {
        let px = at_frame(r, frame);
        let t = truth(&mut still, yaw, frame);
        if let Some(name) = name {
            write_compared(&format!("div{div}_{name}"), &px, &t);
        }
        dither(&px, &t, p)
    };
    look(&mut r, 0.0);
    // Settle into the first view.
    for _ in 0..8 {
        frame += 1;
        at_frame(&mut r, frame);
    }
    // A cut: the history starts over. The first four frames after it.
    let mut restart = (0.0, 0.0);
    look(&mut r, 30.0);
    for k in 0..4 {
        frame += 1;
        let name = [Some("restart1"), None, Some("restart"), None][k];
        restart = worst(restart, measure(&mut r, frame, 30.0, name));
    }
    for _ in 0..24 {
        frame += 1;
        at_frame(&mut r, frame);
    }
    let mut settled = (0.0, 0.0);
    for k in 0..2 {
        frame += 1;
        let name = (k == 0).then_some("settled");
        settled = worst(settled, measure(&mut r, frame, 30.0, name));
    }
    let mut panning = (0.0, 0.0);
    for k in 0..12 {
        frame += 1;
        let yaw = 30.0 + 0.5 * (k + 1) as f32;
        look(&mut r, yaw);
        if k >= 10 {
            let name = (k == 11).then_some("panning");
            panning = worst(panning, measure(&mut r, frame, yaw, name));
        } else {
            at_frame(&mut r, frame);
        }
    }
    [restart, settled, panning]
}

/// Right after the history starts over, the clouds at a third and a quarter of the render size
/// are no grainier than while the view pans. A restart that marched every history texel with a
/// single jitter left its banding as a dither of light and dark cells, half as strong again as
/// while panning, wearing off over a second or so (and Auto's resolution changes restart it
/// often).
#[test]
fn low_cloud_presets_show_no_dither_after_a_restart() {
    let mut failures = Vec::new();
    for div in [3, 4] {
        let [restart, settled, panning] = run(div);
        eprintln!(
            "clouds={div}: (dither, on the grid) restart {restart:.3?}, settled {settled:.3?}, panning {panning:.3?}"
        );
        if restart.0 > 1.1 * panning.0 {
            failures.push(format!(
                "clouds={div}: {:.3} after a restart, {:.3} panning",
                restart.0, panning.0
            ));
        }
    }
    assert!(failures.is_empty(), "a dither in the clouds: {failures:?}");
}
