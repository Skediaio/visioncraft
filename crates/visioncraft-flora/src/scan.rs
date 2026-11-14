//! Scanned leaves and bark (CC0 photo scans, `assets/flora.json`) in place of the drawn ones.
//! A leaf set is a sheet of real leaves photographed flat, each cut out with its petiole base
//! (`pivot`), the direction to its tip (`axis`) and its length in sheet widths; the atlas
//! stamps them where it would draw a leaf, at the size and angle the drawing would have had,
//! so the species' leaf size and spray layout stay. A bark set is a tileable photo of a known
//! physical size, repeated round the trunk as often as the trunk's girth asks.

use crate::Image;
use std::sync::Arc;

/// One leaf on a scanned sheet, in sheet coordinates (0..1, origin top-left, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Cutout {
    /// x, y, width, height.
    pub rect: [f32; 4],
    /// Where the petiole meets the twig.
    pub pivot: [f32; 2],
    /// Unit direction from the pivot to the tip.
    pub axis: [f32; 2],
    /// Pivot to tip, in sheet widths.
    pub length: f32,
}

/// Premultiplied RGBA (sRGB-encoded, as the atlas canvas) and the tangent normal's x, y
/// (premultiplied, signed) at one mip level, eight bits each: a 1k sheet holds about 8 MB.
struct Level {
    w: usize,
    h: usize,
    px: Vec<[u8; 4]>,
    n: Vec<[i8; 2]>,
}

