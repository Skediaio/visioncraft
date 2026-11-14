//! Block compression (BC1, BC3, BC4, BC5) of RGBA8 images on the CPU, with a mip chain made
//! on the CPU too (a compressed texture cannot be rendered into). Fast and simple encoders:
//! endpoints from the block's bounding box (inset a little), every texel to its nearest palette
//! entry. Four to eight times less GPU memory than RGBA8 for the same image.
//!
//! The formats exist on desktop GPUs (`TEXTURE_COMPRESSION_BC`, also in desktop browsers);
//! where they do not, callers keep RGBA8 (`Renderer::add_texture`).

use crate::texture::ColorSpace;

/// What an image holds, which decides its format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcKind {
    /// Opaque colour or packed data in RGB (BC1, 4 bits per texel).
    Rgb,
    /// Colour with alpha, e.g. a cutout leaf atlas (BC3, 8 bits per texel); mips keep the
    /// alpha-tested coverage of the full image.
    Rgba,
    /// A tangent-space normal map: X and Y kept (BC5, 8 bits per texel), Z rebuilt in the
    /// shader.
    Normal,
    /// One channel, the red one (BC4, 4 bits per texel).
    Gray,
}

/// A compressed image on its way to the GPU over several frames (`Renderer::upload_step`).
pub struct UploadJob {
    pub(crate) c: std::sync::Arc<Compressed>,
    pub(crate) repeat: bool,
    pub(crate) texture: Option<wgpu::Texture>,
    pub(crate) level: usize,
    pub(crate) row: u32,
}

impl UploadJob {
    pub fn new(c: std::sync::Arc<Compressed>, repeat: bool) -> Self {
        Self {
            c,
            repeat,
            texture: None,
            level: 0,
            row: 0,
        }
    }
}

/// A compressed image with its mip chain, level 0 first.
pub struct Compressed {
    pub width: u32,
    pub height: u32,
    pub kind: BcKind,
    pub color_space: ColorSpace,
    pub levels: Vec<Vec<u8>>,
}

impl Compressed {
    pub fn bytes(&self) -> u64 {
        self.levels.iter().map(|l| l.len() as u64).sum()
    }

    pub(crate) fn format(&self) -> wgpu::TextureFormat {
        use wgpu::TextureFormat as F;
        match (self.kind, self.color_space) {
            (BcKind::Rgb, ColorSpace::Srgb) => F::Bc1RgbaUnormSrgb,
            (BcKind::Rgb, ColorSpace::Linear) => F::Bc1RgbaUnorm,
            (BcKind::Rgba, ColorSpace::Srgb) => F::Bc3RgbaUnormSrgb,
            (BcKind::Rgba, ColorSpace::Linear) => F::Bc3RgbaUnorm,
            (BcKind::Normal, _) => F::Bc5RgUnorm,
            (BcKind::Gray, _) => F::Bc4RUnorm,
        }
    }

    pub(crate) fn block_bytes(kind: BcKind) -> usize {
        match kind {
            BcKind::Rgb | BcKind::Gray => 8,
            BcKind::Rgba | BcKind::Normal => 16,
        }
    }
}

/// Compresses an RGBA8 image (rows top to bottom) with a full mip chain. Both sides must be
/// multiples of 4 (`None` otherwise: keep it RGBA8). Mips average in linear light for sRGB
/// colour.
pub fn compress(
    rgba: &[u8],
    width: u32,
    height: u32,
    kind: BcKind,
    color_space: ColorSpace,
) -> Option<Compressed> {
    if !width.is_multiple_of(4)
        || !height.is_multiple_of(4)
        || rgba.len() != (width * height * 4) as usize
    {
        return None;
    }
    let srgb = color_space == ColorSpace::Srgb && matches!(kind, BcKind::Rgb | BcKind::Rgba);
    let coverage = (kind == BcKind::Rgba).then(|| alpha_coverage(rgba, 1.0));
    let mut levels = Vec::new();
    let (mut w, mut h) = (width, height);
    let mut img = rgba.to_vec();
    loop {
        levels.push(encode(&img, w, h, kind));
        if w == 1 && h == 1 {
            break;
        }
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        img = downsample(&img, w, h, nw, nh, srgb);
        if let Some(c) = coverage {
            keep_coverage(&mut img, c);
        }
        (w, h) = (nw, nh);
    }
    Some(Compressed {
        width,
        height,
        kind,
        color_space,
        levels,
    })
}

fn to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(l: f32) -> u8 {
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Box filter to the next level (odd sizes take the last row or column twice).
fn downsample(src: &[u8], w: u32, h: u32, nw: u32, nh: u32, srgb: bool) -> Vec<u8> {
    let (w, h, nw, nh) = (w as usize, h as usize, nw as usize, nh as usize);
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            for k in 0..4 {
                let mut sum = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((2 * x + dx).min(w - 1), (2 * y + dy).min(h - 1));
                    let v = src[(sy * w + sx) * 4 + k];
                    sum += if srgb && k < 3 {
                        to_linear(v)
                    } else {
                        f32::from(v) / 255.0
                    };
                }
                let m = sum / 4.0;
                out[(y * nw + x) * 4 + k] = if srgb && k < 3 {
                    to_srgb(m)
                } else {
                    (m.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                };
            }
        }
    }
    out
}

