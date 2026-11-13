//! Self-contained binary glTF writer and the image encoders it uses.

use crate::scene::{Alpha, Model};
use anyhow::Result;
use image::{DynamicImage, ImageFormat, codecs::jpeg::JpegEncoder, imageops::FilterType};
use serde_json::{Value, json};
use std::{collections::HashMap, io::Cursor, sync::Arc};

/// Downscale (never upscale) so the long side is at most `max` px.
pub fn fit(img: &DynamicImage, max: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let long = w.max(h);
    if long <= max {
        return img.clone();
    }
    let s = max as f32 / long as f32;
    img.resize_exact(
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
        FilterType::Lanczos3,
    )
}

pub fn jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let img = if matches!(img, DynamicImage::ImageLuma8(_)) {
        img.clone()
    } else {
        DynamicImage::ImageRgb8(img.to_rgb8())
    };
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality).encode_image(&img)?;
    Ok(buf)
}

pub fn png(img: &DynamicImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)?;
    Ok(buf)
}

fn has_alpha(img: &DynamicImage) -> bool {
    img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] < 255)
}

#[derive(Default)]
struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Bin {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.data.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = t.into();
        }
        self.data.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        bytes: &[u8],
        target: u32,
        component: u32,
        count: usize,
        kind: &str,
        minmax: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let view = self.view(bytes, Some(target));
        let mut a =
            json!({"bufferView": view, "componentType": component, "count": count, "type": kind});
        if let Some((lo, hi)) = minmax {
            a["min"] = json!(lo);
            a["max"] = json!(hi);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }
}

fn f32_bytes(it: impl Iterator<Item = f32>) -> Vec<u8> {
    it.flat_map(f32::to_le_bytes).collect()
}

