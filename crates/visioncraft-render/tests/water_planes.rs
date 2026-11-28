//! Several water surfaces in one frame (`water.rs`): what the last frame's stats say of them
//! and their tiers, and what each tier shows, by pixel statistics on any adapter (CI's software
//! one included). With `VISIONCRAFT_WATER_FRAMES=<dir>` the frames are written there as PNGs.

use std::path::PathBuf;
use std::sync::OnceLock;

use glam::{Mat4, Vec3};
use visioncraft_render::demo::{self, Camera, MeshData};
use visioncraft_render::{
    Material, MaterialId, Quality, Renderer, Sun, Surface, WaterTier, headless_device,
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
    sum.map(|s| s / ((x1 - x0) * (y1 - y0)).max(1) as f32)
}

/// The colour round the pixel a world position projects to (3x3), or black off the frame.
fn at(r: &Renderer, px: &[u8], world: Vec3) -> [f32; 3] {
    let Some(p) = r.project(world) else {
        return [0.0; 3];
    };
    let (x, y) = (p.x as i32, p.y as i32);
    if x < 2 || y < 2 || x + 3 >= W as i32 || y + 3 >= H as i32 {
        return [0.0; 3];
    }
    mean(
        px,
        (x - 2) as u32,
        (y - 2) as u32,
        (x + 3) as u32,
        (y + 3) as u32,
    )
}

/// How much redder than the rest a colour is (0..255 scale; a red pillar's reflection is far
/// over 15).
fn redness(c: [f32; 3]) -> f32 {
    c[0] - (c[1] + c[2]) * 0.5
}

