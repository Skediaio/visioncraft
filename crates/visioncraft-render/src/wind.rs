//! Wind for vegetation, in real units: a speed in m/s, a direction, a gustiness and gusts. This module
//! turns the speed into what the vertex shader needs (lean, detail amplitude, phases) and keeps
//! the CPU mirror of the shader's sway (`common.wgsl` `wind_sway`), which `tests/wind.rs` checks
//! against the GPU. Nothing here is per frame heavy: a few multiplications and one uniform.
//!
//! Physics, from open sources only:
//! - Drag on a crown grows with the speed squared, but crowns reconfigure (streamline) as the
//!   wind rises, so the force grows more slowly: drag ~ U^(2 + nu) with nu about -0.7 for broad
//!   leaves (Vogel 1994; de Langre, "Effects of wind on plants", Ann. Rev. Fluid Mech. 40, 2008).
//!   `lean` is that law, soft-saturated, as a share of the distance from the plant's base.
//! - Leaves and twigs flutter at a rate that grows with the wind speed (vortex shedding:
//!   f = St U / d, de Langre 2008). Clamped to 1..6 Hz: faster rates alias at 60 fps.
//! - A whole plant sways at its natural frequency, which does not depend on the wind: for a
//!   stem f ~ D / H^2 (cantilever beam; Moore & Maguire 2004 for conifers). With stem diameter
//!   D ~ H^1.25 (between geometric similarity, 1, and elastic similarity, 1.5; McMahon 1973) that
//!   gives f ~ H^-0.75. Jackson et al. 2019 find the cantilever law fails for leafed broadleaves,
//!   whose frequency is lower: a plain factor tuned by eye.
//! - The Beaufort scale is NOAA/WMO's: v = 0.836 B^1.5 m/s.
//! - Gusts (`Wind::gusts`) are a field in the world carried downwind at the wind speed
//!   (Taylor's frozen turbulence hypothesis: eddies drift with the mean flow), so a gust crosses
//!   a lawn as a travelling wave and reaches one crown after another (`gust_field`).

use glam::Vec3;

/// The old relative wind strength (0 calm, 1 a moderate breeze) in m/s: strength 1 is the middle
/// of Beaufort 4 (5.5..7.9 m/s). Projects written before wind had units map through this.
pub const RELATIVE_MPS: f32 = 6.5;
/// Mean lean (share of the distance from the base) a storm approaches.
pub const LEAN_SATURATION: f32 = 0.08;
/// Gusts are a travelling pattern of this wavelength (m); it moves at the wind speed.
pub const GUST_WAVELENGTH: f32 = 80.0;
/// The gust field's lattice repeats every this many cells along the wind (`common.wgsl`
/// `GUST_CELLS`), so the distance it has travelled wraps exactly.
pub const GUST_CELLS: i32 = 64;
/// Metres per cell of the gust field's coarse and fine layers, along and across the wind.
pub const GUST_COARSE: [f32; 2] = [14.0, 22.0];
pub const GUST_FINE: [f32; 2] = [5.0, 7.0];
/// The fine layer drifts slower than the coarse one, so the patches change shape as they go.
pub const GUST_FINE_SPEED: f64 = 0.8;
/// The sway clock wraps here (s). Per-plant frequencies are whole cycles per wrap, so the
/// wrap is invisible (`common.wgsl` `WIND_CLOCK`).
pub const SWAY_CLOCK: f64 = 64.0;

/// Wind for the vegetation animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wind {
    /// Where the wind blows to (world; only the horizontal part is used).
    pub direction: Vec3,
    /// Wind speed in m/s (0: perfectly still air).
    pub speed_mps: f32,
    /// 0..1: how much the strength swells and fades along the wind (a slow travelling wave).
    pub gustiness: f32,
    /// 0..1: gusts as patches of stronger wind travelling downwind through grass and crowns
    /// (`gust_field`); lawn grass bends with them. 0: none (the sway as it was before them).
    pub gusts: f32,
}

impl Default for Wind {
    fn default() -> Self {
        Self {
            direction: Vec3::X,
            speed_mps: 0.0,
            gustiness: 0.5,
            gusts: 0.0,
        }
    }
}

impl Wind {
    /// The old relative strength (speed / `RELATIVE_MPS`), still what water ripples follow.
    pub fn strength(&self) -> f32 {
        self.speed_mps.max(0.0) / RELATIVE_MPS
    }
}

