//! A fresh renderer's pictures: the first is the second is the third, byte for byte, for
//! `render` and for `render_still`.
//!
//! The first picture of a renderer used to differ from every later one by one 8-bit step in a
//! few per cent of its bytes, on its surfaces only: with the aerial perspective on (the app and
//! GameCraft always have it on) the first frame went without it. The physical sky's two fixed
//! LUTs were drawn after the frame's globals were written, and the globals turn the perspective
//! on only once they exist (`PhysicalSky::ready`); the air's pass itself ran. `render_part` now
//! draws the LUTs first. Reproduced on llvmpipe; the simple scenes of the reference tests never
//! showed it because they leave the perspective off.
//!
use std::path::PathBuf;
use std::sync::OnceLock;

use glam::{Mat4, Vec3};
use visioncraft_render::demo::{self, Camera, MeshData};
use visioncraft_render::{ColorSpace, Material, Quality, Renderer, Sky, Sun, headless_device};

fn gpu() -> &'static (wgpu::Device, wgpu::Queue) {
    static GPU: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();
    GPU.get_or_init(|| {
        pollster::block_on(headless_device()).expect("headless wgpu device (no adapter?)")
    })
}

fn renderer(w: u32, h: u32, quality: Quality) -> Renderer {
    let (device, queue) = gpu();
    let mut r = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    r.resize(w, h);
    r.set_quality(quality);
    r
}

/// Renders one frame and reads the output back as tightly packed RGBA8.
fn frame(r: &mut Renderer, w: u32, h: u32) -> Vec<u8> {
    let (device, queue) = gpu();
    let mut encoder = device.create_command_encoder(&Default::default());
    r.render(&mut encoder);
    let stride = (w * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (stride * h) as u64,
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
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
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
    buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped range")
        .chunks(stride as usize)
        .flat_map(|row| &row[..(w * 4) as usize])
        .copied()
        .collect()
}

/// One offscreen still: what a tool or an agent gets.
fn still(r: &mut Renderer, (view, proj, eye): Camera, w: u32, h: u32) -> Vec<u8> {
    r.render_still(w, h, view, proj, eye)
        .expect("still")
        .wait()
        .expect("pixels")
}

/// The colour bytes of `a` and `b` that differ, and by how much at most.
fn delta(a: &[u8], b: &[u8]) -> (usize, u8) {
    let mut bytes = 0;
    let mut worst = 0;
    for (x, y) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
        for c in 0..3 {
            let d = x[c].abs_diff(y[c]);
            bytes += usize::from(d != 0);
            worst = worst.max(d);
        }
    }
    (bytes, worst)
}

fn write_png(path: &PathBuf, pixels: &[u8], w: u32, h: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    let mut enc = png::Encoder::new(std::fs::File::create(path).expect("png file"), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(pixels)
        .expect("png data");
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("first_frame")
}

/// Renders `shots` pictures of one fresh renderer; prints, between 1 and 2 and between 2 and
/// the last, how many colour bytes differ and by how much at most; keeps the pictures (and the
/// first difference, x 16, over the second picture) in `CARGO_TARGET_TMPDIR/first_frame`.
fn report(name: &str, w: u32, h: u32, shots: &[Vec<u8>]) {
    let (a, worst_a) = delta(&shots[0], &shots[1]);
    let (b, worst_b) = delta(&shots[1], &shots[shots.len() - 1]);
    eprintln!(
        "{name}: 1-2 {a} of {} colour bytes differ ({:.2} %), largest delta {worst_a}; 2-{} {b} bytes, largest delta {worst_b}",
        shots[0].len() / 4 * 3,
        100.0 * a as f32 / (shots[0].len() / 4 * 3) as f32,
        shots.len(),
    );
    let dir = out_dir();
    write_png(&dir.join(format!("{name}-1.png")), &shots[0], w, h);
    write_png(&dir.join(format!("{name}-2.png")), &shots[1], w, h);
    let mut diff = shots[1].clone();
    let (changed, _) = diff.as_chunks_mut::<4>();
    let (first, _) = shots[0].as_chunks::<4>();
    for (d, f) in changed.iter_mut().zip(first) {
        let m = (0..3).map(|c| d[c].abs_diff(f[c])).max().unwrap_or(0);
        let v = m.saturating_mul(16);
        d[0] = (d[0] / 2).saturating_add(v);
        d[1] /= 2;
        d[2] = (d[2] / 2).saturating_add(v);
    }
    write_png(&dir.join(format!("{name}-diff.png")), &diff, w, h);
}

/// A textured crate: a box with planks.
fn crate_textured(r: &mut Renderer) {
    let (w, h) = (256u32, 32u32);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let plank = x / 32;
            let seam = x % 32 == 0 || y == 0;
            let grain = ((x * 7 + y * 13 + plank * 13_37) % 23) as f32 / 23.0;
            let c = if seam {
                [0.25, 0.15, 0.07, 1.0]
            } else {
                [
                    0.72 - 0.1 * grain,
                    0.5 - 0.08 * grain,
                    0.28 - 0.05 * grain,
                    1.0,
                ]
            };
            rgba.extend(c.map(|v| (v * 255.0 + 0.5) as u8));
        }
    }
    let image = demo::Image {
        width: w,
        height: h,
        rgba,
    };
    let albedo = image.upload(r, ColorSpace::Srgb, true);
    let planks = r.add_material(&Material {
        base_color_map: Some(albedo),
        roughness: 0.8,
        ..Default::default()
    });
    let mut box_ = MeshData::default();
    box_.add_box(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
    let mesh = r.add_mesh(&box_.mesh()).expect("box");
    r.add_item(
        mesh,
        planks,
        &[Mat4::from_translation(Vec3::new(0.0, 0.0, 0.0))],
    );
}

