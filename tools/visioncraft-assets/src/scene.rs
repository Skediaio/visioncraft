//! In-memory model: merged meshes (one per material), normalisation, simplification, reading glTF.

use anyhow::{Context, Result, bail};
use image::{DynamicImage, GrayImage, RgbImage, imageops::FilterType};
use std::{collections::HashMap, path::Path, sync::Arc};

#[derive(Clone, Default)]
pub struct Mesh {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub idx: Vec<u32>,
    pub mat: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alpha {
    Opaque,
    Mask,
    Blend,
}

#[derive(Clone)]
pub struct Material {
    pub base: Option<Arc<DynamicImage>>,
    pub base_factor: [f32; 4],
    /// Packed R = AO, G = roughness, B = metallic (the glTF convention).
    pub orm: Option<Arc<DynamicImage>>,
    pub normal: Option<Arc<DynamicImage>>,
    pub metallic: f32,
    pub roughness: f32,
    pub alpha: Alpha,
    pub cutoff: f32,
    pub double_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base: None,
            base_factor: [1.0; 4],
            orm: None,
            normal: None,
            metallic: 0.0,
            roughness: 1.0,
            alpha: Alpha::Opaque,
            cutoff: 0.5,
            double_sided: true,
        }
    }
}

#[derive(Clone, Default)]
pub struct Model {
    pub meshes: Vec<Mesh>,
    pub mats: Vec<Material>,
}

impl Mesh {
    pub fn tris(&self) -> usize {
        self.idx.len() / 3
    }
}

impl Model {
    pub fn tris(&self) -> usize {
        self.meshes.iter().map(Mesh::tris).sum()
    }

    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut b = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in self.meshes.iter().flat_map(|m| &m.pos) {
            for k in 0..3 {
                b.0[k] = b.0[k].min(p[k]);
                b.1[k] = b.1[k].max(p[k]);
            }
        }
        b
    }

    pub fn size(&self) -> [f32; 3] {
        let (lo, hi) = self.bounds();
        std::array::from_fn(|k| hi[k] - lo[k])
    }

    /// Pivot at the centre of the footprint, lowest point at y = 0.
    pub fn normalise(&mut self) {
        let (lo, hi) = self.bounds();
        let t = visioncraft_core::catalog::normalising_transform(lo, hi);
        for p in self.meshes.iter_mut().flat_map(|m| &mut m.pos) {
            for k in 0..3 {
                p[k] += t[k];
            }
        }
    }

    /// Put the lowest point back on y = 0 (simplification can lift it by a few millimetres).
    pub fn ground(mut self) -> Model {
        let lo = self.bounds().0[1];
        self.meshes
            .iter_mut()
            .flat_map(|m| &mut m.pos)
            .for_each(|p| p[1] -= lo);
        self
    }

    /// Simplify to about `target` triangles, spread over the material meshes by their share.
    pub fn simplified(&self, target: usize) -> Model {
        let total = self.tris();
        let mut out = self.clone();
        if target >= total {
            return out;
        }
        for m in &mut out.meshes {
            let share = (target as f64 * m.tris() as f64 / total as f64).round() as usize;
            *m = simplify_mesh(m, share.max(4.min(m.tris())));
        }
        out
    }
}

/// meshopt's quadric edge collapse with UV and normal as attributes (they protect seams); the
/// error limit grows until the target is reached, so geometry is only spoiled when it must be.
/// Hard-edged models (split normals) stall that pass, so the second try welds vertices that
/// share position and UV, then ones that share only position (normals get averaged: LODs are
/// shaded a little softer, UVs can stretch at island borders). Whatever is still short is finished with meshopt's clustering simplifier.
/// shortcut: that last step can smear UVs on LOD2, upgrade to own seam-aware collapse if seen.
fn simplify_mesh(m: &Mesh, target_tris: usize) -> Mesh {
    if target_tris >= m.tris() {
        return m.clone();
    }
    let ok = |idx: &[u32]| idx.len() / 3 <= target_tris * 6 / 5;
    let idx = collapse(m, target_tris);
    if ok(&idx) {
        return compact(m, &idx);
    }
    let mut w = weld(m, true);
    let mut idx = collapse(&w, target_tris);
    if !ok(&idx) {
        w = weld(m, false);
        idx = collapse(&w, target_tris);
    }
    if !ok(&idx) {
        idx = meshopt::simplify_sloppy_decoder(&idx, &w.pos, target_tris * 3, 1.0, None);
    }
    compact(&w, &idx)
}

