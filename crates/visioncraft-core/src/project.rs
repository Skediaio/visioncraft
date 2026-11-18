//! The document VisionCraft saves as `.visioncraft`: JSON.

use crate::light::Light;
use crate::material::{Assignment, Material, builtin_palette};
use crate::object::UserObject;
use crate::planting::Generator;
use crate::shape::Shape;
use crate::terrain::Terrain;
use crate::viewpoint::Viewpoint;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// The newest `format_version` this build reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// Extension data by namespace (e.g. `"com.example.paving"`), kept untouched when the owner is not
/// installed.
pub type Extensions = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelKind {
    #[serde(rename = "3dcraft")]
    Craft3d,
    #[serde(rename = "obj")]
    Obj,
}

/// Placement of a model: scale, then rotation about +Y, then translation (metres).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    pub translation: [f32; 3],
    /// Degrees, counter-clockwise seen from above (+Y).
    pub rotation_y_deg: f32,
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation_y_deg: 0.0,
            scale: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    pub id: String,
    pub name: String,
    pub kind: ModelKind,
    /// A path relative to the project file, or a URL.
    pub source: String,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "model_layer")]
    pub layer: String,
    /// Bodies left out of the view, by body name or feature id (`model.set_hidden_bodies`):
    /// a roof taken off for a furnished plan from above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_bodies: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

impl ModelRef {
    /// Whether `body` (a body of this model's file) is hidden: its name or feature id is listed.
    pub fn hides(&self, body: &crate::Body) -> bool {
        self.hidden_bodies
            .iter()
            .any(|h| h == &body.id || h.eq_ignore_ascii_case(&body.name))
    }
}

pub const LAYER_MODEL: &str = "model";
pub const LAYER_SHAPES: &str = "shapes";
pub const LAYER_PLANTING: &str = "planting";
pub const LAYER_LIGHTING: &str = "lighting";

fn model_layer() -> String {
    LAYER_MODEL.into()
}

/// A named group that can be hidden; models, shapes, generators and lights each sit on one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    pub name: String,
    pub visible: bool,
    /// Surroundings, not part of the design: neighbours' houses and the like. They cast and
    /// receive shadow, draw with one neutral massing look and stay out of `query.quantities`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub context: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn default_layers() -> Vec<Layer> {
    [
        (LAYER_MODEL, "Model"),
        (LAYER_SHAPES, "Shapes"),
        (LAYER_PLANTING, "Planting"),
        (LAYER_LIGHTING, "Lighting"),
    ]
    .map(|(id, name)| Layer {
        id: id.into(),
        name: name.into(),
        visible: true,
        context: false,
    })
    .into()
}

