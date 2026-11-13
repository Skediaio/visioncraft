//! Daylight: the sun's light and the analytic sky, on the CPU. The sky is
//! Preetham, Shirley & Smits, "A Practical Analytic Model for Daylight"
//! (SIGGRAPH 1999), formulas and coefficients from its Appendix A. sky.wgsl
//! evaluates the same distribution per pixel from the coefficients made here.
//!
//! Why Preetham and not Hosek-Wilkie (2012): Preetham is a handful of
//! published coefficients that can be checked line by line against the
//! paper; Hosek-Wilkie needs the authors' fitted datasets (thousands of
//! numbers per channel and turbidity) embedded in the binary. Preetham's
//! known weaknesses (too little colour at sunset, nonsense below the
//! horizon) are handled by the night fade and the horizon clamp.

use glam::Vec3;

/// Atmospheric turbidity: 2.5 is a clear day.
pub const TURBIDITY: f32 = 2.5;

/// The sun's angular radius as drawn: 0.5 degrees, about twice the real one,
/// so the disc reads at laptop resolutions.
pub const SUN_DISC_RADIUS: f32 = 0.5 * std::f32::consts::PI / 180.0;

/// Night sky radiance (cd/m2, linear Rec.709), present day and night as a
/// floor under the daylight model: a clear, moonlit night sky is of the
/// order of 0.01-0.05 cd/m2 (moonlit ground ~0.1 lux). Absolute, not
/// display-relative: at a daylight exposure it vanishes, and a night view
/// needs a night exposure (see `Renderer::set_exposure`), there is no auto
/// exposure.
pub const NIGHT: Vec3 = Vec3::new(0.012, 0.018, 0.04);

/// Directional sun light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    /// Unit vector from the scene towards the sun (world, Y up).
    pub direction: Vec3,
    /// Illuminance on a surface facing the sun, in lux.
    pub illuminance: f32,
    /// Linear Rec.709 colour, normalised to luminance 1.
    pub color: Vec3,
}

/// Rec.709 luminance weights.
pub fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

impl Sun {
    /// Sun from compass azimuth (degrees clockwise from north, -Z) and
    /// elevation above the horizon (degrees). Colour and illuminance follow
    /// the air mass: Kasten & Young, "Revised optical air mass tables and
    /// approximation formula", Applied Optics 1989, with per-channel optical
    /// depths of Rayleigh scattering (Bucholtz, Applied Optics 1995, at
    /// 680/550/440 nm) plus a clear-day aerosol term; 128 klx outside the
    /// atmosphere (Filament documentation, "Physically based units").
    pub fn from_angles(azimuth_deg: f32, elevation_deg: f32) -> Sun {
        let (az, el) = (azimuth_deg.to_radians(), elevation_deg.to_radians());
        let direction = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos()).normalize();
        if elevation_deg <= 0.0 {
            return Sun {
                direction,
                illuminance: 0.0,
                color: Vec3::ONE,
            };
        }
        let zenith_deg = 90.0 - elevation_deg;
        let air_mass =
            1.0 / (zenith_deg.to_radians().cos() + 0.50572 * (96.07995 - zenith_deg).powf(-1.6364));
        let tau = Vec3::new(0.043, 0.097, 0.236) + Vec3::new(0.07, 0.08, 0.09);
        let transmittance = (-tau * air_mass).exp();
        let lum = luminance(transmittance);
        Sun {
            direction,
            illuminance: 128_000.0 * lum,
            color: transmittance / lum,
        }
    }
}

/// Perez coefficients A..E for (Y, x, y), and the zenith (Y [kcd/m2], x, y)
/// already divided by F(0, theta_sun), for a sun direction.
#[derive(Clone, Copy, Debug)]
pub struct Sky {
    pub perez: [Vec3; 5],
    pub zenith: Vec3,
    pub sun: Vec3,
    /// 1 in daylight, fading to 0 (night floor) through twilight.
    pub day: f32,
}

