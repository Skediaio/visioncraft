//! Large water: the project's sea level (`water.set_sea`). One plane at `level` that fills
//! everything below it out to the horizon, or only inside `outline` (a lake or a wide river
//! drawn like an area). Terrain and the endless ground that rise above it cut it; the renderer
//! draws the waves from `wave_height_m` and `peak_wavelength_m`.

use serde::{Deserialize, Serialize};

/// What the water is: it sets how long the waves are for their height, whether it freezes and
/// whether it flows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaterKind {
    /// Open sea: long waves (a fully developed sea), never freezes.
    #[default]
    Sea,
    /// A lake: short, steep waves (little fetch), freezes when still in a snowy winter.
    Lake,
    /// A river: small waves carried along by the flow, never freezes.
    River,
}

/// The project's sea level and its look.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sea {
    /// Height of the mean water surface (m).
    pub level: f32,
    pub kind: WaterKind,
    /// WMO sea state code 0 (calm, glassy) to 6 (very rough), fractions allowed: the
    /// significant wave height (`wave_height_m`).
    pub sea_state: f32,
    /// Direction the waves travel towards, clockwise from north (deg); `None`: the wind's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction_deg: Option<f32>,
    /// Linear RGB of deep water.
    pub color: [f32; 3],
    /// 0 (murky: the bed vanishes within half a metre) to 1 (clear: seen through metres).
    pub clarity: f32,
    /// River flow: direction it runs towards (deg, as `direction_deg`) and speed (m/s).
    pub flow_deg: f32,
    pub flow_speed: f32,
    /// Foam on wave crests and along the shore, 0..1 (0.5 is natural for the sea state).
    pub foam: f32,
    /// Limits the water to this polygon (`[x, z]`, 3..=64 points); empty: to the horizon.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<[f32; 2]>,
}

/// How deep a person walks into the water (about knee-deep) before walking stops.
pub const WADE_DEPTH_M: f32 = 0.6;

/// The most points an outline may have (the renderer tests them per pixel).
pub const MAX_OUTLINE: usize = 64;

impl Default for Sea {
    fn default() -> Self {
        Sea {
            level: 0.0,
            kind: WaterKind::Sea,
            sea_state: 2.0,
            direction_deg: None,
            // A temperate coastal sea: dark blue-green.
            color: [0.01, 0.05, 0.07],
            clarity: 0.6,
            flow_deg: 0.0,
            flow_speed: 0.0,
            foam: 0.5,
            outline: Vec::new(),
        }
    }
}

impl Sea {
    pub fn validate(&self) -> Result<(), String> {
        let finite = |v: f32| v.is_finite();
        if !finite(self.level) || self.level.abs() > 10_000.0 {
            return Err("`level` must be a height in metres".into());
        }
        if !(0.0..=6.0).contains(&self.sea_state) {
            return Err("`sea_state` runs from 0 (calm) to 6 (very rough)".into());
        }
        if !self.direction_deg.is_none_or(finite) || !finite(self.flow_deg) {
            return Err("a direction is not a usable number".into());
        }
        if !(0.0..=10.0).contains(&self.flow_speed) {
            return Err("`flow_speed` runs from 0 to 10 m/s".into());
        }
        if !(0.0..=1.0).contains(&self.clarity) || !(0.0..=1.0).contains(&self.foam) {
            return Err("`clarity` and `foam` run from 0 to 1".into());
        }
        if !self.color.iter().all(|c| (0.0..=1.0).contains(c)) {
            return Err("`color` is linear RGB, each 0..1".into());
        }
        if !self.outline.is_empty()
            && (self.outline.len() < 3
                || self.outline.len() > MAX_OUTLINE
                || !self.outline.iter().flatten().all(|c| c.is_finite()))
        {
            return Err(format!(
                "an `outline` has 3 to {MAX_OUTLINE} finite points, or none for water to the horizon"
            ));
        }
        Ok(())
    }

    /// Significant wave height (m): the WMO sea state table's midpoints (code 1: 0-0.1 m, 2:
    /// 0.1-0.5, 3: 0.5-1.25, 4: 1.25-2.5, 5: 2.5-4, 6: 4-6), interpolated; a lake's waves stop at
    /// 1 m and a river's at 0.3 m.
    pub fn wave_height_m(&self) -> f32 {
        const MID: [f32; 7] = [0.02, 0.05, 0.3, 0.875, 1.875, 3.25, 5.0];
        let s = self.sea_state.clamp(0.0, 6.0);
        let i = (s.floor() as usize).min(5);
        let h = MID[i] + (MID[i + 1] - MID[i]) * (s - i as f32);
        match self.kind {
            WaterKind::Sea => h,
            WaterKind::Lake => h.min(1.0),
            WaterKind::River => h.min(0.3),
        }
    }

