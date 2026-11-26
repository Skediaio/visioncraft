//! Reads a library GLB (self-contained glTF 2.0, metres, Y up) into plain CPU data: one merged
//! triangle list per material with the node transforms applied, the materials with the roles of
//! their images, and the still-encoded images. Decoding the images and everything on the GPU is
//! somebody else's job (`assets`, `library`).
//!
//! A model with animations (glTF 2.0, "Animations") keeps the nodes they move as rigid parts
//! ([`Glb::parts`]) and its clips ([`Glb::clips`]). A channel that is malformed, or that moves
//! nothing here, is left out with a warning ([`Glb::animation_warnings`]): the model still
//! loads, as a file without that animation would. Only broken geometry refuses a file.
//! [`Glb::rest_prims`] draws such a model whole at its rest pose.

use glam::{Mat3, Mat4, Vec3};
use gltf::accessor::{DataType, Dimensions};
use gltf::animation::Property;
use gltf::scene::Transform;
use std::collections::{HashMap, HashSet};
use visioncraft_core::keyframes::{self, Channel, Clip, Interpolation, Path, Trs};
use visioncraft_core::playback;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Alpha {
    Opaque,
    Mask(f32),
    Blend,
}

#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prim {
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// A node an animation moves, kept whole with the nodes under it that no animation moves on
/// their own. Until a clip plays, [`Glb::rest_prims`] draws it where the file has it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimatedPart {
    /// Its triangles in the node's own space: the node's own transform is not applied.
    pub prims: Vec<Prim>,
    /// The node's transform in the file.
    pub rest: Trs,
    /// The part it hangs under (`None`: the model's root). Parts are in the order of their
    /// nodes in the file, so a parent may have a higher index than its child: nothing may
    /// assume parents come first.
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glb {
    /// The triangles of the nodes no animation moves, in model space.
    pub prims: Vec<Prim>,
    pub materials: Vec<GlbMaterial>,
    /// Encoded (JPEG or PNG) bytes per glTF image.
    pub images: Vec<Vec<u8>>,
    /// The nodes the animations move, by their index in the file: every node a channel targets,
    /// with the nodes between it and the root (a fixed node there is part of its chain). Empty
    /// without animations.
    pub parts: Vec<AnimatedPart>,
    /// The animations, their channels targeting `parts`. Empty without animations, and
    /// without an animation nothing of which can play (see `animation_warnings`).
    pub clips: Vec<Clip>,
    /// What was left out of the animations and why: a malformed channel, an animation that
    /// moves nothing here. Empty for most files.
    pub animation_warnings: Vec<String>,
}

impl Glb {
    /// The bones the clips move, for playback and queries: each part's rest transform and
    /// parent part, with the clips. `None` for a model that stands still.
    pub fn rig(&self) -> Option<keyframes::Rig> {
        if self.parts.is_empty() {
            return None;
        }
        Some(keyframes::Rig {
            rest: self.parts.iter().map(|p| p.rest).collect(),
            parents: self.parts.iter().map(|p| p.parent).collect(),
            clips: self.clips.clone(),
        })
    }

    /// The clips, what was left out of them, and the bones they move ([`Self::rig`]).
    pub fn animation(&self) -> playback::Animation {
        playback::Animation {
            clips: self.clips.clone(),
            warnings: self.animation_warnings.clone(),
            rig: self.rig(),
        }
    }
}

impl Glb {
    /// The whole model at its rest pose: `prims` with every part where the file has it. For
    /// what a model measures and its thumbnail; the scene draws `prims` and the parts apart,
    /// so that a clip can move them.
    /// Nothing moves in most models: then these are `prims` themselves, not a copy.
    pub fn rest_prims(&self) -> std::borrow::Cow<'_, [Prim]> {
        if self.parts.is_empty() {
            return std::borrow::Cow::Borrowed(&self.prims);
        }
        let mut out = self.prims.clone();
        let parents: Vec<Option<usize>> = self.parts.iter().map(|p| p.parent).collect();
        let rest: Vec<Trs> = self.parts.iter().map(|p| p.rest).collect();
        for (part, m) in self
            .parts
            .iter()
            .zip(keyframes::model_space(&parents, &rest))
        {
            let xf = Mat4::from_cols_array(&m);
            for prim in &part.prims {
                merge(
                    &mut out,
                    prim.material,
                    &prim.positions,
                    &prim.normals,
                    &prim.uvs,
                    &prim.indices,
                    xf,
                );
            }
        }
        std::borrow::Cow::Owned(out)
    }
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

/// A primitive's triangles into the `Prim` of their material: positions through `xf`, normals
/// through the inverse transpose, the winding turned where `xf` mirrors.
fn merge(
    prims: &mut Vec<Prim>,
    material: usize,
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
    xf: Mat4,
) {
    let at = match prims.iter().position(|q| q.material == material) {
        Some(i) => i,
        None => {
            prims.push(Prim {
                material,
                ..Default::default()
            });
            prims.len() - 1
        }
    };
    let normal_xf = Mat3::from_mat4(xf).inverse().transpose();
    let flip = xf.determinant() < 0.0;
    let dst = &mut prims[at];
    let base = dst.positions.len() as u32;
    dst.positions.extend(
        positions
            .iter()
            .map(|&q| xf.transform_point3(Vec3::from(q)).to_array()),
    );
    dst.normals.extend(
        normals
            .iter()
            .map(|&n| (normal_xf * Vec3::from(n)).normalize_or(Vec3::Y).to_array()),
    );
    dst.uvs.extend(uvs);
    if flip {
        for t in indices.as_chunks::<3>().0 {
            dst.indices.extend([t[0], t[2], t[1]].map(|i| i + base));
        }
    } else {
        dst.indices.extend(indices.iter().map(|i| i + base));
    }
}

/// The parent of every node the scene reaches (the first one it is found under, as the walk
/// draws it) and how many times each is reached. A hierarchy that loops back on itself is
/// refused: the walk below would never end in it.
fn tree_of(
    doc: &gltf::Document,
    scene: gltf::Scene,
) -> Result<(Vec<Option<usize>>, Vec<u8>), String> {
    let n = doc.nodes().count();
    let (mut parent, mut reached, mut state) = (vec![None; n], vec![0u8; n], vec![0u8; n]);
    // `state`: 0 unseen, 1 on the path being walked, 2 left behind.
    let mut stack: Vec<(gltf::Node, bool)> = Vec::new();
    for root in scene.nodes() {
        reached[root.index()] = reached[root.index()].saturating_add(1);
        stack.push((root, false));
    }
    while let Some((node, leaving)) = stack.pop() {
        let i = node.index();
        if leaving {
            state[i] = 2;
            continue;
        }
        if state[i] == 1 {
            return Err("the node hierarchy loops".into());
        }
        if state[i] == 2 {
            continue;
        }
        state[i] = 1;
        stack.push((node.clone(), true));
        for child in node.children() {
            let c = child.index();
            reached[c] = reached[c].saturating_add(1);
            if parent[c].is_none() {
                parent[c] = Some(i);
            }
            stack.push((child, false));
        }
    }
    Ok((parent, reached))
}