fn perez(c: &[Vec3; 5], cos_theta: f32, gamma: f32) -> Vec3 {
    let [a, b, cc, d, e] = *c;
    let cos_gamma = gamma.cos();
    (Vec3::ONE + a * (b / cos_theta).exp())
        * (Vec3::ONE + cc * (d * gamma).exp() + e * cos_gamma * cos_gamma)
}

impl Sky {
    pub fn new(sun_dir: Vec3) -> Sky {
        let t = TURBIDITY;
        // The model is defined for the sun above the horizon only; below it,
        // keep the horizon sky and fade to night (civil twilight, -6 deg).
        let elevation = sun_dir.y.clamp(-1.0, 1.0).asin();
        let day = twilight(elevation.to_degrees());
        let theta_s = (std::f32::consts::FRAC_PI_2 - elevation).clamp(0.0, 89.5f32.to_radians());
        let sun = Vec3::new(sun_dir.x, theta_s.cos(), sun_dir.z);
        let horizontal = (sun.x * sun.x + sun.z * sun.z).sqrt();
        let sun = if horizontal > 1e-6 {
            let s = theta_s.sin() / horizontal;
            Vec3::new(sun.x * s, theta_s.cos(), sun.z * s)
        } else {
            Vec3::Y
        };

        let coeffs = [
            Vec3::new(
                0.1787 * t - 1.4630,
                -0.0193 * t - 0.2592,
                -0.0167 * t - 0.2608,
            ),
            Vec3::new(
                -0.3554 * t + 0.4275,
                -0.0665 * t + 0.0008,
                -0.0950 * t + 0.0092,
            ),
            Vec3::new(
                -0.0227 * t + 5.3251,
                -0.0004 * t + 0.2125,
                -0.0079 * t + 0.2102,
            ),
            Vec3::new(
                0.1206 * t - 2.5771,
                -0.0641 * t - 0.8989,
                -0.0441 * t - 1.6537,
            ),
            Vec3::new(
                -0.0670 * t + 0.3703,
                -0.0033 * t + 0.0452,
                -0.0109 * t + 0.0529,
            ),
        ];
        let chi = (4.0 / 9.0 - t / 120.0) * (std::f32::consts::PI - 2.0 * theta_s);
        let big_y = (4.0453 * t - 4.9710) * chi.tan() - 0.2155 * t + 2.4192;
        let th = [theta_s.powi(3), theta_s.powi(2), theta_s, 1.0];
        let tt = [t * t, t, 1.0];
        let zenith_chroma = |m: [[f32; 4]; 3]| -> f32 {
            (0..3)
                .map(|i| tt[i] * (0..4).map(|j| m[i][j] * th[j]).sum::<f32>())
                .sum()
        };
        let x = zenith_chroma([
            [0.00166, -0.00375, 0.00209, 0.0],
            [-0.02903, 0.06377, -0.03202, 0.00394],
            [0.11693, -0.21196, 0.06052, 0.25886],
        ]);
        let y = zenith_chroma([
            [0.00275, -0.00610, 0.00317, 0.0],
            [-0.04214, 0.08970, -0.04153, 0.00516],
            [0.15346, -0.26756, 0.06670, 0.26688],
        ]);
        let zenith = Vec3::new(big_y.max(0.0), x, y) / perez(&coeffs, 1.0, theta_s);
        Sky {
            perez: coeffs,
            zenith,
            sun,
            day,
        }
    }

    /// Daylight sky radiance (cd/m2, linear Rec.709) towards `dir`, without
    /// the sun disc and without the night fallback. Mirrors sky.wgsl.
    pub fn radiance(&self, dir: Vec3) -> Vec3 {
        let cos_theta = dir.y.max(0.001);
        let clamped = Vec3::new(dir.x, cos_theta, dir.z).normalize();
        let gamma = clamped.dot(self.sun).clamp(-1.0, 1.0).acos();
        let yxy = self.zenith * perez(&self.perez, cos_theta, gamma);
        let big_y = yxy.x * 1000.0;
        let xyz = Vec3::new(
            yxy.y / yxy.z * big_y,
            big_y,
            (1.0 - yxy.y - yxy.z) / yxy.z * big_y,
        );
        let rgb = Vec3::new(
            3.2406 * xyz.x - 1.5372 * xyz.y - 0.4986 * xyz.z,
            -0.9689 * xyz.x + 1.8758 * xyz.y + 0.0415 * xyz.z,
            0.0557 * xyz.x - 0.2040 * xyz.y + 1.0570 * xyz.z,
        );
        let below = 1.0 + (0.35 - 1.0) * smoothstep(0.0, -0.15, dir.y);
        rgb.max(Vec3::ZERO) * below
    }

