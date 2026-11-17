//! Hours of sun: the sun's directions over a day, and plant crowns as the shadow casters the
//! host adds to the models and shapes it can raycast (`query.sun_hours`). Headless: the host
//! does the ray casting against geometry.

use crate::growth::{Grower, growth_of};
use crate::project::{Site, SunTime};
use crate::season::Season;
use crate::sun::{sun_position, sunrise_sunset};

/// A slice of the day with the sun above the horizon, sampled at its middle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunSample {
    pub start_hour: f64,
    pub end_hour: f64,
    /// Unit vector from the ground towards the sun at the middle of the slice (drawing space).
    pub direction: [f32; 3],
}

impl SunSample {
    pub fn hours(&self) -> f64 {
        self.end_hour - self.start_hour
    }
}

/// The sun over one day at a site.
#[derive(Clone, Debug, PartialEq)]
pub struct DaySun {
    /// Local clock hours; `None` in polar day or night.
    pub sunrise: Option<f64>,
    pub sunset: Option<f64>,
    /// Hours the sun is above the horizon.
    pub daylight_hours: f64,
    pub samples: Vec<SunSample>,
}

/// The sun's directions over the day `year-month-day`: the time from sunrise to sunset cut into
/// slices of about `step_minutes`, each sampled at its middle. Where the sun neither rises nor
/// sets the whole day is cut and only slices with the sun up count.
pub fn day_sun(site: &Site, year: i32, month: u32, day: u32, step_minutes: f64) -> DaySun {
    let time = |hour: f64| SunTime {
        year,
        month,
        day,
        hour: hour as f32,
    };
    let step = step_minutes.max(1.0 / 60.0);
    let times = sunrise_sunset(site, &time(12.0));
    let (from, to) = match times {
        Some((rise, set)) => (rise.clamp(0.0, 24.0), set.clamp(0.0, 24.0)),
        None => (0.0, 24.0),
    };
    let n = (((to - from) * 60.0 / step).ceil() as usize).max(1);
    let width = (to - from) / n as f64;
    let samples: Vec<SunSample> = (0..n)
        .filter_map(|i| {
            let (start_hour, end_hour) = (from + width * i as f64, from + width * (i + 1) as f64);
            let sun = sun_position(site, &time((start_hour + end_hour) / 2.0));
            (sun.elevation_deg > 0.0 && end_hour > start_hour).then_some(SunSample {
                start_hour,
                end_hour,
                direction: sun.direction,
            })
        })
        .collect();
    let daylight_hours = match times {
        Some((rise, set)) => (set.clamp(0.0, 24.0) - rise.clamp(0.0, 24.0)).max(0.0),
        None => samples.iter().map(SunSample::hours).sum(),
    };
    DaySun {
        sunrise: times.map(|t| t.0),
        sunset: times.map(|t| t.1),
        daylight_hours,
        samples,
    }
}

/// The date a season is studied on: 21 March, 21 June, 21 September, 21 December at a northern
/// site, the same dates six months on in the south (spring is then 21 September).
pub fn season_date(season: Season, latitude: f64) -> (u32, u32) {
    let (m, d) = match season {
        Season::Spring => (3, 21),
        Season::Summer => (6, 21),
        Season::Autumn => (9, 21),
        Season::Winter => (12, 21),
    };
    if latitude < 0.0 {
        ((m + 5) % 12 + 1, d)
    } else {
        (m, d)
    }
}

/// Days in `month` of `year` (Gregorian); 0 for a month outside 1..=12.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Where the day's lit slices sit: runs of consecutive slices with any light, as `[start, end]`
/// hours. `lit` is each slice with the share of the sun that reaches the spot (0..=1).
pub fn lit_intervals(lit: &[(SunSample, f32)]) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::new();
    for (s, light) in lit {
        if *light <= 0.0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if (s.start_hour - last[1]).abs() < 1e-9 => last[1] = s.end_hour,
            _ => out.push([s.start_hour, s.end_hour]),
        }
    }
    out
}

/// At least this many hours of sun a day: full sun (the usual plant label threshold).
pub const FULL_SUN_HOURS: f64 = 6.0;
/// At least this many hours, and less than full sun: part shade (3 to 6 hours); under it, shade.
/// The thresholds are the ones plant labels and gardening guides use.
pub const PART_SHADE_HOURS: f64 = 3.0;

