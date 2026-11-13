//! The document VisionCraft saves as `.visioncraft`: JSON.

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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
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
