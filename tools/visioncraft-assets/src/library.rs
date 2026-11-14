//! `catalog`, `check`, and the contact-sheet / LOD-comparison helpers.

use crate::{
    flora::FloraFile,
    glb::png,
    ingest::{MIN_TRIS, MaterialEntry, SkyEntry, budget},
    raster,
    scene::read_glb,
};
use anyhow::{Context, Result, bail};
use image::{DynamicImage, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use visioncraft_core::{Asset, Catalog, attribution_markdown, validate_open_licences};

/// What `materials.json` may call a kind.
const KINDS: [&str; 14] = [
    "brick", "plaster", "concrete", "wood", "paving", "asphalt", "gravel", "roof", "stone", "tile",
    "metal", "ground", "fabric", "other",
];
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// `materials.json`.
#[derive(Serialize, Deserialize, Default)]
pub struct Materials {
    pub materials: Vec<MaterialEntry>,
}

/// `skies.json`.
#[derive(Serialize, Deserialize, Default)]
pub struct Skies {
    pub skies: Vec<SkyEntry>,
}

fn entries<T: serde::de::DeserializeOwned>(out: &Path, kind: &str) -> Result<Vec<T>> {
    let mut dirs: Vec<PathBuf> = match fs::read_dir(out.join(kind)) {
        Ok(d) => d.filter_map(Result::ok).map(|e| e.path()).collect(),
        Err(_) => Vec::new(),
    };
    dirs.sort();
    dirs.iter()
        .filter(|d| d.join("entry.json").exists())
        .map(|d| Ok(serde_json::from_slice(&fs::read(d.join("entry.json"))?)?))
        .collect()
}

pub fn catalog(out: &Path) -> Result<()> {
    let catalog = Catalog {
        assets: entries::<Asset>(out, "models")?,
    };
    let bytes = catalog.to_json_bytes();
    let catalog = Catalog::from_json_bytes(&bytes).context("catalogue does not validate")?;
    if let Err(bad) = validate_open_licences(&catalog) {
        bail!("licence check failed:\n{}", bad.join("\n"));
    }
    let materials = Materials {
        materials: entries(out, "materials")?,
    };
    let skies = Skies {
        skies: entries(out, "skies")?,
    };
    let mut md = attribution_markdown(&catalog);
    md += "\n## Materials (CC0-1.0)\n\n";
    for m in &materials.materials {
        md += &format!("- {} by {} ({})\n", m.name, m.author, m.source_url);
    }
    md += "\n## Skies (CC0-1.0)\n\n";
    for s in &skies.skies {
        md += &format!("- {} by {} ({})\n", s.name, s.author, s.source_url);
    }
    md += "\n## Higher-resolution tiers\n\nThe 2k and 4k files listed in `tiers.json` are the same assets from the same sources, with the same licence and author as their entries above.\n";
    md += "\n## Sources\n\nModels and skies: [Poly Haven](https://polyhaven.com), CC0. Materials: [ambientCG](https://ambientcg.com), CC0.\n";
    fs::write(out.join("catalog.json"), bytes)?;
    fs::write(
        out.join("materials.json"),
        serde_json::to_vec_pretty(&materials)?,
    )?;
    fs::write(out.join("skies.json"), serde_json::to_vec_pretty(&skies)?)?;
    let flora = FloraFile {
        version: 1,
        leaf_sets: entries(out, "flora/leaves")?,
        bark_sets: entries(out, "flora/bark")?,
        species: fs::read(out.join("flora/species.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default(),
    };
    fs::write(out.join("flora.json"), serde_json::to_vec_pretty(&flora)?)?;
    md += "\n## Scanned leaves and bark (CC0-1.0)\n\n";
    for s in &flora.leaf_sets {
        md += &format!("- {} (leaves) by {} ({})\n", s.name, s.author, s.source_url);
    }
    for s in &flora.bark_sets {
        md += &format!("- {} (bark) by {} ({})\n", s.name, s.author, s.source_url);
    }
    fs::write(out.join("ATTRIBUTION.md"), md)?;
    let tiers = build_tiers(out, &catalog, &materials, &skies, &flora);
    fs::write(out.join("tiers.json"), serde_json::to_vec_pretty(&tiers)?)?;
    println!(
        "tiers: {} materials, {} skies, {} models with higher tiers",
        tiers.materials.len(),
        tiers.skies.len(),
        tiers.models.len()
    );
    println!(
        "catalogue: {} models, {} materials, {} skies",
        catalog.assets.len(),
        materials.materials.len(),
        skies.skies.len()
    );
    Ok(())
}

/// One higher-resolution set of a material; paths relative to the library root.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MaterialTier {
    pub basecolor: String,
    pub normal: String,
    pub orm: String,
    pub height: Option<String>,
    /// Longest side of the maps in pixels.
    pub px: u32,
    /// Total size of the files of this tier.
    pub bytes: u64,
}

/// One higher-resolution file of a sky or a model.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileTier {
    pub file: String,
    pub bytes: u64,
}

/// An asset's higher tiers with its licence, author and source, keyed by tier name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TierEntry<T> {
    pub license: String,
    pub author: String,
    pub source_url: String,
    pub tiers: std::collections::BTreeMap<String, T>,
}