/// The nodes the animations keep apart, by index: every node a channel moves, with the nodes
/// between it and the root, so that a fixed node there (a chest under a lid) is not lost. A
/// node no channel moves whose own transform is the identity adds nothing to that chain and
/// stays in the static model.
fn kept_nodes(doc: &gltf::Document, tree: &[Option<usize>], moved: &HashSet<usize>) -> Vec<usize> {
    let mut kept = moved.clone();
    let mut under: Vec<usize> = kept.iter().copied().collect();
    while let Some(n) = under.pop() {
        if let Some(p) = tree[n]
            && kept.insert(p)
        {
            under.push(p);
        }
    }
    let identity = |n: usize| {
        doc.nodes().nth(n).is_some_and(|node| {
            let (t, r, s) = node.transform().decomposed();
            t == [0.0; 3] && r == [0.0, 0.0, 0.0, 1.0] && s == [1.0; 3]
        })
    };
    kept.retain(|n| moved.contains(n) || !identity(*n));
    let mut kept: Vec<usize> = kept.into_iter().collect();
    kept.sort_unstable();
    kept
}

/// The floats of an animation accessor with `dims` components per element, or a message. The
/// crate's reader trusts the file where glTF does not: an empty accessor, a view too small, a
/// stride under the element all end badly there, so they are refused here first. A rotation may
/// be stored as normalised 8- or 16-bit integers (`quantised`).
fn floats<'a, 's, F>(
    accessor: gltf::Accessor<'a>,
    dims: Dimensions,
    quantised: bool,
    data: F,
) -> Result<Vec<f32>, String>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    if accessor.sparse().is_some() {
        return Err("sparse, which an animation does not use here".into());
    }
    let Some(view) = accessor.view() else {
        return Err("in no buffer view".into());
    };
    if data(view.buffer()).is_none() {
        return Err("in a buffer that is not embedded".into());
    }
    let (count, item) = (accessor.count(), accessor.size());
    if count == 0 {
        return Err("empty".into());
    }
    let stride = view.stride().unwrap_or(item);
    if stride < item {
        return Err(format!("packed with a byteStride of {stride} under {item}"));
    }
    let need = accessor.offset() as u64 + stride as u64 * (count as u64 - 1) + item as u64;
    if need > view.length() as u64 {
        return Err("beyond their buffer view".into());
    }
    if accessor.dimensions() != dims {
        return Err(format!("{:?}, not {dims:?}", accessor.dimensions()));
    }
    let mut out = Vec::with_capacity(count * dims.multiplicity());
    macro_rules! read {
        (scalar $t:ty) => {{
            for v in gltf::accessor::Iter::<$t>::new(accessor, data.clone())
                .ok_or("beyond their buffer view")?
            {
                out.push(v as f32);
            }
        }};
        ($t:ty) => {{
            for v in gltf::accessor::Iter::<$t>::new(accessor, data.clone())
                .ok_or("beyond their buffer view")?
            {
                out.extend(v.map(|c| c as f32));
            }
        }};
    }
    match accessor.data_type() {
        DataType::F32 => match dims {
            Dimensions::Scalar => read!(scalar f32),
            Dimensions::Vec3 => read!([f32; 3]),
            _ => read!([f32; 4]),
        },
        DataType::I8 if quantised => {
            read!([i8; 4]);
            for v in &mut out {
                *v = (*v / 127.0).max(-1.0);
            }
        }
        DataType::U8 if quantised => {
            read!([u8; 4]);
            for v in &mut out {
                *v /= 255.0;
            }
        }
        DataType::I16 if quantised => {
            read!([i16; 4]);
            for v in &mut out {
                *v = (*v / 32767.0).max(-1.0);
            }
        }
        DataType::U16 if quantised => {
            read!([u16; 4]);
            for v in &mut out {
                *v /= 65535.0;
            }
        }
        other => return Err(format!("{other:?}, not floats")),
    }
    Ok(out)
}

/// The keys of a sampler for a `path`: how it interpolates, its times and its values. Anything
/// glTF calls malformed is a message, never a panic.
fn sampler_keys<'a, 's, F>(
    sampler: gltf::animation::Sampler<'a>,
    path: Path,
    data: F,
) -> Result<(Interpolation, Vec<f32>, Vec<f32>), String>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    use gltf::animation::Interpolation as Gltf;
    let interpolation = match sampler.interpolation() {
        Gltf::Step => Interpolation::Step,
        Gltf::Linear => Interpolation::Linear,
        Gltf::CubicSpline => Interpolation::CubicSpline,
    };
    let times = floats(sampler.input(), Dimensions::Scalar, false, data.clone())
        .map_err(|e| format!("its key times are {e}"))?;
    if times.iter().any(|t| *t < 0.0) {
        return Err("a key time is negative".into());
    }
    if interpolation == Interpolation::CubicSpline && times.len() < 2 {
        return Err("a cubic spline needs two keys".into());
    }
    let rotation = path == Path::Rotation;
    let dims = if rotation {
        Dimensions::Vec4
    } else {
        Dimensions::Vec3
    };
    let values = floats(sampler.output(), dims, rotation, data)
        .map_err(|e| format!("its values are {e}"))?;
    let stride = interpolation.stride(path);
    if values.len() != times.len() * stride {
        return Err(format!(
            "it has {} keys but values for {}",
            times.len(),
            values.len() / stride
        ));
    }
    Ok((interpolation, times, values))
}

