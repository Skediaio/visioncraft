//! Films: a camera path described as shots, so a person or an agent can compose one with
//! commands and have it rendered to numbered frames.
//!
//! A [`Film`] is a list of [`Shot`]s played one after the other, each with its own camera
//! (the join between two shots is a cut). A shot moves the camera along a path (walk, glide,
//! flyover), up (rise), round a spot (orbit) or not at all (hold). Inside a shot the camera is
//! eased by arc length (`ease`), and the sun, the date, the years of growth and the snow, the wetness, the rain, the
//! haze, the mist and the gusts can change along it. [`Timeline::camera_at`] answers where the camera is at a time, as a pure
//! function of the film and an [`Env`] (the ground the camera walks on, the viewpoints).
//!
//! Conventions: metres, Y up, +X east, -Z north. An orbit's angle is measured at the centre
//! from +X, **positive degrees counter-clockwise seen from above** (+X turns towards -Z), the
//! same as `rotation_y_deg`.

use crate::project::{Grade, Project, SunTime, YEARS_MAX};
use crate::season::Season;
use crate::viewpoint::Viewpoint;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

/// Longest shot, in seconds.
pub const SHOT_MAX_S: f32 = 3600.0;
/// Frames per second a film may have.
pub const FPS_MAX: u32 = 120;
const DEFAULT_FPS: u32 = 25;
const DEFAULT_FOV: f32 = 60.0;
const WALK_SPEED: f32 = 1.2;
const GLIDE_SPEED: f32 = 2.0;
const FLYOVER_SPEED: f32 = 8.0;
const DEFAULT_DURATION: f32 = 4.0;
/// How far ahead along the path a walking camera looks.
const LOOK_AHEAD_M: f32 = 4.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Film {
    pub id: String,
    pub name: String,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default)]
    pub shots: Vec<Shot>,
}

fn default_fps() -> u32 {
    DEFAULT_FPS
}

impl Film {
    pub fn new(id: String, name: String, fps: u32) -> Self {
        Self {
            id,
            name,
            fps,
            shots: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("the film needs a name".into());
        }
        if !(1..=FPS_MAX).contains(&self.fps) {
            return Err(format!("`fps` must be from 1 to {FPS_MAX}"));
        }
        Ok(())
    }

    /// Total length in seconds.
    pub fn duration(&self, env: &Env) -> Result<f32, String> {
        Ok(Timeline::new(self, env)?.duration_s)
    }

    /// Frames of the film: one per `1 / fps` s from 0, the last before the end.
    pub fn frame_count(&self, env: &Env) -> Result<usize, String> {
        Ok(Timeline::new(self, env)?.frame_count())
    }
}

/// The camera at film time `t` (seconds; clamped into the film).
pub fn camera_at(film: &Film, t: f32, env: &Env) -> Result<Frame, String> {
    Timeline::new(film, env)?.camera_at(t)
}

// ---------------------------------------------------------------------------------- shots

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    #[serde(flatten)]
    pub kind: ShotKind,
    /// How long the shot lasts; or give `speed_mps` and the length of the path decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_mps: Option<f32>,
    #[serde(default)]
    pub ease: Ease,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look_at: Option<Look>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov_y_deg: Option<f32>,
    /// Hours of the day at the start and the end of the shot (a single number: constant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_of_day: Option<Span<f32>>,
    /// The date at the start and the end of the shot; the season follows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<Span<Date>>,
    /// A season's own day (as `view.set season`); not together with `date`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<Season>,
    /// Years after planting at the start and the end of the shot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub years: Option<Span<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snow: Option<Span<f32>>,
    /// Wet surfaces at the start and the end of the shot (as `view.set wetness`): a shot can
    /// ease from dry to wet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wetness: Option<Span<f32>>,
    /// Rain at the start and the end of the shot (as `view.set rain`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rain: Option<Span<f32>>,
    /// Haze as a turbidity at the start and the end of the shot (as `view.set haze`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub haze: Option<Span<f32>>,
    /// Mist at the start and the end of the shot (as `view.set mist`): a shot can ease from a
    /// misty morning into a clear one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mist: Option<Span<f32>>,
    /// Gusts at the start and the end of the shot (as `view.set gustiness`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gustiness: Option<Span<f32>>,
    /// Light shafts at the start and the end of the shot (as `view.set shafts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shafts: Option<Span<f32>>,
    /// The weather at the start and the end of the shot (as `view.set weather`): two are
    /// mixed over the shot, eased in and out, so a sky clears or closes in. Rain, mist, gusts,
    /// wetness and clouds given on the shot as well win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weather: Option<Span<crate::Weather>>,
    /// The clouds' cover 0..1 at the start and the end of the shot (as `view.set clouds`'
    /// `cover`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clouds: Option<Span<f32>>,
    /// How often lightning flashes in a storm sky, 0..1, at the start and the end of the shot
    /// (as `view.set clouds.lightning`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightning: Option<Span<f32>>,
    /// How much of the lamps' light is on, 0..1, at the start and the end of the shot: lamps
    /// fade in instead of popping on at sunset. Any value forces the lamps on or off (else they
    /// follow the sun).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lights: Option<Span<f32>>,
    /// White balance in kelvin (2000..12000) at the start and the end of the shot (as
    /// `view.set white_balance`; 6500 is daylight).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub white_balance: Option<Span<f32>>,
    /// The colour grade at the start and the end of the shot; two grades are mixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<Span<Grade>>,
    /// Exposure compensation in stops **relative to the automatic exposure** at the start and the
    /// end of the shot: 0 is no correction, +1 one stop brighter, -1 darker. (`view.set
    /// exposure_ev` is the other thing: an absolute exposure value, where 0 is white daylight.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_ev: Option<Span<f32>>,
    /// `"continue"`: the shot starts where the one before ended, in position, look direction and
    /// speed, and its easing is blended across the boundary (no dip in speed). `"cut"` (the
    /// default): it starts where its own fields say. For walk, glide, flyover and orbit shots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<Join>,
}

/// How a shot follows the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    Cut,
    Continue,
}

/// A stop of a walk: the camera stands still at `point` for `s` seconds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pause {
    /// Where, `[x, z]`: the nearest point of the path.
    pub point: [f32; 2],
    pub s: f32,
}

/// A look of a walk: from `at_s` into the shot the camera turns to `target`, holds it for
/// `hold_s` seconds and turns back.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookStop {
    pub at_s: f32,
    pub target: [f32; 3],
    #[serde(default = "look_hold")]
    pub hold_s: f32,
}

fn look_hold() -> f32 {
    2.0
}

/// How long the camera takes to turn to a look and back.
const LOOK_TURN_S: f32 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShotKind {
    /// Eye height above the ground, along a path on the ground.
    Walk {
        path: Vec<[f32; 2]>,
        #[serde(default = "eye_height")]
        eye_height_m: f32,
        /// Stops along the path; the speed is solved so that walking and stopping fit
        /// `duration_s`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pause_at: Vec<Pause>,
        /// Looks away from the way ahead, by time in the shot.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        look: Vec<LookStop>,
    },
    /// A smooth curve (Catmull-Rom) through points in space.
    Glide { path: Vec<[f32; 3]> },
    /// Straight up (or down) over a spot.
    Rise {
        at: [f32; 2],
        from_height: f32,
        to_height: f32,
    },
    /// Round a centre at a radius, eye `height_m` above the centre's Y.
    Orbit {
        center: [f32; 3],
        radius_m: f32,
        height_m: f32,
        from_deg: f32,
        to_deg: f32,
    },
    /// Drone-like: `height_m` above the (smoothed) ground along a path, looking ahead and down.
    Flyover {
        path: Vec<[f32; 2]>,
        height_m: f32,
        #[serde(default = "pitch")]
        pitch_deg: f32,
    },
    /// A fixed camera: a viewpoint, or an eye and a target.
    Hold {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        viewpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        eye: Option<[f32; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<[f32; 3]>,
    },
}

fn eye_height() -> f32 {
    1.6
}

fn pitch() -> f32 {
    35.0
}

impl ShotKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Walk { .. } => "walk",
            Self::Glide { .. } => "glide",
            Self::Rise { .. } => "rise",
            Self::Orbit { .. } => "orbit",
            Self::Flyover { .. } => "flyover",
            Self::Hold { .. } => "hold",
        }
    }

    /// The fields of one kind of shot.
    pub fn fields(kind: &str) -> &'static [&'static str] {
        match kind {
            "walk" => &["path", "eye_height_m", "pause_at", "look"],
            "glide" => &["path"],
            "rise" => &["at", "from_height", "to_height"],
            "orbit" => &["center", "radius_m", "height_m", "from_deg", "to_deg"],
            "flyover" => &["path", "height_m", "pitch_deg"],
            "hold" => &["viewpoint", "eye", "target"],
            _ => &[],
        }
    }
}

/// The fields every kind of shot takes (the others are its kind's: [`ShotKind::fields`]).
pub const COMMON_FIELDS: &[&str] = &[
    "kind",
    "duration_s",
    "speed_mps",
    "ease",
    "look_at",
    "fov_y_deg",
    "time_of_day",
    "date",
    "season",
    "years",
    "snow",
    "wetness",
    "rain",
    "haze",
    "mist",
    "gustiness",
    "shafts",
    "weather",
    "clouds",
    "lightning",
    "lights",
    "white_balance",
    "grade",
    "exposure_ev",
    "join",
];

/// The highest haze (a turbidity) a shot or a picture takes: thick fog-like haze.
pub const HAZE_MAX: f32 = 50.0;

pub const SHOT_KINDS: &[&str] = &["walk", "glide", "rise", "orbit", "flyover", "hold"];

