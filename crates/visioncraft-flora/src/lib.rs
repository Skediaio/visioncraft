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
//!   Algorithm", 2007: the idea that a crown envelope bounds the branch system; used here as the
//!   crown-envelope calibration rather than as the full colonisation algorithm.
//! * Standard botanical leaf morphology (ovate, lanceolate, cordate, lobed, palmate, pinnate).

// Geometry and raster helpers take many plain scalars (positions, angles, lengths, colours);
// bundling them into structs would only add noise.
#![allow(clippy::too_many_arguments)]

pub mod math;
mod mesh;
mod params;
mod species;
mod texture;

mod gen_branching;
mod gen_clipped;
mod gen_herb;
mod gen_palm;

use std::ops::Range;

pub use params::*;
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
    /// Tileable bark albedo, [`BARK_SIZE`] square, opaque.
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
/// spring and autumn; flowers appear only in the bloom months.
pub fn generate_in_season(params: &PlantParams, seed: u64, lod: Lod, season: Season) -> PlantMesh {
    generate_in(params, seed, lod, Some(season))
}

fn generate_in(params: &PlantParams, seed: u64, lod: Lod, season: Option<Season>) -> PlantMesh {
    let state = SeasonState::new(params, season);
    let budget = params.budget.triangles(lod);
    let mut mesh = match &params.form {
        Form::Branching(b) => gen_branching::generate(params, b, seed, lod, &state, budget),
        Form::Palm(p) => gen_palm::generate(params, p, seed, lod, &state, budget),
        Form::Clipped(c) => gen_clipped::generate(params, c, seed, lod, &state, budget),
        Form::Tussock(t) => gen_herb::tussock(params, t, seed, lod, &state, budget),
        Form::Rosette(r) => gen_herb::rosette(params, r, seed, lod, &state, budget),
        Form::GroundCover(g) => gen_herb::ground_cover(params, g, seed, lod, &state, budget),
        Form::Climber(c) => gen_herb::climber(params, c, seed, lod, &state, budget),
    };
    mesh.foliage_tint = state.tint;
    // Leaves and drooping cards near the base may dip below the ground plane: lay them on it.
    for p in &mut mesh.positions {
        p[1] = p[1].max(0.0);
    }
    debug_assert!(
        mesh.triangle_count() <= budget,
        "budget {budget} exceeded: {}",
        mesh.triangle_count()
    );
    mesh
}

/// Generate the textures (leaf atlas and bark) for a species.
pub fn textures(params: &PlantParams) -> PlantTextures {
    texture::generate(params)
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
        let flowers = p.bloom_months.contains(&month);
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
