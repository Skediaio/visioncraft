//! Photo mode on the real GPU: averaged samples smooth a hard edge, a thin lens blurs what
//! lies off its focal plane while what is on it stays sharp, and a lamp's glowing head leaves
//! no white speckles on the wall behind it. Run on a machine with a GPU.

use std::sync::OnceLock;

use glam::{Mat4, Vec3};
use visioncraft_render::demo::MeshData;
use visioncraft_render::{
    Focus, Lens, Light, LightKind, Material, Quality, Renderer, Sun, headless_device,
};

const W: u32 = 240;
const H: u32 = 180;

fn gpu() -> &'static (wgpu::Device, wgpu::Queue) {
    static GPU: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();
    GPU.get_or_init(|| {
        pollster::block_on(headless_device()).expect("headless wgpu device (no adapter?)")
    })
}

/// No multisampling, FXAA or bloom: what smooths an edge here is the photo's averaging.
const HARD: Quality = Quality {
    msaa: false,
    fxaa: false,
    bloom: false,
    render_scale: 1.0,
    ..Quality::HIGH
};

/// The camera looks up into an open sky; `boxes` are dark boxes given in its own space (x
/// right, y up, looking down -z), so their edges stand against the sky.
fn scene(boxes: &[(Vec3, Vec3, f32)]) -> (Renderer, (Mat4, Mat4, Vec3)) {
    let (device, queue) = gpu();
    let mut r = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    r.resize(W, H);
    r.set_quality(HARD);
    r.set_ground(None);
    let eye = Vec3::new(0.0, 1.5, 0.0);
    let view = Mat4::look_at_rh(eye, eye + Vec3::new(0.0, 0.35, -1.0), Vec3::Y);
    let proj = Mat4::perspective_rh(50f32.to_radians(), W as f32 / H as f32, 0.1, 1000.0);
    let to_world = view.inverse();
    let dark = r.add_material(&Material {
        base_color: [0.02, 0.02, 0.02, 1.0],
        roughness: 0.9,
        ..Default::default()
    });
    for (lo, hi, turn) in boxes {
        let mut m = MeshData::default();
        m.add_box(*lo, *hi);
        let mesh = r.add_mesh(&m.mesh()).expect("box");
        r.add_item(mesh, dark, &[to_world * Mat4::from_rotation_z(*turn)]);
    }
    r.set_camera(view, proj, eye);
    r.set_sun(&Sun::from_angles(180.0, 40.0));
    r.set_exposure(14.0);
    (r, (view, proj, eye))
}

/// The photo's pixels after `samples` samples through `lens`, and where it focused.
fn photo(
    r: &mut Renderer,
    camera: (Mat4, Mat4, Vec3),
    samples: u32,
    lens: Lens,
) -> (Vec<u8>, Option<f32>) {
    let mut p = r.photo_begin(W, H, HARD, camera, lens).expect("photo");
    while p.done < samples {
        if !r.photo_sample(&mut p).expect("sample") {
            r.wait_gpu().expect("poll");
        }
    }
    let focus = p.focus_m();
    (
        r.photo_finish(p).expect("finish").wait().expect("pixels"),
        focus,
    )
}

fn luma(px: &[u8], x: u32, y: u32) -> f32 {
    let i = ((y * W + x) * 4) as usize;
    0.25 * f32::from(px[i]) + 0.6 * f32::from(px[i + 1]) + 0.15 * f32::from(px[i + 2])
}

/// The mean number of pixels per row between 10 % and 90 % of the step from box to sky, over
/// rows `rows`, between columns `inside` (in the box) and `sky`.
fn edge_width(px: &[u8], rows: std::ops::Range<u32>, inside: u32, sky: u32) -> f32 {
    let mut total = 0.0;
    for y in rows.clone() {
        let (lo, hi) = (luma(px, inside, y), luma(px, sky, y));
        assert!(hi - lo > 25.0, "row {y}: no edge ({lo} to {hi})");
        let between = (inside.min(sky)..=inside.max(sky))
            .filter(|&x| {
                let t = (luma(px, x, y) - lo) / (hi - lo);
                (0.1..0.9).contains(&t)
            })
            .count();
        total += between as f32;
    }
    total / rows.len() as f32
}

