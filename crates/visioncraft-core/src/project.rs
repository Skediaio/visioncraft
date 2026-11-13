//! The document VisionCraft saves as `.visioncraft`: JSON.

use crate::light::Light;
use crate::material::{Assignment, Material, builtin_palette};
use crate::planting::Generator;
use crate::shape::Shape;
use crate::viewpoint::Viewpoint;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
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
    pub haze: f32,
    pub wind_strength: f32,
    pub wind_direction_deg: f32,
    pub ground: Option<GroundPlane>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            exposure_ev: None,
            sky: SkyChoice::Physical,
            haze: 2.5,
            wind_strength: 0.3,
            wind_direction_deg: 0.0,
            ground: Some(GroundPlane {
                height: 0.0,
                color: [0.10, 0.17, 0.08],
            }),
        }
    }
}

impl ViewState {
    pub fn validate(&self) -> Result<(), String> {
        let finite = |v: f32| v.is_finite();
        let ok = self.exposure_ev.is_none_or(finite)
            && self.haze.is_finite()
            && self.haze >= 0.0
            && self.wind_strength.is_finite()
            && self.wind_strength >= 0.0
            && finite(self.wind_direction_deg)
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
}

impl Default for Site {
    /// Brussels, CET.
    fn default() -> Self {
        Self {
            latitude: 50.85,
            longitude: 4.35,
            utc_offset_hours: 1.0,
        }
    }
}

/// A moment in local time at the site.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
    #[serde(default)]
    pub view: ViewState,
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
            view: ViewState::default(),
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
        serde_json::from_value(value).map_err(|e| ProjectError::Json(e.to_string()))
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
