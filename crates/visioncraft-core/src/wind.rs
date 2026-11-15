//! Wind in real units: m/s, and the Beaufort scale (NOAA / WMO) as named presets.
//!
//! The scale's empirical relation is v = 0.836 B^1.5 m/s (WMO), so force 0 is exactly still air.
//! The land descriptions are the public ones from NOAA's scale, reworded.

use serde_json::Value;

/// What the old relative `wind_strength` 1 means in m/s (the middle of Beaufort 4). Projects
/// written before wind had units map through this on load.
pub const RELATIVE_MPS: f32 = 6.5;

/// Highest wind speed a view accepts (m/s): beyond a hurricane's.
pub const MAX_SPEED_MPS: f32 = 60.0;

/// One step of the Beaufort scale.
#[derive(Clone, Copy, Debug)]
pub struct Beaufort {
    pub force: u8,
    pub name: &'static str,
    /// What it does on land.
    pub on_land: &'static str,
}

/// Forces 0 to 10 (11 and 12 are hurricane-sea states, not garden weather).
pub const BEAUFORT: [Beaufort; 11] = [
    Beaufort {
        force: 0,
        name: "calm",
        on_land: "smoke rises vertically",
    },
    Beaufort {
        force: 1,
        name: "light air",
        on_land: "smoke drifts, wind vanes do not turn",
    },
    Beaufort {
        force: 2,
        name: "light breeze",
        on_land: "wind felt on the face, leaves rustle",
    },
    Beaufort {
        force: 3,
        name: "gentle breeze",
        on_land: "leaves and small twigs in constant motion",
    },
    Beaufort {
        force: 4,
        name: "moderate breeze",
        on_land: "dust and loose paper rise, small branches move",
    },
    Beaufort {
        force: 5,
        name: "fresh breeze",
        on_land: "small trees in leaf begin to sway",
    },
    Beaufort {
        force: 6,
        name: "strong breeze",
        on_land: "large branches in motion, umbrellas hard to hold",
    },
    Beaufort {
        force: 7,
        name: "near gale",
        on_land: "whole trees in motion, walking against the wind is hard",
    },
    Beaufort {
        force: 8,
        name: "gale",
        on_land: "twigs break off trees, progress is impeded",
    },
    Beaufort {
        force: 9,
        name: "strong gale",
        on_land: "slight structural damage: chimney pots and slates blown off",
    },
    Beaufort {
        force: 10,
        name: "storm",
        on_land: "trees uprooted, considerable structural damage",
    },
];

/// Wind speed (m/s) of a Beaufort force: v = 0.836 B^1.5 (WMO).
pub fn beaufort_mps(force: u8) -> f32 {
    0.836 * f32::from(force).powf(1.5)
}

/// The Beaufort step a wind speed (m/s) falls in, clamped to 0..=10.
pub fn beaufort_of(speed_mps: f32) -> Beaufort {
    let force = (speed_mps.max(0.0) / 0.836).powf(2.0 / 3.0).round();
    BEAUFORT[(force as usize).min(10)]
}

/// A Beaufort step from a command value: an integer 0..=10, or a step's name (`"gale"`).
pub fn parse_beaufort(value: &Value) -> Option<u8> {
    match value {
        Value::Number(n) => n
            .as_f64()
            .filter(|f| f.fract() == 0.0 && (0.0..=10.0).contains(f))
            .map(|f| f as u8),
        Value::String(s) => BEAUFORT
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(s.trim()))
            .map(|b| b.force),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn calm_is_still_and_the_scale_climbs() {
        assert_eq!(beaufort_mps(0), 0.0);
        // WMO's own midpoints: force 4 about 6.7 m/s, force 8 about 18.9 m/s.
        assert!((beaufort_mps(4) - 6.69).abs() < 0.01);
        assert!((beaufort_mps(8) - 18.9).abs() < 0.1);
        for b in 1..=10 {
            assert!(beaufort_mps(b) > beaufort_mps(b - 1));
            assert_eq!(beaufort_of(beaufort_mps(b)).force, b);
        }
    }

    #[test]
    fn steps_parse_by_number_and_name() {
        assert_eq!(parse_beaufort(&json!(5)), Some(5));
        assert_eq!(parse_beaufort(&json!("Gale")), Some(8));
        assert_eq!(parse_beaufort(&json!(11)), None);
        assert_eq!(parse_beaufort(&json!(2.5)), None);
        assert_eq!(parse_beaufort(&json!("hurricane")), None);
    }
}
