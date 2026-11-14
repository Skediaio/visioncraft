//! Scanned leaf atlases and bark for the plant generator: fetch candidates, ingest, cut-outs.

use crate::{
    glb::{fit, jpeg},
    ingest::{MaterialMaps3, material_maps},
    net::{Candidate, ph_info},
};
use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage, ImageFormat, RgbaImage, imageops::FilterType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::Read, path::Path};

/// `candidates/flora.json`: which sets, what they are, and which species use which.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FloraSpec {
    pub leaf_sets: Vec<LeafSetSpec>,
    pub bark_sets: Vec<BarkSetSpec>,
    pub species: BTreeMap<String, SpeciesMap>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LeafSetSpec {
    pub source: String,
    pub id: String,
    pub genus: Option<String>,
    /// `source` when the source's tags state it, `eye` when identified by looking.
    pub genus_from: String,
    pub shape: String,
    pub season: String,
    pub notes: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BarkSetSpec {
    pub source: String,
    pub id: String,
    pub genus: Option<String>,
    pub genus_from: String,
    pub pattern: String,
    pub notes: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SpeciesMap {
    /// [set id, `genus` or `shape`]
    pub leaf: Option<[String; 2]>,
    pub leaf_autumn: Option<String>,
    pub bark: Option<[String; 2]>,
}

impl FloraSpec {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(
            &fs::read(path).with_context(|| path.display().to_string())?,
        )?)
    }

    /// The sets as fetch/ingest candidates (category `leafset` or `barkset`).
    pub fn candidates(&self) -> Vec<Candidate> {
        let leaf = self.leaf_sets.iter().map(|s| ("leafset", &s.source, &s.id));
        let bark = self.bark_sets.iter().map(|s| ("barkset", &s.source, &s.id));
        leaf.chain(bark)
            .map(|(cat, source, id)| {
                let url = if source == "Poly Haven" {
                    format!("https://polyhaven.com/a/{id}")
                } else {
                    format!("https://ambientcg.com/a/{id}")
                };
                let author = if source == "ambientCG" {
                    "ambientCG (Lennart Demes)"
                } else {
                    ""
                };
                serde_json::from_value(json!({"category": cat, "name": id, "source": source,
                    "page_url": url, "author": author, "triangles": null, "species": null}))
                .expect("a candidate")
            })
            .collect()
    }
}

