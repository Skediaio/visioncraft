//! Procedural vegetation for VisionCraft: trees, conifers, palms, shrubs, clipped topiary and
//! hedges, grasses, perennials, ferns, ground cover and climbers, generated in pure Rust as
//! renderer-agnostic meshes plus RGBA8 textures.
//!
//! * World units are metres, Y up, the plant's base at the origin, growing towards +Y.
//! * Output is deterministic: the same [`PlantParams`], seed and [`Lod`] give bit-identical
//!   meshes on every IEEE-754 platform (see [`math`] for how trigonometry is kept portable).
//! * No GPU, windowing or file I/O; builds for `wasm32-unknown-unknown`.
//!
//! Literature the generators rest on (implemented from the papers, no code or presets copied):
//! * J. Weber & J. Penn, "Creation and Rendering of Realistic Trees", SIGGRAPH 1995: recursive
//!   branching levels with down angle, rotation, curvature, crown shape envelopes, length of
//!   children along the parent, attraction up/down (weeping).
//! * P. Prusinkiewicz & A. Lindenmayer, "The Algorithmic Beauty of Plants", 1990: phyllotaxis
//!   (golden angle, whorled and decussate arrangements), rosettes, branching angles.
//! * A. Runions, B. Lane & P. Prusinkiewicz, "Modeling Trees with a Space Colonization
//!   Algorithm", 2007: the full algorithm grows broadleaf trees inside their crown envelope
//!   (`gen_tree`, with W. Palubicki et al., "Self-organizing Tree Models for Image Synthesis",
//!   SIGGRAPH 2009, for light, and the pipe model of Shinozaki et al. 1964 for radii); other
//!   branching forms use the envelope only to calibrate their recursive skeleton.
//! * Standard botanical leaf morphology (ovate, lanceolate, cordate, lobed, palmate, pinnate).

// Geometry and raster helpers take many plain scalars (positions, angles, lengths, colours);
// bundling them into structs would only add noise.
#![allow(clippy::too_many_arguments)]

pub mod math;
mod mesh;
mod params;
pub mod scan;
mod shadow;
mod species;
mod texture;

mod gen_branching;
mod gen_clipped;
mod gen_conifer;
mod gen_herb;
mod gen_palm;
mod gen_tree;
mod schema;

use std::ops::Range;

pub use params::*;
pub use schema::{ParamInfo, get_param, param_schema, set_param};
pub use shadow::{MAX_TRIANGLES as SHADOW_MAX_TRIANGLES, ShadowCaster, shadow_caster};
pub use species::{Category, Exposure, Species, species};
pub use texture::{ATLAS_CELLS, ATLAS_SIZE, BARK_SIZE};

/// Level of detail. All three come from the same seed and skeleton, so they swap without the
/// silhouette jumping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lod {
    /// Close up.
    Lod0,
    /// Mid distance.
    Lod1,
    /// Far: few large cards and a simplified trunk.
    Lod2,
}

impl Lod {
    pub const ALL: [Lod; 3] = [Lod::Lod0, Lod::Lod1, Lod::Lod2];
    pub fn index(self) -> usize {
        self as usize
    }
}

/// What a range of triangles is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Material {
    /// Opaque, back-face culled, sampled from [`PlantTextures::bark`] (UVs in [0,1], already
    /// repeated along the branch by splitting rings, so no wrap sampler is needed).
    Bark,
    /// Alpha-cut (threshold 0.5), double-sided (draw with culling off; the stored normals are
    /// bent outwards so both faces shade alike), sampled from [`PlantTextures::atlas`] and
    /// multiplied by [`PlantMesh::foliage_tint`].
    Foliage,
    /// Like `Foliage` but not tinted by season.
    Flower,
    /// Bare winter twig cards: alpha-cut, double-sided, sampled from the atlas (bark-coloured
    /// twig cells), not tinted. Only present on deciduous trees out of leaf.
    Twigs,
}

impl Material {
    pub const ALL: [Material; 4] = [
        Material::Bark,
        Material::Foliage,
        Material::Flower,
        Material::Twigs,
    ];
}

/// One material's share of a [`PlantMesh`], re-indexed to its own compact vertex arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshPart {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub wind: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub material: Material,
    /// Range into [`PlantMesh::indices`]; always a multiple of 3 long.
    pub index_range: Range<u32>,
}

