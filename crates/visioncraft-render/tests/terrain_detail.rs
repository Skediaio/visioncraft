//! A detail texture per terrain layer (`terrain.set_layer_detail`; terrain.wgsl `terrain_detail`):
//! what it shows, that it costs nothing where it is not, and that it is skipped altogether
//! without one, by pixel statistics on any adapter (CI's software one included). With
//! `VISIONCRAFT_TERRAIN_DETAIL_PNG=<dir>` the pictures of the visual check are written there.

use std::path::PathBuf;
use std::sync::OnceLock;

use glam::{Mat4, Vec2, Vec3};
use visioncraft_render::demo::{self, Camera, Image};
use visioncraft_render::{
    ColorSpace, Ground, Material, Quality, Renderer, Sun, TerrainDetail, TerrainLayer,
    headless_device,
};

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

fn setup(r: &mut Renderer, (view, proj, eye): Camera, sun: Sun, ev100: f32) {
    r.set_camera(view, proj, eye);
    r.set_sun(&sun);
    r.set_exposure(ev100);
}

fn settle(r: &mut Renderer) {
    for _ in 0..3 {
        capture_any(r);
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
        .expect("mapped")
        .to_vec();
    padded
        .chunks(stride as usize)
        .flat_map(|row| &row[..(w * 4) as usize])
        .copied()
        .collect()
}

// ---- textures ----

/// Smooth value noise in -1..1 on a lattice of `cells` cells over the unit square, wrapping.
fn noise(x: f32, y: f32, cells: u32, seed: u32) -> f32 {
    let (fx, fy) = (x * cells as f32, y * cells as f32);
    let (ix, iy) = (fx.floor(), fy.floor());
    let f = |v: f32| v * v * (3.0 - 2.0 * v);
    let (tx, ty) = (f(fx - ix), f(fy - iy));
    let v = |i: i64, j: i64| {
        let h = |a: u32, b: u32| {
            let n = a
                .wrapping_mul(374761393)
                .wrapping_add(b.wrapping_mul(668265263))
                .wrapping_add(seed.wrapping_mul(2246822519));
            (n ^ (n >> 13)).wrapping_mul(1274126177) >> 8 & 0xFFFF
        };
        (h(
            (ix as i64 + i).rem_euclid(cells as i64) as u32,
            (iy as i64 + j).rem_euclid(cells as i64) as u32,
        ) as f32
            / 65535.0)
            * 2.0
            - 1.0
    };
    let (a, b) = (
        v(0, 0) + (v(1, 0) - v(0, 0)) * tx,
        v(0, 1) + (v(1, 1) - v(0, 1)) * tx,
    );
    a + (b - a) * ty
}

/// A sand map: light grains with darker pebbles of a few texels and wind ripples a sixteenth
/// of the tile apart, around 0.65; and its normal map (the red channel as a height field).
fn sand(size: u32, pebble_cells: u32, ripples: f32) -> (Image, Image) {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
            let pebbles = noise(u, v, pebble_cells, 7).clamp(-0.2, 1.0);
            let ripple = ((v * ripples + 0.15 * noise(u, v, 4, 9)) * std::f32::consts::TAU).sin();
            let grain = noise(u, v, 64, 3);
            let c = 0.66 - 0.28 * pebbles * pebbles + 0.05 * ripple + 0.04 * grain;
            rgba.extend(
                [c, c * 0.94, c * 0.82, 1.0].map(|k| (k.clamp(0.0, 1.0) * 255.0 + 0.5) as u8),
            );
        }
    }
    let base = Image {
        width: size,
        height: size,
        rgba,
    };
    let at = |x: i32, y: i32| {
        base.rgba
            [((y.rem_euclid(size as i32) * size as i32 + x.rem_euclid(size as i32)) * 4) as usize]
            as f32
            / 255.0
    };
    let mut normal = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (x, y) = (x as i32, y as i32);
            let dx = (at(x + 1, y) - at(x - 1, y)) * 2.5;
            let dy = (at(x, y + 1) - at(x, y - 1)) * 2.5;
            let n = Vec3::new(-dx, dy, 1.0).normalize();
            normal.extend(
                [n.x * 0.5 + 0.5, n.y * 0.5 + 0.5, n.z * 0.5 + 0.5, 1.0]
                    .map(|k| (k.clamp(0.0, 1.0) * 255.0 + 0.5) as u8),
            );
        }
    }
    (
        base,
        Image {
            width: size,
            height: size,
            rgba: normal,
        },
    )
}

