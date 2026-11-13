//! `ingest`: cached sources -> library folders, each with an `entry.json` that `catalog` collects.

use crate::{
    emit,
    glb::{fit, jpeg, write_glb},
    net::{Candidate, ph_info},
    raster,
    scene::{Model, pack_orm, read_gltf_variant, variant_count},
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
    let has = |words: &[&str]| words.iter().any(|w| id.to_lowercase().contains(w));
    let (lumens, range_m, kind) = if has(&["street"]) {
        (
            3000.0,
            15.0,
            LightKind::Spot {
                direction: [0.0, -1.0, 0.0],
                inner_cone_deg: 30.0,
                outer_cone_deg: 60.0,
            },
        )
    } else if has(&["wall", "security", "sconce"]) {
        (500.0, 8.0, LightKind::Point)
    } else if has(&["chandelier", "ceiling", "hanging", "caged", "fluorescent"]) {
        (800.0, 10.0, LightKind::Point)
    } else if has(&["candle", "diya", "oil_lamp"]) {
        (100.0, 3.0, LightKind::Point)
    } else if has(&["bollard"]) {
        (300.0, 5.0, LightKind::Point)
    } else {
        (400.0, 6.0, LightKind::Point)
    };
    LightSpec {
        kind,
        color_temperature_k: 2700.0,
        lumens,
        range_m,
        offset_m: [0.0; 3],
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
        evergreen: Some(evergreen),
        bloom_months: Vec::new(),
        exposure: Some(exposure),
    })
}

/// Every plant gets its measured size (the model is a single specimen, so it stands for the
/// mature size); evergreen and exposure only where the candidate states the species.
fn plant_entry(species: Option<&str>, size: [f32; 3]) -> PlantInfo {
    species
        .and_then(|s| plant_info(s, size))
        .unwrap_or(PlantInfo {
            mature_height_m: size[1],
            mature_width_m: size[0].max(size[2]),
            evergreen: None,
            bloom_months: Vec::new(),
            exposure: None,
        })
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("no parent")?)?;
    fs::write(path, bytes).with_context(|| path.display().to_string())
}

/// Biggest source geometry we accept: beyond this simplification is too slow and too lossy.
const MAX_SOURCE_TRIS: usize = 2_000_000;
const MAX_VARIANTS: usize = 4;
const MAX_GLB_BYTES: usize = 7_500_000;

/// A GLB under the file budget: textures are halved until it fits.
fn glb_fitting(m: &Model, name: &str, mut px: u32) -> Result<Vec<u8>> {
    loop {
        let bytes = write_glb(m, name, px)?;
        if bytes.len() <= MAX_GLB_BYTES || px <= 128 {
            return Ok(bytes);
        }
        px /= 2;
    }
}

/// The model of a candidate, plus up to three further variants as `<id>_b`, `_c`, `_d`.
pub fn ingest_model(c: &Candidate, cache: &Path, out: &Path) -> Result<Vec<Asset>> {
    let src = c.dir(cache);
    let gltf = fs::read_dir(&src)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "gltf"))
        .context("no .gltf in cache (run fetch)")?;
    let info = ph_info(&src)?;
    let mut assets = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    for v in 0..variant_count(&gltf, c.split_roots)?.min(MAX_VARIANTS) {
        let mut model = read_gltf_variant(&gltf, v, c.split_roots)?;
        if model.tris() > MAX_SOURCE_TRIS {
            if v == 0 {
                bail!(
                    "source geometry has {} triangles (over {MAX_SOURCE_TRIS})",
                    model.tris()
                );
            }
            continue;
        }
        // Variants with the same triangle count are copies, not distinct plants.
        if seen.contains(&model.tris()) {
            continue;
        }
        seen.push(model.tris());
        model.normalise();
        let (id, name) = if v == 0 {
            (c.id(), info.name.clone())
        } else {
            let l = (b'a' + v as u8) as char;
            (
                format!("{}_{l}", c.id()),
                format!("{} {}", info.name, l.to_ascii_uppercase()),
            )
        };
        if c.exclude.contains(&id) {
            continue;
        }
        assets.push(ingest_variant(c, out, &info, id, name, model, v > 0)?);
    }
    Ok(assets)
}

/// A lower LOD of `lod0`, at about `target` triangles, that still resembles it. When it does
/// not, the target is raised step by step, and finally the previous LOD is reused.
fn guarded_lod(
    lod0: &Model,
    prev: &Model,
    target: usize,
    notes: &mut Vec<String>,
    k: usize,
) -> Model {
    let mut why = String::new();
    for f in [1.0, 1.4, 2.0, 3.0] {
        let t = (target as f64 * f) as usize;
        if t * 10 >= prev.tris() * 9 {
            break;
        }
        let m = lod0.simplified(t).ground();
        match m.resembles(lod0) {
            Ok(()) => {
                if f > 1.0 {
                    notes.push(format!("lod{k}: gentler target x{f} ({why})"));
                }
                return m;
            }
            Err(e) => why = e,
        }
    }
    if prev.tris() < lod0.tris() || k == 1 {
        notes.push(format!("lod{k}: reused the previous LOD ({why})"));
    } else {
        notes.push(format!("lod{k}: reused lod0 ({why})"));
    }
    prev.clone()
}