/// Mean lean of a crown tip per unit of its distance from the base at wind speed `u` (m/s): the
/// drag law above, d = 2 x^2 / (1 + x^0.7) with x = u / 6.5 m/s, soft-saturated by d / (1 + d).
/// 0 at 0 m/s, 0.04 at 6.5 m/s, approaching `LEAN_SATURATION`.
pub fn lean(u: f32) -> f32 {
    let x = u.max(0.0) / RELATIVE_MPS;
    let d = 2.0 * x * x / (1.0 + x.powf(0.7));
    LEAN_SATURATION * d / (1.0 + d)
}

/// Amplitude of branch and leaf motion relative to 6.5 m/s: it follows the lean.
pub fn detail(u: f32) -> f32 {
    lean(u) / lean(RELATIVE_MPS)
}

/// Leaf flutter rate (Hz) at wind speed `u`: St U / d with a Strouhal number of 0.2 and a
/// 0.4 m leaf cluster, within 1..6 Hz.
pub fn flutter_hz(u: f32) -> f32 {
    (0.5 * u).clamp(1.0, 6.0)
}

/// Natural sway frequency (Hz) of a plant `height_m` tall: 0.5 Hz for a 10 m conifer, falling
/// with H^-0.75 (see the module docs), 20 % lower for a leafed broadleaf, within 0.15..3 Hz.
pub fn plant_hz(height_m: f32, conifer: bool) -> f32 {
    let f = 0.5 * (height_m.max(0.01) / 10.0).powf(-0.75);
    (f * if conifer { 1.0 } else { 0.8 }).clamp(0.15, 3.0)
}

/// How far (m) a plant of `extent` metres can move from its rest bounds at wind speed `u`, so
/// culling bounds never clip a swaying tip: twice the mean lean (a full gust) times the extent,
/// plus the detail amplitude doubled by a gust, times the shader's `0.02 + 0.01 min(size, 6)`.
/// Never less than the old fixed room of 10 % of the extent and 0.1 m (still air and breezes).
/// `gusts` (0..1) raises a full gust's factor from 2 to 3 (`gust_factor`); lawn `grass` in gusts
/// bends up to 50 degrees, which moves a tip by up to 0.85 of its length.
pub fn bounds_pad(extent: f32, u: f32, gusts: f32, grass: bool) -> f32 {
    let gust = 2.0 + gusts.clamp(0.0, 1.0);
    let physical = gust * lean(u) * extent + gust * detail(u) * (0.02 + 0.01 * extent.min(6.0));
    let pad = physical.max(0.1 * extent + 0.1);
    if grass && gusts > 0.0 && u > 0.0 {
        pad.max(0.9 * extent + 0.05)
    } else {
        pad
    }
}

/// Phases integrated on the CPU. The wind's phases advance by the frequency of the moment times
/// the time step, so a change of speed bends the rate and never jumps the phase, and wrapping
/// them is exact (no `f * t` that jumps when `t` wraps).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Phases {
    /// The clock the last `advance` ran to (s).
    pub clock: f64,
    /// Gust pattern, cycles 0..1.
    pub gust: f64,
    /// Sway clock, s in 0..`SWAY_CLOCK`.
    pub sway: f64,
    /// Leaf flutter, cycles 0..1.
    pub flutter: f64,
    /// How far the gust field's coarse layer has travelled downwind (m, wraps every
    /// `GUST_CELLS` of its cells).
    pub gust_coarse_m: f64,
    /// The same for the fine layer, which drifts at `GUST_FINE_SPEED` of the wind.
    pub gust_fine_m: f64,
}

impl Phases {
    /// Moves to `clock` (s) with the wind at `speed` m/s over the step. Going backwards runs the
    /// phases backwards, so a frame at a given time is the same however it was reached
    /// (as long as the speed was constant).
    pub fn advance(&mut self, clock: f64, speed: f32) {
        let dt = clock - self.clock;
        self.clock = clock;
        let speed = f64::from(speed.max(0.0));
        self.gust = (self.gust + dt * speed / f64::from(GUST_WAVELENGTH)).rem_euclid(1.0);
        self.flutter = (self.flutter + dt * f64::from(flutter_hz(speed as f32))).rem_euclid(1.0);
        self.sway = (self.sway + dt).rem_euclid(SWAY_CLOCK);
        let coarse = f64::from(GUST_CELLS) * f64::from(GUST_COARSE[0]);
        let fine = f64::from(GUST_CELLS) * f64::from(GUST_FINE[0]);
        self.gust_coarse_m = (self.gust_coarse_m + dt * speed).rem_euclid(coarse);
        self.gust_fine_m = (self.gust_fine_m + dt * speed * GUST_FINE_SPEED).rem_euclid(fine);
    }
}