/// Renderer-agnostic plant geometry. Triangles are counter-clockwise seen from outside.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlantMesh {
    pub positions: Vec<[f32; 3]>,
    /// Unit length. Bark: the surface normal. Foliage and flowers: bent towards the crown's
    /// outward direction so cards shade as one volume.
    pub normals: Vec<[f32; 3]>,
    /// In [0,1]; bark into the bark texture, foliage and flowers into the leaf atlas
    /// (origin top-left, v down).
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Per vertex: `[bend weight, phase]`. The weight is 0 at the trunk base and rises along the
    /// branch hierarchy to 1 at the outermost tips; the phase in [0,1) differs per branch (one
    /// full sway cycle = 1.0), so a tree does not move as one block.
    pub wind: Vec<[f32; 2]>,
    pub parts: Vec<Part>,
    /// Seasonal multiplier for the foliage albedo (spring flush, autumn colour). `[1,1,1]` in
    /// summer and for evergreens.
    pub foliage_tint: [f32; 3],
}

impl PlantMesh {
    /// One compact mesh per material (vertices shared between materials are duplicated, unused
    /// ones dropped), in [`Material::ALL`] order, skipping empty materials.
    pub fn split_parts(&self) -> Vec<(Material, MeshPart)> {
        let mut out = Vec::new();
        for m in Material::ALL {
            let mut part = MeshPart::default();
            let mut remap = vec![u32::MAX; self.positions.len()];
            for p in self.parts.iter().filter(|p| p.material == m) {
                for &i in &self.indices[p.index_range.start as usize..p.index_range.end as usize] {
                    let i = i as usize;
                    if remap[i] == u32::MAX {
                        remap[i] = part.positions.len() as u32;
                        part.positions.push(self.positions[i]);
                        part.normals.push(self.normals[i]);
                        part.uvs.push(self.uvs[i]);
                        part.wind.push(self.wind[i]);
                    }
                    part.indices.push(remap[i]);
                }
            }
            if !part.indices.is_empty() {
                out.push((m, part));
            }
        }
        out
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
    pub fn triangles_of(&self, material: Material) -> usize {
        self.parts
            .iter()
            .filter(|p| p.material == material)
            .map(|p| p.index_range.len() / 3)
            .sum()
    }
    /// Axis-aligned bounds `(min, max)`; zeros for an empty mesh.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        if self.positions.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }
}

/// A raw RGBA8 image, rows top to bottom, not premultiplied, sRGB colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Textures for one species.
#[derive(Clone, Debug, PartialEq)]
pub struct PlantTextures {
    /// Leaf / flower atlas, [`ATLAS_SIZE`] square, [`ATLAS_CELLS`] x [`ATLAS_CELLS`] cells.
    /// Colour outside the silhouettes is bled from the nearest edge so mipmaps do not darken.
    pub atlas: Image,
    /// Tangent-space normal map of the atlas (leaf curl and veins, the bark normal's
    /// convention), made with the finer atlases of [`textures_at`] only.
    pub atlas_normal: Option<Image>,
    /// Tileable bark albedo, [`BARK_SIZE`] square (three times that from atlas scale 2), opaque.
    pub bark: Image,
    /// Tileable tangent-space normal map for the bark (RGB = xyz * 0.5 + 0.5, +Z out of the
    /// surface, U around the branch, V along it).
    pub bark_normal: Image,
}

/// A calendar month, 1..=12 (northern hemisphere), for seasonal foliage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Season {
    pub month: u8,
}

/// Generate a plant at its peak display: full foliage, and flowers if it has any bloom months.
pub fn generate(params: &PlantParams, seed: u64, lod: Lod) -> PlantMesh {
    generate_in(params, seed, lod, None)
}

/// Generate a plant as it looks in `season`: deciduous plants are bare in winter, tinted in
/// spring and autumn; flowers appear when a bloom month falls in the month's season (March to
/// May, June to August, September to November, December to February).
pub fn generate_in_season(params: &PlantParams, seed: u64, lod: Lod, season: Season) -> PlantMesh {
    generate_in(params, seed, lod, Some(season))
}

fn generate_in(params: &PlantParams, seed: u64, lod: Lod, season: Option<Season>) -> PlantMesh {
    generate_with(params, seed, lod, season, params.budget.triangles(lod))
}

