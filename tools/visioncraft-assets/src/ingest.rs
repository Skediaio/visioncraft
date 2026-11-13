//! `ingest`: cached sources -> library folders, each with an `entry.json` that `catalog` collects.

use crate::{
    glb::{fit, jpeg, write_glb},
    net::{Candidate, ph_info},
    raster,
    scene::{pack_orm, read_gltf_file},
};
use anyhow::{Context, Result, bail};
use image::{DynamicImage, GrayImage, imageops::FilterType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, io::Read, path::Path, sync::Mutex};
use visioncraft_core::{
    Asset,
    catalog::{Category, Exposure, Lod, Origin, PlantInfo},
    light::{LightKind, LightSpec},
};

pub const OBJECT_BUDGET: usize = 10_000;
pub const PLANT_BUDGET: usize = 20_000;
pub const MIN_TRIS: usize = 24;
pub const REUSE_BELOW: usize = 400;
const MAX_PX: [u32; 3] = [1024, 512, 256];

pub fn is_plant(c: Category) -> bool {
    matches!(
        c,
        Category::Tree
            | Category::Shrub
            | Category::Perennial
            | Category::Grass
            | Category::Flower
            | Category::Hedge
    )
}

pub fn budget(c: Category) -> usize {
    if is_plant(c) {
        PLANT_BUDGET
    } else {
        OBJECT_BUDGET
    }
}

fn category(s: &str) -> Result<Category> {
    let s = match s {
        "light" => "lighting",
        "car" => "vehicle",
        s => s,
    };
    serde_json::from_value(Value::String(s.into()))
        .with_context(|| format!("unknown category `{s}`"))
}

/// Default emission of a fixture, chosen from its name. Warm white, 2700 K.
fn light_spec(id: &str) -> LightSpec {
    let (lumens, range_m, kind) = if id.contains("street") {
        (
            3000.0,
            15.0,
            LightKind::Spot {
                direction: [0.0, -1.0, 0.0],
                inner_cone_deg: 30.0,
                outer_cone_deg: 60.0,
            },
        )
    } else if id.contains("wall") || id.contains("security") {
        (500.0, 8.0, LightKind::Point)
    } else if id.contains("bollard") {
        (300.0, 5.0, LightKind::Point)
    } else {
        (400.0, 6.0, LightKind::Point)
    };
    LightSpec {
        kind,
        color_temperature_k: 2700.0,
        lumens,
        range_m,
    }
}

/// Plant data only for species the candidate list states; size is the modelled size (these
/// are single specimens, so it stands for the mature size), the rest is genus-level.
fn plant_info(species: &str, size: [f32; 3]) -> Option<PlantInfo> {
    let (evergreen, exposure) = match species.split([' ', '(']).next()? {
        "Anthurium" => (true, Exposure::HalfShade),
        "Rumex" | "Empodium" => (false, Exposure::Sun),
        _ => return None,
    };
    Some(PlantInfo {
        mature_height_m: size[1],
        mature_width_m: size[0].max(size[2]),
        evergreen,
        bloom_months: Vec::new(),
        exposure,
    })
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("no parent")?)?;
    fs::write(path, bytes).with_context(|| path.display().to_string())
}