/// A plant species the user made, for example by editing a built-in one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomSpecies {
    /// `sp1`, `sp2`...: usable wherever a palette takes an asset id.
    pub id: String,
    pub name: String,
    /// The built-in species id it was derived from.
    #[serde(default)]
    pub base: Option<String>,
    /// The plant generator's parameters as JSON; the core does not read them.
    pub params: Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundPlane {
    pub height: f32,
    /// Linear RGB.
    pub color: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkyChoice {
    Physical,
    Hdri {
        id: String,
        rotation_deg: f32,
        intensity: f32,
    },
}

/// App-level view settings that travel with the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewState {
    /// `None`: automatic exposure.
    pub exposure_ev: Option<f32>,
    pub sky: SkyChoice,
    /// Haze as a turbidity (1 pure air, 2.5 a clear day, 20 misty; 0 off); the window turns it
    /// into the haze's extinction (`studio::haze_extinction`).
    pub haze: f32,
    /// Wind speed in m/s (0: still air; `wind::BEAUFORT` names the steps).
    pub wind_speed_mps: f32,
    /// 0..=1: how much the wind's strength swells and fades in a slow wave travelling downwind
    /// (gust patches are `gustiness`).
    pub wind_gustiness: f32,
    pub wind_direction_deg: f32,
    /// The wind of files from before it had units: relative, 1 a moderate breeze. Read, never
    /// written; `migrate_wind` turns it into `wind_speed_mps` on load.
    #[serde(skip_serializing)]
    wind_strength: Option<f32>,
    pub ground: Option<GroundPlane>,
    /// How far plants, shadows and haze reach, as a multiple of the default (0.25..=4). Not
    /// written when 1, so files from before it existed round-trip unchanged.
    #[serde(skip_serializing_if = "is_one")]
    pub view_distance: f32,
    /// The picture's look: exposure and contrast on top of the tone map. Not written when
    /// `natural`, the default.
    #[serde(skip_serializing_if = "Look::is_natural")]
    pub look: Look,
    /// The cloud layer over the analytic sky. Not written while the default.
    #[serde(skip_serializing_if = "Clouds::is_default")]
    pub clouds: Clouds,
    /// Years after planting to show the plants at (`growth`), 0..=60; `None` shows every plant
    /// at its full size, as projects did before growth existed (the default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub years: Option<f32>,
    /// Lamps and lit rooms on or off; `None` (the default): on while the sun is below the
    /// horizon at the time shown (`Project::lights_on`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lights: Option<bool>,
    /// Ground clutter: fallen leaves under deciduous trees, stones and twigs on soil and gravel,
    /// grass hanging over the lawn's edge. On by default (not written then); the window still
    /// leaves it out at LOW quality.
    #[serde(skip_serializing_if = "is_true")]
    pub clutter: bool,
    /// Snow over the ground, roofs, hedge tops and branches, 0 (none, the default; not
    /// written) to 1. The season stays the date's (`Project::season`).
    #[serde(skip_serializing_if = "is_zero")]
    pub snow: f32,
    /// Weathering of every standard material at once, 0 (none, the default; not written) to 1:
    /// dirt, moss, wear and rain streaks with the default mix, added to each material's own
    /// `weathering` (`Weathering::strengths`). Explicit: the view's `years` does not drive it.
    #[serde(skip_serializing_if = "is_zero")]
    pub weathering: f32,
    /// Wet surfaces, 0 (dry, the default; not written) to 1: porous materials darken and turn
    /// glossy, and above about 0.5 puddles form on flat, open, upward-facing paving and soil.
    /// Rain implies it: the wetness drawn is `max(wetness, rain)` (`ViewState::wet`).
    #[serde(skip_serializing_if = "is_zero")]
    pub wetness: f32,
    /// Rain falling, 0 (none, the default; not written) to 1: slanted streaks (by the wind),
    /// ripples on puddles and water, an overcast sky and a weaker sun.
    #[serde(skip_serializing_if = "is_zero")]
    pub rain: f32,
    /// Mist, 0 (none, the default; not written) to 1: a low layer of droplets over the ground
    /// under the haze; the visibility falls from 1.3 km at 0.3 to 330 m at 0.6 and 120 m at 1.
    /// Rain brings its own fog on top.
    #[serde(skip_serializing_if = "is_zero")]
    pub mist: f32,
    /// Gusts, 0 (none, the default; not written) to 1: patches of stronger wind travelling
    /// downwind at the wind speed, rolling over lawns as waves of bending grass and swaying one
    /// crown after another (the slow swell is `wind_gustiness`).
    #[serde(skip_serializing_if = "is_zero")]
    pub gustiness: f32,
    /// Light shafts, 0 (none, the default; not written) to 1: the sun's light in the haze, mist
    /// and rain fog is shadowed by crowns, hedges and houses, so beams and darker lanes run
    /// through the air towards a low sun. 1 is the physical amount; there must be a medium
    /// (mist, rain or thick haze) for them to show.
    #[serde(skip_serializing_if = "is_zero")]
    pub shafts: f32,
    /// The camera's white balance: fixed for daylight (the default, not written), automatic
    /// from the light of the sun and sky, or a colour temperature in kelvin.
    #[serde(skip_serializing_if = "WhiteBalance::is_daylight")]
    pub white_balance: WhiteBalance,
    /// A colour grade before the tone map. Not written while `neutral`, the default.
    #[serde(skip_serializing_if = "Grade::is_neutral")]
    pub grade: Grade,
    /// The lens' light fall-off towards the corners, 0 (none, the default; not written) to 1:
    /// 0.3 takes about half a stop off the corners, 1 two stops.
    #[serde(skip_serializing_if = "is_zero")]
    pub vignette: f32,
    /// Film grain after the tone map, 0 (none, the default; not written) to 1: noise in the
    /// luminance, most in the mid-tones.
    #[serde(skip_serializing_if = "is_zero")]
    pub grain: f32,
    /// How a photo is taken (`view.set_photo`): frames, aperture, focus, shutter. Not written
    /// while the default.
    #[serde(skip_serializing_if = "crate::photo::PhotoSettings::is_default")]
    pub photo: crate::photo::PhotoSettings,
    /// What lies round the design on the endless ground, beyond its plot. Not written while the
    /// default (fields).
    #[serde(skip_serializing_if = "Surroundings::is_default")]
    pub surroundings: Surroundings,
    /// The hours of sun over an area drawn on the ground (`view.set_sun_map`); not written
    /// while off, so files from before it existed round-trip unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sun_map: Option<crate::sun_map::SunMap>,
}