/// A lawn map: blades of green, a little darker and lighter.
fn grass(size: u32) -> Image {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
            let blades = noise(u, v, 32, 11) * 0.6 + noise(u, v, 96, 5) * 0.4;
            let c = 0.32 + 0.12 * blades;
            rgba.extend(
                [c * 0.55, c, c * 0.3, 1.0].map(|k| (k.clamp(0.0, 1.0) * 255.0 + 0.5) as u8),
            );
        }
    }
    Image {
        width: size,
        height: size,
        rgba,
    }
}

/// The maps of the test scene, uploaded once.
struct Maps {
    sand: visioncraft_render::TextureId,
    sand_n: visioncraft_render::TextureId,
    grass: visioncraft_render::TextureId,
}

fn maps(r: &mut Renderer) -> Maps {
    let (s, s_n) = sand(256, 12, 16.0);
    Maps {
        sand: s.upload(r, ColorSpace::Srgb, true),
        sand_n: s_n.upload(r, ColorSpace::Linear, true),
        grass: grass(256).upload(r, ColorSpace::Srgb, true),
    }
}

fn detail(m: &Maps, strength: f32) -> TerrainDetail {
    TerrainDetail {
        base_color_map: Some(m.sand),
        normal_map: Some(m.sand_n),
        tile_m: 0.5,
        strength,
        fade_m: 40.0,
    }
}

/// Two layers: sand on the left half of the terrain (x < 0), lawn on the right; the sand may
/// carry a detail.
fn two_layers(m: &Maps, detail: Option<TerrainDetail>) -> Vec<TerrainLayer> {
    vec![
        TerrainLayer {
            base_color_map: Some(m.sand),
            normal_map: Some(m.sand_n),
            scale_m: 2.0,
            variation: 0.0,
            detail,
            ..TerrainLayer::default()
        },
        TerrainLayer {
            base_color_map: Some(m.grass),
            scale_m: 1.5,
            variation: 0.0,
            ..TerrainLayer::default()
        },
    ]
}

fn add_chunk(
    r: &mut Renderer,
    data: &demo::TerrainData,
    mat: visioncraft_render::MaterialId,
) -> (visioncraft_render::MeshId, visioncraft_render::ItemId) {
    let id = r.add_terrain_chunk(&data.chunk()).expect("terrain chunk");
    (id, r.add_item(id, mat, &[Mat4::IDENTITY]))
}

fn chunk_mat(r: &mut Renderer) -> visioncraft_render::MaterialId {
    r.add_material(&Material {
        base_color: [0.5, 0.5, 0.5, 1.0],
        roughness: 0.9,
        ..Material::default()
    })
}

fn ground(r: &mut Renderer) {
    r.set_ground(Some(Ground {
        color: [0.25, 0.22, 0.16],
        height: 0.0,
        ..Ground::default()
    }));
}

// ---- picture statistics ----

fn luminance(p: &[u8]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// Mean and variance of the luminance of a pixel rectangle (the picture is `w` wide).
type Rect = (u32, u32, u32, u32);

fn mean_variance(px: &[u8], w: u32, (x0, y0, x1, y1): Rect) -> (f32, f32) {
    let (mut sum, mut sum2, mut n) = (0.0, 0.0, 0.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let l = luminance(&px[((y * w + x) * 4) as usize..]);
            sum += l;
            sum2 += l * l;
            n += 1.0;
        }
    }
    let mean = sum / n;
    (mean, sum2 / n - mean * mean)
}

/// Pixels of a rectangle whose largest channel difference between two pictures is past `tol`.
fn differing(a: &[u8], b: &[u8], w: u32, (x0, y0, x1, y1): Rect, tol: u8) -> usize {
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 4) as usize;
            n += (0..3).any(|c| a[i + c].abs_diff(b[i + c]) > tol) as usize;
        }
    }
    n
}