fn generate_with(
    params: &PlantParams,
    seed: u64,
    lod: Lod,
    season: Option<Season>,
    budget: usize,
) -> PlantMesh {
    let state = SeasonState::new(params, season);
    let mut mesh = match &params.form {
        Form::Branching(b) => gen_branching::generate(params, b, seed, lod, &state, budget),
        Form::Palm(p) => gen_palm::generate(params, p, seed, lod, &state, budget),
        Form::Clipped(c) => gen_clipped::generate(params, c, seed, lod, &state, budget),
        Form::Tussock(t) => gen_herb::tussock(params, t, seed, lod, &state, budget),
        Form::Rosette(r) => gen_herb::rosette(params, r, seed, lod, &state, budget),
        Form::GroundCover(g) => gen_herb::ground_cover(params, g, seed, lod, &state, budget),
        Form::Climber(c) => gen_herb::climber(params, c, seed, lod, &state, budget),
        Form::Conifer(c) => gen_conifer::generate(params, c, seed, lod, &state, budget),
    };
    settle(&mut mesh, &state, params, seed);
    debug_assert!(
        mesh.triangle_count() <= budget,
        "budget {budget} exceeded: {}",
        mesh.triangle_count()
    );
    mesh
}

/// Every level a renderer draws a plant with, nearest first. With `detail`, trees (the
/// broadleaf species grown by space colonisation) get two more levels before [`Lod::Lod0`]:
/// first every lit shoot with its leaves at true size ([`detail_triangles`]`[0]`), then the
/// spray cards at a larger budget (`[1]`); grasses, perennials and herbaceous mounds one, every
/// blade, stem and spike at true size (`[0]`). Otherwise the three [`Lod`]s. The skeleton is grown
/// once for all of them, so this is cheaper than one [`generate_in_season`] per level.
pub fn generate_levels(
    params: &PlantParams,
    seed: u64,
    season: Option<Season>,
    detail: bool,
) -> Vec<PlantMesh> {
    let Form::Branching(b) = &params.form else {
        let mut out = Vec::new();
        let near = gen_herb::detail_budget(params);
        if detail && near > 0 {
            // Herbs and grasses: one level before Lod0 with every stem, blade and spike as
            // its own geometry (`gen_herb`'s generators at the larger budget).
            out.push(generate_with(params, seed, Lod::Lod0, season, near));
        }
        out.extend(Lod::ALL.map(|lod| generate_in(params, seed, lod, season)));
        return out;
    };
    let state = SeasonState::new(params, season);
    let sk = gen_branching::skeleton(params, b, seed);
    let budgets = Lod::ALL.map(|l| params.budget.triangles(l));
    let mut out = Vec::new();
    let detail = detail && gen_branching::grown(params);
    if detail {
        let [near, sprays] = gen_tree::detail_budget(params.budget);
        out.push(gen_tree::leaf_level(params, b, &sk, seed, &state, near));
        // The spray level: the card generator's first level at a larger budget, its far
        // levels unchanged, so its sprays sit where the near card level's do.
        let t = [sprays, budgets[1], budgets[2]];
        out.push(gen_branching::cards(
            params, b, &sk, seed, 0, &state, sprays, t,
        ));
    }
    for (li, &budget) in budgets.iter().enumerate() {
        out.push(gen_branching::cards(
            params, b, &sk, seed, li, &state, budget, budgets,
        ));
    }
    for m in &mut out {
        settle(m, &state, params, seed);
    }
    out
}

/// Triangle budgets of the detail levels of [`generate_levels`]: a tree's leaf and spray
/// levels, a herb's or grass's one stem level (`[near, 0]`), `[0, 0]` for other plants.
pub fn detail_triangles(params: &PlantParams) -> [usize; 2] {
    match &params.form {
        Form::Branching(_) if gen_branching::grown(params) => {
            gen_tree::detail_budget(params.budget)
        }
        _ => [gen_herb::detail_budget(params), 0],
    }
}

