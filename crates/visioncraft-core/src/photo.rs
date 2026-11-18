//! Photo settings and the thin lens behind them: how many frames a photo averages, the lens'
//! aperture as an f-number with the focal length a 36 mm (full-frame) sensor gives for the
//! field of view, where it is focused, and how long the shutter stays open. The renderer moves
//! the eye over the aperture and shears the frustum back onto the focal plane, so a point at
//! depth `d` spreads over the circle of confusion of [`circle_of_confusion_m`].

use serde::{Deserialize, Serialize};

/// The sensor's width (m): full frame, 36 x 24 mm.
pub const SENSOR_WIDTH_M: f32 = 0.036;
/// The frames a photo can average.
pub const SAMPLES: std::ops::RangeInclusive<u32> = 1..=1024;
/// The f-numbers the lens takes.
pub const F_NUMBERS: std::ops::RangeInclusive<f32> = 1.0..=32.0;
/// The focus distances that can be set (m).
pub const FOCUS_M: std::ops::RangeInclusive<f32> = 0.1..=1000.0;
/// The longest shutter time (s).
pub const SHUTTER_MAX_S: f32 = 1.0;

/// Where the lens is focused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "FocusRepr", into = "FocusRepr")]
pub enum Focus {
    /// At the camera's target (the orbit's pivot, a viewpoint's target): the default.
    #[default]
    Target,
    /// At what the picture shows at `focus_point`, as a camera's autofocus.
    Auto,
    /// At this distance along the view (m).
    Metres(f32),
}

/// `focus` as written: "target", "auto" or metres.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum FocusRepr {
    Name(String),
    Metres(f32),
}

impl TryFrom<FocusRepr> for Focus {
    type Error = String;

    fn try_from(r: FocusRepr) -> Result<Self, String> {
        match r {
            FocusRepr::Name(n) if n == "target" => Ok(Focus::Target),
            FocusRepr::Name(n) if n == "auto" => Ok(Focus::Auto),
            FocusRepr::Name(n) => Err(format!(
                "focus `{n}`: give \"auto\", \"target\" or a distance in metres"
            )),
            FocusRepr::Metres(m) => Ok(Focus::Metres(m)),
        }
    }
}

impl From<Focus> for FocusRepr {
    fn from(f: Focus) -> Self {
        match f {
            Focus::Target => FocusRepr::Name("target".into()),
            Focus::Auto => FocusRepr::Name("auto".into()),
            Focus::Metres(m) => FocusRepr::Metres(m),
        }
    }
}

/// How a photo is taken (`view.set_photo`): the defaults of the app's photo export and of
/// `view.render` at quality `photo`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhotoSettings {
    /// Frames averaged, 1..=1024.
    pub samples: u32,
    /// The aperture as an f-number (1..=32); `None`: a pinhole, everything sharp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<f32>,
    pub focus: Focus,
    /// Where an autofocus looks, 0..1 from the picture's top left.
    pub focus_point: [f32; 2],
    /// How long the shutter stays open (s), 0..=1: plants in the wind, water and rain blur
    /// with their motion. 0: a frozen instant.
    pub shutter_s: f32,
}

impl Default for PhotoSettings {
    fn default() -> Self {
        PhotoSettings {
            samples: 64,
            aperture: None,
            focus: Focus::Target,
            focus_point: [0.5, 0.5],
            shutter_s: 0.0,
        }
    }
}

impl PhotoSettings {
    pub fn is_default(&self) -> bool {
        *self == PhotoSettings::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        if !SAMPLES.contains(&self.samples) {
            return Err("`samples` must be from 1 to 1024".into());
        }
        if self.aperture.is_some_and(|n| !F_NUMBERS.contains(&n)) {
            return Err("`aperture` must be an f-number from 1 to 32, or null for none".into());
        }
        if let Focus::Metres(m) = self.focus
            && !FOCUS_M.contains(&m)
        {
            return Err("`focus` in metres must be from 0.1 to 1000".into());
        }
        if !self.focus_point.iter().all(|c| (0.0..=1.0).contains(c)) {
            return Err("`focus_point` must be [x, y] from 0 to 1".into());
        }
        if !(0.0..=SHUTTER_MAX_S).contains(&self.shutter_s) {
            return Err("`shutter_s` must be from 0 to 1".into());
        }
        Ok(())
    }
}