/// The scene: 20 x 20 m of flat ground, sand left and lawn right. The camera stands on the
/// boundary looking along it and halfway down: the sand is left of the centre column, the lawn
/// right of it, and the ground runs from 0.7 m (the bottom of the picture) to 5.5 m (the top).
fn scene(r: &mut Renderer) -> visioncraft_render::MaterialId {
    let data = demo::terrain_chunk(
        Vec2::new(-10.0, -10.0),
        40,
        0.5,
        1,
        &|_, _| 0.0,
        &|x, _, _| {
            let mut w = [0u8; 8];
            w[usize::from(x >= 0.0)] = 255;
            w
        },
    );
    let mat = chunk_mat(r);
    add_chunk(r, &data, mat);
    ground(r);
    mat
}

fn near_camera(aspect: f32) -> Camera {
    demo::camera(Vec3::new(0.0, 2.0, 6.0), Vec3::new(0.0, 0.0, 4.0), aspect)
}

fn far_camera(aspect: f32) -> Camera {
    demo::camera(
        Vec3::new(0.0, 60.0, 400.0),
        Vec3::new(0.0, 0.0, 0.0),
        aspect,
    )
}

/// A shot of the scene with these layers and this camera, settled as the reference tests are.
fn shot(r: &mut Renderer, layers: &[TerrainLayer], cam: Camera) -> Vec<u8> {
    r.set_terrain_layers(layers).expect("terrain layers");
    setup(r, cam, demo::noon_sun(), 14.0);
    settle(r);
    capture(r, W, H)
}

// Sand left of the boundary, clear of its half-metre blend (48 px wide at 3 m, wider nearer:
// so the left third over the rows nearer than that); lawn right of it, everywhere.
const LEFT: Rect = (8, 74, 180, 262);
const RIGHT: Rect = (248, 8, 472, 262);

/// Without a detail the picture is the base; a detail with strength 0 and a detail seen from
/// past its fade distance are the base to the bit; near the eye the detail changes the sand's
/// area (more contrast, the same brightness) and not the lawn's.
#[test]
fn a_detail_shows_near_the_eye_only_where_its_layer_is() {
    let mut r = renderer(W, H);
    let m = maps(&mut r);
    scene(&mut r);
    let aspect = W as f32 / H as f32;
    let (near, far) = (near_camera(aspect), far_camera(aspect));
    let none = two_layers(&m, None);
    let base = shot(&mut r, &none, near);

    // A detail of strength 0 leaves every pixel as it was.
    let zero = shot(&mut r, &two_layers(&m, Some(detail(&m, 0.0))), near);
    assert_eq!(base, zero, "a detail of strength 0 changed the picture");

    // Past the fade distance (40 m; the eye is 400 m away) the picture is the base to the bit.
    let far_base = shot(&mut r, &none, far);
    let far_detail = shot(&mut r, &two_layers(&m, Some(detail(&m, 0.8))), far);
    assert_eq!(
        far_base, far_detail,
        "a detail past its fade distance changed the picture"
    );

    // Near the eye (0.7 to 5.5 m): the sand changes, the lawn does not.
    let with = shot(&mut r, &two_layers(&m, Some(detail(&m, 0.8))), near);
    let pixels = |p: Rect| ((p.2 - p.0) * (p.3 - p.1)) as usize;
    let changed = differing(&base, &with, W, LEFT, 2);
    assert!(
        changed > pixels(LEFT) / 3,
        "{changed} of {} sand pixels changed",
        pixels(LEFT)
    );
    let n = differing(&base, &with, W, RIGHT, 1);
    if let Some(dir) = std::env::var_os("VISIONCRAFT_TERRAIN_DETAIL_PNG") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("pictures dir");
        for (name, px) in [("test_base", &base), ("test_with", &with)] {
            let file = std::fs::File::create(dir.join(format!("td_{name}.png"))).expect("png");
            let mut enc = png::Encoder::new(file, W, H);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .expect("png header")
                .write_image_data(px)
                .expect("png data");
        }
    }
    // (A pixel away from the sand may move by one step: bloom and FXAA reach over the line.)
    assert_eq!(n, 0, "lawn: {n} pixels changed by more than one step");
    let (mean0, var0) = mean_variance(&base, W, LEFT);
    let (mean1, var1) = mean_variance(&with, W, LEFT);
    eprintln!(
        "sand: mean {mean0:.1} -> {mean1:.1}, variance {var0:.1} -> {var1:.1}; \
         {changed} of {} pixels changed by more than two steps",
        pixels(LEFT)
    );
    assert!(
        var1 > var0 * 1.15,
        "the detail adds no contrast: variance {var0:.1} -> {var1:.1}"
    );
    assert!(
        (mean1 - mean0).abs() < mean0 * 0.08,
        "the detail changed the brightness: mean {mean0:.1} -> {mean1:.1}"
    );
}

