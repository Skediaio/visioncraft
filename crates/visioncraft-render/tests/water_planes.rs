//! Several water surfaces in one frame (`water.rs`): what the last frame's stats say of them
//! and their tiers, on any adapter (CI's software one included).

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
