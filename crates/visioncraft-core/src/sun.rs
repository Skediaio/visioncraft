//! Solar position and sunrise/sunset.
//!
//! Equations from the public NOAA Global Monitoring Laboratory "General Solar Position
//! Calculations" document and its solar calculator spreadsheet (after Meeus, *Astronomical
//! Algorithms*): Julian century, geometric mean longitude and anomaly, equation of centre,
//! apparent longitude, obliquity, declination, equation of time, true solar time, hour angle,
//! zenith and azimuth, the standard refraction correction, and the sunrise hour angle for a
//! zenith of 90.833 degrees.

use crate::project::{Site, SunTime};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunPosition {
    /// Degrees clockwise from north.
    pub azimuth_deg: f64,
    /// Degrees above the horizon, atmospheric refraction included.
    pub elevation_deg: f64,
    /// Unit vector from the ground towards the sun, world axes (+X east, +Y up, -Z north).
    pub direction: [f32; 3],
}

/// Julian day number at 0h UT of a Gregorian date (Meeus).
fn julian_day(year: i32, month: u32, day: u32) -> f64 {
    let (y, m) = if month <= 2 {
        (f64::from(year - 1), f64::from(month + 12))
    } else {
        (f64::from(year), f64::from(month))
    };
    let a = (y / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y + 4716.0)).floor() + (30.6001 * (m + 1.0)).floor() + f64::from(day) + b - 1524.5
}

struct Sky {
    declination: f64,      // radians
    equation_of_time: f64, // minutes
}

fn sky(jd: f64) -> Sky {
    let t = (jd - 2451545.0) / 36525.0;
    let l0 = (280.46646 + t * (36000.76983 + t * 0.0003032)).rem_euclid(360.0);
    let m = (357.52911 + t * (35999.05029 - 0.0001537 * t)).to_radians();
    let e = 0.016708634 - t * (0.000042037 + 0.0000001267 * t);
    let c = m.sin() * (1.914602 - t * (0.004817 + 0.000014 * t))
        + (2.0 * m).sin() * (0.019993 - 0.000101 * t)
        + (3.0 * m).sin() * 0.000289;
    let omega = (125.04 - 1934.136 * t).to_radians();
    let lambda = (l0 + c - 0.00569 - 0.00478 * omega.sin()).to_radians();
    let eps0 = 23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.00059 - t * 0.001813))) / 60.0) / 60.0;
    let eps = (eps0 + 0.00256 * omega.cos()).to_radians();
    let declination = (eps.sin() * lambda.sin()).asin();
    let y = (eps / 2.0).tan().powi(2);
    let l0 = l0.to_radians();
    let eot = y * (2.0 * l0).sin() - 2.0 * e * m.sin() + 4.0 * e * y * m.sin() * (2.0 * l0).cos()
        - 0.5 * y * y * (4.0 * l0).sin()
        - 1.25 * e * e * (2.0 * m).sin();
    Sky {
        declination,
        equation_of_time: 4.0 * eot.to_degrees(),
    }
}

/// Atmospheric refraction in degrees for a true elevation in degrees (NOAA's approximation).
fn refraction(elevation: f64) -> f64 {
    let tan = elevation.to_radians().tan();
    let arcsec = if elevation > 85.0 {
        0.0
    } else if elevation > 5.0 {
        58.1 / tan - 0.07 / tan.powi(3) + 0.000086 / tan.powi(5)
    } else if elevation > -0.575 {
        1735.0
            + elevation * (-518.2 + elevation * (103.4 + elevation * (-12.79 + elevation * 0.711)))
    } else {
        -20.772 / tan
    };
    arcsec / 3600.0
}

pub fn sun_position(site: &Site, time: &SunTime) -> SunPosition {
    let hour = f64::from(time.hour);
    let jd = julian_day(time.year, time.month, time.day) + (hour - site.utc_offset_hours) / 24.0;
    let sky = sky(jd);
    let true_solar_minutes =
        hour * 60.0 + sky.equation_of_time + 4.0 * site.longitude - 60.0 * site.utc_offset_hours;
    let hour_angle = (true_solar_minutes / 4.0 - 180.0).to_radians();
    let lat = site.latitude.to_radians();
    let cos_zenith = (lat.sin() * sky.declination.sin()
        + lat.cos() * sky.declination.cos() * hour_angle.cos())
    .clamp(-1.0, 1.0);
    let zenith = cos_zenith.acos();
    let true_elevation = 90.0 - zenith.to_degrees();
    let denominator = lat.cos() * zenith.sin();
    // At the poles or straight overhead the azimuth is undefined: call it south.
    let azimuth_deg = if denominator.abs() < 1e-9 {
        180.0
    } else {
        let a = (((lat.sin() * cos_zenith - sky.declination.sin()) / denominator).clamp(-1.0, 1.0))
            .acos()
            .to_degrees();
        if hour_angle > 0.0 {
            (a + 180.0).rem_euclid(360.0)
        } else {
            (540.0 - a).rem_euclid(360.0)
        }
    };
    let elevation_deg = true_elevation + refraction(true_elevation);
    let (az, el) = (azimuth_deg.to_radians(), elevation_deg.to_radians());
    SunPosition {
        azimuth_deg,
        elevation_deg,
        direction: [
            (el.cos() * az.sin()) as f32,
            el.sin() as f32,
            (-el.cos() * az.cos()) as f32,
        ],
    }
}