/// The visual check: a sand terrain near and far, and a steep slope, with and without the
/// detail (written as PNGs when `VISIONCRAFT_TERRAIN_DETAIL_PNG` names a directory).
#[test]
fn terrain_detail_pictures() {
    let Some(dir) = std::env::var_os("VISIONCRAFT_TERRAIN_DETAIL_PNG") else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("pictures dir");
    let (w, h) = (960u32, 540u32);
    let aspect = w as f32 / h as f32;
    let mut r = renderer(w, h);
    // Coarser grain (pebbles of 8 cm, ripples 8 cm apart at the detail's 0.5 m): it reads at
    // the few metres the pictures are taken from.
    let (s, s_n) = sand(256, 6, 6.0);
    let m = Maps {
        sand: s.upload(&mut r, ColorSpace::Srgb, true),
        sand_n: s_n.upload(&mut r, ColorSpace::Linear, true),
        grass: grass(64).upload(&mut r, ColorSpace::Srgb, true),
    };
    let mat = chunk_mat(&mut r);
    let one = |detail| {
        vec![TerrainLayer {
            base_color_map: Some(m.sand),
            normal_map: Some(m.sand_n),
            scale_m: 2.0,
            variation: 0.2,
            detail,
            ..TerrainLayer::default()
        }]
    };
    let save = |name: &str, px: &[u8]| {
        let file = std::fs::File::create(dir.join(format!("td_{name}.png"))).expect("png file");
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .expect("png header")
            .write_image_data(px)
            .expect("png data");
    };
    // Flat sand: standing on it, and from 80 m.
    let flat = demo::terrain_chunk(
        Vec2::new(-20.0, -20.0),
        80,
        0.5,
        1,
        &|_, _| 0.0,
        &|_, _, _| [255, 0, 0, 0, 0, 0, 0, 0],
    );
    let flat_ids = add_chunk(&mut r, &flat, mat);
    ground(&mut r);
    // Noon sun: the sand is bright, the sunlit slope (facing the sun) nearly twice as bright as
    // the flat ground, so its exposure (EV) is higher.
    let sun = demo::noon_sun();
    let eye = demo::camera(Vec3::new(0.0, 1.1, 13.0), Vec3::new(1.5, 0.0, 8.0), aspect);
    let close = demo::camera(Vec3::new(0.6, 0.9, 10.3), Vec3::new(1.0, 0.0, 9.2), aspect);
    let far = demo::camera(
        Vec3::new(30.0, 25.0, 70.0),
        Vec3::new(0.0, 0.0, 0.0),
        aspect,
    );
    for (name, layers, cam) in [
        ("flat_without", one(None), eye),
        ("flat_with", one(Some(detail(&m, 0.8))), eye),
        ("close_without", one(None), close),
        ("close_with", one(Some(detail(&m, 0.8))), close),
        ("far_without", one(None), far),
        ("far_with", one(Some(detail(&m, 0.8))), far),
    ] {
        r.set_terrain_layers(&layers).expect("terrain layers");
        setup(&mut r, cam, sun, 14.0);
        settle(&mut r);
        save(name, &capture(&mut r, w, h));
    }
    // A ridge 6 m high with faces at 53 degrees, from 5 m before its foot: the detail is
    // projected triplanar on the face, at the grain of the flat ground before it. The flat
    // chunk gives way to it: one renderer for every picture (a second renderer on the same
    // device drew the terrain's maps flat here).
    r.remove_item(flat_ids.1);
    r.remove_mesh(flat_ids.0);
    let ridge = demo::terrain_chunk(
        Vec2::new(-20.0, -20.0),
        80,
        0.5,
        1,
        &|x, z| {
            6.0 * (1.0 - (z.abs() / 4.5)).max(0.0)
                + 0.2 * (x / 7.0).sin() * (1.0 - (z.abs() / 4.5)).max(0.0)
        },
        &|_, _, _| [255, 0, 0, 0, 0, 0, 0, 0],
    );
    add_chunk(&mut r, &ridge, mat);
    ground(&mut r);
    let slope = demo::camera(Vec3::new(2.0, 1.2, 9.5), Vec3::new(0.0, 2.5, 2.0), aspect);
    for (name, layers) in [
        ("slope_without", one(None)),
        ("slope_with", one(Some(detail(&m, 0.8)))),
    ] {
        r.set_terrain_layers(&layers).expect("terrain layers");
        setup(&mut r, slope, sun, 16.5);
        settle(&mut r);
        save(name, &capture(&mut r, w, h));
    }
}