/// `tiers.json`: what exists above the base set, written by `catalog` from the files on disk.
/// Base files (1k materials and skies, lod0..2 models) are never replaced by these.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Tiers {
    pub version: u32,
    pub materials: std::collections::BTreeMap<String, TierEntry<MaterialTier>>,
    pub skies: std::collections::BTreeMap<String, TierEntry<FileTier>>,
    pub models: std::collections::BTreeMap<String, TierEntry<FileTier>>,
    /// Scanned leaf atlases and bark; absent in older files.
    #[serde(default)]
    pub flora: FloraTiers,
}

/// A leaf atlas at a higher resolution.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LeafTier {
    pub color: String,
    pub opacity: String,
    pub normal: String,
    pub translucency: Option<String>,
    pub px: u32,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct FloraTiers {
    pub leaves: std::collections::BTreeMap<String, TierEntry<LeafTier>>,
    pub bark: std::collections::BTreeMap<String, TierEntry<MaterialTier>>,
}

fn size_of(out: &Path, rel: &str) -> Option<u64> {
    fs::metadata(out.join(rel)).ok().map(|m| m.len())
}

pub fn build_tiers(
    out: &Path,
    cat: &Catalog,
    mats: &Materials,
    skies: &Skies,
    flora: &FloraFile,
) -> Tiers {
    let mut t = Tiers {
        version: 1,
        ..Tiers::default()
    };
    for m in &mats.materials {
        let mut tiers = std::collections::BTreeMap::new();
        for tier in ["2k", "4k"] {
            let rel = |f: &str| format!("materials/{}/{tier}/{f}", m.id);
            let Some(mut bytes) = size_of(out, &rel("basecolor.jpg")) else {
                continue;
            };
            let height = size_of(out, &rel("height.jpg")).inspect(|b| bytes += b);
            bytes += ["normal.jpg", "orm.jpg"]
                .iter()
                .filter_map(|f| size_of(out, &rel(f)))
                .sum::<u64>();
            let px =
                image::image_dimensions(out.join(rel("basecolor.jpg"))).map_or(0, |d| d.0.max(d.1));
            tiers.insert(
                tier.to_string(),
                MaterialTier {
                    basecolor: rel("basecolor.jpg"),
                    normal: rel("normal.jpg"),
                    orm: rel("orm.jpg"),
                    height: height.map(|_| rel("height.jpg")),
                    px,
                    bytes,
                },
            );
        }
        if !tiers.is_empty() {
            t.materials.insert(
                m.id.clone(),
                TierEntry {
                    license: m.license.clone(),
                    author: m.author.clone(),
                    source_url: m.source_url.clone(),
                    tiers,
                },
            );
        }
    }
    for s in &skies.skies {
        let tiers: std::collections::BTreeMap<_, _> = ["4k", "8k"]
            .iter()
            .filter_map(|tier| {
                let file = format!("skies/{}/sky_{tier}.hdr", s.id);
                size_of(out, &file).map(|bytes| (tier.to_string(), FileTier { file, bytes }))
            })
            .collect();
        if !tiers.is_empty() {
            t.skies.insert(
                s.id.clone(),
                TierEntry {
                    license: s.license.clone(),
                    author: s.author.clone(),
                    source_url: s.source_url.clone(),
                    tiers,
                },
            );
        }
    }
    for a in &cat.assets {
        let file = format!("models/{}/lod0_2k.glb", a.id);
        if let Some(bytes) = size_of(out, &file) {
            let tiers =
                std::collections::BTreeMap::from([("2k".to_string(), FileTier { file, bytes })]);
            t.models.insert(
                a.id.clone(),
                TierEntry {
                    license: a.license.clone(),
                    author: a.author.clone(),
                    source_url: a.source_url.clone(),
                    tiers,
                },
            );
        }
    }
    for s in &flora.leaf_sets {
        let rel = |f: &str| format!("flora/leaves/{}/2k/{f}", s.id);
        let Some(mut bytes) = size_of(out, &rel("color.jpg")) else {
            continue;
        };
        bytes += ["opacity.png", "normal.jpg"]
            .iter()
            .filter_map(|f| size_of(out, &rel(f)))
            .sum::<u64>();
        let translucency = size_of(out, &rel("translucency.jpg")).inspect(|b| bytes += b);
        let px = image::image_dimensions(out.join(rel("color.jpg"))).map_or(0, |d| d.0.max(d.1));
        let tier = LeafTier {
            color: rel("color.jpg"),
            opacity: rel("opacity.png"),
            normal: rel("normal.jpg"),
            translucency: translucency.map(|_| rel("translucency.jpg")),
            px,
            bytes,
        };
        t.flora.leaves.insert(
            s.id.clone(),
            TierEntry {
                license: s.license.clone(),
                author: s.author.clone(),
                source_url: s.source_url.clone(),
                tiers: std::collections::BTreeMap::from([("2k".to_string(), tier)]),
            },
        );
    }
    for s in &flora.bark_sets {
        let rel = |f: &str| format!("flora/bark/{}/2k/{f}", s.id);
        let Some(mut bytes) = size_of(out, &rel("basecolor.jpg")) else {
            continue;
        };
        bytes += ["normal.jpg", "orm.jpg"]
            .iter()
            .filter_map(|f| size_of(out, &rel(f)))
            .sum::<u64>();
        let height = size_of(out, &rel("height.jpg")).inspect(|b| bytes += b);
        let px =
            image::image_dimensions(out.join(rel("basecolor.jpg"))).map_or(0, |d| d.0.max(d.1));
        let tier = MaterialTier {
            basecolor: rel("basecolor.jpg"),
            normal: rel("normal.jpg"),
            orm: rel("orm.jpg"),
            height: height.map(|_| rel("height.jpg")),
            px,
            bytes,
        };
        t.flora.bark.insert(
            s.id.clone(),
            TierEntry {
                license: s.license.clone(),
                author: s.author.clone(),
                source_url: s.source_url.clone(),
                tiers: std::collections::BTreeMap::from([("2k".to_string(), tier)]),
            },
        );
    }
    t
}

