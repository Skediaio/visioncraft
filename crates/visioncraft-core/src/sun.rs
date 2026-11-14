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

// ---- illuminance and the moon ---------------------------------------------------------------
//
// Janiczek & DeYoung, "Computer Programs for Sun and Moon Illuminance With Contingent Tables
// and Diagrams", U.S. Naval Observatory Circular No. 171 (1987), a U.S. government work: its
// clear-sky illuminance fit (subroutine ATMOS, valid from the zenith down to the end of
// twilight) and its low-precision moon (position to a fraction of a degree, phase, and the
// moon's illuminance relative to the sun's).

/// The circular's clear-sky factor for an apparent altitude in degrees: direct light through
/// the air mass of a curved atmosphere plus the scattered light, which carries on below the
/// horizon as twilight.
fn usno_atmos(altitude_deg: f64) -> f64 {
    const X: f64 = 753.66156;
    let h = altitude_deg.to_radians();
    let u = h.sin();
    let s = (X * h.cos() / (X + 1.0)).asin();
    let m = X * (s.cos() - u) + s.cos();
    ((-0.21 * m).exp() * u
        + 0.0289 * (-0.042 * m).exp() * (1.0 + (altitude_deg + 90.0) * u / 57.29577951))
        .max(0.0)
}

/// Illuminance (lux) on open level ground from the sun and the clear sky together, for the
/// sun's apparent elevation in degrees: about 100 klx at 60 degrees, 760 lux at sunset, 3 lux
/// at the end of civil twilight (-6), 4 mlx at -12 (USNO Circular 171).
pub fn daylight_lux(elevation_deg: f64) -> f64 {
    133_775.0 * usno_atmos(elevation_deg)
}

/// Starlight and airglow on a moonless night (lux), the circular's constant.
pub const STARLIGHT_LUX: f64 = 0.0005;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoonPosition {
    /// Degrees clockwise from north.
    pub azimuth_deg: f64,
    /// Apparent degrees above the horizon (parallax and refraction included).
    pub elevation_deg: f64,
    /// Unit vector towards the moon, world axes as `SunPosition::direction`.
    pub direction: [f32; 3],
    /// Lit fraction of the disc, 0 (new) to 1 (full).
    pub fraction: f64,
    /// Illuminance (lux) on open level ground from the moon and the sky it lights: about 0.3
    /// for a full moon high up, 0 below the horizon.
    pub illuminance_lux: f64,
    /// The moon's light as a fraction of the sun's at the same altitude (full moon ~3e-6): the
    /// renderer scales its sun and sky by it when the moon lights the scene.
    pub sun_ratio: f64,
}

