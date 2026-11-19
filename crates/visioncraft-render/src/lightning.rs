//! Lightning in storm clouds: when a flash happens, where, and whether a bolt is seen, all from
//! the animation clock alone, so a film shows the same flashes every time it is drawn.
//!
//! Time is cut into slots; a hash of the slot's number says whether it holds a flash (more
//! often with a higher frequency: the flashes of a lower one stay, more are added), when in the
//! slot it starts, where it is, and how many strokes it has. A flash is a few strokes
//! (Rakov & Uman, "Lightning: Physics and Effects", 2003: a ground flash has three to five
//! return strokes some 60 ms apart, each lighting the channel for a few tens of
//! milliseconds), so it flickers over a few frames.

/// A flash's light in the cloud at full strength (kcd/m2 of the cloud right at it; clouds.wgsl
/// spreads it through the cloud).
pub(crate) const FLASH_KCD: f32 = 60.0;
/// Its light on the scene falls off with the flash's distance past this (m).
const FILL_M: f32 = 6000.0;

/// The length of one slot (s).
const SLOT_S: f64 = 1.5;
/// The chance of a flash in a slot at frequency 1.
const MOST_PER_SLOT: f32 = 0.3;
/// A stroke's light decays with this time constant (s), and a quarter of it, the current that
/// goes on flowing in the channel, with the slower one.
const STROKE_DECAY_S: f32 = 0.06;
const AFTERGLOW_S: f32 = 0.15;
/// A stroke is gone after this long (s).
const STROKE_S: f32 = 0.4;

/// One flash at the moment asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Flash {
    /// How bright it is now, 0..about 1.
    pub strength: f32,
    /// Its direction from the camera (rad about the vertical, from world x towards z).
    pub azimuth: f32,
    /// Its horizontal distance from the camera (m).
    pub distance_m: f32,
    /// Its height in the cloud layer, 0 (base) .. 1 (top).
    pub height: f32,
    /// A bolt to the ground is seen, drawn from this seed.
    pub bolt: Option<u32>,
}

fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747796405).wrapping_add(2891336453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277803737);
    (word >> 22) ^ word
}

/// 0..1 from a hash.
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

/// The flash in the slot `slot` at `frequency` (0..1), at `since` seconds after the slot's start.
fn in_slot(slot: i64, since: f32, frequency: f32) -> Option<Flash> {
    let h = pcg(slot as u32 ^ pcg((slot >> 32) as u32 ^ 0x51ed_270b));
    let r = |k: u32| unit(pcg(h.wrapping_add(k)));
    if r(0) >= frequency.clamp(0.0, 1.0) * MOST_PER_SLOT {
        return None;
    }
    let start = r(1) * 0.5 * SLOT_S as f32;
    let strokes = 1 + (pcg(h ^ 0x2545_f491) % 4);
    let mut at = start;
    let mut strength = 0.0;
    for k in 0..strokes {
        let amp = if k == 0 { 1.0 } else { 0.45 + 0.4 * r(10 + k) };
        let dt = since - at;
        if (0.0..STROKE_S).contains(&dt) {
            strength += amp * ((-dt / STROKE_DECAY_S).exp() + 0.25 * (-dt / AFTERGLOW_S).exp());
        }
        at += 0.04 + 0.08 * r(20 + k);
    }
    if strength <= 1e-3 {
        return None;
    }
    let distance_m = 1500.0 + 11000.0 * r(2) * r(2);
    Some(Flash {
        strength: strength.min(1.6),
        azimuth: r(3) * std::f32::consts::TAU,
        distance_m,
        height: 0.1 + 0.4 * r(4),
        bolt: (r(5) < 0.5 && distance_m > 3000.0).then(|| pcg(h ^ 0x68e3_1da4)),
    })
}

/// A flash's light on the scene, as the radiance it adds to the sky over it (kcd/m2; common.wgsl
/// `flash_sky`), with the sun's illuminance on level ground over the clouds `sun_lux`: at full
/// strength some twice the light of a storm sky by day (which lets through about a twentieth
/// of the sun's), and about a hundred lux at night, so a flash shows in a dark
/// afternoon's storm as in a night's; less the further off it is.
pub(crate) fn fill_kcd(flash: &Flash, sun_lux: f32) -> f32 {
    let near = 1.0 / (1.0 + (flash.distance_m / FILL_M).powi(2));
    flash.strength * near * (0.03 + 2.0 * 0.05 * sun_lux / 1000.0 / std::f32::consts::PI)
}

/// The flash lit at `t` seconds of the animation clock in a storm with lightning at `frequency`
/// (0..1), the brightest when two overlap; `None` between flashes.
pub(crate) fn flash_at(t: f64, frequency: f32) -> Option<Flash> {
    if frequency <= 0.0 || !t.is_finite() {
        return None;
    }
    let slot = (t / SLOT_S).floor();
    [0.0, 1.0]
        .into_iter()
        .filter_map(|back| {
            let s = slot - back;
            in_slot(s as i64, (t - s * SLOT_S) as f32, frequency)
        })
        .max_by(|a, b| a.strength.total_cmp(&b.strength))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(frequency: f32) -> Vec<f64> {
        (0..60 * 50)
            .map(|i| f64::from(i) * 0.02)
            .filter(|&t| flash_at(t, frequency).is_some_and(|f| f.strength > 0.2))
            .collect()
    }

    #[test]
    fn flashes_follow_the_clock_alone() {
        for t in [0.0, 3.3, 47.81, 1.0e6] {
            assert_eq!(flash_at(t, 0.7), flash_at(t, 0.7));
        }
        assert!(lit(0.0).is_empty());
    }

    #[test]
    fn a_higher_frequency_keeps_the_flashes_and_adds_more() {
        let few = lit(0.3);
        let many = lit(1.0);
        assert!(!few.is_empty(), "a minute of storm at 0.3 has flashes");
        assert!(many.len() > few.len());
        assert!(few.iter().all(|t| many.contains(t)));
        // A flash lasts a few frames, not the whole minute.
        assert!(many.len() < 60 * 50 / 4, "{}", many.len());
    }

    #[test]
    fn a_flash_flickers_and_is_over_within_its_slot() {
        let t = lit(1.0)[0];
        let f = flash_at(t, 1.0).unwrap();
        assert!(f.distance_m >= 1500.0 && f.distance_m <= 12500.0);
        assert!((0.1..=0.5).contains(&f.height));
        let mut flashes = 0;
        for slot in 0..2000 {
            let s = (0..75).map(|k| in_slot(slot, k as f32 * 0.02, 1.0).map(|f| f.strength));
            let strengths: Vec<f32> = s.flatten().collect();
            if strengths.is_empty() {
                continue;
            }
            flashes += 1;
            // Lit for a few frames of a film, not more than a second.
            assert!(strengths.len() < 50, "{}", strengths.len());
            assert!(in_slot(slot, SLOT_S as f32, 1.0).is_none());
        }
        assert!((400..800).contains(&flashes), "{flashes}");
    }
}