/// The land round a design, painted and shaped on the endless ground beyond the plot: never
/// inside it, never on a coast's sea side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surroundings {
    /// A plain plane in the ground's colour, as before surroundings existed.
    None,
    /// Farmland: fields of grass, cereal and ploughed soil with hedgerows, woods here and
    /// there, gently rolling, hills on the horizon.
    #[default]
    Fields,
    /// Mostly woodland with clearings.
    Woodland,
    /// Sand dunes with marram grass and scrub.
    Dunes,
}

impl Surroundings {
    fn is_default(&self) -> bool {
        *self == Surroundings::default()
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_true(v: &bool) -> bool {
    *v
}

/// The longest look ahead the years setting allows.
pub const YEARS_MAX: f32 = 60.0;

/// A procedural cloud layer: what kind, and how much of the sky it covers.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clouds {
    pub kind: CloudKind,
    /// 0..1 of the sky.
    pub cover: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudKind {
    Clear,
    /// Light fair-weather cumulus.
    #[default]
    Cumulus,
    /// A closed grey layer: soft shadows, little direct sun.
    Overcast,
}

impl Default for Clouds {
    fn default() -> Self {
        Clouds {
            kind: CloudKind::Cumulus,
            cover: 0.35,
        }
    }
}

impl Clouds {
    fn is_default(&self) -> bool {
        *self == Clouds::default()
    }
}

/// How the picture is developed, as a photographer would choose a film.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    /// Plain AgX at the sky's exposure: flat, nothing clips, for checking values.
    Neutral,
    /// Exposed for an average scene as a camera meters it, a little contrast and colour.
    #[default]
    Natural,
    /// More contrast and colour, as a holiday photograph.
    Vivid,
}

impl Look {
    fn is_natural(&self) -> bool {
        *self == Look::Natural
    }
}

/// The colour temperatures `white_balance` takes (kelvin).
pub const WHITE_BALANCE_KELVIN: std::ops::RangeInclusive<f32> = 2000.0..=12000.0;

/// The camera's white balance, as on a camera: what light it takes for white.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WhiteBalanceRepr", into = "WhiteBalanceRepr")]
pub enum WhiteBalance {
    /// Fixed for daylight (6500 K): a low sun reads orange, an overcast sky a little blue, as
    /// on film. The default, as pictures were before white balance existed.
    #[default]
    Daylight,
    /// From the light of the sun and the sky at the time shown, as a camera on automatic white
    /// balance: most of a warm or cool cast is taken out, some of the mood is kept.
    Auto,
    /// A colour temperature in kelvin (2000..=12000): light of that temperature reads white.
    Kelvin(f32),
}

impl WhiteBalance {
    fn is_daylight(&self) -> bool {
        *self == WhiteBalance::Daylight
    }
}

/// `white_balance` as written: "daylight", "auto" or a number of kelvin.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum WhiteBalanceRepr {
    Name(String),
    Kelvin(f32),
}

impl TryFrom<WhiteBalanceRepr> for WhiteBalance {
    type Error = String;