pub fn ingest_model(c: &Candidate, cache: &Path, out: &Path) -> Result<Asset> {
    let id = c.id();
    let src = c.dir(cache);
    let gltf = fs::read_dir(&src)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "gltf"))
        .context("no .gltf in cache (run fetch)")?;
    let info = ph_info(&src)?;
    let cat = category(&c.category)?;
    let mut model = read_gltf_file(&gltf)?;
    model.normalise();
    let source_tris = model.tris();
    let mut tags = vec![c.category.clone()];
    tags.extend(info.tags.iter().map(|t| t.to_lowercase()));
    let lod0 = if source_tris > budget(cat) {
        tags.push("simplified".into());
        {
            // meshopt may land a little over its target; the budget is a hard limit.
            let mut target = budget(cat) * 95 / 100;
            loop {
                let m = model.simplified(target).ground();
                if m.tris() <= budget(cat) {
                    break m;
                }
                target = target * 9 / 10;
            }
        }
    } else {
        model
    };
    let t0 = lod0.tris();
    let reuse = t0 < REUSE_BELOW;
    let target = |f: f64| (((t0 as f64 * f).round() as usize).max(MIN_TRIS)).min(t0);
    let models = [
        lod0.clone(),
        lod0.simplified(target(0.35)).ground(),
        lod0.simplified(target(0.08)).ground(),
    ];
    let dir = out.join("models").join(&id);
    let mut lods = Vec::new();
    let size = lod0.size();
    let largest = size.into_iter().fold(0.0_f32, f32::max);
    for (k, m) in models.iter().enumerate() {
        let file = if reuse {
            "lod0.glb".to_string()
        } else {
            format!("lod{k}.glb")
        };
        if !reuse || k == 0 {
            write(&dir.join(&file), &write_glb(m, &id, MAX_PX[k])?)?;
        }
        let max_distance_m = [largest * 12.0, largest * 40.0, f32::MAX][k];
        lods.push(Lod {
            file: format!("models/{id}/{file}"),
            max_distance_m,
        });
    }
    let mut thumb = Vec::new();
    raster::render(&lod0, 256).write_to(
        &mut std::io::Cursor::new(&mut thumb),
        image::ImageFormat::Png,
    )?;
    write(&dir.join("thumb.png"), &thumb)?;
    tags.sort();
    tags.dedup();
    let asset = Asset {
        id: id.clone(),
        name: info.name,
        category: cat,
        tags,
        license: "CC0-1.0".into(),
        origin: Origin::BuiltIn,
        author: info.author,
        source_url: c.page_url.clone(),
        size_m: size,
        lods,
        thumbnail: format!("models/{id}/thumb.png"),
        plant: c
            .species
            .as_deref()
            .and_then(|s| plant_info(s, size))
            .filter(|_| is_plant(cat)),
        light: (cat == Category::Lighting).then(|| light_spec(&id)),
    };
    write(&dir.join("entry.json"), &serde_json::to_vec_pretty(&asset)?)?;
    println!(
        "model {id}: {source_tris} -> {t0} / {} / {} triangles",
        models[1].tris(),
        models[2].tris()
    );
    Ok(asset)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MaterialMaps {
    pub basecolor: String,
    pub normal: String,
    pub orm: String,
    pub height: Option<String>,
}

/// One entry of `materials.json`. All paths are relative to the library root.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MaterialEntry {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub license: String,
    pub author: String,
    pub source_url: String,
    /// Width, height in metres one repeat of the texture covers; null when the source does not say.
    pub size_m: Option<[f32; 2]>,
    pub maps: MaterialMaps,
    pub thumbnail: String,
}

/// One entry of `skies.json`. Paths are relative to the library root.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SkyEntry {
    pub id: String,
    pub name: String,
    pub license: String,
    pub author: String,
    pub source_url: String,
    pub file: String,
    pub thumbnail: String,
}

fn zip_images(zip: &Path) -> Result<Vec<(String, DynamicImage)>> {
    let mut z = zip::ZipArchive::new(fs::File::open(zip)?)?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let name = f.name().to_string();
        if name.ends_with(".jpg") {
            let mut b = Vec::new();
            f.read_to_end(&mut b)?;
            out.push((name, image::load_from_memory(&b)?));
        }
    }
    Ok(out)
}