/// What the detail costs on the GPU: the passes' times over a terrain that fills the view, with
/// a detail on every fragment (strength 1, faded out only at 1 km) and without one. Ignored:
/// run it on a real GPU with `--ignored`; it prints the means and only checks that the draws
/// are the same (the detail shades, it draws nothing).
#[test]
#[ignore]
fn a_detail_costs_gpu_time_in_the_forward_pass_only() {
    let (w, h) = (1280u32, 720u32);
    let mut r = renderer(w, h);
    let m = maps(&mut r);
    let mat = chunk_mat(&mut r);
    // 160 x 160 m of sand the camera stands on, looking to the horizon: terrain on every pixel.
    let data = demo::terrain_chunk(
        Vec2::new(-80.0, -80.0),
        160,
        1.0,
        1,
        &|_, _| 0.0,
        &|_, _, _| [255, 0, 0, 0, 0, 0, 0, 0],
    );
    add_chunk(&mut r, &data, mat);
    ground(&mut r);
    let one = |detail| {
        vec![TerrainLayer {
            base_color_map: Some(m.sand),
            normal_map: Some(m.sand_n),
            scale_m: 2.0,
            detail,
            ..TerrainLayer::default()
        }]
    };
    let cam = demo::camera(
        Vec3::new(0.0, 1.6, 20.0),
        Vec3::new(0.0, 0.8, -30.0),
        w as f32 / h as f32,
    );
    let mean = |r: &mut Renderer, detail| {
        r.set_terrain_layers(&one(detail)).expect("terrain layers");
        setup(r, cam, demo::noon_sun(), 14.0);
        settle(r);
        let mut sum = std::collections::HashMap::<&'static str, f32>::new();
        let mut draws = 0;
        for _ in 0..60 {
            capture_any(r);
            for (label, ms) in &r.stats().gpu_passes {
                *sum.entry(label).or_default() += ms;
            }
            draws = r.stats().draw_calls;
        }
        (sum, draws)
    };
    let (without, draws0) = mean(&mut r, None);
    let (with, draws1) = mean(
        &mut r,
        Some(TerrainDetail {
            base_color_map: Some(m.sand),
            normal_map: Some(m.sand_n),
            tile_m: 0.5,
            strength: 1.0,
            fade_m: 1000.0,
        }),
    );
    for label in ["shadow", "depth", "forward", "post"] {
        let a = without.get(label).copied().unwrap_or(0.0) / 60.0;
        let b = with.get(label).copied().unwrap_or(0.0) / 60.0;
        eprintln!(
            "{label:>8}: {a:.3} ms without the detail, {b:.3} ms with ({:+.3})",
            b - a
        );
    }
    eprintln!("draw calls: {draws0} without the detail, {draws1} with");
    eprintln!(
        "terrain textures: {} bytes",
        r.stats()
            .texture_breakdown
            .iter()
            .find(|(l, _)| *l == "terrain")
            .map_or(0, |(_, b)| *b)
    );
    assert_eq!(draws0, draws1, "the detail draws");
}