impl Shot {
    /// A shot from JSON, with unknown fields refused (naming the nearest), and checked.
    pub fn from_value(value: &Value, latitude: f64) -> Result<Shot, String> {
        let map = value.as_object().ok_or("a shot is an object")?;
        let kind = map
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("a shot needs `kind`: one of {}", SHOT_KINDS.join(", ")))?;
        if !SHOT_KINDS.contains(&kind) {
            return Err(format!(
                "unknown shot kind `{kind}`; the kinds are {}",
                SHOT_KINDS.join(", ")
            ));
        }
        let own = ShotKind::fields(kind);
        if kind == "walk" && map.contains_key("route") {
            return Err(
                "a walk shot has no field `route`: film.add_shot and film.update_shot plan it into the `path`".into(),
            );
        }
        for key in map.keys() {
            if !COMMON_FIELDS.contains(&key.as_str()) && !own.contains(&key.as_str()) {
                let fields = || COMMON_FIELDS.iter().chain(own.iter()).copied();
                let near = crate::spec::nearest(key, fields()).map_or_else(
                    || {
                        format!(
                            "; its fields are {}",
                            fields().collect::<Vec<_>>().join(", ")
                        )
                    },
                    |n| format!("; did you mean `{n}`?"),
                );
                return Err(format!("a {kind} shot has no field `{key}`{near}"));
            }
        }
        let shot: Shot = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        shot.validate(latitude)?;
        Ok(shot)
    }

    pub fn validate(&self, latitude: f64) -> Result<(), String> {
        let finite = |vals: &[f32]| vals.iter().all(|v| v.is_finite());
        let kind = self.kind.name();
        let err = |m: &str| Err(format!("{kind} shot: {m}"));
        if self.duration_s.is_some() && self.speed_mps.is_some() {
            return err("give `duration_s` or `speed_mps`, not both");
        }
        if let Some(d) = self.duration_s
            && !(d > 0.0 && d <= SHOT_MAX_S)
        {
            return err(&format!(
                "`duration_s` must be above 0 and at most {SHOT_MAX_S}"
            ));
        }
        if let Some(s) = self.speed_mps
            && !(s > 0.0 && s.is_finite())
        {
            return err("`speed_mps` must be above 0");
        }
        if let Some(f) = self.fov_y_deg
            && !(1.0..=179.0).contains(&f)
        {
            return err("`fov_y_deg` must be from 1 to 179");
        }
        if self.look_at.is_some() && matches!(self.kind, ShotKind::Hold { .. }) {
            return err("a hold has `target` instead of `look_at`");
        }
        if let Some(Look::At(p)) = self.look_at
            && !finite(&p)
        {
            return err("`look_at` must be finite numbers");
        }
        match &self.kind {
            ShotKind::Walk {
                path,
                eye_height_m,
                pause_at,
                look,
            } => {
                if path.len() < 2 || !path.iter().flatten().all(|v| v.is_finite()) {
                    return err("`path` needs at least 2 finite [x, z] points");
                }
                for p in pause_at {
                    if !(p.s > 0.0 && p.s <= SHOT_MAX_S && p.point.iter().all(|v| v.is_finite())) {
                        return err(&format!(
                            "`pause_at` needs a finite `point` [x, z] and `s` above 0 and at most {SHOT_MAX_S}"
                        ));
                    }
                }
                for l in look {
                    if !(l.at_s >= 0.0
                        && l.hold_s >= 0.0
                        && l.at_s + l.hold_s <= SHOT_MAX_S
                        && l.target.iter().all(|v| v.is_finite()))
                    {
                        return err(
                            "`look` needs `at_s` (0 or more), a finite `target` [x, y, z] and `hold_s` (0 or more)",
                        );
                    }
                }

                if !(0.0..=20.0).contains(eye_height_m) || *eye_height_m <= 0.0 {
                    return err("`eye_height_m` must be above 0 and at most 20");
                }
                if plan_length(path) < 0.05 {
                    return err("`path` has no length (all points are the same)");
                }
            }
            ShotKind::Glide { path } => {
                if path.len() < 2 || !path.iter().flatten().all(|v| v.is_finite()) {
                    return err("`path` needs at least 2 finite [x, y, z] points");
                }
                if path.windows(2).all(|w| dist3(w[0], w[1]) < 0.01) {
                    return err("`path` has no length (all points are the same)");
                }
            }
            ShotKind::Rise {
                at,
                from_height,
                to_height,
            } => {
                if !finite(&[at[0], at[1], *from_height, *to_height]) {
                    return err("`at`, `from_height` and `to_height` must be finite");
                }
                match self.look_at {
                    Some(Look::At(p)) => {
                        if (p[0] - at[0]).hypot(p[2] - at[1]) < 0.1 {
                            return err("`look_at` must be at least 0.1 m aside from `at`");
                        }
                    }
                    _ => return err("`look_at` [x, y, z] is required: what the camera looks at"),
                }
                if self.speed_mps.is_some() && from_height == to_height {
                    return err("`speed_mps` needs different heights; give `duration_s`");
                }
            }
            ShotKind::Orbit {
                center,
                radius_m,
                height_m,
                from_deg,
                to_deg,
            } => {
                if !finite(&[
                    center[0], center[1], center[2], *height_m, *from_deg, *to_deg,
                ]) {
                    return err("`center`, `height_m`, `from_deg` and `to_deg` must be finite");
                }
                if !(*radius_m > 0.0 && radius_m.is_finite()) {
                    return err("`radius_m` must be above 0");
                }
                if from_deg == to_deg {
                    return err("`from_deg` and `to_deg` must differ");
                }
            }
            ShotKind::Flyover {
                path,
                height_m,
                pitch_deg,
            } => {
                if path.len() < 2 || !path.iter().flatten().all(|v| v.is_finite()) {
                    return err("`path` needs at least 2 finite [x, z] points");
                }
                if plan_length(path) < 0.05 {
                    return err("`path` has no length (all points are the same)");
                }
                if !height_m.is_finite() || !(-1000.0..=10_000.0).contains(height_m) {
                    return err("`height_m` must be a number of metres");
                }
                if !(0.0..=89.0).contains(pitch_deg) {
                    return err("`pitch_deg` must be from 0 (level) to 89 (straight down)");
                }
            }
            ShotKind::Hold {
                viewpoint,
                eye,
                target,
            } => {
                if self.speed_mps.is_some() {
                    return err("a hold has no speed; give `duration_s`");
                }
                match (viewpoint, eye, target) {
                    (Some(_), None, None) => {}
                    (None, Some(e), Some(t)) => {
                        if !finite(e) || !finite(t) || e == t {
                            return err("`eye` and `target` must be finite and differ");
                        }
                    }
                    _ => return err("give a `viewpoint` id, or both `eye` and `target`"),
                }
            }
        }
        if let Some(s) = self.time_of_day
            && (!(0.0..24.0).contains(&s.from) || !(0.0..24.0).contains(&s.to))
        {
            return err("`time_of_day` hours must be from 0 to 24 (24 excluded)");
        }
        if let Some(s) = self.years
            && (!(0.0..=YEARS_MAX).contains(&s.from) || !(0.0..=YEARS_MAX).contains(&s.to))
        {
            return err(&format!("`years` must be from 0 to {YEARS_MAX}"));
        }
        if let Some(s) = self.snow
            && (!(0.0..=1.0).contains(&s.from) || !(0.0..=1.0).contains(&s.to))
        {
            return err("`snow` must be from 0 to 1");
        }
        for (name, span) in [
            ("wetness", self.wetness),
            ("rain", self.rain),
            ("mist", self.mist),
            ("gustiness", self.gustiness),
            ("shafts", self.shafts),
            ("clouds", self.clouds),
            ("lightning", self.lightning),
        ] {
            if let Some(s) = span
                && (!(0.0..=1.0).contains(&s.from) || !(0.0..=1.0).contains(&s.to))
            {
                return err(&format!("`{name}` must be from 0 to 1"));
            }
        }
        if let Some(s) = self.haze
            && (!(0.0..=HAZE_MAX).contains(&s.from) || !(0.0..=HAZE_MAX).contains(&s.to))
        {
            return err(&format!("`haze` must be from 0 to {HAZE_MAX}"));
        }
        if let Some(s) = self.lights
            && (!(0.0..=1.0).contains(&s.from) || !(0.0..=1.0).contains(&s.to))
        {
            return err("`lights` must be from 0 (off) to 1 (all the lamps' light)");
        }
        if let Some(s) = self.white_balance {
            let k = crate::project::WHITE_BALANCE_KELVIN;
            if !k.contains(&s.from) || !k.contains(&s.to) {
                return err(&format!(
                    "`white_balance` is a colour temperature from {} to {} kelvin (6500 is daylight)",
                    k.start(),
                    k.end()
                ));
            }
        }
        if let Some(s) = self.exposure_ev
            && (!(-8.0..=8.0).contains(&s.from) || !(-8.0..=8.0).contains(&s.to))
        {
            return err(
                "`exposure_ev` is a correction in stops from -8 to 8 relative to the automatic exposure (0: none)",
            );
        }
        if self.join == Some(Join::Continue)
            && !matches!(
                self.kind,
                ShotKind::Walk { .. }
                    | ShotKind::Glide { .. }
                    | ShotKind::Flyover { .. }
                    | ShotKind::Orbit { .. }
            )
        {
            return err(
                "`join: \"continue\"` is for walk, glide, flyover and orbit shots (a rise or a hold starts where its own fields say)",
            );
        }
        if self.date.is_some() && self.season.is_some() {
            return err("give `date` or `season`, not both");
        }
        if let Some(s) = self.date
            && (!s.from.valid() || !s.to.valid())
        {
            return err("`date` needs {month: 1..12, day: 1..31} that exists");
        }
        // Snow lies in winter only; refuse the shot that asks for both snow and another season.
        if self.snow.is_some_and(|s| s.from > 0.0 || s.to > 0.0) {
            let other = match (self.season, self.date) {
                (Some(s), _) => s != Season::Winter,
                (None, Some(d)) => [d.from, d.to]
                    .iter()
                    .any(|d| Season::of(d.month, latitude) != Season::Winter),
                _ => false,
            };
            if other {
                return err(
                    "snow lies only in winter: give `season: \"winter\"` or a winter `date`",
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// Starts slowly, ends at full speed.
    In,
    /// Starts at full speed, ends slowly.
    Out,
    /// Starts and ends slowly.
    #[default]
    InOut,
    /// Constant speed.
    None,
}

impl Ease {
    /// The share of the path covered `u` (0..1) of the way through the shot's time.
    pub fn apply(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Ease::In => u * u,
            Ease::Out => 1.0 - (1.0 - u) * (1.0 - u),
            Ease::InOut => u * u * (3.0 - 2.0 * u),
            Ease::None => u,
        }
    }
}

/// What the camera looks at: a point, or ahead along the path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    Ahead,
    At([f32; 3]),
}

impl Serialize for Look {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Look::Ahead => s.serialize_str("ahead"),
            Look::At(p) => p.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Look {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Word(String),
            Point([f32; 3]),
        }
        match Raw::deserialize(d)? {
            Raw::Point(p) => Ok(Look::At(p)),
            Raw::Word(w) if w == "ahead" => Ok(Look::Ahead),
            Raw::Word(w) => Err(serde::de::Error::custom(format!(
                "`look_at` is [x, y, z] or \"ahead\", got \"{w}\""
            ))),
        }
    }
}

