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

/// The camera's white balance (`Renderer::set_white_balance`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum WhiteBalance {
    /// Fixed for daylight: the picture as before white balance existed (nothing is applied).
    #[default]
    Daylight,
    /// From the light falling on open ground (the sun and the sky, tonemap.wgsl
    /// `scene_white`), adapted by `AUTO_ADAPTATION`.
    Auto,
    /// Light of this colour temperature (kelvin) reads white.
    Kelvin(f32),
}

/// A colour grade before the tone map (`Renderer::set_color_grade`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorGrade {
    #[default]
    Neutral,
    Warm,
    Cool,
    Soft,
}

/// How much of the scene light's cast automatic white balance takes out (von Kries degree of
/// adaptation): cameras leave some of it, so a sunset stays warm and an overcast day a little
/// cool. The research report proposes 70-90 %; the low end keeps the most mood.
pub const AUTO_ADAPTATION: f32 = 0.7;

/// Linear Rec.709 to CIE XYZ (D65).
pub const RGB_TO_XYZ: Mat3 = Mat3::from_cols(
    Vec3::new(0.4124, 0.2126, 0.0193),
    Vec3::new(0.3576, 0.7152, 0.1192),
    Vec3::new(0.1805, 0.0722, 0.9505),
);
/// CIE XYZ to linear Rec.709 (D65).
pub const XYZ_TO_RGB: Mat3 = Mat3::from_cols(
    Vec3::new(3.2406, -0.9689, 0.0557),
    Vec3::new(-1.5372, 1.8758, -0.2040),
    Vec3::new(-0.4986, 0.0415, 1.0570),
);
/// The Bradford cone response (Lam 1985, as in CIECAM97s): XYZ to the sharpened LMS in which
/// von Kries scaling is done.
pub const BRADFORD: Mat3 = Mat3::from_cols(
    Vec3::new(0.8951, -0.7502, 0.0389),
    Vec3::new(0.2664, 1.7135, -0.0685),
    Vec3::new(-0.1614, 0.0367, 1.0296),
);

/// The chromaticity (x, y) of a black body at `kelvin` (1667..25000 K): the cubic spline fit of
/// Kim et al., "Design of Advanced Color Temperature Control System for HDTV Applications",
/// Journal of the Korean Physical Society 41(6), 2002.
pub fn planck_xy(kelvin: f32) -> (f32, f32) {
    let t = f64::from(kelvin.clamp(1667.0, 25000.0));
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t + 0.240_390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.106_381_4 * x3 - 1.348_110_20 * x2 + 2.185_558_32 * x - 0.202_196_83
    } else if t <= 4000.0 {
        -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x - 0.370_014_83
    };
    (x as f32, y as f32)
}

/// A black body's colour at `kelvin` in linear Rec.709, scaled to a luminance of 1.
pub fn kelvin_white(kelvin: f32) -> Vec3 {
    let (x, y) = planck_xy(kelvin);
    XYZ_TO_RGB * Vec3::new(x / y, 1.0, (1.0 - x - y) / y)
}

/// The colour temperature a fixed white balance refers to: a black body at the temperature of
/// D65, so `Kelvin(6504)` changes nothing.
pub const DAYLIGHT_KELVIN: f32 = 6504.0;

/// Von Kries adaptation in Bradford space: a colour seen under `white` as it would look under
/// `target`, `degree` of the way (0 none, 1 full). tonemap.wgsl `adapt` is the same.
pub fn adapt(c: Vec3, white: Vec3, target: Vec3, degree: f32) -> Vec3 {
    let to = BRADFORD * RGB_TO_XYZ;
    let lms_w = to * white.max(Vec3::splat(1e-6));
    let lms_t = to * target;
    let gain = Vec3::ONE + (lms_t / lms_w - Vec3::ONE) * degree;
    to.inverse() * (gain * (to * c))
}

/// A grade's matrix (linear Rec.709, columns) and its contrast exponent about mid grey; the
/// identity and 1 for `Neutral`. tonemap.wgsl `color_grade` applies it.
pub fn grade_matrix(grade: ColorGrade) -> (Mat3, f32) {
    // Gains per channel (normalised to keep the luminance), saturation, contrast.
    let (gain, saturation, contrast) = match grade {
        ColorGrade::Neutral => return (Mat3::IDENTITY, 1.0),
        ColorGrade::Warm => (Vec3::new(1.07, 1.0, 0.88), 1.06, 1.0),
        ColorGrade::Cool => (Vec3::new(0.93, 1.0, 1.10), 0.96, 1.0),
        ColorGrade::Soft => (Vec3::ONE, 0.86, 0.86),
    };
    let luma = Vec3::new(0.2126, 0.7152, 0.0722);
    let gain = gain / gain.dot(luma);
    // Saturation about the luminance: s I + (1 - s) * (rows of luma weights).
    let sat = Mat3::from_diagonal(Vec3::splat(saturation))
        + Mat3::from_cols(luma, luma, luma).transpose() * (1.0 - saturation);
    (sat * Mat3::from_diagonal(gain), contrast)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_balance_and_grades() {
        // The black body at D65's temperature is close to D65 (white).
        let d65 = kelvin_white(DAYLIGHT_KELVIN);
        assert!((d65 - Vec3::ONE).abs().max_element() < 0.05, "{d65}");
        // Lower is warmer.
        let tungsten = kelvin_white(3200.0);
        assert!(
            tungsten.x > tungsten.y && tungsten.y > tungsten.z,
            "{tungsten}"
        );
        let shade = kelvin_white(9000.0);
        assert!(shade.z > shade.x, "{shade}");
        // Full adaptation takes the white to the target; none leaves it.
        let w = Vec3::new(1.3, 1.0, 0.6);
        let out = adapt(w, w, Vec3::ONE, 1.0);
        assert!((out - Vec3::ONE).abs().max_element() < 1e-4, "{out}");
        assert!((adapt(w, w, Vec3::ONE, 0.0) - w).abs().max_element() < 1e-5);
        // Partial: still warm, less so.
        let part = adapt(w, w, Vec3::ONE, AUTO_ADAPTATION);
        assert!(part.x > part.z && part.x / part.z < w.x / w.z, "{part}");
        // A 3200 K balance on daylight turns it blue.
        let blue = adapt(
            Vec3::ONE,
            tungsten / tungsten.dot(Vec3::new(0.2126, 0.7152, 0.0722)),
            d65,
            1.0,
        );
        assert!(blue.z > blue.x, "{blue}");
        // Neutral is the identity; the others keep grey grey and its luminance.
        assert_eq!(grade_matrix(ColorGrade::Neutral), (Mat3::IDENTITY, 1.0));
        for g in [ColorGrade::Warm, ColorGrade::Cool, ColorGrade::Soft] {
            let (m, _) = grade_matrix(g);
            let grey = m * Vec3::splat(0.18);
            let y = grey.dot(Vec3::new(0.2126, 0.7152, 0.0722));
            assert!((y - 0.18).abs() < 1e-4, "{g:?} {grey}");
        }
        let (warm, _) = grade_matrix(ColorGrade::Warm);
        let c = warm * Vec3::splat(0.18);
        assert!(c.x > c.z);
    }

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
