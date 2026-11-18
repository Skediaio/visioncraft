//! Weather states: one name for the clouds, the rain, the wetness, the mist, the gusts and the
//! wind together (`view.set weather`, a film shot's `weather`, `view.render weather`), and the
//! mix of two of them, so a sky can clear or close in over time.
//!
//! A weather writes its numbers into the view state; nothing of the name is kept, so every
//! field can still be changed on its own afterwards. A rain or storm sky is the dense, low,
//! closed cloud of `CloudKind::Overcast` and `CloudKind::Storm`, which also takes the sun's
//! share (the renderer's `sun_share`), so the light under it stays coherent with the sky.

use serde::{Deserialize, Serialize};

use crate::project::{CloudBlend, CloudKind, ViewState};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weather {
    /// No clouds, still air.
    Clear,
    /// Fair-weather cumulus over a third of the sky, a light breeze.
    Fair,
    /// Cumulus over most of the sky, some taller, a fresh breeze with gusts.
    Cloudy,
    /// A closed grey layer, soft shadows, a little haze of mist.
    Overcast,
    /// A low rain sky, a steady rain, everything wet.
    Rain,
    /// Towering storm clouds, a downpour, strong gusting wind.
    Storm,
}

/// The weather names, in order from fair to foul.
pub const WEATHERS: [&str; 6] = ["clear", "fair", "cloudy", "overcast", "rain", "storm"];

/// What a weather sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherState {
    pub kind: CloudKind,
    pub cover: f32,
    pub rain: f32,
    pub wetness: f32,
    pub mist: f32,
    pub gustiness: f32,
    pub wind_speed_mps: f32,
}

impl Weather {
    pub fn parse(name: &str) -> Option<Weather> {
        serde_json::from_value(serde_json::json!(name)).ok()
    }

    pub fn name(self) -> &'static str {
        WEATHERS[self as usize]
    }

    pub fn state(self) -> WeatherState {
        let s = |kind, cover, rain, wetness, mist, gustiness, wind_speed_mps| WeatherState {
            kind,
            cover,
            rain,
            wetness,
            mist,
            gustiness,
            wind_speed_mps,
        };
        use CloudKind::*;
        match self {
            Weather::Clear => s(Clear, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0),
            Weather::Fair => s(Cumulus, 0.3, 0.0, 0.0, 0.0, 0.2, 3.0),
            Weather::Cloudy => s(Cumulus, 0.7, 0.0, 0.0, 0.0, 0.35, 5.0),
            Weather::Overcast => s(Overcast, 0.95, 0.0, 0.0, 0.05, 0.3, 4.0),
            Weather::Rain => s(Overcast, 1.0, 0.6, 0.8, 0.15, 0.4, 6.0),
            Weather::Storm => s(Storm, 1.0, 0.95, 1.0, 0.1, 1.0, 13.0),
        }
    }
}

/// Writes `from`, or `from` mixed with `to` by its share (0..1), into `view`: the clouds' kind
/// and cover, the rain, the wetness, the mist, the gusts and the wind speed. The clouds' height,
/// speed, drawing and sun gap stay the view's own.
pub fn apply(view: &mut ViewState, from: Weather, to: Option<(Weather, f32)>) {
    let a = from.state();
    let (b, t) = match to {
        Some((w, t)) => (w.state(), t.clamp(0.0, 1.0)),
        None => (a, 0.0),
    };
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let clouds = &mut view.clouds;
    // A clear sky mixed with another is that other's cloud, thin: the cover carries the mix.
    let (kind, blend) = if t >= 1.0 || a.kind == b.kind {
        (if t >= 1.0 { b.kind } else { a.kind }, None)
    } else {
        (
            a.kind,
            Some(CloudBlend {
                kind: b.kind,
                share: t,
            }),
        )
    };
    clouds.kind = kind;
    clouds.blend = blend;
    clouds.cover = lerp(a.cover, b.cover);
    view.rain = lerp(a.rain, b.rain);
    view.wetness = lerp(a.wetness, b.wetness);
    view.mist = lerp(a.mist, b.mist);
    view.gustiness = lerp(a.gustiness, b.gustiness);
    view.wind_speed_mps = lerp(a.wind_speed_mps, b.wind_speed_mps);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_weather_has_a_name_and_a_valid_state() {
        for name in WEATHERS {
            let w = Weather::parse(name).unwrap();
            assert_eq!(w.name(), name);
            let mut view = ViewState::default();
            apply(&mut view, w, None);
            view.validate().unwrap();
        }
        assert_eq!(Weather::parse("drizzle"), None);
    }

    #[test]
    fn foul_weather_closes_the_sky_and_wets_the_ground() {
        let mut view = ViewState::default();
        apply(&mut view, Weather::Storm, None);
        let shape = view.clouds.shape();
        assert_eq!((shape.overcast, shape.storm), (1.0, 1.0));
        assert!(view.rain > 0.9 && view.wetness == 1.0 && view.gustiness == 1.0);
        apply(&mut view, Weather::Clear, None);
        assert_eq!(view.clouds.shape().cover, 0.0);
        assert_eq!((view.rain, view.wetness), (0.0, 0.0));
    }

    #[test]
    fn a_mix_eases_between_two_weathers() {
        let mut view = ViewState::default();
        apply(&mut view, Weather::Fair, Some((Weather::Rain, 0.5)));
        let shape = view.clouds.shape();
        assert!((shape.cover - 0.65).abs() < 1e-6);
        assert!((shape.overcast - 0.5).abs() < 1e-6);
        assert!((view.rain - 0.3).abs() < 1e-6);
        // The ends are the weathers themselves, with no blend left over.
        apply(&mut view, Weather::Fair, Some((Weather::Rain, 1.0)));
        assert_eq!(view.clouds.blend, None);
        assert_eq!(view.clouds.kind, CloudKind::Overcast);
        // From a clear sky the cover grows from nothing.
        apply(&mut view, Weather::Clear, Some((Weather::Cloudy, 0.25)));
        assert!((view.clouds.shape().cover - 0.175).abs() < 1e-6);
        view.validate().unwrap();
    }
}
