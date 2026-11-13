//! A tiny software rasteriser: orthographic three-quarter view, flat shading, base-colour
//! texture, transparent background. Used for thumbnails and LOD comparison images.

use crate::scene::{Alpha, Model};
use image::{DynamicImage, RgbaImage};

const SS: u32 = 2;

/// `size` x `size` RGBA render of the model from the front-right, 30 degrees above.
pub fn render(model: &Model, size: u32, marker: Option<[f32; 3]>) -> RgbaImage {
    let s = size * SS;
    let (yaw, pitch) = (-45.0_f32.to_radians(), 30.0_f32.to_radians());
    let (sy, cy, sp, cp) = (yaw.sin(), yaw.cos(), pitch.sin(), pitch.cos());
    // world -> view: x right, y up, z towards the viewer
    let view = |p: [f32; 3]| {
        let (x1, z1) = (p[0] * cy + p[2] * sy, -p[0] * sy + p[2] * cy);
        [x1, p[1] * cp - z1 * sp, p[1] * sp + z1 * cp]
    };
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in model.meshes.iter().flat_map(|m| &m.pos) {
        let v = view(*p);
        for k in 0..2 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let margin = 0.04 * s as f32;
    let scale = (s as f32 - 2.0 * margin) / (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-6);
    let to_px = |v: [f32; 3]| {
        [
            (v[0] - (lo[0] + hi[0]) / 2.0) * scale + s as f32 / 2.0,
            s as f32 / 2.0 - (v[1] - (lo[1] + hi[1]) / 2.0) * scale,
        ]
    };
    let light = {
        let l = [-0.35_f32, 0.55, 0.75];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    let textures: Vec<Option<RgbaImage>> = model
        .mats
        .iter()
        .map(|m| {
            m.base
                .as_ref()
                .map(|b| crate::glb::fit(b, 512))
                .map(|b: DynamicImage| b.to_rgba8())
        })
        .collect();
    let mut depth = vec![f32::MIN; (s * s) as usize];
    let mut col = vec![[0.0_f32; 3]; (s * s) as usize];
    let mut hit = vec![false; (s * s) as usize];
    for mesh in &model.meshes {
        let mat = &model.mats[mesh.mat];
        let tex = textures[mesh.mat].as_ref();
        for t in mesh.idx.as_chunks::<3>().0 {
            let i = [t[0] as usize, t[1] as usize, t[2] as usize];
            let v: [[f32; 3]; 3] = std::array::from_fn(|k| view(mesh.pos[i[k]]));
            let px: [[f32; 2]; 3] = std::array::from_fn(|k| to_px(v[k]));
            let e1: [f32; 3] = std::array::from_fn(|k| v[1][k] - v[0][k]);
            let e2: [f32; 3] = std::array::from_fn(|k| v[2][k] - v[0][k]);
            let mut n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-12 {
                continue;
            }
            if n[2] < 0.0 {
                n = [-n[0], -n[1], -n[2]];
            }
            let shade = 0.35
                + 0.65 * ((n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / len).max(0.0);
            let area = (px[1][0] - px[0][0]) * (px[2][1] - px[0][1])
                - (px[2][0] - px[0][0]) * (px[1][1] - px[0][1]);
            if area.abs() < 1e-9 {
                continue;
            }
            let min_x = px
                .iter()
                .map(|p| p[0])
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as u32;
            let max_x = (px
                .iter()
                .map(|p| p[0])
                .fold(f32::MIN, f32::max)
                .ceil()
                .max(0.0) as u32)
                .min(s - 1);
            let min_y = px
                .iter()
                .map(|p| p[1])
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as u32;
            let max_y = (px
                .iter()
                .map(|p| p[1])
                .fold(f32::MIN, f32::max)
                .ceil()
                .max(0.0) as u32)
                .min(s - 1);
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w1 = ((fx - px[0][0]) * (px[2][1] - px[0][1])
                        - (px[2][0] - px[0][0]) * (fy - px[0][1]))
                        / area;
                    let w2 = ((px[1][0] - px[0][0]) * (fy - px[0][1])
                        - (fx - px[0][0]) * (px[1][1] - px[0][1]))
                        / area;
                    let w0 = 1.0 - w1 - w2;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * v[0][2] + w1 * v[1][2] + w2 * v[2][2];
                    let o = (y * s + x) as usize;
                    if z <= depth[o] {
                        continue;
                    }
                    let mut rgba = mat.base_factor;
                    if let Some(tex) = tex {
                        let u =
                            w0 * mesh.uv[i[0]][0] + w1 * mesh.uv[i[1]][0] + w2 * mesh.uv[i[2]][0];
                        let w =
                            w0 * mesh.uv[i[0]][1] + w1 * mesh.uv[i[1]][1] + w2 * mesh.uv[i[2]][1];
                        let tx = ((u.rem_euclid(1.0)) * tex.width() as f32) as u32 % tex.width();
                        let ty = ((w.rem_euclid(1.0)) * tex.height() as f32) as u32 % tex.height();
                        let p = tex.get_pixel(tx, ty).0;
                        for k in 0..4 {
                            rgba[k] *= f32::from(p[k]) / 255.0;
                        }
                    }
                    let cut = match mat.alpha {
                        Alpha::Opaque => false,
                        Alpha::Mask => rgba[3] < mat.cutoff,
                        Alpha::Blend => rgba[3] < 0.5,
                    };
                    if cut {
                        continue;
                    }
                    depth[o] = z;
                    hit[o] = true;
                    col[o] = std::array::from_fn(|k| (rgba[k] * shade).min(1.0));
                }
            }
        }
    }
    let mut out = RgbaImage::new(size, size);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let (mut c, mut n) = ([0.0_f32; 3], 0.0_f32);
        for dy in 0..SS {
            for dx in 0..SS {
                let o = ((y * SS + dy) * s + x * SS + dx) as usize;
                if hit[o] {
                    n += 1.0;
                    for k in 0..3 {
                        c[k] += col[o][k];
                    }
                }
            }
        }
        if n > 0.0 {
            let byte = |v: f32| (v * 255.0).round() as u8;
            *p = image::Rgba([
                byte(c[0] / n),
                byte(c[1] / n),
                byte(c[2] / n),
                byte(n / (SS * SS) as f32),
            ]);
        }
    }
    if let Some(m) = marker {
        // red cross with a white outline, to check where a light's emitter sits
        let [px, py] = to_px(view(m));
        let (cx, cy) = ((px / SS as f32) as i32, (py / SS as f32) as i32);
        for (r, c) in [(3, [255, 255, 255, 255]), (2, [255, 0, 0, 255])] {
            for d in -(r + 4)..=(r + 4) {
                for (x, y) in [(cx + d, cy), (cx, cy + d)] {
                    for w in -(r - 2)..=(r - 2) {
                        let (x, y) = if y == cy { (x, y + w) } else { (x + w, y) };
                        if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
                            out.put_pixel(x as u32, y as u32, image::Rgba(c));
                        }
                    }
                }
            }
        }
    }
    out
}