/// Growth form from the name, where it is obvious.
fn growth_form(id: &str) -> Option<&'static str> {
    [
        ("fern", "fern"),
        ("succulent", "succulent"),
        ("iceplant", "succulent"),
        ("sapling", "sapling"),
        ("grass", "grass"),
        ("flower", "flower"),
        ("dandelion", "flower"),
        ("celandine", "flower"),
        ("shrub", "shrub"),
        ("bush", "shrub"),
    ]
    .iter()
    .find(|(w, _)| id.contains(w))
    .map(|(_, t)| *t)
}

fn ingest_variant(
    c: &Candidate,
    out: &Path,
    info: &crate::net::ApiInfo,
    id: String,
    name: String,
    model: Model,
    variant: bool,
) -> Result<Asset> {
    let cat = category(&c.category)?;
    let source_tris = model.tris();
    let mut tags = vec![c.category.clone()];
    tags.extend(c.tags.iter().cloned());
    tags.extend(info.tags.iter().map(|t| t.to_lowercase()));
    if variant {
        tags.push("variant".into());
    }
    let mut model = model;
    if cat != Category::Lighting {
        emit::apply(&mut model, &[]);
    }
    let lod0 = if source_tris > budget(cat) {
        tags.push("simplified".into());
        // meshopt may land a little over its target; the budget is a hard limit.
        let mut target = budget(cat) * 95 / 100;
        loop {
            let m = model.simplified(target).ground();
            if m.tris() <= budget(cat) {
                if let Err(why) = m.resembles(&model) {
                    println!("FALLBACK {id} lod0: simplified to the budget but {why}");
                }
                break m;
            }
            target = target * 9 / 10;
        }
    } else {
        model
    };
    let mut lod0 = lod0;
    let (mut light, mut emitter_note) = (None, String::new());
    if cat == Category::Lighting {
        let wall = ["wall", "sconce", "security"]
            .iter()
            .any(|w| c.id().contains(w));
        let e = if c.id().contains("caged") {
            emit::split_body(&mut lod0).unwrap_or_else(|| emit::find(&lod0, &c.id(), wall))
        } else {
            emit::find(&lod0, &c.id(), wall)
        };
        emit::apply(&mut lod0, &e.mats);
        let mut spec = light_spec(&id);
        spec.offset_m = e.offset;
        light = Some(spec);
        emitter_note = format!("{} -> {:?}", e.how, e.offset);
    }
    let t0 = lod0.tris();
    let reuse = t0 < REUSE_BELOW;
    let target = |f: f64| (((t0 as f64 * f).round() as usize).max(MIN_TRIS)).min(t0);
    let mut notes = Vec::new();
    let lod1 = guarded_lod(&lod0, &lod0, target(0.35), &mut notes, 1);
    let lod2 = guarded_lod(&lod0, &lod1, target(0.08), &mut notes, 2);
    for n in &notes {
        println!("FALLBACK {id} {n}");
    }
    let models = [lod0.clone(), lod1, lod2];
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
            write(&dir.join(&file), &glb_fitting(m, &id, MAX_PX[k])?)?;
        }
        let max_distance_m = [largest * 12.0, largest * 40.0, f32::MAX][k];
        lods.push(Lod {
            file: format!("models/{id}/{file}"),
            max_distance_m,
        });
    }
    let mut thumb = Vec::new();
    raster::render(&lod0, 256, None).write_to(
        &mut std::io::Cursor::new(&mut thumb),
        image::ImageFormat::Png,
    )?;
    write(&dir.join("thumb.png"), &thumb)?;
    if is_plant(cat) {
        tags.extend(growth_form(&c.id()).map(str::to_string));
    }
    tags.sort();
    tags.dedup();
    let asset = Asset {
        id: id.clone(),
        name,
        category: cat,
        tags,
        license: "CC0-1.0".into(),
        origin: Origin::BuiltIn,
        author: info.author.clone(),
        source_url: c.page_url.clone(),
        size_m: size,
        lods,
        thumbnail: format!("models/{id}/thumb.png"),
        plant: (is_plant(cat) || c.id().starts_with("potted_plant"))
            .then(|| plant_entry(c.species.as_deref(), size)),
        light,
    };
    write(&dir.join("entry.json"), &serde_json::to_vec_pretty(&asset)?)?;
    println!(
        "model {id}: {source_tris} -> {t0} / {} / {} triangles",
        models[1].tris(),
        models[2].tris()
    );
    if !emitter_note.is_empty() {
        println!("emitter {id}: {emitter_note}");
    }
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
    /// brick, plaster, concrete, wood, paving, gravel, roof, stone, tile, metal, ground, fabric
    /// or other: what the app filters materials by.
    #[serde(default = "other")]
    pub kind: String,
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

fn other() -> String {
    "other".into()
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
        kind: c.kind.clone().unwrap_or_else(other),
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
