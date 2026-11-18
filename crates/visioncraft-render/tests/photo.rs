//! Photo mode on the real GPU: averaged samples smooth a hard edge, and a thin lens blurs what
//! lies off its focal plane while what is on it stays sharp. Run on a machine with a GPU.

use std::sync::OnceLock;

use glam::{Mat4, Vec3};
use visioncraft_render::demo::MeshData;
use visioncraft_render::{Focus, Lens, Material, Quality, Renderer, Sun, headless_device};

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
