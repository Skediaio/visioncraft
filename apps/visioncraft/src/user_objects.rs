//! Objects a person imports (`.glb`): checked here, their bytes held by the asset store
//! (`AssetStore::provide`), recorded in the project with `object.add`, and shown to the rest of the
//! app as catalogue assets `user:<id>` with one level of detail, so every planting tool places
//! them and the library streams them like its own models.

use crate::gltf_load;
use crate::library::Library;
use glam::Vec3;
use std::sync::Arc;
use visioncraft_core::UserObject;
use visioncraft_core::catalog::{Asset, Category, Lod, Origin};

/// A `.glb` larger than this is refused: it would not stream or draw well on the web.
pub const MAX_GLB_BYTES: usize = 64 * 1024 * 1024;

/// What an imported file gives: its size in metres (x, y, z of its bounds) and a picture for the
/// browser (its first embedded image), if any.
#[derive(Debug)]
pub struct Checked {
    pub size_m: [f32; 3],
    pub thumbnail: Option<Vec<u8>>,
}

/// Reads an untrusted `.glb`: a message for anything wrong, never a panic.
pub fn check_glb(bytes: &[u8]) -> Result<Checked, String> {
    if bytes.len() > MAX_GLB_BYTES {
        return Err(format!(
            "the file is {} MB; at most {} MB can be imported",
            bytes.len() / (1024 * 1024),
            MAX_GLB_BYTES / (1024 * 1024)
        ));
    }
    let glb = gltf_load::parse(bytes)?;
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for p in glb.prims.iter().flat_map(|p| &p.positions) {
        let v = Vec3::from(*p);
        if v.is_finite() {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return Err("the file has no triangles to show".into());
    }
    let size = (hi - lo).max(Vec3::splat(0.01));
    if size.max_element() > 500.0 {
        return Err(format!(
            "the object is {:.0} m across: is it in metres? Objects up to 500 m are imported",
            size.max_element()
        ));
    }
    let thumbnail = match glb.images.first() {
        Some(img) => Some(img.clone()),
        None => render_thumbnail(&glb, lo, hi),
    };
    Ok(Checked {
        size_m: size.to_array(),
        thumbnail,
    })
}

/// Side of a rendered thumbnail, pixels.
const THUMB_PX: usize = 128;

/// A picture of a model without images: its triangles seen from the front left and a little
/// above, shaded by one light, in their materials' base colours (a small software rasteriser,
/// so the GPU and the scene are not involved). PNG bytes.
pub fn render_thumbnail(glb: &gltf_load::Glb, lo: Vec3, hi: Vec3) -> Option<Vec<u8>> {
    let n = THUMB_PX;
    let centre = (lo + hi) * 0.5;
    let rot = glam::Mat3::from_rotation_x(0.45) * glam::Mat3::from_rotation_y(-0.6);
    let radius = ((hi - lo).length() * 0.5).max(1e-3);
    let to_px = |v: Vec3| {
        let q = rot * (v - centre) / radius;
        // x right, y up, z towards the viewer
        Vec3::new(
            (q.x * 0.48 + 0.5) * n as f32,
            (0.5 - q.y * 0.48) * n as f32,
            q.z,
        )
    };
    let light = Vec3::new(-0.4, 0.8, 0.45).normalize();
    let mut depth = vec![f32::NEG_INFINITY; n * n];
    let mut px = vec![0u8; n * n * 4];
    for p in &glb.prims {
        let base = glb
            .materials
            .get(p.material)
            .map_or([0.8; 4], |m| m.base_color);
        for t in p.indices.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (
                p.positions.get(t[0] as usize),
                p.positions.get(t[1] as usize),
                p.positions.get(t[2] as usize),
            ) else {
                continue;
            };
            let (a, b, c) = (Vec3::from(*a), Vec3::from(*b), Vec3::from(*c));
            let normal = (rot * (b - a).cross(c - a)).normalize_or_zero();
            let shade = 0.35 + 0.65 * normal.dot(rot * light).abs();
            let (pa, pb, pc) = (to_px(a), to_px(b), to_px(c));
            let area = (pb.x - pa.x) * (pc.y - pa.y) - (pb.y - pa.y) * (pc.x - pa.x);
            if area.abs() < 1e-6 {
                continue;
            }
            let x0 = pa.x.min(pb.x).min(pc.x).floor().max(0.0) as usize;
            let x1 = (pa.x.max(pb.x).max(pc.x).ceil() as usize).min(n - 1);
            let y0 = pa.y.min(pb.y).min(pc.y).floor().max(0.0) as usize;
            let y1 = (pa.y.max(pb.y).max(pc.y).ceil() as usize).min(n - 1);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((pb.x - fx) * (pc.y - fy) - (pb.y - fy) * (pc.x - fx)) / area;
                    let w1 = ((pc.x - fx) * (pa.y - fy) - (pc.y - fy) * (pa.x - fx)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * pa.z + w1 * pb.z + w2 * pc.z;
                    let i = y * n + x;
                    if z <= depth[i] {
                        continue;
                    }
                    depth[i] = z;
                    // linear colour to sRGB-ish bytes
                    let c = |v: f32| ((v * shade).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
                    px[i * 4..i * 4 + 4].copy_from_slice(&[
                        c(base[0]),
                        c(base[1]),
                        c(base[2]),
                        255,
                    ]);
                }
            }
        }
    }
    crate::studio::png_bytes(&px, n as u32, n as u32).ok()
}

