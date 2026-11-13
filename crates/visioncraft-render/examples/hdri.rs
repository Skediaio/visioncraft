//! Real HDRI skies: loads Radiance .hdr files (a small RGBE reader here, so
//! the library has no image dependency), calibrates each with
//! `calibrate_hdri`, prints where the sun is, and renders the garden lit by
//! it (HIGH still) plus a view straight at the sun, where the image's own sun
//! and the analytic sun disc must coincide.
//!
//! `cargo run --release -p visioncraft-render --example hdri -- <out dir> <file.hdr>...`

use std::path::{Path, PathBuf};

use glam::Vec3;
use visioncraft_render::demo;
use visioncraft_render::{HdriImage, Renderer, Sky, calibrate_hdri, headless_device};

/// Radiance RGBE (Ward, "Real Pixels", Graphics Gems II, 1991): a text
/// header, a "-Y h +X w" line, then scanlines, run-length encoded per
/// channel when they start with 2, 2.
fn read_hdr(path: &Path) -> Result<(u32, u32, Vec<f32>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut at = 0;
    let mut line = || {
        let start = at;
        while at < bytes.len() && bytes[at] != b'\n' {
            at += 1;
        }
        at += 1;
        String::from_utf8_lossy(&bytes[start..at - 1]).to_string()
    };
    while !line().is_empty() {}
    let size = line();
    let f: Vec<&str> = size.split_whitespace().collect();
    if f.len() != 4 || f[0] != "-Y" || f[2] != "+X" {
        return Err(format!("unsupported orientation {size:?}"));
    }
    let (h, w): (usize, usize) = (
        f[1].parse().map_err(|_| "height")?,
        f[3].parse().map_err(|_| "width")?,
    );
    let mut rgb = Vec::with_capacity(w * h * 3);
    let mut scan = vec![0u8; w * 4];
    for _ in 0..h {
        if bytes.get(at..at + 2) == Some(&[2, 2]) {
            at += 4;
            for c in 0..4 {
                let mut x = 0;
                while x < w {
                    let n = bytes[at] as usize;
                    at += 1;
                    if n > 128 {
                        for _ in 0..n - 128 {
                            scan[x * 4 + c] = bytes[at];
                            x += 1;
                        }
                        at += 1;
                    } else {
                        for _ in 0..n {
                            scan[x * 4 + c] = bytes[at];
                            at += 1;
                            x += 1;
                        }
                    }
                }
            }
        } else {
            scan.copy_from_slice(&bytes[at..at + w * 4]);
            at += w * 4;
        }
        for p in scan.chunks(4) {
            let scale = if p[3] == 0 {
                0.0
            } else {
                2f32.powi(p[3] as i32 - 136)
            };
            rgb.extend([
                p[0] as f32 * scale,
                p[1] as f32 * scale,
                p[2] as f32 * scale,
            ]);
        }
    }
    Ok((w as u32, h as u32, rgb))
}

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

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().expect("out dir"));
    std::fs::create_dir_all(&out).expect("out dir");
    let (device, queue) = pollster::block_on(headless_device()).expect("device");
    for file in args {
        let path = PathBuf::from(&file);
        let (w, h, rgb) = read_hdr(&path).expect("hdr");
        let image = HdriImage {
            width: w,
            height: h,
            rgb: &rgb,
        };
        let cal = calibrate_hdri(&image, 0.0).expect("calibrate");
        let d = cal.sun.direction;
        let name = path
            .parent()
            .and_then(|p| p.file_name())
            .map_or("sky".into(), |n| n.to_string_lossy().to_string());
        println!(
            "{name}: sun azimuth {:.1} elevation {:.1} deg, {:.0} lux; intensity {:.1}, EV {:.2}",
            d.x.atan2(-d.z).to_degrees(),
            d.y.asin().to_degrees(),
            cal.sun.illuminance,
            cal.intensity,
            cal.ev100
        );
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(64, 36);
        demo::garden(&mut r);
        r.set_time(2.0);
        r.set_sky(&Sky::Hdri {
            image,
            rotation_deg: 0.0,
            intensity: cal.intensity,
        })
        .expect("sky");
        r.set_sun(&cal.sun);
        r.set_exposure(cal.ev100);
        let (sw, sh) = (1280, 720);
        let aspect = sw as f32 / sh as f32;
        let (view, proj, eye) = demo::garden_camera(aspect);
        let px = r
            .render_still(sw, sh, view, proj, eye)
            .and_then(|s| s.wait())
            .expect("still");
        write_png(&out.join(format!("{name}.png")), &px, sw, sh);
        // Straight at the sun (or where it would be): one disc, not two.
        let eye = Vec3::new(0.0, 1.7, 20.0);
        let (view, proj, eye) = demo::camera(eye, eye + d * 10.0, aspect);
        let px = r
            .render_still(sw, sh, view, proj, eye)
            .and_then(|s| s.wait())
            .expect("still");
        write_png(&out.join(format!("{name}_sun.png")), &px, sw, sh);
    }
}