    fn try_from(r: WhiteBalanceRepr) -> Result<Self, String> {
        match r {
            WhiteBalanceRepr::Name(n) if n == "daylight" => Ok(WhiteBalance::Daylight),
            WhiteBalanceRepr::Name(n) if n == "auto" => Ok(WhiteBalance::Auto),
            WhiteBalanceRepr::Name(n) => Err(format!(
                "white balance `{n}`: give \"daylight\", \"auto\" or a temperature in kelvin"
            )),
            WhiteBalanceRepr::Kelvin(k) => Ok(WhiteBalance::Kelvin(k)),
        }
    }
}

impl From<WhiteBalance> for WhiteBalanceRepr {
    fn from(w: WhiteBalance) -> Self {
        match w {
            WhiteBalance::Daylight => WhiteBalanceRepr::Name("daylight".into()),
            WhiteBalance::Auto => WhiteBalanceRepr::Name("auto".into()),
            WhiteBalance::Kelvin(k) => WhiteBalanceRepr::Kelvin(k),
        }
    }
}

/// A colour grade applied before the tone map, as a photographer picks a preset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    /// No grade: the picture as before grades existed.
    #[default]
    Neutral,
    /// A little warmer and richer: late afternoon.
    Warm,
    /// A little cooler and calmer.
    Cool,
    /// Lower contrast and colour: a matte, misty print.
    Soft,
}

impl Grade {
    fn is_neutral(&self) -> bool {
        *self == Grade::Neutral
    }
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

/// The range `view_distance` accepts.
pub const VIEW_DISTANCE_RANGE: std::ops::RangeInclusive<f32> = 0.25..=4.0;

impl Default for ViewState {
    fn default() -> Self {
        Self {
            exposure_ev: None,
            sky: SkyChoice::Physical,
            haze: 2.5,
            wind_speed_mps: 2.0,
            wind_gustiness: 0.5,
            wind_direction_deg: 0.0,
            wind_strength: None,
            // The library lawn's mean colour, a touch lighter: the endless ground then reads as
            // more lawn into the distance instead of an olive field round a green garden.
            ground: Some(GroundPlane {
                height: 0.0,
                color: [0.065, 0.12, 0.035],
            }),
            view_distance: 1.0,
            look: Look::Natural,
            clouds: Clouds::default(),
            years: None,
            lights: None,
            clutter: true,
            snow: 0.0,
            weathering: 0.0,
            wetness: 0.0,
            rain: 0.0,
            mist: 0.0,
            gustiness: 0.0,
            shafts: 0.0,
            white_balance: WhiteBalance::Daylight,
            grade: Grade::Neutral,
            vignette: 0.0,
            grain: 0.0,
            photo: crate::photo::PhotoSettings::default(),
            surroundings: Surroundings::default(),
            sun_map: None,
        }
    }
}

impl ViewState {
    /// The wetness drawn: rain wets everything, whatever `wetness` says.
    pub fn wet(&self) -> f32 {
        self.wetness.max(self.rain)
    }