    /// Radiance (cd/m2) of a flat grey ground (albedo 0.2) lit by the sun and
    /// this sky: what the environment shows below the horizon.
    pub fn ground_radiance(&self, sun: &Sun) -> Vec3 {
        const GROUND_ALBEDO: f32 = 0.2;
        let (n_theta, n_phi) = (16, 32);
        let d_theta = std::f32::consts::FRAC_PI_2 / n_theta as f32;
        let d_phi = 2.0 * std::f32::consts::PI / n_phi as f32;
        let mut sky_on_ground = Vec3::ZERO;
        for i in 0..n_theta {
            for j in 0..n_phi {
                let theta = (i as f32 + 0.5) * d_theta;
                let phi = (j as f32 + 0.5) * d_phi;
                let d = Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.cos(),
                    theta.sin() * phi.sin(),
                );
                sky_on_ground += self.radiance(d) * d.y * theta.sin() * d_theta * d_phi;
            }
        }
        let sun_on_ground = sun.color * sun.illuminance * sun.direction.y.max(0.0);
        (sky_on_ground * self.day + sun_on_ground) * GROUND_ALBEDO / std::f32::consts::PI
    }

    /// Irradiance of the sky (plus a grey ground bouncing sun and sky) as L2
    /// spherical harmonics, already convolved with the clamped cosine
    /// (Ramamoorthi & Hanrahan 2001: A0 = pi, A1 = 2pi/3, A2 = pi/4), in
    /// cd/m2 * sr = lux. `night` is added as uniform radiance.
    pub fn irradiance_sh(&self, sun: &Sun, night: Vec3) -> [Vec3; 9] {
        let (n_theta, n_phi) = (32, 64);
        let d_theta = std::f32::consts::PI / n_theta as f32;
        let d_phi = 2.0 * std::f32::consts::PI / n_phi as f32;
        let dirs = (0..n_theta).flat_map(|i| {
            (0..n_phi).map(move |j| {
                let theta = (i as f32 + 0.5) * d_theta;
                let phi = (j as f32 + 0.5) * d_phi;
                let dir = Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.cos(),
                    theta.sin() * phi.sin(),
                );
                (dir, theta.sin() * d_theta * d_phi)
            })
        });
        let ground = self.ground_radiance(sun);
        let mut sh = [Vec3::ZERO; 9];
        for (d, w) in dirs {
            let l = if d.y > 0.0 {
                self.radiance(d) * self.day
            } else {
                ground
            } + night;
            for (c, y) in sh.iter_mut().zip(sh_basis(d)) {
                *c += l * y * w;
            }
        }
        let band = [
            std::f32::consts::PI,
            2.0 * std::f32::consts::PI / 3.0,
            std::f32::consts::FRAC_PI_4,
        ];
        for (i, c) in sh.iter_mut().enumerate() {
            *c *= band[[0, 1, 1, 1, 2, 2, 2, 2, 2][i]];
        }
        sh
    }
}

/// Real SH basis up to l = 2, in the order sky/scene.wgsl evaluates it.
pub fn sh_basis(n: Vec3) -> [f32; 9] {
    [
        0.282095,
        0.488603 * n.y,
        0.488603 * n.z,
        0.488603 * n.x,
        1.092548 * n.x * n.y,
        1.092548 * n.y * n.z,
        0.315392 * (3.0 * n.z * n.z - 1.0),
        1.092548 * n.x * n.z,
        0.546274 * (n.x * n.x - n.y * n.y),
    ]
}

