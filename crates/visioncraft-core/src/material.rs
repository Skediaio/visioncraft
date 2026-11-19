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
    /// A packed map: occlusion in red, roughness in green, metallic in blue (glTF channel order).
    /// When set it stands in for the three separate maps below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orm: Option<String>,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lawn3d {
    pub density_per_m2: f32,
    pub blade_height_m: f32,
    pub seed: u64,
    /// Mowing stripes this wide (metres, 0.3 to 5; 0: none, not written), running at
    /// `stripes_deg` clockwise from north.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stripes_m: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stripes_deg: f32,
    /// Small wild flowers among the blades, 0 (none, not written) to 1 (a flowering meadow):
    /// drawn with the GPU grass in their season (spring to late summer).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flowers: f32,
    /// The blades' own colour (linear RGB) when it is not the ground's: grey-green dune grass
    /// over sand. Absent (not written): the blades take the colour of the ground they grow in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blade_color: Option<[f32; 3]>,
}

/// The tallest grass a lawn or meadow material may ask for (m).
pub const MAX_BLADE_HEIGHT_M: f32 = 1.2;

fn is_zero(v: &f32) -> bool {
    *v == 0.0
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
        /// Depth of the height map's relief in metres (parallax occlusion on up-facing ground,
        /// 0..0.1; 0: off, not written). Only a material with a `height` map shows it.
        #[serde(default, skip_serializing_if = "is_zero")]
        parallax_depth_m: f32,
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
            parallax_depth_m: 0.0,
        }
    }
}

/// How far a material has aged: dirt, moss, wear and rain streaks, each 0..1, all scaled by
/// `amount`. Only `amount` is needed; the others default to a plausible mix. Procedural and
/// drawn in the renderer (it changes albedo and roughness only); not written while a material
/// has none, so files from before it existed round-trip unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weathering {
    /// 0..1: how much of all of it shows.
    pub amount: f32,
    /// Darkening and greying near the ground, in crevices and under overhangs.
    #[serde(default = "Weathering::default_dirt")]
    pub dirt: f32,
    /// Green-brown patches on up-facing surfaces and on the shaded side, in low and damp spots.
    #[serde(default = "Weathering::default_moss")]
    pub moss: f32,
    /// Lighter, smoother patches on up-facing paving where feet go, and worn edges.
    #[serde(default = "Weathering::default_wear")]
    pub wear: f32,
    /// Vertical rain streaks running down walls from their top edges and sills.
    #[serde(default = "Weathering::default_streaks")]
    pub streaks: f32,
}

impl Weathering {
    pub const DEFAULT_DIRT: f32 = 0.6;
    pub const DEFAULT_MOSS: f32 = 0.4;
    pub const DEFAULT_WEAR: f32 = 0.4;
    pub const DEFAULT_STREAKS: f32 = 0.5;
    fn default_dirt() -> f32 {
        Self::DEFAULT_DIRT
    }
    fn default_moss() -> f32 {
        Self::DEFAULT_MOSS
    }
    fn default_wear() -> f32 {
        Self::DEFAULT_WEAR
    }
    fn default_streaks() -> f32 {
        Self::DEFAULT_STREAKS
    }