/// The focal length (m) a full-frame sensor needs for a vertical field of view `fov_y_rad` in
/// a picture `aspect` (width / height) wide: the sensor's width spans the horizontal field.
pub fn focal_length_m(fov_y_rad: f32, aspect: f32) -> f32 {
    0.5 * SENSOR_WIDTH_M / ((fov_y_rad / 2.0).tan() * aspect)
}

/// The aperture's radius (m) at f-number `f_number`: half the focal length over the f-number.
pub fn aperture_radius_m(focal_m: f32, f_number: f32) -> f32 {
    focal_m / (2.0 * f_number)
}

/// The thin lens' circle of confusion on the sensor (its diameter, m) for a point at `depth_m`
/// with the lens focused at `focus_m`: A f |d - F| / (d (F - f)), A = f / N the aperture's
/// diameter. 0 on the focal plane, growing with the defocus towards A f / (F - f) far behind it.
pub fn circle_of_confusion_m(focal_m: f32, f_number: f32, focus_m: f32, depth_m: f32) -> f32 {
    let a = focal_m / f_number;
    a * focal_m * (depth_m - focus_m).abs() / (depth_m * (focus_m - focal_m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focal_length_follows_the_field_of_view() {
        // A 36 mm wide sensor sees 39.6 degrees across through a 50 mm lens.
        let fov_x = 2.0 * (0.018f32 / 0.05).atan();
        let aspect = 1.5;
        let fov_y = 2.0 * ((fov_x / 2.0).tan() / aspect).atan();
        assert!((focal_length_m(fov_y, aspect) - 0.05).abs() < 1e-6);
        assert!((aperture_radius_m(0.05, 2.5) - 0.01).abs() < 1e-7);
    }

    #[test]
    fn the_circle_of_confusion_is_zero_at_focus_and_grows_with_defocus() {
        let (f, n, focus) = (0.05, 2.8, 4.0);
        assert_eq!(circle_of_confusion_m(f, n, focus, focus), 0.0);
        // Worked by hand: A = 17.86 mm; behind at 8 m, 17.86 * 50 * 4 / (8000 * 3.95) mm.
        let behind = circle_of_confusion_m(f, n, focus, 8.0);
        assert!((behind - 0.113e-3).abs() < 0.001e-3, "{behind}");
        // In front blurs faster than behind for the same distance off the focal plane.
        assert!(circle_of_confusion_m(f, n, focus, 2.0) > circle_of_confusion_m(f, n, focus, 6.0));
        // It grows with the defocus on either side and with a wider aperture.
        let mut last = 0.0;
        for d in [4.5, 6.0, 10.0, 50.0, 1000.0] {
            let c = circle_of_confusion_m(f, n, focus, d);
            assert!(c > last);
            last = c;
        }
        assert!(
            circle_of_confusion_m(f, 1.4, focus, 8.0) > circle_of_confusion_m(f, 8.0, focus, 8.0)
        );
        // Focused at the hyperfocal distance H = f^2 / (N c) + f, the far distance blurs by c.
        let c = 0.03e-3;
        let h = f * f / (n * c) + f;
        let far = circle_of_confusion_m(f, n, h, 1.0e9);
        assert!((far - c).abs() < 1e-8, "{far}");
    }

    #[test]
    fn settings_round_trip_and_validate() {
        let s: PhotoSettings =
            serde_json::from_value(serde_json::json!({"aperture": 2.8, "focus": "auto"})).unwrap();
        assert_eq!(
            (s.samples, s.aperture, s.focus),
            (64, Some(2.8), Focus::Auto)
        );
        let m: PhotoSettings = serde_json::from_value(serde_json::json!({"focus": 4})).unwrap();
        assert_eq!(m.focus, Focus::Metres(4.0));
        assert_eq!(
            serde_json::to_value(m).unwrap()["focus"],
            serde_json::json!(4.0)
        );
        assert!(
            serde_json::from_value::<PhotoSettings>(serde_json::json!({"focus": "near"})).is_err()
        );
        for bad in [
            PhotoSettings {
                samples: 0,
                ..Default::default()
            },
            PhotoSettings {
                aperture: Some(0.5),
                ..Default::default()
            },
            PhotoSettings {
                focus: Focus::Metres(0.0),
                ..Default::default()
            },
            PhotoSettings {
                focus_point: [1.2, 0.5],
                ..Default::default()
            },
            PhotoSettings {
                shutter_s: 2.0,
                ..Default::default()
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert!(PhotoSettings::default().validate().is_ok());
    }
}