fn meadow(r: &mut Renderer) {
    let mut ground = MeshData::default();
    ground.add_box(Vec3::new(-40.0, -0.1, -40.0), Vec3::new(40.0, 0.0, 40.0));
    let ground = r.add_mesh(&ground.mesh()).expect("ground");
    let grass = r.add_material(&Material {
        base_color: [0.14, 0.3, 0.06, 1.0],
        roughness: 0.95,
        ..Default::default()
    });
    r.add_item(ground, grass, &[Mat4::IDENTITY]);
}

/// A meadow and a textured crate under the physical sky, the sun high in the south: the yard
/// GameCraft draws. Returns the camera.
fn yard(r: &mut Renderer) -> Camera {
    r.set_sky(&Sky::Physical).expect("sky");
    meadow(r);
    crate_textured(r);
    r.set_sun(&Sun::from_angles(180.0, 40.0));
    r.set_sky_scale(1.0);
    r.set_exposure(14.0);
    demo::camera(
        Vec3::new(0.0, 2.2, 6.0),
        Vec3::new(0.0, 1.2, 0.0),
        4.0 / 3.0,
    )
}

/// `n` frames of the same view through `Renderer::render`, the clock at 0 each time as a
/// picture or a paused view has it.
fn frames(r: &mut Renderer, cam: Camera, (w, h): (u32, u32), n: usize) -> Vec<Vec<u8>> {
    r.set_camera(cam.0, cam.1, cam.2);
    (0..n)
        .map(|_| {
            r.set_time(0.0);
            frame(r, w, h)
        })
        .collect()
}

/// `n` stills of the same view through `Renderer::render_still`.
fn stills(r: &mut Renderer, cam: Camera, size: (u32, u32), n: usize) -> Vec<Vec<u8>> {
    (0..n)
        .map(|_| {
            r.set_time(0.0);
            still(r, cam, size.0, size.1)
        })
        .collect()
}

/// Between the first and the second picture, and between the second and the third, no colour
/// byte differs. Prints the counts; on a difference keeps the pictures and the difference in
/// `CARGO_TARGET_TMPDIR/first_frame`.
fn assert_steady(name: &str, shots: &[Vec<u8>], (w, h): (u32, u32)) {
    assert_eq!(shots.len(), 3, "three pictures");
    let first = delta(&shots[0], &shots[1]);
    let later = delta(&shots[1], &shots[2]);
    eprintln!(
        "{name}: 1-2 {} bytes, largest delta {}; 2-3 {} bytes, largest delta {}",
        first.0, first.1, later.0, later.1
    );
    if first != (0, 0) || later != (0, 0) {
        report(name, w, h, shots);
    }
    assert_eq!(
        later,
        (0, 0),
        "{name}: the second and the third picture differ"
    );
    assert_eq!(
        first,
        (0, 0),
        "{name}: the first picture of a fresh renderer differs from the second"
    );
}

/// A fresh renderer at the default quality, three frames of the same view: the first is the
/// second is the third. The aerial perspective is on, as the app and GameCraft always have it.
#[test]
fn the_first_frame_is_every_later_frame() {
    let size = (320, 240);
    let mut r = renderer(size.0, size.1, Quality::default());
    let cam = yard(&mut r);
    r.set_aerial_perspective(true);
    let shots = frames(&mut r, cam, size, 3);
    assert_steady("render", &shots, size);
}

/// The same through stills, as GameCraft takes its pictures: live at LOW, each still at HIGH.
#[test]
fn the_first_still_is_every_later_still() {
    let size = (320, 240);
    let mut r = renderer(size.0, size.1, Quality::LOW);
    let cam = yard(&mut r);
    r.set_aerial_perspective(true);
    r.finish_environment();
    let shots = stills(&mut r, cam, size, 3);
    assert_steady("still", &shots, size);
}