fn frame(name: &str, px: &[u8]) {
    let Some(dir) = std::env::var_os("VISIONCRAFT_WATER_FRAMES") else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("frames dir");
    let file = std::fs::File::create(dir.join(format!("mw_{name}.png"))).expect("png file");
    let mut enc = png::Encoder::new(file, W, H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(px)
        .expect("png data");
}

fn ground(r: &mut Renderer, top: f32) {
    let mut m = MeshData::default();
    m.add_box(
        Vec3::new(-60.0, top - 0.3, -60.0),
        Vec3::new(60.0, top, 60.0),
    );
    let mesh = r.add_mesh(&m.mesh()).expect("ground");
    let mat = r.add_material(&Material {
        base_color: [0.16, 0.2, 0.08, 1.0],
        roughness: 0.95,
        ..Default::default()
    });
    r.add_item(mesh, mat, &[Mat4::IDENTITY]);
}

fn water_material(r: &mut Renderer) -> MaterialId {
    r.add_material(&Material {
        base_color: [0.02, 0.04, 0.03, 0.45],
        roughness: 0.05,
        surface: Surface::Water,
        ..Default::default()
    })
}

/// A flat water surface from `(x0, z0)` to `(x1, z1)` at height `y`, its own item, on a block
/// of ground that rises to just under it.
fn pond(r: &mut Renderer, water: MaterialId, (x0, z0, x1, z1): (f32, f32, f32, f32), y: f32) {
    let mut m = MeshData::default();
    m.add_quad(
        [
            Vec3::new(x0, y, z0),
            Vec3::new(x0, y, z1),
            Vec3::new(x1, y, z1),
            Vec3::new(x1, y, z0),
        ],
        [[0.0; 2]; 4],
    );
    let mesh = r.add_mesh(&m.mesh()).expect("water");
    r.add_item(mesh, water, &[Mat4::IDENTITY]);
    let mut block = MeshData::default();
    block.add_box(
        Vec3::new(x0 - 0.05, -0.3, z0 - 0.05),
        Vec3::new(x1 + 0.05, y - 0.01, z1 + 0.05),
    );
    let block = r.add_mesh(&block.mesh()).expect("block");
    let stone = r.add_material(&Material {
        base_color: [0.3, 0.29, 0.27, 1.0],
        roughness: 0.9,
        ..Default::default()
    });
    r.add_item(block, stone, &[Mat4::IDENTITY]);
}

/// Five ponds at 0, 0.5, 1, 1.5 and 2 m, and a sixth at 1 m beside the third. With a budget of
/// two planar passes, two planes get one: the plane at 1 m, whose two surfaces both report
/// `planar`, and the best of the others; the remaining three follow screen-space reflections
/// when they are on, else the environment cube.
#[test]
fn five_surfaces_two_planar_both_of_one_level_and_the_rest_follow_ssr() {
    let mut r = renderer(W, H);
    ground(&mut r, 0.0);
    let water = water_material(&mut r);
    let at = [
        (-6.0, -7.0, -2.0, -4.0, 0.0),
        (0.0, -7.0, 4.0, -4.0, 0.5),
        (-5.0, 1.0, 5.0, 7.0, 1.0),
        (6.0, 2.0, 12.0, 8.0, 1.0),
        (-6.0, -13.0, -2.0, -10.0, 1.5),
        (1.0, -15.0, 6.0, -11.0, 2.0),
    ];
    for &(x0, z0, x1, z1, y) in &at {
        pond(&mut r, water, (x0, z0, x1, z1), y);
    }
    let cam = demo::camera(
        Vec3::new(0.0, 3.0, 12.0),
        Vec3::new(0.0, 1.0, 0.0),
        W as f32 / H as f32,
    );
    for (ssr, fallback) in [(0, WaterTier::Environment), (1, WaterTier::ScreenSpace)] {
        r.set_quality(Quality {
            water_planes: 2,
            water_reflection: 2,
            screen_reflections: ssr,
            ..Quality::HIGH
        });
        setup(&mut r, cam, demo::afternoon_sun(), 15.0);
        capture(&mut r, W, H);
        let stats = r.stats();
        let listed: Vec<(f32, WaterTier)> = stats
            .water
            .iter()
            .map(|w| {
                assert!(w.item.is_some(), "no sea here: {w:?}");
                (w.level_m, w.tier)
            })
            .collect();
        assert_eq!(listed.len(), 6, "{listed:?}");
        // By plane (height); the two at 1 m follow each other and share the tier.
        assert_eq!(
            listed.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
            [0.0, 0.5, 1.0, 1.0, 1.5, 2.0]
        );
        // Two planes have a pass: the plane at 1 m, whose two surfaces both say so, and the
        // best of the others (the surfaces of one plane follow each other in the list).
        let planar: Vec<f32> = listed
            .iter()
            .filter(|(_, t)| *t == WaterTier::Planar)
            .map(|(l, _)| *l)
            .collect();
        let planes = planar
            .iter()
            .enumerate()
            .filter(|(i, l)| *i == 0 || planar[i - 1] != **l)
            .count();
        assert_eq!(planes, 2, "with screen-space reflections {ssr}: {listed:?}");
        assert_eq!(
            planar.iter().filter(|l| **l == 1.0).count(),
            2,
            "{listed:?}"
        );
        let rest: Vec<WaterTier> = listed
            .iter()
            .filter(|(l, _)| *l != 1.0)
            .map(|(_, t)| *t)
            .collect();
        assert_eq!(
            rest.iter().filter(|t| **t == WaterTier::Planar).count(),
            1,
            "{listed:?}"
        );
        assert_eq!(
            rest.iter().filter(|t| **t == fallback).count(),
            3,
            "{listed:?}"
        );
        // Each surface names its item, part and material.
        for w in &stats.water {
            let i = w.item.expect("item");
            assert_eq!((i.part, i.material), (0, water.index()));
        }
    }
    // The planar reflection off altogether: nothing is ranked.
    r.set_quality(Quality {
        water_reflection: 0,
        ..Quality::HIGH
    });
    capture(&mut r, W, H);
    assert!(r.stats().water.is_empty(), "{:?}", r.stats().water);
}

/// A red pillar (0.4 m square, 2 m tall) standing with its base at `base`.
fn pillar(r: &mut Renderer, base: Vec3) {
    let mut m = MeshData::default();
    m.add_box(base, base + Vec3::new(0.4, 2.0, 0.4));
    let mesh = r.add_mesh(&m.mesh()).expect("pillar");
    let red = r.add_material(&Material {
        base_color: [0.7, 0.04, 0.03, 1.0],
        ..Default::default()
    });
    r.add_item(mesh, red, &[Mat4::IDENTITY]);
}

/// A pond from `(x0, z0)` to `(x1, z1)` at 2 cm over the ground, with a red pillar on its far
/// shore (the camera looks towards -z). Answers the mirror image of the pillar's point 0.5 m
/// up: where it projects, the water shows the pillar's base.
fn shore_pond(
    r: &mut Renderer,
    water: MaterialId,
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
    pillar_x: f32,
) -> Vec3 {
    pond(r, water, (x0, z0, x1, z1), 0.02);
    pillar(r, Vec3::new(pillar_x, 0.0, z0 - 0.6));
    // The mirror image, about the water at 0.02, of the pillar's point 0.5 m up.
    Vec3::new(pillar_x + 0.2, 0.04 - 0.5, z0 - 0.4)
}

/// A pond on a terrace 1.5 m up: the block rises to 1.49, the water lies at 1.5.
fn terrace_pond(
    r: &mut Renderer,
    water: MaterialId,
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
    pillar_x: f32,
) -> Vec3 {
    pond(r, water, (x0, z0, x1, z1), 1.5);
    pillar(r, Vec3::new(pillar_x, 1.5, z0 - 0.6));
    // The mirror image, about the water at 1.5, of the pillar's point 0.2 m up.
    Vec3::new(pillar_x + 0.2, 3.0 - 1.7, z0 - 0.4)
}

fn pond_camera() -> Camera {
    demo::camera(
        Vec3::new(2.0, 3.6, 9.0),
        Vec3::new(2.0, 0.9, -4.0),
        W as f32 / H as f32,
    )
}

/// One pond, one pillar: its mirror image shows red where it projects (the planar pass is
/// scissored to the pond's rectangle since `water.rs`: what it covers must not be cut off).
#[test]
fn one_pond_mirrors_the_pillar() {
    let mut r = renderer(W, H);
    ground(&mut r, 0.0);
    let water = water_material(&mut r);
    let mirrored = shore_pond(&mut r, water, -3.0, 3.0, -4.0, 0.0, -0.2);
    r.set_quality(Quality {
        water_planes: 1,
        screen_reflections: 0,
        ..Quality::HIGH
    });
    setup(&mut r, pond_camera(), demo::afternoon_sun(), 15.0);
    let px = capture(&mut r, W, H);
    let c = at(&r, &px, mirrored);
    assert!(redness(c) > 15.0, "{c:?}");
    let tiers: Vec<_> = r
        .stats()
        .water
        .iter()
        .map(|w| (w.level_m, w.tier))
        .collect();
    assert_eq!(tiers, [(0.02, WaterTier::Planar)]);
}

/// Two ponds, one at ground level and one on a terrace 1.5 m up, a red pillar on the far shore
/// of each. With two planar passes both mirrors show their pillar; with one, only the pond
/// that ranks first (the lower one: nearer, larger on screen) does, and the other shows the
/// environment cube.
#[test]
fn two_ponds_at_two_levels_each_mirror_their_shore_with_two_planar_passes() {
    let mut r = renderer(W, H);
    ground(&mut r, 0.0);
    let water = water_material(&mut r);
    let low = shore_pond(&mut r, water, -3.0, 3.0, -4.0, 0.0, -0.2);
    let high = terrace_pond(&mut r, water, 0.0, 5.0, -9.0, -5.2, 2.8);
    let mut red = Vec::new();
    for (name, planes) in [("budget1", 1), ("budget2", 2)] {
        r.set_quality(Quality {
            water_planes: planes,
            screen_reflections: 0,
            ..Quality::HIGH
        });
        setup(&mut r, pond_camera(), demo::afternoon_sun(), 15.0);
        let px = capture(&mut r, W, H);
        frame(name, &px);
        let tiers: Vec<_> = r
            .stats()
            .water
            .iter()
            .map(|w| (w.level_m, w.tier))
            .collect();
        assert_eq!(
            tiers,
            [
                (0.02, WaterTier::Planar),
                (
                    1.5,
                    if planes == 2 {
                        WaterTier::Planar
                    } else {
                        WaterTier::Environment
                    }
                )
            ],
            "{name}"
        );
        red.push((redness(at(&r, &px, low)), redness(at(&r, &px, high))));
    }
    let [(one_low, one_high), (two_low, two_high)] = [red[0], red[1]];
    assert!(one_low > 15.0 && two_low > 15.0, "{red:?}");
    assert!(
        one_high < 15.0,
        "one pass: the upper pond shows no pillar: {red:?}"
    );
    assert!(
        two_high > 15.0 && two_high > one_high + 15.0,
        "two passes: the upper pond mirrors its pillar: {red:?}"
    );
}

/// One planar pass and screen-space reflections on: the pond without a pass reflects its
/// pillar from the opaque scene of the frame where the pillar is on screen, and the
/// environment cube where the pillar is out of the picture (the camera tilted down past it).
#[test]
fn without_a_planar_pass_the_pond_reflects_what_is_on_screen() {
    let mut r = renderer(W, H);
    ground(&mut r, 0.0);
    let water = water_material(&mut r);
    let _low = shore_pond(&mut r, water, -3.0, 3.0, -4.0, 0.0, -0.2);
    let high = terrace_pond(&mut r, water, 0.0, 5.0, -9.0, -5.2, 2.8);
    // Water a little in front of the upper pillar, on the upper pond: its reflection is the
    // pillar's shaft.
    let shore = Vec3::new(2.95, 1.5, -7.0);
    r.set_quality(Quality {
        water_planes: 1,
        screen_reflections: 1,
        ..Quality::HIGH
    });
    let aspect = W as f32 / H as f32;
    let (eye, past) = (Vec3::new(2.0, 3.6, 9.0), Vec3::new(2.0, -4.5, -4.0));
    for (name, target) in [("ssr", Vec3::new(2.0, 0.9, -4.0)), ("ssr_offscreen", past)] {
        setup(
            &mut r,
            demo::camera(eye, target, aspect),
            demo::afternoon_sun(),
            15.0,
        );
        let px = capture(&mut r, W, H);
        frame(name, &px);
        let tiers: Vec<_> = r
            .stats()
            .water
            .iter()
            .map(|w| (w.level_m, w.tier))
            .collect();
        assert_eq!(
            tiers,
            [(0.02, WaterTier::Planar), (1.5, WaterTier::ScreenSpace)],
            "{name}"
        );
        // The whole pillar is above the picture's top edge in the second frame.
        let top = r.project(Vec3::new(3.0, 1.5, -9.4)).expect("projected").y;
        assert_eq!(
            top < 0.0,
            name == "ssr_offscreen",
            "{name}: pillar base at y {top}"
        );
        let water = at(&r, &px, shore);
        assert!(
            r.project(shore)
                .is_some_and(|p| p.y >= 0.0 && p.y < H as f32),
            "{name}: the shore water is in the picture"
        );
        if name == "ssr" {
            assert!(
                redness(at(&r, &px, high)) > 15.0,
                "{name}: the mirror image of the pillar"
            );
            assert!(
                redness(water) > 15.0,
                "{name}: the water by the shore {water:?}"
            );
        } else {
            assert!(redness(water) < 15.0, "{name}: {water:?}");
        }
    }
}