    /// Maps the old relative `wind_strength` (files and commands from before wind had units) to
    /// m/s, once: it then stays out of the project.
    pub fn migrate_wind(&mut self) {
        if let Some(strength) = self.wind_strength.take()
            && strength.is_finite()
            && strength >= 0.0
        {
            self.wind_speed_mps =
                (strength * crate::wind::RELATIVE_MPS).min(crate::wind::MAX_SPEED_MPS);
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let finite = |v: f32| v.is_finite();
        let ok = self.exposure_ev.is_none_or(finite)
            && self.haze.is_finite()
            && self.haze >= 0.0
            && (0.0..=crate::wind::MAX_SPEED_MPS).contains(&self.wind_speed_mps)
            && (0.0..=1.0).contains(&self.wind_gustiness)
            && finite(self.wind_direction_deg)
            && VIEW_DISTANCE_RANGE.contains(&self.view_distance)
            && (0.0..=1.0).contains(&self.clouds.cover)
            && self.years.is_none_or(|y| (0.0..=YEARS_MAX).contains(&y))
            && (0.0..=1.0).contains(&self.snow)
            && (0.0..=1.0).contains(&self.weathering)
            && (0.0..=1.0).contains(&self.wetness)
            && (0.0..=1.0).contains(&self.rain)
            && (0.0..=1.0).contains(&self.mist)
            && (0.0..=1.0).contains(&self.gustiness)
            && (0.0..=1.0).contains(&self.shafts)
            && (0.0..=1.0).contains(&self.vignette)
            && (0.0..=1.0).contains(&self.grain)
            && self.photo.validate().is_ok()
            && match self.white_balance {
                WhiteBalance::Kelvin(k) => WHITE_BALANCE_KELVIN.contains(&k),
                _ => true,
            }
            && self.ground.is_none_or(|g| {
                finite(g.height) && g.color.iter().all(|c| c.is_finite() && *c >= 0.0)
            })
            && match &self.sky {
                SkyChoice::Physical => true,
                SkyChoice::Hdri {
                    rotation_deg,
                    intensity,
                    ..
                } => finite(*rotation_deg) && finite(*intensity) && *intensity >= 0.0,
            };
        if ok {
            Ok(())
        } else {
            Err("a view setting is not a usable number".into())
        }
    }
}

/// How the scene is drawn, independent of its content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderStyle {
    /// Every surface white: a presentation mode.
    #[serde(default)]
    pub clay: bool,
}

/// Where the project stands on Earth, for the sun.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Site {
    /// Degrees, north positive.
    pub latitude: f64,
    /// Degrees, east positive.
    pub longitude: f64,
    /// Hours east of UTC that `SunTime` is expressed in.
    pub utc_offset_hours: f64,
    /// Where true north lies in the drawing: degrees clockwise from the drawing's -Z axis, seen
    /// from above (+X is 90). 0 is the default convention (-Z is north); 90 means true north
    /// points along +X, so a true bearing `b` is drawing bearing `b + north_deg`.
    #[serde(default)]
    pub north_deg: f64,
}

impl Site {
    /// A compass bearing (degrees clockwise from true north) as a bearing in the drawing
    /// (degrees clockwise from -Z, in `0..360`): the one place `north_deg` is applied.
    pub fn bearing_to_drawing_deg(&self, true_bearing_deg: f64) -> f64 {
        (true_bearing_deg + self.north_deg).rem_euclid(360.0)
    }

    /// The horizontal unit vector `[x, 0, z]` in drawing space of a compass bearing (degrees
    /// clockwise from true north).
    pub fn bearing_to_drawing(&self, true_bearing_deg: f64) -> [f32; 3] {
        let a = self.bearing_to_drawing_deg(true_bearing_deg).to_radians();
        [a.sin() as f32, 0.0, (-a.cos()) as f32]
    }
}

impl Default for Site {
    /// Brussels, on summer time (CEST, UTC+2): the default date (`SunTime::default`, 21 June)
    /// falls in it, so a new project's clock and the "summer time follows the date" rule agree
    /// from the start instead of only once the sun is dragged.
    fn default() -> Self {
        Self {
            latitude: 50.85,
            longitude: 4.35,
            utc_offset_hours: 2.0,
            north_deg: 0.0,
        }
    }
}

/// A moment in local time at the site.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SunTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// Local time of day in hours, `0.0..24.0`.
    pub hour: f32,
}