/// The clips of the file, their channels naming glTF nodes, and what was left out. A channel
/// that is malformed, or that moves nothing here (a morph target, a node that does not exist,
/// a node given as a matrix, a node outside the scene or one the scene does not reach once),
/// is left out with a warning, and so is an animation nothing is left of: the model still
/// loads. The nodes the kept channels name are what [`kept_nodes`] keeps apart.
fn read_clips<'a, 's, F>(
    doc: &'a gltf::Document,
    tree: &[Option<usize>],
    reached: &[u8],
    data: F,
) -> (Vec<Clip>, Vec<String>)
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    // The node and every node above it are each reached once.
    let once =
        |first: usize| std::iter::successors(Some(first), |i| tree[*i]).all(|i| reached[i] == 1);
    let raw = doc.as_json();
    let nodes = raw.nodes.len();
    let (mut clips, mut warnings) = (Vec::new(), Vec::new());
    for animation in doc.animations() {
        let name = animation.name().unwrap_or_default().to_string();
        let json = &raw.animations[animation.index()];
        let mut clip = Clip {
            name: name.clone(),
            duration: 0.0,
            channels: Vec::new(),
        };
        for (i, channel) in animation.channels().enumerate() {
            let target = channel.target();
            let path = match target.property() {
                Property::Translation => Path::Translation,
                Property::Rotation => Path::Rotation,
                Property::Scale => Path::Scale,
                Property::MorphTargetWeights => {
                    warnings.push(format!(
                        "the animation `{name}`: channel {i} moves morph targets, which are not drawn here; the channel is left out"
                    ));
                    continue;
                }
            };
            let index = json.channels[i].target.node.value();
            let what = format!("the animation `{name}`: channel {i} ({path:?} of node {index})");
            // The crate does not check a channel's node, and panics on a bad one later.
            if index >= nodes {
                warnings.push(format!(
                    "{what}: the node does not exist; the channel is left out"
                ));
                continue;
            }
            let node = doc.nodes().nth(index).expect("the index is in range");
            if matches!(node.transform(), Transform::Matrix { .. }) {
                warnings.push(format!(
                    "{what}: the node is given as a matrix, which has nothing to animate; the channel is left out"
                ));
                continue;
            }
            if reached[index] != 1 {
                warnings.push(format!(
                    "{what}: the scene reaches the node {} times, so it would not move in one place; the channel is left out",
                    reached[index]
                ));
                continue;
            }
            if !once(index) {
                warnings.push(format!(
                    "{what}: a node above it is reached more than once by the scene; the channel is left out"
                ));
                continue;
            }
            match sampler_keys(channel.sampler(), path, data.clone()) {
                Ok((interpolation, times, values)) => {
                    let ch = Channel {
                        node: index,
                        path,
                        interpolation,
                        times,
                        values,
                    };
                    if let Err(e) = ch.check(&what) {
                        warnings.push(format!("{e}; the channel is left out"));
                    } else {
                        clip.channels.push(ch);
                    }
                }
                Err(e) => warnings.push(format!("{what}: {e}; the channel is left out")),
            }
        }
        if clip.channels.is_empty() {
            warnings.push(format!(
                "the animation `{name}` is left out: nothing of it can move here"
            ));
            continue;
        }
        clip.duration = clip.end();
        if let Err(e) = clip.validate() {
            warnings.push(format!("the animation `{name}`: {e}; it is left out"));
            continue;
        }
        clips.push(clip);
    }
    (clips, warnings)
}

