use glam::{Mat4, Vec3};
use visioncraft_render::{headless_device, Renderer};

const W: u32 = 640;
const H: u32 = 360;

#[test]
fn renders_5000_cubes_headless() {
    // Fails loudly (unwrap) when there is no adapter: no silent skip.
    let (device, queue) = pollster::block_on(headless_device()).expect("headless wgpu device");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(&device, &queue, format);
    r.resize(W, H);

    // 71 x 71 grid (5041 > 5000, trimmed), cubes of varying height.
    let models: Vec<Mat4> = (0..5000u32)
        .map(|i| {
            let (x, z) = ((i % 71) as f32 - 35.0, (i / 71) as f32 - 35.0);
            let h = 1.0 + ((i * 7919) % 13) as f32 * 0.4;
            Mat4::from_translation(Vec3::new(x * 2.5, h / 2.0, z * 2.5)) * Mat4::from_scale(Vec3::new(1.0, h, 1.0))
        })
        .collect();
    r.set_instances(&models);
    let proj = Mat4::perspective_rh(55f32.to_radians(), W as f32 / H as f32, 0.1, 5000.0);
    let view = Mat4::look_at_rh(Vec3::new(0.0, 45.0, 150.0), Vec3::new(0.0, 0.0, 0.0), Vec3::Y);
    r.set_camera(proj * view);

    let mut encoder = device.create_command_encoder(&Default::default());
    r.render(&mut encoder);
    let stride = W * 4; // 2560, a multiple of 256
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
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(H) },
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |res| res.expect("map"));
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let pixels = buffer.slice(..).get_mapped_range().expect("mapped range").to_vec();

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("spike-render.png");
    let mut enc = png::Encoder::new(std::fs::File::create(&path).expect("png file"), W, H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().expect("png header").write_image_data(&pixels).expect("png data");
    eprintln!("wrote {}", path.display());

    let px = |x: u32, y: u32| {
        let i = ((y * W + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    let clear = px(0, 0); // top-left is the sky (clear colour)
    let differing = pixels.chunks(4).filter(|p| p[..3] != clear).count();
    assert!(differing > (W * H / 4) as usize, "image is (nearly) uniform: {differing} differing pixels");
    let c = px(W / 2, H / 2);
    assert!(c.iter().any(|&v| v > 10), "centre pixel is black: {c:?}");
}
