//! `assets/tiers.json`: the higher-resolution files of library assets, fetched on demand. An
//! asset without higher tiers has no entry; a missing file means no tiers at all. Paths are
//! relative to the library root.

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Tiers {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub materials: BTreeMap<String, TieredMaterial>,
    #[serde(default)]
    pub skies: BTreeMap<String, TieredFile>,
    #[serde(default)]
    pub models: BTreeMap<String, TieredFile>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct TieredMaterial {
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub source_url: String,
    /// By tier name ("2k", "4k").
    #[serde(default)]
    pub tiers: BTreeMap<String, MaterialTier>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct MaterialTier {
    #[serde(default)]
    pub basecolor: Option<String>,
    #[serde(default)]
    pub normal: Option<String>,
    #[serde(default)]
    pub orm: Option<String>,
    #[serde(default)]
    pub height: Option<String>,
    /// Side of the maps in pixels.
    #[serde(default)]
    pub px: u32,
    #[serde(default)]
    pub bytes: u64,
}

/// A sky or model with its tiers, each one file.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct TieredFile {
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub source_url: String,
    #[serde(default)]
    pub tiers: BTreeMap<String, FileTier>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct FileTier {
    pub file: String,
    #[serde(default)]
    pub bytes: u64,
}

impl Tiers {
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Tiers, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("tiers.json: {e}"))
    }

    /// The `tier` ("2k", "4k") file standing in for the base map at library path `base`
    /// (`materials/<id>/<map>.<ext>`, map one of basecolor, normal, orm, height).
    pub fn material_map(&self, base: &str, tier: &str) -> Option<(&str, u32)> {
        let mut parts = base.split('/');
        let (Some("materials"), Some(id), Some(file)) = (parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        let t = self.materials.get(id)?.tiers.get(tier)?;
        let map = match file.split('.').next()? {
            "basecolor" => &t.basecolor,
            "normal" => &t.normal,
            "orm" => &t.orm,
            "height" => &t.height,
            _ => return None,
        };
        map.as_deref().map(|p| (p, t.px))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_documented_shape_and_maps_base_paths() {
        let j = br#"{"version": 1,
            "materials": {"bricks028": {"license": "CC0-1.0", "author": "a", "source_url": "u",
                "tiers": {"2k": {"basecolor": "materials/bricks028/2k/basecolor.jpg",
                    "normal": "materials/bricks028/2k/normal.jpg", "orm": "materials/bricks028/2k/orm.jpg",
                    "height": null, "px": 2048, "bytes": 10}}}},
            "skies": {"noon": {"tiers": {"4k": {"file": "skies/noon/sky_4k.hdr", "bytes": 5}}}},
            "models": {"bench": {"tiers": {"2k": {"file": "models/bench/lod0_2k.glb"}}}}}"#;
        let t = Tiers::from_json_bytes(j).unwrap();
        assert_eq!(
            t.material_map("materials/bricks028/basecolor.jpg", "2k"),
            Some(("materials/bricks028/2k/basecolor.jpg", 2048))
        );
        assert_eq!(t.material_map("materials/bricks028/height.png", "2k"), None);
        assert_eq!(
            t.material_map("materials/bricks028/basecolor.jpg", "4k"),
            None
        );
        assert_eq!(t.material_map("materials/other/basecolor.jpg", "2k"), None);
        assert_eq!(t.skies["noon"].tiers["4k"].file, "skies/noon/sky_4k.hdr");
        assert_eq!(t.models["bench"].tiers["2k"].bytes, 0);
        assert_eq!(Tiers::from_json_bytes(b"{}").unwrap(), Tiers::default());
    }
}
