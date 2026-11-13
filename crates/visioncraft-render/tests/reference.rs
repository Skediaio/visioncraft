//! Headless reference-image tests on the real GPU. Each scene renders at
//! 480x270 and is compared with tests/reference/<name>.png: a pixel differs
//! when any channel is off by more than `TOLERANCE`, and at most
//! `MAX_DIFFERING` pixels may differ. Every run writes the actual image to
//! target/tmp/reference/<name>.png, plus <name>-diff.png on failure.
//!
//! Regenerate the references: `VISIONCRAFT_UPDATE_REFERENCES=1 cargo test -p visioncraft-render`.
//! No adapter means failure (the device `expect`s), never a silent skip.

use std::path::PathBuf;
use std::sync::OnceLock;

use glam::Vec3;
use visioncraft_render::demo::{self, Camera};
use visioncraft_render::{LineVertex, Quality, Renderer, Sun, headless_device};

const W: u32 = 480;
const H: u32 = 270;
const TOLERANCE: u8 = 10;
const MAX_DIFFERING: usize = (W * H / 400) as usize; // 0.25 %

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

fn setup(r: &mut Renderer, (view, proj, eye): Camera, sun: Sun, ev100: f32) {
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

fn read_png(path: &PathBuf) -> Option<(Vec<u8>, u32, u32)> {
    let decoder = png::Decoder::new(std::fs::File::open(path).ok()?);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    (info.color_type == png::ColorType::Rgba)
        .then(|| (buf[..info.buffer_size()].to_vec(), info.width, info.height))
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("reference")
}

fn check(name: &str, pixels: &[u8]) {
    let actual = out_dir().join(format!("{name}.png"));
    write_png(&actual, pixels, W, H);
    let reference = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/reference")
        .join(format!("{name}.png"));
    if std::env::var_os("VISIONCRAFT_UPDATE_REFERENCES").is_some() {
        write_png(&reference, pixels, W, H);
        eprintln!("updated {}", reference.display());
        return;
    }
    let Some((expected, w, h)) = read_png(&reference) else {
        panic!(
            "missing {}; run with VISIONCRAFT_UPDATE_REFERENCES=1",
            reference.display()
        );
    };
    assert_eq!((w, h), (W, H), "reference size");
    let mut diff = vec![0u8; pixels.len()];
    let mut differing = 0;
    for (i, (a, e)) in pixels.chunks(4).zip(expected.chunks(4)).enumerate() {
        let d = (0..3).map(|c| a[c].abs_diff(e[c])).max().unwrap_or(0);
        let bad = d > TOLERANCE;
        differing += bad as usize;
        diff[i * 4..i * 4 + 4].copy_from_slice(&if bad {
            [255, 0, 0, 255]
        } else {
            [d * 8, d * 8, d * 8, 255]
        });
    }
    if differing > MAX_DIFFERING {
        let path = out_dir().join(format!("{name}-diff.png"));
        write_png(&path, &diff, W, H);
        panic!(
            "{name}: {differing} pixels differ by more than {TOLERANCE} (allowed {MAX_DIFFERING}); see {} and {}",
            actual.display(),
            path.display()
        );
    }
}

/// Mean RGB of a pixel rectangle, for picture-level assertions.
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

#[test]
fn a_pbr_spheres_noon() {
    let mut r = renderer(W, H);
    demo::spheres(&mut r);
    setup(
        &mut r,
        demo::spheres_camera(W as f32 / H as f32),
        demo::noon_sun(),
        15.0,
    );
    let px = capture(&mut r, W, H);
    // The sky at the top is blue, brighter than black, not blown out.
    let sky = mean(&px, 0, 0, W, 10);
    assert!(
        sky[2] > sky[0] && sky[2] > 60.0 && sky[2] < 250.0,
        "sky {sky:?}"
    );
    check("a_pbr_spheres_noon", &px);
}

#[test]
fn b_house_evening() {
    let mut r = renderer(W, H);
    demo::house(&mut r);
    setup(
        &mut r,
        demo::house_camera(W as f32 / H as f32),
        demo::evening_sun(),
        14.0,
    );
    let px = capture(&mut r, W, H);
    check("b_house_evening", &px);
}

#[test]
fn c_overlay_lines() {
    let mut r = renderer(W, H);
    demo::house(&mut r);
    setup(
        &mut r,
        demo::house_camera(W as f32 / H as f32),
        demo::noon_sun(),
        15.0,
    );
    let mut tested = demo::grid_lines(10, 1.0, [40, 40, 48, 200], 1.0);
    // A thick red line running behind the house (partly hidden by it).
    tested.extend([
        LineVertex {
            position: [-7.0, 1.5, -5.0],
            color: [230, 40, 30, 255],
            width: 4.0,
        },
        LineVertex {
            position: [7.0, 1.5, -5.0],
            color: [230, 40, 30, 255],
            width: 4.0,
        },
    ]);
    let on_top = {
        let mut v = demo::box_lines(
            Vec3::new(-4.8, 0.0, -3.8),
            Vec3::new(4.8, 3.25, 3.8),
            [255, 160, 0, 255],
            2.0,
        );
        // Points (equal ends) on the table tops, one behind the house.
        for p in [
            [-2.4, 0.76, 5.6],
            [0.6, 0.76, 6.4],
            [3.0, 0.76, 5.2],
            [0.0, 1.0, -6.0],
        ] {
            let v0 = LineVertex {
                position: p,
                color: [30, 120, 255, 255],
                width: 8.0,
            };
            v.extend([v0, v0]);
        }
        v
    };
    r.set_overlay_lines(&tested);
    r.set_overlay_lines_on_top(&on_top);
    let px = capture(&mut r, W, H);
    check("c_overlay_lines", &px);
    // The projected label anchor for a table lands inside the image.
    let p = r
        .project(Vec3::new(-2.4, 0.76, 5.6))
        .expect("in front of the camera");
    assert!(
        p.x > 0.0 && p.x < W as f32 && p.y > 0.0 && p.y < H as f32,
        "{p}"
    );
    assert!(
        r.project(Vec3::new(30.0, 1.0, 40.0)).is_none(),
        "behind the camera"
    );
}

#[test]
fn d_face_highlight() {
    let mut r = renderer(W, H);
    let house = demo::house(&mut r);
    let aspect = W as f32 / H as f32;
    setup(
        &mut r,
        demo::camera(Vec3::new(2.5, 1.7, 8.5), Vec3::new(0.0, 1.3, 2.6), aspect),
        demo::noon_sun(),
        15.0,
    );
    let plain = capture(&mut r, W, H);
    r.set_highlight(Some((house.block, house.door_face)));
    let lit = capture(&mut r, W, H);
    check("d_face_highlight", &lit);
    // Only the door changes, and it turns bluer.
    let changed = plain
        .chunks(4)
        .zip(lit.chunks(4))
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    assert!(
        changed > 200 && changed < (W * H / 4) as usize,
        "{changed} pixels changed"
    );
    let blue = |px: &[u8]| {
        let c = mean(px, W / 2 - 10, H / 2 - 10, W / 2 + 10, H / 2 + 10);
        c[2] - c[0]
    };
    assert!(blue(&lit) > blue(&plain) + 5.0, "door not tinted");
}

/// Re-tessellation: replacing and removing meshes between frames must work.
#[test]
fn replace_and_remove_meshes() {
    let mut r = renderer(160, 90);
    let house = demo::house(&mut r);
    setup(
        &mut r,
        demo::house_camera(16.0 / 9.0),
        demo::noon_sun(),
        15.0,
    );
    capture_any(&mut r);
    let mut smaller = demo::MeshData::default();
    smaller.add_box(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 2.0, 2.0));
    r.replace_mesh(house.block, &smaller.mesh())
        .expect("replace");
    r.set_highlight(Some((house.block, 3)));
    capture_any(&mut r);
    r.remove_mesh(house.block);
    capture_any(&mut r);
    assert!(
        r.replace_mesh(house.block, &smaller.mesh()).is_err(),
        "removed meshes stay removed"
    );
    let bad = visioncraft_render::Mesh {
        indices: &[0, 1, 99],
        ..smaller.mesh()
    };
    assert!(r.add_mesh(&bad).is_err(), "out-of-range index accepted");
    for q in [Quality::LOW, Quality::MEDIUM, Quality::HIGH] {
        r.set_quality(q);
        capture_any(&mut r);
    }
}

