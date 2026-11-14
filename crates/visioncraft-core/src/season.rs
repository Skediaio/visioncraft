//! The four seasons a garden is shown in. Plants have one state per season (not one per month):
//! the season of the project's date decides it, and each season has a representative date at the
//! site, which `view.set {season}` sets. The exact date stays for sun studies.

use crate::project::{Project, SunTime};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    pub const ALL: [Season; 4] = [
        Season::Spring,
        Season::Summer,
        Season::Autumn,
        Season::Winter,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Season::Spring => "spring",
            Season::Summer => "summer",
            Season::Autumn => "autumn",
            Season::Winter => "winter",
        }
    }

    pub fn parse(s: &str) -> Option<Season> {
        let s = s.to_ascii_lowercase();
        Season::ALL
            .into_iter()
            .find(|x| x.name() == s || (s == "fall" && *x == Season::Autumn))
    }

    /// The season of `month` (1..=12) at `latitude`: meteorological seasons (March to May is
    /// spring in the north), six months on south of the equator.
    pub fn of(month: u32, latitude: f64) -> Season {
        let m = if latitude < 0.0 {
            (month + 5) % 12 + 1
        } else {
            month
        };
        match m {
            3..=5 => Season::Spring,
            6..=8 => Season::Summer,
            9..=11 => Season::Autumn,
            _ => Season::Winter,
        }
    }

    /// The date shown for the season at `latitude`: (month, day). 20 April, 21 June, 20 October,
    /// 15 January in the north; the same six months on in the south.
    pub fn date(self, latitude: f64) -> (u32, u32) {
        let (m, d) = match self {
            Season::Spring => (4, 20),
            Season::Summer => (6, 21),
            Season::Autumn => (10, 20),
            Season::Winter => (1, 15),
        };
        if latitude < 0.0 {
            ((m + 5) % 12 + 1, d)
        } else {
            (m, d)
        }
    }

    /// The month the plants' tables (bloom, leaf fall, autumn colour; written for the north)
    /// show the season by: one state per season and species.
    pub fn plant_month(self) -> u8 {
        self.date(1.0).0 as u8
    }

    /// `time` moved to this season's date at `latitude`, its year and hour kept.
    pub fn time(self, time: SunTime, latitude: f64) -> SunTime {
        let (month, day) = self.date(latitude);
        SunTime { month, day, ..time }
    }
}

impl Project {
    /// The season of the project's date at its site.
    pub fn season(&self) -> Season {
        Season::of(self.time.month, self.site.latitude)
    }

    /// The snow drawn: the project's, in winter only (a date moved to summer leaves it stored
    /// and not drawn).
    pub fn snow(&self) -> f32 {
        if self.season() == Season::Winter {
            self.view.snow
        } else {
            0.0
        }
    }

    /// The month the plants are drawn in: their state for the season of the date shown.
    pub fn plant_month(&self) -> u8 {
        self.season().plant_month()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_month_has_a_season_and_each_season_a_date_that_maps_back() {
        for lat in [50.85, -33.9] {
            for s in Season::ALL {
                let (m, d) = s.date(lat);
                assert_eq!(Season::of(m, lat), s, "{s:?} at {lat}");
                assert!((1..=31).contains(&d));
            }
        }
        assert_eq!(Season::of(10, 50.0), Season::Autumn);
        assert_eq!(Season::of(10, -34.0), Season::Spring);
        assert_eq!(Season::Summer.date(-34.0), (12, 21));
        // Plants take one state per season: four months in all, whatever the date.
        let months: std::collections::BTreeSet<u8> = (1..=12)
            .map(|m| Season::of(m, 50.0).plant_month())
            .collect();
        assert_eq!(months.into_iter().collect::<Vec<_>>(), vec![1, 4, 6, 10]);
        assert_eq!(Season::parse("Autumn"), Some(Season::Autumn));
        assert_eq!(Season::parse("fall"), Some(Season::Autumn));
        assert_eq!(Season::parse("monsoon"), None);
    }

    #[test]
    fn view_set_season_moves_the_date_and_snow_is_kept_with_the_project() {
        use crate::{Project, Session};
        use serde_json::json;
        let mut s = Session::new(Project::default());
        s.run("time.set", json!({"month": 6, "day": 3, "hour": 15.5}))
            .unwrap();
        s.run("view.set", json!({"season": "autumn", "snow": 0.0}))
            .unwrap();
        let t = s.project().time;
        assert_eq!((t.month, t.day, t.hour), (10, 20, 15.5));
        assert_eq!(s.project().season(), Season::Autumn);
        s.run("view.set", json!({"season": "winter", "snow": 0.7}))
            .unwrap();
        assert_eq!(s.project().view.snow, 0.7);
        let back = Project::from_json_bytes(&s.project().to_json_bytes()).unwrap();
        assert_eq!((back.view.snow, back.season()), (0.7, Season::Winter));
        assert!(s.run("view.set", json!({"snow": 1.5})).is_err());
        // Snow belongs to winter: refused in another season, kept but not drawn after a date
        // moved out of winter.
        let autumn = s.project().clone();
        let mut a = Session::new(Project {
            time: Season::Autumn.time(autumn.time, 50.0),
            ..autumn
        });
        assert!(a.run("view.set", json!({"snow": 0.5})).is_err());
        s.run("time.set", json!({"month": 7})).unwrap();
        assert_eq!((s.project().view.snow, s.project().snow()), (0.7, 0.0));
        s.run("view.set", json!({"season": "winter"})).unwrap();
        assert!(s.run("view.set", json!({"season": "monsoon"})).is_err());
        // Without snow nothing is written: older files stay as they were.
        s.run("view.set", json!({"snow": 0})).unwrap();
        assert!(
            !String::from_utf8(s.project().to_json_bytes())
                .unwrap()
                .contains("snow")
        );
        // A viewpoint keeps its season: its own day, the hour given.
        s.run("viewpoint.add", json!({"name": "Spring", "position": [0, 2, 9], "target": [0, 1, 0], "season": "spring", "time": {"hour": 10}})).unwrap();
        let v = &s.project().viewpoints[0];
        let t = v.time.unwrap();
        assert_eq!((t.month, t.day, t.hour), (4, 20, 10.0));
        s.run("viewpoint.update", json!({"id": "v1", "season": "summer"}))
            .unwrap();
        assert_eq!(s.project().viewpoints[0].time.unwrap().month, 6);
    }
}
