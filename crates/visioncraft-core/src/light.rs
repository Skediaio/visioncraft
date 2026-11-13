//! Lights and their colour.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LightKind {
    Point,
    Spot {
        /// Where it points, world axes; need not be unit length.
        direction: [f32; 3],
        inner_cone_deg: f32,
        outer_cone_deg: f32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Light {
    pub id: String,
    pub name: String,
    pub layer: String,
    pub kind: LightKind,
    pub position: [f32; 3],
    pub color_temperature_k: f32,
    pub lumens: f32,
    pub range_m: f32,
    pub enabled: bool,
}

/// What a catalogue fixture emits: a [`Light`] without id, name, layer and position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightSpec {
    pub kind: LightKind,
    pub color_temperature_k: f32,
    pub lumens: f32,
    pub range_m: f32,
}

impl LightSpec {
    /// The light of a fixture placed at `position` and turned `rotation_y_deg` about +Y (counter-
    /// clockwise from above, like a model's transform); a spot's direction turns with it.
    pub fn light(
        &self,
        (id, name, layer): (String, String, String),
        position: [f32; 3],
        rotation_y_deg: f32,
    ) -> Light {
        let mut kind = self.kind;
        if let LightKind::Spot { direction: d, .. } = &mut kind {
            let (s, c) = rotation_y_deg.to_radians().sin_cos();
            *d = [c * d[0] + s * d[2], d[1], -s * d[0] + c * d[2]];
        }
        Light {
            id,
            name,
            layer,
            kind,
            position,
            color_temperature_k: self.color_temperature_k,
            lumens: self.lumens,
            range_m: self.range_m,
            enabled: true,
        }
    }
}

impl LightKind {
    pub fn validate(&self) -> Result<(), String> {
        if let Self::Spot {
            direction,
            inner_cone_deg,
            outer_cone_deg,
        } = self
        {
            let len = direction.iter().map(|c| c * c).sum::<f32>();
            if !(len.is_finite() && len > 0.0)
                || !(0.0 <= *inner_cone_deg
                    && inner_cone_deg <= outer_cone_deg
                    && *outer_cone_deg <= 90.0)
            {
                return Err(
                    "a spot needs a direction and 0 <= inner <= outer <= 90 degrees".into(),
                );
            }
        }
        Ok(())
    }
}

impl Light {
    pub fn validate(&self) -> Result<(), String> {
        let finite = self.position.iter().all(|c| c.is_finite());
        if !finite
            || !(1000.0..=40000.0).contains(&self.color_temperature_k)
            || !(self.lumens.is_finite() && self.lumens >= 0.0)
            || !(self.range_m.is_finite() && self.range_m > 0.0)
        {
            return Err("position, 1000..40000 K, lumens >= 0 and range > 0 are required".into());
        }
        self.kind.validate()
    }
}

/// Colour of a black-body-like light of `kelvin` as linear RGB with the largest channel at 1.
///
/// Tanner Helland's public curve fit of black-body colours ("How to Convert Temperature (K) to
/// RGB: Algorithm and Sample Code", 2012, tannerhelland.com), valid for 1000-40000 K and clamped
/// outside it. The fit gives sRGB; it is converted to linear here.
pub fn kelvin_to_linear_rgb(kelvin: f32) -> [f32; 3] {
    let t = kelvin.clamp(1000.0, 40000.0) / 100.0;
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let g = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_17 * (t - 60.0).powf(-0.075_514_85)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [r, g, b].map(|c: f32| {
        let s = c.clamp(0.0, 255.0) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_and_daylight() {
        let [r, g, b] = kelvin_to_linear_rgb(2700.0);
        assert!(r > g && g > b, "{r} {g} {b}");
        let [r, g, b] = kelvin_to_linear_rgb(6500.0);
        assert!(r > 0.95 && g > 0.95 && b > 0.95, "{r} {g} {b}");
        assert!(kelvin_to_linear_rgb(12000.0)[2] > kelvin_to_linear_rgb(12000.0)[0]);
    }

    #[test]
    fn spot_direction_turns_with_the_fixture() {
        let spec = LightSpec {
            kind: LightKind::Spot {
                direction: [1.0, 0.0, 0.0],
                inner_cone_deg: 10.0,
                outer_cone_deg: 30.0,
            },
            color_temperature_k: 2700.0,
            lumens: 800.0,
            range_m: 8.0,
        };
        let l = spec.light(
            ("li1".into(), "Lamp".into(), "lighting".into()),
            [1.0, 2.0, 3.0],
            90.0,
        );
        let LightKind::Spot { direction: d, .. } = l.kind else {
            panic!()
        };
        // Counter-clockwise from above turns +X towards -Z (north).
        assert!(d[0].abs() < 1e-6 && (d[2] + 1.0).abs() < 1e-6, "{d:?}");
        assert!(l.validate().is_ok());
    }
}