fn capture_any(r: &mut Renderer) {
    let (device, queue) = gpu();
    let mut encoder = device.create_command_encoder(&Default::default());
    r.render(&mut encoder);
    queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
}

#[test]
fn e_brick_walls_triplanar() {
    let mut r = renderer(W, H);
    demo::brick_walls(&mut r);
    setup(
        &mut r,
        demo::brick_walls_camera(W as f32 / H as f32),
        demo::afternoon_sun(),
        15.0,
    );
    let px = capture(&mut r, W, H);
    // Bricks are red-brown, not grey: the albedo map is applied.
    let wall = mean(&px, 140, 100, 220, 160);
    assert!(wall[0] > wall[2] + 15.0, "wall {wall:?}");
    check("e_brick_walls_triplanar", &px);
}

/// The foliage tree under wind at two fixed times: the leaves (and their
/// shadow on the wall) move between the two.
#[test]
fn f_foliage_wind() {
    let mut r = renderer(W, H);
    demo::foliage_tree(&mut r);
    setup(
        &mut r,
        demo::foliage_camera(W as f32 / H as f32),
        demo::foliage_sun(),
        15.0,
    );
    r.set_time(0.0);
    let a = capture(&mut r, W, H);
    check("f_foliage_wind_t0", &a);
    r.set_time(1.7);
    let b = capture(&mut r, W, H);
    check("f_foliage_wind_t1", &b);
    let changed = a
        .chunks(4)
        .zip(b.chunks(4))
        .filter(|(x, y)| x.iter().zip(y.iter()).any(|(p, q)| p.abs_diff(*q) > 20))
        .count();
    assert!(changed > 500, "wind moved only {changed} pixels");
}