/// A value at the start and the end of a shot; one number or date alone is constant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span<T> {
    pub from: T,
    pub to: T,
}

impl<T: Serialize + PartialEq + Copy> Serialize for Span<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.from == self.to {
            self.from.serialize(s)
        } else {
            [self.from, self.to].serialize(s)
        }
    }
}

impl<'de, T: Deserialize<'de> + Copy> Deserialize<'de> for Span<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw<T> {
            Two([T; 2]),
            One(T),
        }
        Ok(match Raw::<T>::deserialize(d)? {
            Raw::Two([from, to]) => Span { from, to },
            Raw::One(v) => Span { from: v, to: v },
        })
    }
}

impl Span<f32> {
    pub fn at(&self, u: f32) -> f32 {
        self.from + (self.to - self.from) * u
    }
}

/// A day of the year (no year: the calendar of a non-leap year).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Date {
    pub month: u32,
    pub day: u32,
}

const MONTH_DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

impl Date {
    pub fn valid(&self) -> bool {
        (1..=12).contains(&self.month)
            && (1..=MONTH_DAYS[self.month as usize - 1]).contains(&self.day)
    }

    fn day_of_year(&self) -> f32 {
        (MONTH_DAYS[..self.month as usize - 1].iter().sum::<u32>() + self.day) as f32
    }

    fn from_day_of_year(doy: f32) -> Date {
        let mut left = doy.round().clamp(1.0, 365.0) as u32;
        for (m, days) in MONTH_DAYS.iter().enumerate() {
            if left <= *days {
                return Date {
                    month: m as u32 + 1,
                    day: left,
                };
            }
            left -= days;
        }
        Date { month: 12, day: 31 }
    }
}

impl Span<Date> {
    pub fn at(&self, u: f32) -> Date {
        if self.from == self.to {
            return self.from;
        }
        let (a, b) = (self.from.day_of_year(), self.to.day_of_year());
        Date::from_day_of_year(a + (b - a) * u)
    }
}

// ------------------------------------------------------------------------------ environment

/// What a film is played over: the ground the eye follows, the project's viewpoints and its
/// latitude (the season of a date).
pub struct Env<'a> {
    pub ground: &'a dyn Fn(f32, f32) -> f32,
    pub viewpoints: &'a [Viewpoint],
    pub latitude: f64,
}

impl<'a> Env<'a> {
    pub fn new(project: &'a Project, ground: &'a dyn Fn(f32, f32) -> f32) -> Env<'a> {
        Env {
            ground,
            viewpoints: &project.viewpoints,
            latitude: project.site.latitude,
        }
    }
}

/// The ground height at `(x, z)` the core knows: a terrace's floor, the terrain, else 0.
pub fn terrain_ground(project: &Project, x: f32, z: f32) -> f32 {
    crate::terrace::ground_fn(project)(x, z).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------------- frames

/// What changes for one picture, as `view.render` takes it. Unset: the project's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Overrides {
    /// Local hour of the day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_of_day: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<Date>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season: Option<Season>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub years: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snow: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wetness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rain: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub haze: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mist: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gustiness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shafts: Option<f32>,
    /// The weather, and when two are mixed the second and how much of it (0..1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weather: Option<crate::Weather>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weather_to: Option<crate::Weather>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weather_mix: Option<f32>,
    /// The clouds' cover 0..1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clouds: Option<f32>,
    /// How often lightning flashes 0..1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lightning: Option<f32>,
    /// Lamp light 0..1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lights: Option<f32>,
    /// Kelvin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white_balance: Option<f32>,
    /// The grade, and when two are mixed the second and how much of it (0..1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade: Option<Grade>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade_to: Option<Grade>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade_mix: Option<f32>,
    /// Stops relative to the automatic exposure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure_ev: Option<f32>,
}

impl Overrides {
    /// The `view.render` parameters (`time`, `season`, `years`, `snow`) for this frame.
    pub fn render_params(&self) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        let mut time = serde_json::Map::new();
        if let Some(h) = self.time_of_day {
            time.insert("hour".into(), json!(h));
        }
        if let Some(d) = self.date {
            time.insert("month".into(), d.month.into());
            time.insert("day".into(), d.day.into());
        }
        if !time.is_empty() {
            m.insert("time".into(), time.into());
        }
        if let Some(s) = self.season {
            m.insert("season".into(), s.name().into());
        }
        if let Some(y) = self.years {
            m.insert("years".into(), json!(y));
        }
        if let Some(s) = self.snow {
            m.insert("snow".into(), json!(s));
        }
        if let Some(s) = self.wetness {
            m.insert("wetness".into(), json!(s));
        }
        if let Some(s) = self.rain {
            m.insert("rain".into(), json!(s));
        }
        if let Some(s) = self.haze {
            m.insert("haze".into(), json!(s));
        }
        if let Some(s) = self.mist {
            m.insert("mist".into(), json!(s));
        }
        if let Some(s) = self.gustiness {
            m.insert("gustiness".into(), json!(s));
        }
        if let Some(s) = self.shafts {
            m.insert("shafts".into(), json!(s));
        }
        if let Some(w) = self.weather {
            m.insert("weather".into(), w.name().into());
        }
        if let Some(w) = self.weather_to {
            m.insert("weather_to".into(), w.name().into());
        }
        if let Some(x) = self.weather_mix {
            m.insert("weather_mix".into(), json!(x));
        }
        if let Some(c) = self.clouds {
            m.insert("clouds".into(), json!(c));
        }
        if let Some(l) = self.lightning {
            m.insert("lightning".into(), json!(l));
        }
        if let Some(s) = self.lights {
            m.insert("lights".into(), json!(s));
        }
        if let Some(k) = self.white_balance {
            m.insert("white_balance".into(), json!(k));
        }
        if let Some(g) = self.grade {
            m.insert(
                "grade".into(),
                serde_json::to_value(g).unwrap_or(Value::Null),
            );
        }
        if let Some(g) = self.grade_to {
            m.insert(
                "grade_to".into(),
                serde_json::to_value(g).unwrap_or(Value::Null),
            );
        }
        if let Some(x) = self.grade_mix {
            m.insert("grade_mix".into(), json!(x));
        }
        if let Some(e) = self.exposure_ev {
            m.insert("exposure_offset_ev".into(), json!(e));
        }
        m
    }
}

/// The camera and the picture's changes at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub fov_y_deg: f32,
    pub overrides: Overrides,
    /// Index of the shot the time falls in.
    pub shot: usize,
    /// How far through that shot, 0..1, by time.
    pub progress: f32,
}

// --------------------------------------------------------------------------------- timeline

/// A film with its shots' paths worked out over an [`Env`]: durations and cameras.
pub struct Timeline {
    shots: Vec<Built>,
    pub duration_s: f32,
    pub fps: u32,
}

struct Built {
    shot: Shot,
    start: f32,
    duration: f32,
    geom: Geom,
    /// A hold on a viewpoint carries the viewpoint's own time and years.
    base: Overrides,
    fov: f32,
    /// How far along the path (or round the orbit) the camera is at a time in the shot.
    motion: Motion,
    /// The shot takes over from the one before it.
    join: Option<JoinState>,
}

/// What a joined shot blends away from the end of the shot before it.
struct JoinState {
    /// Where the previous camera ended, minus where this shot would start: decays to 0.
    eye_offset: [f32; 3],
    /// The previous look direction: blends into this shot's.
    dir: [f32; 3],
    blend_s: f32,
}

/// The camera at the end of a shot.
struct EndState {
    eye: [f32; 3],
    dir: [f32; 3],
    speed_mps: f32,
}

enum Geom {
    /// The eye on a dense path; `pitch_deg` looks down for a flyover, `level` keeps the gaze
    /// horizontal (a walk).
    Path {
        path: Path3,
        level: bool,
        pitch_deg: f32,
    },
    Rise {
        at: [f32; 2],
        ground: f32,
        from: f32,
        to: f32,
    },
    Orbit,
    Hold {
        eye: [f32; 3],
        target: [f32; 3],
    },
}

/// The share of the way covered at a time in the shot.
enum Motion {
    /// The shot's own ease over its whole time.
    Ease(Ease),
    /// Legs of walking and standing still, each a cubic from `e0` to `e1` of the way over
    /// `t0..t1` seconds with the slopes `m0` and `m1` (the leg's own length over its own time
    /// is 1) at its ends.
    Legs(Vec<Leg>),
}

struct Leg {
    t0: f32,
    t1: f32,
    e0: f32,
    e1: f32,
    m0: f32,
    m1: f32,
}

impl Motion {
    fn share(&self, ts: f32, duration: f32) -> f32 {
        match self {
            Motion::Ease(e) => e.apply(ts / duration),
            Motion::Legs(legs) => {
                let Some(last) = legs.last() else { return 0.0 };
                if ts >= last.t1 {
                    return last.e1;
                }
                let leg = legs.iter().find(|l| ts < l.t1).unwrap_or(last);
                if (leg.e1 - leg.e0).abs() < 1e-9 {
                    return leg.e0;
                }
                let u = ((ts - leg.t0) / (leg.t1 - leg.t0).max(1e-9)).clamp(0.0, 1.0);
                let (u2, u3) = (u * u, u * u * u);
                let p = (u3 - 2.0 * u2 + u) * leg.m0 + (-2.0 * u3 + 3.0 * u2) + (u3 - u2) * leg.m1;
                leg.e0 + (leg.e1 - leg.e0) * p
            }
        }
    }
}