pub fn sh_eval(sh: &[Vec3; 9], n: Vec3) -> Vec3 {
    sh.iter().zip(sh_basis(n)).map(|(c, y)| *c * y).sum()
}

/// Daylight factor of the sky for the sun's elevation in degrees: 1 above
/// the horizon, then falling by a factor of 10 per 3 degrees of depression
/// (outdoor illuminance drops from ~400 lux at sunset to a few lux at the
/// end of civil twilight, -6 degrees, i.e. about two decades in 6 degrees),
/// faded out completely by -12 degrees, where the night floor remains.
pub fn twilight(elevation_deg: f32) -> f32 {
    if elevation_deg >= 0.0 {
        return 1.0;
    }
    10f32.powf(elevation_deg / 3.0) * smoothstep(-12.0, -4.0, elevation_deg)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_gets_brighter_towards_the_sun() {
        for elevation in [60.0, 30.0, 10.0, 3.0] {
            let sun = Sun::from_angles(200.0, elevation);
            let sky = Sky::new(sun.direction);
            // Walk along the great circle at 10 degrees elevation from the
            // anti-sun azimuth to the sun's azimuth (past the darkest point
            // at ~90 degrees from the sun, luminance rises monotonically).
            let el = 10f32.to_radians();
            let mut last = 0.0;
            for step in 0..=18 {
                let az = (200.0 + 90.0 - step as f32 * 5.0).to_radians();
                let dir = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
                let y = luminance(sky.radiance(dir));
                assert!(
                    y >= last,
                    "elevation {elevation}: step {step} luminance {y} < {last}"
                );
                last = y;
            }
            assert!(last > 0.0);
        }
    }

    #[test]
    fn zenith_luminance_is_plausible() {
        // Clear sky zenith at 30 deg sun elevation: a few kcd/m2.
        let sky = Sky::new(Sun::from_angles(180.0, 30.0).direction);
        let y = luminance(sky.radiance(Vec3::Y));
        assert!((1000.0..15000.0).contains(&y), "zenith {y} cd/m2");
        let blue = sky.radiance(Vec3::Y);
        assert!(blue.z > blue.x, "zenith should be blue: {blue}");
    }

    #[test]
    fn sun_dims_and_reddens_towards_the_horizon() {
        let noon = Sun::from_angles(180.0, 60.0);
        let evening = Sun::from_angles(240.0, 8.0);
        assert!(
            (80_000.0..120_000.0).contains(&noon.illuminance),
            "{}",
            noon.illuminance
        );
        assert!(evening.illuminance < noon.illuminance * 0.6);
        assert!(evening.color.x > evening.color.z * 1.5, "{}", evening.color);
        assert!((luminance(evening.color) - 1.0).abs() < 1e-4);
        assert!(Sun::from_angles(0.0, -5.0).illuminance == 0.0);
        // Azimuth 180 is south (+Z), 90 is east (+X).
        assert!(Sun::from_angles(180.0, 0.0).direction.z > 0.99);
        assert!(Sun::from_angles(90.0, 0.0).direction.x > 0.99);
    }

    #[test]
    fn ambient_is_bluish_from_above_and_brighter_than_from_below() {
        let sun = Sun::from_angles(180.0, 45.0);
        let sh = Sky::new(sun.direction).irradiance_sh(&sun, Vec3::ZERO);
        let up = sh_eval(&sh, Vec3::Y);
        let down = sh_eval(&sh, Vec3::NEG_Y);
        assert!(up.z > up.x, "sky irradiance should be blue: {up}");
        assert!(luminance(up) > luminance(down));
        // Diffuse sky on a horizontal surface: 5-40 klx on a clear day.
        assert!((5_000.0..40_000.0).contains(&luminance(up)), "{up}");
    }

    #[test]
    fn night_fades_in_below_the_horizon() {
        assert!(Sky::new(Sun::from_angles(0.0, 20.0).direction).day > 0.99);
        assert!(Sky::new(Sun::from_angles(0.0, -8.0).direction).day < 0.01);
    }
}