/// Fraction of texels whose alpha, scaled by `scale`, passes a 0.5 cutout.
fn alpha_coverage(rgba: &[u8], scale: f32) -> f32 {
    let n = rgba.len() / 4;
    let pass = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| f32::from(p[3]) * scale >= 127.5)
        .count();
    pass as f32 / n.max(1) as f32
}

/// Scales a mip's alpha so as many texels pass the cutout as in the full image: otherwise
/// foliage thins out with distance as averaging pulls alpha under the threshold.
fn keep_coverage(rgba: &mut [u8], target: f32) {
    let (mut lo, mut hi) = (0.25f32, 4.0f32);
    for _ in 0..12 {
        let mid = (lo + hi) / 2.0;
        if alpha_coverage(rgba, mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let s = (lo + hi) / 2.0;
    for p in rgba.as_chunks_mut::<4>().0 {
        p[3] = (f32::from(p[3]) * s).round().clamp(0.0, 255.0) as u8;
    }
}

/// One level, in 4x4 blocks (a level under 4 texels repeats its edge texels).
fn encode(img: &[u8], w: u32, h: u32, kind: BcKind) -> Vec<u8> {
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    let mut out = Vec::with_capacity(bw * bh * Compressed::block_bytes(kind));
    let (w, h) = (w as usize, h as usize);
    for by in 0..bh {
        for bx in 0..bw {
            let mut block = [[0u8; 4]; 16];
            for (i, t) in block.iter_mut().enumerate() {
                let (x, y) = ((bx * 4 + i % 4).min(w - 1), (by * 4 + i / 4).min(h - 1));
                t.copy_from_slice(&img[(y * w + x) * 4..(y * w + x) * 4 + 4]);
            }
            match kind {
                BcKind::Rgb => out.extend(bc1(&block)),
                BcKind::Rgba => {
                    out.extend(bc4(&block.map(|t| t[3])));
                    out.extend(bc1(&block));
                }
                BcKind::Normal => {
                    out.extend(bc4(&block.map(|t| t[0])));
                    out.extend(bc4(&block.map(|t| t[1])));
                }
                BcKind::Gray => out.extend(bc4(&block.map(|t| t[0]))),
            }
        }
    }
    out
}

fn pack565(c: [f32; 3]) -> u16 {
    let r = (c[0].clamp(0.0, 255.0) * 31.0 / 255.0 + 0.5) as u16;
    let g = (c[1].clamp(0.0, 255.0) * 63.0 / 255.0 + 0.5) as u16;
    let b = (c[2].clamp(0.0, 255.0) * 31.0 / 255.0 + 0.5) as u16;
    (r << 11) | (g << 5) | b
}

fn unpack565(c: u16) -> [f32; 3] {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    [
        f32::from(r) * 255.0 / 31.0,
        f32::from(g) * 255.0 / 63.0,
        f32::from(b) * 255.0 / 31.0,
    ]
}

/// BC1 in its four-colour mode (as BC3's colour half requires).
fn bc1(block: &[[u8; 4]; 16]) -> [u8; 8] {
    let px: Vec<[f32; 3]> = block
        .iter()
        .map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
        .collect();
    // Endpoints: the extremes of the texels projected on the block's main axis, inset by 1/16
    // of their range against outliers.
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &px {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let mean = px
        .iter()
        .fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
    let mean = mean.map(|v| v / 16.0);
    // The axis: the diagonal, with each channel's sign following its correlation with the
    // channel of largest range.
    let range = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let main = (0..3)
        .max_by(|&a, &b| range[a].total_cmp(&range[b]))
        .unwrap_or(1);
    let mut axis = range;
    for k in 0..3 {
        if k != main {
            let cov: f32 = px
                .iter()
                .map(|p| (p[k] - mean[k]) * (p[main] - mean[main]))
                .sum();
            if cov < 0.0 {
                axis[k] = -axis[k];
            }
        }
    }
    let len2 = axis.iter().map(|a| a * a).sum::<f32>();
    let (mut c0, mut c1) = if len2 < 1e-6 {
        (mean, mean)
    } else {
        let t = |p: &[f32; 3]| (0..3).map(|k| (p[k] - mean[k]) * axis[k]).sum::<f32>() / len2;
        let (tmin, tmax) = px
            .iter()
            .map(t)
            .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)));
        let inset = (tmax - tmin) / 16.0;
        let at = |s: f32| [0, 1, 2].map(|k| mean[k] + axis[k] * s);
        (at(tmax - inset), at(tmin + inset))
    };
    let (mut e0, mut e1) = (pack565(c0), pack565(c1));
    if e0 < e1 {
        std::mem::swap(&mut e0, &mut e1);
        std::mem::swap(&mut c0, &mut c1);
    }
    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&e0.to_le_bytes());
    out[2..4].copy_from_slice(&e1.to_le_bytes());
    if e0 == e1 {
        return out; // every index 0
    }
    let (p0, p1) = (unpack565(e0), unpack565(e1));
    let pal = [
        p0,
        p1,
        [0, 1, 2].map(|k| (2.0 * p0[k] + p1[k]) / 3.0),
        [0, 1, 2].map(|k| (p0[k] + 2.0 * p1[k]) / 3.0),
    ];
    let mut bits = 0u32;
    for (i, p) in px.iter().enumerate() {
        let best = (0..4)
            .min_by(|&a, &b| {
                let d = |c: &[f32; 3]| (0..3).map(|k| (c[k] - p[k]).powi(2)).sum::<f32>();
                d(&pal[a]).total_cmp(&d(&pal[b]))
            })
            .unwrap_or(0) as u32;
        bits |= best << (2 * i);
    }
    out[4..8].copy_from_slice(&bits.to_le_bytes());
    out
}