/// The live frame, read back as RGBA8.
fn capture(r: &mut Renderer) -> Vec<u8> {
    let (device, queue) = gpu();
    let mut encoder = device.create_command_encoder(&Default::default());
    r.render(&mut encoder);
    let stride = (W * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(stride * H),
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
        .expect("mapped")
        .to_vec();
    padded
        .chunks(stride as usize)
        .flat_map(|row| &row[..(W * 4) as usize])
        .copied()
        .collect()
}

fn save(name: &str, px: &[u8]) {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("photo");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut enc = png::Encoder::new(
        std::fs::File::create(dir.join(format!("{name}.png"))).expect("png"),
        W,
        H,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("header")
        .write_image_data(px)
        .expect("data");
}

#[test]
fn averaged_samples_smooth_a_hard_edge() {
    // A dark box filling the left of the picture, its right edge turned 6 degrees off the
    // vertical: without antialiasing it steps from row to row.
    let (mut r, camera) = scene(&[(
        Vec3::new(-20.0, -20.0, -6.0),
        Vec3::new(0.0, 20.0, -5.0),
        6f32.to_radians(),
    )]);
    let (one, _) = photo(&mut r, camera, 1, Lens::SHARP);
    let (sixteen, _) = photo(&mut r, camera, 16, Lens::SHARP);
    save("edge-1", &one);
    save("edge-16", &sixteen);
    let (a, b) = (
        edge_width(&one, 30..150, 70, 170),
        edge_width(&sixteen, 30..150, 70, 170),
    );
    eprintln!("edge: {a:.2} px between at 1 sample, {b:.2} at 16");
    // One sample: a hard step, rarely a pixel between. Sixteen: about one pixel of coverage
    // ramp in nearly every row.
    assert!(a < 0.3, "{a}");
    assert!(b > 0.7 && b > 3.0 * a, "{a} -> {b}");
    // A photo of one sample is the plain frame at the same quality.
    let frame = capture(&mut r);
    let differing = one
        .chunks(4)
        .zip(frame.chunks(4))
        .filter(|(p, q)| p.iter().zip(*q).any(|(a, b)| a.abs_diff(*b) > 2))
        .count();
    assert!(
        differing < (W * H / 400) as usize,
        "{differing} pixels differ"
    );
}

#[test]
fn a_thin_lens_blurs_far_and_keeps_the_focal_plane_sharp() {
    // A near box at 4 m over the top half, its right edge left of centre; a far box at 30 m
    // under the bottom half, its left edge right of centre.
    let boxes = [
        (
            Vec3::new(-20.0, 0.1, -4.5),
            Vec3::new(-0.6, 20.0, -4.0),
            0.0,
        ),
        (
            Vec3::new(3.0, -40.0, -31.0),
            Vec3::new(60.0, -0.8, -30.0),
            0.0,
        ),
    ];
    let (mut r, camera) = scene(&boxes);
    let (sharp, _) = photo(&mut r, camera, 32, Lens::SHARP);
    let lens = Lens {
        radius_m: 0.15,
        focus: Focus::Auto([0.2, 0.25]),
        shutter_s: 0.0,
    };
    let (blurred, focus) = photo(&mut r, camera, 32, lens);
    save("dof-sharp", &sharp);
    save("dof-lens", &blurred);
    // The autofocus found the near box's face.
    let focus = focus.expect("focused");
    assert!((focus - 4.0).abs() < 0.1, "focused at {focus} m");
    // Near edge (box on the left): rows of the top half; far edge (box on the right): bottom.
    let near = |px: &[u8]| edge_width(px, 20..70, 40, 115);
    let far = |px: &[u8]| edge_width(px, 115..165, 200, 90);
    let (near_sharp, near_lens) = (near(&sharp), near(&blurred));
    let (far_sharp, far_lens) = (far(&sharp), far(&blurred));
    eprintln!(
        "near edge {near_sharp:.2} -> {near_lens:.2} px, far edge {far_sharp:.2} -> {far_lens:.2} px"
    );
    // On the focal plane the lens changes nothing; 26 m behind it the edge spreads over the
    // circle of confusion: 2 R P00 (1/4 - 1/30) W / 2 = about 12 pixels, 80 % of it between
    // 10 % and 90 %.
    assert!(near_lens < 1.6, "{near_lens}");
    assert!(
        (near_lens - near_sharp).abs() < 0.8,
        "{near_sharp} -> {near_lens}"
    );
    assert!(far_sharp < 1.6, "{far_sharp}");
    assert!(far_lens > 6.0, "{far_lens}");
}

/// Pixels at least 40 (of 255, luma) brighter than all four neighbours, outside `skip` (x0, y0,
/// x1, y1): the single white pixels a firefly leaves.
fn isolated_bright(px: &[u8], skip: (u32, u32, u32, u32)) -> usize {
    let mut n = 0;
    for y in 1..H - 1 {
        for x in 1..W - 1 {
            if (skip.0..skip.2).contains(&x) && (skip.1..skip.3).contains(&y) {
                continue;
            }
            let around = [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
                .map(|(i, j)| luma(px, i, j))
                .into_iter()
                .fold(0.0, f32::max);
            if luma(px, x, y) - around > 40.0 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn a_lamp_head_leaves_no_speckles_on_the_wall_behind_it() {
    // Dusk: a pale wall, a garden bollard half a metre in front of it, its head glowing (about
    // 2000 cd/m2: 350 lm out of a band of 0.05 m2) round a 350 lm point light. A photo's bounce
    // rays (scene.wgsl `bounce_ray`) used to take the head's colour where they met it on
    // screen: one ray in thousands, each bringing thousands of times the wall's light, so the
    // wall above the lamp was strewn with single white pixels.
    let quality = Quality {
        bloom: false,
        ..Quality::ULTRA
    };
    let (device, queue) = gpu();
    let mut r = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    r.resize(W, H);
    r.set_quality(quality);
    r.set_ground(None);
    let mut add = |lo: Vec3, hi: Vec3, m: &Material| {
        let mut mesh = MeshData::default();
        mesh.add_box(lo, hi);
        let mesh = r.add_mesh(&mesh.mesh()).expect("box");
        let m = r.add_material(m);
        r.add_item(mesh, m, &[Mat4::IDENTITY]);
    };
    let matt = |rgb: f32| Material {
        base_color: [rgb, rgb, rgb, 1.0],
        roughness: 0.9,
        ..Default::default()
    };
    // Ground, wall, the bollard's post and its glowing head.
    add(
        Vec3::new(-10.0, -0.1, -12.0),
        Vec3::new(10.0, 0.0, 2.0),
        &matt(0.1),
    );
    add(
        Vec3::new(-4.0, 0.0, -6.2),
        Vec3::new(4.0, 4.0, -6.0),
        &matt(0.6),
    );
    let lamp = Vec3::new(0.6, 0.51, -5.5);
    add(
        lamp + Vec3::new(-0.08, -0.51, -0.08),
        lamp + Vec3::new(0.08, 0.09, 0.08),
        &Material {
            base_color: [0.06, 0.06, 0.065, 1.0],
            metallic: 0.6,
            roughness: 0.45,
            ..Default::default()
        },
    );
    add(
        lamp + Vec3::new(-0.085, -0.06, -0.085),
        lamp + Vec3::new(0.085, 0.04, 0.085),
        &Material {
            base_color: [1.0, 0.9, 0.75, 1.0],
            roughness: 0.3,
            emissive: [1.0, 0.72, 0.42],
            emissive_strength: 2000.0,
            ..Default::default()
        },
    );
    let lights = [Light {
        position: lamp,
        kind: LightKind::Point,
        color: Vec3::new(1.0, 0.72, 0.42),
        intensity: 350.0,
        range_m: 6.0,
    }];
    assert_eq!(r.set_lights(&lights), 0, "no light dropped");
    let eye = Vec3::new(0.0, 1.2, -1.8);
    let view = Mat4::look_at_rh(eye, Vec3::new(0.3, 1.0, -6.0), Vec3::Y);
    let proj = Mat4::perspective_rh(50f32.to_radians(), W as f32 / H as f32, 0.1, 1000.0);
    r.set_camera(view, proj, eye);
    r.set_sun(&Sun::from_angles(300.0, -5.0));
    r.set_exposure(3.0);
    let mut p = r
        .photo_begin(W, H, quality, (view, proj, eye), Lens::SHARP)
        .expect("photo");
    while p.done < 32 {
        if !r.photo_sample(&mut p).expect("sample") {
            r.wait_gpu().expect("poll");
        }
    }
    let px = r.photo_finish(p).expect("finish").wait().expect("pixels");
    save("lamp-speckles", &px);
    // The lamp's head on screen and a margin round it: its own bright pixels are no speckles.
    let c = proj * view * lamp.extend(1.0);
    let (x, y) = (
        ((c.x / c.w * 0.5 + 0.5) * W as f32) as u32,
        ((0.5 - c.y / c.w * 0.5) * H as f32) as u32,
    );
    let speckles = isolated_bright(&px, (x - 8, y - 8, x + 8, y + 8));
    // The point light lights the wall beside the lamp.
    let wall = luma(&px, x + 15, y - 20);
    eprintln!("lamp at ({x}, {y}); wall by it {wall:.0}; {speckles} speckles");
    assert!(wall > 10.0, "the lamp lights the wall: {wall}");
    assert!(
        speckles <= 2,
        "{speckles} isolated bright pixels round the lamp"
    );
}