fn unit(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn signed(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

/// A leaf sheet ready to stamp: its mip chain (so small stamps do not alias) and cut-outs.
pub struct LeafScan {
    levels: Vec<Level>,
    pub leaves: Vec<Cutout>,
    /// Mean colour of the leaves (0..1, the sheet's encoding): stamps are brought to the drawn
    /// leaf's brightness, so a sheet photographed lighter or darker does not change the plant.
    pub mean: [f32; 3],
}

/// A tileable bark photo and its size on the tree.
pub struct BarkScan {
    pub color: Image,
    pub normal: Option<Image>,
    /// Width and height the photo covers, metres.
    pub size_m: [f32; 2],
}

/// The scans a species' textures are made from; `None` draws that part.
#[derive(Clone, Default)]
pub struct Scans {
    pub leaf: Option<Arc<LeafScan>>,
    pub bark: Option<Arc<BarkScan>>,
    /// The leaves keep the sheet's own colour (scans of the species' genus); otherwise they
    /// take the drawn colour, so a look-alike's sheet changes the shape and not the hue.
    pub leaf_hue: bool,
}

impl LeafScan {
    /// `color` (sRGB), `opacity` (its red channel) and an optional OpenGL-convention normal map,
    /// all the same size.
    pub fn new(
        color: &Image,
        opacity: &Image,
        normal: Option<&Image>,
        leaves: Vec<Cutout>,
    ) -> Option<LeafScan> {
        let (w, h) = (color.width as usize, color.height as usize);
        if leaves.is_empty()
            || w == 0
            || (opacity.width, opacity.height) != (color.width, color.height)
            || normal.is_some_and(|n| (n.width, n.height) != (color.width, color.height))
        {
            return None;
        }
        let mut px = Vec::with_capacity(w * h);
        let mut n = Vec::with_capacity(w * h);
        for i in 0..w * h {
            let a = f32::from(opacity.rgba[i * 4]) / 255.0;
            let c = |k: usize| unit(f32::from(color.rgba[i * 4 + k]) / 255.0 * a);
            px.push([c(0), c(1), c(2), opacity.rgba[i * 4]]);
            n.push(normal.map_or([0; 2], |m| {
                let v = |k: usize| signed((f32::from(m.rgba[i * 4 + k]) / 127.5 - 1.0) * a);
                [v(0), v(1)]
            }));
        }
        Some(LeafScan::with_mips(Level { w, h, px, n }, leaves))
    }

    fn with_mips(first: Level, leaves: Vec<Cutout>) -> LeafScan {
        let (mut sum, mut a) = ([0.0f64; 3], 0.0f64);
        for p in &first.px {
            for k in 0..3 {
                sum[k] += f64::from(p[k]);
            }
            a += f64::from(p[3]);
        }
        let mean = sum.map(|v| (v / a.max(1.0)) as f32);
        let mut levels = vec![first];
        while let Some(l) = levels.last()
            && l.w > 8
            && l.h > 8
        {
            let (w2, h2) = (l.w / 2, l.h / 2);
            let at = |x: usize, y: usize| y * l.w + x;
            let mut px = Vec::with_capacity(w2 * h2);
            let mut n = Vec::with_capacity(w2 * h2);
            for y in 0..h2 {
                for x in 0..w2 {
                    let q = [
                        at(2 * x, 2 * y),
                        at(2 * x + 1, 2 * y),
                        at(2 * x, 2 * y + 1),
                        at(2 * x + 1, 2 * y + 1),
                    ];
                    px.push(std::array::from_fn(|k| {
                        (q.iter().map(|&i| u32::from(l.px[i][k])).sum::<u32>() / 4) as u8
                    }));
                    n.push(std::array::from_fn(|k| {
                        (q.iter().map(|&i| i32::from(l.n[i][k])).sum::<i32>() / 4) as i8
                    }));
                }
            }
            levels.push(Level {
                w: w2,
                h: h2,
                px,
                n,
            });
        }
        LeafScan {
            levels,
            leaves,
            mean,
        }
    }

    /// Premultiplied colour and normal x, y at sheet point (`u`, `v`) for a footprint of
    /// `texels` sheet texels per canvas texel: bilinear in the mip level that fits.
    pub(crate) fn sample(&self, u: f32, v: f32, texels: f32) -> ([f32; 4], [f32; 2]) {
        let lod = (texels.max(1.0).log2().floor() as usize).min(self.levels.len() - 1);
        let l = &self.levels[lod];
        let (x, y) = (u * l.w as f32 - 0.5, v * l.h as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let mut c = [0.0; 4];
        let mut n = [0.0; 2];
        for (dx, dy, wt) in [
            (0, 0, (1.0 - fx) * (1.0 - fy)),
            (1, 0, fx * (1.0 - fy)),
            (0, 1, (1.0 - fx) * fy),
            (1, 1, fx * fy),
        ] {
            let (xi, yi) = (x0 as i64 + dx, y0 as i64 + dy);
            if xi < 0 || yi < 0 || xi >= l.w as i64 || yi >= l.h as i64 {
                continue;
            }
            let i = yi as usize * l.w + xi as usize;
            for k in 0..4 {
                c[k] += f32::from(l.px[i][k]) / 255.0 * wt;
            }
            for k in 0..2 {
                n[k] += f32::from(l.n[i][k]) / 127.0 * wt;
            }
        }
        (c, n)
    }

    /// Sheet width in texels at full size.
    pub(crate) fn width(&self) -> f32 {
        self.levels[0].w as f32
    }
}

impl Scans {
    /// The scans as bytes, to hand to another thread or a web worker (`unpack`).
    pub fn pack(&self) -> Vec<u8> {
        let mut b = Vec::new();
        let u32s = |b: &mut Vec<u8>, v: &[u32]| v.iter().for_each(|x| b.extend(x.to_le_bytes()));
        let f32s = |b: &mut Vec<u8>, v: &[f32]| v.iter().for_each(|x| b.extend(x.to_le_bytes()));
        let image = |b: &mut Vec<u8>, i: &Image| {
            u32s(b, &[i.width, i.height]);
            b.extend(&i.rgba);
        };
        match &self.leaf {
            Some(l) => {
                let z = &l.levels[0];
                u32s(&mut b, &[1, z.w as u32, z.h as u32, l.leaves.len() as u32]);
                for c in &l.leaves {
                    f32s(&mut b, &c.rect);
                    f32s(&mut b, &c.pivot);
                    f32s(&mut b, &c.axis);
                    f32s(&mut b, &[c.length]);
                }
                z.px.iter().for_each(|p| b.extend(p));
                z.n.iter().for_each(|n| b.extend(n.map(|v| v as u8)));
            }
            None => u32s(&mut b, &[0]),
        }
        u32s(&mut b, &[u32::from(self.leaf_hue)]);
        match &self.bark {
            Some(k) => {
                u32s(&mut b, &[1]);
                f32s(&mut b, &k.size_m);
                image(&mut b, &k.color);
                match &k.normal {
                    Some(n) => {
                        u32s(&mut b, &[1]);
                        image(&mut b, n);
                    }
                    None => u32s(&mut b, &[0]),
                }
            }
            None => u32s(&mut b, &[0]),
        }
        b
    }

    pub fn unpack(b: &[u8]) -> Option<Scans> {
        let mut at = 0usize;
        let mut take = |n: usize| -> Option<&[u8]> {
            let s = b.get(at..at + n)?;
            at += n;
            Some(s)
        };
        fn word(s: &[u8]) -> [u8; 4] {
            [s[0], s[1], s[2], s[3]]
        }
        macro_rules! u32_ {
            () => {
                u32::from_le_bytes(word(take(4)?)) as usize
            };
        }
        macro_rules! f32_ {
            () => {
                f32::from_le_bytes(word(take(4)?))
            };
        }
        let mut out = Scans::default();
        if u32_!() == 1 {
            let (w, h, count) = (u32_!(), u32_!(), u32_!());
            let mut leaves = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                leaves.push(Cutout {
                    rect: [f32_!(), f32_!(), f32_!(), f32_!()],
                    pivot: [f32_!(), f32_!()],
                    axis: [f32_!(), f32_!()],
                    length: f32_!(),
                });
            }
            let px = take(w.checked_mul(h)?.checked_mul(4)?)?
                .as_chunks::<4>()
                .0
                .to_vec();
            let n = take(w * h * 2)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| v.map(|x| x as i8))
                .collect();
            if leaves.is_empty() || w == 0 || h == 0 {
                return None;
            }
            out.leaf = Some(Arc::new(LeafScan::with_mips(Level { w, h, px, n }, leaves)));
        }
        out.leaf_hue = u32_!() == 1;
        if u32_!() == 1 {
            let size_m = [f32_!(), f32_!()];
            let (w, h) = (u32_!(), u32_!());
            let color = Image {
                width: w as u32,
                height: h as u32,
                rgba: take(w.checked_mul(h)?.checked_mul(4)?)?.to_vec(),
            };
            let normal = if u32_!() == 1 {
                let (w, h) = (u32_!(), u32_!());
                Some(Image {
                    width: w as u32,
                    height: h as u32,
                    rgba: take(w.checked_mul(h)?.checked_mul(4)?)?.to_vec(),
                })
            } else {
                None
            };
            out.bark = Some(Arc::new(BarkScan {
                color,
                normal,
                size_m,
            }));
        }
        Some(out)
    }

    pub fn is_empty(&self) -> bool {
        self.leaf.is_none() && self.bark.is_none()
    }
}

/// `src` (tileable) repeated `reps` times across and down an `n` x `n` image, area-averaged.
pub(crate) fn tile(src: &Image, n: usize, reps: [usize; 2]) -> Image {
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (tw, th) = (n / reps[0].max(1), n / reps[1].max(1));
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            // The source box this texel of one repeat covers.
            let (tx, ty) = (x % tw.max(1), y % th.max(1));
            let (x0, x1) = (
                tx * sw / tw.max(1),
                ((tx + 1) * sw / tw.max(1)).max(tx * sw / tw.max(1) + 1),
            );
            let (y0, y1) = (
                ty * sh / th.max(1),
                ((ty + 1) * sh / th.max(1)).max(ty * sh / th.max(1) + 1),
            );
            let mut sum = [0u32; 4];
            for sy in y0..y1.min(sh) {
                for sx in x0..x1.min(sw) {
                    for k in 0..4 {
                        sum[k] += u32::from(src.rgba[(sy * sw + sx) * 4 + k]);
                    }
                }
            }
            let count = ((y1.min(sh) - y0) * (x1.min(sw) - x0)).max(1) as u32;
            for k in 0..4 {
                rgba[(y * n + x) * 4 + k] = (sum[k] / count) as u8;
            }
        }
    }
    Image {
        width: n as u32,
        height: n as u32,
        rgba,
    }
}