pub fn moon_position(site: &Site, time: &SunTime) -> MoonPosition {
    const CE: f64 = 0.91775; // cos and sin of the obliquity
    const SE: f64 = 0.39715;
    let ut = f64::from(time.hour) - site.utc_offset_hours;
    // Days from J2000.0 (2000-01-01 12h UT).
    let d = julian_day(time.year, time.month, time.day) + ut / 24.0 - 2_451_545.0;
    // Sun: mean longitude, mean anomaly, ecliptic longitude.
    let t = (280.46 + 0.98565 * d).rem_euclid(360.0);
    let g = (357.5 + 0.9856 * d).to_radians();
    let ls = (t + 1.91 * g.sin()).to_radians();
    // Local sidereal time (degrees): the mean sun's longitude - 180 + 360 * UT / 24 + longitude.
    let sidereal = t - 180.0 + 15.0 * ut + site.longitude;
    // Moon: mean longitude, anomaly, argument of latitude, twice the elongation less the anomaly.
    let v = (218.32 + 13.1764 * d).rem_euclid(360.0);
    let y = (134.96 + 13.06499 * d).to_radians();
    let o = (93.27 + 13.22935 * d).to_radians();
    let w = (235.7 + 24.38150 * d).to_radians();
    let lon = (v
        + (6.29 - 1.27 * w.cos() + 0.43 * y.cos()) * y.sin()
        + (0.66 + 1.27 * y.cos()) * w.sin()
        - 0.19 * g.sin()
        - 0.23 * o.sin() * o.cos())
    .to_radians();
    let lat = ((5.13 - 0.17 * w.cos()) * o.sin() + (0.56 * y.sin() + 0.17 * w.sin()) * o.cos())
        .to_radians();
    // Ecliptic to equatorial.
    let q = lat.cos() * lon.cos();
    let p = CE * lon.sin() * lat.cos() - SE * lat.sin();
    let sin_dec = SE * lon.sin() * lat.cos() + CE * lat.sin();
    let dec = sin_dec.asin();
    let ra = p.atan2(q).to_degrees();
    // Equatorial to horizontal.
    let hour_angle = (sidereal - ra).to_radians();
    let (si, ci) = (
        site.latitude.to_radians().sin(),
        site.latitude.to_radians().cos(),
    );
    let geo_alt = (sin_dec * si + dec.cos() * ci * hour_angle.cos())
        .asin()
        .to_degrees();
    let qa = sin_dec * ci - dec.cos() * si * hour_angle.cos();
    let pa = -dec.cos() * hour_angle.sin();
    let azimuth_deg = pa.atan2(qa).to_degrees().rem_euclid(360.0);
    // Parallax (about one degree), then refraction.
    let alt = geo_alt - 0.95 * geo_alt.to_radians().cos();
    let elevation_deg = if alt < -5.0 / 6.0 {
        alt
    } else {
        alt + 1.0 / (alt + 8.6 / (alt + 4.42)).to_radians().tan() / 60.0
    };
    // Phase: the elongation from the sun.
    let e = ((lon - ls).cos() * lat.cos()).clamp(-1.0, 1.0).acos();
    let fraction = 0.5 * (1.0 - e.cos());
    let mut lit = 0.892 * (-3.343 / (e / 2.0).tan().max(1e-6).powf(0.632)).exp()
        + 0.0344 * (e.sin() - e * e.cos());
    lit = 0.418 * lit / (1.0 - 0.005 * e.cos() - 0.03 * geo_alt.to_radians().sin());
    let illuminance_lux = if elevation_deg > -0.8 {
        lit * usno_atmos(elevation_deg)
    } else {
        0.0
    };
    let (az, el) = (azimuth_deg.to_radians(), elevation_deg.to_radians());
    MoonPosition {
        azimuth_deg,
        elevation_deg,
        direction: [
            (el.cos() * az.sin()) as f32,
            el.sin() as f32,
            (-el.cos() * az.cos()) as f32,
        ],
        fraction,
        illuminance_lux,
        sun_ratio: lit / 133_775.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daylight_falls_through_twilight_as_the_usno_tables() {
        let e = daylight_lux;
        assert!((80_000.0..120_000.0).contains(&e(60.0)), "{}", e(60.0));
        assert!((500.0..1000.0).contains(&e(0.0)), "{}", e(0.0));
        assert!((2.0..5.0).contains(&e(-6.0)), "{}", e(-6.0));
        assert!(e(-12.0) < 0.01 && e(-18.0) < STARLIGHT_LUX, "{}", e(-12.0));
    }

    #[test]
    fn full_moon_and_new_moon() {
        // Full moon 2026-03-03 11:38 UT (a total lunar eclipse), new moon 2026-03-19 01:23 UT.
        // At midnight in Brussels (UTC+1) on 3 March the full moon stands high in the south.
        let (s, t) = at(50.85, 4.35, 1.0, (2026, 3, 3), 23.9);
        let m = moon_position(&s, &t);
        assert!(m.fraction > 0.97, "{m:?}");
        assert!(m.elevation_deg > 30.0, "{m:?}");
        assert!((m.azimuth_deg - 180.0).abs() < 30.0, "{m:?}");
        assert!((0.1..0.4).contains(&m.illuminance_lux), "{m:?}");
        let (s, t) = at(50.85, 4.35, 1.0, (2026, 3, 19), 2.4);
        let m = moon_position(&s, &t);
        assert!(m.fraction < 0.02 && m.illuminance_lux < 0.002, "{m:?}");
    }

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