/// What the shaders read: `Globals.wind`, `.wind_phase`, `.wind_amp` and `.gust`
/// (`common.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Uniforms {
    pub wind: [f32; 4],
    pub phase: [f32; 4],
    pub amp: [f32; 4],
    pub gust: [f32; 4],
}

impl Uniforms {
    pub fn new(wind: &Wind, phases: &Phases) -> Self {
        let dir = glam::Vec2::new(wind.direction.x, wind.direction.z).normalize_or(glam::Vec2::X);
        let u = wind.speed_mps.max(0.0);
        Self {
            wind: [
                dir.x,
                dir.y,
                wind.strength(),
                wind.gustiness.clamp(0.0, 1.0),
            ],
            phase: [
                phases.gust as f32,
                phases.sway as f32,
                phases.flutter as f32,
                u,
            ],
            amp: [lean(u), detail(u), 0.0, 0.0],
            gust: [
                phases.gust_coarse_m as f32,
                phases.gust_fine_m as f32,
                wind.gusts.clamp(0.0, 1.0),
                0.0,
            ],
        }
    }
}

/// `common.wgsl` `pcg`: Jarzynski & Olano, "Hash Functions for GPU Rendering", JCGT 2020.
fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

/// `common.wgsl` `hash3`.
fn hash3(p: [i32; 3]) -> f32 {
    let h = pcg((p[0] as u32).wrapping_add(pcg((p[1] as u32).wrapping_add(pcg(p[2] as u32)))));
    (h >> 8) as f32 / 16_777_216.0
}

fn gust_lattice(cell: [i32; 2], layer: i32) -> f32 {
    hash3([cell[0].rem_euclid(GUST_CELLS), cell[1], layer])
}

fn gust_noise(p: [f32; 2], layer: i32) -> f32 {
    let i = [p[0].floor() as i32, p[1].floor() as i32];
    let f = [p[0] - p[0].floor(), p[1] - p[1].floor()];
    let u = f.map(|f| f * f * f * (f * (f * 6.0 - 15.0) + 10.0));
    let a = gust_lattice(i, layer);
    let b = gust_lattice([i[0] + 1, i[1]], layer);
    let c = gust_lattice([i[0], i[1] + 1], layer);
    let d = gust_lattice([i[0] + 1, i[1] + 1], layer);
    let mix = |x: f32, y: f32, t: f32| x + (y - x) * t;
    mix(mix(a, b, u[0]), mix(c, d, u[0]), u[1])
}