/// BC4 in its eight-value mode (a0 > a1), or all one value.
fn bc4(v: &[u8; 16]) -> [u8; 8] {
    let (lo, hi) = v
        .iter()
        .fold((255u8, 0u8), |(a, b), &x| (a.min(x), b.max(x)));
    let mut out = [0u8; 8];
    out[0] = hi;
    out[1] = lo;
    if hi == lo {
        return out;
    }
    let (a0, a1) = (f32::from(hi), f32::from(lo));
    let pal: [f32; 8] = std::array::from_fn(|i| match i {
        0 => a0,
        1 => a1,
        i => ((8 - i) as f32 * a0 + (i - 1) as f32 * a1) / 7.0,
    });
    let mut bits = 0u64;
    for (i, &x) in v.iter().enumerate() {
        let x = f32::from(x);
        let best = (0..8)
            .min_by(|&a, &b| (pal[a] - x).abs().total_cmp(&(pal[b] - x).abs()))
            .unwrap_or(0) as u64;
        bits |= best << (3 * i);
    }
    out[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_bc1(b: &[u8]) -> [[f32; 3]; 16] {
        let (e0, e1) = (
            u16::from_le_bytes([b[0], b[1]]),
            u16::from_le_bytes([b[2], b[3]]),
        );
        let (p0, p1) = (unpack565(e0), unpack565(e1));
        let pal = [
            p0,
            p1,
            [0, 1, 2].map(|k| (2.0 * p0[k] + p1[k]) / 3.0),
            [0, 1, 2].map(|k| (p0[k] + 2.0 * p1[k]) / 3.0),
        ];
        let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        std::array::from_fn(|i| pal[((bits >> (2 * i)) & 3) as usize])
    }

    fn decode_bc4(b: &[u8]) -> [f32; 16] {
        let (a0, a1) = (f32::from(b[0]), f32::from(b[1]));
        let mut raw = [0u8; 8];
        raw[..6].copy_from_slice(&b[2..8]);
        let bits = u64::from_le_bytes(raw);
        std::array::from_fn(|i| {
            let j = ((bits >> (3 * i)) & 7) as usize;
            match j {
                0 => a0,
                1 => a1,
                j if a0 > a1 => ((8 - j) as f32 * a0 + (j - 1) as f32 * a1) / 7.0,
                _ => a0,
            }
        })
    }

    #[test]
    fn blocks_decode_close_to_their_source_and_mips_keep_coverage() {
        // A colour ramp (what a block of a real picture mostly is) and a cutout alpha pattern:
        // BC1 within a few levels, BC4 within 1/14 of the range.
        let (w, h) = (16u32, 16u32);
        let img: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let t = (x * 15 + y * 4) as u8;
                [
                    t,
                    t / 2 + 40,
                    200 - t / 3,
                    if (x + y) % 3 == 0 { 0 } else { 255 },
                ]
            })
            .collect();
        let c = compress(&img, w, h, BcKind::Rgba, ColorSpace::Linear).unwrap();
        assert_eq!(c.levels.len(), 5);
        assert_eq!(c.levels[0].len(), 16 * 16);
        let first = &c.levels[0][..16];
        let alpha = decode_bc4(&first[..8]);
        let color = decode_bc1(&first[8..]);
        for i in 0..16 {
            let (x, y) = (i % 4, i / 4);
            let src = &img[(y * 16 + x) * 4..];
            assert!((alpha[i] - f32::from(src[3])).abs() < 20.0);
            for k in 0..3 {
                assert!((color[i][k] - f32::from(src[k])).abs() < 10.0, "{i} {k}");
            }
        }
        // Two thirds of the texels pass the cutout at every level, not fewer and fewer.
        let mut half = downsample(&img, 16, 16, 8, 8, false);
        keep_coverage(&mut half, alpha_coverage(&img, 1.0));
        assert!((alpha_coverage(&half, 1.0) - 2.0 / 3.0).abs() < 0.1);
        assert!(compress(&img[..4 * 6 * 6], 6, 6, BcKind::Rgb, ColorSpace::Srgb).is_none());
    }
}