/// How often a bark photo `size_m` repeats round a trunk of radius `r` (and as often along a
/// length of one girth, as the tube's UVs run): at least once.
pub(crate) fn bark_repeats(size_m: [f32; 2], r: f32) -> [usize; 2] {
    let girth = std::f32::consts::TAU * r.max(0.01);
    size_m.map(|s| ((girth / s.max(0.05)).round() as usize).clamp(1, 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Image {
        let mut rgba = Vec::new();
        for y in 0..w {
            for x in 0..w {
                rgba.extend(f(x, y));
            }
        }
        Image {
            width: w,
            height: w,
            rgba,
        }
    }

    #[test]
    fn a_stamp_samples_the_sheet_and_its_mips() {
        let color = image(64, |x, _| [if x < 32 { 255 } else { 0 }, 128, 0, 255]);
        let opacity = image(64, |x, _| [if x < 32 { 255 } else { 0 }, 0, 0, 255]);
        let cut = Cutout {
            rect: [0.0, 0.0, 0.5, 1.0],
            pivot: [0.25, 1.0],
            axis: [0.0, -1.0],
            length: 1.0,
        };
        let s = LeafScan::new(&color, &opacity, None, vec![cut]).unwrap();
        let (left, _) = s.sample(0.2, 0.5, 1.0);
        let (right, _) = s.sample(0.8, 0.5, 1.0);
        assert!(left[3] > 0.99 && (left[0] - 1.0).abs() < 1e-3, "{left:?}");
        assert!(right[3] < 1e-3, "{right:?}");
        // A coarse footprint reads a smaller level: still opaque on the left.
        assert!(s.sample(0.1, 0.5, 8.0).0[3] > 0.9);
        assert!(LeafScan::new(&color, &image(8, |_, _| [0; 4]), None, vec![cut]).is_none());
        // Through bytes and back (to a web worker): the same samples.
        let scans = Scans {
            leaf: Some(Arc::new(s)),
            bark: Some(Arc::new(BarkScan {
                color: color.clone(),
                normal: None,
                size_m: [1.5, 1.5],
            })),
            leaf_hue: true,
        };
        let back = Scans::unpack(&scans.pack()).unwrap();
        assert!(back.leaf_hue);
        let l = back.leaf.unwrap();
        assert_eq!(l.leaves, vec![cut]);
        assert_eq!(
            l.sample(0.2, 0.5, 1.0),
            scans.leaf.as_ref().unwrap().sample(0.2, 0.5, 1.0)
        );
        assert_eq!(back.bark.unwrap().color, color);
        assert!(Scans::unpack(&scans.pack()[..20]).is_none());
    }

    #[test]
    fn bark_repeats_follow_the_girth() {
        // A 0.3 m trunk is 1.9 m round: a 1 m photo goes round twice, a 2 m one once.
        assert_eq!(bark_repeats([1.0, 1.0], 0.3), [2, 2]);
        assert_eq!(bark_repeats([2.0, 2.0], 0.3), [1, 1]);
        let t = tile(&image(16, |x, _| [x as u8 * 16, 0, 0, 255]), 32, [2, 2]);
        assert_eq!(t.width, 32);
        // Each half repeats the photo: the first and seventeenth texels match.
        assert_eq!(t.rgba[0], t.rgba[16 * 4]);
    }
}