/// `common.wgsl` `gust_field`: the gust at world (x, z), 0 (a lull) to 1 (a gust's core).
pub fn gust_field(u: &Uniforms, at: [f32; 2]) -> f32 {
    let along = at[0] * u.wind[0] + at[1] * u.wind[1];
    let across = -at[0] * u.wind[1] + at[1] * u.wind[0];
    let coarse = gust_noise(
        [
            (along - u.gust[0]) / GUST_COARSE[0],
            across / GUST_COARSE[1],
        ],
        0,
    );
    let fine = gust_noise(
        [(along - u.gust[1]) / GUST_FINE[0], across / GUST_FINE[1]],
        1,
    );
    let x = ((0.7 * coarse + 0.3 * fine - 0.42) / 0.4).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// `common.wgsl` `gust_factor`: the swell times what a lull leaves, plus a gust's push.
pub fn gust_factor(u: &Uniforms, swell: f32, at: [f32; 2]) -> f32 {
    let k = u.gust[2];
    swell * (1.0 - 0.4 * k) + 1.8 * k * gust_field(u, at)
}

fn smooth_triangle(x: f32) -> f32 {
    let t = ((x + 0.5).fract() * 2.0 - 1.0).abs();
    t * t * (3.0 - 2.0 * t)
}

/// `common.wgsl` `wind_sway` on the CPU: the point `rel` (from the plant's origin at `origin`,
/// bend weight `w`, limb phase `phase`, plant sway frequency `hz_in`, negative for lawn grass)
/// displaced by the wind.
pub fn sway(u: &Uniforms, rel: Vec3, origin: Vec3, w: f32, phase: f32, hz_in: f32) -> Vec3 {
    if w <= 0.0 || u.amp[0] <= 0.0 {
        return rel;
    }
    let hz = hz_in.abs();
    let grass = hz_in < 0.0;
    let clock = SWAY_CLOCK as f32;
    let dir = Vec3::new(u.wind[0], 0.0, u.wind[1]);
    let instance_phase = (origin.x * 0.754_877_7 + origin.z * 0.569_840_3).fract();
    let k = (hz * clock).round_ties_even().clamp(2.0, 6.0 * clock);
    let kb = (4.0 * k).min(6.0 * clock);
    let along = origin.x * u.wind[0] + origin.z * u.wind[1];
    let wave = |x: f32| smooth_triangle(x) * 2.0 - 1.0;
    let waves = [
        wave(u.phase[0] - along / GUST_WAVELENGTH),
        wave(u.phase[1] * (k / clock) + instance_phase),
        wave(u.phase[1] * (kb / clock) + phase + instance_phase),
        wave(u.phase[2] + phase * 1.7),
    ];
    let mut gust = 1.0 + u.wind[3] * waves[0];
    let mut gust_detail = gust;
    let size = rel.length();
    if u.gust[2] > 0.0 {
        let swell = gust;
        gust = gust_factor(u, swell, [origin.x, origin.z]);
        gust_detail = gust;
        if !grass {
            gust_detail = gust_factor(u, swell, [origin.x + rel.x, origin.z + rel.z]);
        }
    }
    let mut bend = u.amp[0] * gust * (0.6 + 0.4 * waves[1]);
    if grass {
        bend = (bend * (1.0 + 5.0 * u.gust[2])).min(1.2);
    }
    let mut q = rel + dir * (w * w * bend * size);
    q *= size / q.length().max(1e-6);
    let detail = u.amp[1] * gust_detail * w * (0.02 + 0.01 * size.min(6.0)) * (size * 2.0).min(1.0);
    q + (dir * waves[2] + Vec3::new(0.0, 0.6, 0.0) * waves[3]) * detail
}

/// `common.wgsl` `impostor_sway`: the same for a point of an impostor's quad at height share `h`.
pub fn impostor_sway(u: &Uniforms, rel: Vec3, origin: Vec3, h: f32, hz: f32) -> Vec3 {
    sway(u, rel, origin, h.clamp(0.0, 1.0).powf(1.5), 0.0, hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lean_is_monotone_saturating_and_still_at_rest() {
        assert_eq!(lean(0.0), 0.0);
        assert!((lean(RELATIVE_MPS) - 0.04).abs() < 1e-6);
        let mut last = 0.0;
        for i in 1..=400 {
            let l = lean(i as f32 * 0.1);
            assert!(l > last && l < LEAN_SATURATION, "{i}: {l}");
            last = l;
        }
        // Sub-quadratic above Beaufort 6: doubling the speed less than quadruples the lean.
        assert!(lean(2.0 * 13.8) < 4.0 * lean(13.8));
    }

    #[test]
    fn sway_frequency_follows_size() {
        // Derived, not copied: f = 0.5 Hz (H/10 m)^-0.75 for a conifer.
        assert!((plant_hz(10.0, true) - 0.5).abs() < 1e-6);
        assert!((plant_hz(20.0, true) - 0.5 * 2f32.powf(-0.75)).abs() < 1e-6);
        assert!(plant_hz(20.0, false) < plant_hz(20.0, true));
        assert!(plant_hz(2.0, false) > plant_hz(10.0, false));
        assert_eq!(plant_hz(0.1, true), 3.0);
    }

    #[test]
    fn phases_do_not_jump_when_the_speed_changes() {
        let mut p = Phases::default();
        p.advance(10.0, 2.0);
        let (g, f) = (p.gust, p.flutter);
        // Same instant, other speed: the phases stay where they are.
        p.advance(10.0, 12.0);
        assert_eq!((p.gust, p.flutter), (g, f));
    }
}
