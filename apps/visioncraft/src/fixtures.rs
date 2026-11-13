//! Garden lights the catalogue lacks, made here from primitives: a bollard (0.6 m) and a low path
//! light (0.3 m), each with a warm light at its glass. They are catalogue assets like any other
//! (their GLB bytes are held by the asset store), so every planting tool places them and the
//! lamp reconcile gives them their light.

use crate::gltf_load;
use serde_json::json;
use std::f32::consts::TAU;
use visioncraft_core::catalog::{Asset, Category, Lod, Origin};
use visioncraft_core::{LightKind, LightSpec};

/// Ids of the built-in fixtures, listed first among the garden lamps.
pub const PREFIX: &str = "vc-";

struct Fixture {
    id: &'static str,
    name: &'static str,
    /// Post radius and height, then the glass band's bottom and top (metres).
    post: (f32, f32),
    glass: (f32, f32),
    /// A cap above the glass (a path light's mushroom), radius and thickness; 0 for none.
    cap: (f32, f32),
    lumens: f32,
    range_m: f32,
}

const FIXTURES: [Fixture; 2] = [
    Fixture {
        id: "vc-bollard",
        name: "Garden bollard",
        post: (0.08, 0.6),
        glass: (0.45, 0.55),
        cap: (0.0, 0.0),
        lumens: 250.0,
        range_m: 5.0,
    },
    Fixture {
        id: "vc-path-light",
        name: "Path light",
        post: (0.025, 0.24),
        glass: (0.2, 0.26),
        cap: (0.11, 0.03),
        lumens: 120.0,
        range_m: 3.0,
    },
];

pub fn glb_path(id: &str) -> String {
    format!("builtin/{id}.glb")
}

/// The catalogue entries.
pub fn assets() -> Vec<Asset> {
    FIXTURES
        .iter()
        .map(|f| {
            let top = f.post.1.max(f.glass.1 + f.cap.1);
            let width = 2.0 * f.post.0.max(f.cap.0);
            Asset {
                id: f.id.into(),
                name: f.name.into(),
                category: Category::Lighting,
                tags: vec!["garden".into(), "path".into(), "exterior".into()],
                license: "CC0-1.0".into(),
                origin: Origin::BuiltIn,
                author: "VisionCraft".into(),
                source_url: String::new(),
                size_m: [width, top, width],
                lods: vec![Lod {
                    file: glb_path(f.id),
                    max_distance_m: f32::INFINITY,
                }],
                thumbnail: format!("{}.thumb", glb_path(f.id)),
                plant: None,
                light: Some(LightSpec {
                    kind: LightKind::Point,
                    color_temperature_k: 2700.0,
                    lumens: f.lumens,
                    range_m: f.range_m,
                    offset_m: [0.0, (f.glass.0 + f.glass.1) / 2.0, 0.0],
                }),
            }
        })
        .collect()
}

/// (store path, bytes) of every fixture's model and thumbnail.
pub fn files() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for f in &FIXTURES {
        let bytes = glb(f);
        let path = glb_path(f.id);
        if let Ok(g) = gltf_load::parse(&bytes) {
            let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
            for p in g.prims.iter().flat_map(|p| &p.positions) {
                lo = lo.min(glam::Vec3::from(*p));
                hi = hi.max(glam::Vec3::from(*p));
            }
            if let Some(png) = crate::user_objects::render_thumbnail(&g, lo, hi) {
                out.push((format!("{path}.thumb"), png));
            }
        }
        out.push((path, bytes));
    }
    out
}

/// A closed cylinder from `y0` to `y1` of radius `r`: (positions, normals, indices).
fn cylinder(r: f32, y0: f32, y1: f32, out: &mut (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>)) {
    const N: usize = 16;
    let (pos, nor, idx) = out;
    let ring = |y: f32, pos: &mut Vec<[f32; 3]>, nor: &mut Vec<[f32; 3]>, up: Option<f32>| {
        let base = pos.len() as u32;
        for k in 0..N {
            let a = TAU * k as f32 / N as f32;
            pos.push([r * a.cos(), y, r * a.sin()]);
            nor.push(match up {
                Some(s) => [0.0, s, 0.0],
                None => [a.cos(), 0.0, a.sin()],
            });
        }
        base
    };
    // Side.
    let (b0, b1) = (ring(y0, pos, nor, None), ring(y1, pos, nor, None));
    for k in 0..N as u32 {
        let k1 = (k + 1) % N as u32;
        idx.extend([b0 + k, b1 + k, b0 + k1, b0 + k1, b1 + k, b1 + k1]);
    }
    // Caps (fans).
    for (y, s) in [(y1, 1.0), (y0, -1.0)] {
        let centre = pos.len() as u32;
        pos.push([0.0, y, 0.0]);
        nor.push([0.0, s, 0.0]);
        let b = ring(y, pos, nor, Some(s));
        for k in 0..N as u32 {
            let k1 = (k + 1) % N as u32;
            if s > 0.0 {
                idx.extend([centre, b + k1, b + k]);
            } else {
                idx.extend([centre, b + k, b + k1]);
            }
        }
    }
}

