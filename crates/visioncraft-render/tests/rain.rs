//! Wet surfaces and rain (`Renderer::set_wet`) on the real GPU, by pixel statistics, no reference
//! image: porous paving darkens when wet and metal barely does, rain draws streaks over the
//! scene, and wetness 0 with rain 0 is the picture as it was before they existed (byte for byte).

use std::path::PathBuf;
use std::sync::OnceLock;

use glam::{Mat4, Vec3};
use visioncraft_render::demo::{self, MeshData};
use visioncraft_render::{Material, Projection, Quality, Renderer, Sun, headless_device};

const W: u32 = 480;
const H: u32 = 270;

fn gpu() -> &'static (wgpu::Device, wgpu::Queue) {
    static GPU: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();
    GPU.get_or_init(|| {
        pollster::block_on(headless_device()).expect("headless wgpu device (no adapter?)")
    })
}

fn renderer(w: u32, h: u32) -> Renderer {
    let (device, queue) = gpu();
    let mut r = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    r.resize(w, h);
    r.set_quality(Quality::HIGH);
    r
}

fn setup(r: &mut Renderer, (view, proj, eye): demo::Camera, sun: Sun, ev100: f32) {
    r.set_camera(view, proj, eye);
    r.set_sun(&sun);
    r.set_exposure(ev100);
}

/// Renders one frame and reads the output back as tightly packed RGBA8.
fn capture(r: &mut Renderer, w: u32, h: u32) -> Vec<u8> {
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
    let padded = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped range")
        .to_vec();
    padded
        .chunks(stride as usize)
        .flat_map(|row| &row[..(w * 4) as usize])
        .copied()
        .collect()
}

fn write_png(path: &PathBuf, pixels: &[u8], w: u32, h: u32) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let mut enc = png::Encoder::new(std::fs::File::create(path).expect("png file"), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(pixels)
        .expect("png data");
}

/// Writes the picture beside the other test output, to look at.
fn dump(name: &str, px: &[u8]) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("rain");
    write_png(&dir.join(format!("{name}.png")), px, W, H);
}

fn mean(pixels: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> [f32; 3] {
    let mut sum = [0.0; 3];
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * W + x) * 4) as usize;
            for c in 0..3 {
                sum[c] += pixels[i + c] as f32;
            }
        }
    }
    sum.map(|s| s / ((x1 - x0) * (y1 - y0)) as f32)
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// How many pixels of a block are thin bright lines: brighter than both pixels three to either
/// side by a margin (a streak seen across; paving texture, edges and puddle rims are not).
fn thin_lines(pixels: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
    let g = |x: u32, y: u32| pixels[((y * W + x) * 4 + 1) as usize] as i32;
    let mut count = 0;
    for y in y0..y1 {
        for x in x0 + 3..x1 - 3 {
            if g(x, y) - g(x - 3, y).max(g(x + 3, y)) > 12 {
                count += 1;
            }
        }
    }
    count
}

const PAVING: [f32; 4] = [0.35, 0.34, 0.33, 1.0];
const STEEL: [f32; 4] = [0.55, 0.56, 0.58, 1.0];

/// Two slabs side by side seen from above at a slant: paving (left) and polished steel (right).
fn slabs(wet: Option<(f32, f32)>) -> Vec<u8> {
    let mut r = renderer(W, H);
    for (x0, color, metallic, roughness) in [(-8.0, PAVING, 0.0, 0.85), (0.0, STEEL, 1.0, 0.35)] {
        let mut slab = MeshData::default();
        slab.add_box(Vec3::new(x0, -0.1, -8.0), Vec3::new(x0 + 8.0, 0.0, 8.0));
        let slab = r.add_mesh(&slab.mesh()).expect("slab");
        let m = r.add_material(&Material {
            base_color: color,
            metallic,
            roughness,
            projection: Projection::Triplanar { scale_m: 1.0 },
            ..Default::default()
        });
        r.add_item(slab, m, &[Mat4::IDENTITY]);
    }
    let cam = demo::camera(Vec3::new(0.0, 6.0, 6.0), Vec3::ZERO, W as f32 / H as f32);
    setup(&mut r, cam, demo::afternoon_sun(), 14.5);
    if let Some((wetness, rain)) = wet {
        r.set_wet(wetness, rain);
    }
    capture(&mut r, W, H)
}

fn left(px: &[u8]) -> [f32; 3] {
    mean(px, W / 2 - 160, H / 2 - 30, W / 2 - 20, H / 2 + 70)
}

fn right(px: &[u8]) -> [f32; 3] {
    mean(px, W / 2 + 20, H / 2 - 30, W / 2 + 160, H / 2 + 70)
}

#[test]
fn wet_paving_is_darker_and_steel_barely_changes() {
    let dry = slabs(None);
    let damp = slabs(Some((0.45, 0.0)));
    dump("slabs-dry", &dry);
    dump("slabs-damp", &damp);
    let (d, w) = (luma(left(&dry)), luma(left(&damp)));
    assert!(w < d * 0.94, "dry paving {d:.1}, damp {w:.1}");
    let (ds, ws) = (luma(right(&dry)), luma(right(&damp)));
    assert!(
        (ws - ds).abs() < ds * 0.12,
        "dry steel {ds:.1}, damp steel {ws:.1}"
    );
}

#[test]
fn puddles_form_on_flat_paving_from_about_half_wet() {
    let damp = slabs(Some((0.45, 0.0)));
    let soaked = slabs(Some((1.0, 0.0)));
    dump("slabs-soaked", &soaked);
    // Below the threshold there are no puddles: the picture only darkens. Above it the flat
    // paving breaks into smooth, dark mirrors among the rest: its brightness varies a good deal more.
    let spread = |px: &[u8]| {
        let (x0, y0) = (W / 2 - 160, H / 2 - 30);
        let mut v = Vec::new();
        for y in (y0..y0 + 100).step_by(4) {
            for x in (x0..x0 + 140).step_by(4) {
                v.push(px[((y * W + x) * 4 + 1) as usize] as f32);
            }
        }
        let m = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
    };
    let (a, b) = (spread(&damp), spread(&soaked));
    assert!(b > a * 1.3, "damp spread {a:.2}, soaked {b:.2}");
}

#[test]
fn rain_draws_streaks_and_closes_the_sky() {
    let wet = slabs(Some((0.8, 0.0)));
    let rain = slabs(Some((0.0, 0.8)));
    dump("slabs-rain", &rain);
    // The sky closes to a grey overcast layer: less blue over red than the clear one.
    let sky = |px: &[u8]| {
        let c = mean(px, 4, 2, 60, 12);
        c[2] - c[0]
    };
    assert!(
        sky(&rain) < sky(&wet) - 6.0,
        "wet sky {:?}, rain sky {:?}",
        sky(&wet),
        sky(&rain)
    );
    // Streaks add fine detail over the whole picture, the sky aside.
    let (a, b) = (
        thin_lines(&wet, 8, H / 2, W - 8, H - 8),
        thin_lines(&rain, 8, H / 2, W - 8, H - 8),
    );
    assert!(b > a + 40, "thin lines without rain {a}, with {b}");
}

#[test]
fn without_wetness_and_rain_the_picture_is_byte_for_byte_the_same() {
    let never = slabs(None);
    let zeros = slabs(Some((0.0, 0.0)));
    assert!(never == zeros, "wetness 0 and rain 0 changed the picture");
    // And a wet one is not the dry one.
    assert!(never != slabs(Some((0.5, 0.0))));
}