/// The share of the sun's light that gets through a deciduous crown without its leaves.
// shortcut: a guess; branches and twigs of a bare tree stop about 40 % of the light.
pub const BARE_CROWN_TRANSMISSION: f32 = 0.6;

/// The share of the sun's light that passes a crown in `month` at `latitude`. Evergreens (and
/// plants whose habit is not known) are opaque, 0. A deciduous crown lets through
/// [`BARE_CROWN_TRANSMISSION`] times the share of its leaves that are off: all of it in winter
/// (December to March in the north), little in autumn and none in summer. The leaf fractions are
/// the ones the plants are drawn with by month.
pub fn crown_transmission(evergreen: bool, month: u32, latitude: f64) -> f32 {
    if evergreen {
        return 0.0;
    }
    // The tables are written for the north.
    let m = if latitude < 0.0 {
        (month + 5) % 12 + 1
    } else {
        month
    };
    let leaves = match m {
        12 | 1 | 2 | 3 => 0.0,
        4 => 0.55,
        10 => 0.9,
        11 => 0.3,
        _ => 1.0,
    };
    BARE_CROWN_TRANSMISSION * (1.0 - leaves)
}

/// A crown as an ellipsoid that casts shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Occluder {
    pub center: [f32; 3],
    /// Half the size along x, y and z.
    pub radii: [f32; 3],
    /// The share of the sun's light that passes through (0: opaque).
    pub transmission: f32,
}

impl Occluder {
    /// The share of the light that reaches the ray's origin from along `dir` (any length):
    /// 1 when the ray misses the crown, its transmission when it passes through or starts inside.
    pub fn pass(&self, origin: [f32; 3], dir: [f32; 3]) -> f32 {
        let o: [f32; 3] = std::array::from_fn(|k| (origin[k] - self.center[k]) / self.radii[k]);
        let e: [f32; 3] = std::array::from_fn(|k| dir[k] / self.radii[k]);
        let a: f32 = e.iter().map(|v| v * v).sum();
        let b: f32 = (0..3).map(|k| o[k] * e[k]).sum();
        let c: f32 = o.iter().map(|v| v * v).sum::<f32>() - 1.0;
        let disc = b * b - a * c;
        // The far root lies ahead: the crown is in front of the origin or around it.
        if a > 0.0 && disc > 0.0 && (-b + disc.sqrt()) / a > 0.0 {
            self.transmission
        } else {
            1.0
        }
    }
}

/// Plants shorter than this cast no shadow worth counting.
const MIN_HEIGHT_M: f32 = 0.3;