impl Default for SunTime {
    fn default() -> Self {
        Self {
            year: 2026,
            month: 6,
            day: 21,
            hour: 14.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub format_version: u32,
    #[serde(default)]
    pub models: Vec<ModelRef>,
    #[serde(default)]
    pub site: Site,
    #[serde(default)]
    pub time: SunTime,
    #[serde(default = "default_layers")]
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    #[serde(default = "builtin_palette")]
    pub materials: Vec<Material>,
    #[serde(default)]
    pub assignments: Vec<Assignment>,
    #[serde(default)]
    pub generators: Vec<Generator>,
    #[serde(default)]
    pub lights: Vec<Light>,
    #[serde(default)]
    pub viewpoints: Vec<Viewpoint>,
    #[serde(default)]
    pub style: RenderStyle,
    #[serde(default)]
    pub species: Vec<CustomSpecies>,
    /// The user's own models, placed as the asset `user:<id>`.
    #[serde(default)]
    pub objects: Vec<UserObject>,
    /// Plan images under the drawing to trace over; not written while there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub underlays: Vec<crate::underlay::Underlay>,
    /// Photos of facades on wall faces, straightened; not written while there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wall_photos: Vec<crate::wall_photo::WallPhoto>,
    /// Films: shots of camera moves rendered to frames; not written while there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub films: Vec<crate::film::Film>,
    #[serde(default)]
    pub view: ViewState,
    /// Behind an `Arc`: undo snapshots share it, and a command that edits it copies it once.
    #[serde(default)]
    pub terrain: Option<Arc<Terrain>>,
    /// The sea level (`water.set_sea`); not written while there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sea: Option<crate::water::Sea>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            models: Vec::new(),
            site: Site::default(),
            time: SunTime::default(),
            layers: default_layers(),
            shapes: Vec::new(),
            materials: builtin_palette(),
            assignments: Vec::new(),
            generators: Vec::new(),
            lights: Vec::new(),
            viewpoints: Vec::new(),
            style: RenderStyle::default(),
            species: Vec::new(),
            objects: Vec::new(),
            underlays: Vec::new(),
            wall_photos: Vec::new(),
            films: Vec::new(),
            view: ViewState::default(),
            terrain: None,
            sea: None,
            extensions: Extensions::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProjectError {
    Json(String),
    /// Saved by a newer VisionCraft; reading it could lose data.
    NewerVersion {
        found: u32,
        known: u32,
    },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "not a readable project: {e}"),
            Self::NewerVersion { found, known } => write!(
                f,
                "project format version {found} is newer than this build understands ({known})"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

impl Project {
    /// Whether things on this layer are drawn; an unknown layer counts as visible.
    pub fn layer_visible(&self, layer: &str) -> bool {
        self.layers.iter().all(|l| l.id != layer || l.visible)
    }

    /// Whether the layer holds surroundings (see `Layer::context`); an unknown layer does not.
    pub fn layer_is_context(&self, layer: &str) -> bool {
        self.layers.iter().any(|l| l.id == layer && l.context)
    }

    /// The layer id of any model, shape, generator or light, by its id.
    pub fn layer_of(&self, id: &str) -> Option<&str> {
        if let Some(m) = self.models.iter().find(|m| m.id == id) {
            return Some(&m.layer);
        }
        if let Some(s) = self.shapes.iter().find(|s| s.id() == id) {
            return Some(s.layer());
        }
        if let Some(g) = self.generators.iter().find(|g| g.id() == id) {
            return Some(g.layer());
        }
        self.lights
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.layer.as_str())
    }

    /// Whether the model, shape, generator or light sits on a context layer.
    pub fn owner_is_context(&self, id: &str) -> bool {
        self.layer_of(id).is_some_and(|l| self.layer_is_context(l))
    }

    /// The layer of any model, shape, generator or light, by its id.
    pub(crate) fn layer_of_mut(&mut self, id: &str) -> Option<&mut String> {
        if let Some(m) = self.models.iter_mut().find(|m| m.id == id) {
            return Some(&mut m.layer);
        }
        if let Some(s) = self.shapes.iter_mut().find(|s| s.id() == id) {
            return Some(s.layer_mut());
        }
        if let Some(g) = self.generators.iter_mut().find(|g| g.id() == id) {
            return Some(g.layer_mut());
        }
        if let Some(u) = self.underlays.iter_mut().find(|u| u.id == id) {
            return Some(&mut u.layer);
        }
        self.lights
            .iter_mut()
            .find(|l| l.id == id)
            .map(|l| &mut l.layer)
    }

    /// Whether any model, shape, generator or light sits on the layer.
    pub fn layer_in_use(&self, layer: &str) -> bool {
        self.models.iter().any(|m| m.layer == layer)
            || self.shapes.iter().any(|s| s.layer() == layer)
            || self.generators.iter().any(|g| g.layer() == layer)
            || self.lights.iter().any(|l| l.layer == layer)
            || self.underlays.iter().any(|u| u.layer == layer)
    }

    /// The image files the project refers to (plan underlays and wall photos), each once.
    pub fn image_files(&self) -> Vec<&str> {
        let mut files: Vec<&str> = self
            .underlays
            .iter()
            .map(|u| u.file.as_str())
            .chain(self.wall_photos.iter().map(|p| p.file.as_str()))
            .collect();
        let mut seen = std::collections::HashSet::new();
        files.retain(|f| seen.insert(*f));
        files
    }

    pub fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("a project is always serialisable")
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, ProjectError> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| ProjectError::Json(e.to_string()))?;
        // Checked before the shape: a newer file may not fit today's structs at all.
        let found = value
            .get("format_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| ProjectError::Json("missing format_version".into()))?;
        if found > u64::from(FORMAT_VERSION) {
            return Err(ProjectError::NewerVersion {
                found: u32::try_from(found).unwrap_or(u32::MAX),
                known: FORMAT_VERSION,
            });
        }
        let mut project: Project =
            serde_json::from_value(value).map_err(|e| ProjectError::Json(e.to_string()))?;
        project.view.migrate_wind();
        Ok(project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Project {
        let mut p = Project::default();
        p.models.push(ModelRef {
            id: "m1".into(),
            name: "House".into(),
            kind: ModelKind::Craft3d,
            source: "house.3dcraft".into(),
            transform: Transform {
                translation: [1.0, 0.0, -2.5],
                rotation_y_deg: 90.0,
                scale: 2.0,
            },
            layer: LAYER_MODEL.into(),
            hidden_bodies: Vec::new(),
            extensions: [("nl.example.model".to_string(), json!({"tag": [1, 2]}))].into(),
        });
        p
    }

    #[test]
    fn round_trip_is_equal() {
        let p = sample();
        assert_eq!(Project::from_json_bytes(&p.to_json_bytes()).unwrap(), p);
    }

    #[test]
    fn unknown_extensions_survive_load_and_save() {
        let ext = json!({"paving": {"pattern": "herringbone", "joints_mm": 3.5, "tiles": [[0, 1], [2, 3]], "x": null}});
        let doc = json!({
            "format_version": 1,
            "extensions": ext,
            "models": [{"id": "a", "name": "A", "kind": "obj", "source": "a.obj",
                        "extensions": {"vendor.thing": {"deep": {"n": 1}}}}],
        });
        let p = Project::from_json_bytes(&serde_json::to_vec(&doc).unwrap()).unwrap();
        let saved: Value = serde_json::from_slice(&p.to_json_bytes()).unwrap();
        assert_eq!(saved["extensions"], ext);
        assert_eq!(
            saved["models"][0]["extensions"],
            json!({"vendor.thing": {"deep": {"n": 1}}})
        );
    }

    #[test]
    fn newer_format_version_is_rejected() {
        let bytes = br#"{"format_version": 99, "something_new": true}"#;
        assert_eq!(
            Project::from_json_bytes(bytes),
            Err(ProjectError::NewerVersion {
                found: 99,
                known: FORMAT_VERSION
            })
        );
    }
}

impl Project {
    /// Whether lamps and lit rooms are on at `time`: the view's setting, else after sunset.
    pub fn lights_on(&self, time: &SunTime) -> bool {
        self.view
            .lights
            .unwrap_or_else(|| crate::sun::sun_position(&self.site, time).elevation_deg < 0.0)
    }
}

#[cfg(test)]
mod lights_tests {
    use super::{Project, ViewState};

    #[test]
    fn lights_follow_the_sun_unless_set() {
        let mut p = Project::default();
        let mut t = p.time;
        t.month = 6;
        t.day = 21;
        t.hour = 13.0;
        assert!(!p.lights_on(&t));
        t.hour = 23.5;
        assert!(p.lights_on(&t));
        p.view.lights = Some(false);
        assert!(!p.lights_on(&t));
        p.view.lights = Some(true);
        t.hour = 13.0;
        assert!(p.lights_on(&t));
    }

    #[test]
    fn clutter_is_on_unless_switched_off_and_only_off_is_written() {
        let v = ViewState::default();
        assert!(v.clutter);
        assert!(!serde_json::to_string(&v).unwrap().contains("clutter"));
        let off = ViewState {
            clutter: false,
            ..v
        };
        let json = serde_json::to_string(&off).unwrap();
        assert!(json.contains("\"clutter\":false"));
        assert!(!serde_json::from_str::<ViewState>(&json).unwrap().clutter);
    }
}
