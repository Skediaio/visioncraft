//! CPU copy of tonemap.wgsl's AgX, for unit tests (and for anyone who wants
//! the curve on the CPU). Same constants, same order of operations; the
//! sources are cited in tonemap.wgsl.

use glam::{Mat3, Vec3};

const SRGB_TO_REC2020: Mat3 = Mat3::from_cols(
    Vec3::new(0.6274, 0.0691, 0.0164),
    Vec3::new(0.3293, 0.9195, 0.0880),
    Vec3::new(0.0433, 0.0113, 0.8956),
);
const REC2020_TO_SRGB: Mat3 = Mat3::from_cols(
    Vec3::new(1.6605, -0.1246, -0.0182),
    Vec3::new(-0.5876, 1.1329, -0.1006),
    Vec3::new(-0.0728, -0.0083, 1.1187),
);
// Digit for digit as in the reference, so it can be checked by eye.
#[allow(clippy::excessive_precision)]
const AGX_INSET: Mat3 = Mat3::from_cols(
    Vec3::new(0.856_627_15, 0.137_318_97, 0.111_898_21),
    Vec3::new(0.095_121_24, 0.761_241_99, 0.076_799_42),
    Vec3::new(0.048_251_6, 0.101_439_04, 0.811_302_4),
);
#[allow(clippy::excessive_precision)]
const AGX_OUTSET: Mat3 = Mat3::from_cols(
    Vec3::new(1.127_100_6, -0.141_329_76, -0.141_329_76),
    Vec3::new(-0.110_606_64, 1.157_823_7, -0.110_606_64),
    Vec3::new(-0.016_493_939, -0.016_493_939, 1.251_936_4),
);
const AGX_MIN_EV: f32 = -12.47393;
const AGX_MAX_EV: f32 = 4.026069;

fn contrast(x: f32) -> f32 {
    let x2 = x * x;
    let x4 = x2 * x2;
    15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x
        - 0.00232
}

/// AgX: pre-exposed linear Rec.709 in, linear Rec.709 display value out.
pub fn agx(color: Vec3) -> Vec3 {
    let c = AGX_INSET * (SRGB_TO_REC2020 * color);
    let log = Vec3::from(c.max(Vec3::splat(1e-10)).to_array().map(f32::log2));
    let c = (log - Vec3::splat(AGX_MIN_EV)) / (AGX_MAX_EV - AGX_MIN_EV);
    let c = c.clamp(Vec3::ZERO, Vec3::ONE).to_array().map(contrast);
    let c = (AGX_OUTSET * Vec3::from(c)).max(Vec3::ZERO).powf(2.2);
    (REC2020_TO_SRGB * c).clamp(Vec3::ZERO, Vec3::ONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agx_known_values() {
        // Middle grey: log encoding gives 10 / 16.5 = 0.6061, the sigmoid
        // fit gives 0.4968 there (worked by hand), and 0.4968^2.2 = 0.2146.
        let grey = agx(Vec3::splat(0.18));
        assert!((grey.x - 0.2146).abs() < 2e-3, "{grey}");
        assert!(
            (grey.x - grey.y).abs() < 1e-3 && (grey.y - grey.z).abs() < 1e-3,
            "grey stays grey: {grey}"
        );
        // Black stays (nearly) black; 2^6.5 * 0.18 and above saturate to white.
        assert!(agx(Vec3::ZERO).max_element() < 1e-3);
        assert!(agx(Vec3::splat(16.3)).min_element() > 0.97);
        // Monotonic in exposure.
        let mut last = 0.0;
        for i in 0..64 {
            let y = agx(Vec3::splat(2f32.powf(i as f32 * 0.25 - 10.0))).y;
            assert!(y >= last - 1e-5, "step {i}: {y} < {last}");
            last = y;
        }
        // Saturated colours desaturate towards white instead of clipping hue.
        let red = agx(Vec3::new(8.0, 0.0, 0.0));
        assert!(red.y > 0.05 && red.x > red.y, "{red}");
    }
}
