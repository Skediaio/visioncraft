//! Reads a library GLB (self-contained glTF 2.0, metres, Y up) into plain CPU data: one merged
//! triangle list per material with the node transforms applied, the materials with the roles of
//! their images, and the still-encoded images. Decoding the images and everything on the GPU is
//! somebody else's job (`assets`, `library`).

use glam::{Mat3, Mat4, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Alpha {
    Opaque,
    Mask(f32),
    Blend,
}

#[derive(Clone, Debug)]
pub struct GlbMaterial {
    pub name: String,
    /// Linear RGBA.
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// Indices into [`Glb::images`].
    pub base_image: Option<usize>,
    pub normal_image: Option<usize>,
    pub orm_image: Option<usize>,
    pub emissive: [f32; 3],
    /// `KHR_materials_emissive_strength` (1 when absent).
    pub emissive_strength: f32,
    pub alpha: Alpha,
    pub double_sided: bool,
    /// Named like glass or using `KHR_materials_transmission`.
    pub glass: bool,
    /// A light fixture's emitting part: its name ends in `_emitter`.
    pub emitter: bool,
}

impl Default for GlbMaterial {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.7,
            base_image: None,
            normal_image: None,
            orm_image: None,
            emissive: [0.0; 3],
            emissive_strength: 1.0,
            alpha: Alpha::Opaque,
            double_sided: false,
            glass: false,
            emitter: false,
        }
    }
}

/// All triangles of one material.
#[derive(Clone, Debug, Default)]
pub struct Prim {
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Glb {
    pub prims: Vec<Prim>,
    pub materials: Vec<GlbMaterial>,
    /// Encoded (JPEG or PNG) bytes per glTF image.
    pub images: Vec<Vec<u8>>,
}

fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut n = vec![Vec3::ZERO; positions.len()];
    for t in indices.as_chunks::<3>().0 {
        let p = |i: u32| Vec3::from(positions[i as usize]);
        let f = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        for &i in t.iter() {
            n[i as usize] += f;
        }
    }
    n.into_iter()
        .map(|v| v.normalize_or(Vec3::Y).to_array())
        .collect()
}

