//! Small, deterministic maths: a 3-vector, a PRNG and trigonometry built only from IEEE-754
//! basic operations (`+ - * /`, `sqrt`, `floor`), which are correctly rounded on every platform
//! Rust targets. `f32::sin` and friends call the platform's libm and may differ in the last bit
//! between x86_64, aarch64 and wasm32, so this crate never uses them.

use std::ops::{Add, AddAssign, Mul, Neg, Sub};

pub const PI: f32 = std::f32::consts::PI;
pub const TAU: f32 = std::f32::consts::TAU;
/// The golden angle in radians (137.5 degrees): the divergence angle of spiral phyllotaxis,
/// Prusinkiewicz & Lindenmayer, "The Algorithmic Beauty of Plants" (1990), ch. 4.
pub const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// Sine with a Taylor polynomial after range reduction. Absolute error below 1e-6 on any input
/// the generators produce.
pub fn sin(x: f32) -> f32 {
    let x = x as f64;
    let k = (x / std::f64::consts::TAU + 0.5).floor();
    let mut r = x - k * std::f64::consts::TAU; // [-pi, pi]
    let half = std::f64::consts::FRAC_PI_2;
    if r > half {
        r = std::f64::consts::PI - r;
    } else if r < -half {
        r = -std::f64::consts::PI - r;
    }
    let r2 = r * r;
    // Horner form of r - r^3/3! + r^5/5! - ... + r^13/13!
    let p = 1.0
        - r2 / 6.0
            * (1.0
                - r2 / 20.0
                    * (1.0
                        - r2 / 42.0 * (1.0 - r2 / 72.0 * (1.0 - r2 / 110.0 * (1.0 - r2 / 156.0)))));
    (r * p) as f32
}

pub fn cos(x: f32) -> f32 {
    sin(x + PI * 0.5)
}

/// Arc tangent of y/x in (-pi, pi], from a polynomial on [0, 1] plus octant folding.
pub fn atan2(y: f32, x: f32) -> f32 {
    let ax = x.abs();
    let ay = y.abs();
    if ax == 0.0 && ay == 0.0 {
        return 0.0;
    }
    let (t, swap) = if ay > ax {
        (ax / ay, true)
    } else {
        (ay / ax, false)
    };
    // Odd minimax-style polynomial for atan on [0, 1]; max error about 1e-5 rad.
    let t2 = t * t;
    let mut a = t
        * (0.999_866
            + t2 * (-0.330_299_5 + t2 * (0.180_141 + t2 * (-0.085_133 + t2 * 0.020_835_1))));
    if swap {
        a = PI * 0.5 - a;
    }
    if x < 0.0 {
        a = PI - a;
    }
    if y < 0.0 { -a } else { a }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn deg(d: f32) -> f32 {
    d * (PI / 180.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}

pub const UP: V3 = v3(0.0, 1.0, 0.0);

impl V3 {
    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        v3(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    /// Unit vector; falls back to +Y for (near) zero vectors so no NaN can escape.
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 1e-12 { self * (1.0 / l) } else { UP }
    }
    pub fn lerp(self, o: V3, t: f32) -> V3 {
        self + (o - self) * t
    }
    pub fn arr(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
    /// Any unit vector perpendicular to `self` (which must be unit length).
    pub fn perp(self) -> V3 {
        let a = if self.y.abs() < 0.9 {
            UP
        } else {
            v3(1.0, 0.0, 0.0)
        };
        self.cross(a).norm()
    }
    /// Rodrigues rotation of `self` about the unit `axis` by `angle` radians.
    pub fn rotate(self, axis: V3, angle: f32) -> V3 {
        let (s, c) = (sin(angle), cos(angle));
        self * c + axis.cross(self) * s + axis * (axis.dot(self) * (1.0 - c))
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl AddAssign for V3 {
    fn add_assign(&mut self, o: V3) {
        *self = *self + o;
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, s: f32) -> V3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
}
impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        v3(-self.x, -self.y, -self.z)
    }
}

/// SplitMix64 (Steele, Lea & Flood, "Fast splittable pseudorandom number generators", 2014).
/// Small, fast and fully deterministic; plenty for plant variation.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    /// A stream for one purpose (`stream`) of one plant (`seed`), so that e.g. the skeleton
    /// draws the same numbers whatever the level of detail asks for later.
    pub fn new(seed: u64, stream: u64) -> Rng {
        let mut r = Rng(seed ^ stream.wrapping_mul(0xA076_1D64_78BD_642F));
        r.next_u64();
        r
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
    /// Uniform in [-1, 1).
    pub fn sym(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() >> 33) as usize) % n.max(1)
    }
    /// Approximately normal (sum of three uniforms), mean 0, sd about 1.
    pub fn gauss(&mut self) -> f32 {
        (self.f() + self.f() + self.f() - 1.5) * 2.0
    }
}

/// Integer hash to [0, 1) for lattice noise.
pub fn hash2(x: i32, y: i32, salt: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD816_3841)
        ^ salt.wrapping_mul(0xCB1A_B31F);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5BD1_E995);
    h ^= h >> 15;
    (h >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
}

/// Tileable value noise: lattice of `period_x` x `period_y` cells over the unit square.
pub fn tile_noise(u: f32, v: f32, period_x: i32, period_y: i32, salt: u32) -> f32 {
    let x = u * period_x as f32;
    let y = v * period_y as f32;
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (xi, yi) = (xi as i32, yi as i32);
    let w = |a: i32, p: i32| a.rem_euclid(p);
    let h = |a: i32, b: i32| hash2(w(a, period_x), w(b, period_y), salt);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = lerp(h(xi, yi), h(xi + 1, yi), sx);
    let b = lerp(h(xi, yi + 1), h(xi + 1, yi + 1), sx);
    lerp(a, b, sy)
}

/// Tileable fractal noise in [0, 1].
pub fn tile_fbm(u: f32, v: f32, px: i32, py: i32, octaves: u32, salt: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut fx, mut fy) = (px, py);
    for o in 0..octaves {
        sum += tile_noise(u, v, fx, fy, salt.wrapping_add(o * 7919)) * amp;
        norm += amp;
        amp *= 0.5;
        fx *= 2;
        fy *= 2;
    }
    sum / norm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trig_matches_std_closely() {
        let mut x = -20.0f32;
        while x < 20.0 {
            assert!((sin(x) - x.sin()).abs() < 2e-6, "sin {x}");
            assert!((cos(x) - x.cos()).abs() < 2e-6, "cos {x}");
            x += 0.0137;
        }
        for i in 0..400 {
            let a = i as f32 * 0.0157 - PI;
            let (y, x) = (sin(a) * 3.0, cos(a) * 3.0);
            assert!((atan2(y, x) - y.atan2(x)).abs() < 3e-5, "atan2 {a}");
        }
    }
}