/// Where an imported file's picture is held.
pub fn thumbnail_path(file: &str) -> String {
    format!("{file}.thumb")
}

/// A file name for a new object that no object of `taken` uses.
pub fn file_for(stem: &str, taken: &[UserObject]) -> String {
    let stem = crate::files::file_stem(stem, "object");
    let mut n = 1;
    loop {
        let f = if n == 1 {
            format!("objects/{stem}.glb")
        } else {
            format!("objects/{stem}-{n}.glb")
        };
        if !taken.iter().any(|o| o.file == f) {
            return f;
        }
        n += 1;
    }
}

/// The catalogue entry of an object.
pub fn asset_for(o: &UserObject, has_thumbnail: bool) -> Asset {
    Asset {
        id: o.asset_id(),
        name: o.name.clone(),
        category: Category::Other,
        tags: vec!["imported".into()],
        license: String::new(),
        origin: Origin::User,
        author: String::new(),
        source_url: String::new(),
        size_m: o.size_m,
        lods: vec![Lod {
            file: o.file.clone(),
            max_distance_m: f32::INFINITY,
        }],
        thumbnail: if has_thumbnail {
            thumbnail_path(&o.file)
        } else {
            String::new()
        },
        plant: None,
        light: None,
    }
}

impl Library {
    /// Brings the catalogue's imported objects in line with the project's.
    pub fn set_user_objects(&mut self, objects: &[UserObject]) {
        let assets: Vec<Asset> = objects
            .iter()
            .map(|o| {
                let thumb = self.store.provided(&thumbnail_path(&o.file)).is_some();
                asset_for(o, thumb)
            })
            .collect();
        if assets == self.user_assets {
            return;
        }
        self.user_assets = assets;
        self.merge_user_assets();
    }

    /// Puts the imported objects into the catalogue (also after `catalog.json` arrives).
    pub(crate) fn merge_user_assets(&mut self) {
        let mut cat = self.catalog.as_deref().cloned().unwrap_or_default();
        cat.assets.retain(|a| a.origin != Origin::User);
        cat.assets.extend(self.user_assets.iter().cloned());
        self.catalog = Some(Arc::new(cat));
        self.catalog_arrived = true;
        self.rev += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_and_huge_files_get_a_message() {
        assert!(check_glb(b"not a glb").is_err());
        assert!(check_glb(&[]).is_err());
        // a GLB header that promises more than it has
        let mut lie = b"glTF".to_vec();
        lie.extend_from_slice(&2u32.to_le_bytes());
        lie.extend_from_slice(&1_000_000u32.to_le_bytes());
        assert!(check_glb(&lie).is_err());
        let big = vec![0u8; MAX_GLB_BYTES + 1];
        assert!(check_glb(&big).unwrap_err().contains("MB"));
    }

    /// A one-triangle GLB, 2 m wide and 1 m tall.
    fn triangle_glb() -> Vec<u8> {
        let pos: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let bin: Vec<u8> = pos.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[2,1,0]}}],"bufferViews":[{{"buffer":0,"byteLength":{n}}}],"buffers":[{{"byteLength":{n}}}]}}"#,
            n = bin.len()
        );
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = b"glTF".to_vec();
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    #[test]
    fn a_glb_is_measured() {
        let c = check_glb(&triangle_glb()).unwrap();
        assert_eq!(c.size_m[0], 2.0);
        assert_eq!(c.size_m[1], 1.0);
        // no image in the file: a rendered picture, with the triangle in it
        let png = c.thumbnail.expect("a rendered thumbnail");
        let mut d = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut buf = vec![0; d.output_buffer_size()];
        d.next_frame(&mut buf).unwrap();
        let opaque = buf.chunks(4).filter(|p| p[3] == 255).count();
        assert!(opaque > 500, "{opaque} pixels drawn");
        // and every truncation of it is refused, not a panic
        let g = triangle_glb();
        for n in 0..g.len() {
            assert!(check_glb(&g[..n]).is_err(), "{n}");
        }
    }

    #[test]
    fn file_names_do_not_collide() {
        let o = |f: &str| UserObject {
            id: "uo1".into(),
            name: "x".into(),
            file: f.into(),
            size_m: [1.0; 3],
        };
        assert_eq!(file_for("Garden shed", &[]), "objects/Garden-shed.glb");
        assert_eq!(
            file_for("Garden shed", &[o("objects/Garden-shed.glb")]),
            "objects/Garden-shed-2.glb"
        );
    }
}