fn collapse(m: &Mesh, target_tris: usize) -> Vec<u32> {
    let attrs: Vec<f32> = (0..m.pos.len())
        .flat_map(|i| {
            [
                m.uv[i][0],
                m.uv[i][1],
                m.nrm[i][0],
                m.nrm[i][1],
                m.nrm[i][2],
            ]
        })
        .collect();
    let locks = vec![false; m.pos.len()];
    let mut idx = m.idx.clone();
    for err in [0.01_f32, 0.03, 0.1, 0.3, 1.0] {
        idx = meshopt::simplify_with_attributes_and_locks_decoder(
            &m.idx,
            &m.pos,
            &attrs,
            &[1.0, 1.0, 0.5, 0.5, 0.5],
            20,
            &locks,
            target_tris * 3,
            err,
            meshopt::SimplifyOptions::None,
            None,
        );
        if idx.len() / 3 <= target_tris * 6 / 5 {
            break;
        }
    }
    idx
}

/// Merge vertices with the same position (and UV when `with_uv`; otherwise the first one's UV
/// wins); their normals are averaged.
fn weld(m: &Mesh, with_uv: bool) -> Mesh {
    let mut map: HashMap<([i64; 3], [i64; 2]), u32> = HashMap::new();
    let mut out = Mesh {
        mat: m.mat,
        ..Mesh::default()
    };
    let mut remap = Vec::with_capacity(m.pos.len());
    for i in 0..m.pos.len() {
        let key = (
            m.pos[i].map(|v| (v * 1e5).round() as i64),
            if with_uv {
                m.uv[i].map(|v| (v * 1e4).round() as i64)
            } else {
                [0; 2]
            },
        );
        let id = *map.entry(key).or_insert_with(|| {
            out.pos.push(m.pos[i]);
            out.nrm.push([0.0; 3]);
            out.uv.push(m.uv[i]);
            out.pos.len() as u32 - 1
        });
        for k in 0..3 {
            out.nrm[id as usize][k] += m.nrm[i][k];
        }
        remap.push(id);
    }
    out.nrm.iter_mut().for_each(|n| *n = norm(*n));
    out.idx = m.idx.iter().map(|&i| remap[i as usize]).collect();
    out
}

/// Keep only the vertices an index list uses.
fn compact(m: &Mesh, idx: &[u32]) -> Mesh {
    let mut map = vec![u32::MAX; m.pos.len()];
    let mut out = Mesh {
        mat: m.mat,
        ..Mesh::default()
    };
    for &i in idx {
        let i = i as usize;
        if map[i] == u32::MAX {
            map[i] = out.pos.len() as u32;
            out.pos.push(m.pos[i]);
            out.nrm.push(m.nrm[i]);
            out.uv.push(m.uv[i]);
        }
        out.idx.push(map[i]);
    }
    out
}

/// Pack separate maps: R = AO (1 when absent), G = roughness (1 when absent), B = metallic (0
/// when absent). Maps of another size are resized to the largest.
pub fn pack_orm(
    ao: Option<&GrayImage>,
    rough: Option<&GrayImage>,
    metal: Option<&GrayImage>,
) -> RgbImage {
    let maps = [ao, rough, metal];
    let (w, h) = maps
        .iter()
        .flatten()
        .map(|g| g.dimensions())
        .max_by_key(|d| d.0 * d.1)
        .unwrap_or((1, 1));
    let fit = |g: Option<&GrayImage>| {
        g.map(|g| {
            if g.dimensions() == (w, h) {
                g.clone()
            } else {
                image::imageops::resize(g, w, h, FilterType::Triangle)
            }
        })
    };
    let [ao, rough, metal] = maps.map(fit);
    let mut out = RgbImage::new(w, h);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let get = |g: &Option<GrayImage>, d: u8| g.as_ref().map_or(d, |g| g.get_pixel(x, y).0[0]);
        *p = image::Rgb([get(&ao, 255), get(&rough, 255), get(&metal, 0)]);
    }
    out
}

fn channel(img: &DynamicImage, c: usize) -> GrayImage {
    let rgb = img.to_rgb8();
    GrayImage::from_fn(rgb.width(), rgb.height(), |x, y| {
        image::Luma([rgb.get_pixel(x, y).0[c]])
    })
}