/// The crown of one plant `age_years` old (planted age plus the years looked ahead), standing at
/// `position` at `scale`, from the growth table (`growth_of`): trees and palms an ellipsoid
/// over the upper three quarters of their height, conifers, shrubs and clipped plants (hedges)
/// an ellipsoid over the whole height. Perennials, grasses, ground cover, climbers and the lawn
/// cast nothing. `evergreen` decides how much light a leafless crown lets through.
pub fn plant_occluder(
    asset: &str,
    position: [f32; 3],
    scale: f32,
    age_years: f32,
    evergreen: bool,
    month: u32,
    latitude: f64,
) -> Option<Occluder> {
    let gr = growth_of(asset, None);
    let size = gr.size(age_years);
    let height = gr.mature_height_m * size.height * scale;
    let width = gr.mature_width_m * size.width * scale;
    if height < MIN_HEIGHT_M || width <= 0.0 {
        return None;
    }
    let (centre_k, radius_k) = match gr.grower {
        Grower::Tree | Grower::Palm => (0.625, 0.375),
        Grower::Conifer | Grower::Shrub | Grower::Clipped => (0.5, 0.5),
        _ => return None,
    };
    Some(Occluder {
        center: [position[0], position[1] + height * centre_k, position[2]],
        radii: [width / 2.0, height * radius_k, width / 2.0],
        transmission: crown_transmission(evergreen, month, latitude),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(latitude: f64) -> Site {
        Site {
            latitude,
            longitude: 0.0,
            utc_offset_hours: 0.0,
            north_deg: 0.0,
        }
    }

    #[test]
    fn twelve_hours_at_the_equator_on_an_equinox() {
        let d = day_sun(&site(0.0), 2026, 3, 21, 10.0);
        let (rise, set) = sunrise_sunset(
            &site(0.0),
            &SunTime {
                year: 2026,
                month: 3,
                day: 21,
                hour: 12.0,
            },
        )
        .unwrap();
        assert!((d.daylight_hours - (set - rise)).abs() < 1e-9);
        assert!(
            (d.daylight_hours - 12.1).abs() < 0.2,
            "{}",
            d.daylight_hours
        );
        let sum: f64 = d.samples.iter().map(SunSample::hours).sum();
        assert!((sum - d.daylight_hours).abs() < 0.2, "{sum}");
        // Slices are about 10 minutes.
        assert!(d.samples.iter().all(|s| s.hours() <= 10.0 / 60.0 + 1e-9));
    }

    #[test]
    fn long_days_at_latitude_51_in_june() {
        let d = day_sun(&site(51.0), 2026, 6, 21, 10.0);
        assert!(
            (d.daylight_hours - 16.5).abs() < 0.3,
            "{}",
            d.daylight_hours
        );
        // Every sample has the sun up and a unit direction.
        for s in &d.samples {
            assert!(s.direction[1] > 0.0);
            let n: f32 = s.direction.iter().map(|v| v * v).sum();
            assert!((n - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn polar_day_and_night() {
        let day = day_sun(&site(80.0), 2026, 6, 21, 30.0);
        assert!(day.sunrise.is_none() && (day.daylight_hours - 24.0).abs() < 0.6);
        let night = day_sun(&site(80.0), 2026, 12, 21, 30.0);
        assert!(night.samples.is_empty() && night.daylight_hours == 0.0);
    }

    #[test]
    fn season_dates_turn_in_the_south() {
        assert_eq!(season_date(Season::Spring, 51.0), (3, 21));
        assert_eq!(season_date(Season::Spring, -34.0), (9, 21));
        assert_eq!(season_date(Season::Winter, -34.0), (6, 21));
    }

    #[test]
    fn intervals_join_consecutive_slices() {
        let s = |a: f64, b: f64| SunSample {
            start_hour: a,
            end_hour: b,
            direction: [0.0, 1.0, 0.0],
        };
        let lit = [
            (s(6.0, 7.0), 1.0),
            (s(7.0, 8.0), 0.6),
            (s(8.0, 9.0), 0.0),
            (s(9.0, 10.0), 1.0),
        ];
        assert_eq!(lit_intervals(&lit), vec![[6.0, 8.0], [9.0, 10.0]]);
    }

    #[test]
    fn bare_crowns_let_light_through_and_evergreens_do_not() {
        assert_eq!(crown_transmission(true, 1, 51.0), 0.0);
        assert_eq!(crown_transmission(false, 7, 51.0), 0.0);
        assert_eq!(crown_transmission(false, 1, 51.0), BARE_CROWN_TRANSMISSION);
        assert_eq!(crown_transmission(false, 1, -34.0), 0.0);
        assert_eq!(crown_transmission(false, 7, -34.0), BARE_CROWN_TRANSMISSION);
    }

    #[test]
    fn a_crown_shades_the_ray_through_it() {
        let c = Occluder {
            center: [0.0, 5.0, 0.0],
            radii: [2.0, 2.0, 2.0],
            transmission: 0.0,
        };
        assert_eq!(c.pass([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]), 0.0);
        assert_eq!(c.pass([5.0, 0.0, 0.0], [0.0, 1.0, 0.0]), 1.0);
        // Behind the origin: not in the way.
        assert_eq!(c.pass([0.0, 9.0, 0.0], [0.0, 1.0, 0.0]), 1.0);
        // Starting inside.
        assert_eq!(c.pass([0.0, 5.0, 0.0], [0.0, 1.0, 0.0]), 0.0);
    }

    #[test]
    fn trees_and_hedges_cast_and_flowers_do_not() {
        // Any table tree, any tall perennial.
        let t = crate::growth::GROWTH
            .iter()
            .find(|r| r.1 == Grower::Tree)
            .unwrap();
        let p = crate::growth::GROWTH
            .iter()
            .find(|r| r.1 == Grower::Perennial && r.2 > 0.5)
            .unwrap();
        let t = plant_occluder(t.0, [1.0, 2.0, 3.0], 1.0, 100.0, true, 7, 51.0).unwrap();
        assert!(t.radii[1] > 1.0 && t.center[1] > 2.0 && t.transmission == 0.0);
        assert!(plant_occluder(p.0, [0.0; 3], 1.0, 100.0, true, 7, 51.0).is_none());
    }
}
