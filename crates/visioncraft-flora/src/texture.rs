//! Procedural textures: a leaf/flower atlas per species and a tileable bark texture with a
//! normal map. Everything is rasterised from analytic silhouettes (standard botanical leaf
//! shapes) with coverage-based anti-aliasing.
//!
//! Atlas layout (4 x 4 cells of 256 px, origin top-left):
//! * row 0 (cells 0-3): the LOD0 card: a twig spray of leaves, needles or scales (branching,
//!   clipped, ground cover, climber), one leaf (rosettes), one grass blade (tussocks), a fan
//!   frond (palms).
//! * row 1 (cells 4-6): dense clumps for enlarged LOD1/LOD2 cards; cell 7 an opaque, tileable
//!   leaf mass for topiary cores.
//! * row 2 (cells 8-11): flowers.
//! * row 3: a dead frond (palms, cells 12-13); cells 14-15 a fan of grass blades (grasses).
//!
//! Each cell keeps a transparent gutter of [`CELL_MARGIN`] px, and the colour of every
//! transparent texel is bled from the nearest opaque one within its cell, so bilinear filtering
//! and mipmaps do not pull in black fringes.

use crate::math::{PI, Rng, TAU, atan2, cos, lerp, sin, smoothstep, tile_fbm, tile_noise};
use crate::{BarkPattern, FlowerShape, Form, Image, LeafShape, PlantParams, PlantTextures};

pub const ATLAS_SIZE: u32 = 1024;
pub const ATLAS_CELLS: u32 = 4;
pub const BARK_SIZE: u32 = 256;
pub(crate) const CELL_PX: u32 = ATLAS_SIZE / ATLAS_CELLS;
pub(crate) const CELL_MARGIN: u32 = 6;

type Rgb = [f32; 3];

struct Canvas {
    size: usize,
    px: Vec<[f32; 4]>,
    /// Clip rectangle of the current cell (x0, y0, x1, y1).
    clip: (i32, i32, i32, i32),
    /// Origin of the current cell.
    origin: (f32, f32),
}

impl Canvas {
    fn new(size: usize) -> Canvas {
        Canvas {
            size,
            px: vec![[0.0; 4]; size * size],
            clip: (0, 0, size as i32, size as i32),
            origin: (0.0, 0.0),
        }
    }

    fn select_cell(&mut self, cell: u32) {
        let cx = (cell % ATLAS_CELLS * CELL_PX) as i32;
        let cy = (cell / ATLAS_CELLS * CELL_PX) as i32;
        let m = (CELL_MARGIN / 2) as i32;
        self.clip = (
            cx + m,
            cy + m,
            cx + CELL_PX as i32 - m,
            cy + CELL_PX as i32 - m,
        );
        self.origin = (cx as f32, cy as f32);
    }

    /// Composite `col` with coverage `a` over pixel (x, y) in cell coordinates. The canvas holds
    /// premultiplied colour; [`finish_atlas`] converts back to straight alpha.
    fn blend(&mut self, x: i32, y: i32, col: Rgb, a: f32) {
        let (gx, gy) = (x + self.origin.0 as i32, y + self.origin.1 as i32);
        if a <= 0.0
            || gx < self.clip.0
            || gy < self.clip.1
            || gx >= self.clip.2
            || gy >= self.clip.3
        {
            return;
        }
        let p = &mut self.px[gy as usize * self.size + gx as usize];
        let a = a.min(1.0);
        for k in 0..3 {
            p[k] = p[k] * (1.0 - a) + col[k] * a;
        }
        p[3] = p[3] + a * (1.0 - p[3]);
    }

    /// Fill pixels in the box with `f(x, y) -> Option<(colour, coverage)>` (pixel centres).
    fn fill(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        mut f: impl FnMut(f32, f32) -> Option<(Rgb, f32)>,
    ) {
        let (lo_x, lo_y) = (
            self.clip.0 - self.origin.0 as i32,
            self.clip.1 - self.origin.1 as i32,
        );
        let (hi_x, hi_y) = (
            self.clip.2 - self.origin.0 as i32,
            self.clip.3 - self.origin.1 as i32,
        );
        let (xa, ya) = ((x0.floor() as i32).max(lo_x), (y0.floor() as i32).max(lo_y));
        let (xb, yb) = ((x1.ceil() as i32).min(hi_x), (y1.ceil() as i32).min(hi_y));
        for y in ya..yb {
            for x in xa..xb {
                if let Some((c, a)) = f(x as f32 + 0.5, y as f32 + 0.5) {
                    self.blend(x, y, c, a);
                }
            }
        }
    }

    /// Tapered line (capsule) from `a` to `b` with radii `r0`, `r1`.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), r0: f32, r1: f32, col: Rgb) {
        let r = r0.max(r1) + 1.0;
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l2 = (dx * dx + dy * dy).max(1e-6);
        self.fill(
            a.0.min(b.0) - r,
            a.1.min(b.1) - r,
            a.0.max(b.0) + r,
            a.1.max(b.1) + r,
            |x, y| {
                let t = (((x - a.0) * dx + (y - a.1) * dy) / l2).clamp(0.0, 1.0);
                let (px, py) = (a.0 + dx * t - x, a.1 + dy * t - y);
                let d = (px * px + py * py).sqrt();
                let rr = lerp(r0, r1, t);
                let cov = (rr - d + 0.5).clamp(0.0, 1.0);
                (cov > 0.0).then_some((col, cov))
            },
        );
    }

    fn disc(&mut self, c: (f32, f32), r: f32, col: Rgb) {
        self.fill(
            c.0 - r - 1.0,
            c.1 - r - 1.0,
            c.0 + r + 1.0,
            c.1 + r + 1.0,
            |x, y| {
                let d = ((x - c.0) * (x - c.0) + (y - c.1) * (y - c.1)).sqrt();
                let cov = (r - d + 0.5).clamp(0.0, 1.0);
                (cov > 0.0).then_some((col, cov))
            },
        );
    }
}

