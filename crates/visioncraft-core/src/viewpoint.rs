//! Saved camera positions.

use crate::project::{SunTime, YEARS_MAX};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewpoint {
    pub id: String,
    pub name: String,
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub fov_y_deg: f32,
    /// The sun to show from here; `None` keeps the project's time.
    pub time: Option<SunTime>,
    /// The years after planting to show from here (`ViewState::years`); `None` keeps the
    /// project's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub years: Option<f32>,
}

impl Viewpoint {
    pub fn validate(&self) -> Result<(), String> {
        let finite = self
            .position
            .iter()
            .chain(&self.target)
            .all(|c| c.is_finite());
        if !finite
            || self.position == self.target
            || !(1.0..=179.0).contains(&self.fov_y_deg)
            || self.years.is_some_and(|y| !(0.0..=YEARS_MAX).contains(&y))
        {
            return Err(
                "position and target must be finite and differ, field of view 1..179 degrees, years 0..60"
                    .into(),
            );
        }
        Ok(())
    }
}

/// Camera position and target `t` of the way from `a` to `b` (`t` clamped to 0..1), eased in and
/// out: it starts and ends slowly.
pub fn interpolate(a: &Viewpoint, b: &Viewpoint, t: f32) -> ([f32; 3], [f32; 3]) {
    let t = t.clamp(0.0, 1.0);
    let e = if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (2.0 - 2.0 * t).powi(2) / 2.0
    };
    let lerp = |x: [f32; 3], y: [f32; 3]| std::array::from_fn(|k| x[k] + (y[k] - x[k]) * e);
    (lerp(a.position, b.position), lerp(a.target, b.target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(x: f32) -> Viewpoint {
        Viewpoint {
            id: "v".into(),
            name: "v".into(),
            position: [x, 1.6, 0.0],
            target: [x, 1.6, -5.0],
            fov_y_deg: 60.0,
            time: None,
            years: None,
        }
    }

    #[test]
    fn hits_both_ends_and_eases() {
        let (a, b) = (vp(0.0), vp(10.0));
        assert_eq!(interpolate(&a, &b, 0.0), (a.position, a.target));
        assert_eq!(interpolate(&a, &b, 1.0), (b.position, b.target));
        assert!((interpolate(&a, &b, 0.5).0[0] - 5.0).abs() < 1e-6);
        assert!(interpolate(&a, &b, 0.25).0[0] < 2.5);
    }
}