/// The season's foliage tint, nothing below the ground, and a shrub's own size and shade.
fn settle(mesh: &mut PlantMesh, state: &SeasonState, p: &PlantParams, seed: u64) {
    mesh.foliage_tint = state.tint;
    if let Some((fh, fw, tint)) = gen_branching::shrub_variety(p, seed) {
        // The finished mesh is scaled, every level alike, so the levels keep agreeing (the
        // skeleton scaled before the cards were laid moved a cane shrub's far cards).
        for q in &mut mesh.positions {
            *q = [q[0] * fw, q[1] * fh, q[2] * fw];
        }
        for n in &mut mesh.normals {
            let v = math::v3(n[0] / fw, n[1] / fh, n[2] / fw).norm();
            *n = [v.x, v.y, v.z];
        }
        for k in 0..3 {
            mesh.foliage_tint[k] *= tint[k];
        }
    }
    // Leaves and drooping cards near the base may dip below the ground plane: lay them on it.
    // Anything resting on the ground does not sway: the bend weight is 0 below 2 mm and fades
    // in up to 5 mm.
    for (p, w) in mesh.positions.iter_mut().zip(mesh.wind.iter_mut()) {
        p[1] = p[1].max(0.0);
        if p[1] < 0.005 {
            w[0] *= ((p[1] - 0.002) / 0.003).clamp(0.0, 1.0);
        }
    }
}

/// Generate the textures (leaf atlas and bark) for a species.
pub fn textures(params: &PlantParams) -> PlantTextures {
    texture::generate(params)
}

/// The textures with the leaf atlas at `scale` times [`ATLAS_SIZE`] (1..=4; 2 gives 512 px
/// cells) and, above 1, its normal map.
pub fn textures_at(params: &PlantParams, scale: usize) -> PlantTextures {
    texture::generate_at(params, scale)
}

/// `textures_at` with scanned leaves and bark where `scans` has them.
pub fn textures_scanned(params: &PlantParams, scale: usize, scans: &scan::Scans) -> PlantTextures {
    texture::generate_scanned(params, scale, scans)
}

/// Whether a plant shown in `month` flowers: a month stands for its season (March to May, June
/// to August, September to November, December to February). The app shows a season by one day
/// of it (21 June for summer), and a plant that flowers in July or August belongs in the summer
/// picture too.
pub fn in_bloom(p: &PlantParams, month: u8) -> bool {
    let season_of = |m: u8| (m % 12) / 3;
    p.bloom_months
        .iter()
        .any(|&b| season_of(b) == season_of(month))
}

/// What the season does to a plant.
pub(crate) struct SeasonState {
    /// Fraction of foliage cards kept.
    pub leaf_fraction: f32,
    pub tint: [f32; 3],
    pub flowers: bool,
}

impl SeasonState {
    fn new(p: &PlantParams, season: Option<Season>) -> SeasonState {
        let Some(Season { month }) = season else {
            return SeasonState {
                leaf_fraction: 1.0,
                tint: [1.0; 3],
                flowers: !p.bloom_months.is_empty(),
            };
        };
        let flowers = in_bloom(p, month);
        if !p.deciduous {
            return SeasonState {
                leaf_fraction: 1.0,
                tint: [1.0; 3],
                flowers,
            };
        }
        let base = p.leaves.colour;
        let autumn = p.leaves.autumn_colour;
        let to_autumn = |k: f32| {
            let f = |a: f32, b: f32| math::lerp(1.0, b / a.max(0.02), k).clamp(0.2, 4.0);
            [
                f(base[0], autumn[0]),
                f(base[1], autumn[1]),
                f(base[2], autumn[2]),
            ]
        };
        let dry = [
            0.55 / base[0].max(0.02),
            0.36 / base[1].max(0.02),
            0.2 / base[2].max(0.02),
        ];
        let (leaf_fraction, tint) = match month {
            12 | 1 | 2 | 3 if p.marcescent => (0.85, dry),
            12 | 1 | 2 | 3 => (0.0, [1.0; 3]),
            4 => (0.55, [1.15, 1.2, 0.85]),
            // Hornbeam and beech (which keep their dead leaves) are well turned by late October.
            10 if p.marcescent => (0.95, to_autumn(0.85)),
            10 => (0.9, to_autumn(0.6)),
            11 if p.marcescent => (0.85, to_autumn(1.0)),
            11 => (0.3, to_autumn(1.0)),
            _ => (1.0, [1.0; 3]),
        };
        SeasonState {
            leaf_fraction,
            tint,
            flowers,
        }
    }
}