#[test]
fn g_lod_row() {
    let mut r = renderer(W, H);
    demo::lod_row(&mut r);
    setup(
        &mut r,
        demo::lod_camera(W as f32 / H as f32),
        demo::afternoon_sun(),
        15.0,
    );
    let px = capture(&mut r, W, H);
    check("g_lod_row", &px);
}

#[test]
fn h_glass_room() {
    let mut r = renderer(W, H);
    demo::glass_room(&mut r);
    setup(
        &mut r,
        demo::glass_camera(W as f32 / H as f32),
        demo::afternoon_sun(),
        15.0,
    );
    let px = capture(&mut r, W, H);
    check("h_glass_room", &px);
}

#[test]
fn i_pond() {
    let mut r = renderer(W, H);
    demo::pond(&mut r);
    setup(
        &mut r,
        demo::pond_camera(W as f32 / H as f32),
        demo::afternoon_sun(),
        15.0,
    );
    r.set_time(3.0);
    let px = capture(&mut r, W, H);
    check("i_pond", &px);
}

/// The house at night: three point lights and a spot, manual exposure.
#[test]
fn j_night_lights() {
    let mut r = renderer(W, H);
    demo::house(&mut r);
    assert_eq!(r.set_lights(&demo::house_lights()), 0, "no light dropped");
    setup(
        &mut r,
        demo::house_camera(W as f32 / H as f32),
        demo::night_sun(),
        2.0,
    );
    let px = capture(&mut r, W, H);
    // The lit wall is brighter than the unlit sky above the roof.
    let wall = mean(&px, 150, 120, 330, 150);
    let sky = mean(&px, 0, 0, W, 10);
    assert!(wall[0] > sky[0] + 20.0, "wall {wall:?} sky {sky:?}");
    check("j_night_lights", &px);
}

#[test]
fn k_clay() {
    let mut r = renderer(W, H);
    demo::garden(&mut r);
    r.set_clay(true);
    setup(
        &mut r,
        demo::garden_camera(W as f32 / H as f32),
        demo::afternoon_sun(),
        15.0,
    );
    let px = capture(&mut r, W, H);
    check("k_clay", &px);
}

/// Stills: arbitrary size at HIGH quality without disturbing the live view,
/// errors (not panics) for impossible sizes.
#[test]
fn stills_leave_the_live_view_alone() {
    let mut r = renderer(160, 90);
    r.set_quality(Quality::LOW);
    demo::house(&mut r);
    let cam = demo::house_camera(16.0 / 9.0);
    setup(&mut r, cam, demo::noon_sun(), 15.0);
    let before = capture(&mut r, 160, 90);
    let (view, proj, eye) = demo::spheres_camera(4.0 / 3.0);
    let still = r
        .render_still(400, 300, view, proj, eye)
        .expect("still")
        .wait()
        .expect("pixels");
    assert_eq!(still.len(), 400 * 300 * 4);
    assert!(
        still.chunks(4).any(|p| p[0] != still[0]),
        "not a blank image"
    );
    let after = capture(&mut r, 160, 90);
    assert_eq!(before, after, "the live frame changed");
    assert_eq!(r.quality(), Quality::LOW);
    assert!(r.render_still(5000, 100, view, proj, eye).is_err());
    assert!(r.render_still(0, 100, view, proj, eye).is_err());
}

/// 1920x1080 beauty shots of scene (b), high and medium quality:
/// `cargo test -p visioncraft-render -- --ignored beauty`.
#[test]
#[ignore]
fn beauty_house_evening() {
    let (w, h) = (1920, 1080);
    for (name, quality) in [("high", Quality::HIGH), ("medium", Quality::MEDIUM)] {
        let mut r = renderer(w, h);
        r.set_quality(quality);
        demo::house(&mut r);
        setup(
            &mut r,
            demo::house_camera(w as f32 / h as f32),
            demo::evening_sun(),
            14.0,
        );
        let px = capture(&mut r, w, h);
        let path = out_dir().join(format!("beauty_house_evening_{name}.png"));
        write_png(&path, &px, w, h);
        eprintln!("wrote {}", path.display());
    }
}