    /// `amount` with the default mix of the rest.
    pub fn new(amount: f32) -> Self {
        Self {
            amount,
            dirt: Self::DEFAULT_DIRT,
            moss: Self::DEFAULT_MOSS,
            wear: Self::DEFAULT_WEAR,
            streaks: Self::DEFAULT_STREAKS,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let unit = |v: f32| (0.0..=1.0).contains(&v);
        if [self.amount, self.dirt, self.moss, self.wear, self.streaks]
            .into_iter()
            .all(unit)
        {
            Ok(())
        } else {
            Err("weathering amount, dirt, moss, wear and streaks must lie in 0..1".into())
        }
    }

    /// How strongly dirt, moss, wear and streaks show on a surface, each 0..1, from the
    /// material's own weathering (`None`: the default mix at amount 0) and the view's global
    /// `view.weathering`. The two add up as independent layers of age: `1 - (1 - own)(1 - global)`.
    /// All zero when neither is set, which the renderer skips entirely.
    pub fn strengths(own: Option<Self>, global: f32) -> [f32; 4] {
        let w = own.unwrap_or_else(|| Self::new(0.0));
        let amount = 1.0 - (1.0 - w.amount.clamp(0.0, 1.0)) * (1.0 - global.clamp(0.0, 1.0));
        [w.dirt, w.moss, w.wear, w.streaks].map(|k| k.clamp(0.0, 1.0) * amount)
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
    /// 0..1: how much the colour varies across the surface at large scale (breaks up repetition).
    #[serde(default)]
    pub variation: f32,
    /// Dirt, moss, wear and rain streaks on top of the surface (`Weathering`); `None` (the
    /// default, not written): only the view's global weathering ages it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weathering: Option<Weathering>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

impl Material {
    /// Rejects numbers a shader cannot use.
    pub fn validate(&self) -> Result<(), String> {
        let unit = |v: f32| (0.0..=1.0).contains(&v);
        if !(self.base_color.iter().all(|c| unit(*c))
            && unit(self.metallic)
            && unit(self.roughness)
            && unit(self.variation))
        {
            return Err("colour, metallic, roughness and variation must lie in 0..1".into());
        }
        if let Some(w) = &self.weathering {
            w.validate()?;
        }
        let ok = match &self.kind {
            SurfaceKind::StandardTriplanarPbr {
                map_scale_m,
                relief,
                gloss,
                weathering,
                parallax_depth_m,
            } => {
                *map_scale_m > 0.0
                    && [relief, gloss, weathering].iter().all(|v| unit(**v))
                    && (0.0..=0.1).contains(parallax_depth_m)
            }
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
                    && l.blade_height_m <= MAX_BLADE_HEIGHT_M
                    && (l.stripes_m == 0.0 || (0.3..=5.0).contains(&l.stripes_m))
                    && l.stripes_deg.is_finite()
                    && unit(l.flowers)
                    && l.blade_color.is_none_or(|c| c.iter().all(|v| unit(*v)))
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
        variation: 0.0,
        weathering: None,
        extensions: Extensions::new(),
    }
}

/// The plain materials a new project starts with (our own values, linear colours).
pub fn builtin_palette() -> Vec<Material> {
    let pbr = |scale: f32| SurfaceKind::StandardTriplanarPbr {
        map_scale_m: scale,
        relief: 0.0,
        gloss: 0.0,
        weathering: 0.0,
        parallax_depth_m: 0.0,
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
            "timber-dark",
            "Dark timber",
            [0.14, 0.09, 0.05],
            0.0,
            0.7,
            pbr(1.0),
        ),
        // Weathering steel: a rusty orange-brown, rough, a little different from plate to plate.
        Material {
            variation: 0.15,
            ..plain(
                "corten",
                "Corten steel",
                [0.17, 0.065, 0.025],
                0.2,
                0.85,
                SurfaceKind::StandardTriplanarPbr {
                    map_scale_m: 1.0,
                    relief: 0.0,
                    gloss: 0.0,
                    weathering: 0.4,
                    parallax_depth_m: 0.0,
                },
            )
        },
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
                ..Default::default()
            }),
        ),
        // Uncut grass: mixed grasses about knee-high with seed heads and wild flowers, as a
        // flowering hay meadow. Fewer, taller blades than the lawn; its ground is a duller green.
        plain(
            "meadow",
            "Meadow",
            [0.075, 0.14, 0.045],
            0.0,
            0.95,
            SurfaceKind::Lawn3d(Lawn3d {
                density_per_m2: 1400.0,
                blade_height_m: 0.45,
                seed: 1,
                flowers: 0.35,
                ..Default::default()
            }),
        ),
        plain("gravel", "Gravel", [0.38, 0.35, 0.30], 0.0, 0.95, pbr(0.5)),
        plain("paving", "Paving", [0.35, 0.34, 0.33], 0.0, 0.80, pbr(0.6)),
        plain("metal", "Metal", [0.55, 0.56, 0.58], 1.0, 0.35, pbr(1.0)),
        plain("soil", "Soil", [0.06, 0.035, 0.02], 0.0, 1.0, pbr(1.0)),
        // Humus and moss under woodland (`planting.add_floor`): the library's CC0 Ground 045
        // (ambientCG, Lennart Demes), 3 m to a tile. Variation 0.6 as the lawn's: the macro
        // noise breaks the 3 m repeat (after the two tiling scales of J. Moore, "Terrain
        // Rendering in Far Cry 5", GDC 2018). 3 cm of parallax relief is a guess for litter over
        // humus; its normal map stays off (at full strength the dark floor in canopy shade read
        // as rippled wet tar).
        {
            let mut m = plain(
                "forest-floor",
                "Forest floor",
                [1.0, 1.0, 1.0],
                0.0,
                1.0,
                SurfaceKind::StandardTriplanarPbr {
                    map_scale_m: 3.0,
                    relief: 0.0,
                    gloss: 0.0,
                    weathering: 0.0,
                    parallax_depth_m: 0.03,
                },
            );
            m.maps = MaterialMaps {
                base_color: Some("materials/ground045/basecolor.jpg".into()),
                normal: Some("materials/ground045/normal.jpg".into()),
                orm: Some("materials/ground045/orm.jpg".into()),
                height: Some("materials/ground045/height.jpg".into()),
                ..Default::default()
            };
            m.variation = 0.6;
            m
        },
        // Dry beach sand: pale, warm; darkened where wet by the sea's shore band.
        plain("sand", "Sand", [0.50, 0.42, 0.30], 0.0, 0.95, pbr(1.0)),
        // Furniture: upholstery, oiled teak, black powder coat and canvas.
        plain(
            "fabric-sand",
            "Sand fabric",
            [0.62, 0.55, 0.44],
            0.0,
            0.95,
            pbr(0.3),
        ),
        plain(
            "fabric-charcoal",
            "Charcoal fabric",
            [0.12, 0.12, 0.13],
            0.0,
            0.95,
            pbr(0.3),
        ),
        plain("teak", "Teak", [0.42, 0.26, 0.12], 0.0, 0.6, pbr(1.0)),
        plain(
            "powder-coat-black",
            "Black powder coat",
            [0.03, 0.03, 0.035],
            0.3,
            0.5,
            pbr(1.0),
        ),
        plain(
            "canvas-white",
            "White canvas",
            [0.85, 0.84, 0.80],
            0.0,
            0.9,
            pbr(0.3),
        ),
        // The room behind a window: dark, so glass reads as glass, lit by lamps inside.
        plain(
            "interior",
            "Interior",
            [0.05, 0.045, 0.04],
            0.0,
            0.9,
            pbr(1.0),
        ),
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
    fn mowing_stripes_are_checked_and_not_written_when_off() {
        let mut lawn = builtin_palette()
            .into_iter()
            .find(|m| m.id == "lawn")
            .unwrap();
        assert!(!serde_json::to_string(&lawn).unwrap().contains("stripes"));
        let SurfaceKind::Lawn3d(l) = &mut lawn.kind else {
            panic!()
        };
        l.stripes_m = 0.9;
        l.stripes_deg = 30.0;
        assert!(lawn.validate().is_ok());
        let back: Material = serde_json::from_str(&serde_json::to_string(&lawn).unwrap()).unwrap();
        assert_eq!(back, lawn);
        let SurfaceKind::Lawn3d(l) = &mut lawn.kind else {
            panic!()
        };
        l.stripes_m = 12.0;
        assert!(lawn.validate().is_err());
    }

    #[test]
    fn parallax_depth_round_trips_and_is_checked() {
        let mut m = builtin_palette()
            .into_iter()
            .find(|m| m.id == "gravel")
            .unwrap();
        assert!(!serde_json::to_string(&m).unwrap().contains("parallax"));
        let SurfaceKind::StandardTriplanarPbr {
            parallax_depth_m, ..
        } = &mut m.kind
        else {
            panic!()
        };
        *parallax_depth_m = 0.03;
        assert!(m.validate().is_ok());
        let back: Material = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        for bad in [-0.01, f32::NAN, 0.5] {
            let SurfaceKind::StandardTriplanarPbr {
                parallax_depth_m, ..
            } = &mut m.kind
            else {
                panic!()
            };
            *parallax_depth_m = bad;
            assert!(m.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn palette_is_valid_and_unique() {
        let p = builtin_palette();
        assert_eq!(p.len(), 23);
        assert!(p.iter().all(|m| m.validate().is_ok()));
        let ids: std::collections::BTreeSet<_> = p.iter().map(|m| &m.id).collect();
        assert_eq!(ids.len(), 23);
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