/// Write `model` as one GLB: one node, one mesh with a primitive per material, textures
/// embedded and downscaled to `max_px`. Colour is JPEG q85 (PNG when it has real alpha, so
/// cut-out leaves keep their edges), ORM is JPEG q85, normals are JPEG q95: a 1024 px normal
/// map as PNG is about 2 MB, which with several materials would break the 8 MB file budget,
/// and q95 keeps the vector error well below what a shader's texture filtering adds.
pub fn write_glb(model: &Model, name: &str, max_px: u32) -> Result<Vec<u8>> {
    let mut bin = Bin::default();
    let mut images: Vec<Value> = Vec::new();
    let mut textures: Vec<Value> = Vec::new();
    // (image pointer, role) -> texture index, so a shared image is embedded once.
    let mut tex_ids: HashMap<(usize, u8), usize> = HashMap::new();
    let mut materials = Vec::new();
    let mut emissive_used = false;
    // Only materials a mesh uses are written.
    let mut used: Vec<usize> = model.meshes.iter().map(|m| m.mat).collect();
    used.sort_unstable();
    used.dedup();
    for &mi in &used {
        let m = &model.mats[mi];
        let mut tex = |img: &Option<Arc<DynamicImage>>, role: u8| -> Result<Option<usize>> {
            let Some(img) = img else { return Ok(None) };
            let key = (Arc::as_ptr(img) as usize, role);
            if let Some(&t) = tex_ids.get(&key) {
                return Ok(Some(t));
            }
            // alpha-tested colour is PNG, which is big: half the size keeps the library small
            let cap = if role == 0 && has_alpha(img) {
                max_px / 2
            } else {
                max_px
            };
            let small = fit(img, cap);
            let (bytes, mime) = match role {
                0 if has_alpha(&small) => (
                    png(&DynamicImage::ImageRgba8(small.to_rgba8()))?,
                    "image/png",
                ),
                2 => (jpeg(&small, 95)?, "image/jpeg"),
                _ => (jpeg(&small, 85)?, "image/jpeg"),
            };
            let view = bin.view(&bytes, None);
            images.push(json!({"bufferView": view, "mimeType": mime}));
            textures.push(json!({"source": images.len() - 1, "sampler": 0}));
            tex_ids.insert(key, textures.len() - 1);
            Ok(Some(textures.len() - 1))
        };
        let (base, orm, normal) = (tex(&m.base, 0)?, tex(&m.orm, 1)?, tex(&m.normal, 2)?);
        let mut pbr = json!({"baseColorFactor": m.base_factor, "metallicFactor": m.metallic, "roughnessFactor": m.roughness});
        if let Some(t) = base {
            pbr["baseColorTexture"] = json!({"index": t});
        }
        if let Some(t) = orm {
            pbr["metallicRoughnessTexture"] = json!({"index": t});
        }
        let mut mat =
            json!({"name": m.name, "pbrMetallicRoughness": pbr, "doubleSided": m.double_sided});
        if m.emissive.iter().any(|&e| e > 0.0) {
            mat["emissiveFactor"] = json!(m.emissive);
            mat["extensions"] = json!({"KHR_materials_emissive_strength": {"emissiveStrength": m.emissive_strength}});
            emissive_used = true;
        }
        if let Some(t) = normal {
            mat["normalTexture"] = json!({"index": t});
        }
        if let Some(t) = orm {
            mat["occlusionTexture"] = json!({"index": t});
        }
        match m.alpha {
            Alpha::Opaque => {}
            Alpha::Mask => {
                mat["alphaMode"] = "MASK".into();
                mat["alphaCutoff"] = m.cutoff.into();
            }
            Alpha::Blend => mat["alphaMode"] = "BLEND".into(),
        }
        materials.push(mat);
    }
    let mut prims = Vec::new();
    for mesh in &model.meshes {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &mesh.pos {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let n = mesh.pos.len();
        let pos = bin.accessor(
            &f32_bytes(mesh.pos.iter().flatten().copied()),
            34962,
            5126,
            n,
            "VEC3",
            Some((lo, hi)),
        );
        let nrm = bin.accessor(
            &f32_bytes(mesh.nrm.iter().flatten().copied()),
            34962,
            5126,
            n,
            "VEC3",
            None,
        );
        let uv = bin.accessor(
            &f32_bytes(mesh.uv.iter().flatten().copied()),
            34962,
            5126,
            n,
            "VEC2",
            None,
        );
        let (idx_bytes, comp): (Vec<u8>, u32) = if n <= 65535 {
            (
                mesh.idx
                    .iter()
                    .flat_map(|&i| (i as u16).to_le_bytes())
                    .collect(),
                5123,
            )
        } else {
            (
                mesh.idx.iter().flat_map(|i| i.to_le_bytes()).collect(),
                5125,
            )
        };
        let idx = bin.accessor(&idx_bytes, 34963, comp, mesh.idx.len(), "SCALAR", None);
        prims.push(json!({"attributes": {"POSITION": pos, "NORMAL": nrm, "TEXCOORD_0": uv}, "indices": idx, "material": used.binary_search(&mesh.mat).unwrap_or(0), "mode": 4}));
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "visioncraft-assets"},
        "scene": 0, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0, "name": name}],
        "meshes": [{"name": name, "primitives": prims}],
        "materials": materials,
        "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}],
        "buffers": [{"byteLength": bin.data.len()}],
        "bufferViews": bin.views, "accessors": bin.accessors,
    });
    if emissive_used {
        doc["extensionsUsed"] = json!(["KHR_materials_emissive_strength"]);
    }
    if !images.is_empty() {
        doc["images"] = images.into();
        doc["textures"] = textures.into();
    }
    let mut json_bytes = serde_json::to_vec(&doc)?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    while !bin.data.len().is_multiple_of(4) {
        bin.data.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.data.len();
    let mut out = Vec::with_capacity(total);
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((json_bytes.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(json_bytes);
    out.extend((bin.data.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(bin.data);
    Ok(out)
}
