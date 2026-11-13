//! Materials, their assignment to faces, bodies and whole models or shapes, and the `.mcmat`
//! reader.

use crate::ImportedModel;
use crate::project::{Extensions, Project};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Optional texture sources (a path relative to the project, or a URL) per usage.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialMaps {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_color: Option<String>,
    /// OpenGL convention (+Y up).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roughness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metallic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient_occlusion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lawn3d {
    pub density_per_m2: f32,
    pub blade_height_m: f32,
    pub seed: u64,
}

/// How a surface is shaded; the renderer picks its shader from this.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SurfaceKind {
    StandardTriplanarPbr {
        /// Metres per texture repeat.
        map_scale_m: f32,
        relief: f32,
        gloss: f32,
        weathering: f32,
    },
    ArchitecturalGlass {
        /// Linear RGB.
        tint: [f32; 3],
        roughness: f32,
    },
    AnimatedWater {
        flow_dir_deg: f32,
        speed: f32,
        tint: [f32; 3],
        depth_m: f32,
    },
    Lawn3d(Lawn3d),
}

impl Default for SurfaceKind {
    fn default() -> Self {
        Self::StandardTriplanarPbr {
            map_scale_m: 1.0,
            relief: 0.0,
            gloss: 0.0,
            weathering: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub id: String,
    pub name: String,
    pub kind: SurfaceKind,
    /// Linear RGBA.
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    #[serde(default)]
    pub maps: MaterialMaps,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

impl Material {
    /// Rejects numbers a shader cannot use.
    pub fn validate(&self) -> Result<(), String> {
        let unit = |v: f32| (0.0..=1.0).contains(&v);
        if !(self.base_color.iter().all(|c| unit(*c))
            && unit(self.metallic)
            && unit(self.roughness))
        {
            return Err("colour, metallic and roughness must lie in 0..1".into());
        }
        let ok = match &self.kind {
            SurfaceKind::StandardTriplanarPbr {
                map_scale_m,
                relief,
                gloss,
                weathering,
            } => *map_scale_m > 0.0 && [relief, gloss, weathering].iter().all(|v| unit(**v)),
            SurfaceKind::ArchitecturalGlass { tint, roughness } => {
                tint.iter().all(|c| unit(*c)) && unit(*roughness)
            }
            SurfaceKind::AnimatedWater {
                flow_dir_deg,
                speed,
                tint,
                depth_m,
            } => {
                flow_dir_deg.is_finite()
                    && speed.is_finite()
                    && *speed >= 0.0
                    && tint.iter().all(|c| unit(*c))
                    && depth_m.is_finite()
                    && *depth_m >= 0.0
            }
            SurfaceKind::Lawn3d(l) => {
                l.density_per_m2.is_finite()
                    && l.density_per_m2 > 0.0
                    && l.blade_height_m.is_finite()
                    && l.blade_height_m > 0.0
            }
        };
        if ok {
            Ok(())
        } else {
            Err("a surface parameter is out of range".into())
        }
    }
}

/// What an assignment applies to. `owner` is a model or shape id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Target {
    /// One face by its persistent name.
    Face { owner: String, face: String },
    /// Every face of one body, by the body's id.
    Body { owner: String, body: String },
    /// Everything of the owner.
    Owner { owner: String },
}

impl Target {
    pub fn owner(&self) -> &str {
        match self {
            Self::Face { owner, .. } | Self::Body { owner, .. } | Self::Owner { owner } => owner,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub target: Target,
    /// A material id.
    pub material: String,
}

/// For each face of `model` (owned by `owner_id`) the index into `project.materials` of its
/// material: a face assignment wins over a body assignment, which wins over an owner one; `None`
/// when nothing applies.
pub fn resolve_materials(
    project: &Project,
    owner_id: &str,
    model: &ImportedModel,
) -> Vec<Option<usize>> {
    let index_of = |id: &str| project.materials.iter().position(|m| m.id == id);
    let (mut by_face, mut by_body, mut whole) = (BTreeMap::new(), BTreeMap::new(), None);
    // Later assignments of the same target win, as they replace earlier ones in the commands.
    for a in project
        .assignments
        .iter()
        .filter(|a| a.target.owner() == owner_id)
    {
        let Some(m) = index_of(&a.material) else {
            continue;
        };
        match &a.target {
            Target::Face { face, .. } => by_face.insert(face.as_str(), m),
            Target::Body { body, .. } => by_body.insert(body.as_str(), m),
            Target::Owner { .. } => whole.replace(m),
        };
    }
    model
        .faces
        .iter()
        .map(|f| {
            by_face
                .get(f.name.as_str())
                .or_else(|| by_body.get(model.bodies[f.body as usize].id.as_str()))
                .copied()
                .or(whole)
        })
        .collect()
}

/// Assignments of `owner_id` that name a face or body `model` does not have (after a re-import).
/// They stay in the project: the face may come back.
pub fn dangling_assignments<'a>(
    project: &'a Project,
    owner_id: &str,
    model: &ImportedModel,
) -> Vec<&'a Assignment> {
    project
        .assignments
        .iter()
        .filter(|a| a.target.owner() == owner_id)
        .filter(|a| match &a.target {
            Target::Face { face, .. } => model.faces.iter().all(|f| &f.name != face),
            Target::Body { body, .. } => model.bodies.iter().all(|b| &b.id != body),
            Target::Owner { .. } => false,
        })
        .collect()
}

fn plain(
    id: &str,
    name: &str,
    rgb: [f32; 3],
    metallic: f32,
    roughness: f32,
    kind: SurfaceKind,
) -> Material {
    Material {
        id: id.into(),
        name: name.into(),
        kind,
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        metallic,
        roughness,
        maps: MaterialMaps::default(),
        extensions: Extensions::new(),
    }
}

/// The twelve plain materials a new project starts with (our own values, linear colours).
pub fn builtin_palette() -> Vec<Material> {
    let pbr = |scale: f32| SurfaceKind::StandardTriplanarPbr {
        map_scale_m: scale,
        relief: 0.0,
        gloss: 0.0,
        weathering: 0.0,
    };
    vec![
        plain(
            "plaster",
            "White plaster",
            [0.80, 0.79, 0.76],
            0.0,
            0.85,
            pbr(2.0),
        ),
        plain(
            "brick",
            "Brick red",
            [0.32, 0.09, 0.06],
            0.0,
            0.80,
            pbr(1.0),
        ),
        plain(
            "concrete",
            "Concrete",
            [0.30, 0.30, 0.29],
            0.0,
            0.90,
            pbr(2.0),
        ),
        plain("timber", "Timber", [0.34, 0.20, 0.09], 0.0, 0.65, pbr(1.0)),
        plain(
            "roof",
            "Dark roof",
            [0.03, 0.03, 0.035],
            0.0,
            0.70,
            pbr(1.0),
        ),
        plain(
            "glass",
            "Glass",
            [0.60, 0.75, 0.80],
            0.0,
            0.05,
            SurfaceKind::ArchitecturalGlass {
                tint: [0.60, 0.75, 0.80],
                roughness: 0.05,
            },
        ),
        plain(
            "water",
            "Water",
            [0.02, 0.10, 0.12],
            0.0,
            0.05,
            SurfaceKind::AnimatedWater {
                flow_dir_deg: 0.0,
                speed: 0.2,
                tint: [0.02, 0.18, 0.20],
                depth_m: 0.8,
            },
        ),
        plain(
            "lawn",
            "Lawn",
            [0.05, 0.22, 0.04],
            0.0,
            0.95,
            SurfaceKind::Lawn3d(Lawn3d {
                density_per_m2: 2000.0,
                blade_height_m: 0.04,
                seed: 1,
            }),
        ),
        plain("gravel", "Gravel", [0.38, 0.35, 0.30], 0.0, 0.95, pbr(0.5)),
        plain("paving", "Paving", [0.35, 0.34, 0.33], 0.0, 0.80, pbr(0.6)),
        plain("metal", "Metal", [0.55, 0.56, 0.58], 1.0, 0.35, pbr(1.0)),
        plain("soil", "Soil", [0.06, 0.035, 0.02], 0.0, 1.0, pbr(1.0)),
    ]
}

// ---- .mcmat ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum MapUsage {
    #[serde(rename = "baseColor")]
    BaseColor,
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "roughness")]
    Roughness,
    #[serde(rename = "metallic")]
    Metallic,
    #[serde(rename = "height")]
    Height,
    #[serde(rename = "ambientOcclusion")]
    AmbientOcclusion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum ColorSpace {
    #[serde(rename = "srgb")]
    Srgb,
    #[serde(rename = "linear")]
    Linear,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McMatMap {
    pub usage: MapUsage,
    pub color_space: ColorSpace,
    /// The PNG exactly as stored in the archive; decoding is the renderer's job.
    pub png: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McMat {
    pub name: String,
    /// Pixels `[width, height]` of every map.
    pub size: [u32; 2],
    pub maps: Vec<McMatMap>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum McMatError {
    Zip(String),
    /// `material.json` is missing or not what MaterialCraft writes.
    Json(String),
    /// The manifest names a file the archive does not hold.
    MissingFile(String),
    /// A map larger than [`MAX_MAP_BYTES`].
    TooLarge(String),
}

impl fmt::Display for McMatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zip(e) => write!(f, "not a readable .mcmat archive: {e}"),
            Self::Json(e) => write!(f, "bad material.json: {e}"),
            Self::MissingFile(n) => write!(f, "the archive has no `{n}`"),
            Self::TooLarge(n) => write!(f, "`{n}` is too large"),
        }
    }
}