fn mul(c: Rgb, k: f32) -> Rgb {
    [
        (c[0] * k).clamp(0.0, 1.0),
        (c[1] * k).clamp(0.0, 1.0),
        (c[2] * k).clamp(0.0, 1.0),
    ]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

fn jitter(c: Rgb, var: f32, rng: &mut Rng) -> Rgb {
    let k = 1.0 + var * 0.35 * rng.sym();
    let hue = var * 0.12 * rng.sym();
    [
        (c[0] * k * (1.0 + hue)).clamp(0.0, 1.0),
        (c[1] * k).clamp(0.0, 1.0),
        (c[2] * k * (1.0 - hue)).clamp(0.0, 1.0),
    ]
}

// ---------------------------------------------------------------------------------------------
// Leaf silhouettes. Coordinates: u along the midrib (0 = base, 1 = tip), v across, both in units
// of the leaf length. Functions return an approximate signed distance (positive inside).

/// Half-width profile with the widest point at `u0`; `round` 0 gives straight (pointed) flanks,
/// 1 a circular arc (rounded base or tip).
fn prof(u: f32, u0: f32, base_round: f32, tip_round: f32) -> f32 {
    if u <= 0.0 || u >= 1.0 {
        return 0.0;
    }
    let arc = |s: f32| (s * (2.0 - s)).max(0.0).sqrt();
    if u < u0 {
        let s = u / u0;
        lerp(s, arc(s), base_round)
    } else {
        let s = (1.0 - u) / (1.0 - u0);
        lerp(s, arc(s), tip_round)
    }
}

fn teeth(u: f32, serration: f32) -> f32 {
    let f = (u * 34.0).rem_euclid(1.0);
    1.0 - serration * 0.09 * (1.0 - (2.0 * f - 1.0).abs())
}

/// Signed distance of a simple leaf (and lobed/palmate), in leaf lengths.
fn leaf_sd(shape: LeafShape, aspect: f32, serration: f32, u: f32, v: f32) -> f32 {
    let half = aspect * 0.5;
    let av = v.abs();
    let simple = |p: f32| p * half * teeth(u, serration) - av;
    match shape {
        LeafShape::Ovate => simple(prof(u, 0.38, 0.9, 0.35)),
        LeafShape::Elliptic => simple(prof(u, 0.5, 0.7, 0.55)),
        LeafShape::Lanceolate => simple(prof(u, 0.4, 0.35, 0.12)),
        LeafShape::Linear => {
            let p = if u <= 0.0 || u >= 1.0 {
                0.0
            } else if u < 0.04 {
                0.6 + 0.4 * u / 0.04
            } else if u < 0.72 {
                1.0
            } else {
                prof(u, 0.72, 1.0, 0.25)
            };
            simple(p)
        }
        LeafShape::Cordate => {
            // Broad ovate body plus two rounded basal lobes either side of a narrow sinus.
            let body = simple(prof(u, 0.36, 1.0, 0.5));
            let lobe_r = half * 0.6;
            let lobe = |sgn: f32| {
                lobe_r
                    - ((u - 0.12) * (u - 0.12) + (v - sgn * half * 0.36) * (v - sgn * half * 0.36))
                        .sqrt()
            };
            let notch = (av - (0.06 - u) * 1.2).min(1.0);
            body.max(lobe(1.0))
                .max(lobe(-1.0))
                .min(if u < 0.06 { notch } else { 1.0 })
        }
        LeafShape::Deltoid => simple(prof(u, 0.24, 0.55, 0.0)),
        LeafShape::Lobed { lobes } => {
            let n = lobes.max(2) as f32;
            let base = prof(u, 0.62, 0.6, 0.85);
            let s = sin(PI * n * (u - 0.06) / 0.94).abs().sqrt();
            let lobing = if u < 0.06 { 0.7 } else { 0.55 + 0.45 * s };
            let auricle = if u < 0.12 {
                0.25 * (1.0 - u / 0.12)
            } else {
                0.0
            };
            simple((base * lobing).max(auricle))
        }
        LeafShape::Palmate { lobes, depth } => {
            let (cu, r) = (0.47, ((u - 0.47) * (u - 0.47) + v * v).sqrt());
            let th = atan2(v, u - cu);
            let n = lobes.max(1) as i32;
            let spread = if n <= 3 { 0.95 } else { 2.15 / (n - 1) as f32 };
            let mut pk: f32 = 0.0;
            for k in 0..n {
                let tk = (k as f32 - (n - 1) as f32 * 0.5) * spread;
                let amp = 1.0 - 0.13 * (tk / spread).abs();
                let x = ((th - tk).abs() / (spread * 0.5)).min(1.0);
                pk = pk.max(amp * (1.0 - x));
            }
            let rr = 0.53 * ((1.0 - depth) + depth * pk) * teeth(th.abs() * 0.4, serration);
            rr - r
        }
        _ => simple(prof(u, 0.45, 0.6, 0.4)),
    }
}

#[derive(Clone, Copy)]
struct LeafStyle {
    shape: LeafShape,
    aspect: f32,
    serration: f32,
    veins: bool,
}

/// Rasterise one leaf with its base at `base` (cell pixels), pointing at angle `ang` (radians,
/// 0 = up in the image), `len` pixels long.
fn draw_leaf(cv: &mut Canvas, base: (f32, f32), ang: f32, len: f32, st: LeafStyle, col: Rgb) {
    if len < 1.5 {
        return;
    }
    let (dx, dy) = (sin(ang), -cos(ang));
    let half_w = match st.shape {
        LeafShape::Palmate { .. } | LeafShape::Cordate | LeafShape::Lobed { .. } => 0.62,
        _ => (st.aspect * 0.6).max(0.06),
    } * len
        + 1.5;
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (u, v) in [
        (-0.25 * len, -half_w),
        (-0.25 * len, half_w),
        (1.05 * len, -half_w),
        (1.05 * len, half_w),
    ] {
        let (x, y) = (base.0 + dx * u - dy * v, base.1 + dy * u + dx * v);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let midrib = mix(col, [0.85, 0.9, 0.55], 0.35);
    let (nv, slope) = (7.0, 1.0 / st.aspect.max(0.05));
    cv.fill(x0, y0, x1, y1, |x, y| {
        let (rx, ry) = (x - base.0, y - base.1);
        let u = (rx * dx + ry * dy) / len;
        let v = (rx * -dy + ry * dx) / len;
        if !(-0.25..=1.05).contains(&u) {
            return None;
        }
        let d = leaf_sd(st.shape, st.aspect, st.serration, u, v) * len;
        let cov = (d + 0.5).clamp(0.0, 1.0);
        if cov <= 0.0 {
            return None;
        }
        // Shading: darker base, lighter tip, dark rim, light midrib and secondary veins.
        let mut c = mul(col, 0.82 + 0.28 * u.clamp(0.0, 1.0));
        let rim = 1.0 - smoothstep(0.0, 2.5, d);
        c = mul(c, 1.0 - 0.18 * rim);
        if st.veins {
            if v.abs() * len < 0.6 + len * 0.012 && u > 0.0 {
                c = midrib;
            } else {
                let w = (u * nv - v.abs() * slope * 0.9).rem_euclid(1.0);
                if w < 0.07 && len > 24.0 {
                    c = mix(c, midrib, 0.4);
                }
            }
        }
        Some((c, cov))
    });
}

/// Draw one botanical "leaf unit" (simple leaf with petiole, compound leaf, frond).
fn draw_unit(
    cv: &mut Canvas,
    base: (f32, f32),
    ang: f32,
    len: f32,
    shape: LeafShape,
    aspect: f32,
    serration: f32,
    col: Rgb,
    twig: Rgb,
    rng: &mut Rng,
) {
    let dir = (sin(ang), -cos(ang));
    let at = |t: f32| (base.0 + dir.0 * len * t, base.1 + dir.1 * len * t);
    let st = |shape| LeafStyle {
        shape,
        aspect,
        serration,
        veins: true,
    };
    match shape {
        LeafShape::Pinnate { leaflets } => {
            let n = leaflets.max(3) as usize;
            let pairs = (n - 1) / 2;
            cv.line(base, at(0.95), (len * 0.012).max(0.6), 0.5, twig);
            let ll = len * (0.9 / (pairs as f32 + 1.0)).clamp(0.12, 0.42) * 1.25;
            for k in 0..pairs {
                let t = 0.18 + 0.72 * k as f32 / pairs.max(1) as f32;
                for s in [-1.0, 1.0] {
                    let a = ang + s * (1.05 - 0.2 * t) + 0.1 * rng.sym();
                    draw_leaf(
                        cv,
                        at(t),
                        a,
                        ll * (1.0 - 0.15 * t),
                        st(LeafShape::Elliptic),
                        jitter(col, 0.15, rng),
                    );
                }
            }
            draw_leaf(cv, at(0.82), ang, ll * 0.95, st(LeafShape::Elliptic), col);
        }
        LeafShape::FernFrond => {
            cv.line(base, at(0.98), (len * 0.01).max(0.7), 0.4, mul(col, 0.7));
            let n = 22;
            for k in 0..n {
                let t = 0.12 + 0.86 * k as f32 / n as f32;
                let pl = len * 0.24 * prof(t, 0.38, 0.85, 0.2).max(0.08);
                for s in [-1.0, 1.0] {
                    let pst = LeafStyle {
                        shape: LeafShape::Lanceolate,
                        aspect: 0.28,
                        serration: 1.0,
                        veins: false,
                    };
                    draw_leaf(cv, at(t), ang + s * 1.2, pl, pst, jitter(col, 0.1, rng));
                }
            }
        }
        LeafShape::FanFrond => {
            let c = at(0.42);
            cv.line(base, c, len * 0.02, len * 0.012, mul(col, 0.8));
            let n = 34;
            for k in 0..n {
                let a = ang + (k as f32 / (n - 1) as f32 - 0.5) * deg_r(250.0);
                let l = len * 0.56 * (0.82 + 0.18 * rng.f());
                let seg = LeafStyle {
                    shape: LeafShape::Linear,
                    aspect: 0.07,
                    serration: 0.0,
                    veins: true,
                };
                draw_leaf(cv, c, a, l, seg, jitter(col, 0.12, rng));
            }
        }
        LeafShape::FeatherFrond => {
            cv.line(base, at(0.98), len * 0.015, 0.5, mul(col, 0.75));
            for k in 0..30 {
                let t = 0.15 + 0.83 * k as f32 / 30.0;
                for s in [-1.0, 1.0] {
                    let seg = LeafStyle {
                        shape: LeafShape::Linear,
                        aspect: 0.06,
                        serration: 0.0,
                        veins: true,
                    };
                    draw_leaf(
                        cv,
                        at(t),
                        ang + s * 0.9,
                        len * 0.32 * (1.0 - 0.6 * t),
                        seg,
                        jitter(col, 0.1, rng),
                    );
                }
            }
        }
        LeafShape::Palmate { .. } => {
            let pet = len * 0.35;
            cv.line(base, at(0.3), 0.8, 0.6, twig);
            let b2 = (base.0 + dir.0 * pet * 0.6, base.1 + dir.1 * pet * 0.6);
            draw_leaf(cv, b2, ang, len, st(shape), col);
        }
        _ => {
            let pet = len * 0.12;
            cv.line(base, at(0.12), 0.7, 0.6, twig);
            draw_leaf(
                cv,
                (base.0 + dir.0 * pet, base.1 + dir.1 * pet),
                ang,
                len,
                st(shape),
                col,
            );
        }
    }
}

fn deg_r(d: f32) -> f32 {
    d * PI / 180.0
}

/// Needles along a twig segment.
fn needles(
    cv: &mut Canvas,
    a: (f32, f32),
    b: (f32, f32),
    nl: f32,
    bundle: u8,
    col: Rgb,
    rng: &mut Rng,
    flat: bool,
) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let ang = atan2(dx, -dy);
    let step = if bundle >= 2 { 2.2 } else { 1.6 };
    let n = (len / step) as usize;
    for i in 0..n {
        let t = (i as f32 + rng.f()) / n.max(1) as f32;
        let p = (a.0 + dx * t, a.1 + dy * t);
        let k = bundle.max(1);
        for j in 0..k {
            let side = if (i + j as usize).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            let spread = if flat {
                deg_r(80.0)
            } else {
                deg_r(25.0 + 60.0 * rng.f())
            };
            let na = ang + side * spread + 0.15 * rng.sym();
            let l = nl * (0.8 + 0.3 * rng.f()) * if flat { 1.0 } else { 0.6 + 0.4 * rng.f() };
            let e = (p.0 + sin(na) * l, p.1 - cos(na) * l);
            let c = if flat && rng.f() < 0.3 {
                mix(col, [0.75, 0.85, 0.8], 0.35)
            } else {
                jitter(col, 0.25, rng)
            };
            cv.line(p, e, 0.9, 0.5, c);
        }
    }
}

/// A flattened scale-leaf spray (thuja, cypress): a frond branching twice, drawn as thick lines.
fn scale_spray(
    cv: &mut Canvas,
    a: (f32, f32),
    ang: f32,
    len: f32,
    width: f32,
    col: Rgb,
    rng: &mut Rng,
    depth: u32,
) {
    let b = (a.0 + sin(ang) * len, a.1 - cos(ang) * len);
    cv.line(a, b, width, width * 0.55, jitter(col, 0.15, rng));
    if depth == 0 || len < 6.0 {
        return;
    }
    let n = 5;
    for k in 0..n {
        let t = 0.2 + 0.75 * k as f32 / n as f32;
        let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        let s = if k % 2 == 0 { 1.0 } else { -1.0 };
        scale_spray(
            cv,
            p,
            ang + s * deg_r(48.0),
            len * 0.42 * (1.0 - 0.5 * t),
            width * 0.8,
            mix(col, [0.7, 0.85, 0.45], 0.12 * t),
            rng,
            depth - 1,
        );
    }
}

/// Foliage of one twig: leaves (simple or compound), needles or nothing, alternating along it.
fn twig(
    cv: &mut Canvas,
    p: &PlantParams,
    a: (f32, f32),
    ang: f32,
    len: f32,
    depth: u32,
    leaf_px: f32,
    col: Rgb,
    rng: &mut Rng,
) {
    let lf = &p.leaves;
    let twig_col = mix(p.bark.colour, [0.35, 0.3, 0.2], 0.5);
    let dir = (sin(ang), -cos(ang));
    let b = (a.0 + dir.0 * len, a.1 + dir.1 * len);
    cv.line(a, b, 0.6 + 0.5 * depth as f32, 0.5, twig_col);
    // Side twigs first so this twig's leaves lie on top of them.
    if depth > 0 {
        let n = 4;
        for k in 0..n {
            let t = 0.12 + 0.72 * (k as f32 + 0.3 * rng.f()) / n as f32;
            let s = if k % 2 == 0 { 1.0 } else { -1.0 };
            let q = (a.0 + dir.0 * len * t, a.1 + dir.1 * len * t);
            twig(
                cv,
                p,
                q,
                ang + s * deg_r(36.0 + 14.0 * rng.f()),
                len * (0.62 - 0.32 * t),
                depth - 1,
                leaf_px,
                jitter(col, 0.08, rng),
                rng,
            );
        }
    }
    match lf.shape {
        LeafShape::Needles { length_m, bundle } => {
            let nl = leaf_px * length_m / lf.leaf_m.max(1e-3);
            needles(cv, a, b, nl, bundle, col, rng, bundle == 0);
        }
        _ => {
            let spacing = (leaf_px * lf.aspect.clamp(0.25, 0.8) * 0.7).max(2.5);
            let n = ((len / spacing) as usize).clamp(1, 40);
            for j in 0..n {
                let t = 0.1 + 0.9 * (j as f32 + 0.5) / n as f32;
                let q = (a.0 + dir.0 * len * t, a.1 + dir.1 * len * t);
                let s = if j % 2 == 0 { 1.0 } else { -1.0 };
                let la = ang + s * deg_r(35.0 + 25.0 * rng.f());
                let l = leaf_px * (0.8 + 0.35 * rng.f());
                draw_unit(
                    cv,
                    q,
                    la,
                    l,
                    lf.shape,
                    lf.aspect,
                    lf.serration,
                    jitter(col, lf.colour_var, rng),
                    twig_col,
                    rng,
                );
            }
            draw_unit(
                cv,
                b,
                ang + 0.2 * rng.sym(),
                leaf_px * 0.9,
                lf.shape,
                lf.aspect,
                lf.serration,
                jitter(col, lf.colour_var, rng),
                twig_col,
                rng,
            );
        }
    }
}

/// A twig spray filling a card: a twig branching `depth` times with foliage along every twig.
/// `px_per_m` maps metres to cell pixels.
fn spray(
    cv: &mut Canvas,
    p: &PlantParams,
    centre: (f32, f32),
    ang: f32,
    len: f32,
    px_per_m: f32,
    rng: &mut Rng,
    cell_tint: f32,
) {
    let lf = &p.leaves;
    let col = mul(lf.colour, cell_tint);
    let leaf_px = lf.leaf_m * px_per_m;
    let base = (
        centre.0 - sin(ang) * len * 0.5,
        centre.1 + cos(ang) * len * 0.5,
    );
    if matches!(lf.shape, LeafShape::Scale) {
        scale_spray(cv, base, ang, len, (len * 0.035).max(2.0), col, rng, 2);
        return;
    }
    // Branch more when a card holds many leaves (big tree cards), less for big leaves.
    let ratio = len / leaf_px.max(1.0);
    let depth = if ratio > 10.0 {
        2
    } else if ratio > 3.5 {
        1
    } else {
        0
    };
    twig(cv, p, base, ang, len, depth, leaf_px, col, rng);
}

/// A dense, roughly round clump of foliage for the enlarged LOD1/LOD2 cards: short twigs radiating
/// from and covering the centre, brightness matched to the sprays.
fn clump(cv: &mut Canvas, p: &PlantParams, rng: &mut Rng, tint: f32) {
    let c = (CELL_PX as f32 * 0.5, CELL_PX as f32 * 0.5);
    let r = CELL_PX as f32 * 0.5 - CELL_MARGIN as f32 - 2.0;
    // A clump card stands for an enlarged card, about 2.5 LOD0 cards across.
    let px_per_m = (2.0 * r) / (p.leaves.card_m * 2.5);
    let leaf_px = (p.leaves.leaf_m * px_per_m).max(3.0);
    let col = mul(p.leaves.colour, tint);
    let n = 34;
    for k in 0..n {
        let f = (k as f32 + rng.f()) / n as f32;
        let a = rng.f() * TAU;
        // Twigs start near the centre and point outwards, so the centre is covered too.
        let rad = r * 0.3 * (1.0 - f).sqrt();
        let q = (c.0 + sin(a) * rad, c.1 - cos(a) * rad);
        // Later (inner, top) twigs are lighter, outer ones slightly darker: a lit dome.
        let shade = 0.9 + 0.2 * f + 0.08 * (-cos(a));
        let len = r * (0.45 + 0.25 * rng.f());
        let depth = if len / leaf_px > 5.0 { 1 } else { 0 };
        twig(
            cv,
            p,
            q,
            a + 0.5 * rng.sym(),
            len,
            depth,
            leaf_px,
            mul(col, shade),
            rng,
        );
    }
}

/// Opaque tileable leaf mass (topiary core): drawn with wrap-around into an S x S tile.
fn mass_tile(p: &PlantParams, rng: &mut Rng) -> Vec<[f32; 4]> {
    let s = (CELL_PX - 2 * CELL_MARGIN) as usize;
    let base = mul(p.leaves.colour, 0.42);
    let mut cv = Canvas::new(s * 3);
    cv.clip = (0, 0, (s * 3) as i32, (s * 3) as i32);
    // Draw into the centre tile of a 3x3 canvas and fold everything back modulo the tile.
    let leaf_px = (p.leaves.leaf_m * (s as f32 / (p.leaves.card_m * 1.2))).clamp(6.0, 40.0);
    let lst = |shape| LeafStyle {
        shape,
        aspect: p.leaves.aspect.max(0.3),
        serration: p.leaves.serration,
        veins: leaf_px > 14.0,
    };
    let simple = match p.leaves.shape {
        LeafShape::Needles { .. } | LeafShape::Scale => LeafShape::Linear,
        LeafShape::Pinnate { .. }
        | LeafShape::FernFrond
        | LeafShape::FanFrond
        | LeafShape::FeatherFrond => LeafShape::Elliptic,
        other => other,
    };
    let narrow = matches!(simple, LeafShape::Linear);
    let n = ((s * s) as f32 / (leaf_px * leaf_px * if narrow { 0.08 } else { 0.25 })).min(4000.0)
        as usize;
    for _ in 0..n {
        let q = (s as f32 * (1.0 + rng.f()), s as f32 * (1.0 + rng.f()));
        let a = rng.f() * TAU;
        let col = jitter(
            mul(p.leaves.colour, 0.7 + 0.45 * rng.f()),
            p.leaves.colour_var,
            rng,
        );
        let st = if narrow {
            LeafStyle {
                aspect: 0.12,
                ..lst(simple)
            }
        } else {
            lst(simple)
        };
        draw_leaf(&mut cv, q, a, leaf_px * (0.8 + 0.4 * rng.f()), st, col);
    }
    let mut out = vec![[base[0], base[1], base[2], 1.0]; s * s];
    for y in 0..s * 3 {
        for x in 0..s * 3 {
            let src = cv.px[y * s * 3 + x];
            if src[3] <= 0.0 {
                continue;
            }
            let d = &mut out[(y % s) * s + (x % s)];
            for k in 0..3 {
                d[k] = d[k] * (1.0 - src[3].min(1.0)) + src[k];
            }
        }
    }
    out
}

fn draw_flower(cv: &mut Canvas, p: &PlantParams, cell_var: u32, rng: &mut Rng) {
    let Some(f) = &p.flowers else { return };
    let c = (CELL_PX as f32 * 0.5, CELL_PX as f32 * 0.5);
    let full = CELL_PX as f32 * 0.5 - CELL_MARGIN as f32 - 1.0;
    let col = jitter(f.colour, 0.12, rng);
    let petal = |shape, aspect| LeafStyle {
        shape,
        aspect,
        serration: 0.0,
        veins: false,
    };
    let leafc = p.leaves.colour;
    let twig = mix(p.bark.colour, [0.3, 0.25, 0.2], 0.4);
    match f.shape {
        FlowerShape::Daisy { droop } => {
            let n = 14 + (cell_var as usize % 3);
            for k in 0..n {
                let a = TAU * k as f32 / n as f32 + 0.1 * rng.sym();
                let l =
                    full * (0.85 + 0.1 * rng.f()) * if droop && cos(a) > 0.2 { 0.75 } else { 1.0 };
                draw_leaf(
                    cv,
                    c,
                    a,
                    l,
                    petal(LeafShape::Elliptic, 0.22),
                    jitter(col, 0.1, rng),
                );
            }
            cv.disc(c, full * 0.3, f.centre);
            cv.disc(
                (c.0 - full * 0.08, c.1 - full * 0.08),
                full * 0.12,
                mul(f.centre, 1.3),
            );
        }
        FlowerShape::Blossom => {
            cv.line(
                (c.0 - full * 0.9, c.1 + full * 0.5),
                (c.0 + full * 0.8, c.1 - full * 0.6),
                2.0,
                1.0,
                twig,
            );
            for _ in 0..34 {
                let q = (c.0 + full * 0.85 * rng.sym(), c.1 + full * 0.85 * rng.sym());
                if (q.0 - c.0) * (q.0 - c.0) + (q.1 - c.1) * (q.1 - c.1) > full * full * 0.8 {
                    continue;
                }
                let r = full * (0.13 + 0.06 * rng.f());
                let rot = rng.f() * TAU;
                let pc = jitter(col, 0.1, rng);
                for k in 0..5 {
                    draw_leaf(
                        cv,
                        q,
                        rot + TAU * k as f32 / 5.0,
                        r,
                        petal(LeafShape::Ovate, 0.9),
                        mix(pc, [1.0, 1.0, 1.0], 0.15 * (k % 2) as f32),
                    );
                }
                cv.disc(q, r * 0.22, f.centre);
            }
        }
        FlowerShape::Mophead => {
            for i in 0..260 {
                let rr = full * 0.92 * (rng.f()).sqrt();
                let a = rng.f() * TAU;
                let q = (c.0 + sin(a) * rr, c.1 - cos(a) * rr);
                let shade =
                    0.75 + 0.35 * (1.0 - (q.1 - c.1 + full) / (2.0 * full)) + 0.03 * (i % 3) as f32;
                let pc = mul(jitter(col, 0.12, rng), shade);
                let r = full * 0.1;
                let rot = rng.f() * TAU;
                for k in 0..4 {
                    draw_leaf(
                        cv,
                        q,
                        rot + TAU * k as f32 / 4.0,
                        r,
                        petal(LeafShape::Ovate, 0.95),
                        pc,
                    );
                }
            }
        }
        FlowerShape::Rose => {
            for &(ox, oy, s) in &[(0.0, 0.0, 0.62), (-0.5, 0.45, 0.4), (0.52, 0.38, 0.38)] {
                let q = (c.0 + ox * full, c.1 + oy * full);
                let r = full * s;
                for ring in 0..4 {
                    let rr = r * (1.0 - 0.22 * ring as f32);
                    let n = 5 + ring;
                    let rot = rng.f() * TAU;
                    for k in 0..n {
                        let pc = mul(col, 0.8 + 0.08 * ring as f32 + 0.05 * rng.f());
                        draw_leaf(
                            cv,
                            q,
                            rot + TAU * k as f32 / n as f32,
                            rr,
                            petal(LeafShape::Ovate, 1.0),
                            pc,
                        );
                    }
                }
                cv.disc(q, r * 0.15, mul(col, 0.7));
            }
        }
        FlowerShape::Lily => {
            for k in 0..6 {
                let a = TAU * k as f32 / 6.0 + 0.5 * (k % 2) as f32 * 0.1;
                let l = full * if k % 2 == 0 { 0.98 } else { 0.85 };
                draw_leaf(
                    cv,
                    c,
                    a,
                    l,
                    LeafStyle {
                        shape: LeafShape::Lanceolate,
                        aspect: 0.32,
                        serration: 0.0,
                        veins: true,
                    },
                    jitter(col, 0.06, rng),
                );
            }
            cv.disc(c, full * 0.14, f.centre);
            for k in 0..6 {
                let a = TAU * k as f32 / 6.0 + 0.3;
                cv.line(
                    c,
                    (c.0 + sin(a) * full * 0.5, c.1 - cos(a) * full * 0.5),
                    0.9,
                    0.8,
                    mul(f.centre, 1.2),
                );
            }
        }
        FlowerShape::Goblet => {
            let base = (c.0, c.1 + full * 0.8);
            cv.line((c.0, c.1 + full), base, 3.0, 2.5, twig);
            for k in 0..6 {
                let a = (k as f32 / 5.0 - 0.5) * 1.1;
                let pc = mix(col, f.centre, 0.25 * (k % 2) as f32);
                draw_leaf(
                    cv,
                    base,
                    a,
                    full * 1.65,
                    LeafStyle {
                        shape: LeafShape::Ovate,
                        aspect: 0.5,
                        serration: 0.0,
                        veins: false,
                    },
                    pc,
                );
            }
        }
        FlowerShape::Truss => {
            for k in 0..7 {
                let a = TAU * k as f32 / 7.0;
                draw_leaf(
                    cv,
                    c,
                    a,
                    full * 0.98,
                    LeafStyle {
                        shape: LeafShape::Elliptic,
                        aspect: 0.32,
                        serration: 0.0,
                        veins: true,
                    },
                    mul(leafc, 0.8),
                );
            }
            for _ in 0..16 {
                let rr = full * 0.5 * rng.f().sqrt();
                let a = rng.f() * TAU;
                let q = (c.0 + sin(a) * rr, c.1 - cos(a) * rr);
                let r = full * 0.23;
                let pc = jitter(col, 0.08, rng);
                let rot = rng.f() * TAU;
                for k in 0..5 {
                    draw_leaf(
                        cv,
                        q,
                        rot + TAU * k as f32 / 5.0,
                        r,
                        petal(LeafShape::Ovate, 1.0),
                        pc,
                    );
                }
                cv.disc(q, r * 0.2, f.centre);
            }
        }
        FlowerShape::Spike | FlowerShape::Panicle => {
            let panicle = matches!(f.shape, FlowerShape::Panicle);
            let (bot, top) = (c.1 + full, c.1 - full);
            let bend = if panicle { full * 0.35 } else { full * 0.05 };
            cv.line(
                (c.0, bot),
                (c.0 + bend, top),
                1.6,
                1.0,
                mix(f.centre, leafc, 0.5),
            );
            let n = if panicle { 90 } else { 26 };
            for i in 0..n {
                let t = i as f32 / n as f32;
                let y = lerp(bot, top, t);
                let x = c.0 + bend * t * t;
                let wdt = if panicle {
                    full * 0.42 * (1.0 - t * 0.85)
                } else {
                    full * 0.16 * (1.0 - t * 0.5)
                };
                for _ in 0..if panicle { 3 } else { 4 } {
                    let q = (x + wdt * rng.sym(), y + 2.0 * rng.sym());
                    let r = if panicle { full * 0.07 } else { full * 0.075 };
                    let pc = mul(jitter(col, 0.1, rng), 0.8 + 0.3 * rng.f());
                    for k in 0..4 {
                        draw_leaf(
                            cv,
                            q,
                            TAU * k as f32 / 4.0 + rng.f(),
                            r,
                            petal(LeafShape::Ovate, 0.9),
                            pc,
                        );
                    }
                    if panicle && rng.f() < 0.5 {
                        cv.disc(q, 1.0, f.centre);
                    }
                }
            }
        }
        FlowerShape::Saucer => {
            for k in 0..5 {
                let a = TAU * k as f32 / 5.0;
                draw_leaf(
                    cv,
                    c,
                    a,
                    full * 0.98,
                    LeafStyle {
                        shape: LeafShape::Ovate,
                        aspect: 0.95,
                        serration: 0.0,
                        veins: true,
                    },
                    jitter(col, 0.06, rng),
                );
            }
            cv.disc(c, full * 0.15, f.centre);
        }
        FlowerShape::Plume | FlowerShape::Bottlebrush => {
            plume(cv, p, (c.0, c.1 + full), (c.0, c.1 - full), rng, f.shape);
        }
    }
}

/// Grass plume or bottlebrush between `bot` and `top`.
fn plume(
    cv: &mut Canvas,
    p: &PlantParams,
    bot: (f32, f32),
    top: (f32, f32),
    rng: &mut Rng,
    shape: FlowerShape,
) {
    let Some(f) = &p.flowers else { return };
    // The card is 1.2x the plume's width, so bristles may reach ~40 % of the cell width.
    let wide = CELL_PX as f32
        * if matches!(shape, FlowerShape::Bottlebrush) {
            0.36
        } else {
            0.4
        };
    let n = if matches!(shape, FlowerShape::Bottlebrush) {
        220
    } else {
        160
    };
    for i in 0..n {
        let t = rng.f();
        let y = lerp(bot.1, top.1, t);
        let x = lerp(bot.0, top.0, t);
        let env = if matches!(shape, FlowerShape::Bottlebrush) {
            0.55 + 0.45 * prof(t, 0.5, 1.0, 1.0)
        } else {
            prof(t, 0.35, 0.7, 0.4)
        };
        let l = wide.min((bot.1 - top.1) * 0.45) * env * (0.6 + 0.4 * rng.f());
        let s = if i % 2 == 0 { 1.0 } else { -1.0 };
        let a = s * deg_r(if matches!(shape, FlowerShape::Bottlebrush) {
            75.0 + 15.0 * rng.f()
        } else {
            15.0 + 20.0 * rng.f()
        });
        let e = (x + sin(a) * l, y - cos(a) * l);
        let c = mix(jitter(f.colour, 0.15, rng), f.centre, 0.3 * rng.f());
        cv.line((x, y), e, 0.9, 0.5, c);
    }
}

/// A fan of grass blades from the bottom centre (far grass cards).
fn blade_fan(cv: &mut Canvas, p: &PlantParams, rng: &mut Rng, droop: f32) {
    let base = (
        CELL_PX as f32 * 0.5,
        CELL_PX as f32 - CELL_MARGIN as f32 - 1.0,
    );
    let lpx = CELL_PX as f32 - 2.0 * CELL_MARGIN as f32 - 4.0;
    for _ in 0..90 {
        let a0 = deg_r(70.0) * rng.sym();
        let l = lpx * (0.6 + 0.4 * rng.f()) * (1.0 - 0.25 * (a0 / deg_r(70.0)).abs());
        let segs = 6;
        let mut q = base;
        let mut a = a0 * 0.6;
        let col = jitter(p.leaves.colour, p.leaves.colour_var, rng);
        for k in 0..segs {
            let t = k as f32 / segs as f32;
            let e = (
                q.0 + sin(a) * l / segs as f32,
                q.1 - cos(a) * l / segs as f32,
            );
            let w = 2.2 * (1.0 - t) + 0.4;
            cv.line(q, e, w, w * 0.85, mul(col, 0.8 + 0.3 * t));
            q = e;
            a += a0.signum() * droop * 0.25;
        }
    }
}

/// One grass blade or strap leaf filling the whole cell: base at the bottom, tip at the top,
/// full width (the mesh strip has the blade's width).
fn blade_cell(cv: &mut Canvas, col: Rgb) {
    let cpx = CELL_PX as f32;
    let inner = cpx - 2.0 * CELL_MARGIN as f32;
    let base = (cpx * 0.5, cpx - CELL_MARGIN as f32 - 1.0);
    cv.fill(0.0, 0.0, cpx, cpx, |x, y| {
        let u = (base.1 - y) / (inner - 2.0);
        let half = inner * 0.5 - 1.0;
        let v = (x - base.0) / half;
        // Linear blade: parallel sides, acute tip over the last quarter.
        let w = if u < 0.0 || u > 1.0 {
            0.0
        } else if u < 0.75 {
            1.0
        } else {
            (1.0 - u) / 0.25
        };
        let d = (w - v.abs()) * half;
        let cov = (d + 0.5).clamp(0.0, 1.0);
        if cov <= 0.0 {
            return None;
        }
        let mut c = mul(col, 0.7 + 0.45 * u.clamp(0.0, 1.0));
        if v.abs() < 0.06 {
            c = mix(c, [0.85, 0.9, 0.6], 0.25);
        } else if (v.abs() * 6.0).rem_euclid(1.0) < 0.12 {
            c = mul(c, 0.93);
        }
        Some((c, cov))
    });
}

pub fn generate(p: &PlantParams) -> PlantTextures {
    // Seeded from the leaf parameters so each species gets its own, repeatable variation.
    let mut rng = Rng::new(
        0x7e57 ^ ((p.leaves.leaf_m.to_bits() as u64) << 7) ^ p.leaves.colour[1].to_bits() as u64,
        3,
    );
    let mut cv = Canvas::new(ATLAS_SIZE as usize);
    let cpx = CELL_PX as f32;
    let inner = cpx - 2.0 * CELL_MARGIN as f32;
    let lf = &p.leaves;
    // Row 0: primary cards.
    for cell in 0..4u32 {
        cv.select_cell(cell);
        let tint = 1.0 + lf.colour_var * 0.25 * (cell as f32 - 1.5) / 1.5;
        let col = mul(lf.colour, tint);
        match &p.form {
            Form::Rosette(r) if !r.shoots && matches!(lf.shape, LeafShape::Linear) => {
                blade_cell(&mut cv, col)
            }
            Form::Rosette(r) if !r.shoots => {
                // One leaf (or frond) from the bottom centre to the top edge.
                let base = (cpx * 0.5, cpx - CELL_MARGIN as f32 - 1.0);
                let len = inner - 2.0;
                let st = LeafStyle {
                    shape: lf.shape,
                    aspect: lf.aspect,
                    serration: lf.serration,
                    veins: true,
                };
                match lf.shape {
                    LeafShape::FernFrond | LeafShape::Pinnate { .. } => draw_unit(
                        &mut cv,
                        base,
                        0.0,
                        len,
                        lf.shape,
                        lf.aspect,
                        lf.serration,
                        col,
                        col,
                        &mut rng,
                    ),
                    LeafShape::Linear => draw_leaf(&mut cv, base, 0.0, len, st, col),
                    _ => {
                        // Leaf blade on a petiole taking the lower third.
                        cv.line(base, (base.0, base.1 - len * 0.3), 2.5, 2.0, mul(col, 0.9));
                        let blade = len * 0.72;
                        let w = blade * lf.aspect;
                        let b = if w > inner { blade * inner / w } else { blade };
                        draw_leaf(&mut cv, (base.0, base.1 - len * 0.28), 0.0, b, st, col);
                    }
                }
            }
            Form::Tussock(_) => blade_cell(&mut cv, col),
            Form::Palm(_) => {
                let base = (cpx * 0.5, cpx - CELL_MARGIN as f32 - 1.0);
                draw_unit(
                    &mut cv,
                    base,
                    0.0,
                    inner * 1.05,
                    lf.shape,
                    lf.aspect,
                    0.0,
                    col,
                    col,
                    &mut rng,
                );
            }
            _ => {
                let px_per_m = inner / lf.card_m;
                let ang = 0.12 * rng.sym();
                spray(
                    &mut cv,
                    p,
                    (cpx * 0.5, cpx * 0.5),
                    ang,
                    inner * 0.92,
                    px_per_m,
                    &mut rng,
                    tint,
                );
            }
        }
    }
    // Row 1: clumps and the opaque mass tile.
    for cell in 4..7u32 {
        cv.select_cell(cell);
        let tint = 1.0 + lf.colour_var * 0.2 * (cell as f32 - 5.0);
        match &p.form {
            Form::Tussock(t) => blade_fan(&mut cv, p, &mut rng, t.droop),
            _ => clump(&mut cv, p, &mut rng, tint),
        }
    }
    {
        let tile = mass_tile(p, &mut rng);
        let s = (CELL_PX - 2 * CELL_MARGIN) as i32;
        let (ox, oy) = ((3 * CELL_PX) as usize, CELL_PX as usize);
        for y in 0..CELL_PX as i32 {
            for x in 0..CELL_PX as i32 {
                let t = tile[((y - CELL_MARGIN as i32).rem_euclid(s) * s
                    + (x - CELL_MARGIN as i32).rem_euclid(s))
                    as usize];
                cv.px[(oy + y as usize) * ATLAS_SIZE as usize + ox + x as usize] = t;
            }
        }
    }
    // Row 2: flowers.
    for cell in 8..12u32 {
        cv.select_cell(cell);
        draw_flower(&mut cv, p, cell, &mut rng);
    }
    // Row 3: plumes on stalks / dead frond, blade fans.
    for cell in 12..16u32 {
        cv.select_cell(cell);
        match &p.form {
            Form::Palm(_) => {
                let base = (cpx * 0.5, cpx - CELL_MARGIN as f32 - 1.0);
                let dead = mix(p.bark.colour, [0.55, 0.42, 0.25], 0.6);
                draw_unit(
                    &mut cv,
                    base,
                    0.0,
                    inner * 1.05,
                    lf.shape,
                    lf.aspect,
                    0.0,
                    dead,
                    dead,
                    &mut rng,
                );
            }
            Form::Tussock(t) if cell >= 14 => blade_fan(&mut cv, p, &mut rng, t.droop),
            _ => {}
        }
    }
    let atlas = finish_atlas(cv);
    let (bark, bark_normal) = bark(p);
    PlantTextures {
        atlas,
        bark,
        bark_normal,
    }
}

/// Bleed colours into transparent texels (per cell, multi-source BFS from coverage > 0) and
/// convert to RGBA8.
fn finish_atlas(cv: Canvas) -> Image {
    let n = cv.size;
    let mut px = cv.px;
    // Premultiplied to straight alpha where there is real coverage.
    for p in px.iter_mut() {
        if p[3] > 0.02 {
            for k in 0..3 {
                p[k] = (p[k] / p[3]).min(1.0);
            }
        }
    }
    let cp = CELL_PX as usize;
    let mut queue = std::collections::VecDeque::new();
    for cy in 0..ATLAS_CELLS as usize {
        for cx in 0..ATLAS_CELLS as usize {
            let (x0, y0) = (cx * cp, cy * cp);
            let mut seen = vec![false; cp * cp];
            queue.clear();
            for y in 0..cp {
                for x in 0..cp {
                    if px[(y0 + y) * n + x0 + x][3] > 0.02 {
                        seen[y * cp + x] = true;
                        queue.push_back((x, y));
                    }
                }
            }
            if queue.is_empty() {
                // Empty cell: a neutral leaf green so nothing ever samples black.
                for y in 0..cp {
                    for x in 0..cp {
                        px[(y0 + y) * n + x0 + x] = [0.3, 0.4, 0.2, 0.0];
                    }
                }
                continue;
            }
            // Multi-source BFS: every texel without coverage takes the colour of the nearest
            // covered one (alpha unchanged), within this cell only.
            while let Some((x, y)) = queue.pop_front() {
                let c = px[(y0 + y) * n + x0 + x];
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= cp as i32 || ny >= cp as i32 {
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    if !seen[ny * cp + nx] {
                        seen[ny * cp + nx] = true;
                        let d = &mut px[(y0 + ny) * n + x0 + nx];
                        d[0] = c[0];
                        d[1] = c[1];
                        d[2] = c[2];
                        queue.push_back((nx, ny));
                    }
                }
            }
        }
    }
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let rgba = px
        .iter()
        .flat_map(|p| [to8(p[0]), to8(p[1]), to8(p[2]), to8(p[3])])
        .collect();
    Image {
        width: ATLAS_SIZE,
        height: ATLAS_SIZE,
        rgba,
    }
}

/// Worley (cellular) distances F1, F2 and the nearest cell id on a wrapping grid.
fn worley(u: f32, v: f32, gx: i32, gy: i32, salt: u32) -> (f32, f32, u32) {
    let (x, y) = (u * gx as f32, v * gy as f32);
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2, mut id) = (9.0f32, 9.0f32, 0u32);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (xi + dx, yi + dy);
            let (wx, wy) = (cx.rem_euclid(gx), cy.rem_euclid(gy));
            let px = cx as f32 + crate::math::hash2(wx, wy, salt);
            let py = cy as f32 + crate::math::hash2(wx, wy, salt ^ 0x55);
            let d = ((px - x) * (px - x) + (py - y) * (py - y)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = (wx * 131 + wy) as u32;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

fn bark(p: &PlantParams) -> (Image, Image) {
    let n = BARK_SIZE as usize;
    let b = &p.bark;
    let mut height = vec![0.0f32; n * n];
    let mut col = vec![[0.0f32; 3]; n * n];
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let fine = tile_fbm(u, v, 16, 16, 4, 11);
            let (h, c) = match b.pattern {
                BarkPattern::Smooth => {
                    let lo = tile_fbm(u, v, 3, 3, 4, 3);
                    let lichen = smoothstep(0.62, 0.72, tile_fbm(u, v, 4, 4, 3, 9));
                    let c = mix(
                        mul(b.colour, 0.88 + 0.25 * lo + 0.08 * fine),
                        b.accent,
                        lichen * 0.5,
                    );
                    (0.3 * lo + 0.1 * fine, c)
                }
                BarkPattern::Birch => {
                    let len = tile_noise(u, v, 3, 40, 21);
                    let dash = smoothstep(0.7, 0.78, len)
                        * smoothstep(0.35, 0.6, tile_noise(u, v, 10, 40, 22));
                    let patch = smoothstep(0.6, 0.7, tile_fbm(u, v, 3, 4, 4, 23));
                    let dark = dash.max(patch * (0.5 + 0.5 * fine));
                    let c = mix(mul(b.colour, 0.92 + 0.12 * fine), b.accent, dark);
                    (1.0 - 0.8 * dark, c)
                }
                BarkPattern::Furrowed => {
                    let warp = tile_fbm(u, v, 2, 3, 3, 31) - 0.5;
                    let xr = (u + warp * 0.25) * 7.0;
                    let ridge = 1.0 - ((xr.rem_euclid(1.0)) * 2.0 - 1.0).abs();
                    let breaks = smoothstep(0.55, 0.75, tile_noise(u, v, 7, 5, 32));
                    let h =
                        smoothstep(0.25, 0.7, ridge) * (1.0 - 0.6 * breaks) * (0.8 + 0.3 * fine);
                    (h, mix(b.accent, mul(b.colour, 0.85 + 0.3 * fine), h))
                }
                BarkPattern::Flaky | BarkPattern::Scaly => {
                    let (gx, gy) = if b.pattern == BarkPattern::Scaly {
                        (10, 14)
                    } else {
                        (5, 6)
                    };
                    let (f1, f2, id) = worley(u, v, gx, gy, 41);
                    let crack = smoothstep(0.02, 0.12, f2 - f1);
                    let k = crate::math::hash2(id as i32, 7, 42);
                    let plate = mix(
                        b.colour,
                        b.accent,
                        if b.pattern == BarkPattern::Flaky {
                            k
                        } else {
                            k * 0.35
                        },
                    );
                    let c = mix(mul(b.accent, 0.45), mul(plate, 0.9 + 0.2 * fine), crack);
                    (crack * (0.7 + 0.3 * (1.0 - f1)), c)
                }
                BarkPattern::Fluted => {
                    let w = sin(TAU * (u * 5.0 + 0.35 * tile_fbm(u, v, 2, 2, 3, 51)));
                    let h = 0.5 + 0.5 * w;
                    (
                        h,
                        mul(mix(b.accent, b.colour, 0.5 + 0.5 * h), 0.92 + 0.15 * fine),
                    )
                }
                BarkPattern::Fibrous => {
                    let s = tile_fbm(u + v, v, 24, 6, 3, 61);
                    let t = tile_fbm(u - v, v, 24, 6, 3, 62);
                    let h = (s * 0.6 + t * 0.6).min(1.0);
                    (h, mix(b.accent, b.colour, h))
                }
                BarkPattern::Stem => {
                    let lines = 0.5 + 0.5 * sin(TAU * u * 24.0);
                    let node = 1.0 - smoothstep(0.0, 0.04, v.min(1.0 - v));
                    let c = mix(
                        mul(b.colour, 0.92 + 0.1 * lines + 0.06 * fine),
                        b.accent,
                        node * 0.7,
                    );
                    (0.4 + 0.1 * lines + 0.4 * node, c)
                }
            };
            height[y * n + x] = h;
            col[y * n + x] = c;
        }
    }
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let albedo = col
        .iter()
        .flat_map(|c| [to8(c[0]), to8(c[1]), to8(c[2]), 255])
        .collect();
    let strength = 3.0;
    let mut normal = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let hx = height[y * n + (x + 1) % n] - height[y * n + (x + n - 1) % n];
            let hy = height[((y + 1) % n) * n + x] - height[((y + n - 1) % n) * n + x];
            let (nx, ny, nz) = (-hx * strength, -hy * strength, 1.0);
            let l = (nx * nx + ny * ny + nz * nz).sqrt();
            normal.extend_from_slice(&[
                to8(nx / l * 0.5 + 0.5),
                to8(ny / l * 0.5 + 0.5),
                to8(nz / l * 0.5 + 0.5),
                255,
            ]);
        }
    }
    (
        Image {
            width: BARK_SIZE,
            height: BARK_SIZE,
            rgba: albedo,
        },
        Image {
            width: BARK_SIZE,
            height: BARK_SIZE,
            rgba: normal,
        },
    )
}