fn smooth01(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add_scaled(a: [f32; 3], b: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// The direction `t` of the way from unit vector `a` to `b` at a constant turning rate.
fn slerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
    let angle = dot.acos();
    if angle < 1e-4 || angle > std::f32::consts::PI - 1e-3 {
        return normalize(lerp3(a, b, t));
    }
    let (wa, wb) = (((1.0 - t) * angle).sin(), (t * angle).sin());
    normalize(std::array::from_fn(|k| {
        (a[k] * wa + b[k] * wb) / angle.sin()
    }))
}

impl Timeline {
    pub fn new(film: &Film, env: &Env) -> Result<Timeline, String> {
        let mut start = 0.0;
        let mut shots: Vec<Built> = Vec::new();
        for (i, shot) in film.shots.iter().enumerate() {
            let b = build(shot, start, env, shots.last()).map_err(|e| format!("shot {i}: {e}"))?;
            start += b.duration;
            shots.push(b);
        }
        Ok(Timeline {
            shots,
            duration_s: start,
            fps: film.fps,
        })
    }

    pub fn frame_count(&self) -> usize {
        (self.duration_s * self.fps as f32 - 1e-3).ceil().max(0.0) as usize
    }

    /// Where each shot starts and how long it lasts: `(start_s, duration_s)`.
    pub fn spans(&self) -> Vec<(f32, f32)> {
        self.shots.iter().map(|b| (b.start, b.duration)).collect()
    }

    pub fn camera_at(&self, t: f32) -> Result<Frame, String> {
        if self.shots.is_empty() {
            return Err("the film has no shots".into());
        }
        let t = t.clamp(0.0, self.duration_s);
        let i = self
            .shots
            .partition_point(|b| b.start + b.duration <= t)
            .min(self.shots.len() - 1);
        let b = &self.shots[i];
        let u = ((t - b.start) / b.duration).clamp(0.0, 1.0);
        let (eye, target) = shot_camera(b, t - b.start)?;
        let eye_target = if eye == target {
            [target[0], target[1], target[2] - 0.01]
        } else {
            target
        };
        Ok(Frame {
            eye,
            target: eye_target,
            fov_y_deg: b.fov,
            overrides: overrides_at(&b.shot, b.base, u),
            shot: i,
            progress: u,
        })
    }
}

/// The camera `ts` seconds into a shot: its path, then its looks, then the blend from the shot
/// before.
fn shot_camera(b: &Built, ts: f32) -> Result<([f32; 3], [f32; 3]), String> {
    let (mut eye, mut target) = natural_camera(b, ts)?;
    if let ShotKind::Walk { look, .. } = &b.shot.kind {
        let mut best: Option<(f32, [f32; 3])> = None;
        for l in look {
            let up = smooth01((ts - l.at_s) / LOOK_TURN_S);
            let down = 1.0 - smooth01((ts - (l.at_s + l.hold_s)) / LOOK_TURN_S);
            let w = up * down;
            if w > 0.0 && best.is_none_or(|(bw, _)| w > bw) {
                best = Some((w, l.target));
            }
        }
        if let Some((w, at)) = best {
            let dist = dist3(eye, target).max(1.0);
            let d = slerp3(normalize(sub3(target, eye)), normalize(sub3(at, eye)), w);
            target = add_scaled(eye, d, dist);
        }
    }
    if let Some(j) = &b.join {
        let w = smooth01(ts / j.blend_s);
        eye = add_scaled(eye, j.eye_offset, 1.0 - w);
        let dist = dist3(eye, target).max(1.0);
        let d = slerp3(j.dir, normalize(sub3(target, eye)), w);
        target = add_scaled(eye, d, dist);
    }
    Ok((eye, target))
}

/// The camera of the shot's own fields `ts` seconds in.
fn natural_camera(b: &Built, ts: f32) -> Result<([f32; 3], [f32; 3]), String> {
    let ts = ts.clamp(0.0, b.duration);
    let e = b.motion.share(ts, b.duration);
    Ok(match (&b.geom, &b.shot.kind) {
        (
            Geom::Path {
                path,
                level,
                pitch_deg,
            },
            _,
        ) => {
            let s = e * path.len();
            let eye = path.at(s);
            let target = match b.shot.look_at {
                Some(Look::At(p)) => p,
                _ if *pitch_deg > 0.0 => {
                    let a = path.ahead(s, 1.0);
                    let h = normalize([a[0] - eye[0], 0.0, a[2] - eye[2]]);
                    let (c, sn) = (
                        (*pitch_deg).to_radians().cos(),
                        (*pitch_deg).to_radians().sin(),
                    );
                    let d = 30.0;
                    [
                        eye[0] + h[0] * c * d,
                        eye[1] - sn * d,
                        eye[2] + h[2] * c * d,
                    ]
                }
                _ => {
                    let a = path.ahead(s, LOOK_AHEAD_M);
                    [a[0], if *level { eye[1] } else { a[1] }, a[2]]
                }
            };
            (eye, target)
        }
        (
            Geom::Rise {
                at,
                ground,
                from,
                to,
            },
            ShotKind::Rise { .. },
        ) => {
            let h = from + (to - from) * e;
            let Some(Look::At(target)) = b.shot.look_at else {
                return Err("a rise needs `look_at`".into());
            };
            ([at[0], ground + h, at[1]], target)
        }
        (
            Geom::Orbit,
            ShotKind::Orbit {
                center,
                radius_m,
                height_m,
                from_deg,
                to_deg,
            },
        ) => {
            let a = (from_deg + (to_deg - from_deg) * e).to_radians();
            let eye = [
                center[0] + radius_m * a.cos(),
                center[1] + height_m,
                center[2] - radius_m * a.sin(),
            ];
            let target = match b.shot.look_at {
                Some(Look::At(p)) => p,
                _ => *center,
            };
            (eye, target)
        }
        (Geom::Hold { eye, target }, _) => (*eye, *target),
        _ => return Err("the shot does not match its path".into()),
    })
}

fn end_state(prev: &Built) -> Result<EndState, String> {
    let (eye, target) = shot_camera(prev, prev.duration)?;
    let t0 = (prev.duration - 1.0 / 120.0).max(0.0);
    let (before, _) = shot_camera(prev, t0)?;
    let dt = (prev.duration - t0).max(1e-4);
    Ok(EndState {
        eye,
        dir: normalize(sub3(target, eye)),
        speed_mps: dist3(before, eye) / dt,
    })
}

fn overrides_at(shot: &Shot, base: Overrides, u: f32) -> Overrides {
    let mut o = base;
    if let Some(s) = shot.time_of_day {
        o.time_of_day = Some(s.at(u));
    }
    if let Some(s) = shot.date {
        o.date = Some(s.at(u));
        o.season = None;
    }
    if let Some(s) = shot.season {
        o.season = Some(s);
        o.date = None;
    }
    if let Some(s) = shot.years {
        o.years = Some(s.at(u));
    }
    if let Some(s) = shot.snow {
        o.snow = Some(s.at(u));
    }
    if let Some(s) = shot.wetness {
        o.wetness = Some(s.at(u));
    }
    if let Some(s) = shot.rain {
        o.rain = Some(s.at(u));
    }
    if let Some(s) = shot.haze {
        o.haze = Some(s.at(u));
    }
    if let Some(s) = shot.mist {
        o.mist = Some(s.at(u));
    }
    if let Some(s) = shot.gustiness {
        o.gustiness = Some(s.at(u));
    }
    if let Some(s) = shot.shafts {
        o.shafts = Some(s.at(u));
    }
    if let Some(s) = shot.weather {
        o.weather = Some(s.from);
        (o.weather_to, o.weather_mix) = if s.from == s.to {
            (None, None)
        } else {
            // Eased in and out: a front arrives and settles.
            (Some(s.to), Some(u * u * (3.0 - 2.0 * u)))
        };
    }
    if let Some(s) = shot.clouds {
        o.clouds = Some(s.at(u));
    }
    if let Some(s) = shot.lightning {
        o.lightning = Some(s.at(u));
    }
    if let Some(s) = shot.lights {
        o.lights = Some(s.at(u));
    }
    if let Some(s) = shot.white_balance {
        o.white_balance = Some(s.at(u));
    }
    if let Some(s) = shot.grade {
        o.grade = Some(s.from);
        if s.from != s.to {
            o.grade_to = Some(s.to);
            o.grade_mix = Some(u);
        }
    }
    if let Some(s) = shot.exposure_ev {
        o.exposure_ev = Some(s.at(u));
    }
    o
}

/// A stop or a stretch of a walk, in order.
enum Part {
    Walk { from: f32, to: f32 },
    Wait { at: f32, secs: f32 },
}

/// The legs of a shot: its stretches of walking between the waits at `stops` (arc length,
/// seconds), `move_time` seconds of walking in all over `len`, the first stretch taking over
/// the speed `v0` when the shot continues one before.
fn legs_of(
    ease: Ease,
    len: f32,
    move_time: f32,
    stops: &[(f32, f32)],
    v0: Option<f32>,
) -> Vec<Leg> {
    let mut parts = Vec::new();
    let mut cur = 0.0;
    for &(at, secs) in stops {
        if at > cur + 1e-4 {
            parts.push(Part::Walk { from: cur, to: at });
        }
        parts.push(Part::Wait { at, secs });
        cur = at.max(cur);
    }
    if len > cur + 1e-4 {
        parts.push(Part::Walk { from: cur, to: len });
    }
    let soft_start = matches!(ease, Ease::In | Ease::InOut);
    let soft_end = matches!(ease, Ease::Out | Ease::InOut);
    let mut legs = Vec::new();
    let mut t = 0.0;
    for (k, part) in parts.iter().enumerate() {
        match part {
            Part::Wait { at, secs } => {
                legs.push(Leg {
                    t0: t,
                    t1: t + secs,
                    e0: at / len,
                    e1: at / len,
                    m0: 0.0,
                    m1: 0.0,
                });
                t += secs;
            }
            Part::Walk { from, to } => {
                let dur = move_time * (to - from) / len;
                let first = k == 0;
                let last = k + 1 == parts.len();
                let after_wait = k > 0 && matches!(parts[k - 1], Part::Wait { .. });
                let before_wait = !last && matches!(parts[k + 1], Part::Wait { .. });
                let m0 = if after_wait || (first && v0.is_none() && soft_start) {
                    0.0
                } else if first && let Some(v) = v0 {
                    (v * dur / (to - from).max(1e-6)).clamp(0.0, 3.0)
                } else if first && ease == Ease::Out {
                    2.0
                } else {
                    1.0
                };
                let m1 = if before_wait || (last && soft_end) {
                    0.0
                } else if last && ease == Ease::In {
                    2.0
                } else {
                    1.0
                };
                legs.push(Leg {
                    t0: t,
                    t1: t + dur,
                    e0: from / len,
                    e1: to / len,
                    m0,
                    m1,
                });
                t += dur;
            }
        }
    }
    legs
}

fn build(shot: &Shot, start: f32, env: &Env, prev: Option<&Built>) -> Result<Built, String> {
    let mut base = Overrides::default();
    let mut fov = shot.fov_y_deg.unwrap_or(DEFAULT_FOV);
    let joined = shot.join == Some(Join::Continue);
    let prev_end = if joined {
        let prev = prev.ok_or("`join: \"continue\"` needs a shot before it to continue")?;
        Some(end_state(prev)?)
    } else {
        None
    };
    // Where the shot's stops lie on its path: (arc length, seconds).
    let mut stops: Vec<(f32, f32)> = Vec::new();
    let (geom, length, default_speed) = match &shot.kind {
        ShotKind::Walk {
            path,
            eye_height_m,
            pause_at,
            ..
        } => {
            let mut pts = path.clone();
            if let Some(e) = &prev_end {
                let a = [e.eye[0], e.eye[2]];
                if (pts[0][0] - a[0]).hypot(pts[0][1] - a[1]) > 0.05 {
                    pts.insert(0, a);
                }
            }
            let plan = densify(&pts, 0.25);
            let ground: Vec<f32> = plan.iter().map(|p| (env.ground)(p[0], p[1])).collect();
            let ground = smooth(&ground, 3);
            let pts3: Vec<[f32; 3]> = plan
                .iter()
                .zip(&ground)
                .map(|(p, g)| [p[0], g + eye_height_m, p[1]])
                .collect();
            let path = Path3::new(pts3);
            for p in pause_at {
                let nearest = plan
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        let d = |q: &[f32; 2]| (q[0] - p.point[0]).hypot(q[1] - p.point[1]);
                        d(a.1).total_cmp(&d(b.1))
                    })
                    .map_or(0, |(i, _)| i);
                stops.push((path.cum[nearest], p.s));
            }
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            let len = path.len();
            (
                Geom::Path {
                    path,
                    level: true,
                    pitch_deg: 0.0,
                },
                Some(len),
                WALK_SPEED,
            )
        }
        ShotKind::Glide { path } => {
            let mut pts = path.clone();
            if let Some(e) = &prev_end
                && dist3(pts[0], e.eye) > 0.05
            {
                pts.insert(0, e.eye);
            }
            let path = Path3::new(catmull_rom(&pts));
            let len = path.len();
            (
                Geom::Path {
                    path,
                    level: false,
                    pitch_deg: 0.0,
                },
                Some(len),
                GLIDE_SPEED,
            )
        }
        ShotKind::Flyover {
            path,
            height_m,
            pitch_deg,
        } => {
            let mut pts = path.clone();
            if let Some(e) = &prev_end {
                let a = [e.eye[0], e.eye[2]];
                if (pts[0][0] - a[0]).hypot(pts[0][1] - a[1]) > 0.05 {
                    pts.insert(0, a);
                }
            }
            let plan = densify(&pts, 0.5);
            let ground: Vec<f32> = plan.iter().map(|p| (env.ground)(p[0], p[1])).collect();
            let ground = smooth(&ground, 12);
            let pts3: Vec<[f32; 3]> = plan
                .iter()
                .zip(&ground)
                .map(|(p, g)| [p[0], g + height_m, p[1]])
                .collect();
            let path = Path3::new(pts3);
            let len = path.len();
            (
                Geom::Path {
                    path,
                    level: false,
                    pitch_deg: *pitch_deg,
                },
                Some(len),
                FLYOVER_SPEED,
            )
        }
        ShotKind::Rise {
            at,
            from_height,
            to_height,
        } => (
            Geom::Rise {
                at: *at,
                ground: (env.ground)(at[0], at[1]),
                from: *from_height,
                to: *to_height,
            },
            Some((to_height - from_height).abs()),
            1.0,
        ),
        ShotKind::Orbit {
            radius_m,
            from_deg,
            to_deg,
            ..
        } => (
            Geom::Orbit,
            Some(radius_m * (to_deg - from_deg).abs().to_radians()),
            2.0,
        ),
        ShotKind::Hold {
            viewpoint,
            eye,
            target,
        } => {
            let (eye, target) = match (viewpoint, eye, target) {
                (Some(id), _, _) => {
                    let v = env
                        .viewpoints
                        .iter()
                        .find(|v| v.id == *id || v.name == *id)
                        .ok_or_else(|| format!("no viewpoint `{id}`"))?;
                    if shot.fov_y_deg.is_none() {
                        fov = v.fov_y_deg;
                    }
                    if let Some(t) = v.time {
                        base.time_of_day = Some(t.hour);
                        base.date = Some(Date {
                            month: t.month,
                            day: t.day,
                        });
                    }
                    base.years = v.years;
                    (v.position, v.target)
                }
                (None, Some(e), Some(t)) => (*e, *t),
                _ => return Err("a hold needs a viewpoint or an eye and a target".into()),
            };
            (Geom::Hold { eye, target }, None, 1.0)
        }
    };
    // The time of the stops is standing still: what is left is walking.
    let wait: f32 = stops.iter().map(|s| s.1).sum();
    let speed = match &prev_end {
        Some(e) if e.speed_mps > 0.05 => e.speed_mps,
        _ => default_speed,
    };
    let move_time = match (shot.duration_s, shot.speed_mps, length) {
        (Some(d), _, _) => d - wait,
        (None, Some(v), Some(l)) => l / v,
        (None, None, Some(l))
            if !matches!(shot.kind, ShotKind::Rise { .. } | ShotKind::Orbit { .. }) =>
        {
            l / speed
        }
        _ => DEFAULT_DURATION - wait,
    };
    if !(move_time > 0.0 && move_time.is_finite()) {
        return Err(if wait > 0.0 {
            format!(
                "`duration_s` leaves no time to walk: the pauses alone take {wait} s; give a longer `duration_s`"
            )
        } else {
            "the shot has no length in time; give `duration_s`".into()
        });
    }
    let duration = move_time + wait;
    let motion = match length {
        Some(len) if len > 1e-4 && (joined || !stops.is_empty()) => Motion::Legs(legs_of(
            shot.ease,
            len,
            move_time,
            &stops,
            prev_end.as_ref().map(|e| e.speed_mps),
        )),
        _ => Motion::Ease(shot.ease),
    };
    let mut built = Built {
        shot: shot.clone(),
        start,
        duration,
        geom,
        base,
        fov,
        motion,
        join: None,
    };
    if let Some(e) = prev_end {
        let (eye0, _) = natural_camera(&built, 0.0)?;
        built.join = Some(JoinState {
            eye_offset: sub3(e.eye, eye0),
            dir: e.dir,
            blend_s: (0.4 * duration).clamp(0.05, 1.2),
        });
    }
    Ok(built)
}