impl std::error::Error for McMatError {}

/// Refuses a map above this many bytes (a decompression bomb guard).
pub const MAX_MAP_BYTES: usize = 64 << 20;

/// Reads a MaterialCraft `.mcmat`: a ZIP (stored or deflated entries) with `material.json` and
/// `maps/<usage>.png`. Maps with a usage this build does not know are skipped. `source.sbs` and
/// other entries are ignored.
pub fn read_mcmat(bytes: &[u8]) -> Result<McMat, McMatError> {
    #[derive(Deserialize)]
    struct Manifest {
        format: String,
        name: String,
        size: [u32; 2],
        maps: Vec<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Entry {
        usage: MapUsage,
        file: String,
        #[serde(rename = "colorSpace")]
        color_space: ColorSpace,
    }
    let zip_err = |e: craft3d_zipstore::ZipError| McMatError::Zip(e.to_string());
    let entries = craft3d_zipstore::read(bytes).map_err(zip_err)?;
    let get = |name: &str, limit: usize| {
        let e = entries
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| McMatError::MissingFile(name.to_string()))?;
        if e.size > limit {
            return Err(McMatError::TooLarge(name.to_string()));
        }
        e.data().map(|d| d.into_owned()).map_err(zip_err)
    };
    let manifest: Manifest = serde_json::from_slice(&get("material.json", 1 << 20)?)
        .map_err(|e| McMatError::Json(e.to_string()))?;
    if manifest.format != "craft-material" {
        return Err(McMatError::Json(format!("format `{}`", manifest.format)));
    }
    let mut maps = Vec::new();
    for raw in manifest.maps {
        // An unknown usage is a map for a newer build, not a broken file.
        let Ok(e) = serde_json::from_value::<Entry>(raw) else {
            continue;
        };
        maps.push(McMatMap {
            usage: e.usage,
            color_space: e.color_space,
            png: get(&e.file, MAX_MAP_BYTES)?,
        });
    }
    Ok(McMat {
        name: manifest.name,
        size: manifest.size,
        maps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_is_valid_and_unique() {
        let p = builtin_palette();
        assert_eq!(p.len(), 12);
        assert!(p.iter().all(|m| m.validate().is_ok()));
        let ids: std::collections::BTreeSet<_> = p.iter().map(|m| &m.id).collect();
        assert_eq!(ids.len(), 12);
    }

    #[test]
    fn mcmat_sample_has_six_maps_with_their_colour_spaces() {
        let m = read_mcmat(include_bytes!("../tests/data/test.mcmat")).unwrap();
        assert_eq!((m.name.as_str(), m.size), ("Test", [128, 128]));
        let got: Vec<_> = m.maps.iter().map(|x| (x.usage, x.color_space)).collect();
        use ColorSpace::{Linear, Srgb};
        use MapUsage::*;
        assert_eq!(
            got,
            [
                (BaseColor, Srgb),
                (Normal, Linear),
                (Roughness, Linear),
                (Metallic, Linear),
                (Height, Linear),
                (AmbientOcclusion, Linear)
            ]
        );
        assert!(m.maps.iter().all(|x| x.png.starts_with(b"\x89PNG")));
    }

    #[test]
    fn mcmat_rejects_garbage() {
        assert!(matches!(read_mcmat(b"nope"), Err(McMatError::Zip(_))));
    }
}
