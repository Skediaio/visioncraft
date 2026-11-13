//! Photographic skies: equirectangular HDR images (for example Poly Haven's
//! 1k Radiance files, decoded by the caller to linear f32 RGB) as background,
//! environment cube and ambient light, and a CPU estimate of the sun they
//! show.
//!
//! Convention (sky.wgsl `equirect_uv`): row 0 is the zenith, the image centre
//! (u = 0.5) looks north (-Z), u = 0.75 east (+X), so a compass azimuth `a`
//! (as in `Sun::from_angles`) is at u = 0.5 + a / 360. `rotation_deg` turns
//! the image clockwise seen from above (adds to every azimuth).

use glam::Vec3;

use crate::sky::luminance;

/// Linear RGB, three f32 per pixel, rows from the zenith down.
#[derive(Clone, Copy, Debug)]
pub struct HdriImage<'a> {
    pub width: u32,
    pub height: u32,
    pub rgb: &'a [f32],
}

impl HdriImage<'_> {
    fn check(&self) -> Result<(), String> {
        let n = self.width as usize * self.height as usize * 3;
        if self.width < 2 || self.height < 1 || self.rgb.len() != n {
            return Err(format!(
                "HDRI {}x{} needs {n} floats, got {}",
                self.width,
                self.height,
                self.rgb.len()
            ));
        }
        Ok(())
    }

    fn pixel(&self, x: u32, y: u32) -> Vec3 {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        Vec3::new(self.rgb[i], self.rgb[i + 1], self.rgb[i + 2])
    }
}

/// World direction through the centre of pixel (x, y) for rotation 0.
fn direction(w: u32, h: u32, x: u32, y: u32, rotation_deg: f32) -> Vec3 {
    let azimuth =
        ((x as f32 + 0.5) / w as f32 - 0.5) * std::f32::consts::TAU + rotation_deg.to_radians();
    let theta = (y as f32 + 0.5) / h as f32 * std::f32::consts::PI;
    Vec3::new(
        theta.sin() * azimuth.sin(),
        theta.cos(),
        -theta.sin() * azimuth.cos(),
    )
}

/// Solid angle of a pixel in row `y`.
fn solid_angle(w: u32, h: u32, y: u32) -> f32 {
    let theta = (y as f32 + 0.5) / h as f32 * std::f32::consts::PI;
    theta.sin() * (std::f32::consts::PI / h as f32) * (std::f32::consts::TAU / w as f32)
}

/// The sun in an HDRI: (unit direction towards it, illuminance it adds on a
/// surface facing it, linear colour normalised to luminance 1), in the
/// image's own units (multiply the illuminance by the `intensity` given to
/// `Sky::Hdri` for lux). Found as the brightest 3x3 neighbourhood; the sun is
/// what exceeds the sky around it (a 4 to 8 degree ring) within 4 degrees of
/// that peak. `None` for an image without a clear peak (overcast). An image
/// whose sun is clipped (low dynamic range) gives too low an illuminance.
pub fn estimate_sun(image: &HdriImage, rotation_deg: f32) -> Option<(Vec3, f32, Vec3)> {
    image.check().ok()?;
    let (w, h) = (image.width, image.height);
    let lum = |x: u32, y: u32| luminance(image.pixel(x, y));
    let mut peak = (0.0, 0, 0);
    for y in 1..h.saturating_sub(1) {
        for x in 0..w {
            let mut s = 0.0;
            for dy in 0..3 {
                for dx in 0..3 {
                    s += lum((x + w + dx - 1) % w, y + dy - 1);
                }
            }
            if s > peak.0 {
                peak = (s, x, y);
            }
        }
    }
    let centre = direction(w, h, peak.1, peak.2, 0.0);
    let (near, far) = (4f32.to_radians().cos(), 8f32.to_radians().cos());
    let (mut ring, mut ring_w) = (Vec3::ZERO, 0.0);
    let mut inside = Vec::new();
    for y in 0..h {
        let d_omega = solid_angle(w, h, y);
        for x in 0..w {
            let d = direction(w, h, x, y, 0.0);
            let c = d.dot(centre);
            if c >= near {
                inside.push((d, image.pixel(x, y), d_omega));
            } else if c >= far {
                ring += image.pixel(x, y) * d_omega;
                ring_w += d_omega;
            }
        }
    }
    let background = ring / ring_w.max(1e-12);
    let (mut energy, mut weighted) = (Vec3::ZERO, Vec3::ZERO);
    for (d, c, d_omega) in inside {
        let excess = (c - background).max(Vec3::ZERO) * d_omega;
        energy += excess;
        weighted += d * luminance(excess);
    }
    let illuminance = luminance(energy);
    // A sun stands out from its surroundings by orders of magnitude.
    if illuminance <= 0.0 || peak.0 / 9.0 < 20.0 * luminance(background) {
        return None;
    }
    let dir = weighted.normalize();
    let rotated = glam::Quat::from_rotation_y(-rotation_deg.to_radians()) * dir;
    Some((rotated, illuminance, energy / illuminance))
}