// ---------------------------------------------------------------------------- path helpers

fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-9 {
        [0.0, 0.0, -1.0]
    } else {
        [v[0] / l, v[1] / l, v[2] / l]
    }
}

fn plan_length(path: &[[f32; 2]]) -> f32 {
    path.windows(2)
        .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
        .sum()
}

/// The path with points added so none is further than `step` from the next.
fn densify(path: &[[f32; 2]], step: f32) -> Vec<[f32; 2]> {
    let mut out = vec![path[0]];
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

/// A moving average over `radius` samples either side (fewer at the ends): steps and kerbs
/// do not jerk the camera.
fn smooth(values: &[f32], radius: usize) -> Vec<f32> {
    (0..values.len())
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(radius), (i + radius).min(values.len() - 1));
            values[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32
        })
        .collect()
}

/// A path through points with the length so far at each, to move along by distance.
struct Path3 {
    pts: Vec<[f32; 3]>,
    cum: Vec<f32>,
}

impl Path3 {
    fn new(pts: Vec<[f32; 3]>) -> Path3 {
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum[cum.len() - 1] + dist3(w[0], w[1]));
        }
        Path3 { pts, cum }
    }

    fn len(&self) -> f32 {
        *self.cum.last().unwrap_or(&0.0)
    }

    fn at(&self, s: f32) -> [f32; 3] {
        let s = s.clamp(0.0, self.len());
        let i = self
            .cum
            .partition_point(|&c| c <= s)
            .saturating_sub(1)
            .min(self.pts.len().saturating_sub(2));
        let (a, b) = (self.pts[i], self.pts[(i + 1).min(self.pts.len() - 1)]);
        let seg = self.cum[(i + 1).min(self.pts.len() - 1)] - self.cum[i];
        if seg <= 1e-9 {
            return a;
        }
        let t = (s - self.cum[i]) / seg;
        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
    }

    /// The point `d` further along than `s`; past the end, on the line of the last stretch.
    fn ahead(&self, s: f32, d: f32) -> [f32; 3] {
        let target = s + d;
        if target <= self.len() {
            return self.at(target);
        }
        let end = self.at(self.len());
        let before = self.at((self.len() - 0.3).max(0.0));
        let dir = normalize([end[0] - before[0], end[1] - before[1], end[2] - before[2]]);
        let extra = target - self.len();
        std::array::from_fn(|k| end[k] + dir[k] * extra)
    }
}