/// Sunrise and sunset as local time of day in hours, for the date of `time`; `None` when the sun
/// does not rise or set that day (polar day or night).
pub fn sunrise_sunset(site: &Site, time: &SunTime) -> Option<(f64, f64)> {
    // The sky at local noon of that day.
    let jd = julian_day(time.year, time.month, time.day) + (12.0 - site.utc_offset_hours) / 24.0;
    let sky = sky(jd);
    let lat = site.latitude.to_radians();
    let cos_ha = 90.833f64.to_radians().cos() / (lat.cos() * sky.declination.cos())
        - lat.tan() * sky.declination.tan();
    if !(-1.0..=1.0).contains(&cos_ha) {
        return None;
    }
    let ha = cos_ha.acos().to_degrees();
    let local = |sign: f64| {
        (720.0 - 4.0 * (site.longitude + sign * ha) - sky.equation_of_time
            + 60.0 * site.utc_offset_hours)
            / 60.0
    };
    Some((local(1.0), local(-1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(lat: f64, lon: f64, tz: f64, (y, mo, d): (i32, u32, u32), hour: f32) -> (Site, SunTime) {
        (
            Site {
                latitude: lat,
                longitude: lon,
                utc_offset_hours: tz,
            },
            SunTime {
                year: y,
                month: mo,
                day: d,
                hour,
            },
        )
    }

    #[test]
    fn julian_day_of_j2000() {
        assert_eq!(julian_day(2000, 1, 1), 2451544.5);
    }

    #[test]
    fn equator_equinox_noon_is_overhead() {
        // 2026-03-20, solar noon at longitude 0 is about 12:07 UTC.
        let (s, t) = at(0.0, 0.0, 0.0, (2026, 3, 20), 12.1);
        let p = sun_position(&s, &t);
        assert!(p.elevation_deg > 89.5, "{p:?}");
    }

    #[test]
    fn brussels_midsummer_noon_elevation() {
        // 90 - 50.85 + 23.44 = 62.59; solar noon is about 12:44 at UTC+1 (4.35 E, equation of time -1.7 min).
        let (s, t) = at(50.85, 4.35, 1.0, (2026, 6, 21), 12.74);
        let p = sun_position(&s, &t);
        assert!((p.elevation_deg - 62.6).abs() < 0.5, "{p:?}");
        assert!((p.azimuth_deg - 180.0).abs() < 2.0, "{p:?}");
        // South is +Z in world axes, up is +Y.
        assert!(p.direction[2] > 0.4 && p.direction[1] > 0.85 && p.direction[0].abs() < 0.05);
        let len: f32 = p.direction.iter().map(|c| c * c).sum();
        assert!((len - 1.0).abs() < 1e-5);
    }

    #[test]
    fn brussels_december_midnight_is_dark() {
        let (s, t) = at(50.85, 4.35, 1.0, (2026, 12, 21), 0.0);
        assert!(sun_position(&s, &t).elevation_deg < -50.0);
    }

    #[test]
    fn morning_is_east_afternoon_is_west() {
        let (s, t) = at(50.85, 4.35, 1.0, (2026, 6, 21), 8.0);
        let am = sun_position(&s, &t);
        let (s, t) = at(50.85, 4.35, 1.0, (2026, 6, 21), 17.0);
        let pm = sun_position(&s, &t);
        assert!(am.azimuth_deg > 60.0 && am.azimuth_deg < 120.0, "{am:?}");
        assert!(pm.azimuth_deg > 240.0 && pm.azimuth_deg < 300.0, "{pm:?}");
        assert!(am.direction[0] > 0.0 && pm.direction[0] < 0.0);
    }

    #[test]
    fn brussels_midsummer_day_length() {
        let (s, t) = at(50.85, 4.35, 1.0, (2026, 6, 21), 12.0);
        let (rise, set) = sunrise_sunset(&s, &t).unwrap();
        // About 16 h 40 min of daylight, centred on solar noon (12:44 at UTC+1).
        assert!((set - rise - 16.65).abs() < 0.15, "{rise} {set}");
        assert!(((rise + set) / 2.0 - 12.74).abs() < 0.1);
    }

    #[test]
    fn polar_night_has_no_sunrise() {
        let (s, t) = at(80.0, 0.0, 0.0, (2026, 12, 21), 12.0);
        assert_eq!(sunrise_sunset(&s, &t), None);
    }
}