/// One scanned leaf (or leaf cluster) in an atlas, in atlas coordinates 0..1, y down.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LeafCut {
    /// x, y, width, height of the bounding rectangle.
    pub rect: [f32; 4],
    /// Where the leaf is attached: the end of the petiole (grass: the base).
    pub pivot: [f32; 2],
    /// True when the pivot is the end of a real petiole found in the scan; false when no
    /// petiole is visible and the pivot is a guess at the thinner end (round leaves: use
    /// `centre`).
    pub petiole: bool,
    /// Centre of the leaf shape.
    pub centre: [f32; 2],
    /// Unit vector from the pivot towards the leaf tip, in atlas space (y down).
    pub axis: [f32; 2],
    /// Length from pivot to tip, as a fraction of the atlas width.
    pub length: f32,
    /// Share of the atlas the leaf covers.
    pub area: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LeafMaps {
    pub color: String,
    pub opacity: String,
    pub normal: String,
    pub translucency: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LeafSetEntry {
    pub id: String,
    pub name: String,
    pub license: String,
    pub author: String,
    pub source_url: String,
    pub genus: Option<String>,
    pub genus_from: String,
    pub shape: String,
    pub season: String,
    pub notes: String,
    pub px: u32,
    pub maps: LeafMaps,
    pub leaves: Vec<LeafCut>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BarkMaps {
    pub basecolor: String,
    pub normal: String,
    pub orm: String,
    pub height: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BarkSetEntry {
    pub id: String,
    pub name: String,
    pub license: String,
    pub author: String,
    pub source_url: String,
    pub genus: Option<String>,
    pub genus_from: String,
    pub pattern: String,
    pub notes: String,
    /// Metres one repeat covers, when the source says.
    pub size_m: Option<[f32; 2]>,
    pub px: u32,
    pub maps: BarkMaps,
}

/// The species table with lower-case set ids, as written to `flora.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SpeciesRef {
    pub leaf: Option<[String; 2]>,
    pub leaf_autumn: Option<String>,
    pub bark: Option<[String; 2]>,
}

/// `flora.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FloraFile {
    pub version: u32,
    pub leaf_sets: Vec<LeafSetEntry>,
    pub bark_sets: Vec<BarkSetEntry>,
    pub species: BTreeMap<String, SpeciesRef>,
}

/// Find the leaves in an opacity map: connected pieces above half opacity, each with its
/// bounding box, pivot and direction. The pivot is the end of the petiole (the thin protrusion that
/// sticks out furthest), or for `grass` the base of the blade (lower or left end).
pub fn cutouts(opacity: &GrayImage, grass: bool) -> Vec<LeafCut> {
    let (w, h) = opacity.dimensions();
    let mut seen = vec![false; (w * h) as usize];
    let inside = |x: u32, y: u32| opacity.get_pixel(x, y).0[0] >= 128;
    let mut out = Vec::new();
    for sy in 0..h {
        for sx in 0..w {
            let i = (sy * w + sx) as usize;
            if seen[i] || !inside(sx, sy) {
                continue;
            }
            let mut pixels: Vec<(u32, u32)> = Vec::new();
            let mut stack = vec![(sx, sy)];
            seen[i] = true;
            while let Some((x, y)) = stack.pop() {
                pixels.push((x, y));
                for (nx, ny) in [
                    (x.wrapping_sub(1), y),
                    (x + 1, y),
                    (x, y.wrapping_sub(1)),
                    (x, y + 1),
                ] {
                    if nx < w && ny < h && !seen[(ny * w + nx) as usize] && inside(nx, ny) {
                        seen[(ny * w + nx) as usize] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            let area = pixels.len() as f64 / f64::from(w * h);
            if area >= 0.0015 {
                out.push(cut_of(&pixels, w, h, grass));
            }
        }
    }
    out.sort_by(|a, b| {
        let (ay, by) = ((a.rect[1] * 6.0).floor(), (b.rect[1] * 6.0).floor());
        ay.total_cmp(&by).then(a.rect[0].total_cmp(&b.rect[0]))
    });
    out.truncate(96);
    out
}

/// Chamfer distance (3 per step, 4 per diagonal) of every pixel of a piece to the outside.
fn distance_inside(pixels: &[(u32, u32)], x0: u32, y0: u32, bw: u32, bh: u32) -> Vec<u32> {
    let (bw, bh) = (bw as usize + 2, bh as usize + 2);
    let big = u32::MAX / 2;
    let mut d = vec![0u32; bw * bh];
    for &(x, y) in pixels {
        d[((y - y0) as usize + 1) * bw + (x - x0) as usize + 1] = big;
    }
    for y in 1..bh - 1 {
        for x in 1..bw - 1 {
            let i = y * bw + x;
            if d[i] != 0 {
                d[i] = d[i]
                    .min(d[i - 1] + 3)
                    .min(d[i - bw] + 3)
                    .min(d[i - bw - 1] + 4)
                    .min(d[i - bw + 1] + 4);
            }
        }
    }
    for y in (1..bh - 1).rev() {
        for x in (1..bw - 1).rev() {
            let i = y * bw + x;
            if d[i] != 0 {
                d[i] = d[i]
                    .min(d[i + 1] + 3)
                    .min(d[i + bw] + 3)
                    .min(d[i + bw + 1] + 4)
                    .min(d[i + bw - 1] + 4);
            }
        }
    }
    pixels
        .iter()
        .map(|&(x, y)| d[((y - y0) as usize + 1) * bw + (x - x0) as usize + 1])
        .collect()
}

fn cut_of(pixels: &[(u32, u32)], w: u32, h: u32, grass: bool) -> LeafCut {
    let n = pixels.len() as f64;
    let (mut sx, mut sy) = (0.0, 0.0);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for &(x, y) in pixels {
        sx += f64::from(x);
        sy += f64::from(y);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let (cx, cy) = (sx / n, sy / n);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for &(x, y) in pixels {
        let (dx, dy) = (f64::from(x) - cx, f64::from(y) - cy);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u = (angle.cos(), angle.sin());
    let t = |&(x, y): &(u32, u32)| (f64::from(x) - cx) * u.0 + (f64::from(y) - cy) * u.1;
    let (tmin, tmax) = pixels
        .iter()
        .map(t)
        .fold((f64::MAX, f64::MIN), |a, v| (a.0.min(v), a.1.max(v)));
    let len = (tmax - tmin).max(1.0);
    let (wf, hf) = (f64::from(w), f64::from(h));
    let mut pivot = None;
    let mut petiole = false;
    let mut axis = (u.0, u.1);
    if grass {
        // the base: lower end of an upright blade, left end of a lying one
        let (lo, hi) = (tmin, tmax);
        let end_of = |e: f64| {
            let near: Vec<&(u32, u32)> = pixels
                .iter()
                .filter(|p| (t(p) - e).abs() <= 0.02 * len + 1.0)
                .collect();
            let k = near.len().max(1) as f64;
            let (px, py) = near.iter().fold((0.0, 0.0), |a, p| {
                (a.0 + f64::from(p.0), a.1 + f64::from(p.1))
            });
            (px / k, py / k)
        };
        let (a, b) = (end_of(lo), end_of(hi));
        let base_is_a = if u.0.abs() > u.1.abs() {
            a.0 < b.0
        } else {
            a.1 > b.1
        };
        let (p, q) = if base_is_a { (a, b) } else { (b, a) };
        pivot = Some(p);
        let l = ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt().max(1.0);
        axis = ((q.0 - p.0) / l, (q.1 - p.1) / l);
    } else {
        // petiole: the thin protrusion that sticks out furthest from the thick body
        let d = distance_inside(pixels, x0, y0, x1 - x0 + 1, y1 - y0 + 1);
        let maxd = *d.iter().max().unwrap_or(&1);
        let thin_below = (maxd as f64 * 0.22) as u32;
        let body: Vec<&(u32, u32)> = pixels
            .iter()
            .zip(&d)
            .filter(|(_, d)| **d >= thin_below)
            .map(|(p, _)| p)
            .collect();
        if !body.is_empty() {
            let bk = body.len() as f64;
            let (bx, by) = body.iter().fold((0.0, 0.0), |a, p| {
                (a.0 + f64::from(p.0) / bk, a.1 + f64::from(p.1) / bk)
            });
            let dist = |p: &(u32, u32)| {
                ((f64::from(p.0) - bx).powi(2) + (f64::from(p.1) - by).powi(2)).sqrt()
            };
            let rb = body.iter().map(|p| dist(p)).fold(0.0, f64::max);
            let thin: Vec<&(u32, u32)> = pixels
                .iter()
                .zip(&d)
                .filter(|(_, d)| **d < thin_below)
                .map(|(p, _)| p)
                .collect();
            let far = thin.iter().map(|p| dist(p)).fold(0.0, f64::max);
            if far > rb * 1.35 {
                let near: Vec<&&(u32, u32)> = thin
                    .iter()
                    .filter(|p| dist(p) >= far - 0.05 * far)
                    .collect();
                let k = near.len().max(1) as f64;
                let (px, py) = near.iter().fold((0.0, 0.0), |a, p| {
                    (a.0 + f64::from(p.0) / k, a.1 + f64::from(p.1) / k)
                });
                let l = ((bx - px).powi(2) + (by - py).powi(2)).sqrt().max(1.0);
                pivot = Some((px, py));
                petiole = true;
                axis = ((bx - px) / l, (by - py) / l);
            }
        }
        if pivot.is_none() {
            // no petiole: the thinner end of the leaf
            let band = 0.2 * len;
            let lo = pixels.iter().filter(|p| t(p) < tmin + band).count();
            let hi = pixels.iter().filter(|p| t(p) > tmax - band).count();
            let (edge, sign) = if lo < hi { (tmin, 1.0) } else { (tmax, -1.0) };
            let near: Vec<&(u32, u32)> = pixels
                .iter()
                .filter(|p| (t(p) - edge).abs() <= 0.02 * len + 1.0)
                .collect();
            let k = near.len().max(1) as f64;
            let (px, py) = near.iter().fold((0.0, 0.0), |a, p| {
                (a.0 + f64::from(p.0) / k, a.1 + f64::from(p.1) / k)
            });
            pivot = Some((px, py));
            axis = (u.0 * sign, u.1 * sign);
        }
    }
    petiole |= grass;
    let (px, py) = pivot.unwrap_or((cx, cy));
    // length: furthest extent of the piece from the pivot along the axis
    let reach = pixels
        .iter()
        .map(|&(x, y)| (f64::from(x) - px) * axis.0 + (f64::from(y) - py) * axis.1)
        .fold(1.0, f64::max);
    LeafCut {
        rect: [
            x0 as f32 / w as f32,
            y0 as f32 / h as f32,
            (x1 - x0 + 1) as f32 / w as f32,
            (y1 - y0 + 1) as f32 / h as f32,
        ],
        petiole,
        centre: [(cx / wf) as f32, (cy / hf) as f32],
        pivot: [(px / wf) as f32, (py / hf) as f32],
        axis: [axis.0 as f32, axis.1 as f32],
        length: (reach / wf) as f32,
        area: (n / (wf * hf)) as f32,
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("no parent")?)?;
    fs::write(path, bytes).with_context(|| path.display().to_string())
}

fn png_gray(img: &GrayImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    DynamicImage::ImageLuma8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)?;
    Ok(buf)
}

fn zip_entry(zip: &Path, suffix: &str) -> Result<Option<DynamicImage>> {
    let mut z =
        zip::ZipArchive::new(fs::File::open(zip).with_context(|| zip.display().to_string())?)?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        if f.name()?.ends_with(suffix) {
            let mut b = Vec::new();
            f.read_to_end(&mut b)?;
            return Ok(Some(image::load_from_memory(&b)?));
        }
    }
    Ok(None)
}

/// A leaf atlas: colour (opaque colour spread under the transparent area so mips do not bleed),
/// opacity (PNG), GL normal, and the source's scattering map as translucency when it has one.
/// With `tier`, only the files are written, into `<set>/<tier>/`.
pub fn ingest_leafset(
    c: &Candidate,
    spec: &LeafSetSpec,
    cache: &Path,
    out: &Path,
    tier: Option<&str>,
) -> Result<()> {
    let (id, sid) = (c.id(), c.source_id());
    let (px, attr) = match tier {
        Some("2k") => (2048, "2K-JPG"),
        _ => (1024, "1K-JPG"),
    };
    let src = c.dir(cache);
    let zip = src.join(format!("{sid}_{attr}.zip"));
    let color = zip_entry(&zip, "_Color.jpg")?.context("no colour map")?;
    let opacity = zip_entry(&zip, "_Opacity.jpg")?
        .context("no opacity map")?
        .to_luma8();
    let normal = zip_entry(&zip, "_NormalGL.jpg")?.context("no GL normal map")?;
    let scatter = zip_entry(&zip, "_Scattering.jpg")?;
    let (color, opacity) = (
        fit(&color, px),
        fit(&DynamicImage::ImageLuma8(opacity), px).to_luma8(),
    );
    let opacity = if opacity.dimensions() == (color.width(), color.height()) {
        opacity
    } else {
        image::imageops::resize(
            &opacity,
            color.width(),
            color.height(),
            FilterType::Triangle,
        )
    };
    let mut rgba: RgbaImage = color.to_rgba8();
    for (p, a) in rgba.pixels_mut().zip(opacity.pixels()) {
        p.0[3] = a.0[0];
    }
    let filled = DynamicImage::ImageRgba8(crate::scene::dilate(&rgba)).to_rgb8();
    let dir = out.join("flora/leaves").join(&id);
    let dir = tier.map_or(dir.clone(), |t| dir.join(t));
    write(
        &dir.join("color.jpg"),
        &jpeg(&DynamicImage::ImageRgb8(filled), 90)?,
    )?;
    write(&dir.join("opacity.png"), &png_gray(&opacity)?)?;
    write(&dir.join("normal.jpg"), &jpeg(&fit(&normal, px), 95)?)?;
    if let Some(s) = &scatter {
        write(&dir.join("translucency.jpg"), &jpeg(&fit(s, px), 85)?)?;
    }
    if tier.is_some() {
        println!("flora leaf tier {id}");
        return Ok(());
    }
    let info: Value = serde_json::from_slice(&fs::read(src.join("info.json"))?)?;
    let rel = |f: &str| format!("flora/leaves/{id}/{f}");
    let entry = LeafSetEntry {
        id: id.clone(),
        name: info["assets"][0]["title"].as_str().unwrap_or(sid).into(),
        license: "CC0-1.0".into(),
        author: c.author.clone(),
        source_url: c.page_url.clone(),
        genus: spec.genus.clone(),
        genus_from: spec.genus_from.clone(),
        shape: spec.shape.clone(),
        season: spec.season.clone(),
        notes: spec.notes.clone(),
        px: color.width().max(color.height()),
        maps: LeafMaps {
            color: rel("color.jpg"),
            opacity: rel("opacity.png"),
            normal: rel("normal.jpg"),
            translucency: scatter.map(|_| rel("translucency.jpg")),
        },
        leaves: cutouts(&opacity, sid.starts_with("Foliage")),
    };
    write(&dir.join("entry.json"), &serde_json::to_vec_pretty(&entry)?)?;
    println!("flora leaf {id}: {} leaves", entry.leaves.len());
    Ok(())
}

/// Bark: ambientCG materials go through the material maps; Poly Haven's JPEG set (diffuse,
/// GL normal, packed ARM = ORM, displacement) is copied as it is.
pub fn ingest_barkset(
    c: &Candidate,
    spec: &BarkSetSpec,
    cache: &Path,
    out: &Path,
    tier: Option<&str>,
) -> Result<()> {
    let (id, sid) = (c.id(), c.source_id());
    let src = c.dir(cache);
    let dir = out.join("flora/bark").join(&id);
    let dir = tier.map_or(dir.clone(), |t| dir.join(t));
    let res = tier.unwrap_or("1k");
    let (name, author, size_m, px);
    if spec.source == "Poly Haven" {
        let base = if tier.is_some() {
            src.join(res)
        } else {
            src.clone()
        };
        let find = |key: &str| -> Result<Vec<u8>> {
            let suffix = format!("_{key}_{res}.jpg");
            let p = fs::read_dir(&base)?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .find(|p| p.to_string_lossy().ends_with(&suffix))
                .with_context(|| format!("no {suffix} in the cache (run fetch)"))?;
            Ok(fs::read(p)?)
        };
        let colour = find("diff")?;
        let img = image::load_from_memory(&colour)?;
        px = img.width().max(img.height());
        write(&dir.join("basecolor.jpg"), &colour)?;
        write(&dir.join("normal.jpg"), &find("nor_gl")?)?;
        write(&dir.join("orm.jpg"), &find("arm")?)?;
        write(&dir.join("height.jpg"), &find("disp")?)?;
        let info = ph_info(&src)?;
        let raw: Value = serde_json::from_slice(&fs::read(src.join("info.json"))?)?;
        let dims = raw["dimensions"]
            .as_array()
            .map(|d| [d[0].as_f64().unwrap_or(0.0), d[1].as_f64().unwrap_or(0.0)]);
        size_m = dims
            .filter(|d| d[0] > 0.0 && d[1] > 0.0)
            .map(|d| [(d[0] / 1000.0) as f32, (d[1] / 1000.0) as f32]);
        (name, author) = (info.name, info.author);
    } else {
        let side = if tier.is_some() { 2048 } else { 1024 };
        let attr = if tier.is_some() { "2K-JPG" } else { "1K-JPG" };
        let MaterialMaps3 {
            color,
            normal,
            orm,
            height,
        } = material_maps(&src.join(format!("{sid}_{attr}.zip")), side)?;
        px = color.width().max(color.height());
        write(&dir.join("basecolor.jpg"), &jpeg(&color, 90)?)?;
        write(&dir.join("normal.jpg"), &jpeg(&normal, 95)?)?;
        write(
            &dir.join("orm.jpg"),
            &jpeg(&DynamicImage::ImageRgb8(orm), 90)?,
        )?;
        if let Some(h) = height {
            write(
                &dir.join("height.jpg"),
                &jpeg(&DynamicImage::ImageLuma8(h), 90)?,
            )?;
        }
        let info: Value = serde_json::from_slice(&fs::read(src.join("info.json"))?)?;
        let a = &info["assets"][0];
        let d = &a["dimensions"];
        let (dw, dh) = (
            d["width"].as_f64().unwrap_or(0.0),
            d["height"].as_f64().unwrap_or(0.0),
        );
        size_m = (dw > 0.0 && dh > 0.0).then(|| [(dw / 100.0) as f32, (dh / 100.0) as f32]);
        name = a["title"].as_str().unwrap_or(sid).to_string();
        author = c.author.clone();
    }
    if tier.is_some() {
        println!("flora bark tier {id}");
        return Ok(());
    }
    let rel = |f: &str| format!("flora/bark/{id}/{f}");
    let entry = BarkSetEntry {
        id: id.clone(),
        name,
        license: "CC0-1.0".into(),
        author,
        source_url: c.page_url.clone(),
        genus: spec.genus.clone(),
        genus_from: spec.genus_from.clone(),
        pattern: spec.pattern.clone(),
        notes: spec.notes.clone(),
        size_m,
        px,
        maps: BarkMaps {
            basecolor: rel("basecolor.jpg"),
            normal: rel("normal.jpg"),
            orm: rel("orm.jpg"),
            height: dir.join("height.jpg").exists().then(|| rel("height.jpg")),
        },
    };
    write(&dir.join("entry.json"), &serde_json::to_vec_pretty(&entry)?)?;
    println!("flora bark {id}");
    Ok(())
}

/// Ingest every set of the spec (base or a tier), and write the species table next to them.
pub fn ingest_all(spec: &FloraSpec, cache: &Path, out: &Path, tier: Option<&str>) -> Result<()> {
    let cands = spec.candidates();
    let mut failed = Vec::new();
    for c in &cands {
        let r = if c.category == "leafset" {
            let s = spec
                .leaf_sets
                .iter()
                .find(|s| s.id == c.source_id())
                .context("spec")?;
            ingest_leafset(c, s, cache, out, tier)
        } else {
            let s = spec
                .bark_sets
                .iter()
                .find(|s| s.id == c.source_id())
                .context("spec")?;
            ingest_barkset(c, s, cache, out, tier)
        };
        if let Err(e) = r {
            println!("FAILED {}: {e:#}", c.source_id());
            failed.push(c.source_id().to_string());
        }
    }
    let species: BTreeMap<String, SpeciesRef> = spec
        .species
        .iter()
        .map(|(k, v)| {
            let lower =
                |p: &Option<[String; 2]>| p.as_ref().map(|[a, b]| [a.to_lowercase(), b.clone()]);
            (
                k.clone(),
                SpeciesRef {
                    leaf: lower(&v.leaf),
                    leaf_autumn: v.leaf_autumn.as_ref().map(|s| s.to_lowercase()),
                    bark: lower(&v.bark),
                },
            )
        })
        .collect();
    write(
        &out.join("flora/species.json"),
        &serde_json::to_vec_pretty(&species)?,
    )?;
    if failed.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("failed: {}", failed.join(", "))
    }
}