/// A centripetal Catmull-Rom curve through `points`, sampled finely.
fn catmull_rom(points: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let mut p: Vec<[f32; 3]> = Vec::new();
    for q in points {
        if p.last().is_none_or(|l| dist3(*l, *q) > 1e-4) {
            p.push(*q);
        }
    }
    if p.len() < 3 {
        return p;
    }
    let n = p.len();
    let ext = |i: isize| -> [f32; 3] {
        if i < 0 {
            std::array::from_fn(|k| 2.0 * p[0][k] - p[1][k])
        } else if i as usize >= n {
            std::array::from_fn(|k| 2.0 * p[n - 1][k] - p[n - 2][k])
        } else {
            p[i as usize]
        }
    };
    let mix = |a: [f32; 3], b: [f32; 3], wa: f32, wb: f32| -> [f32; 3] {
        std::array::from_fn(|k| a[k] * wa + b[k] * wb)
    };
    let mut out = vec![p[0]];
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (ext(i as isize - 1), p[i], p[i + 1], ext(i as isize + 2));
        let t1 = dist3(p0, p1).sqrt();
        let t2 = t1 + dist3(p1, p2).sqrt();
        let t3 = t2 + dist3(p2, p3).sqrt();
        let (t0, t1, t2, t3) = (0.0, t1, t2, t3);
        let steps = ((dist3(p1, p2) / 0.25).ceil() as usize).clamp(8, 400);
        for s in 1..=steps {
            let t = t1 + (t2 - t1) * s as f32 / steps as f32;
            let w = |a: f32, b: f32, x: f32| {
                if (b - a).abs() < 1e-9 {
                    0.5
                } else {
                    (x - a) / (b - a)
                }
            };
            let a1 = mix(p0, p1, 1.0 - w(t0, t1, t), w(t0, t1, t));
            let a2 = mix(p1, p2, 1.0 - w(t1, t2, t), w(t1, t2, t));
            let a3 = mix(p2, p3, 1.0 - w(t2, t3, t), w(t2, t3, t));
            let b1 = mix(a1, a2, 1.0 - w(t0, t2, t), w(t0, t2, t));
            let b2 = mix(a2, a3, 1.0 - w(t1, t3, t), w(t1, t3, t));
            out.push(mix(b1, b2, 1.0 - w(t1, t2, t), w(t1, t2, t)));
        }
    }
    out
}