    /// Wavelength of the most energetic waves (m). A fully developed sea (Pierson-Moskowitz:
    /// H_s = 0.21 U^2 / g, peak frequency 0.877 g / U) has lambda_p = 2 pi H_s / (0.21 * 0.77),
    /// about 39 H_s; a lake's fetch keeps waves shorter and steeper, a river's shorter still.
    pub fn peak_wavelength_m(&self) -> f32 {
        let h = self.wave_height_m();
        match self.kind {
            WaterKind::Sea => (39.0 * h).max(3.0),
            WaterKind::Lake => (14.0 * h).max(1.5),
            WaterKind::River => (8.0 * h).max(0.8),
        }
    }

    /// Whether the surface freezes in a snowy winter: still lake water only.
    pub fn freezes(&self) -> bool {
        self.kind == WaterKind::Lake && self.sea_state <= 1.0
    }

    /// Whether the water covers the point (x, z) at all (inside the outline, if there is one).
    pub fn covers(&self, x: f32, z: f32) -> bool {
        self.outline.is_empty() || crate::shape::inside_outline([x, z], &self.outline)
    }

    /// Whether a person may stand at (x, z) on ground `y` high: dry, or water up to
    /// [`WADE_DEPTH_M`] (walking stops there).
    pub fn wadeable(&self, x: f32, z: f32, y: f32) -> bool {
        self.depth_over(x, z, y).is_none_or(|d| d <= WADE_DEPTH_M)
    }

    /// The water's depth over a ground point `y` high at (x, z): `None` when the point is dry.
    pub fn depth_over(&self, x: f32, z: f32, y: f32) -> Option<f32> {
        (self.covers(x, z) && y < self.level).then(|| self.level - y)
    }
}

/// Whether an asset is meant to sit in the water (a boat, a buoy, a floating platform): its
/// catalogue tags say so. The library has none yet.
pub fn floats(asset: &crate::Asset) -> bool {
    asset
        .tags
        .iter()
        .any(|t| matches!(t.as_str(), "boat" | "floating" | "buoy"))
}

/// Applies the sea to plant and object placements: what floats sits on the mean water level,
/// what would stand more than 5 cm under the water is left out. `floating` says whether an
/// asset id floats.
pub fn keep_dry(sea: Option<&Sea>, e: &mut crate::Evaluation, floating: &dyn Fn(&str) -> bool) {
    let Some(sea) = sea else { return };
    e.placements.retain_mut(|p| {
        let [x, y, z] = p.position;
        match sea.depth_over(x, z, y) {
            Some(_) if floating(&p.asset) => {
                p.position[1] = sea.level;
                true
            }
            Some(d) => d <= 0.05,
            None => true,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sea_states_map_to_the_wmo_heights_and_kinds_shorten_the_waves() {
        let sea = |s: f32, kind| Sea {
            sea_state: s,
            kind,
            ..Sea::default()
        };
        assert!((sea(3.0, WaterKind::Sea).wave_height_m() - 0.875).abs() < 1e-6);
        assert!((sea(3.5, WaterKind::Sea).wave_height_m() - 1.375).abs() < 1e-6);
        assert_eq!(sea(6.0, WaterKind::Sea).wave_height_m(), 5.0);
        assert_eq!(sea(6.0, WaterKind::Lake).wave_height_m(), 1.0);
        let l = |k| sea(3.0, k).peak_wavelength_m();
        assert!(l(WaterKind::Sea) > l(WaterKind::Lake) && l(WaterKind::Lake) > l(WaterKind::River));
        // Waves stay far from breaking everywhere: H / lambda under 1/7.
        for k in [WaterKind::Sea, WaterKind::Lake, WaterKind::River] {
            for s in 0..=6 {
                let w = sea(s as f32, k);
                assert!(
                    w.wave_height_m() / w.peak_wavelength_m() < 1.0 / 7.0,
                    "{k:?} {s}"
                );
            }
        }
        assert!(sea(0.5, WaterKind::Lake).freezes() && !sea(0.5, WaterKind::Sea).freezes());
    }

    #[test]
    fn validation_and_the_outline() {
        assert!(Sea::default().validate().is_ok());
        let bad = |s: Sea| assert!(s.validate().is_err());
        bad(Sea {
            sea_state: 7.0,
            ..Sea::default()
        });
        bad(Sea {
            outline: vec![[0.0, 0.0], [1.0, 0.0]],
            ..Sea::default()
        });
        bad(Sea {
            level: f32::NAN,
            ..Sea::default()
        });
        let lake = Sea {
            outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            level: 1.0,
            ..Sea::default()
        };
        assert!(lake.validate().is_ok());
        assert_eq!(lake.depth_over(5.0, 5.0, 0.2), Some(0.8));
        assert_eq!(lake.depth_over(15.0, 5.0, 0.2), None, "outside the lake");
        assert_eq!(lake.depth_over(5.0, 5.0, 1.5), None, "dry ground");
        assert!(lake.wadeable(5.0, 5.0, 0.5) && !lake.wadeable(5.0, 5.0, 0.2));
    }
}