/// A two-material GLB: the dark metal body and the warm glass that glows (`lamp_emitter`).
fn glb(f: &Fixture) -> Vec<u8> {
    let mut body = (Vec::new(), Vec::new(), Vec::new());
    let mut glass = (Vec::new(), Vec::new(), Vec::new());
    let (r, h) = f.post;
    cylinder(r, 0.0, f.glass.0, &mut body);
    cylinder(r * 1.05, f.glass.0, f.glass.1, &mut glass);
    if f.cap.0 > 0.0 {
        cylinder(f.cap.0, f.glass.1, f.glass.1 + f.cap.1, &mut body);
    } else {
        cylinder(r, f.glass.1, h, &mut body);
    }
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut prims = Vec::new();
    for (m, (pos, nor, idx)) in [body, glass].iter().enumerate() {
        let mut view = |bytes: Vec<u8>, target: u32| {
            let offset = bin.len();
            bin.extend(&bytes);
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len(), "target": target}));
            views.len() - 1
        };
        let floats = |v: &[[f32; 3]]| v.iter().flatten().flat_map(|c| c.to_le_bytes()).collect();
        let (lo, hi) = pos
            .iter()
            .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])],
                    [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])],
                )
            });
        let vp = view(floats(pos), 34962);
        let vn = view(floats(nor), 34962);
        let vi = view(idx.iter().flat_map(|i| i.to_le_bytes()).collect(), 34963);
        let a = accessors.len();
        accessors.push(json!({"bufferView": vp, "componentType": 5126, "count": pos.len(), "type": "VEC3", "min": lo, "max": hi}));
        accessors.push(
            json!({"bufferView": vn, "componentType": 5126, "count": nor.len(), "type": "VEC3"}),
        );
        accessors.push(
            json!({"bufferView": vi, "componentType": 5125, "count": idx.len(), "type": "SCALAR"}),
        );
        prims.push(json!({"attributes": {"POSITION": a, "NORMAL": a + 1}, "indices": a + 2, "material": m}));
    }
    let json = json!({
        "asset": {"version": "2.0", "generator": "VisionCraft fixtures"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": f.name}],
        "meshes": [{"primitives": prims}],
        "materials": [
            {"name": "body", "pbrMetallicRoughness": {"baseColorFactor": [0.06, 0.06, 0.065, 1.0], "metallicFactor": 0.6, "roughnessFactor": 0.45}},
            {"name": "lamp_emitter", "pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.9, 0.75, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.3},
             "emissiveFactor": [1.0, 0.72, 0.42],
             "extensions": {"KHR_materials_emissive_strength": {"emissiveStrength": 6.0}}}
        ],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}],
        "extensionsUsed": ["KHR_materials_emissive_strength"]
    });
    let mut j = serde_json::to_vec(&json).expect("json");
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let total = 12 + 8 + j.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((j.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(&j);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(&bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_load_with_a_glowing_glass_and_a_light_at_its_height() {
        for (f, a) in FIXTURES.iter().zip(assets()) {
            let g = gltf_load::parse(&glb(f)).expect("loads");
            assert_eq!(g.prims.len(), 2);
            assert!(g.materials.iter().any(|m| m.emitter));
            let top = g
                .prims
                .iter()
                .flat_map(|p| &p.positions)
                .map(|p| p[1])
                .fold(0.0, f32::max);
            assert!((top - a.size_m[1]).abs() < 1e-4, "{top} vs {:?}", a.size_m);
            let light = a.light.unwrap();
            assert!(light.offset_m[1] > 0.1 && light.offset_m[1] < top);
        }
        assert!(files().iter().any(|(p, _)| p.ends_with(".thumb")));
    }
}