/// A shot's time of the sun as a project time: handy for tools that show it.
pub fn sun_time(base: SunTime, o: &Overrides, latitude: f64) -> SunTime {
    let mut t = base;
    if let Some(s) = o.season {
        t = s.time(t, latitude);
    }
    if let Some(d) = o.date {
        t.month = d.month;
        t.day = d.day;
    }
    if let Some(h) = o.time_of_day {
        t.hour = h;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_: f32, _: f32) -> f32 {
        0.0
    }

    fn env<'a>(ground: &'a dyn Fn(f32, f32) -> f32, vps: &'a [Viewpoint]) -> Env<'a> {
        Env {
            ground,
            viewpoints: vps,
            latitude: 50.8,
        }
    }

    fn shot(v: Value) -> Shot {
        Shot::from_value(&v, 50.8).unwrap()
    }

    fn film(shots: Vec<Shot>) -> Film {
        Film {
            id: "f1".into(),
            name: "Test".into(),
            fps: 25,
            shots,
        }
    }

    #[test]
    fn a_walk_keeps_its_eye_height_and_speed() {
        let slope = |_: f32, z: f32| 0.05 * z;
        let ground: &dyn Fn(f32, f32) -> f32 = &slope;
        let f = film(vec![shot(json!({
            "kind": "walk", "path": [[0, 0], [0, 12], [8, 12]], "speed_mps": 1.2, "ease": "none"
        }))]);
        let e = env(ground, &[]);
        let tl = Timeline::new(&f, &e).unwrap();
        // 20 m at a bit over 1.2 m/s along the slope
        assert!(
            (tl.duration_s - 20.0 / 1.2).abs() < 0.3,
            "{}",
            tl.duration_s
        );
        let mut prev: Option<[f32; 3]> = None;
        let mut steps = Vec::new();
        for i in 0..tl.frame_count() - 1 {
            let fr = tl.camera_at(i as f32 / 25.0).unwrap();
            let above = fr.eye[1] - slope(fr.eye[0], fr.eye[2]);
            assert!((above - 1.6).abs() < 0.15, "frame {i}: {above}");
            // eyes level: the gaze stays horizontal
            assert!((fr.target[1] - fr.eye[1]).abs() < 1e-4);
            if let Some(p) = prev {
                steps.push(dist3(p, fr.eye));
            }
            prev = Some(fr.eye);
        }
        // equal arc length per frame, except where the path turns a corner
        let straight = &steps[3..steps.len() / 2 - 5];
        let first = straight[0];
        assert!(first > 1.2 / 25.0 * 0.99, "{first}");
        assert!(
            straight.iter().all(|s| (s - first).abs() < 1e-3),
            "{straight:?}"
        );
    }

    #[test]
    fn a_walk_looks_ahead_along_the_path() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![shot(json!({
            "kind": "walk", "path": [[0, 0], [0, -10]], "duration_s": 5, "ease": "none"
        }))]);
        let fr = camera_at(&f, 2.0, &env(ground, &[])).unwrap();
        assert!(fr.target[2] < fr.eye[2] - 3.0 && (fr.target[0] - fr.eye[0]).abs() < 1e-4);
        // the end of the path: still looking the way it goes
        let end = camera_at(&f, 5.0, &env(ground, &[])).unwrap();
        assert!(end.target[2] < end.eye[2] - 3.0);
    }

    #[test]
    fn an_orbit_keeps_its_radius_and_turns_counter_clockwise() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![shot(json!({
            "kind": "orbit", "center": [10.5, 0, 22.5], "radius_m": 9, "height_m": 4,
            "from_deg": 0, "to_deg": 720, "duration_s": 8, "ease": "none"
        }))]);
        let e = env(ground, &[]);
        for i in 0..=40 {
            let fr = camera_at(&f, i as f32 * 0.2, &e).unwrap();
            let r = (fr.eye[0] - 10.5).hypot(fr.eye[2] - 22.5);
            assert!((r - 9.0).abs() < 1e-3, "{r}");
            assert_eq!(fr.eye[1], 4.0);
            assert_eq!(fr.target, [10.5, 0.0, 22.5]);
        }
        // 0 degrees is east of the centre; a quarter turn counter-clockwise seen from above
        // (+X towards -Z) is north of it, which is -Z.
        let start = camera_at(&f, 0.0, &e).unwrap();
        assert!((start.eye[0] - 19.5).abs() < 1e-3 && (start.eye[2] - 22.5).abs() < 1e-3);
        let quarter = camera_at(&f, 8.0 / 8.0, &e).unwrap();
        assert!((quarter.eye[0] - 10.5).abs() < 1e-3 && (quarter.eye[2] - 13.5).abs() < 1e-3);
        // more than a turn is allowed: 720 degrees ends where it began
        let end = camera_at(&f, 8.0, &e).unwrap();
        assert!((end.eye[0] - 19.5).abs() < 1e-3);
    }

    #[test]
    fn eases_stand_still_at_their_ends() {
        let h = 1e-3;
        for ease in [Ease::In, Ease::Out, Ease::InOut, Ease::None] {
            assert_eq!(ease.apply(0.0), 0.0);
            assert!((ease.apply(1.0) - 1.0).abs() < 1e-6);
            let (v0, v1) = (ease.apply(h) / h, (1.0 - ease.apply(1.0 - h)) / h);
            match ease {
                Ease::In => assert!(v0 < 0.01 && v1 > 1.9),
                Ease::Out => assert!(v0 > 1.9 && v1 < 0.01),
                Ease::InOut => assert!(v0 < 0.01 && v1 < 0.01),
                Ease::None => assert!((v0 - 1.0).abs() < 1e-3 && (v1 - 1.0).abs() < 1e-3),
            }
        }
        // and the camera of an eased shot barely moves in its first frame
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![shot(json!({
            "kind": "glide", "path": [[0, 2, 0], [10, 2, 0]], "duration_s": 4
        }))]);
        let e = env(ground, &[]);
        let a = camera_at(&f, 0.0, &e).unwrap().eye;
        let b = camera_at(&f, 1.0 / 25.0, &e).unwrap().eye;
        let mid = camera_at(&f, 2.0, &e).unwrap().eye;
        assert!(b[0] - a[0] < 0.01 && (mid[0] - 5.0).abs() < 1e-3);
    }

    #[test]
    fn time_of_day_season_and_years_interpolate() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![
            shot(
                json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 2,
                "time_of_day": [17, 19], "years": [0, 10], "date": [{"month": 3, "day": 1}, {"month": 9, "day": 1}]}),
            ),
            shot(
                json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 2,
                "season": "winter", "snow": 0.5}),
            ),
        ]);
        let e = env(ground, &[]);
        let mid = camera_at(&f, 1.0, &e).unwrap();
        assert!((mid.overrides.time_of_day.unwrap() - 18.0).abs() < 1e-5);
        assert!((mid.overrides.years.unwrap() - 5.0).abs() < 1e-5);
        assert_eq!(mid.overrides.date, Some(Date { month: 6, day: 1 }));
        assert_eq!(mid.shot, 0);
        let end = camera_at(&f, 3.0, &e).unwrap();
        assert_eq!(end.shot, 1);
        assert_eq!(end.overrides.season, Some(Season::Winter));
        assert_eq!(end.overrides.snow, Some(0.5));
        assert_eq!(end.overrides.time_of_day, None);
        let params = mid.overrides.render_params();
        assert_eq!(params["time"], json!({"hour": 18.0, "month": 6, "day": 1}));
    }

    #[test]
    fn shafts_ease_along_a_shot() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![shot(
            json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 4,
            "shafts": [0.0, 1.0]}),
        )]);
        let e = env(ground, &[]);
        assert_eq!(camera_at(&f, 0.0, &e).unwrap().overrides.shafts, Some(0.0));
        let mid = camera_at(&f, 2.0, &e).unwrap().overrides;
        assert!((mid.shafts.unwrap() - 0.5).abs() < 1e-5);
        assert!((mid.render_params()["shafts"].as_f64().unwrap() - 0.5).abs() < 1e-5);
        let bad = Shot::from_value(
            &json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 1,
            "shafts": 1.5}),
            52.0,
        );
        assert!(bad.unwrap_err().contains("`shafts` must be from 0 to 1"));
    }

    #[test]
    fn lightning_eases_along_a_shot() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![shot(
            json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 4,
            "weather": ["storm", "storm"], "lightning": [0.2, 1.0]}),
        )]);
        let e = env(ground, &[]);
        let mid = camera_at(&f, 2.0, &e).unwrap().overrides;
        assert!((mid.lightning.unwrap() - 0.6).abs() < 1e-5);
        assert!((mid.render_params()["lightning"].as_f64().unwrap() - 0.6).abs() < 1e-5);
        let bad = Shot::from_value(
            &json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 1,
            "lightning": -0.1}),
            52.0,
        );
        assert!(bad.unwrap_err().contains("`lightning` must be from 0 to 1"));
    }

    #[test]
    fn mist_haze_and_gusts_ease_along_a_shot() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![shot(
            json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 4,
            "mist": [0.6, 0.0], "haze": [6, 2.5], "gustiness": 1}),
        )]);
        let e = env(ground, &[]);
        let start = camera_at(&f, 0.0, &e).unwrap().overrides;
        assert_eq!(
            (start.mist, start.haze, start.gustiness),
            (Some(0.6), Some(6.0), Some(1.0))
        );
        let mid = camera_at(&f, 2.0, &e).unwrap().overrides;
        assert!((mid.mist.unwrap() - 0.3).abs() < 1e-5);
        let params = mid.render_params();
        assert!((params["mist"].as_f64().unwrap() - 0.3).abs() < 1e-5);
        assert!((params["haze"].as_f64().unwrap() - 4.25).abs() < 1e-5);
        assert_eq!(params["gustiness"], json!(1.0f32));
        for (key, value, message) in [
            ("mist", json!(1.2), "`mist` must be from 0 to 1"),
            ("gustiness", json!(-0.1), "`gustiness` must be from 0 to 1"),
            ("haze", json!(80), "`haze` must be from 0 to 50"),
        ] {
            let mut v =
                json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 1});
            v[key] = value;
            let bad = Shot::from_value(&v, 52.0);
            assert!(bad.unwrap_err().contains(message), "{key}");
        }
    }

    #[test]
    fn wetness_and_rain_ease_from_dry_to_wet() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![shot(
            json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 4,
            "wetness": [0, 0.8], "rain": 0.6}),
        )]);
        let e = env(ground, &[]);
        let start = camera_at(&f, 0.0, &e).unwrap().overrides;
        assert_eq!((start.wetness, start.rain), (Some(0.0), Some(0.6)));
        let mid = camera_at(&f, 2.0, &e).unwrap().overrides;
        assert!((mid.wetness.unwrap() - 0.4).abs() < 1e-5);
        let params = mid.render_params();
        assert_eq!(params["rain"], json!(0.6f32));
        assert!((params["wetness"].as_f64().unwrap() - 0.4).abs() < 1e-5);
        // Out of range is refused, and a shot without them carries nothing.
        let bad = Shot::from_value(
            &json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 1, "rain": 1.5}),
            52.0,
        );
        assert!(bad.unwrap_err().contains("`rain` must be from 0 to 1"));
        let dry = camera_at(
            &film(vec![shot(
                json!({"kind": "hold", "eye": [0, 2, 5], "target": [0, 1, 0], "duration_s": 1}),
            )]),
            0.5,
            &e,
        )
        .unwrap();
        assert!(dry.overrides.render_params().is_empty());
    }

    #[test]
    fn durations_add_up_and_frames_are_counted() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let vps = [Viewpoint {
            id: "v1".into(),
            name: "Gate".into(),
            position: [0.0, 1.6, 5.0],
            target: [0.0, 1.6, 0.0],
            fov_y_deg: 70.0,
            time: Some(SunTime {
                hour: 8.5,
                ..SunTime::default()
            }),
            years: Some(3.0),
        }];
        let e = env(ground, &vps);
        let f = film(vec![
            shot(json!({"kind": "walk", "path": [[0, 0], [0, 12]], "speed_mps": 1.2})),
            shot(
                json!({"kind": "orbit", "center": [0, 0, 6], "radius_m": 5, "height_m": 3,
                "from_deg": 0, "to_deg": 90, "duration_s": 6}),
            ),
            shot(
                json!({"kind": "flyover", "path": [[0, 0], [40, 0]], "height_m": 20, "speed_mps": 10}),
            ),
            shot(json!({"kind": "hold", "viewpoint": "v1", "duration_s": 2})),
        ]);
        assert!((f.duration(&e).unwrap() - (10.0 + 6.0 + 4.0 + 2.0)).abs() < 1e-3);
        assert_eq!(f.frame_count(&e).unwrap(), 550);
        let last = camera_at(&f, 100.0, &e).unwrap();
        assert_eq!((last.shot, last.fov_y_deg), (3, 70.0));
        assert_eq!(last.overrides.time_of_day, Some(8.5));
        assert_eq!(last.overrides.years, Some(3.0));
        assert_eq!(last.eye, [0.0, 1.6, 5.0]);
        // a missing viewpoint is said, with the shot
        let gone = film(vec![shot(
            json!({"kind": "hold", "viewpoint": "v9", "duration_s": 1}),
        )]);
        assert!(f.duration(&e).is_ok());
        assert!(
            gone.duration(&e)
                .unwrap_err()
                .contains("shot 0: no viewpoint `v9`")
        );
    }

    #[test]
    fn a_flyover_looks_ahead_and_down() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let f = film(vec![shot(json!({
            "kind": "flyover", "path": [[0, 0], [30, 0]], "height_m": 15, "pitch_deg": 45, "duration_s": 6,
            "ease": "none"
        }))]);
        let fr = camera_at(&f, 3.0, &env(ground, &[])).unwrap();
        assert!((fr.eye[1] - 15.0).abs() < 1e-3);
        let (dx, dy) = (fr.target[0] - fr.eye[0], fr.target[1] - fr.eye[1]);
        assert!(dx > 5.0 && dy < -5.0 && (dx + dy).abs() < 1e-2, "{dx} {dy}");
    }

    #[test]
    fn a_rise_climbs_over_the_ground_and_a_glide_goes_through_its_points() {
        let hill = |_: f32, _: f32| 2.0;
        let ground: &dyn Fn(f32, f32) -> f32 = &hill;
        let e = env(ground, &[]);
        let f = film(vec![shot(json!({
            "kind": "rise", "at": [3, 3], "from_height": 1.6, "to_height": 11.6,
            "look_at": [10, 0, 10], "duration_s": 4, "ease": "none"
        }))]);
        assert!((camera_at(&f, 0.0, &e).unwrap().eye[1] - 3.6).abs() < 1e-4);
        assert!((camera_at(&f, 4.0, &e).unwrap().eye[1] - 13.6).abs() < 1e-4);
        let g = film(vec![shot(json!({
            "kind": "glide", "path": [[0, 2, 0], [5, 3, 5], [10, 2, 0]], "speed_mps": 2, "ease": "none"
        }))]);
        let tl = Timeline::new(&g, &e).unwrap();
        let mid = (0..=200)
            .map(|i| tl.camera_at(tl.duration_s * i as f32 / 200.0).unwrap().eye)
            .map(|p| dist3(p, [5.0, 3.0, 5.0]))
            .fold(f32::MAX, f32::min);
        assert!(mid < 0.1, "{mid}");
        assert!(tl.duration_s > 15.0 / 2.0 / 1.2 * 0.5);
    }

    #[test]
    fn bad_shots_are_refused_clearly() {
        let why = |v: Value| Shot::from_value(&v, 50.8).unwrap_err();
        assert!(why(json!({"kind": "walk", "path": [], "speed_mps": 1})).contains("at least 2"));
        assert!(
            why(json!({"kind": "walk", "path": [[0, 0], [1, 1]], "speed_mps": 0}))
                .contains("above 0")
        );
        assert!(
            why(json!({"kind": "walk", "path": [[0, 0], [1, 1]], "duration_s": -2}))
                .contains("duration_s")
        );
        assert!(
            why(json!({"kind": "walk", "path": [[0, 0], [1, 1]], "duration_s": 2, "speed_mps": 1}))
                .contains("not both")
        );
        assert!(
            why(json!({"kind": "walk", "path": [[0, 0], [1, 1]], "speeed_mps": 1}))
                .contains("did you mean `speed_mps`")
        );
        assert!(why(json!({"kind": "hold", "duration_s": 2})).contains("viewpoint"));
        assert!(why(json!({"kind": "swim"})).contains("unknown shot kind"));
        // Nothing near: the fields are listed, not the least far one (`lights` is not `mist`).
        let far = why(
            json!({"kind": "hold", "eye": [0, 1, 0], "target": [0, 1, 1], "duration_s": 1, "sunbeams": 1}),
        );
        assert!(
            far.contains("its fields are") && !far.contains("did you mean"),
            "{far}"
        );
        assert!(why(json!({"kind": "rise", "at": [0, 0], "from_height": 1, "to_height": 5, "duration_s": 1})).contains("look_at"));
        assert!(
            why(json!({"kind": "orbit", "center": [0, 0, 0], "radius_m": 0, "height_m": 3, "from_deg": 0, "to_deg": 90}))
                .contains("radius_m")
        );
        assert!(
            why(json!({"kind": "hold", "eye": [0, 1, 0], "target": [0, 1, 1], "duration_s": 1, "time_of_day": 25}))
                .contains("time_of_day")
        );
        assert!(
            why(json!({"kind": "hold", "eye": [0, 1, 0], "target": [0, 1, 1], "duration_s": 1, "season": "summer", "snow": 1}))
                .contains("snow lies only in winter")
        );
    }

    #[test]
    fn a_shot_round_trips_through_json() {
        let v = json!({"kind": "walk", "path": [[0.0, 0.0], [0.0, 5.0]], "eye_height_m": 1.5, "speed_mps": 1.25,
            "ease": "in_out", "look_at": "ahead", "time_of_day": [17.0, 18.0], "years": 5.0});
        let s = Shot::from_value(&v, 50.8).unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), v);
    }

    /// The eye's speed (m/s) just before and just after `t`, by differences.
    fn speeds_around(tl: &Timeline, t: f32) -> (f32, f32) {
        let h = 0.01;
        let at = |t: f32| tl.camera_at(t).unwrap().eye;
        (
            dist3(at(t - h), at(t - 2.0 * h)) / h,
            dist3(at(t + 2.0 * h), at(t + h)) / h,
        )
    }

    #[test]
    fn a_continued_shot_starts_where_the_last_ended_with_its_speed() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        // The first walk stops at ease none: 1.4 m/s all the way; the second is told to
        // continue and would ease in from rest on its own.
        let first =
            json!({"kind": "walk", "path": [[0, 0], [14, 0]], "speed_mps": 1.4, "ease": "none"});
        let second = json!({"kind": "walk", "path": [[14, 0.5], [14, 10]], "duration_s": 8, "ease": "in_out", "join": "continue"});
        let cut = json!({"kind": "walk", "path": [[14, 0.5], [14, 10]], "duration_s": 8, "ease": "in_out"});
        let e = env(ground, &[]);
        let joined = Timeline::new(&film(vec![shot(first.clone()), shot(second)]), &e).unwrap();
        let t = joined.spans()[1].0;
        // Position is continuous at the boundary, so is the speed (no dip), and so is the look.
        let (before, after) = (
            joined.camera_at(t - 1e-3).unwrap(),
            joined.camera_at(t + 1e-3).unwrap(),
        );
        assert!(dist3(before.eye, after.eye) < 0.01, "{before:?} {after:?}");
        let (v0, v1) = speeds_around(&joined, t);
        assert!(
            (v0 - 1.4).abs() < 0.05 && (v1 - 1.4).abs() < 0.15,
            "{v0} {v1}"
        );
        let look = |f: &Frame| normalize(sub3(f.target, f.eye));
        assert!(dist3(look(&before), look(&after)) < 0.02);
        // The same shot as a cut starts from rest: the speed dips to 0.
        let cutfilm = Timeline::new(&film(vec![shot(first), shot(cut)]), &e).unwrap();
        let (_, v1cut) = speeds_around(&cutfilm, cutfilm.spans()[1].0);
        assert!(v1cut < 0.2, "{v1cut}");
        // Along the joined shot the speed never falls near zero before its easing out.
        let mid = joined.camera_at(t + 1.0).unwrap().eye;
        let mid2 = joined.camera_at(t + 1.01).unwrap().eye;
        assert!(dist3(mid, mid2) / 0.01 > 0.8);
        // It ends at rest, as its ease says.
        let end = joined.duration_s;
        let (v_end, _) = speeds_around(&joined, end - 0.05);
        assert!(v_end < 0.5, "{v_end}");
    }

    #[test]
    fn glides_and_orbits_continue_too() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let e = env(ground, &[]);
        let f = film(vec![
            shot(
                json!({"kind": "glide", "path": [[0, 2, 0], [10, 3, 0], [20, 4, 5]], "speed_mps": 2, "ease": "none", "look_at": [10, 0, 10]}),
            ),
            shot(
                json!({"kind": "orbit", "center": [20, 0, 10], "radius_m": 6, "height_m": 3, "from_deg": 0, "to_deg": 120, "duration_s": 6, "join": "continue", "ease": "none"}),
            ),
        ]);
        let tl = Timeline::new(&f, &e).unwrap();
        let t = tl.spans()[1].0;
        let (a, b) = (
            tl.camera_at(t - 1e-3).unwrap(),
            tl.camera_at(t + 1e-3).unwrap(),
        );
        assert!(dist3(a.eye, b.eye) < 0.02, "{:?} {:?}", a.eye, b.eye);
        // Not the first shot, and a rise cannot continue.
        let err = Timeline::new(
            &film(vec![shot(
                json!({"kind": "walk", "path": [[0, 0], [5, 0]], "join": "continue"}),
            )]),
            &e,
        )
        .err()
        .unwrap();
        assert!(err.contains("needs a shot before it"), "{err}");
        assert!(
            Shot::from_value(
                &json!({"kind": "hold", "eye": [0, 1, 0], "target": [0, 1, 1], "join": "continue"}),
                50.8
            )
            .unwrap_err()
            .contains("walk, glide, flyover and orbit")
        );
    }

    #[test]
    fn a_walk_pauses_and_the_speed_is_solved_to_fit_the_duration() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let e = env(ground, &[]);
        let f = film(vec![shot(json!({
            "kind": "walk", "path": [[0, 0], [10, 0], [10, 10]], "duration_s": 20,
            "pause_at": [{"point": [10, 0], "s": 4}], "ease": "none"
        }))]);
        let tl = Timeline::new(&f, &e).unwrap();
        assert!((tl.duration_s - 20.0).abs() < 1e-3);
        let at = |t: f32| tl.camera_at(t).unwrap().eye;
        // 20 m in 16 s of walking: the first 10 m take 8 s, then 4 s standing at the corner.
        assert!(
            dist3(at(8.5), at(11.5)) < 0.7,
            "{:?} {:?}",
            at(8.5),
            at(11.5)
        );
        let corner = at(10.0);
        assert!(
            (corner[0] - 10.0).abs() < 0.3 && corner[2].abs() < 0.3,
            "{corner:?}"
        );
        assert!(dist3(at(14.0), at(10.0)) > 0.5);
        let end = at(20.0);
        assert!((end[0] - 10.0).abs() < 1e-3 && (end[2] - 10.0).abs() < 1e-3);
        // Too short a duration for the pauses is refused.
        let bad = film(vec![shot(
            json!({"kind": "walk", "path": [[0, 0], [10, 0]], "duration_s": 3, "pause_at": [{"point": [5, 0], "s": 4}]}),
        )]);
        assert!(Timeline::new(&bad, &e).err().unwrap().contains("pauses"));
    }

    #[test]
    fn a_walk_looks_away_and_back_without_a_cut() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let e = env(ground, &[]);
        let f = film(vec![shot(json!({
            "kind": "walk", "path": [[0, 0], [20, 0]], "speed_mps": 2, "ease": "none",
            "look": [{"at_s": 3, "target": [8, 1.6, 20], "hold_s": 2}]
        }))]);
        let tl = Timeline::new(&f, &e).unwrap();
        let dir = |t: f32| {
            let f = tl.camera_at(t).unwrap();
            normalize(sub3(f.target, f.eye))
        };
        assert!(dir(1.0)[0] > 0.99, "{:?}", dir(1.0));
        // Held on the target at 4 s: looking towards +Z (south) from x = 8.
        assert!(dir(4.0)[2] > 0.95, "{:?}", dir(4.0));
        // Back ahead after the turn.
        assert!(dir(7.0)[0] > 0.99, "{:?}", dir(7.0));
        // Turning, not jumping: neighbouring frames differ little.
        let mut t = 0.0;
        while t < 8.0 {
            assert!(dist3(dir(t), dir(t + 0.04)) < 0.12, "{t}");
            t += 0.04;
        }
        // The walk keeps its speed through the look.
        let (a, b) = (
            tl.camera_at(3.0).unwrap().eye,
            tl.camera_at(4.0).unwrap().eye,
        );
        assert!((dist3(a, b) - 2.0).abs() < 1e-3);
    }

    #[test]
    fn lights_white_balance_grade_and_exposure_ease_along_a_shot() {
        let ground: &dyn Fn(f32, f32) -> f32 = &flat;
        let e = env(ground, &[]);
        let f = film(vec![shot(json!({
            "kind": "hold", "eye": [0, 1.6, 0], "target": [0, 1.6, -5], "duration_s": 10,
            "lights": [0, 1], "white_balance": [6500, 4500], "grade": ["neutral", "warm"],
            "exposure_ev": [0, 1]
        }))]);
        let tl = Timeline::new(&f, &e).unwrap();
        let o = |t: f32| tl.camera_at(t).unwrap().overrides;
        assert_eq!(o(0.0).lights, Some(0.0));
        assert!((o(5.0).lights.unwrap() - 0.5).abs() < 1e-5);
        assert!((o(5.0).white_balance.unwrap() - 5500.0).abs() < 1.0);
        assert_eq!(o(0.0).exposure_ev, Some(0.0));
        assert!((o(10.0).exposure_ev.unwrap() - 1.0).abs() < 1e-5);
        assert_eq!(
            (o(2.5).grade, o(2.5).grade_to),
            (Some(Grade::Neutral), Some(Grade::Warm))
        );
        assert!((o(2.5).grade_mix.unwrap() - 0.25).abs() < 1e-5);
        let p = o(5.0).render_params();
        assert_eq!(p["lights"], json!(0.5));
        assert_eq!(p["grade_to"], json!("warm"));
        assert_eq!(p["exposure_offset_ev"], json!(0.5));
        // A single grade is constant; a shot without them leaves the project's own.
        let g = shot(
            json!({"kind": "hold", "eye": [0, 1, 0], "target": [0, 1, 1], "duration_s": 1, "grade": "cool"}),
        );
        let tl = Timeline::new(&film(vec![g]), &e).unwrap();
        let o = tl.camera_at(0.5).unwrap().overrides;
        assert_eq!(
            (o.grade, o.grade_to, o.lights),
            (Some(Grade::Cool), None, None)
        );
        let why = |v: Value| Shot::from_value(&v, 50.8).unwrap_err();
        let hold = |extra: (&str, Value)| {
            let mut v =
                json!({"kind": "hold", "eye": [0, 1, 0], "target": [0, 1, 1], "duration_s": 1});
            v[extra.0] = extra.1;
            v
        };
        assert!(why(hold(("lights", json!(2)))).contains("`lights`"));
        assert!(why(hold(("white_balance", json!(100)))).contains("kelvin"));
        assert!(why(hold(("exposure_ev", json!(20)))).contains("relative to the automatic"));
    }
}
