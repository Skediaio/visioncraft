//! `catalog`, `check`, and the contact-sheet / LOD-comparison helpers.

use crate::{
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
const KINDS: [&str; 13] = [
    "brick", "plaster", "concrete", "wood", "paving", "gravel", "roof", "stone", "tile", "metal",
    "ground", "fabric", "other",
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
    md += "\n## Sources\n\nModels and skies: [Poly Haven](https://polyhaven.com), CC0. Materials: [ambientCG](https://ambientcg.com), CC0.\n";
    fs::write(out.join("catalog.json"), bytes)?;
    fs::write(
        out.join("materials.json"),
        serde_json::to_vec_pretty(&materials)?,
    )?;
    fs::write(out.join("skies.json"), serde_json::to_vec_pretty(&skies)?)?;
    fs::write(out.join("ATTRIBUTION.md"), md)?;
    println!(
        "catalogue: {} models, {} materials, {} skies",
        catalog.assets.len(),
        materials.materials.len(),
        skies.skies.len()
    );
    Ok(())
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
    walk(out, &mut |p| {
        if fs::metadata(p).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            errors.push(format!("{} is over 8 MB", p.display()));
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