fn walk(dir: &Path, f: &mut impl FnMut(&Path)) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == ".cache") {
            continue;
        }
        if p.is_dir() { walk(&p, f) } else { f(&p) }
    }
}

/// Every problem found in the library on disk; empty when it is sound.
pub fn check(out: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    let need = |rel: &str, errors: &mut Vec<String>| {
        if !out.join(rel).is_file() {
            errors.push(format!("missing file {rel}"));
            false
        } else {
            true
        }
    };
    let cat = fs::read(out.join("catalog.json"))
        .map_err(|e| e.to_string())
        .and_then(|b| Catalog::from_json_bytes(&b).map_err(|e| e.to_string()));
    match cat {
        Err(e) => errors.push(format!("catalog.json: {e}")),
        Ok(cat) => {
            if let Err(bad) = validate_open_licences(&cat) {
                errors.extend(bad);
            }
            for a in &cat.assets {
                need(&a.thumbnail, &mut errors);
                let mut tris = Vec::new();
                for l in &a.lods {
                    if !need(&l.file, &mut errors) {
                        continue;
                    }
                    match fs::read(out.join(&l.file))
                        .map_err(anyhow::Error::from)
                        .and_then(|b| read_glb(&b))
                    {
                        Ok(m) => {
                            tris.push(m.tris());
                            let (lo, _) = m.bounds();
                            if lo[1].abs() > 1e-3 {
                                errors.push(format!(
                                    "{}: lowest point is at y = {}, not 0",
                                    l.file, lo[1]
                                ));
                            }
                        }
                        Err(e) => errors.push(format!("{}: {e:#}", l.file)),
                    }
                }
                if tris.first().is_some_and(|&t| t > budget(a.category)) {
                    errors.push(format!(
                        "{}: {} triangles, budget {}",
                        a.id,
                        tris[0],
                        budget(a.category)
                    ));
                }
                if tris.windows(2).any(|w| w[1] > w[0])
                    || tris.iter().any(|&t| t < MIN_TRIS.min(tris[0]))
                {
                    errors.push(format!("{}: LOD triangle counts {tris:?} are not decreasing or fall below {MIN_TRIS}", a.id));
                }
                if a.size_m.iter().any(|&s| s <= 0.0 || !s.is_finite()) {
                    errors.push(format!("{}: bad size {:?}", a.id, a.size_m));
                }
            }
        }
    }
    match fs::read(out.join("materials.json"))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Materials>(&b).map_err(|e| e.to_string()))
    {
        Err(e) => errors.push(format!("materials.json: {e}")),
        Ok(m) => {
            for m in m.materials {
                let maps = [
                    Some(&m.maps.basecolor),
                    Some(&m.maps.normal),
                    Some(&m.maps.orm),
                    m.maps.height.as_ref(),
                    Some(&m.thumbnail),
                ];
                for f in maps.into_iter().flatten() {
                    need(f, &mut errors);
                }
                if m.license != "CC0-1.0" && m.license != "CC-BY-4.0" {
                    errors.push(format!("material {} has licence {}", m.id, m.license));
                }
                if !KINDS.contains(&m.kind.as_str()) {
                    errors.push(format!("material {} has unknown kind {}", m.id, m.kind));
                }
            }
        }
    }
    match fs::read(out.join("skies.json"))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Skies>(&b).map_err(|e| e.to_string()))
    {
        Err(e) => errors.push(format!("skies.json: {e}")),
        Ok(s) => {
            for s in s.skies {
                need(&s.file, &mut errors);
                need(&s.thumbnail, &mut errors);
                if s.license != "CC0-1.0" && s.license != "CC-BY-4.0" {
                    errors.push(format!("sky {} has licence {}", s.id, s.license));
                }
            }
        }
    }
    need("ATTRIBUTION.md", &mut errors);
    // tiers.json: every file exists and the recorded sizes are the real ones
    match fs::read(out.join("tiers.json"))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Tiers>(&b).map_err(|e| e.to_string()))
    {
        Err(e) => errors.push(format!("tiers.json: {e}")),
        Ok(t) => {
            let mut files: Vec<(String, u64)> = Vec::new();
            for e in t.materials.values().flat_map(|e| e.tiers.values()) {
                for f in [
                    Some(&e.basecolor),
                    Some(&e.normal),
                    Some(&e.orm),
                    e.height.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    files.push((f.clone(), 0));
                }
            }
            for e in t
                .skies
                .values()
                .chain(t.models.values())
                .flat_map(|e| e.tiers.values())
            {
                files.push((e.file.clone(), e.bytes));
            }
            for e in t.flora.leaves.values().flat_map(|e| e.tiers.values()) {
                for f in [
                    Some(&e.color),
                    Some(&e.opacity),
                    Some(&e.normal),
                    e.translucency.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    files.push((f.clone(), 0));
                }
            }
            for e in t.flora.bark.values().flat_map(|e| e.tiers.values()) {
                for f in [
                    Some(&e.basecolor),
                    Some(&e.normal),
                    Some(&e.orm),
                    e.height.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    files.push((f.clone(), 0));
                }
            }
            for (f, bytes) in files {
                match fs::metadata(out.join(&f)) {
                    Err(_) => errors.push(format!("tiers.json: missing file {f}")),
                    Ok(m) if bytes != 0 && m.len() != bytes => {
                        errors.push(format!("tiers.json: {f} is {} bytes, not {bytes}", m.len()))
                    }
                    Ok(_) => {}
                }
            }
        }
    }
    // flora.json: every file exists, every cut-out is inside its atlas, every species points at a set
    match fs::read(out.join("flora.json"))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<FloraFile>(&b).map_err(|e| e.to_string()))
    {
        Err(e) => errors.push(format!("flora.json: {e}")),
        Ok(f) => {
            for s in &f.leaf_sets {
                for p in [
                    Some(&s.maps.color),
                    Some(&s.maps.opacity),
                    Some(&s.maps.normal),
                    s.maps.translucency.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    need(p, &mut errors);
                }
                if s.leaves.is_empty() {
                    errors.push(format!("leaf set {} has no cut-outs", s.id));
                }
                for c in &s.leaves {
                    let inside = c.pivot.iter().all(|v| (0.0..=1.0).contains(v))
                        && c.rect[0] + c.rect[2] <= 1.001
                        && c.rect[1] + c.rect[3] <= 1.001;
                    if !inside {
                        errors.push(format!("leaf set {} has a cut-out outside the atlas", s.id));
                    }
                }
            }
            for s in &f.bark_sets {
                for p in [
                    Some(&s.maps.basecolor),
                    Some(&s.maps.normal),
                    Some(&s.maps.orm),
                    s.maps.height.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    need(p, &mut errors);
                }
            }
            let leaf_ids: Vec<&String> = f.leaf_sets.iter().map(|s| &s.id).collect();
            let bark_ids: Vec<&String> = f.bark_sets.iter().map(|s| &s.id).collect();
            for (sp, r) in &f.species {
                if r.leaf.as_ref().is_some_and(|l| !leaf_ids.contains(&&l[0]))
                    || r.leaf_autumn
                        .as_ref()
                        .is_some_and(|l| !leaf_ids.contains(&l))
                {
                    errors.push(format!(
                        "flora.json: species {sp} points at a leaf set that is not there"
                    ));
                }
                if r.bark.as_ref().is_some_and(|b| !bark_ids.contains(&&b[0])) {
                    errors.push(format!(
                        "flora.json: species {sp} points at a bark set that is not there"
                    ));
                }
            }
        }
    }
    // the tracked text files must not carry anything private
    for f in [
        "catalog.json",
        "materials.json",
        "skies.json",
        "tiers.json",
        "flora.json",
        "ATTRIBUTION.md",
    ] {
        let text = fs::read_to_string(out.join(f)).unwrap_or_default();
        if ["/home/", "/Users/", "192.168.", "10.0.0."]
            .iter()
            .any(|p| text.contains(p))
        {
            errors.push(format!("{f} contains a path or address that looks private"));
        }
    }
    walk(out, &mut |p| {
        let name = p.to_string_lossy();
        let tier = name.contains("/2k/")
            || name.contains("/4k/")
            || name.contains("_2k.")
            || name.contains("_4k.")
            || name.contains("_8k.");
        let limit = if tier {
            crate::ingest::MAX_TIER_BYTES as u64
        } else {
            MAX_FILE_BYTES
        };
        if fs::metadata(p).is_ok_and(|m| m.len() > limit) {
            let rel = p.strip_prefix(out).unwrap_or(p);
            errors.push(format!(
                "{} is over {} MB",
                rel.display(),
                limit / 1_000_000
            ));
        }
    });
    errors
}