type M4 = [[f32; 4]; 4];

fn mul(a: &M4, b: &M4) -> M4 {
    std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| a[k][r] * b[c][k]).sum()))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

struct Reader<'a> {
    base: &'a Path,
    bufs: Vec<gltf::buffer::Data>,
    images: HashMap<usize, Arc<DynamicImage>>,
}

impl Reader<'_> {
    fn image(&mut self, img: gltf::Image) -> Result<Arc<DynamicImage>> {
        if let Some(i) = self.images.get(&img.index()) {
            return Ok(i.clone());
        }
        let decoded = match img.source() {
            gltf::image::Source::Uri { uri, .. } => {
                let path = self.base.join(uri.replace("%20", " "));
                image::open(&path).with_context(|| format!("texture {}", path.display()))?
            }
            gltf::image::Source::View { view, .. } => {
                let data = &self.bufs[view.buffer().index()].0;
                image::load_from_memory(&data[view.offset()..view.offset() + view.length()])?
            }
        };
        let decoded = Arc::new(decoded);
        self.images.insert(img.index(), decoded.clone());
        Ok(decoded)
    }

    fn material(&mut self, m: gltf::Material) -> Result<Material> {
        let pbr = m.pbr_metallic_roughness();
        let base = pbr
            .base_color_texture()
            .map(|t| self.image(t.texture().source()))
            .transpose()?;
        let normal = m
            .normal_texture()
            .map(|t| self.image(t.texture().source()))
            .transpose()?;
        let mr = pbr
            .metallic_roughness_texture()
            .map(|t| t.texture().source());
        let occ = m.occlusion_texture().map(|t| t.texture().source());
        let orm = match (mr, occ) {
            (Some(mr), Some(occ)) if mr.index() != occ.index() => {
                let (mr, occ) = (self.image(mr)?, self.image(occ)?);
                let (g, b) = (channel(&mr, 1), channel(&mr, 2));
                let ao = channel(&occ, 0);
                Some(Arc::new(DynamicImage::ImageRgb8(pack_orm(
                    Some(&ao),
                    Some(&g),
                    Some(&b),
                ))))
            }
            (Some(mr), _) => Some(self.image(mr)?),
            (None, Some(occ)) => {
                let ao = channel(&*self.image(occ)?, 0);
                Some(Arc::new(DynamicImage::ImageRgb8(pack_orm(
                    Some(&ao),
                    None,
                    None,
                ))))
            }
            (None, None) => None,
        };
        Ok(Material {
            base,
            base_factor: pbr.base_color_factor(),
            orm,
            normal,
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            alpha: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => Alpha::Opaque,
                gltf::material::AlphaMode::Mask => Alpha::Mask,
                gltf::material::AlphaMode::Blend => Alpha::Blend,
            },
            cutoff: m.alpha_cutoff().unwrap_or(0.5),
            double_sided: m.double_sided(),
        })
    }

    fn walk(&self, node: gltf::Node, parent: &M4, default_mat: usize, out: &mut Vec<Mesh>) {
        let m = mul(parent, &node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for p in mesh
                .primitives()
                .filter(|p| p.mode() == gltf::mesh::Mode::Triangles)
            {
                let r = p.reader(|b| Some(&self.bufs[b.index()]));
                let Some(pos) = r.read_positions() else {
                    continue;
                };
                let a: [[f32; 3]; 3] = std::array::from_fn(|c| [m[c][0], m[c][1], m[c][2]]);
                let det: f32 = a[0].iter().zip(cross(a[1], a[2])).map(|(x, y)| x * y).sum();
                let (c0, c1, c2) = (cross(a[1], a[2]), cross(a[2], a[0]), cross(a[0], a[1]));
                let s = det.signum();
                let mut mesh = Mesh {
                    pos: pos
                        .map(|v| {
                            std::array::from_fn(|k| {
                                m[0][k] * v[0] + m[1][k] * v[1] + m[2][k] * v[2] + m[3][k]
                            })
                        })
                        .collect(),
                    mat: p.material().index().unwrap_or(default_mat),
                    ..Mesh::default()
                };
                let n = mesh.pos.len();
                mesh.idx = match r.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..n as u32).collect(),
                };
                if det < 0.0 {
                    mesh.idx
                        .as_chunks_mut::<3>()
                        .0
                        .iter_mut()
                        .for_each(|t| t.swap(1, 2));
                }
                mesh.uv = r
                    .read_tex_coords(0)
                    .map_or_else(|| vec![[0.0; 2]; n], |t| t.into_f32().collect());
                mesh.nrm = match r.read_normals() {
                    Some(ns) => ns
                        .map(|v| {
                            norm(std::array::from_fn(|k| {
                                (c0[k] * v[0] + c1[k] * v[1] + c2[k] * v[2]) * s
                            }))
                        })
                        .collect(),
                    None => face_normals(&mesh),
                };
                if mesh.uv.len() == n
                    && mesh.nrm.len() == n
                    && mesh.idx.iter().all(|&i| (i as usize) < n)
                {
                    out.push(mesh);
                }
            }
        }
        for c in node.children() {
            self.walk(c, &m, default_mat, out);
        }
    }
}

