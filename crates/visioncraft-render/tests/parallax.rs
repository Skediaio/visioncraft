//! Parallax occlusion on gravel ground (scene.wgsl `parallax`), drawn for a look by eye: a
//! standing eye looking 10 m ahead, the same eye zoomed in on the ground 4 m ahead, and the
//! ground from 60 degrees of elevation, 2 m away, zoomed in (the zoomed views put about a
//! texel of a 1k map in a pixel, as a large high-resolution screen does, so the march runs at
//! full strength), on the slower and on the fast preset.
//!
//! Ignored: it needs the library's maps, which are not in the repository. `VC_GRAVEL_DIR`
//! (relative to the workspace root) holds `basecolor.jpg`, `normal.jpg`, `orm.jpg` and
//! `height.jpg` (`assets/materials/gravel016` of a checkout with the library), and
//! `VC_PARALLAX_DEPTH` sets the relief depth in metres (default 0.05, what library gravel used
//! to start with). The pictures go to `out/parallax/` under the workspace root. The layer
//! budget itself is checked without a GPU (lib.rs `parallax_layers_stay_within_a_texel_step`).

use std::path::PathBuf;
use std::sync::OnceLock;

use glam::{Mat4, Vec3};
use visioncraft_render::demo::{self, MeshData};
use visioncraft_render::{
    ColorSpace, Material, Projection, Quality, Renderer, TextureDesc, TextureId, headless_device,
};

const W: u32 = 960;
const H: u32 = 540;

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

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Writes the picture under `out/parallax/`.
fn dump(name: &str, px: &[u8]) {
    let path = workspace().join("out/parallax").join(format!("{name}.png"));
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let mut enc = png::Encoder::new(std::fs::File::create(&path).expect("png file"), W, H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(px)
        .expect("png data");
}

/// Square RGBA8 maps: base colour, normal, ORM, height.
struct Maps {
    size: u32,
    base: Vec<u8>,
    normal: Vec<u8>,
    orm: Vec<u8>,
    height: Vec<u8>,
}

fn load(dir: &str) -> Maps {
    let dir = workspace().join(dir);
    let map = |name: &str| {
        let img = image::open(dir.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .to_rgba8();
        (img.width(), img.into_raw())
    };
    let (size, base) = map("basecolor.jpg");
    Maps {
        size,
        base,
        normal: map("normal.jpg").1,
        orm: map("orm.jpg").1,
        height: map("height.jpg").1,
    }
}

/// A view: eye, target, vertical field of view (degrees).
type View = (Vec3, Vec3, f32);

/// A 1 m tile of `maps` laid over an 80 m square of flat ground, drawn at `quality` with
/// `depth_m` of relief in the afternoon sun.
fn ground(maps: &Maps, quality: Quality, depth_m: f32, (eye, target, fov_deg): View) -> Vec<u8> {
    let (device, queue) = gpu();
    let mut r = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    r.resize(W, H);
    r.set_quality(quality);
    let mut tex = |px: &[u8], color_space: ColorSpace| -> TextureId {
        r.add_texture(&TextureDesc {
            width: maps.size,
            height: maps.size,
            rgba8: px,
            color_space,
            repeat: true,
        })
        .expect("texture")
    };
    let base = tex(&maps.base, ColorSpace::Srgb);
    let normal = tex(&maps.normal, ColorSpace::Linear);
    let orm = tex(&maps.orm, ColorSpace::Linear);
    let height = tex(&maps.height, ColorSpace::Linear);
    let mut slab = MeshData::default();
    slab.add_box(Vec3::new(-40.0, -0.1, -40.0), Vec3::new(40.0, 0.0, 40.0));
    let slab = r.add_mesh(&slab.mesh()).expect("ground");
    let material = r.add_material(&Material {
        base_color: [1.0; 4],
        metallic: 1.0,
        roughness: 1.0,
        base_color_map: Some(base),
        normal_map: Some(normal),
        orm_map: Some(orm),
        height_map: Some(height),
        parallax_depth_m: depth_m,
        projection: Projection::Triplanar { scale_m: 1.0 },
        ..Default::default()
    });
    r.add_item(slab, material, &[Mat4::IDENTITY]);
    let view = Mat4::look_at_rh(eye, target, Vec3::Y);
    let proj = Mat4::perspective_rh(fov_deg.to_radians(), W as f32 / H as f32, 0.1, 1000.0);
    r.set_camera(view, proj, eye);
    r.set_sun(&demo::afternoon_sun());
    r.set_exposure(14.5);
    capture(&mut r)
}

#[test]
#[ignore = "pictures to look at; needs VC_GRAVEL_DIR"]
fn gravel_pictures() {
    let maps = load(&std::env::var("VC_GRAVEL_DIR").expect("VC_GRAVEL_DIR"));
    let depth = std::env::var("VC_PARALLAX_DEPTH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.05);
    let standing = Vec3::new(0.0, 1.7, 0.0);
    let shots: [(&str, View); 3] = [
        ("eye", (standing, Vec3::new(0.0, 0.0, -10.0), 50.0)),
        ("eyezoom", (standing, Vec3::new(0.0, 0.0, -4.0), 10.0)),
        ("top", (Vec3::new(0.0, 1.732, 1.0), Vec3::ZERO, 20.0)),
    ];
    for (preset, quality) in [("high", Quality::HIGH), ("fast", Quality::MEDIUM)] {
        for (shot, view) in shots {
            dump(
                &format!("{shot}-{preset}"),
                &ground(&maps, quality, depth, view),
            );
        }
    }
}