fn save(img: &RgbaImage, to: &Path) -> Result<()> {
    fs::write(to, png(&DynamicImage::ImageRgba8(img.clone()))?)?;
    Ok(())
}

/// All model thumbnails on a grey grid, in catalogue order (6 per row).
pub fn sheet(out: &Path, to: &Path) -> Result<()> {
    let cat = Catalog::from_json_bytes(&fs::read(out.join("catalog.json"))?)?;
    let rows = cat.assets.len().div_ceil(6) as u32;
    let mut sheet = RgbaImage::from_pixel(6 * 256, rows * 256, image::Rgba([200, 200, 200, 255]));
    for (i, a) in cat.assets.iter().enumerate() {
        let t = image::open(out.join(&a.thumbnail))?.to_rgba8();
        image::imageops::overlay(&mut sheet, &t, (i as i64 % 6) * 256, (i as i64 / 6) * 256);
        println!("{} {}", i, a.id);
    }
    save(&sheet, to)
}

/// One image per light fixture: lod0 with a marker where the catalogue puts the emitter.
pub fn lightsheet(out: &Path, to: &Path) -> Result<()> {
    let cat = Catalog::from_json_bytes(&fs::read(out.join("catalog.json"))?)?;
    fs::create_dir_all(to)?;
    for a in cat.assets.iter().filter(|a| a.light.is_some()) {
        let spec = a.light.context("no light")?;
        let m = read_glb(&fs::read(out.join(&a.lods[0].file))?)?;
        println!("{}: offset {:?}", a.id, spec.offset_m);
        let img = raster::render(&m, 384, Some(spec.offset_m));
        img.save(to.join(format!("{}.png", a.id)))?;
    }
    Ok(())
}

/// lod0, lod1 and lod2 of one asset side by side, rendered from the GLBs on disk.
pub fn lodcmp(out: &Path, id: &str, to: &Path) -> Result<()> {
    let cat = Catalog::from_json_bytes(&fs::read(out.join("catalog.json"))?)?;
    let a = cat.asset(id).with_context(|| format!("no asset {id}"))?;
    let mut img = RgbaImage::from_pixel(
        a.lods.len() as u32 * 512,
        512,
        image::Rgba([200, 200, 200, 255]),
    );
    for (i, l) in a.lods.iter().enumerate() {
        let m = read_glb(&fs::read(out.join(&l.file))?)?;
        println!("{}: {} triangles", l.file, m.tris());
        image::imageops::overlay(&mut img, &raster::render(&m, 512, None), i as i64 * 512, 0);
    }
    save(&img, to)
}
