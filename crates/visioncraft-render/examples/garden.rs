//! Visual test bed: renders the demo scenes to PNG files for inspection,
//! including the 1920x1080 garden beauty shots (afternoon, golden hour, dusk
//! and night with the garden lights on, overcast), through
//! `Renderer::render_still` (HIGH quality).
//!
//! `cargo run --release -p visioncraft-render --example garden -- <out dir> [names...]`
//! With `VC_LIVE=low|medium|high` the shots go through the live path
//! (`resize`, `set_quality`, `render`) instead, named `<shot>_<preset>`.

use std::path::{Path, PathBuf};

use glam::Vec3;
use visioncraft_render::demo::{self, Camera};
use visioncraft_render::{Atmosphere, HdriImage, Quality, Renderer, Sky, Sun, headless_device};

fn write_png(path: &Path, pixels: &[u8], w: u32, h: u32) {
    let mut enc = png::Encoder::new(std::fs::File::create(path).expect("png file"), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(pixels)
        .expect("png data");
    eprintln!("wrote {}", path.display());
}

struct Shot {
    name: &'static str,
    size: (u32, u32),
    build: fn(&mut Renderer, f32) -> (Camera, Sun, f32),
}

fn shots() -> Vec<Shot> {
    vec![
        Shot {
            name: "garden_afternoon",
            size: (1920, 1080),
            build: |r, aspect| {
                demo::garden(r);
                r.set_time(2.0);
                (demo::garden_camera(aspect), demo::afternoon_sun(), 14.7)
            },
        },
        Shot {
            name: "garden_golden",
            size: (1920, 1080),
            build: |r, aspect| {
                demo::garden(r);
                r.set_time(2.0);
                (demo::garden_camera(aspect), demo::golden_sun(), 12.7)
            },
        },
        Shot {
            name: "garden_dusk",
            size: (1920, 1080),
            build: |r, aspect| {
                let lights = demo::garden(r);
                r.set_lights(&lights);
                r.set_time(2.0);
                (demo::garden_camera(aspect), demo::dusk_sun(), 7.0)
            },
        },
        Shot {
            name: "garden_overcast",
            size: (1920, 1080),
            build: |r, aspect| {
                demo::garden(r);
                r.set_time(2.0);
                let pixels = demo::overcast_hdri(512);
                let image = HdriImage {
                    width: 512,
                    height: 256,
                    rgb: &pixels,
                };
                r.set_sky(&Sky::Hdri {
                    image,
                    rotation_deg: 0.0,
                    intensity: 6000.0,
                })
                .expect("overcast sky");
                r.set_atmosphere(&Atmosphere {
                    haze: 3.0,
                    ..Default::default()
                });
                let sun = Sun {
                    illuminance: 0.0,
                    ..demo::afternoon_sun()
                };
                (demo::garden_camera(aspect), sun, 12.0)
            },
        },
        Shot {
            name: "garden_night",
            size: (1920, 1080),
            build: |r, aspect| {
                let lights = demo::garden(r);
                r.set_lights(&lights);
                (demo::garden_camera(aspect), demo::night_sun(), 1.5)
            },
        },
        // The planar reflection from two camera positions: the mirrored
        // house and trees must stay put on the water as the camera moves.
        Shot {
            name: "garden_pond_a",
            size: (1280, 720),
            build: |r, aspect| {
                demo::garden(r);
                r.set_time(2.0);
                let cam = demo::camera(Vec3::new(9.0, 1.6, 15.0), Vec3::new(4.0, 0.0, 6.0), aspect);
                (cam, demo::afternoon_sun(), 14.7)
            },
        },
        Shot {
            name: "garden_pond_b",
            size: (1280, 720),
            build: |r, aspect| {
                demo::garden(r);
                r.set_time(2.0);
                let cam = demo::camera(Vec3::new(1.0, 2.4, 15.5), Vec3::new(5.0, 0.0, 6.0), aspect);
                (cam, demo::afternoon_sun(), 14.7)
            },
        },
        // A long view over the field to the horizon: haze and the ground.
        Shot {
            name: "garden_long",
            size: (1920, 1080),
            build: |r, aspect| {
                demo::garden(r);
                r.set_time(2.0);
                let cam = demo::camera(
                    Vec3::new(-6.0, 1.7, -12.0),
                    Vec3::new(-40.0, 1.2, -60.0),
                    aspect,
                );
                (cam, demo::afternoon_sun(), 14.7)
            },
        },
        Shot {
            name: "garden_clay",
            size: (1280, 720),
            build: |r, aspect| {
                demo::garden(r);
                r.set_clay(true);
                (demo::garden_camera(aspect), demo::afternoon_sun(), 14.7)
            },
        },
        Shot {
            name: "bricks",
            size: (1280, 720),
            build: |r, aspect| {
                demo::brick_walls(r);
                (
                    demo::brick_walls_camera(aspect),
                    demo::afternoon_sun(),
                    15.0,
                )
            },
        },
        Shot {
            name: "foliage_small",
            size: (480, 270),
            build: |r, aspect| {
                demo::foliage_tree(r);
                (demo::foliage_camera(aspect), demo::foliage_sun(), 15.0)
            },
        },
        Shot {
            name: "foliage_large",
            size: (1920, 1080),
            build: |r, aspect| {
                demo::foliage_tree(r);
                (demo::foliage_camera(aspect), demo::foliage_sun(), 15.0)
            },
        },
        Shot {
            name: "foliage_backlit",
            size: (1280, 720),
            build: |r, aspect| {
                demo::foliage_tree(r);
                (
                    demo::foliage_camera(aspect),
                    Sun::from_angles(350.0, 25.0),
                    14.0,
                )
            },
        },
        Shot {
            name: "lods",
            size: (1280, 720),
            build: |r, aspect| {
                demo::lod_row(r);
                (demo::lod_camera(aspect), demo::afternoon_sun(), 15.0)
            },
        },
        Shot {
            name: "spheres",
            size: (1280, 720),
            build: |r, aspect| {
                demo::spheres(r);
                (demo::spheres_camera(aspect), demo::noon_sun(), 15.0)
            },
        },
        Shot {
            name: "glass",
            size: (1280, 720),
            build: |r, aspect| {
                demo::glass_room(r);
                (demo::glass_camera(aspect), demo::afternoon_sun(), 15.0)
            },
        },
        Shot {
            name: "pond",
            size: (1280, 720),
            build: |r, aspect| {
                demo::pond(r);
                r.set_time(3.0);
                (demo::pond_camera(aspect), demo::afternoon_sun(), 15.0)
            },
        },
        Shot {
            name: "night",
            size: (1280, 720),
            build: |r, aspect| {
                demo::house(r);
                r.set_lights(&demo::house_lights());
                (demo::house_camera(aspect), demo::night_sun(), 2.0)
            },
        },
    ]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| "target/garden".into()));
    let only: Vec<String> = args.collect();
    std::fs::create_dir_all(&out).expect("out dir");
    let (device, queue) = pollster::block_on(headless_device()).expect("device");
    for shot in shots() {
        if !only.is_empty() && !only.iter().any(|n| shot.name.starts_with(n.as_str())) {
            continue;
        }
        let (w, h) = shot.size;
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(64, 36);
        r.set_quality(Quality::LOW);
        let ((view, proj, eye), sun, ev) = (shot.build)(&mut r, w as f32 / h as f32);
        r.set_sun(&sun);
        r.set_exposure(ev);
        let start = std::time::Instant::now();
        let live = std::env::var("VC_LIVE").ok();
        let preset = live
            .as_deref()
            .and_then(|l| Quality::PRESETS.iter().find(|(n, _)| *n == l));
        let (px, name) = match preset {
            Some((preset, quality)) => {
                r.resize(w, h);
                r.set_quality(*quality);
                r.set_camera(view, proj, eye);
                let px = live_frame(&device, &queue, &mut r, w, h);
                (px, format!("{}_{preset}", shot.name))
            }
            None => {
                let px = r
                    .render_still(w, h, view, proj, eye)
                    .and_then(|s| s.wait())
                    .expect("still");
                (px, shot.name.to_string())
            }
        };
        eprintln!("{name}: {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);
        write_png(&out.join(format!("{name}.png")), &px, w, h);
    }
}

/// One live frame read back (tightly packed RGBA8).
fn live_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &mut Renderer,
    w: u32,
    h: u32,
) -> Vec<u8> {
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
