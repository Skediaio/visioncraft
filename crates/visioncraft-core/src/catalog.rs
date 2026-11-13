//! The asset library's schema. Read from JSON; not part of the project.

use crate::light::LightSpec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Tree,
    Shrub,
    Perennial,
    Grass,
    Flower,
    Hedge,
    Furniture,
    Pot,
    Lighting,
    Fence,
    Play,
    Vehicle,
    Person,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exposure {
    Sun,
    HalfShade,
    Shade,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    #[default]
    BuiltIn,
    User,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lod {
    pub file: String,
    /// Used up to this distance from the camera.
    pub max_distance_m: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlantInfo {
    pub mature_height_m: f32,
    pub mature_width_m: f32,
    pub evergreen: bool,
    /// 1 = January.
    #[serde(default)]
    pub bloom_months: Vec<u8>,
    pub exposure: Exposure,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub category: Category,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Free text. The built-in library must use `CC0-1.0` or `CC-BY-4.0`
    /// ([`validate_open_licences`]); a user's own assets may say anything, e.g. `private`.
    pub license: String,
    #[serde(default)]
    pub origin: Origin,
    pub author: String,
    pub source_url: String,
    /// Width, height, depth in metres.
    pub size_m: [f32; 3],
    #[serde(default)]
    pub lods: Vec<Lod>,
    #[serde(default)]
    pub thumbnail: String,
    #[serde(default)]
    pub plant: Option<PlantInfo>,
    #[serde(default)]
    pub light: Option<LightSpec>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub assets: Vec<Asset>,
}

pub const LICENSES: [&str; 2] = ["CC0-1.0", "CC-BY-4.0"];

#[derive(Clone, Debug, PartialEq)]
pub enum CatalogError {
    Json(String),
    /// The licence is empty (a missing field is a [`CatalogError::Json`]).
    MissingLicense(String),
    DuplicateId(String),
    NoLod(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "not a readable catalogue: {e}"),
            Self::MissingLicense(asset) => write!(f, "asset `{asset}` has no licence"),
            Self::DuplicateId(id) => write!(f, "asset id `{id}` appears twice"),
            Self::NoLod(id) => write!(f, "asset `{id}` has no LOD"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl Catalog {
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, CatalogError> {
        let catalog: Self =
            serde_json::from_slice(bytes).map_err(|e| CatalogError::Json(e.to_string()))?;
        let mut seen = BTreeSet::new();
        for a in &catalog.assets {
            if a.license.trim().is_empty() {
                return Err(CatalogError::MissingLicense(a.id.clone()));
            }
            if !seen.insert(&a.id) {
                return Err(CatalogError::DuplicateId(a.id.clone()));
            }
            if a.lods.is_empty() {
                return Err(CatalogError::NoLod(a.id.clone()));
            }
        }
        Ok(catalog)
    }

    pub fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("a catalogue is always serialisable")
    }

    pub fn asset(&self, id: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Adds the assets of `other` whose id is new; returns the ids skipped because they exist
    /// (the entry already here wins, so a user cannot shadow a built-in asset).
    pub fn merge(&mut self, other: Catalog) -> Vec<String> {
        let mut skipped = Vec::new();
        for a in other.assets {
            if self.asset(&a.id).is_some() {
                skipped.push(a.id);
            } else {
                self.assets.push(a);
            }
        }
        skipped
    }

    /// Assets whose name or a tag contains `query` (case-insensitive; empty matches all), that
    /// also pass the filters given: category, exposure of a plant (assets without plant data
    /// fail an exposure or height filter), mature height at most `max_height_m`. By name.
    pub fn search(
        &self,
        query: &str,
        category: Option<Category>,
        exposure: Option<Exposure>,
        max_height_m: Option<f32>,
    ) -> Vec<&Asset> {
        let q = query.to_lowercase();
        let mut found: Vec<&Asset> = self
            .assets
            .iter()
            .filter(|a| {
                a.name.to_lowercase().contains(&q)
                    || a.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .filter(|a| category.is_none_or(|c| a.category == c))
            .filter(|a| exposure.is_none_or(|e| a.plant.as_ref().is_some_and(|p| p.exposure == e)))
            .filter(|a| {
                max_height_m
                    .is_none_or(|h| a.plant.as_ref().is_some_and(|p| p.mature_height_m <= h))
            })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        found
    }
}

/// Every asset whose licence is not exactly `CC0-1.0` or `CC-BY-4.0`, one line each. CI runs
/// this on the built-in library only.
pub fn validate_open_licences(catalog: &Catalog) -> Result<(), Vec<String>> {
    let bad: Vec<String> = catalog
        .assets
        .iter()
        .filter(|a| !LICENSES.contains(&a.license.as_str()))
        .map(|a| format!("asset `{}` has licence `{}`", a.id, a.license))
        .collect();
    if bad.is_empty() { Ok(()) } else { Err(bad) }
}

/// A catalogue entry for a model the user imports: id is the slug of the file name (the caller
/// makes it unique), size from the bounds, licence `private`, origin User, one LOD pointing at
/// the file, category `other`.
pub fn suggest_asset(
    file_name: &str,
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
    _triangle_count: u32,
) -> Asset {
    let stem = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
    let slug: String = stem
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    Asset {
        id: if slug.is_empty() {
            "asset".into()
        } else {
            slug
        },
        name: stem.to_string(),
        category: Category::Other,
        tags: Vec::new(),
        license: "private".into(),
        origin: Origin::User,
        author: String::new(),
        source_url: String::new(),
        size_m: std::array::from_fn(|k| bounds_max[k] - bounds_min[k]),
        lods: vec![Lod {
            file: file_name.to_string(),
            max_distance_m: f32::MAX,
        }],
        thumbnail: String::new(),
        plant: None,
        light: None,
    }
}

/// The translation that puts a model's pivot at the centre of its footprint on the ground (its
/// lowest point).
pub fn normalising_transform(bounds_min: [f32; 3], bounds_max: [f32; 3]) -> [f32; 3] {
    [
        -(bounds_min[0] + bounds_max[0]) / 2.0,
        -bounds_min[1],
        -(bounds_min[2] + bounds_max[2]) / 2.0,
    ]
}

/// What to tell the user about an imported model: too many triangles, or a size that suggests
/// the wrong unit (above 60 m or below 1 cm in any direction).
pub fn import_warnings(
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
    triangle_count: u32,
) -> Vec<String> {
    let mut out = Vec::new();
    if triangle_count > 50_000 {
        out.push(format!(
            "{triangle_count} triangles: heavy for a plant or a prop (above 50,000)"
        ));
    }
    for (axis, k) in ["width", "height", "depth"].into_iter().zip(0..3) {
        let size = bounds_max[k] - bounds_min[k];
        if size > 60.0 || size < 0.01 {
            out.push(format!("{axis} is {size} m: probably the wrong unit"));
        }
    }
    out
}

/// The body of `ATTRIBUTION.md`: every asset with author and source, grouped by licence.
pub fn attribution_markdown(catalog: &Catalog) -> String {
    let mut out = String::from("# Attribution\n");
    // Only the built-in library is credited; a user's own assets are theirs.
    for license in LICENSES {
        let mut assets: Vec<_> = catalog
            .assets
            .iter()
            .filter(|a| a.origin == Origin::BuiltIn && a.license == license)
            .collect();
        if assets.is_empty() {
            continue;
        }
        assets.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        out += &format!("\n## {license}\n\n");
        for a in assets {
            out += &format!("- {} by {} ({})\n", a.name, a.author, a.source_url);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn asset(id: &str, license: &str) -> Value {
        json!({"id": id, "name": format!("Name {id}"), "category": "tree", "license": license,
               "author": format!("Author {id}"), "source_url": format!("https://example.org/{id}"),
               "size_m": [1.0, 2.0, 1.0], "lods": [{"file": "a.glb", "max_distance_m": 30.0}]})
    }

    fn load(assets: Vec<Value>) -> Result<Catalog, CatalogError> {
        Catalog::from_json_bytes(&serde_json::to_vec(&json!({ "assets": assets })).unwrap())
    }

    #[test]
    fn rejects_missing_licence_duplicate_and_missing_lod() {
        let mut no_licence = asset("a", "");
        no_licence.as_object_mut().unwrap().remove("license");
        assert!(matches!(load(vec![no_licence]), Err(CatalogError::Json(_))));
        assert_eq!(
            load(vec![asset("a", " ")]),
            Err(CatalogError::MissingLicense("a".into()))
        );
        assert_eq!(
            load(vec![asset("a", "CC0-1.0"), asset("a", "CC0-1.0")]),
            Err(CatalogError::DuplicateId("a".into()))
        );
        let mut no_lod = asset("a", "CC0-1.0");
        no_lod["lods"] = json!([]);
        assert_eq!(load(vec![no_lod]), Err(CatalogError::NoLod("a".into())));
    }

    #[test]
    fn private_asset_loads_but_is_not_open() {
        let c = load(vec![asset("ok", "CC-BY-4.0"), asset("mine", "private")]).unwrap();
        let errors = validate_open_licences(&c).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("mine"));
        assert_eq!(
            validate_open_licences(&load(vec![asset("ok", "CC0-1.0")]).unwrap()),
            Ok(())
        );
        assert_eq!(Catalog::from_json_bytes(&c.to_json_bytes()).unwrap(), c);
    }

    #[test]
    fn merge_keeps_the_existing_entry() {
        let mut built_in = load(vec![asset("oak", "CC0-1.0")]).unwrap();
        let mut mine = asset("oak", "private");
        mine["origin"] = json!("User");
        let mut other = asset("tree2", "private");
        other["origin"] = json!("User");
        let skipped = built_in.merge(load(vec![mine, other]).unwrap());
        assert_eq!(skipped, ["oak"]);
        assert_eq!(built_in.assets.len(), 2);
        assert_eq!(built_in.asset("oak").unwrap().license, "CC0-1.0");
    }

    #[test]
    fn search_by_text_category_exposure_and_height() {
        let plant = |h: f64, e: &str| json!({"mature_height_m": h, "mature_width_m": 1.0, "evergreen": true, "exposure": e});
        let mut oak = asset("oak", "CC0-1.0");
        oak["name"] = json!("Oak");
        oak["tags"] = json!(["Native", "shade tree"]);
        oak["plant"] = plant(20.0, "half_shade");
        let mut fern = asset("fern", "CC0-1.0");
        fern["name"] = json!("Fern");
        fern["category"] = json!("perennial");
        fern["tags"] = json!(["native"]);
        fern["plant"] = plant(0.6, "shade");
        let mut chair = asset("chair", "CC0-1.0");
        chair["name"] = json!("Chair");
        chair["category"] = json!("furniture");
        let c = load(vec![oak, fern, chair]).unwrap();
        fn ids(v: Vec<&Asset>) -> Vec<&str> {
            v.into_iter().map(|a| a.id.as_str()).collect()
        }
        assert_eq!(
            ids(c.search("", None, None, None)),
            ["chair", "fern", "oak"]
        );
        assert_eq!(ids(c.search("NATIVE", None, None, None)), ["fern", "oak"]);
        assert_eq!(
            ids(c.search("", Some(Category::Perennial), None, None)),
            ["fern"]
        );
        assert_eq!(
            ids(c.search("", None, Some(Exposure::HalfShade), None)),
            ["oak"]
        );
        assert_eq!(ids(c.search("native", None, None, Some(1.0))), ["fern"]);
        assert!(c.search("zzz", None, None, None).is_empty());
    }

    #[test]
    fn suggested_asset_is_valid_and_warnings_fire() {
        let a = suggest_asset(
            "models/My Garden Chair (v2).glb",
            [-1.0, 2.0, -0.5],
            [1.0, 3.0, 0.5],
            1000,
        );
        assert_eq!(
            (a.id.as_str(), a.name.as_str()),
            ("my-garden-chair-v2", "My Garden Chair (v2)")
        );
        assert_eq!(
            (a.size_m, a.origin, a.license.as_str()),
            ([2.0, 1.0, 1.0], Origin::User, "private")
        );
        assert_eq!(
            normalising_transform([-1.0, 2.0, -0.5], [3.0, 3.0, 1.5]),
            [-1.0, -2.0, -0.5]
        );
        let c = Catalog { assets: vec![a] };
        assert_eq!(Catalog::from_json_bytes(&c.to_json_bytes()).unwrap(), c);
        assert!(import_warnings([0.0; 3], [1.0; 3], 1000).is_empty());
        let w = import_warnings([0.0; 3], [100.0, 1.0, 0.001], 60_000);
        assert_eq!(w.len(), 3, "{w:?}");
    }

    #[test]
    fn attribution_lists_every_asset_by_licence() {
        let mut mine = asset("mine", "private");
        mine["origin"] = json!("User");
        let mut lamp = asset("lamp", "CC-BY-4.0");
        lamp["category"] = json!("lighting");
        lamp["light"] = json!({"kind": {"type": "point"}, "color_temperature_k": 2700.0, "lumens": 400.0, "range_m": 5.0});
        let mut c = load(vec![
            asset("oak", "CC0-1.0"),
            lamp,
            asset("box", "CC0-1.0"),
            mine,
        ])
        .unwrap();
        assert!(c.asset("lamp").unwrap().light.is_some());
        let md = attribution_markdown(&c);
        assert!(!md.contains("Name mine"));
        c.assets.pop();
        for a in &c.assets {
            assert!(
                md.contains(&a.name) && md.contains(&a.author) && md.contains(&a.source_url),
                "{md}"
            );
        }
        let (cc0, by) = (
            md.find("## CC0-1.0").unwrap(),
            md.find("## CC-BY-4.0").unwrap(),
        );
        assert!(cc0 < md.find("Name oak").unwrap() && md.find("Name lamp").unwrap() > by);
        assert!(md.find("Name box").unwrap() < by);
    }
}
