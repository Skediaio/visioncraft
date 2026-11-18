//! Weathering (`Material::aging`) on the real GPU, by pixel statistics, no reference image: a
//! weathered wall is darker low down than high up, a weathered up-facing paving face shifts
//! towards green where moss is high, and a material without it is drawn byte for byte as one
//! with all zeros (the reference images stay as they are).

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
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("weathering");
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

/// A plain pale material, standard triplanar, with the given ageing.
fn plaster(aging: [f32; 4]) -> Material {
    Material {
        base_color: [0.6, 0.58, 0.54, 1.0],
        roughness: 0.85,
        projection: Projection::Triplanar { scale_m: 2.0 },
        aging,
        ..Default::default()
    }
}

/// A 10 m wide, 5 m high wall facing +Z (south, towards the camera) on flat ground.
fn wall_scene(aging: [f32; 4]) -> (Renderer, demo::Camera) {
    let mut r = renderer(W, H);
    let mut wall = MeshData::default();
    wall.add_box(Vec3::new(-5.0, 0.0, -0.4), Vec3::new(5.0, 5.0, 0.0));
    let wall = r.add_mesh(&wall.mesh()).expect("wall");
    let m = r.add_material(&plaster(aging));
    r.add_item(wall, m, &[Mat4::IDENTITY]);
    // The ground, so the wall has a foot.
    let mut ground = MeshData::default();
    ground.add_box(Vec3::new(-40.0, -0.1, -40.0), Vec3::new(40.0, 0.0, 40.0));
    let ground = r.add_mesh(&ground.mesh()).expect("ground");
    let gm = r.add_material(&Material {
        base_color: [0.1, 0.1, 0.1, 1.0],
        roughness: 0.95,
        ..Default::default()
    });
    r.add_item(ground, gm, &[Mat4::IDENTITY]);
    // Eye height, straight on.
    let cam = demo::camera(
        Vec3::new(0.0, 2.5, 11.0),
        Vec3::new(0.0, 2.5, 0.0),
        W as f32 / H as f32,
    );
    (r, cam)
}

/// The rows of the wall's foot (up to about 0.8 m) against those of its upper part (3 to 4.5 m),
/// over the middle of the wall: the footing collects the dirt.
fn bands(px: &[u8], r: &Renderer) -> ([f32; 3], [f32; 3]) {
    let row = |y: f32| r.project(Vec3::new(0.0, y, 0.0)).expect("in view").y as u32;
    let (x0, x1) = (W / 2 - 80, W / 2 + 80);
    let low = mean(px, x0, row(0.7), x1, row(0.1));
    let high = mean(px, x0, row(4.4), x1, row(3.2));
    (low, high)
}

#[test]
fn a_weathered_wall_is_darker_at_its_foot_than_higher_up() {
    let aging = [0.7, 0.4, 0.3, 0.5];
    let (mut r, cam) = wall_scene(aging);
    setup(&mut r, cam, demo::afternoon_sun(), 15.0);
    let px = capture(&mut r, W, H);
    let (low, high) = bands(&px, &r);
    dump("wall-weathered", &px);

    let (mut r0, cam0) = wall_scene([0.0; 4]);
    setup(&mut r0, cam0, demo::afternoon_sun(), 15.0);
    let plain = capture(&mut r0, W, H);
    let (plain_low, plain_high) = bands(&plain, &r0);
    dump("wall-plain", &plain);

    // Unweathered, the foot is no darker than the top by more than the light's own difference;
    // weathered, it is darker by a good margin of that.
    let plain_gap = luma(plain_high) - luma(plain_low);
    let gap = luma(high) - luma(low);
    assert!(
        gap > plain_gap + 12.0,
        "weathered gap {gap:.1} ({low:?} against {high:?}), plain gap {plain_gap:.1} ({plain_low:?} against {plain_high:?})"
    );
    // And the whole wall is not simply darker: the top is close to what it was.
    assert!(
        luma(high) > luma(plain_high) - 25.0,
        "top {high:?}, plain top {plain_high:?}"
    );
}

/// How green a pixel block is: green over the mean of red and blue.
fn greenness(c: [f32; 3]) -> f32 {
    c[1] - 0.5 * (c[0] + c[2])
}

#[test]
fn weathered_paving_turns_green_where_moss_is_high() {
    let aspect = W as f32 / H as f32;
    let mut shots = Vec::new();
    for aging in [[0.0; 4], [0.0, 1.0, 0.0, 0.0]] {
        let mut r = renderer(W, H);
        // Up-facing paving, seen from above at a slant, in the open.
        let mut slab = MeshData::default();
        slab.add_box(Vec3::new(-8.0, -0.1, -8.0), Vec3::new(8.0, 0.0, 8.0));
        let slab = r.add_mesh(&slab.mesh()).expect("slab");
        let m = r.add_material(&Material {
            base_color: [0.32, 0.31, 0.3, 1.0],
            roughness: 0.9,
            projection: Projection::Triplanar { scale_m: 1.0 },
            aging,
            ..Default::default()
        });
        r.add_item(slab, m, &[Mat4::IDENTITY]);
        let cam = demo::camera(Vec3::new(0.0, 6.0, 6.0), Vec3::new(0.0, 0.0, 0.0), aspect);
        setup(&mut r, cam, demo::afternoon_sun(), 14.5);
        let px = capture(&mut r, W, H);
        shots.push(mean(&px, W / 2 - 100, H / 2 - 40, W / 2 + 100, H / 2 + 60));
    }
    let (plain, mossy) = (shots[0], shots[1]);
    assert!(
        greenness(mossy) > greenness(plain) + 4.0,
        "plain {plain:?}, mossy {mossy:?}"
    );
}

#[test]
fn without_weathering_the_picture_is_byte_for_byte_the_same() {
    let default = {
        let (mut r, cam) = {
            let mut r = renderer(W, H);
            let mut wall = MeshData::default();
            wall.add_box(Vec3::new(-5.0, 0.0, -0.4), Vec3::new(5.0, 5.0, 0.0));
            let wall = r.add_mesh(&wall.mesh()).expect("wall");
            // The material as every caller made it before ageing existed.
            let m = r.add_material(&Material {
                base_color: [0.6, 0.58, 0.54, 1.0],
                roughness: 0.85,
                projection: Projection::Triplanar { scale_m: 2.0 },
                ..Default::default()
            });
            r.add_item(wall, m, &[Mat4::IDENTITY]);
            let cam = demo::camera(
                Vec3::new(0.0, 2.5, 11.0),
                Vec3::new(0.0, 2.5, 0.0),
                W as f32 / H as f32,
            );
            (r, cam)
        };
        setup(&mut r, cam, demo::afternoon_sun(), 15.0);
        capture(&mut r, W, H)
    };
    let (mut r, cam) = {
        let mut r = renderer(W, H);
        let mut wall = MeshData::default();
        wall.add_box(Vec3::new(-5.0, 0.0, -0.4), Vec3::new(5.0, 5.0, 0.0));
        let wall = r.add_mesh(&wall.mesh()).expect("wall");
        let m = r.add_material(&plaster([0.0; 4]));
        r.add_item(wall, m, &[Mat4::IDENTITY]);
        let cam = demo::camera(
            Vec3::new(0.0, 2.5, 11.0),
            Vec3::new(0.0, 2.5, 0.0),
            W as f32 / H as f32,
        );
        (r, cam)
    };
    setup(&mut r, cam, demo::afternoon_sun(), 15.0);
    let zeros = capture(&mut r, W, H);
    assert!(default == zeros, "an all-zero ageing changed the picture");
}