/// `x_a`, `x_b_LOD0`: the letter that names a variant.
fn variant_letter(name: &str) -> Option<char> {
    let s = name.strip_suffix("_LOD0").unwrap_or(name);
    let (_, tail) = s.rsplit_once('_')?;
    let mut cs = tail.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) if c.is_ascii_lowercase() => Some(c),
        _ => None,
    }
}

fn face_normals(m: &Mesh) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0_f32; 3]; m.pos.len()];
    for t in m.idx.as_chunks::<3>().0 {
        let (a, b, c) = (
            m.pos[t[0] as usize],
            m.pos[t[1] as usize],
            m.pos[t[2] as usize],
        );
        let n = cross(
            std::array::from_fn(|k| b[k] - a[k]),
            std::array::from_fn(|k| c[k] - a[k]),
        );
        for &i in t {
            for k in 0..3 {
                acc[i as usize][k] += n[k];
            }
        }
    }
    acc.into_iter().map(norm).collect()
}

/// Read a glTF (node transforms applied to the vertices; primitives with one material merged;
/// cameras, lights, animations, skins and everything unreferenced are ignored). Textures are
/// resolved relative to `base`.
pub fn read_gltf(g: &gltf::Gltf, base: &Path) -> Result<Model> {
    let doc = &g.document;
    let bufs = gltf::import_buffers(doc, Some(base), g.blob.clone())?;
    let mut r = Reader {
        base,
        bufs,
        images: HashMap::new(),
    };
    let mut mats = doc
        .materials()
        .map(|m| r.material(m))
        .collect::<Result<Vec<_>>>()?;
    let default_mat = mats.len();
    mats.push(Material::default());
    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .context("no scene")?;
    let identity: M4 =
        std::array::from_fn(|c| std::array::from_fn(|r| f32::from(u8::from(c == r))));
    let mut parts = Vec::new();
    // Poly Haven plants come as a row of variants (`x_a`, `x_b`, ...): keep the first one.
    let mut roots: Vec<gltf::Node> = scene.nodes().collect();
    if roots.len() > 1
        && roots
            .iter()
            .all(|n| n.name().and_then(variant_letter).is_some())
    {
        roots.sort_by_key(|n| n.name().and_then(variant_letter));
        roots.truncate(1);
    }
    for n in roots {
        r.walk(n, &identity, default_mat, &mut parts);
    }
    let mut meshes: Vec<Mesh> = Vec::new();
    for p in parts {
        match meshes.iter_mut().find(|m| m.mat == p.mat) {
            Some(m) => {
                let off = m.pos.len() as u32;
                m.idx.extend(p.idx.iter().map(|i| i + off));
                m.pos.extend(p.pos);
                m.nrm.extend(p.nrm);
                m.uv.extend(p.uv);
            }
            None => meshes.push(p),
        }
    }
    if meshes.is_empty() {
        bail!("no triangle geometry");
    }
    Ok(Model { meshes, mats })
}

pub fn read_gltf_file(path: &Path) -> Result<Model> {
    let g = gltf::Gltf::open(path).with_context(|| path.display().to_string())?;
    read_gltf(&g, path.parent().unwrap_or(Path::new(".")))
}

pub fn read_glb(bytes: &[u8]) -> Result<Model> {
    read_gltf(&gltf::Gltf::from_slice(bytes)?, Path::new("."))
}