/// Where the walk is: the node, its matrix in model space, and when it lies under a part, that
/// part and the matrix from the part's node down to the node's parent.
type Frame<'a> = (gltf::Node<'a>, Mat4, Option<(usize, Mat4)>);

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
    let (tree, reached) = tree_of(doc, scene.clone())?;
    let (mut clips, animation_warnings) = read_clips(doc, &tree, &reached, data);
    let moved: HashSet<usize> = clips
        .iter()
        .flat_map(|c| c.channels.iter().map(|ch| ch.node))
        .collect();
    let kept = kept_nodes(doc, &tree, &moved);
    let part_of: HashMap<usize, usize> = kept.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut parts: Vec<AnimatedPart> = kept
        .iter()
        .map(|&n| {
            let node = doc.nodes().nth(n);
            let (translation, rotation, scale) = node
                .map_or(([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]), |n| {
                    n.transform().decomposed()
                });
            AnimatedPart {
                prims: Vec::new(),
                rest: Trs {
                    translation,
                    rotation,
                    scale,
                },
                parent: std::iter::successors(tree[n], |p| tree[*p])
                    .find_map(|a| part_of.get(&a).copied()),
            }
        })
        .collect();

    // A node an animation keeps opens its part at its own origin; a node under one is taken
    // relative to it; every other node is merged in model space, as always.
    let mut stack: Vec<Frame> = scene.nodes().map(|n| (n, Mat4::IDENTITY, None)).collect();
    while let Some((node, parent_xf, frame)) = stack.pop() {
        let local = Mat4::from_cols_array_2d(&node.transform().matrix());
        let model_xf = parent_xf * local;
        let (xf, under, into) = match (part_of.get(&node.index()).copied(), frame) {
            (Some(part), _) => (Mat4::IDENTITY, Some((part, Mat4::IDENTITY)), Some(part)),
            (None, Some((part, base))) => (base * local, Some((part, base * local)), Some(part)),
            (None, None) => (model_xf, None, None),
        };
        stack.extend(node.children().map(|c| (c, model_xf, under)));
        let Some(mesh) = node.mesh() else {
            continue;
        };
        for p in mesh.primitives() {
            if p.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let r = p.reader(data);
            let Some(pos) = r.read_positions() else {
                continue;
            };
            let pos: Vec<[f32; 3]> = pos.collect();
            let indices: Vec<u32> = match r.read_indices() {
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
            match into {
                Some(part) => merge(
                    &mut parts[part].prims,
                    material,
                    &pos,
                    &normals,
                    &uvs,
                    &indices,
                    xf,
                ),
                None => merge(&mut out.prims, material, &pos, &normals, &uvs, &indices, xf),
            }
        }
    }
    out.prims.retain(|p| !p.indices.is_empty());
    for part in &mut parts {
        part.prims.retain(|p| !p.indices.is_empty());
    }
    if out.prims.is_empty() && parts.iter().all(|p| p.prims.is_empty()) {
        return Err("the file has no triangles".into());
    }
    out.parts = parts;
    // The channels now name parts instead of nodes.
    for ch in clips.iter_mut().flat_map(|c| c.channels.iter_mut()) {
        ch.node = part_of[&ch.node];
    }
    out.clips = clips;
    out.animation_warnings = animation_warnings;
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// `doc` as the JSON chunk and `bin` as the BIN chunk of a GLB.
    fn pack(doc: &Value, bin: &[u8]) -> Vec<u8> {
        let mut j = serde_json::to_vec(doc).expect("json");
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
        out.extend(bin);
        out
    }

    /// A GLB with one triangle under a node that moves it up by 2 m, with an emitter material.
    fn glb(translation: [f32; 3], material_name: &str) -> Vec<u8> {
        let mut bin = Vec::new();
        for v in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in v {
                bin.extend(c.to_le_bytes());
            }
        }
        pack(
            &json!({
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
            }),
            &bin,
        )
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

    /// The loader as it was before animations were read: every node's triangles flattened into
    /// model space. The reference for a model without animations.
    fn flat_parse(bytes: &[u8]) -> Result<Glb, String> {
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
        let mut stack: Vec<(gltf::Node, Mat4)> =
            scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
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

    const A: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    const B: [[f32; 3]; 3] = [[2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 0.0]];
    const C: [[f32; 3]; 3] = [[0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 0.0, 1.0]];
    /// The sine and the cosine of 45 degrees.
    const Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

    /// A triangle as the loader hands it out when nothing moves it: at its own place, its
    /// normal facing +Z (computed, the file has none), no UVs.
    fn as_is(p: [[f32; 3]; 3]) -> Prim {
        Prim {
            material: 0,
            positions: p.to_vec(),
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0; 2]; 3],
            indices: vec![0, 1, 2],
        }
    }

    /// A GLB under construction for the tests: triangles, a node tree, materials and animations,
    /// all in one BIN chunk. Every node no node lists as a child is a root.
    #[derive(Default)]
    pub(crate) struct Builder {
        bin: Vec<u8>,
        accessors: Vec<Value>,
        views: Vec<Value>,
        nodes: Vec<Value>,
        meshes: Vec<Value>,
        materials: Vec<Value>,
        animations: Vec<Value>,
    }

    impl Builder {
        /// No triangles yet, and one material named `material` (an emitter when it ends in
        /// `_emitter`, as the fixtures' are).
        pub(crate) fn new(material: &str) -> Self {
            Self {
                materials: vec![json!({
                    "name": material, "alphaMode": "MASK", "alphaCutoff": 0.3, "doubleSided": true,
                    "emissiveFactor": [1.0, 0.5, 0.0],
                    "extensions": {"KHR_materials_emissive_strength": {"emissiveStrength": 4.0}}
                })],
                ..Default::default()
            }
        }

        /// `bytes` as an accessor of `component` values, `ty` wide, over a new buffer view,
        /// with the `min` and `max` glTF asks of a position or a key time.
        fn accessor(&mut self, component: u32, ty: &str, bytes: &[u8]) -> usize {
            let width = match ty {
                "SCALAR" => 1,
                "VEC3" => 3,
                _ => 4,
            };
            // Floats are 4 bytes, 16-bit integers 2.
            let wide = component == 5126;
            let size = if wide { 4 } else { 2 };
            while !self.bin.len().is_multiple_of(4) {
                self.bin.push(0);
            }
            let at = self.bin.len();
            self.bin.extend(bytes);
            self.views
                .push(json!({"buffer": 0, "byteOffset": at, "byteLength": bytes.len()}));
            let values: Vec<f32> = bytes
                .chunks_exact(size)
                .map(|b| {
                    if wide {
                        f32::from_le_bytes(b.try_into().expect("four bytes"))
                    } else {
                        u16::from_le_bytes(b.try_into().expect("two bytes")) as f32
                    }
                })
                .collect();
            // Per component the least and the greatest finite value (JSON carries no NaN).
            let (mut min, mut max) = (Vec::new(), Vec::new());
            for c in 0..width {
                let column: Vec<f32> = (c..values.len())
                    .step_by(width)
                    .filter_map(|i| values[i].is_finite().then_some(values[i]))
                    .collect();
                let lo = column.first().copied().unwrap_or(0.0);
                min.push(column.iter().fold(lo, |m, v| m.min(*v)));
                max.push(column.iter().fold(lo, |m, v| m.max(*v)));
            }
            self.accessors.push(json!({
                "bufferView": self.views.len() - 1, "componentType": component,
                "count": bytes.len() / size / width, "type": ty, "normalized": !wide,
                "min": min, "max": max,
            }));
            self.accessors.len() - 1
        }

        /// `v` as a float accessor of type `ty` (`SCALAR`, `VEC3` or `VEC4`); the accessor's index.
        pub(crate) fn floats(&mut self, ty: &str, v: &[f32]) -> usize {
            let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
            self.accessor(5126, ty, &bytes)
        }

        /// `v` as normalised 16-bit integers of type `ty` (a quantised rotation).
        pub(crate) fn shorts(&mut self, ty: &str, v: &[u16]) -> usize {
            let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
            self.accessor(5122, ty, &bytes)
        }

        /// A triangle `p` as its own mesh with the first material; the mesh's index.
        pub(crate) fn triangle(&mut self, p: [[f32; 3]; 3]) -> usize {
            let a = self.floats("VEC3", &p.iter().flatten().copied().collect::<Vec<_>>());
            self.meshes
                .push(json!({"primitives": [{"attributes": {"POSITION": a}, "material": 0}]}));
            self.meshes.len() - 1
        }

        /// A node; its index. `fields` are the node's glTF properties (`mesh`, `translation`,
        /// `rotation`, `scale`, `matrix`, `children`).
        pub(crate) fn node(&mut self, fields: Value) -> usize {
            self.nodes.push(fields);
            self.nodes.len() - 1
        }

        /// An animation `name` with one channel: `node`'s `path` follows `values` from key time
        /// to key time, interpolated `interpolation` (`LINEAR`, `STEP` or `CUBICSPLINE`).
        pub(crate) fn animation(
            &mut self,
            name: &str,
            node: usize,
            path: &str,
            interpolation: &str,
            times: &[f32],
            values: &[f32],
        ) {
            let t = self.floats("SCALAR", times);
            let v = self.floats(if path == "rotation" { "VEC4" } else { "VEC3" }, values);
            self.animations.push(json!({
                "name": name,
                "samplers": [{"input": t, "interpolation": interpolation, "output": v}],
                "channels": [{"sampler": 0, "target": {"node": node, "path": path}}],
            }));
        }

        /// The GLB, `edit` applied to its JSON first.
        pub(crate) fn finish_json(self, edit: impl FnOnce(&mut Value)) -> Vec<u8> {
            let child: HashSet<usize> = self
                .nodes
                .iter()
                .flat_map(|n| n["children"].as_array().into_iter().flatten())
                .filter_map(|c| c.as_u64())
                .map(|c| c as usize)
                .collect();
            let roots: Vec<usize> = (0..self.nodes.len())
                .filter(|i| !child.contains(i))
                .collect();
            let mut doc = json!({
                "asset": {"version": "2.0"},
                "scene": 0,
                "scenes": [{"nodes": roots}],
                "nodes": self.nodes,
                "meshes": self.meshes,
                "materials": self.materials,
                "accessors": self.accessors,
                "bufferViews": self.views,
                "buffers": [{"byteLength": self.bin.len()}],
                "extensionsUsed": ["KHR_materials_emissive_strength"],
            });
            if !self.animations.is_empty() {
                doc["animations"] = Value::Array(self.animations);
            }
            edit(&mut doc);
            pack(&doc, &self.bin)
        }

        /// The GLB.
        pub(crate) fn finish(self) -> Vec<u8> {
            self.finish_json(|_| {})
        }
    }

    /// A hinge: a body with a lid on it, a quarter turned in the file, that swings in one clip.
    pub(crate) fn hinge() -> Builder {
        let mut b = Builder::new("steel");
        let (body, lid) = (b.triangle(A), b.triangle(B));
        let n0 = b.node(json!({"mesh": body}));
        let n1 = b.node(json!({
            "mesh": lid, "translation": [1.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.3826834, 0.9238795],
        }));
        b.nodes[n0]["children"] = json!([n1]);
        b.animation(
            "swing",
            n1,
            "rotation",
            "LINEAR",
            &[0.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, Q, Q],
        );
        b
    }

    #[test]
    fn a_hinge_is_one_part_with_its_clip() {
        let g = parse(&hinge().finish()).unwrap();
        // The body is the static model; the lid is kept apart, in its own space.
        assert_eq!(g.prims, vec![as_is(A)]);
        assert_eq!(
            g.parts,
            vec![AnimatedPart {
                prims: vec![as_is(B)],
                rest: Trs {
                    translation: [1.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.3826834, 0.9238795],
                    scale: [1.0; 3],
                },
                parent: None,
            }]
        );
        assert_eq!(
            g.clips,
            vec![Clip {
                name: "swing".into(),
                duration: 1.0,
                channels: vec![Channel {
                    node: 0,
                    path: Path::Rotation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, Q, Q],
                }],
            }]
        );
    }

    #[test]
    fn an_empty_node_moves_the_mesh_under_it() {
        let mut b = Builder::new("steel");
        let m = b.triangle(A);
        let empty = b.node(json!({"translation": [0.0, 2.0, 0.0]}));
        let child = b.node(json!({"mesh": m, "translation": [1.0, 0.0, 0.0]}));
        b.nodes[empty]["children"] = json!([child]);
        b.animation(
            "turn",
            empty,
            "rotation",
            "STEP",
            &[0.0, 0.5],
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, Q, Q],
        );
        let g = parse(&b.finish()).unwrap();
        assert!(g.prims.is_empty(), "nothing of it stands still");
        assert_eq!(g.parts.len(), 1);
        // The mesh where the child has it, relative to the node that moves.
        assert_eq!(
            g.parts[0].prims,
            vec![Prim {
                positions: vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
                ..as_is(A)
            }]
        );
        assert_eq!(
            g.parts[0].rest,
            Trs {
                translation: [0.0, 2.0, 0.0],
                ..Trs::IDENTITY
            }
        );
        assert_eq!(g.parts[0].parent, None);
        assert_eq!(g.clips[0].channels[0].interpolation, Interpolation::Step);
        assert_eq!(g.clips[0].duration, 0.5);
        // At rest the whole model stands where the file has it: 1 m east, 2 m up.
        let whole = g.rest_prims();
        assert_eq!(whole.len(), 1);
        let got = whole[0].positions[0];
        assert!(
            (got[0] - 1.0).abs() < 1e-5 && (got[1] - 2.0).abs() < 1e-5 && got[2].abs() < 1e-5,
            "{got:?}"
        );
    }

    #[test]
    fn a_fixed_node_between_the_root_and_a_moving_one_is_kept_too() {
        // A chest turned and set down 5 m east, with a lid 1 m up that opens. Without the chest
        // in the chain the lid would swing round the model's origin.
        let mut b = Builder::new("steel");
        let (chest, lid) = (b.triangle(A), b.triangle(B));
        let n0 = b.node(json!({
            "mesh": chest, "translation": [5.0, 0.0, 0.0], "rotation": [0.0, Q, 0.0, Q],
        }));
        let n1 = b.node(json!({"mesh": lid, "translation": [0.0, 1.0, 0.0]}));
        b.nodes[n0]["children"] = json!([n1]);
        b.animation(
            "open",
            n1,
            "rotation",
            "LINEAR",
            &[0.0, 2.0],
            &[0.0, 0.0, 0.0, 1.0, 0.0, Q, 0.0, Q],
        );
        let bytes = b.finish();
        let g = parse(&bytes).unwrap();
        assert!(g.prims.is_empty() && g.parts.len() == 2);
        assert_eq!((g.parts[0].parent, g.parts[1].parent), (None, Some(0)));
        assert_eq!(g.parts[0].rest.translation, [5.0, 0.0, 0.0]);
        assert_eq!(g.parts[1].rest.translation, [0.0, 1.0, 0.0]);
        assert_eq!(g.parts[1].prims, vec![as_is(B)]);
        assert_eq!(g.clips[0].channels[0].node, 1);
        // At rest it is the model the flat loader draws.
        let flat = flat_parse(&bytes).unwrap();
        let rest = g.rest_prims();
        assert_eq!(rest.len(), flat.prims.len());
        for (mine, theirs) in rest.iter().zip(&flat.prims) {
            assert_eq!(mine.material, theirs.material);
            assert_eq!(mine.indices, theirs.indices);
            assert_eq!(mine.positions.len(), theirs.positions.len());
            for (a, b) in mine
                .positions
                .iter()
                .chain(&mine.normals)
                .zip(theirs.positions.iter().chain(&theirs.normals))
            {
                assert!(
                    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn a_static_model_loads_as_before() {
        // Three deep with a mirror in the chain and two materials, and a second root: nothing
        // an animation touches.
        let mut b = Builder::new("lamp_emitter");
        b.materials.push(json!({"name": "Window_Glass"}));
        let (a, c, d) = (b.triangle(A), b.triangle(B), b.triangle(C));
        b.meshes[d]["primitives"][0]["material"] = json!(1);
        let n0 = b.node(json!({"mesh": a, "scale": [2.0, 2.0, 2.0]}));
        let n1 = b
            .node(json!({"mesh": c, "rotation": [0.0, Q, 0.0, Q], "translation": [0.0, 1.0, 0.0]}));
        let n2 = b.node(json!({"mesh": d, "scale": [-1.0, 1.0, 1.0]}));
        b.nodes[n0]["children"] = json!([n1]);
        b.nodes[n1]["children"] = json!([n2]);
        b.node(json!({"mesh": a, "translation": [0.0, 0.0, 3.0]}));
        for bytes in [
            glb([0.0, 2.0, 0.0], "lamp_emitter"),
            glb([0.0; 3], "Window_Glass"),
            b.finish(),
        ] {
            let g = parse(&bytes).unwrap();
            assert!(g.parts.is_empty() && g.clips.is_empty(), "nothing moves");
            assert_eq!(g, flat_parse(&bytes).unwrap(), "the same model as before");
            assert_eq!(g.rest_prims().as_ref(), g.prims.as_slice());
        }
    }

    #[test]
    fn cubic_keys_and_quantised_rotations_are_read() {
        let mut b = Builder::new("steel");
        let m = b.triangle(A);
        let n0 = b.node(json!({"mesh": m}));
        let t = b.floats("SCALAR", &[0.0, 2.0]);
        // A tangent of 1 m/s out of the first key, none into the second.
        let v = b.floats(
            "VEC3",
            &[
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0,
                0.0, 0.0,
            ],
        );
        // Two rotations as snorm16: none, then a quarter turn about Y (32767 * sin 45 degrees).
        let r = b.shorts("VEC4", &[0, 0, 0, 32767, 0, 0, 23170, 23170]);
        b.animations.push(json!({
            "name": "slide",
            "samplers": [
                {"input": t, "interpolation": "CUBICSPLINE", "output": v},
                {"input": t, "interpolation": "LINEAR", "output": r},
            ],
            "channels": [
                {"sampler": 0, "target": {"node": n0, "path": "translation"}},
                {"sampler": 1, "target": {"node": n0, "path": "rotation"}},
            ],
        }));
        let g = parse(&b.finish()).unwrap();
        let clip = &g.clips[0];
        assert_eq!((clip.name.as_str(), clip.duration), ("slide", 2.0));
        assert_eq!(
            clip.channels[0],
            Channel {
                node: 0,
                path: Path::Translation,
                interpolation: Interpolation::CubicSpline,
                times: vec![0.0, 2.0],
                values: vec![
                    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0,
                    0.0, 0.0,
                ],
            }
        );
        assert_eq!(clip.channels[1].path, Path::Rotation);
        let rotations = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, Q, Q];
        for (got, want) in clip.channels[1].values.iter().zip(rotations) {
            assert!((got - want).abs() < 1e-4, "{got}");
        }
        // Half way: the spline leaves its first key along the tangent, 0.125 * 1 m/s * 2 s =
        // 0.25 m, on its way to 4 m.
        let at = keyframes::sample(clip, 1.0, false, &[g.parts[0].rest])[0].translation;
        assert!((at[0] - 2.25).abs() < 1e-4 && at[1].abs() < 1e-5, "{at:?}");
    }

    /// A triangle with its normals, as comparable bits of rounded floats.
    type Triangle = (usize, [[u32; 3]; 3], [[u32; 3]; 3]);

    /// The triangles of `prims` whatever order they were merged in: per material, each triangle
    /// turned to start at its least vertex (so its winding stays), with its normals, rounded to
    /// a tenth of a millimetre, sorted.
    fn oriented(prims: &[Prim]) -> Vec<Triangle> {
        // A tenth of a millimetre, as bits (and no negative zero: the two loaders round it
        // either way).
        let round = |c: f32| {
            let r = (c * 10_000.0).round() / 10_000.0;
            (if r == 0.0 { 0.0 } else { r }).to_bits()
        };
        let mut out = Vec::new();
        for p in prims {
            for t in p.indices.as_chunks::<3>().0 {
                let v: [[f32; 3]; 3] = std::array::from_fn(|k| p.positions[t[k] as usize]);
                let n: [[f32; 3]; 3] = std::array::from_fn(|k| p.normals[t[k] as usize]);
                let (rv, rn) = (v.map(|q| q.map(round)), n.map(|q| q.map(round)));
                let first = (0..3).min_by_key(|k| rv[*k]).expect("three corners");
                let turn = |a: [[u32; 3]; 3]| std::array::from_fn(|k| a[(first + k) % 3]);
                out.push((p.material, turn(rv), turn(rn)));
            }
        }
        out.sort();
        out
    }

    /// A model that moves: its triangles at rest are the flat loader's, the same way round.
    fn rest_is_flat(bytes: &[u8]) {
        let g = parse(bytes).unwrap();
        assert!(
            !g.parts.is_empty() && !g.clips.is_empty(),
            "something moves"
        );
        let flat = flat_parse(bytes).unwrap();
        assert_eq!(g.materials, flat.materials);
        let rest = g.rest_prims();
        assert_eq!(oriented(&rest), oriented(&flat.prims));
        assert_eq!(
            g.rig()
                .map(|r| (r.rest.len(), r.parents.len(), r.clips.len())),
            Some((g.parts.len(), g.parts.len(), g.clips.len()))
        );
    }

    #[test]
    fn a_mirrored_part_and_a_mirrored_node_under_it_draw_as_the_flat_loader() {
        // The body stands; the lid is mirrored (its node scales x by -1) and swings; under the
        // lid a fixed node mirrored again carries a third triangle.
        let mut b = Builder::new("steel");
        let (body, lid, tag) = (b.triangle(A), b.triangle(B), b.triangle(C));
        let n0 = b.node(json!({"mesh": body}));
        let n1 = b.node(json!({
            "mesh": lid, "scale": [-1.0, 1.0, 1.0], "translation": [1.0, 0.0, 0.0],
        }));
        let n2 = b.node(json!({
            "mesh": tag, "scale": [-1.0, 1.0, 1.0], "translation": [0.0, 1.0, 0.0],
        }));
        b.nodes[n0]["children"] = json!([n1]);
        b.nodes[n1]["children"] = json!([n2]);
        b.animation(
            "swing",
            n1,
            "rotation",
            "LINEAR",
            &[0.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, Q, Q],
        );
        let bytes = b.finish();
        let g = parse(&bytes).unwrap();
        assert_eq!(g.prims, vec![as_is(A)]);
        assert_eq!(g.parts.len(), 1);
        assert!(
            g.animation_warnings.is_empty(),
            "{:?}",
            g.animation_warnings
        );
        // The part's rest carries the mirror, so its own triangle is not mirrored; the tag under
        // it lies as its node has it relative to the lid (mirrored again, 1 m up).
        assert_eq!(g.parts[0].prims.len(), 1, "one material");
        assert_eq!(
            g.parts[0].prims[0].positions,
            [
                B[0],
                B[1],
                B[2],
                [0.0, 1.0, 1.0],
                [0.0, 2.0, 1.0],
                [-1.0, 1.0, 1.0]
            ]
        );
        assert_eq!(
            g.parts[0].rest,
            Trs {
                translation: [1.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [-1.0, 1.0, 1.0],
            }
        );
        rest_is_flat(&bytes);
    }

    #[test]
    fn a_chain_keeps_its_moved_ends_and_skips_an_identity_node_between() {
        // P (a chest, moved) has I under it (the identity, with a triangle of its own), and I
        // has M under it (a lid, moved). The file lists M first: P is the part after its child.
        let mut b = Builder::new("steel");
        let (a, c, bb) = (b.triangle(A), b.triangle(C), b.triangle(B));
        let m = b.node(json!({"mesh": bb, "translation": [1.0, 0.0, 0.0]}));
        let i = b.node(json!({"mesh": c}));
        let p = b.node(json!({
            "mesh": a, "translation": [0.0, 1.0, 0.0], "rotation": [0.0, Q, 0.0, Q],
        }));
        b.nodes[p]["children"] = json!([i]);
        b.nodes[i]["children"] = json!([m]);
        b.animation(
            "lift",
            p,
            "translation",
            "LINEAR",
            &[0.0, 1.0],
            &[0.0, 1.0, 0.0, 0.0, 3.0, 0.0],
        );
        b.animation(
            "open",
            m,
            "rotation",
            "LINEAR",
            &[0.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 0.0, Q, 0.0, Q],
        );
        let bytes = b.finish();
        let g = parse(&bytes).unwrap();
        assert!(
            g.prims.is_empty(),
            "the chest and what is under it move with it"
        );
        assert_eq!(
            g.parts.len(),
            2,
            "M and P; I is the identity and adds nothing"
        );
        assert_eq!(
            (g.parts[0].rest.translation, g.parts[1].rest.translation),
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
        );
        assert_eq!(
            g.parts[0].parent,
            Some(1),
            "M hangs under P, the part after it"
        );
        assert_eq!(g.parts[1].parent, None);
        // I's triangle belongs to P, where it lies relative to P (one material: one prim).
        assert_eq!(g.parts[1].prims.len(), 1);
        assert_eq!(
            g.parts[1].prims[0].positions,
            [A[0], A[1], A[2], C[0], C[1], C[2]]
        );
        assert_eq!(g.parts[1].prims[0].indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(g.parts[0].prims, vec![as_is(B)]);
        assert_eq!(
            (g.clips[0].name.as_str(), g.clips[0].channels[0].node),
            ("lift", 1)
        );
        assert_eq!(
            (g.clips[1].name.as_str(), g.clips[1].channels[0].node),
            ("open", 0)
        );
        rest_is_flat(&bytes);
        // The rig poses with a parent after its child: half open, only the lid's matrix differs
        // from its rest.
        let rig = g.rig().unwrap();
        let rest = keyframes::model_space(&rig.parents, &rig.rest);
        let pose = |clip: usize, t: f32| {
            keyframes::model_space(
                &rig.parents,
                &keyframes::sample(&rig.clips[clip], t, false, &rig.rest),
            )
        };
        let half = pose(1, 0.5);
        assert_eq!(half[1], rest[1]);
        assert_ne!(half[0], rest[0]);
        // A quarter of the way into the lift (1 to 3 m in a second), the chest is 0.5 m higher,
        // the lid with it.
        let pose = pose(0, 0.25);
        assert!(
            (pose[1][13] - rest[1][13] - 0.5).abs() < 1e-5,
            "{}",
            pose[1][13]
        );
        assert!(
            (pose[0][13] - rest[0][13] - 0.5).abs() < 1e-5,
            "{}",
            pose[0][13]
        );
    }

    /// What `parse` says of a file it refuses; it must not panic on any of them.
    fn refused(bytes: &[u8]) -> String {
        std::panic::catch_unwind(|| parse(bytes))
            .expect("no panic")
            .expect_err("the file is refused")
    }

    /// The one-triangle model with a translation channel over two keys, `edit` on its JSON:
    /// accessor 1 holds the key times over buffer view 1, accessor 2 the values over view 2.
    fn bad_animation(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut b = Builder::new("steel");
        let m = b.triangle(A);
        let n0 = b.node(json!({"mesh": m}));
        b.animation(
            "x",
            n0,
            "translation",
            "LINEAR",
            &[0.0, 1.0],
            &[1.0, 0.0, 0.0, 2.0, 0.0, 0.0],
        );
        b.finish_json(edit)
    }

    #[test]
    fn a_bad_animation_is_dropped_not_the_model() {
        type Edit = Box<dyn FnOnce(&mut Value)>;
        let cases: Vec<(&str, Edit, &str)> = vec![
            (
                "a node that does not exist",
                Box::new(|j| j["animations"][0]["channels"][0]["target"]["node"] = json!(7)),
                "does not exist",
            ),
            (
                "one value for two keys",
                Box::new(|j| j["accessors"][2]["count"] = json!(1)),
                "keys",
            ),
            (
                "an empty accessor",
                Box::new(|j| j["accessors"][1]["count"] = json!(0)),
                "empty",
            ),
            (
                "keys beyond their buffer view",
                Box::new(|j| j["accessors"][1]["count"] = json!(5)),
                "buffer view",
            ),
            (
                "times that are not floats",
                Box::new(|j| j["accessors"][1]["componentType"] = json!(5123)),
                "key times",
            ),
            (
                "times with a width",
                Box::new(|j| j["accessors"][1]["type"] = json!("VEC2")),
                "key times",
            ),
            (
                "a stride under the element",
                Box::new(|j| j["bufferViews"][2]["byteStride"] = json!(4)),
                "byteStride",
            ),
            (
                "values with the wrong width",
                Box::new(|j| j["accessors"][2]["type"] = json!("VEC4")),
                "values",
            ),
            (
                "a buffer outside the file",
                Box::new(|j| {
                    j["buffers"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"uri": "keys.bin", "byteLength": 8}));
                    for v in [1, 2] {
                        j["bufferViews"][v]["buffer"] = json!(1);
                    }
                }),
                "embedded",
            ),
            (
                "a quantised integer translation (KHR_mesh_quantization)",
                Box::new(|j| {
                    j["accessors"][2]["componentType"] = json!(5123);
                    j["accessors"][2]["normalized"] = json!(false);
                    j["extensionsUsed"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!("KHR_mesh_quantization"));
                }),
                "not floats",
            ),
        ];
        for (what, edit, word) in cases {
            let bytes = bad_animation(edit);
            let flat = flat_parse(&bytes).unwrap();
            let g = parse(&bytes).unwrap_or_else(|e| panic!("{what}: the model is refused: {e}"));
            assert_eq!(g.prims, flat.prims, "{what}: drawn as before");
            assert!(g.parts.is_empty() && g.clips.is_empty(), "{what}");
            // The channel says why it went, then the animation it was all of.
            assert_eq!(
                g.animation_warnings.len(),
                2,
                "{what}: {:?}",
                g.animation_warnings
            );
            assert!(
                g.animation_warnings[0].contains(word),
                "{what}: {:?}",
                g.animation_warnings
            );
            assert!(g.animation_warnings[1].contains("left out"), "{what}");
            assert_eq!(g.rest_prims().as_ref(), g.prims.as_slice(), "{what}");
        }
        let one = |times: &[f32], values: &[f32], interpolation: &str| -> Vec<u8> {
            let mut b = Builder::new("steel");
            let m = b.triangle(A);
            let n0 = b.node(json!({"mesh": m}));
            b.animation("x", n0, "translation", interpolation, times, values);
            b.finish()
        };
        for (times, values, interpolation, word) in [
            (
                &[0.0, f32::NAN][..],
                &[1.0, 0.0, 0.0, 2.0, 0.0, 0.0][..],
                "LINEAR",
                "finite",
            ),
            (
                &[0.0, 1.0][..],
                &[f32::INFINITY, 0.0, 0.0, 2.0, 0.0, 0.0][..],
                "LINEAR",
                "finite",
            ),
            (&[-0.5, 1.0][..], &[1.0; 6][..], "LINEAR", "negative"),
            (&[1.0, 1.0][..], &[1.0; 6][..], "LINEAR", "ascending"),
            (&[0.0][..], &[0.0; 9][..], "CUBICSPLINE", "two keys"),
        ] {
            let bytes = one(times, values, interpolation);
            let g = parse(&bytes)
                .unwrap_or_else(|e| panic!("{interpolation} {times:?}: the model is refused: {e}"));
            assert!(
                g.clips.is_empty() && g.parts.is_empty(),
                "{interpolation} {times:?}"
            );
            let w = &g.animation_warnings;
            assert_eq!(w.len(), 2, "the channel, then the animation: {w:?}");
            assert!(w[0].contains(word), "{w:?}");
            assert!(w[1].contains("left out"), "{w:?}");
            assert_eq!(g.prims, flat_parse(&bytes).unwrap().prims);
        }
        // A rotation that is all zero, and a value beyond what a float animation may hold.
        let mut b = Builder::new("steel");
        let m = b.triangle(A);
        let n0 = b.node(json!({"mesh": m}));
        b.animation("spin", n0, "rotation", "LINEAR", &[0.0, 1.0], &[0.0; 8]);
        let g = parse(&b.finish()).unwrap();
        assert!(g.clips.is_empty() && g.animation_warnings[0].contains("zero"));
        let g = parse(&one(
            &[0.0, 1.0],
            &[1e30, 0.0, 0.0, 0.0, 0.0, 0.0],
            "LINEAR",
        ))
        .unwrap();
        assert!(
            g.animation_warnings[0].contains("1e6"),
            "{:?}",
            g.animation_warnings
        );
        // Still refused, as broken geometry is: a file that is not a GLB (a sampler that does
        // not exist fails the crate's own check), and a hierarchy that loops back on itself.
        let bytes = bad_animation(|j| j["animations"][0]["channels"][0]["sampler"] = json!(3));
        assert!(refused(&bytes).contains("GLB"));
        let mut b = Builder::new("steel");
        let m = b.triangle(A);
        let n0 = b.node(json!({"mesh": m}));
        let n1 = b.node(json!({}));
        let root = b.node(json!({}));
        b.nodes[n0]["children"] = json!([n1]);
        b.nodes[n1]["children"] = json!([n0]);
        b.nodes[root]["children"] = json!([n0]);
        assert!(refused(&b.finish()).contains("loops"));
    }

    #[test]
    fn a_clip_keeps_its_good_channels_and_loses_the_bad_ones() {
        // A hinge whose swing has a good rotation, a translation with one key time missing and
        // a scale that is all beyond the limits.
        let mut b = hinge();
        let t = b.floats("SCALAR", &[0.0, 1.0]);
        let short = b.floats("VEC3", &[0.0, 0.0, 0.0]);
        let big = b.floats("VEC3", &[2.0, 2.0, 2.0, 1e7, 1.0, 1.0]);
        b.animations[0]["samplers"].as_array_mut().unwrap().extend([
            json!({"input": t, "interpolation": "LINEAR", "output": short}),
            json!({"input": t, "interpolation": "LINEAR", "output": big}),
        ]);
        b.animations[0]["channels"].as_array_mut().unwrap().extend([
            json!({"sampler": 1, "target": {"node": b.nodes.len() - 1, "path": "translation"}}),
            json!({"sampler": 2, "target": {"node": b.nodes.len() - 1, "path": "scale"}}),
        ]);
        let g = parse(&b.finish()).unwrap();
        assert_eq!(g.parts.len(), 1);
        assert_eq!(g.clips.len(), 1, "the clip stays");
        assert_eq!(g.clips[0].channels.len(), 1, "with its rotation");
        assert_eq!(g.clips[0].channels[0].path, Path::Rotation);
        assert_eq!(g.animation_warnings.len(), 2, "{:?}", g.animation_warnings);
        assert!(
            g.animation_warnings[0].contains("keys") && g.animation_warnings[1].contains("1e6"),
            "{:?}",
            g.animation_warnings
        );
    }

    #[test]
    fn channels_that_move_nothing_here_are_left_out() {
        // A node given as a matrix, a morph target, a node outside the scene and a node the
        // scene reaches twice: the model loads as it always did.
        let mut b = Builder::new("steel");
        let (m, m2) = (b.triangle(A), b.triangle(B));
        let plain = b.node(json!({"mesh": m}));
        let matrix = b.node(json!({
            "mesh": m2,
            "matrix": [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 1.0],
        }));
        let twice = b.node(json!({"mesh": m}));
        let outside = b.node(json!({"mesh": m, "translation": [3.0, 0.0, 0.0]}));
        b.nodes[plain]["children"] = json!([twice]);
        b.nodes[matrix]["children"] = json!([twice]);
        let (t, v3, w) = (
            b.floats("SCALAR", &[0.0]),
            b.floats("VEC3", &[0.0; 3]),
            b.floats("SCALAR", &[0.0]),
        );
        b.animations.push(json!({
            "name": "mixed",
            "samplers": [{"input": t, "output": v3}, {"input": t, "output": w}, {"input": t, "output": v3}, {"input": t, "output": v3}],
            "channels": [
                {"sampler": 0, "target": {"node": matrix, "path": "translation"}},
                {"sampler": 1, "target": {"node": plain, "path": "weights"}},
                {"sampler": 2, "target": {"node": outside, "path": "translation"}},
                {"sampler": 3, "target": {"node": twice, "path": "translation"}},
            ],
        }));
        let bytes = b.finish_json(|j| j["scenes"][0]["nodes"] = json!([plain, matrix]));
        let g = parse(&bytes).unwrap();
        assert!(g.parts.is_empty() && g.clips.is_empty());
        // Each channel says why it went, then the animation.
        assert_eq!(g.animation_warnings.len(), 5, "{:?}", g.animation_warnings);
        for (i, word) in [
            "given as a matrix",
            "morph targets",
            "reaches the node 0 times",
            "reaches the node 2 times",
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                g.animation_warnings[i].contains(word),
                "{:?}",
                g.animation_warnings
            );
        }
        assert!(
            g.animation_warnings[4].contains("left out"),
            "{:?}",
            g.animation_warnings
        );
        let flat = flat_parse(&bytes).unwrap();
        assert_eq!(g.prims, flat.prims);
        assert_eq!(g.materials, flat.materials);
    }
}