pub fn ingest_material(c: &Candidate, cache: &Path, out: &Path) -> Result<MaterialEntry> {
    let (id, sid) = (c.id(), c.source_id());
    let src = c.dir(cache);
    let maps = zip_images(&src.join(format!("{sid}_1K-JPG.zip")))?;
    let find = |suffix: &str| {
        maps.iter()
            .find(|(n, _)| n.ends_with(suffix))
            .map(|(_, i)| i)
    };
    let gray = |s: &str| find(s).map(|i| fit(i, 1024).to_luma8());
    let color = fit(find("_Color.jpg").context("no colour map")?, 1024);
    let normal = fit(find("_NormalGL.jpg").context("no GL normal map")?, 1024);
    let orm = pack_orm(
        gray("_AmbientOcclusion.jpg").as_ref(),
        gray("_Roughness.jpg").as_ref(),
        gray("_Metalness.jpg").as_ref(),
    );
    let height: Option<GrayImage> = gray("_Displacement.jpg");
    let dir = out.join("materials").join(&id);
    let rel = |f: &str| format!("materials/{id}/{f}");
    write(&dir.join("basecolor.jpg"), &jpeg(&color, 90)?)?;
    write(&dir.join("normal.jpg"), &jpeg(&normal, 95)?)?;
    write(
        &dir.join("orm.jpg"),
        &jpeg(&DynamicImage::ImageRgb8(orm), 90)?,
    )?;
    if let Some(h) = &height {
        write(
            &dir.join("height.jpg"),
            &jpeg(&DynamicImage::ImageLuma8(h.clone()), 90)?,
        )?;
    }
    let side = color.width().min(color.height());
    let crop = color.crop_imm(
        (color.width() - side) / 2,
        (color.height() - side) / 2,
        side,
        side,
    );
    write(
        &dir.join("thumb.jpg"),
        &jpeg(&crop.resize_exact(128, 128, FilterType::Lanczos3), 85)?,
    )?;
    let info: Value = serde_json::from_slice(&fs::read(src.join("info.json"))?)?;
    let a = &info["assets"][0];
    let d = &a["dimensions"];
    let (w, h) = (
        d["width"].as_f64().unwrap_or(0.0),
        d["height"].as_f64().unwrap_or(0.0),
    );
    let entry = MaterialEntry {
        id: id.clone(),
        name: a["title"].as_str().unwrap_or(sid).into(),
        tags: a["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        license: "CC0-1.0".into(),
        // The ambientCG API does not state authors; the candidate list does (site-wide credit).
        author: c.author.clone(),
        source_url: c.page_url.clone(),
        size_m: (w > 0.0 && h > 0.0).then(|| [(w / 100.0) as f32, (h / 100.0) as f32]),
        maps: MaterialMaps {
            basecolor: rel("basecolor.jpg"),
            normal: rel("normal.jpg"),
            orm: rel("orm.jpg"),
            height: height.map(|_| rel("height.jpg")),
        },
        thumbnail: rel("thumb.jpg"),
    };
    write(&dir.join("entry.json"), &serde_json::to_vec_pretty(&entry)?)?;
    println!("material {id}");
    Ok(entry)
}

pub fn ingest_sky(c: &Candidate, cache: &Path, out: &Path) -> Result<SkyEntry> {
    let id = c.id();
    let src = c.dir(cache);
    let hdr = fs::read_dir(&src)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "hdr"))
        .context("no .hdr in cache")?;
    let bytes = fs::read(&hdr)?;
    let dir = out.join("skies").join(&id);
    write(&dir.join("sky_1k.hdr"), &bytes)?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Hdr)?.to_rgb32f();
    let small = image::imageops::resize(&img, 256, 128, FilterType::Triangle);
    // Exposure from the log-average luminance (the sun alone would make the sky look black),
    // then Reinhard and gamma 2.2.
    let lum = |p: &image::Rgb<f32>| 0.2126 * p.0[0] + 0.7152 * p.0[1] + 0.0722 * p.0[2];
    let log_avg = (small.pixels().map(|p| (lum(p) + 1e-4).ln()).sum::<f32>()
        / (small.width() * small.height()) as f32)
        .exp();
    let k = 0.18 / log_avg;
    let ldr = image::RgbImage::from_fn(small.width(), small.height(), |x, y| {
        image::Rgb(small.get_pixel(x, y).0.map(|v| {
            let t = v * k;
            ((t / (1.0 + t)).powf(1.0 / 2.2) * 255.0).round() as u8
        }))
    });
    write(
        &dir.join("thumb.jpg"),
        &jpeg(&DynamicImage::ImageRgb8(ldr), 85)?,
    )?;
    let info = ph_info(&src)?;
    let entry = SkyEntry {
        id: id.clone(),
        name: info.name,
        license: "CC0-1.0".into(),
        author: info.author,
        source_url: c.page_url.clone(),
        file: format!("skies/{id}/sky_1k.hdr"),
        thumbnail: format!("skies/{id}/thumb.jpg"),
    };
    write(&dir.join("entry.json"), &serde_json::to_vec_pretty(&entry)?)?;
    println!("sky {id}");
    Ok(entry)
}

/// Ingest every candidate, `jobs` at a time; a failure is reported and makes the result an error.
pub fn ingest_all(cands: &[Candidate], cache: &Path, out: &Path, jobs: usize) -> Result<()> {
    let queue = Mutex::new(cands.iter());
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let next = queue.lock().unwrap().next();
                    let Some(c) = next else { break };
                    let r = if c.is_sky() {
                        ingest_sky(c, cache, out).map(drop)
                    } else if c.is_material() {
                        ingest_material(c, cache, out).map(drop)
                    } else {
                        ingest_model(c, cache, out).map(drop)
                    };
                    if let Err(e) = r {
                        println!("FAILED {}: {e:#}", c.source_id());
                        failed.lock().unwrap().push(c.source_id().to_string());
                    }
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    if failed.is_empty() {
        Ok(())
    } else {
        bail!("failed: {}", failed.join(", "))
    }
}