/// RGBA16F texels for the GPU (alpha 1) with the luminance of every pixel
/// clamped at 50x the mean of the upper hemisphere: the sun (and its
/// direct light) comes from the analytic sun instead, so the environment
/// and the ambient must not count it again. Also returns that mean (linear
/// RGB, image units), for the ground's sky light.
pub(crate) fn prepare(image: &HdriImage) -> Result<(Vec<u16>, Vec3), String> {
    image.check()?;
    let (w, h) = (image.width, image.height);
    let (mut sum, mut weight) = (Vec3::ZERO, 0.0);
    for y in 0..h / 2 {
        let d_omega = solid_angle(w, h, y);
        for x in 0..w {
            sum += image.pixel(x, y) * d_omega;
            weight += d_omega;
        }
    }
    let mean_raw = sum / weight.max(1e-12);
    let limit = (50.0 * luminance(mean_raw)).clamp(1e-6, 60000.0);
    let mut texels = Vec::with_capacity((w * h * 4) as usize);
    let (mut sum, mut weight) = (Vec3::ZERO, 0.0);
    for y in 0..h {
        let d_omega = solid_angle(w, h, y);
        for x in 0..w {
            let c = image.pixel(x, y).max(Vec3::ZERO);
            let l = luminance(c);
            let c = if l > limit { c * (limit / l) } else { c };
            if y < h / 2 {
                sum += c * d_omega;
                weight += d_omega;
            }
            texels.extend([f16_bits(c.x), f16_bits(c.y), f16_bits(c.z), f16_bits(1.0)]);
        }
    }
    Ok((texels, sum / weight.max(1e-12)))
}

/// IEEE 754 binary16 bits of an f32, rounded to nearest even (overflow
/// becomes infinity, tiny values subnormals or zero).
pub(crate) fn f16_bits(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    let round = |value: u32, rem: u32, half: u32| {
        if rem > half || (rem == half && value & 1 == 1) {
            value + 1
        } else {
            value
        }
    };
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let value = m >> shift;
        return sign | round(value, m & ((1 << shift) - 1), 1 << (shift - 1)) as u16;
    }
    let value = ((e as u32) << 10) | (man >> 13);
    sign | round(value, man & 0x1fff, 0x1000) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1e6), 0x7c00);
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(5.960_464_5e-8), 1, "smallest subnormal");
        assert_eq!(f16_bits(1.0 + 1.0 / 2048.0), 0x3c00, "ties to even");
        assert_eq!(f16_bits(1.0 + 3.0 / 2048.0), 0x3c02);
    }

    /// A synthetic sky: blue-white gradient plus a 0.6 degree sun disc of
    /// known radiance at azimuth 120, elevation 35.
    #[test]
    fn finds_the_sun_in_a_synthetic_hdri() {
        let (w, h) = (1024u32, 512u32);
        let sun_dir = {
            let (az, el) = (120f32.to_radians(), 35f32.to_radians());
            Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos())
        };
        let radius = 0.6f32.to_radians();
        let sun_radiance = Vec3::new(1.0, 0.9, 0.75) * 40_000.0;
        let mut rgb = Vec::new();
        let mut expected = 0.0;
        for y in 0..h {
            for x in 0..w {
                let d = direction(w, h, x, y, 0.0);
                let sky = Vec3::new(0.3, 0.5, 1.0) * (0.5 + 0.5 * d.y.max(0.0));
                let c = if d.dot(sun_dir) >= radius.cos() {
                    expected += luminance(sun_radiance) * solid_angle(w, h, y);
                    sky + sun_radiance
                } else {
                    sky
                };
                rgb.extend(c.to_array());
            }
        }
        let image = HdriImage {
            width: w,
            height: h,
            rgb: &rgb,
        };
        let (dir, e, color) = estimate_sun(&image, 0.0).expect("a sun");
        assert!(
            dir.dot(sun_dir) > 1f32.to_radians().cos(),
            "{dir} vs {sun_dir}"
        );
        assert!((e / expected - 1.0).abs() < 0.1, "{e} vs {expected}");
        assert!(
            color.x > color.z && (luminance(color) - 1.0).abs() < 1e-3,
            "{color}"
        );
        // Rotating the image by 30 degrees turns the sun with it.
        let (rotated, ..) = estimate_sun(&image, 30.0).expect("a sun");
        let expect = glam::Quat::from_rotation_y(-30f32.to_radians()) * sun_dir;
        assert!(rotated.dot(expect) > 0.9998, "{rotated} vs {expect}");
        // The prepared texture clamps the sun but keeps the sky.
        let (texels, mean) = prepare(&image).expect("prepare");
        assert_eq!(texels.len(), (w * h * 4) as usize);
        assert!(mean.z > mean.x && mean.z < 1.5, "{mean}");
        // An overcast sky has no sun.
        let flat = vec![1.0; (w * h * 3) as usize];
        let overcast = HdriImage {
            rgb: &flat,
            ..image
        };
        assert!(estimate_sun(&overcast, 0.0).is_none());
    }
}