pub fn parse(bytes: &[u8]) -> Result<Glb, String> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| format!("not a GLB: {e}"))?;
    let doc = &gltf.document;
    let blob = gltf.blob.as_deref();
    let data = |b: gltf::Buffer| match b.source() {
        gltf::buffer::Source::Bin => blob,
        gltf::buffer::Source::Uri(_) => None,
    };

    let mut out = Glb::default();
    for img in doc.images() {
        let gltf::image::Source::View { view, .. } = img.source() else {
            return Err("an image is not embedded".into());
        };
        let buf = data(view.buffer()).ok_or("an image has no data")?;
        let bytes = buf
            .get(view.offset()..view.offset() + view.length())
            .ok_or("an image lies outside the file")?;
        out.images.push(bytes.to_vec());
    }
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        let name = m.name().unwrap_or_default().to_string();
        let lower = name.to_lowercase();
        let image = |t: gltf::texture::Texture| t.source().index();
        out.materials.push(GlbMaterial {
            base_color: pbr.base_color_factor(),
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            base_image: pbr.base_color_texture().map(|i| image(i.texture())),
            normal_image: m.normal_texture().map(|i| image(i.texture())),
            orm_image: pbr.metallic_roughness_texture().map(|i| image(i.texture())),
            emissive: m.emissive_factor(),
            emissive_strength: m.emissive_strength().unwrap_or(1.0),
            alpha: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => Alpha::Opaque,
                gltf::material::AlphaMode::Mask => Alpha::Mask(m.alpha_cutoff().unwrap_or(0.5)),
                gltf::material::AlphaMode::Blend => Alpha::Blend,
            },
            double_sided: m.double_sided(),
            glass: lower.contains("glass") || m.transmission().is_some(),
            emitter: lower.ends_with("_emitter"),
            name,
        });
    }
    // A primitive without a material uses this one.
    let default_material = out.materials.len();
    out.materials.push(GlbMaterial::default());

    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .ok_or("the file has no scene")?;
    let mut stack: Vec<(gltf::Node, Mat4)> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let xf = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        stack.extend(node.children().map(|c| (c, xf)));
        let Some(mesh) = node.mesh() else { continue };
        let normal_xf = Mat3::from_mat4(xf).inverse().transpose();
        let flip = xf.determinant() < 0.0;
        for p in mesh.primitives() {
            if p.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let r = p.reader(data);
            let Some(pos) = r.read_positions() else {
                continue;
            };
            let pos: Vec<[f32; 3]> = pos.collect();
            let mut indices: Vec<u32> = match r.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            if indices.iter().any(|&i| i as usize >= pos.len()) {
                return Err("an index points past the vertices".into());
            }
            let normals: Vec<[f32; 3]> = match r.read_normals() {
                Some(n) => n.collect(),
                None => smooth_normals(&pos, &indices),
            };
            let uvs: Vec<[f32; 2]> = match r.read_tex_coords(0) {
                Some(t) => t.into_f32().collect(),
                None => vec![[0.0; 2]; pos.len()],
            };
            if normals.len() != pos.len() || uvs.len() != pos.len() {
                return Err("vertex attributes differ in length".into());
            }
            let material = p.material().index().unwrap_or(default_material);
            let at = match out.prims.iter().position(|q| q.material == material) {
                Some(i) => i,
                None => {
                    out.prims.push(Prim {
                        material,
                        ..Default::default()
                    });
                    out.prims.len() - 1
                }
            };
            let dst = &mut out.prims[at];
            let base = dst.positions.len() as u32;
            dst.positions.extend(
                pos.iter()
                    .map(|&q| xf.transform_point3(Vec3::from(q)).to_array()),
            );
            dst.normals.extend(
                normals
                    .iter()
                    .map(|&n| (normal_xf * Vec3::from(n)).normalize_or(Vec3::Y).to_array()),
            );
            dst.uvs.extend(uvs);
            if flip {
                for t in indices.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
            }
            dst.indices.extend(indices.iter().map(|i| i + base));
        }
    }
    out.prims.retain(|p| !p.indices.is_empty());
    if out.prims.is_empty() {
        return Err("the file has no triangles".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A GLB with one triangle under a node that moves it up by 2 m, with an emitter material.
    fn glb(translation: [f32; 3], material_name: &str) -> Vec<u8> {
        let mut bin = Vec::new();
        for v in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in v {
                bin.extend(c.to_le_bytes());
            }
        }
        let json = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0, "translation": translation}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "material": 0}]}],
            "materials": [{
                "name": material_name,
                "alphaMode": "MASK", "alphaCutoff": 0.3, "doubleSided": true,
                "emissiveFactor": [1.0, 0.5, 0.0],
                "extensions": {"KHR_materials_emissive_strength": {"emissiveStrength": 4.0}}
            }],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                           "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]}],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
            "buffers": [{"byteLength": 36}],
            "extensionsUsed": ["KHR_materials_emissive_strength"]
        });
        let mut j = serde_json::to_vec(&json).unwrap();
        while !j.len().is_multiple_of(4) {
            j.push(b' ');
        }
        let total = 12 + 8 + j.len() + 8 + bin.len();
        let mut out = Vec::new();
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

    #[test]
    fn applies_node_transforms_and_reads_material_flags() {
        let g = parse(&glb([0.0, 2.0, 0.0], "lamp_emitter")).unwrap();
        assert_eq!(g.prims.len(), 1);
        assert_eq!(g.prims[0].positions[0], [0.0, 2.0, 0.0]);
        assert_eq!(g.prims[0].indices, vec![0, 1, 2]);
        // no normals in the file: computed, facing +Z
        assert!(g.prims[0].normals[0][2] > 0.99);
        let m = &g.materials[0];
        assert!(m.emitter && m.double_sided && !m.glass);
        assert_eq!(m.alpha, Alpha::Mask(0.3));
        assert_eq!(m.emissive_strength, 4.0);
    }

    #[test]
    fn recognises_glass_by_name_and_rejects_garbage() {
        assert!(parse(&glb([0.0; 3], "Window_Glass")).unwrap().materials[0].glass);
        assert!(parse(b"not a glb").is_err());
    }
}
