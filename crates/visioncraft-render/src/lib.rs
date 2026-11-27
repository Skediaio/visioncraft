//! VisionCraft's renderer: plain mesh, texture, material, instance and light
//! data in, a tone-mapped sRGB texture out. No UI, no window, WebGPU core
//! features and default limits only.
//!
//! Passes per frame, in order (see `Renderer::render`); "render size" is
//! the output size times `Quality::render_scale`:
//! 0. when the sky changed: the physical sky's sky-view LUT (`Rgba16Float`
//!    192x108, atmosphere.rs), the ambient SH (`Rgba32Float` 9x1, copied into
//!    the globals) and, two mips per frame, the environment cube
//!    (`Rgba16Float` 64^2 x 6 faces x 6 mips, prefiltered per roughness)
//! 1. shadow cascades, depth only, `Depth32Float` 2D array (one layer each);
//!    cutout materials alpha-test, glass stipples, wind animates (not in the
//!    cached far cascade); with 3+ cascades the middle ones alternate frames.
//!    A moving sun (`set_sun`) keeps every cascade's map and matrix and marks them stale: the
//!    near one still draws every frame, the far cached one every 4th, the middle ones on their
//!    alternate frames, and the frame after the sun stops all stale ones draw, so a settled
//!    picture is exact (`cascade_refresh`, `refresh_shadows`). Only a scene change, a resize or
//!    a quality or distance change drops the caches (`invalidate_shadows`)
//! 2. depth pre-pass, `Depth32Float` (4x MSAA when enabled); cutout
//!    materials write their coverage through the sample mask
//! 3. depth resolve: sample 0 -> linear view depth, `R32Float`
//! 4. AO + contact shadows at half (ULTRA: full) render size, `Rg8Unorm`, depth-aware blur (H, V)
//! 5. planar reflections of water (when enabled): the visible water surfaces
//!    are ranked every frame (`water.rs`: planes by height, scored by what
//!    they cover of the screen and how near they are); the best
//!    `Quality::water_planes` of them each draw the scene mirrored about
//!    their height into one layer of an `Rgba16Float` array at render size /
//!    divisor (coarser LODs, no screen-space effects, scissored to where the
//!    plane shows), over one `Depth32Float` cleared per pass. Every other
//!    water surface reflects the opaque scene of the frame along its
//!    reflected ray when `screen_reflections` is on, else the environment
//!    cube (step 6)
//! 6. forward PBR, `Rgba16Float` (MSAA, resolved into a 1x target): opaque
//!    and cutout geometry (cutout with an Equal depth test), the infinite
//!    ground, the sky, then glass, water and alpha-blended materials sorted
//!    back to front (not in the depth pre-pass, so not in AO and not
//!    depth-writing); distance haze on everything
//!    Then the air (shafts.wgsl), when any of it is on: light shafts (a march against the
//!    cascades at half size, `Rgba16Float`, blurred across and down), the sun's glare (its
//!    disc's openness into one texel), the aerial perspective LUT (`Rgba16Float` 32^3 as
//!    1024x32, atmosphere.wgsl), composited onto the HDR image in one full-size pass
//! 7. bloom: 4 half-size `Rgba16Float` levels down, then back up
//! 8. AgX tone map (bloom mixed in) to 8-bit sRGB-encoded values: straight
//!    into the output, or into an LDR target at render size for
//! 9. FXAA (when enabled) and
//! 10. the upscale to the output size with RCAS sharpening (render scale < 1)
//! 11. plan underlays (`set_underlays`): flat, unlit images alpha-blended onto the output,
//!     depth-tested against the scene
//! 12. overlay lines and points, alpha-blended onto the output (`Rgba8UnormSrgb`)
//!
//! MSAA: only the pre-pass and forward pass are multisampled. AO, contact
//! shadows, water depth and the overlay's depth test use sample 0 of each
//! pixel (step 3).
//!
//! Exposure is manual (`set_exposure`, EV100); nothing adapts on its own.
//!
//! Budget (bench: `examples/bench.rs`, 200k static triangles + 5000
//! alpha-cut, textured, wind-animated 500-triangle shrubs at mixed levels of
//! detail, 1920x1080, native offscreen, Intel UHD 620): see that example.

use std::cell::Cell;
use std::iter::once;
use std::num::NonZeroU64;

use glam::{Mat4, Vec2, Vec3, Vec4};

mod atmosphere;
mod batch;
pub mod bc;
mod clouds;
mod coverage;
pub use bc::UploadJob;
pub mod demo;
mod env;
mod grass;
pub mod hardware;
mod hdri;
mod impostor;
mod lightning;
pub mod lights;
pub mod lod;
mod sea;
pub mod shadow;
pub mod sky;
#[cfg(not(target_arch = "wasm32"))]
mod stage;
mod stats;
mod still;
mod terrain;
mod texture;
mod timing;
pub mod tonemap;
mod underlay;
mod water;
pub mod wind;

pub use atmosphere::{
    Atmosphere, fog_transmittance, mist_extinction, mist_sun_share, rain_extinction,
};
pub use grass::{GrassField, GrassKind, MAX_KINDS as GRASS_KINDS};
pub use hardware::{GpuKind, HardwareProbe, Score, calibrate, relative_cost, starting_quality};
pub use hdri::{HdriCalibration, HdriImage, OVERCAST_LUX, calibrate_hdri, estimate_sun};
pub use impostor::{ImpostorId, atlas_layout, impostor_views};
pub use lights::{Light, LightKind};
pub use sea::Sea;
pub use sky::Sun;
pub use stats::{RenderStats, TARGET_MS, ULTRA_HEADROOM, WaterItem, WaterSurface, suggest_quality};
pub use still::Still;
pub use still::{FOCUS_RANGE, Focus, Lens, Photo, lens_point, lens_view, pixel_jitter};
pub use terrain::{TerrainChunk, TerrainLayer};
pub use texture::{ColorSpace, TextureDesc, TextureId, mip_count};
pub use tonemap::{ColorGrade, WhiteBalance};
pub use underlay::UnderlayQuad;
pub use water::WaterTier;
pub use wind::Wind;

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const LINEAR_DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
const OCCLUSION: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;
const MSAA_SAMPLES: u32 = 4;
/// Uniform slot size: WebGPU's default `minUniformBufferOffsetAlignment`.
const SLOT: u64 = 256;
const VERTEX_STRIDE: u64 = 44; // position, normal, face id, uv, wind
const MATERIAL_SIZE: u64 = 112;
const CASCADE_SIZE: u64 = 80;
const AO_RADIUS: f32 = 0.6; // metres
/// Staging chunk of the upload belt: a streamed step (256 KB a frame) with row padding fits.
#[cfg(not(target_arch = "wasm32"))]
const UPLOAD_CHUNK: u64 = 1 << 20;
const AO_INTENSITY: f32 = 1.5;
/// Bloom levels (half size down to 1/32).
/// Four half-size levels (render size / 2 .. / 16): a fifth (/ 32) widened
/// the halo by a margin no one saw and cost two of a frame's ~22 passes.
const BLOOM_LEVELS: usize = 4;
/// The light shafts' resolution divisor of the render size.
const SHAFTS_DIV: u32 = 2;
/// How far the shafts' march reaches at most (m; the shadow distance otherwise).
const SHAFTS_REACH_M: f32 = 30.0;
/// The sun's glare at `set_sun_glare(1)`: the share of the sun's light a lens scatters round
/// it, and its two lobes (shafts.wgsl `glare_lobe`): a narrow core (rad), a wide halo (rad)
/// and the halo's share. A share of a few per cent is what a clean camera lens and the eye
/// show (Spencer et al. 1995 put the eye's scattered light at about 10 % within 10 degrees).
const SUN_GLARE: [f32; 4] = [0.03, 0.025, 0.14, 0.35];

/// How often the camera-centred far cascade is redrawn while the sun moves (frames).
const FAR_SUN_PERIOD: u64 = 4;

/// Whether cascade `c` of `count` is drawn this frame. The near cascade every frame; the middle
/// ones on alternate frames; the cached far one (`cached_far`) only when it is not `reuse`d (the
/// camera left its margin) or was never drawn. A cascade a sun change left `stale` is still
/// sampled with the matrix it was drawn with (a picture of an earlier sun): while the sun keeps
/// moving (`sun_moving`) the far one is redrawn every `FAR_SUN_PERIOD` frames and the middle ones
/// keep their alternate frames; the frame after the sun stops, every stale one is redrawn.
#[allow(clippy::too_many_arguments)]
fn cascade_refresh(
    c: usize,
    count: usize,
    cached_far: bool,
    reuse: bool,
    rendered: bool,
    frame: u64,
    sun_moving: bool,
    stale: bool,
    sun_lit: bool,
) -> bool {
    if !sun_lit {
        return false;
    }
    if cached_far {
        return !reuse
            || !rendered
            || (stale && (!sun_moving || frame.is_multiple_of(FAR_SUN_PERIOD)));
    }
    c == 0
        || count < 3
        || !rendered
        || (stale && !sun_moving)
        || (frame + c as u64).is_multiple_of(2)
}

/// The light matrix of the cached far cascade this frame, and whether the cache takes it: a far
/// cascade that draws (`draws`: left the margin, never drawn, or stale) uses the `fresh` matrix,
/// made for the current sun and eye, and replaces the cache; one that does not keeps the `cached`.
fn far_matrix(draws: bool, fresh: Mat4, cached: Option<Mat4>) -> (Mat4, bool) {
    if draws {
        (fresh, true)
    } else {
        (cached.unwrap_or(fresh), false)
    }
}

/// What `Renderer::render_part` draws of a frame.
#[derive(Clone, Copy)]
pub(crate) enum FramePart<'a> {
    Whole,
    /// Up to the HDR image with the air in it, before bloom and the tone map.
    Scene,
    /// The post chain on the HDR image this bind group's pass (`still::develop`) fills.
    Develop(&'a wgpu::BindGroup),
}

/// What of the air runs this frame (`Renderer::air`): the shafts' strength and the glare's
/// (0 off), and the aerial perspective.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Air {
    shafts: f32,
    glare: f32,
    aerial: bool,
}

impl Air {
    fn any(&self) -> bool {
        self.shafts > 0.0 || self.glare > 0.0 || self.aerial
    }
}
/// Solar illuminance at the top of the atmosphere (lux; Filament's
/// documentation, "Physically based units"), in kcd/m2 per unit of the
/// physical sky's LUTs.
const SUN_TOP_KLX: f32 = 128.0;
/// Instances are grouped into square ground cells of this size (metres) so
/// shadow cascades and the camera can skip the ones they do not see, and
/// levels of detail switch per cell.
const CLUSTER_CELL: f32 = 8.0;
/// The ground cell an instance's origin lies in. `set_instances` sorts an item's instances
/// by it (stably) and groups the ones of a cell into a cluster, and `instance_order` gives
/// that same order, so nothing can sort them two ways.
fn cluster_cell(m: &Mat4) -> (i32, i32) {
    (
        (m.w_axis.x / CLUSTER_CELL).floor() as i32,
        (m.w_axis.z / CLUSTER_CELL).floor() as i32,
    )
}

/// Cluster cells a side of a block (`Block`): 128 m, the coarse level the culling tests first.
const BLOCK_CELLS: i32 = 16;
/// `set_time` wraps here (seconds) to keep f32 precision in the shaders.
const TIME_WRAP: f64 = 4096.0;
/// Byte offset of the SH in the globals (written by the GPU, see env.rs): 9 matrices and the
/// vec4s before it.
const SH_OFFSET: u64 = 9 * 64 + 67 * 16;
/// The roughness up to which a metal is a mirror (`Material::mirror`, scene.wgsl `light`).
const MIRROR_ROUGHNESS: f32 = 0.1;

const COMMON: &str = include_str!("common.wgsl");
const SKY: &str = include_str!("sky.wgsl");
const POST: &str = include_str!("post.wgsl");
/// Every shader module as (name, source parts); WGSL has no includes, so the
/// shared declarations are prepended. Also validated with naga in a unit test.
const SHADERS: [(&str, &[&str]); 14] = [
    (
        "scene",
        &[
            COMMON,
            include_str!("scene.wgsl"),
            include_str!("terrain.wgsl"),
            include_str!("impostor.wgsl"),
            include_str!("sea.wgsl"),
            include_str!("wet.wgsl"),
        ],
    ),
    ("sky", &[COMMON, SKY]),
    ("depth", &[COMMON, include_str!("depth.wgsl")]),
    ("ssao", &[COMMON, include_str!("ssao.wgsl")]),
    ("blur", &[COMMON, include_str!("blur.wgsl")]),
    ("tonemap", &[COMMON, POST, include_str!("tonemap.wgsl")]),
    ("overlay", &[COMMON, include_str!("overlay.wgsl")]),
    ("underlay", &[COMMON, include_str!("underlay.wgsl")]),
    ("env", &[COMMON, SKY, include_str!("env.wgsl")]),
    ("mip", &[include_str!("mip.wgsl")]),
    (
        "atmosphere",
        &[COMMON, SKY, include_str!("atmosphere.wgsl")],
    ),
    ("post", &[COMMON, POST]),
    ("dilate", &[include_str!("dilate.wgsl")]),
    ("shafts", &[COMMON, include_str!("shafts.wgsl")]),
];

/// Triangle mesh in plain slices. `normals` match `positions`; `face_ids`
/// (optional, per vertex) name the CAD face each triangle belongs to.
/// Tangents are not needed: normal maps build their frame from screen-space
/// derivatives. A plant from `visioncraft-flora` (one vertex array, parts as
/// index ranges) becomes one `Mesh` per part: the same vertex slices with
/// `indices` sliced to the part's range.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mesh<'a> {
    pub positions: &'a [[f32; 3]],
    pub normals: &'a [[f32; 3]],
    pub indices: &'a [u32],
    pub face_ids: Option<&'a [u32]>,
    /// Texture coordinates (origin top-left, v down), for `Projection::Uv`.
    pub uvs: Option<&'a [[f32; 2]]>,
    /// Per vertex: bend weight 0..1 (0 at the trunk base, 1 at the tips) and
    /// phase in cycles; drives the wind animation.
    pub wind: Option<&'a [[f32; 2]]>,
}

/// How a material's maps are mapped onto the surface.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Projection {
    /// The mesh's UVs (library assets, generated plants).
    #[default]
    Uv,
    /// Projected along the three world axes and blended by the normal (CAD
    /// and drawn geometry without UVs); one texture repeat is `scale_m`.
    Triplanar { scale_m: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum AlphaMode {
    #[default]
    Opaque,
    /// Alpha-tested at `threshold` (base colour alpha times map alpha), in
    /// every pass: shadows and AO see the cut shape.
    Cutout { threshold: f32 },
    /// Alpha-blended, sorted, drawn after opaque geometry; no shadow, no AO.
    Blend,
}

/// Special surface models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Surface {
    #[default]
    Standard,
    /// Thin architectural glass: `base_color.rgb` is the tint, `base_color.a`
    /// the opacity (0 = clear). Blended, sorted, casts no shadow.
    Glass,
    /// Flat water (pond, pool): `base_color.rgb` is the apparent albedo of deep
    /// water (dark: ~0.02-0.05),
    /// `base_color.a` the turbidity 0..1. Blended, sorted, casts no shadow.
    Water,
}

/// Metallic-roughness material (glTF 2.0 core parameters) with optional
/// maps and VisionCraft's artist controls.
///
/// Maps: base colour (sRGB, alpha used by cutout/blend), normal (tangent
/// space, OpenGL convention: green up), packed occlusion-roughness-metallic
/// (glTF: R = AO, G = roughness, B = metallic), and separate roughness,
/// metallic and AO maps (their red channel), as MaterialCraft and ambientCG
/// deliver them. Separate and packed maps are sampled separately and
/// multiplied (each absent map is a neutral 1), not packed on upload.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Linear RGBA.
    pub base_color: [f32; 4],
    pub metallic: f32,
    /// Perceptual roughness, 0..1.
    pub roughness: f32,
    pub base_color_map: Option<TextureId>,
    pub normal_map: Option<TextureId>,
    pub orm_map: Option<TextureId>,
    pub roughness_map: Option<TextureId>,
    pub metallic_map: Option<TextureId>,
    pub occlusion_map: Option<TextureId>,
    /// Height map (red, white = top) for parallax occlusion and self-shadowing. It shares the
    /// occlusion map's binding (the forward stage is at the 16 sampled textures WebGPU
    /// guarantees), so it is used only when `occlusion_map` is absent: a packed `orm_map`
    /// carries the occlusion then.
    pub height_map: Option<TextureId>,
    /// Depth of the height map's relief in metres (0: off).
    pub parallax_depth_m: f32,
    /// Linear emissive colour, scaled by `emissive_strength` (cd/m2).
    pub emissive: [f32; 3],
    pub emissive_strength: f32,
    pub projection: Projection,
    /// Normal map strength (1 = as authored, 0 = flat).
    pub relief: f32,
    /// Roughness offset: positive is glossier (roughness - gloss).
    pub gloss: f32,
    /// 0..1: darkens and desaturates occluded and downward-facing areas and
    /// tints up-facing ones green (procedural, not physical).
    pub weathering: f32,
    /// How strongly dirt, moss, wear and rain streaks age the surface, each 0..1 (all 0: none,
    /// and the shader skips it). Procedural, in world space, on standard surfaces; it changes
    /// albedo, roughness and metallic only.
    pub aging: [f32; 4],
    /// True north on the ground as a unit direction (world x, z): moss favours the wall side
    /// facing it (the shaded side in the northern hemisphere). -Z unless the site turns it.
    pub aging_north: [f32; 2],
    pub alpha_mode: AlphaMode,
    /// Draw both sides (back faces shade with the flipped normal).
    pub double_sided: bool,
    /// Leaves: wrapped diffuse plus light through the leaf from behind;
    /// normals are used as given on both sides (bent normals).
    pub foliage: bool,
    pub surface: Surface,
    /// 0..1: large-scale variation in colour and roughness over several
    /// metres (world-space noise), against the carpet look of big flat
    /// single-material surfaces such as lawns and paving.
    pub variation: f32,
    /// Mowing stripes: width in centimetres in the low 16 bits, direction in degrees clockwise
    /// from north in the high 16; 0 for none.
    pub stripes: u32,
    /// Puddles may form on this surface when it is wet (`set_wet`): flat, open, upward-facing
    /// ground. False on lawns, which stay damp but hold no standing water. Plant leaves, glass and
    /// water never take puddles.
    pub puddles: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.7,
            base_color_map: None,
            normal_map: None,
            orm_map: None,
            roughness_map: None,
            metallic_map: None,
            occlusion_map: None,
            height_map: None,
            parallax_depth_m: 0.0,
            emissive: [0.0; 3],
            emissive_strength: 0.0,
            projection: Projection::Uv,
            relief: 1.0,
            gloss: 0.0,
            weathering: 0.0,
            aging: [0.0; 4],
            aging_north: [0.0, -1.0],
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
            foliage: false,
            surface: Surface::Standard,
            variation: 0.0,
            stripes: 0,
            puddles: true,
        }
    }
}

impl Material {
    fn transparent(&self) -> bool {
        self.surface != Surface::Standard || self.alpha_mode == AlphaMode::Blend
    }

    /// A metal polished to a mirror, which screen-space reflections show the scene in (drawn
    /// after the opaque scene, `Pass::Mirrors`).
    fn mirror(&self) -> bool {
        !self.transparent()
            && !self.cutout()
            && self.metallic >= 0.5
            && self.roughness <= MIRROR_ROUGHNESS
            && self.roughness_map.is_none()
    }

    fn cutout(&self) -> bool {
        matches!(self.alpha_mode, AlphaMode::Cutout { .. }) && !self.transparent()
    }

    /// Index into the depth / forward pipeline arrays.
    fn variant(&self) -> usize {
        self.cutout() as usize * 2 + self.double_sided as usize
    }

    fn maps(&self) -> [Option<TextureId>; 6] {
        [
            self.base_color_map,
            self.normal_map,
            self.orm_map,
            self.roughness_map,
            self.metallic_map,
            self.occlusion_map.or(self.height_map),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialId(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(u32);

impl MaterialId {
    /// Its number, as `RenderStats::water` reports it.
    pub const fn index(self) -> u32 {
        self.0
    }
}

impl ItemId {
    /// Its number, as `RenderStats::water` reports it.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// One end of an overlay segment. Consecutive pairs form segments; a pair
/// with equal positions draws a round point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineVertex {
    pub position: [f32; 3],
    /// sRGB-encoded RGBA, as UI colours usually are.
    pub color: [u8; 4],
    /// Thickness in physical pixels.
    pub width: f32,
}

/// The penumbra radius of the presets' sun shadow filter, in metres in the world: the 16-tap
/// disk's (scene.wgsl `cascade_shadow` spreads it over 1.5 to 6 texels of a cascade), the same
/// in every cascade, so it does not change at a cascade split. The 4- and 9-tap grids keep their
/// fixed texel size.
pub const DEFAULT_SHADOW_RADIUS_M: f32 = 0.03;

/// The largest penumbra of PCSS (`ShadowFilter::Pcss`) when its radius is not given: in metres
/// in the world, measured across the light (so `r / sin(elevation)` over flat ground).
pub const DEFAULT_PCSS_MAX_RADIUS_M: f32 = 0.15;

/// How the sun's cascaded shadow maps are filtered (`Quality::shadow_filter`).
///
/// `#[non_exhaustive]`: another filter may be added later (another tap count, a softer mode),
/// so outside this crate match it with a wildcard; the variants are constructed as ever.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ShadowFilter {
    /// One hardware comparison tap (2x2 bilinear): the sharpest edge.
    Hard,
    /// Percentage-closer filtering. `taps` 4 (2x2 grid at +-0.75 texel) and 9 (3x3 grid at
    /// -1.5, 0, +1.5 texels) are fixed, texel-sized kernels: gap-free, as each hardware
    /// comparison tap covers 2 texels, and they ignore `radius_m`. `taps` 16 is a Vogel disk,
    /// turned by the noise of its place in the shadow map, spread over `radius_m` metres in the
    /// world, the same in every cascade, and never narrower than 1.5 texels of a cascade (so a
    /// radius of 0 is no hard edge: `Hard` is) nor wider than 6 (beyond that the penumbra stops
    /// growing there). Each tap is compared against the receiver plane's own depth where it
    /// lands. Under `Quality::fast_shading` only the 1- and 4-tap paths are compiled (register
    /// pressure on integrated GPUs): 9 and 16 fall back to 4 there.
    Pcf { taps: u32, radius_m: f32 },
    /// Percentage-closer soft shadows (Fernando, "Percentage-Closer Soft Shadows", NVIDIA,
    /// SIGGRAPH 2005 sketch): the 16-tap disk of `Pcf` with a penumbra that grows with the
    /// distance between the receiver and its occluder. Sharp where a caster meets its shadow,
    /// and at most `max_radius_m` metres in the world (a receiver `d` metres behind its
    /// occluder, along the light, gets `d * tan(disc)`), never wider than 6 texels of a cascade
    /// and never narrower than half a texel (where the sun's disc alone gives less, the disk
    /// still rounds the edge's texel steps). The penumbra is about twice as wide as a real
    /// sun's: the sun's disc as drawn (`sky::SUN_DISC_RADIUS`, 0.5 degrees) has about twice the
    /// real sun's angular radius (0.267 degrees).
    ///
    /// The blocker search takes 16 point samples of the shadow map over up to 6 texels round
    /// the receiver, so it can miss a caster one or two texels wide in the map (a pole, a
    /// branch, a fence in a far cascade): such a pixel then reads fully lit. In a still that
    /// shows as holes in a thin caster's shadow and penumbra, a known artefact of PCSS; a photo
    /// (many frames averaged) evens it out.
    ///
    /// Falls back to the 4-tap grid under `Quality::fast_shading`, as `Pcf` with 16 taps does.
    Pcss { max_radius_m: f32 },
}

impl ShadowFilter {
    /// Comparison taps per pixel: 1, 4, 9 or 16.
    pub fn taps(self) -> u32 {
        match self {
            ShadowFilter::Hard => 1,
            ShadowFilter::Pcf { taps, .. } => taps,
            ShadowFilter::Pcss { .. } => 16,
        }
    }

    /// The penumbra's radius in the world (m), PCSS's largest; 0 for `Hard`.
    pub fn radius_m(self) -> f32 {
        match self {
            ShadowFilter::Hard => 0.0,
            ShadowFilter::Pcf { radius_m, .. } => radius_m,
            ShadowFilter::Pcss { max_radius_m } => max_radius_m,
        }
    }

    /// This filter as the renderer holds it: `taps` the nearest of 4, 9 or 16 and a radius
    /// clamped to 0..=1 (NaN: the presets' radius, for PCSS its own default), so two filters
    /// that shade the same compare equal (`Quality: Eq`).
    pub fn sanitized(self) -> Self {
        match self {
            ShadowFilter::Hard => ShadowFilter::Hard,
            ShadowFilter::Pcf { taps, radius_m } => ShadowFilter::Pcf {
                // No ties: 4, 9 and 16 are an odd number apart.
                taps: [4u32, 9, 16]
                    .into_iter()
                    .min_by_key(|&t| t.max(taps) - t.min(taps))
                    .unwrap_or(4),
                radius_m: sanitized_radius(radius_m, DEFAULT_SHADOW_RADIUS_M),
            },
            ShadowFilter::Pcss { max_radius_m } => ShadowFilter::Pcss {
                max_radius_m: sanitized_radius(max_radius_m, DEFAULT_PCSS_MAX_RADIUS_M),
            },
        }
    }
}

/// A penumbra radius as the renderer holds it: clamped to 0..=1, a NaN one `default`.
fn sanitized_radius(radius_m: f32, default: f32) -> f32 {
    let r = if radius_m.is_nan() {
        default
    } else {
        radius_m.clamp(0.0, 1.0)
    };
    // One zero, not -0.0: they compare equal, but print apart.
    if r == 0.0 { 0.0 } else { r }
}

/// Effect switches so the app can fall back on slow GPUs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    pub shadow_resolution: u32,
    /// 1..=4.
    pub cascades: u32,
    /// Ambient-occlusion taps per pixel, 0 = off.
    pub ao_samples: u32,
    pub contact_shadows: bool,
    /// 4x MSAA on the depth pre-pass and forward pass.
    pub msaa: bool,
    /// Scales every level-of-detail switch distance (percent; 100 = as
    /// given to `add_lod_item`). Lower presets switch to coarser plants
    /// sooner.
    pub lod_distance_percent: u32,
    /// 0.5..=1: the scene renders at this fraction of the output size and
    /// is upscaled (bilinear plus RCAS sharpening) before the overlay.
    pub render_scale: f32,
    /// FXAA after tone mapping (meant for presets without MSAA).
    pub fxaa: bool,
    /// Bloom (strength: `Renderer::set_bloom`).
    pub bloom: bool,
    /// Planar reflection of water: resolution divisor of the render size (2
    /// half, 4 quarter); 0 = off (water reflects the environment cube).
    pub water_reflection: u32,
    /// The sun's shadow filter: the comparison taps, and for the 16-tap filters the penumbra's
    /// radius in the world (the disk's, PCSS's largest), the same in every cascade.
    pub shadow_filter: ShadowFilter,
    /// Planar reflections of water per frame (0..=4): the visible water planes ranked by
    /// what they cover on screen and how near they are each get a mirrored pass while the
    /// budget lasts; the others reflect the opaque scene of the frame when `screen_reflections`
    /// is on, else the environment cube (`water.rs`). 0: no planar pass.
    pub water_planes: u32,
    /// Ambient occlusion and contact shadows at the full render size
    /// (otherwise half size, upsampled depth-aware).
    pub ao_full_resolution: bool,
    /// Side of one view in impostor atlases baked from now on (texels,
    /// 64..=1024); impostors baked earlier keep theirs.
    pub impostor_resolution: u32,
    /// Screen-space reflections in glass and mirrors: 0 off (they mirror the sky and the room
    /// estimate), else the ray's step in pixels (2: half resolution).
    pub screen_reflections: u32,
    /// The shaders compiled for this preset's own settings (common.wgsl `FAST_SHADING`): what
    /// the slower presets use (the 9- and 16-tap shadow filters, the ground's 16-step parallax
    /// and its self-shadow) is left out, so integrated GPUs keep their shading in registers.
    /// Changing it compiles the scene's pipelines again.
    pub fast_shading: bool,
    /// Volumetric clouds (clouds.wgsl, when `Clouds::volumetric` asks for them): the cloud
    /// image's divisor of the render size (2 half, 3 a third, 4 quarter), 0 for the flat cloud layer.
    pub clouds: u32,
    /// Grass blades generated on the GPU over the lawns and meadows (`grass.rs`, when the app
    /// gives a `GrassField`): 0 off (the app draws its own lawn tufts), 1 to 3 ever further and
    /// denser.
    pub grass: u32,
}

// `render_scale` is clamped to 0.5..=1 everywhere it is used, never NaN, so
// equality is total (presets are compared with `==`); `shadow_filter` is sanitized
// (`Quality::sanitized`) wherever a quality enters the renderer.
impl Eq for Quality {}

impl Default for Quality {
    fn default() -> Self {
        Quality::MEDIUM
    }
}

impl Quality {
    pub const LOW: Quality = Quality {
        shadow_resolution: 1024,
        cascades: 2,
        ao_samples: 0,
        contact_shadows: false,
        msaa: false,
        lod_distance_percent: 40,
        render_scale: 0.65,
        fxaa: true,
        bloom: false,
        water_reflection: 0,
        water_planes: 0,
        shadow_filter: ShadowFilter::Pcf {
            taps: 4,
            radius_m: DEFAULT_SHADOW_RADIUS_M,
        },
        ao_full_resolution: false,
        impostor_resolution: 256,
        screen_reflections: 0,
        fast_shading: true,
        clouds: 0,
        grass: 0,
    };
    pub const MEDIUM: Quality = Quality {
        shadow_resolution: 1024,
        cascades: 2,
        ao_samples: 8,
        // Contact shadows (~2 ms) are HIGH only since foliage is alpha-cut;
        // plants also switch to coarser LODs sooner than on HIGH.
        contact_shadows: false,
        msaa: false,
        lod_distance_percent: 50,
        render_scale: 0.75,
        fxaa: true,
        bloom: true,
        water_reflection: 4,
        water_planes: 1,
        shadow_filter: ShadowFilter::Pcf {
            taps: 4,
            radius_m: DEFAULT_SHADOW_RADIUS_M,
        },
        ao_full_resolution: false,
        impostor_resolution: 256,
        screen_reflections: 2,
        fast_shading: true,
        clouds: 4,
        grass: 1,
    };
    pub const HIGH: Quality = Quality {
        shadow_resolution: 2048,
        cascades: 4,
        ao_samples: 16,
        contact_shadows: true,
        msaa: true,
        lod_distance_percent: 100,
        render_scale: 1.0,
        fxaa: false,
        bloom: true,
        water_reflection: 2,
        water_planes: 2,
        shadow_filter: ShadowFilter::Pcf {
            taps: 9,
            radius_m: DEFAULT_SHADOW_RADIUS_M,
        },
        ao_full_resolution: false,
        impostor_resolution: 256,
        screen_reflections: 1,
        fast_shading: false,
        clouds: 3,
        grass: 2,
    };
    /// For GPUs with headroom (a desktop card at 100+ fps on HIGH): 4096
    /// shadow maps, full-resolution AO with more taps and a full-resolution
    /// water reflection, mesh plants twice as far before their impostors,
    /// 512-texel impostor views. Native resolution with 4x MSAA as on HIGH
    /// (WebGPU core has no 8x); cutout foliage, impostors and the LOD
    /// dissolve resolve through alpha to coverage, not a stipple.
    pub const ULTRA: Quality = Quality {
        shadow_resolution: 4096,
        cascades: 4,
        ao_samples: 24,
        contact_shadows: true,
        msaa: true,
        lod_distance_percent: 200,
        render_scale: 1.0,
        fxaa: false,
        bloom: true,
        water_reflection: 1,
        water_planes: 3,
        shadow_filter: ShadowFilter::Pcf {
            taps: 16,
            radius_m: DEFAULT_SHADOW_RADIUS_M,
        },
        ao_full_resolution: true,
        impostor_resolution: 512,
        screen_reflections: 1,
        fast_shading: false,
        clouds: 2,
        grass: 3,
    };
    pub const PRESETS: [(&'static str, Quality); 4] = [
        ("low", Quality::LOW),
        ("medium", Quality::MEDIUM),
        ("high", Quality::HIGH),
        ("ultra", Quality::ULTRA),
    ];

    fn samples(&self) -> u32 {
        if self.msaa { MSAA_SAMPLES } else { 1 }
    }

    /// The sun shadow's comparison taps the shaders run: under `fast_shading` only the 1- and
    /// 4-tap paths are compiled, so 9 and 16 (and PCSS) fall back to 4.
    fn shadow_taps(&self) -> u32 {
        let taps = self.shadow_filter.taps();
        if self.fast_shading && taps > 4 {
            4
        } else {
            taps
        }
    }

    /// `Globals.shadow_filter.xy` (common.wgsl): the penumbra's radius in the world, the
    /// 16-tap disk's or PCSS's largest, and the mode the 16-tap branch of `cascade_shadow`
    /// runs: 0 PCF, 1 PCSS. Under `fast_shading` always PCF: only the 1- and 4-tap paths are
    /// compiled there.
    fn shadow_filter_uniform(&self) -> [f32; 2] {
        let pcss = matches!(self.shadow_filter, ShadowFilter::Pcss { .. }) && !self.fast_shading;
        [self.shadow_filter.radius_m(), if pcss { 1.0 } else { 0.0 }]
    }

    /// This quality as the renderer holds it (`set_quality`, still.rs `render_offscreen` and
    /// `photo_begin`): every field the renderer indexes, divides by or compares with `==` in
    /// range, the shadow filter sanitized (`ShadowFilter::sanitized`). A preset is its own
    /// sanitized self, so setting one again changes nothing.
    fn sanitized(self) -> Quality {
        Quality {
            cascades: self.cascades.clamp(1, shadow::MAX_CASCADES as u32),
            impostor_resolution: self.impostor_resolution.clamp(64, 1024),
            render_scale: if self.render_scale.is_nan() {
                1.0
            } else {
                self.render_scale.clamp(0.5, 1.0)
            },
            water_planes: self.water_planes.min(water::MAX_PLANES),
            shadow_filter: self.shadow_filter.sanitized(),
            ..self
        }
    }

    /// The planar passes a frame may draw: `water_planes` (at most the globals' planes) while
    /// the planar reflection is on, else none.
    pub fn plane_budget(&self) -> u32 {
        if self.water_reflection == 0 {
            0
        } else {
            self.water_planes.min(water::MAX_PLANES)
        }
    }

    /// The volumetric clouds' march steps through the layer and towards the sun.
    fn cloud_steps(&self) -> (u32, u32) {
        match (self.clouds, self.fast_shading) {
            (0..=2, _) => (64, 6),
            (3, _) | (_, false) => (48, 6),
            (_, true) => (32, 5),
        }
    }
}

/// An endless ground plane at `height`, drawn wherever no geometry covers
/// the view below the horizon (a backdrop: it never hides or z-fights with
/// the scene's own ground), lit, shadowed and hazed like any surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ground {
    pub height: f32,
    /// Linear albedo.
    pub color: [f32; 3],
    pub roughness: f32,
    /// Large-scale variation, as `Material::variation`.
    pub variation: f32,
    /// The land painted and shaped on the ground beyond `plot` (scene.wgsl `surroundings`).
    pub surroundings: Surroundings,
    /// The design's plot (min x, min z, max x, max z): the surroundings keep 15 m clear of it.
    pub plot: [f32; 4],
}

impl Default for Ground {
    fn default() -> Self {
        Self {
            height: 0.0,
            color: [0.1, 0.16, 0.05],
            roughness: 0.95,
            variation: 0.6,
            surroundings: Surroundings::None,
            plot: [0.0; 4],
        }
    }
}

/// The land round a design on the endless ground: procedural colour (fields with hedgerows,
/// woods, dunes with marram grass) and relief (rolling land, a woods' canopy, hills on the
/// horizon, dune ridges), from the world position alone. Far layers only: none of it inside the
/// plot, none on a coast's sea side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surroundings {
    /// The plain ground.
    #[default]
    None = 0,
    Fields = 1,
    Woodland = 2,
    Dunes = 3,
}

/// Where the sky's light comes from.
#[derive(Clone, Copy, Debug)]
pub enum Sky<'a> {
    /// Physical sky (Hillaire 2020): blue zenith, warm horizon, real
    /// sunsets and twilight. The default.
    Physical,
    /// Preetham 1999 analytic sky: the earlier model, as a fallback.
    Preetham,
    /// A photograph of the sky (equirectangular HDRI) as background,
    /// environment and ambient light. The sun (direct light, its disc) still
    /// comes from `set_sun`: line it up with the image's sun (see
    /// `estimate_sun`), or give it zero illuminance for an overcast image.
    /// `intensity` scales pixel values to cd/m2; `calibrate_hdri` gives it
    /// (with a matching sun and exposure) for files of arbitrary scale.
    Hdri {
        image: HdriImage<'a>,
        rotation_deg: f32,
        intensity: f32,
    },
}

/// Fraction of light left after `path_m` metres through water of
/// `turbidity` 0..1 (Beer-Lambert, absorption 0.25..3 per metre). The
/// water shader uses the same curve for its depth tint.
pub fn water_transmittance(path_m: f32, turbidity: f32) -> f32 {
    let k = 0.25 + (3.0 - 0.25) * turbidity.clamp(0.0, 1.0);
    (-path_m.max(0.0) * k).exp()
}

type Bounds = (Vec3, Vec3);

struct GpuMesh {
    /// Its vertices and indices in the arenas (`Renderer::arenas`).
    place: batch::Placement,
    /// Indices drawn: the range may be longer (a replacement that fitted kept it).
    index_count: u32,
    bounds: Option<Bounds>,
    /// Any vertex with a bend weight: bounds get padded for the sway.
    wind: bool,
    /// A terrain chunk: eight u8 layer weights per vertex, in its arena's weight buffer.
    weights: bool,
}

/// One (mesh, material) of an item at one level of detail.
struct Part {
    mesh: MeshId,
    material: MaterialId,
    lod: u8,
    /// Draw uniform slot (mesh id for the highlight).
    slot: u32,
}

struct Item {
    parts: Vec<Part>,
    /// Max camera distance per level of detail, ascending.
    lods: Vec<f32>,
    /// Sorted by `CLUSTER_CELL`, so each cluster is a contiguous range.
    transforms: Vec<Mat4>,
    /// Its instances' range in the pool (`batch::InstancePool`): `cap` from `base`.
    base: u32,
    cap: u32,
    bounds: Option<Bounds>,
    clusters: Vec<Cluster>,
    /// Its clusters in 128 m blocks of the ground grid, each with the bounds of its clusters
    /// (`update_lods` steps over those of a block past the item's last level);
    /// `block_of[ci]`: the block of cluster `ci`.
    blocks: Vec<Block>,
    block_of: Vec<u32>,
    /// The bounding radius of its largest instance (m, `update_bounds`): what the reflection
    /// leaves out when it would be under `MIN_REFLECTED_PX`.
    size: f32,
    /// The baked impostor drawn as the last level, and its draw slot.
    impostor: Option<(ImpostorId, u32)>,
    /// Metres before the last level's distance over which instances dither
    /// out (0: they are culled at once).
    cull_fade: f32,
    /// The impostor's end follows `set_impostor_reach` (small plants).
    follows_reach: bool,
    /// Casts shadows with the level it is drawn with (`set_item_shadow_exact`).
    shadow_exact: bool,
    /// Casts shadows into the cascades from this index on (`set_item_shadow_cascades`); 0: all.
    shadow_from: u8,
    /// Casts shadows past its finest level with this mesh (a tree's few untextured blobs,
    /// dithered by the density in its UVs) instead of its impostor's cards.
    shadow_caster: Option<MeshId>,
    /// Sways like a conifer (`set_item_conifer`): the plant's natural frequency.
    conifer: bool,
    /// Lawn grass (`set_item_grass`): bends far more with the gusts.
    grass: bool,
    /// Listed in `Renderer::moved` (an instance moved since the last frame).
    moved: bool,
}

/// Instances in one ground cell: culled as a unit, drawn as one range, one
/// level of detail.
struct Cluster {
    range: std::ops::Range<u32>,
    bounds: Option<Bounds>,
    /// View level of detail drawn (`lods.len()` = culled by distance;
    /// `LOD_UNSET` before the first frame); while a hand-over dissolves, the
    /// nearer of its two levels.
    lod: u8,
    /// While a hand-over dissolves: the farther of its two levels (a mesh
    /// level, the impostor, or `lods.len()` for nothing).
    far: u8,
    /// The level the distance asks for (with hysteresis); differs from
    /// `lod` while a hand-over dissolves.
    target: u8,
    /// Front-to-back sort keys seen from the camera and from the sun.
    key: [f32; 2],
    /// Frames left of a dissolve between two levels (`lod` and `far`); 0
    /// when settled.
    fade: u8,
    /// The dissolve goes towards the far side.
    to_far: bool,
    /// Dissolving this frame: both sides draw, dithered by `fade_slot`.
    fading: bool,
    /// First of this frame's fade slots (one per part, then the impostor).
    fade_slot: u32,
}

/// The item's clusters of one 128 m block of the ground grid (16 x 16 cells), with their
/// bounds; which are its clusters says `Item::block_of`. `update_lods` leaves the clusters of
/// a block beyond the item's last level out.
#[derive(Default)]
struct Block {
    bounds: Option<Bounds>,
    /// `update_lods`: past the item's last level with nothing dissolving, no level there can
    /// change, and its clusters are left out (an item with an impostor draws them at that
    /// level, so theirs get their sort keys only).
    beyond: bool,
}

/// The ranges of consecutive elements of `items` that are `same` as the one before: a partition
/// of `0..items.len()`, in order.
fn same_ranges<T>(items: &[T], same: impl Fn(&T, &T) -> bool) -> Vec<std::ops::Range<u32>> {
    let mut out: Vec<std::ops::Range<u32>> = Vec::new();
    for i in 0..items.len() {
        match out.last_mut() {
            Some(r) if same(&items[i - 1], &items[i]) => r.end = i as u32 + 1,
            _ => out.push(i as u32..i as u32 + 1),
        }
    }
    out
}

impl Item {
    /// Instances `r` of the item, in the pool.
    fn pool_range(&self, r: &std::ops::Range<u32>) -> std::ops::Range<u32> {
        self.base + r.start..self.base + r.end
    }
}

/// A pipeline a planned draw uses (`Renderer::pipe`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Pipe {
    /// `Pipelines::shadow` (opaque, cutout, glass, shadow caster).
    Shadow(usize),
    Depth(usize),
    DepthFade(usize),
    /// `SampledPipelines::forward` by `Material::variant`.
    Forward(usize),
    Terrain,
    ReflectTerrain,
    Reflect(usize),
    /// `SampledPipelines::impostor`: depth, forward.
    Impostor(usize),
    ReflectImpostor,
    ImpostorShadow,
}

/// What a planned draw binds as group 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Group1 {
    /// A texture set (`Renderer::texture_sets`): every material with these maps shares it, and
    /// each instance names its own row of the material table.
    Set(u32),
    Terrain,
    Impostor(u32),
}

/// Where a planned draw's instances are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Src {
    Pool,
    /// The frame's gathered batches (`batch::Gather`).
    Gathered,
}

/// One draw of a pass, planned before the frame records (`Renderer::plan_items`).
struct Draw {
    pipe: Pipe,
    g1: Option<Group1>,
    /// Group 2: from the fade slots or the draw slots, and the slot.
    g2: Option<(bool, u32)>,
    /// The mesh drawn indexed, or `None` for impostor quads.
    mesh: Option<DrawMesh>,
    /// The mesh's indices (`DrawMesh::range`'s length) or the quads' vertices.
    vertices: u32,
    src: Src,
    instances: std::ops::Range<u32>,
    triangles: u64,
    /// Counts as instances in view (`RenderStats::instances`).
    view: bool,
}

/// A mesh a planned draw names (`Draw::mesh`): its id, its arena and its index range there, and
/// its first vertex (the base vertex of `draw_indexed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DrawMesh {
    id: u32,
    arena: u32,
    range: batch::Span,
    base_vertex: i32,
}

impl GpuMesh {
    /// What a draw of all its indices names.
    fn draw(&self, id: MeshId) -> DrawMesh {
        DrawMesh {
            id: id.0,
            arena: self.place.arena,
            range: batch::Span::new(self.place.indices.first, self.index_count),
            base_vertex: self.place.vertices.first as i32,
        }
    }
}

/// Draws that can share one: by `tag` (the category; impostors and shadow casters have their
/// own), pipeline, group 1, mesh, and whether group 2 is bound (every part's draw slot holds
/// just its mesh id, and every settled impostor's the same fade, so any of them serves).
type BatchKey = (u8, Pipe, Option<Group1>, Option<DrawMesh>, u32, bool);

struct Batch {
    tag: u8,
    draw: Draw,
    /// The nearest run's sort key.
    first: f32,
    /// Sort key, first pool instance, count and material row of each run.
    runs: Vec<(f32, u32, u32, u32)>,
}

#[derive(Default)]
struct Batches {
    index: std::collections::HashMap<BatchKey, usize, std::hash::BuildHasherDefault<Fx>>,
    list: Vec<Batch>,
}

/// A small, fast hash for the batches' keys (the standard one showed in browser profiles).
#[derive(Default, Clone, Copy)]
struct Fx(u64);

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        bytes.iter().for_each(|&b| self.write_u64(u64::from(b)));
    }
    fn write_u8(&mut self, v: u8) {
        self.write_u64(u64::from(v));
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(u64::from(v));
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

/// An item's runs within one pass, per level and whether dissolving clusters are left out
/// (`Renderer::runs`): made once for all the parts at that level.
#[derive(Default)]
struct RunCache(Vec<(u8, bool, Vec<Run>)>);

/// A run of instances with its sort key.
type Run = (f32, std::ops::Range<u32>);

impl RunCache {
    fn get(
        &mut self,
        item: &Item,
        levels: &[Option<u8>],
        order: usize,
        level: u8,
        skip_fading: bool,
    ) -> &[(f32, std::ops::Range<u32>)] {
        let i = match self
            .0
            .iter()
            .position(|e| e.0 == level && e.1 == skip_fading)
        {
            Some(i) => i,
            None => {
                let runs = Renderer::runs(item, order, levels, level, skip_fading);
                self.0.push((level, skip_fading, runs));
                self.0.len() - 1
            }
        };
        &self.0[i].2
    }
}

impl Batches {
    /// Adds `item`'s `runs` (of its own instances, with sort keys) of parts with material `row`
    /// to the batch of `draw`.
    fn add(
        &mut self,
        tag: u8,
        draw: Draw,
        item: &Item,
        row: u32,
        runs: &[(f32, std::ops::Range<u32>)],
    ) {
        if runs.is_empty() {
            return;
        }
        let key = (
            tag,
            draw.pipe,
            draw.g1,
            draw.mesh,
            draw.vertices,
            draw.g2.is_some(),
        );
        let list = &mut self.list;
        let i = *self.index.entry(key).or_insert_with(|| {
            list.push(Batch {
                tag,
                draw,
                first: f32::INFINITY,
                runs: Vec::new(),
            });
            list.len() - 1
        });
        let b = &mut self.list[i];
        for (k, r) in runs {
            let k = *k;
            b.first = b.first.min(k);
            b.runs.push((k, item.base + r.start, r.len() as u32, row));
        }
    }

    /// One draw per batch (by tag and pipeline, then front to back), all gathered: each run's
    /// pool indices go into `gather` with its material row, nearest first (a single run too: the
    /// draw then needs no slot with a material, and every batch draws the same way).
    fn emit(&mut self, draws: &mut Vec<Draw>, gather: &mut batch::GatherList) {
        self.index.clear();
        let mut list = std::mem::take(&mut self.list);
        list.sort_by(|a, b| {
            (a.tag, a.draw.pipe)
                .cmp(&(b.tag, b.draw.pipe))
                .then(a.first.total_cmp(&b.first))
        });
        for mut b in list {
            b.runs.sort_by(|x, y| x.0.total_cmp(&y.0));
            let start = gather.indices.len() as u32;
            for &(_, first, n, row) in &b.runs {
                gather.indices.extend(first..first + n);
                gather
                    .materials
                    .extend(std::iter::repeat_n(row, n as usize));
            }
            let mesh = b.draw.mesh.map_or(FROM_INSTANCE, |m| m.id);
            b.draw.instances = gather.batch(start, mesh);
            b.draw.src = Src::Gathered;
            draws.push(b.draw);
        }
    }
}

/// A frame's planned passes (`Renderer::render_part`).
struct FramePlans {
    /// Per shadow layer (empty where the cascade is not refreshed this frame).
    shadows: Vec<Vec<Draw>>,
    snow: Option<Vec<Draw>>,
    depth: Vec<Draw>,
    /// Per layer of the planar reflection (empty: none): the scene mirrored about that
    /// plane.
    reflections: Vec<Vec<Draw>>,
    forward: Vec<Draw>,
    mirrors: Option<Vec<Draw>>,
    /// Where each pass's indirect arguments start (`Renderer::write_indirect`): the shadow
    /// layers, the snow cover map, the depth pre-pass, one slot per water plane whether it has
    /// a pass this frame or not (`water::MAX_PLANES`), the forward pass, the mirrors.
    args: Vec<u32>,
}

/// Bytes of one `DrawIndexedIndirectArgs` (five u32).
const INDIRECT_ARGS: u32 = 20;

/// A batched mesh draw can go through the indirect buffer (`Calls`).
fn indirectable(d: &Draw) -> bool {
    d.src == Src::Gathered && d.mesh.is_some()
}

/// Whether two draws share what one indirect call keeps: the pipeline, group 1 and group 2, and
/// the arena, whose vertex and index buffers the pass binds for the run's first draw.
fn same_bindings(a: &Draw, b: &Draw) -> bool {
    a.pipe == b.pipe
        && a.g1 == b.g1
        && a.g2 == b.g2
        && a.mesh.map(|m| m.arena) == b.mesh.map(|m| m.arena)
}

/// The indirect arguments of a batched mesh draw: what its direct `draw_indexed` names.
fn draw_args(d: &Draw) -> [u32; 5] {
    let m = d.mesh.expect("a mesh draw");
    [
        m.range.count,
        d.instances.len() as u32,
        m.range.first,
        m.base_vertex as u32,
        d.instances.start,
    ]
}

/// How `record` issues planned draws, in their order: each on its own, or a run of two or more
/// batched mesh draws with the same bindings as one indirect call (whose sub-draws are the
/// run's draws). Yields (first draw, count).
#[derive(Clone, Copy)]
struct Calls<'a> {
    draws: &'a [Draw],
    at: usize,
    /// Runs may be indirect calls (the device's `INDIRECT_FIRST_INSTANCE`).
    indirect: bool,
}

impl<'a> Calls<'a> {
    fn new(draws: &'a [Draw], indirect: bool) -> Self {
        Self {
            draws,
            at: 0,
            indirect,
        }
    }
}

impl Iterator for Calls<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let i = self.at;
        let draws = self.draws;
        let d = draws.get(i)?;
        let mut n = 1;
        if self.indirect && indirectable(d) {
            while draws
                .get(i + n)
                .is_some_and(|next| indirectable(next) && same_bindings(d, next))
            {
                n += 1;
            }
        }
        self.at = i + n;
        Some((i, n))
    }
}

/// Appends the arguments of every draw `record` sends through an indirect call.
fn pack_args(out: &mut Vec<u8>, draws: &[Draw], indirect: bool) {
    for (at, n) in Calls::new(draws, indirect) {
        if n > 1 {
            for d in &draws[at..at + n] {
                out.extend_from_slice(&u32_bytes(&draw_args(d)));
            }
        }
    }
}

const LOD_UNSET: u8 = u8::MAX;
/// What `cluster_lod` answers for a cluster drawn by its item's shadow caster.
const SHADOW_CASTER_LEVEL: u8 = u8::MAX - 1;
/// The smallest bounding radius in pixels the planar water reflection draws a cluster's
/// instances with: under it the rippled reflection of a herb, a stone or a chair far off shows
/// nothing, and leaving them out spared the pass most of its triangles.
const MIN_REFLECTED_PX: f32 = 1.5;
/// Frames a hand-over between two levels (two mesh levels, the last mesh
/// level and the impostor, or a cull fade) dissolves over: the dither shows
/// only while it runs, never as a steady stipple on a parked camera.
const FADE_FRAMES: u8 = 16;
/// Draw slots for this frame's dissolving clusters; more dissolving at once
/// switch without one.
const FADE_SLOTS: usize = 1024;

struct FarCache {
    eye: Vec3,
    proj: Mat4,
    matrix: Mat4,
}

/// The endless ground's own material: row 0 of the material table.
const GROUND_ROW: u32 = 0;

/// The mesh id, or material row, of a draw slot whose draw takes it from each instance instead
/// (`Gather`; `scene.wgsl` `FROM_INSTANCE`).
const FROM_INSTANCE: u32 = u32::MAX;

struct GpuMaterial {
    /// Its texture set: the bind group of group 1 (`Renderer::texture_sets`).
    set: u32,
    cpu: Material,
    /// Its base map is bound (the impostor bake picks its pipeline by it).
    has_base: bool,
    /// The identity instance the impostor bake draws its parts with, carrying the material's
    /// row as a gathered instance does (`write_instances` leaves it out of the pool).
    identity: wgpu::Buffer,
}

/// One bind group of group 1: a material table and six maps with their samplers.
struct GpuTextureSet {
    bind: wgpu::BindGroup,
    /// The maps it binds, to make its bind group again when the table grows.
    maps: [Option<TextureId>; 6],
}

/// The texture sets in use and how many materials use each (index 0, never given back, binds
/// no map: every untextured material's).
#[derive(Default)]
struct TextureSets {
    index:
        std::collections::HashMap<[Option<TextureId>; 6], u32, std::hash::BuildHasherDefault<Fx>>,
    users: Vec<usize>,
    free: Vec<u32>,
}

impl TextureSets {
    /// The set that binds `maps`, counted one user more: a new one (a freed index, or the next)
    /// when none does.
    fn acquire(&mut self, maps: [Option<TextureId>; 6]) -> (u32, bool) {
        if let Some(&i) = self.index.get(&maps) {
            self.users[i as usize] += 1;
            return (i, false);
        }
        let i = self.free.pop().unwrap_or(self.users.len() as u32);
        if i as usize == self.users.len() {
            self.users.push(0);
        }
        self.users[i as usize] = 1;
        self.index.insert(maps, i);
        (i, true)
    }

    /// One user fewer of the set `set` of `maps`: whether it was the last (index 0 stays).
    fn release(&mut self, maps: [Option<TextureId>; 6], set: u32) -> bool {
        let users = &mut self.users[set as usize];
        *users -= 1;
        if *users > 0 || set == 0 {
            return false;
        }
        self.index.remove(&maps);
        self.free.push(set);
        true
    }
}

/// The row of material `id` in the material table.
fn row(id: MaterialId) -> u32 {
    id.0 + 1
}

struct Layouts {
    /// Group 0 of every pass but the environment's: globals, environment
    /// cube and sampler, light list and grid, sky source.
    globals: wgpu::BindGroupLayout,
    /// Just the globals uniform (the sky LUT passes).
    globals_only: wgpu::BindGroupLayout,
    /// Globals, sampler and sky source (the environment passes, which
    /// render the cube and so cannot bind it).
    sky_source: wgpu::BindGroupLayout,
    /// Post-processing input: texture, bilinear sampler, second texture.
    post: wgpu::BindGroupLayout,
    /// Terrain layers (group 1 of the terrain pipelines).
    terrain: wgpu::BindGroupLayout,
    /// An impostor's atlas (group 1 of the impostor pipelines).
    impostor: wgpu::BindGroupLayout,
    /// The sea's parameters (group 1 of the sea pipelines).
    sea: wgpu::BindGroupLayout,
    material: wgpu::BindGroupLayout,
    draw: wgpu::BindGroupLayout,
    cascade: wgpu::BindGroupLayout,
    forward: wgpu::BindGroupLayout,
    depth_ms: wgpu::BindGroupLayout,
    depth_1x: wgpu::BindGroupLayout,
    texture: wgpu::BindGroupLayout,
    blur: wgpu::BindGroupLayout,
    /// A plan underlay's image and its sampler (group 2 of the underlay pipeline).
    underlay: wgpu::BindGroupLayout,
    /// The air passes (shafts.wgsl): shadow map, its sampler, linear depth, the half-size
    /// shafts, the aerial perspective LUT, its sampler, the sun's openness.
    shafts: wgpu::BindGroupLayout,
    /// The volumetric clouds' image and its sampler (group 2 of the sky pipelines).
    sky_clouds: wgpu::BindGroupLayout,
}

/// Pipelines that depend on the MSAA sample count. Depth and forward are
/// indexed by `Material::variant` (cutout * 2 + double-sided), transparent
/// by double-sided.
struct SampledPipelines {
    depth: [wgpu::RenderPipeline; 4],
    forward: [wgpu::RenderPipeline; 4],
    transparent: [wgpu::RenderPipeline; 2],
    /// Glass: [transmission (multiplied), reflection (added)] by double-sided.
    glass: [[wgpu::RenderPipeline; 2]; 2],
    /// The endless ground near the sky volume and away from it (`ground_pipeline`).
    ground: [wgpu::RenderPipeline; 2],
    terrain: wgpu::RenderPipeline,
    /// Impostor quads: depth pre-pass (alpha test, dither), forward (Equal).
    impostor: [wgpu::RenderPipeline; 2],
    /// The last mesh level handing over to an impostor (by double-sided).
    depth_fade: [wgpu::RenderPipeline; 2],
    sky: wgpu::RenderPipeline,
    /// Falling rain: depth-tested streaks over the finished forward pass (wet.wgsl).
    rain: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
    sea: sea::SeaPipelines,
}

struct Pipelines {
    /// Opaque, cutout, glass (stippled).
    shadow: [wgpu::RenderPipeline; 4],
    /// The mirrored scene, by `Material::variant`, then its ground and sky.
    reflect: [wgpu::RenderPipeline; 4],
    reflect_ground: wgpu::RenderPipeline,
    reflect_terrain: wgpu::RenderPipeline,
    reflect_impostor: wgpu::RenderPipeline,
    impostor_shadow: wgpu::RenderPipeline,
    reflect_sky: wgpu::RenderPipeline,
    ssao: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    /// First (Karis-weighted) and further downsamples, upsample.
    bloom: [wgpu::RenderPipeline; 3],
    /// Into the output (opaque), into the LDR image (luma in alpha).
    tonemap: [wgpu::RenderPipeline; 2],
    fxaa: wgpu::RenderPipeline,
    upscale: wgpu::RenderPipeline,
    overlay: wgpu::RenderPipeline,
    underlay: wgpu::RenderPipeline,
    /// The air (shafts.wgsl): the shafts' march, their blur across and down, the sun's
    /// openness, the composite onto the HDR image.
    shafts_march: wgpu::RenderPipeline,
    shafts_blur: [wgpu::RenderPipeline; 2],
    sun_seen: wgpu::RenderPipeline,
    air: wgpu::RenderPipeline,
}

/// Everything sized by the viewport or the quality settings.
struct Targets {
    /// The scene's size (output size times the render scale).
    size: (u32, u32),
    depth: wgpu::TextureView,
    hdr_msaa: Option<wgpu::TextureView>,
    hdr: wgpu::TextureView,
    hdr_texture: wgpu::Texture,
    /// A photo's previous sample (`photo`: full size, else 1x1): what the one-bounce diffuse
    /// light of the next sample gathers; with screen-space reflections (full size) also the
    /// opaque scene of this frame, copied before the glass and mirrors, which they reflect.
    prev_hdr: wgpu::Texture,
    linear_depth: wgpu::TextureView,
    /// Read back at one texel for a photo's autofocus (`still.rs`).
    linear_depth_texture: wgpu::Texture,
    occlusion: [wgpu::TextureView; 2],
    output: wgpu::Texture,
    /// sRGB view the overlay renders through.
    output_view: wgpu::TextureView,
    /// Non-sRGB view: post passes write sRGB-encoded values directly.
    output_unorm: wgpu::TextureView,
    /// View in the texture's own format, returned by `render`.
    display_view: wgpu::TextureView,
    /// Tone-mapped image at render size, and FXAA's output when an upscale
    /// follows.
    ldr: [wgpu::TextureView; 2],
    bloom: Vec<wgpu::TextureView>,
    /// The planar reflection of the water planes (`Quality::water_planes` layers, at least
    /// one: a 1x1 stand-in while it is off): the whole array, to sample.
    reflection: wgpu::TextureView,
    /// Its layers, to draw into (one pass each).
    reflection_layers: Vec<wgpu::TextureView>,
    reflection_depth: wgpu::TextureView,
    shadow_layers: Vec<wgpu::TextureView>,
    resolve_bind: wgpu::BindGroup,
    ssao_bind: wgpu::BindGroup,
    blur_bind: [wgpu::BindGroup; 2],
    /// HDR, then each level as the source of the next (down), then each
    /// level as the source of the one above (up).
    bloom_down_bind: Vec<wgpu::BindGroup>,
    bloom_up_bind: Vec<wgpu::BindGroup>,
    tonemap_bind: wgpu::BindGroup,
    fxaa_bind: wgpu::BindGroup,
    /// Upscale from `ldr[0]` (no FXAA) or `ldr[1]`.
    upscale_bind: [wgpu::BindGroup; 2],
    overlay_bind: wgpu::BindGroup,
    forward_bind: wgpu::BindGroup,
    /// For the reflection pass: no screen-space inputs.
    reflect_forward_bind: wgpu::BindGroup,
    /// The light shafts at half the render size, marched into [0], blurred across into [1]
    /// and down into [0] again (rgb the light to take out, a the linear depth).
    shafts: [wgpu::TextureView; 2],
    /// One texel: the share of the sun's disc the scene leaves open (shafts.wgsl).
    sun_seen: wgpu::TextureView,
    /// The air passes' inputs: for the march and the sun's openness, the blur across, the blur
    /// down, the composite.
    air_bind: [wgpu::BindGroup; 4],
    bytes: u64,
}

/// Which geometry pass `plan_items` plans.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// `far`: the cached, camera-independent last cascade; `coarse`: a
    /// middle cascade (draws the coarsest level of detail).
    Shadow {
        far: bool,
        coarse: bool,
        /// The cascade, to leave out the items that cast into later ones only
        /// (`set_item_shadow_cascades`); `usize::MAX` for the snow cover map, which takes all.
        cascade: usize,
    },
    Depth,
    Forward,
    /// The mirrors (`Material::mirror`) after the opaque scene is copied for screen-space
    /// reflections; with those off they go with `Forward`.
    Mirrors,
    /// `coarsest`: every plant at its last level (its impostor), every other item at its
    /// coarsest mesh (a quarter-size reflection, MEDIUM: the plants' meshes cost the pass more
    /// than its pixels).
    Reflection {
        coarsest: bool,
    },
}

/// The sky source as set by `set_sky`.
enum SkySource {
    Physical,
    Preetham,
    Hdri {
        view: wgpu::TextureView,
        mips: u32,
        /// Mean radiance of the upper half (image units, clamped).
        mean: Vec3,
        rotation: f32,
        intensity: f32,
        bytes: u64,
    },
}

/// A water surface of the frame as it was gathered: what draws it, and what the ranking
/// (`water.rs`) sees of it.
struct Seen {
    /// The item (its `ItemId`), its part, and the part's material; `None` for the sea.
    item: Option<(u32, u32, MaterialId)>,
    surface: water::Surface,
}

/// Which clusters the passes' culling keeps (`Renderer::visible_clusters`): one flag per
/// cluster of every item, item `i`'s at `offsets[i]..offsets[i + 1]` (`None` slots get none).
/// Kept between passes and frames; the offsets are remade after items or their clusters change
/// (`dirty`).
#[derive(Default)]
struct Visibility {
    seen: Vec<bool>,
    offsets: Vec<u32>,
    dirty: bool,
}

impl Visibility {
    /// `offsets` over `items` and `seen` over all their clusters, after a change.
    fn follow(&mut self, items: &[Option<Item>]) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.offsets.clear();
        self.offsets.push(0);
        let mut n = 0u32;
        for item in items {
            n += item.as_ref().map_or(0, |i| i.clusters.len() as u32);
            self.offsets.push(n);
        }
        self.seen.clear();
        self.seen.resize(n as usize, false);
    }
}

/// Per-frame counters for `stats` (`record` takes `&self`).
#[derive(Default)]
struct Counters {
    draws: Cell<u32>,
    triangles: Cell<u64>,
    instances: Cell<u32>,
    /// Draws per pass group, closed by `split`.
    passes: std::cell::RefCell<Vec<(&'static str, u32)>>,
    mark: Cell<u32>,
    /// Triangles per pass group, as `passes`.
    pass_triangles: std::cell::RefCell<Vec<(&'static str, u64)>>,
    triangle_mark: Cell<u64>,
    /// CPU ms of the passes' planning (`plan_items`, all passes) and of the cluster visibility
    /// tests inside it (`visible_clusters`): `StageLog`. Native only: the web has no clock.
    #[cfg(not(target_arch = "wasm32"))]
    plan_ms: Cell<f64>,
    #[cfg(not(target_arch = "wasm32"))]
    visible_ms: Cell<f64>,
    /// Draw calls issued since the last split (`call`), per pass group as `passes`.
    calls: Cell<u32>,
    pass_calls: std::cell::RefCell<Vec<(&'static str, u32)>>,
    call_mark: Cell<u32>,
}

impl Counters {
    fn add(&self, triangles: u64, instances: u32, view: bool) {
        self.draws.set(self.draws.get() + 1);
        self.triangles
            .set(self.triangles.get() + triangles * instances as u64);
        if view {
            self.instances.set(self.instances.get() + instances);
        }
    }

    /// `n` draw calls issued (`pass_calls`).
    fn calls(&self, n: u32) {
        self.calls.set(self.calls.get() + n);
    }

    /// One draw call issued (`pass_calls`).
    fn call(&self) {
        self.calls(1);
    }

    /// Attributes the draws since the last split to `label`.
    fn split(&self, label: &'static str) {
        let n = self.draws.get() - self.mark.replace(self.draws.get());
        let mut passes = self.passes.borrow_mut();
        match passes.iter_mut().find(|(l, _)| *l == label) {
            Some(e) => e.1 += n,
            None => passes.push((label, n)),
        }
        let t = self.triangles.get() - self.triangle_mark.replace(self.triangles.get());
        let mut triangles = self.pass_triangles.borrow_mut();
        match triangles.iter_mut().find(|(l, _)| *l == label) {
            Some(e) => e.1 += t,
            None => triangles.push((label, t)),
        }
        let c = self.calls.get() - self.call_mark.replace(self.calls.get());
        let mut calls = self.pass_calls.borrow_mut();
        match calls.iter_mut().find(|(l, _)| *l == label) {
            Some(e) => e.1 += c,
            None => calls.push((label, c)),
        }
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    output_format: wgpu::TextureFormat,
    layouts: Layouts,
    modules: Vec<wgpu::ShaderModule>,
    pipelines: Pipelines,
    sampled: SampledPipelines,
    /// The pipelines of the other shading made before (`set_quality`): by fast shading, its
    /// modules and pipelines; by fast shading and samples, the sampled ones. A quality change
    /// back and forth (Auto, the menu) takes them instead of compiling every pipeline again.
    other_pipelines: Vec<(bool, Vec<wgpu::ShaderModule>, Pipelines)>,
    other_sampled: Vec<(bool, u32, SampledPipelines)>,
    /// Times every pipeline was made (`stats`): at the start, and per new shading.
    pipeline_builds: u32,
    targets: Targets,
    size: (u32, u32),
    quality: Quality,
    /// How the MSAA depth pre-pass writes coverage on this device (`coverage.rs`).
    coverage: coverage::Coverage,

    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    /// The mirrored camera's globals for the reflection passes: one per plane with a planar
    /// reflection (layer `k` of the planar reflection draws with `reflect_globals[k]`).
    reflect_globals: [wgpu::Buffer; water::MAX_PLANES as usize],
    reflect_globals_bind: [wgpu::BindGroup; water::MAX_PLANES as usize],
    globals_only_bind: wgpu::BindGroup,
    /// Globals, sampler and sky source for the environment passes.
    sky_bind: wgpu::BindGroup,
    cascade_buffer: wgpu::Buffer,
    cascade_bind: wgpu::BindGroup,
    draw_buffer: wgpu::Buffer,
    draw_bind: wgpu::BindGroup,
    draw_capacity: usize,
    /// An impostor got a slot, or the draw buffer grew (`alloc_slot` writes plain mesh slots):
    /// the impostors' slots are written again before the next frame.
    impostor_slots_dirty: bool,
    /// This frame's dissolving clusters' draw slots (`FADE_SLOTS`).
    fade_buffer: wgpu::Buffer,
    fade_bind: wgpu::BindGroup,
    next_slot: u32,
    free_slots: Vec<u32>,
    white: wgpu::TextureView,
    repeat_sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    env_sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    post_sampler: wgpu::Sampler,
    mipmapper: texture::Mipmapper,
    env: env::Env,
    physical: atmosphere::PhysicalSky,
    sky_source: SkySource,
    /// The sky-view LUT must follow the sun.
    sky_view_dirty: bool,
    /// Below the horizon for the environment: an HDRI's ground radiance
    /// (kcd/m2), or the ground albedo the GPU lights (analytic skies).
    ground_kcd: Vec3,
    /// The albedo light bounces off below the horizon (`set_bounce_albedo`); `None`: the
    /// ground plane's colour.
    bounce_albedo: Option<Vec3>,
    atmosphere: Atmosphere,
    ground: Option<Ground>,
    /// A scene material the ground is drawn with (`set_ground_material`).
    ground_look: Option<MaterialId>,
    ground_slot: u32,
    /// The draw slot of every gathered mesh batch: its mesh id and material row come from each
    /// instance, so the slot stays bound over them all (`FROM_INSTANCE`).
    batch_slot: u32,
    terrain: terrain::TerrainGpu,
    sea: sea::SeaGpu,
    /// GPU grass over the lawns and meadows (`set_grass`).
    grass: grass::GrassGpu,
    baker: impostor::Baker,
    impostors: Vec<Option<impostor::GpuImpostor>>,
    bloom_strength: f32,
    /// The display look (`set_grade`): contrast power and saturation.
    grade: (f32, f32),
    /// The camera is in a room (`set_indoors`): glass mirrors the room, not the sky.
    indoors: bool,
    /// The sky visibility volume (`set_sky_volume`), two tiles side by side along x: its room
    /// light (per sample the lamps' and the sun's light passed round the rooms (lux) and the
    /// sky's, per unit of its mean light, < 0 inside a wall), then the visible fraction and the
    /// mean visible direction; open sky and no room light when unset.
    room_volume: (wgpu::Texture, wgpu::TextureView),
    /// Its occluders' voxels (`set_sky_volume`): 1 where solid; one empty voxel when unset.
    occupancy: (wgpu::Texture, wgpu::TextureView),
    /// Globals `sky_vol` and `sky_vol_size`: origin (m) and 1 / cell, 1 / texture size in
    /// cells and on (0/1).
    sky_volume_box: [f32; 8],
    /// The baked sky visibility (`set_sky_visibility`) as a 3D texture; one empty probe when
    /// unset.
    sky_occ: (wgpu::Texture, wgpu::TextureView),
    /// Globals `sky_occ` and `sky_occ_size` for it (`None`: no grid).
    sky_occ_box: Option<[f32; 8]>,
    /// Sky occlusion on (`set_sky_occlusion`): off by default.
    sky_occlusion: bool,
    /// The procedural cloud layer of the analytic skies (`set_clouds`).
    clouds: Clouds,
    /// The volumetric clouds' march and its images (clouds.rs).
    cloud_volume: clouds::CloudVolume,
    /// How far the volumetric clouds have drifted (m, wrapped to `clouds::CLOUD_PERIOD`),
    /// integrated by `set_time` with the clouds' speed of the moment.
    cloud_drift: [f64; 2],
    /// The clouds changed too much since the last frame for its clouds to blend into this one's.
    cloud_jump: bool,
    /// The next frame is a picture of its own (`finish_environment`): its clouds start over.
    cloud_picture: bool,
    /// Snow cover 0..1 (`set_snow`), and the top-down depth map of what lies over the ground
    /// around the eye (the last shadow layer): where it shows something overhead, no snow.
    snow: f32,
    snow_cover: Option<Mat4>,
    /// The matrix the snow cover map was last drawn with: drawn again only when it moves (the
    /// eye a step of eight texels) or the scene changes (`invalidate_shadows`), not every frame.
    snow_drawn: Option<Mat4>,
    /// Wet surfaces and rain, 0..1 each (`set_wet`).
    wetness: f32,
    rain: f32,
    /// Mist 0..1 (`set_mist`).
    mist: f32,
    /// Light shafts 0..1 (`set_shafts`), the sun's glare (`set_sun_glare`), the aerial
    /// perspective (`set_aerial_perspective`), white balance and colour grade.
    shafts: f32,
    sun_glare: f32,
    aerial_perspective: bool,
    white_balance: WhiteBalance,
    color_grade: ColorGrade,
    /// A second grade mixed over `color_grade`, and its share 0..1.
    color_grade_mix: Option<(ColorGrade, f32)>,
    /// A photo's sample (`still::Photo`): the shadow's light direction moved within the sun's
    /// disc, and the noise seed of ambient occlusion; zero for live frames.
    shadow_jitter: Vec3,
    photo_seed: f32,
    /// The lens' light fall-off towards the corners, 0..=1 (tonemap.wgsl).
    vignette: f32,
    /// Film grain after the tone map, 0..=1 (tonemap.wgsl).
    grain: f32,
    /// Photo mode's accumulate and resolve passes, made at the first photo.
    photo_pipes: Option<still::PhotoPipes>,
    light_buffer: wgpu::Buffer,
    grid_buffer: wgpu::Buffer,
    grid: lights::Grid,
    light_count: usize,
    lights_cpu: Vec<Light>,
    /// Staging memory for streamed texture uploads, kept mapped and reused (`upload_step`):
    /// `write_texture` makes and frees a staging buffer per call, which showed as frame spikes.
    #[cfg(not(target_arch = "wasm32"))]
    upload_belt: wgpu::util::StagingBelt,

    /// Every item's instances (`batch.rs`), and the frame's batches gathered from them.
    pool: batch::InstancePool,
    gather: batch::Gather,
    /// The frame's indirect draws (`write_indirect`): one buffer, rewritten every frame, a
    /// range per pass; grown when needed, never shrunk.
    indirect: wgpu::Buffer,
    /// Arguments (`INDIRECT_ARGS` bytes each) it holds.
    indirect_capacity: u32,
    /// This frame's arguments, packed; kept between frames.
    indirect_args: Vec<u8>,
    /// Batched draws go through the indirect buffer (the device's `INDIRECT_FIRST_INSTANCE`):
    /// else each is a direct draw of the same arguments (`record`). A test switches it off.
    multi_draw: bool,
    /// The static meshes' vertex and index buffers: one arena until it is full at the device's
    /// `max_buffer_size`, then one more (`batch::MeshRanges` says which mesh lies where).
    arenas: Vec<batch::MeshArena>,
    mesh_ranges: batch::MeshRanges,
    meshes: Vec<Option<GpuMesh>>,
    /// Every material's parameters (`scene.wgsl` `materials`): row 0 the endless ground's own,
    /// row `id + 1` each material. Grown like the arenas (every texture set's bind group is made
    /// again: all of them bind it); `material_bytes` is its CPU copy for that.
    material_table: wgpu::Buffer,
    material_capacity: u32,
    material_bytes: Vec<u8>,
    /// Group 1 by texture set: which maps, how many materials use them (`TextureSets`), and
    /// the bind group of each index.
    sets: TextureSets,
    texture_sets: Vec<Option<GpuTextureSet>>,
    materials: Vec<Option<GpuMaterial>>,
    textures: Vec<Option<texture::GpuTexture>>,
    items: Vec<Option<Item>>,
    bounds_dirty: bool,
    /// Items an instance of which `set_instance` moved since the last frame: their bounds, the
    /// scene's and the shadows are made again once, when the frame is drawn.
    moved: Vec<u32>,
    /// How often the shadow caches were thrown away (for tests that posing does not do it per
    /// instance).
    #[cfg(test)]
    shadow_invalidations: usize,
    scene_bounds: Option<Bounds>,
    /// The passes' cluster visibility (`Visibility`), rewritten per pass.
    visibility: Visibility,
    /// `update_lods` skips blocks past the last level whose clusters are all culled (a test
    /// switches it off).
    lod_skip: bool,

    view: Mat4,
    proj: Mat4,
    eye: Vec3,
    sun: Sun,
    sky: sky::Sky,
    ev100: f32,
    /// The physical sky's light per unit of the sun's (`set_sky_scale`).
    sky_scale: f32,
    shadow_distance: f32,
    /// Multiplies every level-of-detail distance (with the quality's percentage).
    view_distance: f32,
    /// Multiplies how far impostors draw (their last distance only).
    impostor_reach: f32,
    wind: Wind,
    /// The wind's phases, integrated by `set_time` (wind.rs).
    wind_phases: wind::Phases,
    time: f32,
    /// The clock unwrapped (`set_time`): what the clouds drift by, so they never jump.
    clock: f64,
    clay: bool,
    highlight: Option<(MeshId, u32)>,
    /// The cached last shadow cascade (see `shadow`): where the camera was
    /// when it was rendered, and with which projection and light matrix.
    far_cache: Option<FarCache>,
    /// The light matrix each cascade's map was last rendered with (middle
    /// cascades are refreshed every other frame); `None` until rendered.
    cascade_rendered: [Option<Mat4>; shadow::MAX_CASCADES],
    /// Cascades whose map shows an earlier sun than `sun` (`set_sun` moved it, a picture asks for
    /// a fresh one): still sampled with their own matrix until `cascade_refresh` redraws them.
    cascade_stale: [bool; shadow::MAX_CASCADES],
    /// `set_sun` moved the sun since the last frame: the sun is moving (a dragged time of day).
    sun_moving: bool,
    frame: u64,
    /// Depth-tested and on-top segments; kept so either list can be
    /// replaced alone.
    overlay_cpu: (Vec<LineVertex>, Vec<LineVertex>),
    overlay: Option<wgpu::Buffer>,
    overlay_count: u32,
    overlay_on_top: u32,
    underlays: underlay::Drawn,
    timer: Option<timing::Timer>,
    counters: Counters,
    last_stats: RenderStats,
    /// The water surfaces of the frame, their planes and tiers (`water.rs`); the buffers are
    /// reused every frame.
    water: water::Ranking,
    water_seen: Vec<Seen>,
    water_surfaces: Vec<water::Surface>,
    /// The CPU stage times of each frame, a line per frame (`StageLog`): when
    /// `VISIONCRAFT_STAGE_TIMINGS` names a file (native tools: the bench).
    #[cfg(not(target_arch = "wasm32"))]
    stage_log: Option<Box<stage::StageLog>>,
}

impl Renderer {
    /// `format` is the output texture's format: `Rgba8UnormSrgb`, or
    /// `Rgba8Unorm` for sRGB-encoded bytes in a plain texture (what egui
    /// expects of user textures). Either way the passes write sRGB-encoded
    /// values (the overlay blends through an sRGB view, in hardware).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self::with_quality(device, queue, format, Quality::HIGH)
    }

    /// As `new`, with the scene's pipelines made for `start` (the quality the window will use:
    /// made for another one, they are all compiled again at the first frame).
    pub fn with_quality(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        start: Quality,
    ) -> Self {
        let layouts = Layouts::new(device);
        let coverage = coverage::Coverage::of(device);
        // The environment, sky, mips and impostor bakes always use the full shading (HIGH's).
        let full = coverage.quality(Quality::HIGH);
        let modules = shader_modules(device, full.fast_shading, coverage);
        let start = Quality {
            fast_shading: coverage.quality(start).fast_shading,
            msaa: coverage.quality(start).msaa,
            ..full
        };
        let scene_modules = if start.fast_shading == full.fast_shading {
            modules.clone()
        } else {
            shader_modules(device, start.fast_shading, coverage)
        };
        let pipelines = Pipelines::new(device, &layouts, &scene_modules, format);
        let sampled =
            SampledPipelines::new(device, &layouts, &scene_modules, start.samples(), coverage);
        let quality = start;

        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = uniform("globals", globals_size());
        let reflect_globals =
            std::array::from_fn(|_| uniform("reflection globals", globals_size()));
        let env = env::Env::new(device, queue, module(&modules, "env"), &layouts.sky_source);
        let globals_only_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals only"),
            layout: &layouts.globals_only,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let physical = atmosphere::PhysicalSky::new(
            device,
            module(&modules, "atmosphere"),
            &layouts.globals_only,
        );
        // The cascades, then the snow cover map's slot.
        let cascade_buffer = uniform("cascades", SLOT * (shadow::MAX_CASCADES as u64 + 1));
        let cascade_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cascade"),
            layout: &layouts.cascade,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: slot_binding(&cascade_buffer, CASCADE_SIZE),
            }],
        });
        let draw_capacity = 64;
        let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("draws"),
            size: SLOT * draw_capacity as u64,
            usage: draw_slot_usage(),
            mapped_at_creation: false,
        });
        let draw_bind = draw_bind_group(device, &layouts.draw, &draw_buffer);
        let fade_buffer = uniform("fade draws", SLOT * FADE_SLOTS as u64);
        let fade_bind = draw_bind_group(device, &layouts.draw, &fade_buffer);

        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            white_texture.as_image_copy(),
            &[255; 4],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white = white_texture.create_view(&Default::default());
        let room_volume = room_volume_texture(
            device,
            queue,
            [1, 1, 1],
            &[[0.0, 0.0, 0.0, 1.0]],
            &[[1.0, 0.0, 0.0, 0.0]],
        );
        let occupancy = occupancy_texture(device, queue, [1, 1, 1], &[0]);
        let sky_occ = sky_occ_texture(device, queue, [1, 1, 1], &[[0.0; 4]; 3]);
        // Material samplers: trilinear with 16x anisotropy (WebGPU core
        // allows up to 16 when all filters are linear).
        let material_sampler = |label, mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                anisotropy_clamp: 16,
                ..Default::default()
            })
        };
        let repeat_sampler = material_sampler("repeat", wgpu::AddressMode::Repeat);
        let clamp_sampler = material_sampler("clamp", wgpu::AddressMode::ClampToEdge);
        // Repeats in u for the HDRI's azimuth seam (a cube ignores it).
        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("environment"),
            address_mode_u: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let storage = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let light_buffer = storage("lights", lights::LIGHT_BYTES as u64);
        let grid_buffer = storage("light grid", 16);
        let grid = lights::build_grid(&[]);
        queue.write_buffer(&grid_buffer, 0, &u32_bytes(&grid.data));
        let globals_bind = globals_bind_group(
            device,
            &layouts.globals,
            &globals,
            &env.view,
            &env_sampler,
            &light_buffer,
            &grid_buffer,
            &physical.sky_view,
            [&room_volume.1, &occupancy.1, &sky_occ.1],
        );
        let reflect_globals_bind = reflect_globals.each_ref().map(|buffer| {
            globals_bind_group(
                device,
                &layouts.globals,
                buffer,
                &env.view,
                &env_sampler,
                &light_buffer,
                &grid_buffer,
                &physical.sky_view,
                [&room_volume.1, &occupancy.1, &sky_occ.1],
            )
        });
        let sky_bind = sky_bind_group(
            device,
            &layouts.sky_source,
            &globals,
            &env_sampler,
            &physical.sky_view,
        );
        let targets = Targets::new(
            device,
            &layouts,
            format,
            quality,
            (1, 1),
            &shadow_sampler,
            &post_sampler,
            &white,
            &physical.aerial,
            false,
        );
        let mipmapper = texture::Mipmapper::new(device, module(&modules, "mip"));
        let indirect_capacity = 256u32;
        let indirect = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("indirect draws"),
            size: u64::from(indirect_capacity) * u64::from(INDIRECT_ARGS),
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sun = Sun::from_angles(180.0, 45.0);
        let sky = sky::Sky::new(sun.direction);
        let material_capacity = 64u32;
        let material_table = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("materials"),
            size: u64::from(material_capacity) * MATERIAL_SIZE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cloud_volume = clouds::CloudVolume::new(device, queue, &layouts);
        let layouts_terrain = layouts.terrain.clone();
        let layouts_sea = layouts.sea.clone();
        let baker = impostor::Baker::new(
            device,
            &layouts,
            module(&modules, "scene"),
            module(&modules, "dilate"),
        );
        let mut renderer = Self {
            device: device.clone(),
            queue: queue.clone(),
            output_format: format,
            layouts,
            modules: scene_modules,
            pipelines,
            sampled,
            other_pipelines: Vec::new(),
            other_sampled: Vec::new(),
            pipeline_builds: 1,
            coverage,
            targets,
            size: (1, 1),
            quality,
            globals,
            globals_bind,
            reflect_globals,
            reflect_globals_bind,
            globals_only_bind,
            sky_bind,
            cascade_buffer,
            cascade_bind,
            draw_buffer,
            draw_bind,
            fade_buffer,
            fade_bind,
            draw_capacity,
            impostor_slots_dirty: false,
            next_slot: 0,
            free_slots: Vec::new(),
            white,
            repeat_sampler,
            clamp_sampler,
            env_sampler,
            shadow_sampler,
            post_sampler,
            mipmapper,
            env,
            physical,
            sky_source: SkySource::Physical,
            sky_view_dirty: true,
            ground_kcd: Vec3::ZERO,
            bounce_albedo: None,
            atmosphere: Atmosphere::default(),
            ground: None,
            ground_look: None,
            ground_slot: 0,
            batch_slot: 0,
            terrain: terrain::TerrainGpu::new(device, queue, &layouts_terrain),
            sea: sea::SeaGpu::new(device, &layouts_sea),
            grass: grass::GrassGpu::new(device),
            baker,
            impostors: Vec::new(),
            bloom_strength: 0.025,
            grade: (1.0, 1.0),
            indoors: false,
            room_volume,
            occupancy,
            sky_volume_box: [0.0; 8],
            sky_occ,
            sky_occ_box: None,
            sky_occlusion: false,
            clouds: Clouds::default(),
            cloud_volume,
            cloud_drift: [0.0; 2],
            cloud_jump: false,
            cloud_picture: false,
            snow: 0.0,
            snow_cover: None,
            snow_drawn: None,
            wetness: 0.0,
            rain: 0.0,
            mist: 0.0,
            shafts: 0.0,
            sun_glare: 0.0,
            aerial_perspective: false,
            white_balance: WhiteBalance::Daylight,
            color_grade: ColorGrade::Neutral,
            color_grade_mix: None,
            shadow_jitter: Vec3::ZERO,
            photo_seed: 0.0,
            vignette: 0.0,
            grain: 0.0,
            photo_pipes: None,
            light_buffer,
            grid_buffer,
            grid,
            light_count: 0,
            lights_cpu: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            upload_belt: wgpu::util::StagingBelt::new(device.clone(), UPLOAD_CHUNK),
            pool: batch::InstancePool::new(device, 4096),
            gather: batch::Gather::new(device),
            indirect,
            indirect_capacity,
            indirect_args: Vec::new(),
            multi_draw: device
                .features()
                .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE),
            arenas: Vec::new(),
            mesh_ranges: batch::MeshRanges::default(),
            meshes: Vec::new(),
            material_table,
            material_capacity,
            material_bytes: vec![0; (material_capacity * MATERIAL_SIZE as u32) as usize],
            sets: TextureSets::default(),
            texture_sets: Vec::new(),
            materials: Vec::new(),
            textures: Vec::new(),
            items: Vec::new(),
            visibility: Visibility {
                dirty: true,
                ..Visibility::default()
            },
            lod_skip: true,
            bounds_dirty: false,
            moved: Vec::new(),
            #[cfg(test)]
            shadow_invalidations: 0,
            scene_bounds: None,
            view: Mat4::look_at_rh(Vec3::new(0.0, 1.7, 10.0), Vec3::ZERO, Vec3::Y),
            proj: Mat4::perspective_rh(1.0, 1.0, 0.1, 1000.0),
            eye: Vec3::new(0.0, 1.7, 10.0),
            sun,
            sky,
            ev100: 15.0,
            sky_scale: 1.0,
            shadow_distance: 80.0,
            view_distance: 1.0,
            impostor_reach: 1.0,
            wind: Wind::default(),
            wind_phases: wind::Phases::default(),
            time: 0.0,
            clock: 0.0,
            clay: false,
            highlight: None,
            far_cache: None,
            cascade_rendered: [None; shadow::MAX_CASCADES],
            cascade_stale: [false; shadow::MAX_CASCADES],
            sun_moving: false,
            frame: 0,
            overlay_cpu: (Vec::new(), Vec::new()),
            overlay: None,
            overlay_count: 0,
            overlay_on_top: 0,
            underlays: underlay::Drawn::default(),
            timer: timing::Timer::new(device, queue),
            counters: Counters::default(),
            last_stats: RenderStats::default(),
            water: water::Ranking::default(),
            water_seen: Vec::new(),
            water_surfaces: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            stage_log: stage::StageLog::from_env(),
        };
        // The set without maps (every untextured material's), the ground's row and the two
        // slots that never change.
        let (set, new) = renderer.sets.acquire([None; 6]);
        assert!(new && set == 0);
        renderer.put_set(set, [None; 6]);
        renderer.write_row(GROUND_ROW, &material_bytes(&Material::default(), 0));
        renderer.ground_slot = renderer.alloc_slot(MeshId(u32::MAX - 1), GROUND_ROW);
        renderer.batch_slot = renderer.alloc_slot(MeshId(FROM_INSTANCE), FROM_INSTANCE);
        renderer.update_ambient(true);
        renderer
    }

    /// Recreates the size-dependent targets. Call before the first `render`
    /// and whenever the output size changes.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
        self.rebuild_targets();
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// How many times every scene pipeline was made: once at the start, once more for each
    /// other shading a quality change asked for (`set_quality` keeps them).
    pub fn pipeline_builds(&self) -> u32 {
        self.pipeline_builds
    }

    /// On a Vulkan device that cannot shade per sample, MSAA stays off (`coverage.rs`).
    pub fn set_quality(&mut self, quality: Quality) {
        let quality = self.coverage.quality(quality.sanitized());
        if quality == self.quality {
            return;
        }
        // The shadow filter only reaches the globals: no pipeline, target or shadow map of
        // the renderer depends on it, so a picture's own filter costs nothing to swap in.
        let filter_only = Quality {
            shadow_filter: quality.shadow_filter,
            ..self.quality
        } == quality;
        let (fast, samples) = (self.quality.fast_shading, self.quality.samples());
        if quality.fast_shading != fast {
            // Other shaders: every pipeline of the scene and the sky, made once per shading and
            // kept for the next change back.
            let i = self
                .other_pipelines
                .iter()
                .position(|v| v.0 == quality.fast_shading);
            let (modules, pipelines) = match i {
                Some(i) => {
                    let (_, m, p) = self.other_pipelines.swap_remove(i);
                    (m, p)
                }
                None => {
                    self.pipeline_builds += 1;
                    let m = shader_modules(&self.device, quality.fast_shading, self.coverage);
                    let p = Pipelines::new(&self.device, &self.layouts, &m, self.output_format);
                    (m, p)
                }
            };
            let old_m = std::mem::replace(&mut self.modules, modules);
            let old_p = std::mem::replace(&mut self.pipelines, pipelines);
            self.other_pipelines.push((fast, old_m, old_p));
        }
        if quality.fast_shading != fast || quality.samples() != samples {
            let i = self
                .other_sampled
                .iter()
                .position(|v| v.0 == quality.fast_shading && v.1 == quality.samples());
            let sampled = match i {
                Some(i) => self.other_sampled.swap_remove(i).2,
                None => SampledPipelines::new(
                    &self.device,
                    &self.layouts,
                    &self.modules,
                    quality.samples(),
                    self.coverage,
                ),
            };
            let old = std::mem::replace(&mut self.sampled, sampled);
            self.other_sampled.push((fast, samples, old));
        }
        self.quality = quality;
        if !filter_only {
            self.rebuild_targets();
        }
    }

    /// What wgpu reports about this renderer's adapter and device
    /// (`hardware` module docs: little in a browser).
    pub fn probe(&self) -> HardwareProbe {
        HardwareProbe::from_device(&self.device)
    }

    /// `hardware::calibrate` on this renderer's device.
    pub async fn calibrate(&self, clock: Option<&dyn Fn() -> f64>) -> Result<Score, String> {
        calibrate(&self.device, &self.queue, clock).await
    }

    /// Camera: world-to-view matrix, a projection with depth 0..1
    /// (`Mat4::perspective_rh`, `perspective_infinite_rh` or
    /// `orthographic_rh`, recognised from the matrix), and the eye position
    /// in world space (orthographic: any point on the view axis in front of
    /// the scene; it centres the ground backdrop).
    pub fn set_camera(&mut self, view: Mat4, proj: Mat4, position: Vec3) {
        self.view = view;
        self.proj = proj;
        self.eye = position;
    }

    /// The sun's illuminance on a surface facing it (lux, linear RGB), after the clouds' share.
    pub fn sun_lux(&self) -> Vec3 {
        self.sun.color * self.sun.illuminance * self.sun_share()
    }

    /// The cloud layer drawn, as (cover, overcast 0..1, storm 0..1): the caller's
    /// (`set_clouds`; a storm is overcast too), and in rain thickened to a closed grey one, the
    /// more so the harder it rains (cover 0.3 + 0.7 rain, overcast from a drizzle of 0.1 to
    /// 0.6). Without rain exactly the caller's.
    fn cloud_state(&self) -> (f32, f32, f32) {
        let cover = self.clouds.cover.clamp(0.0, 1.0);
        let storm = self.clouds.storm.clamp(0.0, 1.0);
        let overcast = self.clouds.overcast.clamp(0.0, 1.0).max(storm);
        if self.rain <= 0.0 {
            return (cover, overcast, storm);
        }
        let t = ((self.rain - 0.1) / 0.5).clamp(0.0, 1.0);
        (
            cover.max((0.3 + 1.4 * self.rain).min(1.0)),
            overcast.max(t * t * (3.0 - 2.0 * t)),
            storm,
        )
    }

    /// Volumetric clouds are drawn: asked for (`Clouds::volumetric`), at a quality that has
    /// them (`Quality::clouds`), under an analytic sky (an HDRI brings its own) with clouds in it.
    fn volumetric_clouds(&self) -> bool {
        self.clouds.volumetric
            && self.quality.clouds > 0
            && !matches!(self.sky_source, SkySource::Hdri { .. })
            && self.cloud_state().0 > 0.0
    }

    /// The strength of the volumetric clouds' shadows on the ground (common.wgsl
    /// `cloud_shadow`) for a camera with `proj`: the broken part of the sky (1 - overcast); the
    /// closed part dims the sun everywhere (`sun_share`). None in a plan or an elevation.
    fn cloud_shadows(&self, proj: Mat4) -> f32 {
        if !self.volumetric_clouds() || shadow::is_orthographic(proj) {
            return 0.0;
        }
        1.0 - self.cloud_state().1
    }

    /// The clouds' drift speed (m/s): the caller's, else faster than the wind at the ground as
    /// the wind aloft is.
    fn cloud_speed(&self) -> f32 {
        self.clouds
            .speed_mps
            .unwrap_or(4.0 + 1.2 * self.wind.speed_mps)
            .max(0.0)
    }

    /// How much of the sun's direct light gets through: the caller's clouds (`Clouds::sun_share`)
    /// and, in rain, a share that falls steeply with it: about 0.15 of dry at 0.8 and 0.03 at 1,
    /// so hard shadows all but vanish under a rain sky and some sun is left in a drizzle (0.3: 0.9).
    ///
    /// Mist dims the sun too, by its optical depth along the sun's path down through the layer
    /// (`atmosphere::mist_sun_share`): a low sun in a morning mist is a pale disc.
    ///
    /// With volumetric clouds the broken part of the sky does not dim the sun everywhere: its
    /// clouds cast their own shadows, which drift over the ground (`cloud_shadows`).
    fn sun_share(&self) -> f32 {
        let t = ((self.rain - 0.1) / 0.9).clamp(0.0, 1.0);
        let rain = 1.0 - 0.97 * t * t * (3.0 - 2.0 * t);
        let clouds = if self.cloud_shadows(self.proj) > 0.0 {
            let c = &self.clouds;
            let closed = c.cover.clamp(0.0, 1.0) * (0.85 + 0.12 * c.storm.clamp(0.0, 1.0));
            1.0 - closed * c.overcast.max(c.storm).clamp(0.0, 1.0)
        } else {
            self.clouds.sun_share()
        };
        let share = if self.rain <= 0.0 {
            clouds
        } else {
            clouds * rain
        };
        if self.mist <= 0.0 {
            share
        } else {
            share * atmosphere::mist_sun_share(self.mist, self.sun.direction.y)
        }
    }

    pub fn set_sun(&mut self, sun: &Sun) {
        let sun = Sun {
            direction: sun.direction.normalize(),
            ..*sun
        };
        if sun == self.sun {
            return; // keeps the cached cascade, the ambient and the environment
        }
        self.sun = sun;
        // Not `invalidate_shadows`: the cascades keep their maps and matrices (each is sampled
        // with its own, a picture of an earlier sun) and `cascade_refresh` redraws them over the
        // next frames; the snow cover looks straight down and does not depend on the sun.
        self.cascade_stale = [true; shadow::MAX_CASCADES];
        self.sun_moving = true;
        self.sky = sky::Sky::new(self.sun.direction);
        self.update_ambient(false);
    }

    /// Camera exposure as EV at ISO 100. Manual only: the renderer never
    /// adapts it. Sensible values for this sky model and lights:
    /// sunny noon 15, overcast or sun low (10 deg) 13-14, sunset 11-12,
    /// dusk (sun 2-4 deg below the horizon) 7-9, late dusk with garden
    /// lights 4-6, night lit by garden lamps 1-3, moonlight alone -2..0.
    /// The physical sky's brightness per unit of the light set with `set_sun`: 1 for the sun;
    /// when the moon stands in for the sun (`set_sun` given the moonlight), the moon's light as a
    /// fraction of the sun's, so the moonlit sky is the daylight sky scaled down, as it is.
    pub fn set_sky_scale(&mut self, scale: f32) {
        let scale = scale.max(0.0);
        if scale != self.sky_scale {
            self.sky_scale = scale;
            self.update_ambient(false);
        }
    }

    pub fn set_exposure(&mut self, ev100: f32) {
        self.ev100 = ev100;
    }

    /// How far plants and objects keep their detail and stay drawn, as a
    /// multiple of their own distances (default 1; with the quality's LOD
    /// percentage on top).
    pub fn set_view_distance(&mut self, scale: f32) {
        self.view_distance = scale.clamp(0.05, 20.0);
    }

    /// How far the impostors of items marked with `set_item_follows_reach`
    /// draw, as a multiple of their own end distance (default 1). An
    /// overview from above sees small plants hundreds of metres away; at
    /// eye level the ground and everything on it hide them long before.
    pub fn set_impostor_reach(&mut self, scale: f32) {
        self.impostor_reach = scale.clamp(0.01, 10.0);
    }

    /// Makes the end of `item`'s impostor follow `set_impostor_reach`.
    pub fn set_item_follows_reach(&mut self, item: ItemId, follows: bool) {
        if let Some(Some(it)) = self.items.get_mut(item.0 as usize) {
            it.follows_reach = follows;
        }
    }

    /// How far from the camera sun shadows reach (metres, default 80).
    pub fn set_shadow_distance(&mut self, metres: f32) {
        self.shadow_distance = metres.max(1.0);
        self.invalidate_shadows();
    }

    /// Wind for vegetation (meshes with `wind` weights). Shadows move with
    /// the leaves in every cascade but the cached far one, which keeps the
    /// rest pose (it is re-rendered only when the camera moves far).
    pub fn set_wind(&mut self, wind: &Wind) {
        if self.wind.speed_mps != wind.speed_mps || self.wind.gusts != wind.gusts {
            // The culling bounds' room for the sway follows the speed.
            self.bounds_dirty = true;
        }
        self.wind = *wind;
    }

    /// Animation time in seconds (wind, water). The water's clock wraps every 4096 s; the wind's
    /// phases are integrated here with the wind of the moment (never a jump).
    ///
    /// The volumetric clouds drift with the wind of the moment from the last time, or, after a
    /// jump back or of more than 5 s (a picture at its own time), by the time at the current speed.
    pub fn set_time(&mut self, seconds: f64) {
        let flash = self.flash();
        let dt = seconds - self.clock;
        let wind = Vec2::new(self.wind.direction.x, self.wind.direction.z).normalize_or_zero();
        let speed = f64::from(self.cloud_speed());
        for (k, d) in self.cloud_drift.iter_mut().enumerate() {
            let v = f64::from(wind[k]) * speed;
            let at = if (0.0..=5.0).contains(&dt) {
                *d + v * dt
            } else {
                self.cloud_jump = true;
                v * seconds
            };
            *d = at.rem_euclid(clouds::CLOUD_PERIOD);
        }
        self.time = seconds.rem_euclid(TIME_WRAP) as f32;
        self.clock = seconds;
        self.wind_phases.advance(seconds, self.wind.speed_mps);
        // A lightning flash lights the scene through the ambient light and the environment
        // (sky.wgsl `sky_kcd`): redone each frame while one comes and goes.
        if self.flash() != flash {
            self.update_ambient(false);
        }
    }

    /// A light vignette (the lens' fall-off towards the corners) and film grain, each 0..=1;
    /// 0 leaves the picture as it was.
    pub fn set_vignette_grain(&mut self, vignette: f32, grain: f32) {
        self.vignette = vignette.clamp(0.0, 1.0);
        self.grain = grain.clamp(0.0, 1.0);
    }

    /// Clay presentation: opaque and cutout materials draw matte white
    /// (cutout keeps its mask), glass and water as pale matte surfaces.
    pub fn set_clay(&mut self, clay: bool) {
        if clay != self.clay {
            self.clay = clay;
            // The light the ground bounces up (ambient SH, environment) is
            // clay grey too, or leaves and anything facing down stay green.
            self.update_ambient(true);
        }
    }

    /// Point and spot lights (replaces the previous set). At most
    /// `lights::MAX_LIGHTS` (1024) are used and at most
    /// `lights::MAX_PER_CELL` (32) light any one grid cell; returns how many
    /// lights had to be left out of some crowded cell (0 normally).
    pub fn set_lights(&mut self, list: &[Light]) -> usize {
        let grid = lights::build_grid(list);
        let packed = lights::pack(list);
        let light_bytes = f32_bytes(&packed);
        let grid_bytes = u32_bytes(&grid.data);
        let mut rebind = false;
        if self.light_buffer.size() < light_bytes.len() as u64 {
            self.light_buffer = self.storage_buffer("lights", light_bytes.len() as u64);
            rebind = true;
        }
        if self.grid_buffer.size() < grid_bytes.len() as u64 {
            self.grid_buffer = self.storage_buffer("light grid", grid_bytes.len() as u64);
            rebind = true;
        }
        if rebind {
            self.rebind_globals();
        }
        self.queue.write_buffer(&self.light_buffer, 0, &light_bytes);
        self.queue.write_buffer(&self.grid_buffer, 0, &grid_bytes);
        self.light_count = list.len().min(lights::MAX_LIGHTS);
        self.lights_cpu = list[..self.light_count].to_vec();
        let dropped = grid.dropped;
        self.grid = grid;
        dropped
    }

    /// Haze (distance fog); `Atmosphere::default()` is on, `haze: 0.0` off.
    pub fn set_atmosphere(&mut self, atmosphere: &Atmosphere) {
        self.atmosphere = Atmosphere {
            haze: atmosphere.haze.max(0.0),
            haze_height_m: atmosphere.haze_height_m.max(1.0),
            sun_glow: atmosphere.sun_glow.clamp(0.0, 1.0),
            clear_m: atmosphere.clear_m.max(0.0),
        };
    }

    /// The endless ground plane (off by default). Its colour also lights
    /// the environment below the horizon.
    pub fn set_ground(&mut self, ground: Option<Ground>) {
        if ground == self.ground {
            return;
        }
        // The surroundings and the plot do not light the environment.
        let lit = |g: Option<Ground>| g.map(|g| (g.height, g.color, g.roughness, g.variation));
        let relight = lit(ground) != lit(self.ground);
        self.ground = ground;
        if let Some(g) = ground {
            let material = Material {
                base_color: [g.color[0], g.color[1], g.color[2], 1.0],
                roughness: g.roughness,
                variation: g.variation,
                // Triplanar: no UVs needed (and none are sampled: no maps).
                projection: Projection::Triplanar { scale_m: 1.0 },
                ..Default::default()
            };
            self.write_row(GROUND_ROW, &material_bytes(&material, 0));
        }
        if relight {
            self.update_ambient(true);
        }
    }

    /// Draws the endless ground with `material` (its maps, scale and variation) instead of the
    /// plain colour of `set_ground`, which still lights the environment below the horizon.
    pub fn set_ground_material(&mut self, material: Option<MaterialId>) {
        if self.ground_look == material {
            return;
        }
        self.ground_look = material;
        self.write_ground_slot();
    }

    /// The scene material the endless ground is drawn with (`set_ground_material`): `None`
    /// without one, and once it was removed (the ground then takes its own look again).
    fn ground_look_material(&self) -> Option<&GpuMaterial> {
        self.materials.get(self.ground_look?.0 as usize)?.as_ref()
    }

    /// The row of the material table the endless ground is shaded with: its look's
    /// (`set_ground_material`), else its own (`set_ground`, row 0). Its draw slot names it
    /// (`write_ground_slot`).
    fn ground_row(&self) -> u32 {
        self.ground_look
            .filter(|&id| matches!(self.materials.get(id.0 as usize), Some(Some(_))))
            .map_or(GROUND_ROW, row)
    }

    /// The ground's draw slot, written for the material it is shaded with now (`ground_row`).
    fn write_ground_slot(&mut self) {
        let bytes = mesh_slot(MeshId(u32::MAX - 1), self.ground_row());
        self.queue.write_buffer(
            &self.draw_buffer,
            u64::from(self.ground_slot) * SLOT,
            &bytes,
        );
    }

    /// The sky's source of light: the physical model (default), the
    /// Preetham fallback, or an HDRI (uploaded here; errors for an image
    /// whose size does not match its data or the device limits).
    pub fn set_sky(&mut self, sky: &Sky) -> Result<(), String> {
        self.sky_source = match *sky {
            Sky::Physical => SkySource::Physical,
            Sky::Preetham => SkySource::Preetham,
            Sky::Hdri {
                image,
                rotation_deg,
                intensity,
            } => {
                let max = self.device.limits().max_texture_dimension_2d;
                if image.width > max || image.height > max {
                    return Err(format!(
                        "HDRI {}x{} exceeds {max}",
                        image.width, image.height
                    ));
                }
                let (texels, mean) = hdri::prepare(&image)?;
                let (view, mips) = self.mipmapper.upload_half(
                    &self.device,
                    &self.queue,
                    (image.width, image.height),
                    &texels,
                );
                SkySource::Hdri {
                    view,
                    mips,
                    mean,
                    rotation: rotation_deg.to_radians(),
                    intensity: intensity.max(0.0),
                    bytes: texture::mip_bytes(image.width, image.height, 8),
                }
            }
        };
        self.rebind_globals();
        self.update_ambient(true);
        Ok(())
    }

    /// The albedo of what lies below, for the light it bounces up (the sky's lower half in the
    /// ambient light and reflections); `None` uses the ground plane's colour. Rebuilds the
    /// environment, so callers change it only when it changes noticeably.
    pub fn set_bounce_albedo(&mut self, albedo: Option<[f32; 3]>) {
        let a = albedo.map(Vec3::from);
        if a != self.bounce_albedo {
            self.bounce_albedo = a;
            self.update_ambient(false);
        }
    }

    /// Whether the camera stands in a room: windows then mirror the dim room around it instead
    /// of the sky.
    pub fn set_indoors(&mut self, indoors: bool) {
        self.indoors = indoors;
    }

    pub fn indoors(&self) -> bool {
        self.indoors
    }

    /// The sky's visibility round the buildings (`visioncraft_core::sky_volume`): sample
    /// `(x, y, z)` at `origin + cell * (x, y, z)` holds the visible fraction of directions and
    /// the mean visible direction, x fastest; `room` per sample the lamps' light passed round
    /// the room (lux, linear RGB) and the sky's (scene.wgsl `room_bounce`); the occluders'
    /// voxels, which keep the interpolation from reaching through walls. It dims the
    /// sky's ambient light and reflections where little sky is seen and adds the light bounced
    /// round a room. `None`: open sky everywhere.
    pub fn set_sky_volume(&mut self, volume: Option<SkyVolume<'_>>) {
        let (dims, samples, room, open) = match volume {
            Some((_, _, dims, samples, room, _))
                if dims.iter().all(|&d| d > 0)
                    && samples.len() == dims.iter().map(|&d| d as usize).product::<usize>()
                    && room.len() == samples.len() =>
            {
                (dims, samples, room, false)
            }
            _ => (
                [1, 1, 1],
                &[[1.0, 0.0, 0.0, 0.0]][..],
                &[[0.0, 0.0, 0.0, 1.0]][..],
                true,
            ),
        };
        self.room_volume = room_volume_texture(&self.device, &self.queue, dims, room, samples);
        self.occupancy = match volume.filter(|_| !open) {
            Some((.., (solid, d))) if solid.len() == d.iter().map(|&d| d as usize).product() => {
                occupancy_texture(&self.device, &self.queue, d, solid)
            }
            _ => occupancy_texture(&self.device, &self.queue, [1, 1, 1], &[0]),
        };
        self.sky_volume_box = match volume.filter(|_| !open) {
            Some((origin, cell, ..)) => [
                origin[0],
                origin[1],
                origin[2],
                1.0 / cell,
                1.0 / dims[0] as f32,
                1.0 / dims[1] as f32,
                1.0 / dims[2] as f32,
                1.0,
            ],
            None => [0.0; 8],
        };
        self.rebind_globals();
    }

    /// Changes only the light passed round the rooms of the volume `set_sky_volume` gave (`room`,
    /// one per sample, as there), in place: no new textures, no new bind groups. False (nothing
    /// changed) when the volume there has another number of samples, or there is none.
    pub fn set_sky_volume_light(&mut self, room: &[[f32; 4]]) -> bool {
        let size = self.room_volume.0.size();
        let dims = [size.width / 2, size.height, size.depth_or_array_layers];
        if self.sky_volume_box[7] == 0.0
            || room.len() != dims.iter().map(|&d| d as usize).product::<usize>()
        {
            return false;
        }
        // The room half of each row (`room_volume_texture`): texels 0 to dims[0].
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.room_volume.0,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pack_room_light(room),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8 * dims[0]),
                rows_per_image: Some(dims[1]),
            },
            wgpu::Extent3d {
                width: dims[0],
                height: dims[1],
                depth_or_array_layers: dims[2],
            },
        );
        true
    }

    /// The cloud layer over the analytic skies (an HDRI brings its own). It shows in the sky,
    /// in reflections and in the ambient light, and takes its share of the sun.
    /// Snow over the up-facing surfaces, 0 (none) to 1, except where something lies over them.
    pub fn set_snow(&mut self, amount: f32) {
        self.snow = amount.clamp(0.0, 1.0);
    }

    /// Wet surfaces and rain, each 0 (none) to 1. Rain wets everything: the wetness drawn is the
    /// larger of the two. Wet porous materials darken and turn glossy, puddles form on flat open
    /// ground (`Material::puddles`) from about half wet, rain falls in streaks slanted by the wind
    /// (an orthographic plan has none), and rings spread over puddles and water. Rain also
    /// thickens the clouds to a grey overcast layer and weakens the sun (`cloud_state`,
    /// `sun_share`). Both 0 draws nothing at all.
    pub fn set_wet(&mut self, wetness: f32, rain: f32) {
        self.wetness = wetness.clamp(0.0, 1.0);
        let rain = rain.clamp(0.0, 1.0);
        // The rain closes the sky: a jump starts the clouds' image over.
        self.cloud_jump |= (rain - self.rain).abs() > 0.3;
        if rain != self.rain {
            // The rain closes the sky: the environment and the ambient light follow.
            self.rain = rain;
            self.update_ambient(true);
        }
    }

    /// Mist 0..1: a low layer of water droplets over the ground, under the haze
    /// (`set_atmosphere`), with a scale height of 25 m (`atmosphere::MIST_HEIGHT_M`): the
    /// visibility at the ground falls from 1.3 km at 0.3 to 330 m at 0.6 and 120 m at 1
    /// (`atmosphere::mist_extinction`). It veils every surface, the water, the rain and the sky
    /// (the horizon dissolves), scatters the sun forwards (a glow towards it) and the sky's
    /// light, and dims the sun on the ground. Rain brings its own fog on top
    /// (`atmosphere::rain_extinction`). 0 draws nothing of it.
    pub fn set_mist(&mut self, mist: f32) {
        let mist = mist.clamp(0.0, 1.0);
        if mist != self.mist {
            // The sun on the ground and the light it bounces change with it.
            self.mist = mist;
            self.update_ambient(true);
        }
    }

    /// Light shafts 0..1: the sun's light the haze, the mist and the rain fog scatter towards
    /// the camera is taken out again where the medium lies in shadow (a march through the sun's
    /// cascades, shafts.wgsl), so beams and darker lanes run through the air behind crowns and
    /// past house edges. 1 is the physical amount, 0 (the default) draws nothing of it. Nothing
    /// is computed without a medium to light (`Renderer::shafts_on`), at night, or in a plan or
    /// an elevation.
    pub fn set_shafts(&mut self, strength: f32) {
        self.shafts = strength.clamp(0.0, 1.0);
    }

    /// The sun's glare, 0 (none, the default) to 1 (as a camera lens sees it): a halo of the
    /// sun's light round its direction (`SUN_GLARE`), as bright as the share of its disc the
    /// scene leaves open, so a sun behind leaves glows through their gaps.
    pub fn set_sun_glare(&mut self, strength: f32) {
        self.sun_glare = strength.clamp(0.0, 1.0);
    }

    /// The clear air's own colour on what lies far off (the physical sky only; off by
    /// default): the aerial perspective LUT of Hillaire (EGSR 2020) adds the light the air's
    /// molecules scatter towards the camera and takes out what they absorb, blue at noon and
    /// warm at a low sun, on top of the aerosol haze and the mist. Kilometres off it shows;
    /// in a garden it is a few thousandths.
    pub fn set_aerial_perspective(&mut self, on: bool) {
        self.aerial_perspective = on;
    }

    /// Sky occlusion (off by default): with a grid given (`set_sky_visibility`), every
    /// surface's sky light, its reflections of the sky and a photo's sky rays go through the sky
    /// it sees (scene.wgsl `sky_occ_at`), after Ghost of Tsushima's baked sky visibility.
    pub fn set_sky_occlusion(&mut self, on: bool) {
        self.sky_occlusion = on;
    }

    /// Whether sky occlusion is on and has a grid to draw with.
    pub fn sky_occlusion(&self) -> bool {
        self.sky_occlusion && self.sky_occ_box.is_some()
    }

    /// The baked sky visibility (`visioncraft_core::sky_visibility`), drawn while sky occlusion
    /// is on (`set_sky_occlusion`); `None` forgets it. A grid with impossible sizes is ignored.
    pub fn set_sky_visibility(&mut self, grid: Option<SkyVisibilityGrid<'_>>) {
        let limit = self.device.limits().max_texture_dimension_3d;
        let grid = grid.filter(|g| {
            let [nx, nl, nz] = g.dims;
            nx >= 1
                && nl >= 2
                && nz >= 1
                && 3 * nx <= limit
                && nl <= limit
                && nz <= limit
                && g.cell > 0.0
                && g.height > 0.0
                && g.texels.len() == (3 * nx * nl * nz) as usize
        });
        match grid {
            Some(g) => {
                self.sky_occ = sky_occ_texture(&self.device, &self.queue, g.dims, g.texels);
                self.sky_occ_box = Some([
                    g.origin[0],
                    g.origin[1],
                    1.0 / g.cell,
                    g.lift,
                    g.dims[0] as f32,
                    g.dims[1] as f32,
                    g.dims[2] as f32,
                    g.height,
                ]);
            }
            None => {
                self.sky_occ =
                    sky_occ_texture(&self.device, &self.queue, [1, 1, 1], &[[0.0; 4]; 3]);
                self.sky_occ_box = None;
            }
        }
        self.rebind_globals();
    }

    /// The camera's white balance, before the grade and the tone map (tonemap.wgsl):
    /// `Daylight` (the default) applies nothing.
    pub fn set_white_balance(&mut self, balance: WhiteBalance) {
        self.white_balance = balance;
    }

    /// A colour grade before the tone map (`tonemap::grade_matrix`); `Neutral` (the default)
    /// applies nothing.
    pub fn set_color_grade(&mut self, grade: ColorGrade) {
        self.color_grade = grade;
    }

    /// A second grade mixed over the first by `share` (0..1): a grade that eases from one look
    /// to another. `None`: the first alone.
    pub fn set_color_grade_mix(&mut self, mix: Option<(ColorGrade, f32)>) {
        self.color_grade_mix = mix;
    }

    /// What of the air runs this frame for a camera with `proj` (`Air`).
    fn air(&self, proj: Mat4) -> Air {
        let ortho = shadow::is_orthographic(proj);
        let sun_up = self.sun.direction.y > 0.0 && self.sun.illuminance > 0.0;
        let shafts = if self.shafts_on(ortho, sun_up) {
            self.shafts
        } else {
            0.0
        };
        // The sun near enough the picture to glare into it.
        let glare = if self.sun_glare > 0.0 && !ortho && self.sun.direction.y > -0.02 {
            let clip = (proj * self.view) * (self.eye + self.sun.direction * 1000.0).extend(1.0);
            let ndc = clip.truncate().truncate() / clip.w.max(1e-6);
            if clip.w > 0.0 && ndc.abs().max_element() < 2.0 {
                self.sun_glare
            } else {
                0.0
            }
        } else {
            0.0
        };
        let aerial = self.aerial_perspective
            && !ortho
            && matches!(self.sky_source, SkySource::Physical)
            && self.physical.ready();
        Air {
            shafts,
            glare,
            aerial,
        }
    }

    /// Light shafts are drawn: asked for, the sun up and shining, and a medium thick enough
    /// near the camera that its shadowed part could change a pixel (an optical depth of 0.002
    /// over the shadow distance: mist, rain or a thick haze; a clear day's haze is not).
    fn shafts_on(&self, ortho: bool, sun_up: bool) -> bool {
        if self.shafts <= 0.0 || ortho || !sun_up || self.sun_share() < 0.01 {
            return false;
        }
        let a = &self.atmosphere;
        let height = self.eye.y - self.haze_base();
        let haze = a.haze / 1000.0 * (-height / a.haze_height_m).exp();
        let [m, falloff, _, _] = atmosphere::mist_uniform(self.mist, self.rain, 0.0, 0.0);
        let mist = m * (-falloff * height.max(0.0)).exp();
        (haze + mist) * self.shadow_distance.min(SHAFTS_REACH_M) > 0.002
    }

    /// The height the haze and the mist thin upwards from: the lowest open ground (with a
    /// coast, the sea).
    fn haze_base(&self) -> f32 {
        let ground_height = self.ground.map_or(0.0, |g| g.height);
        self.sea()
            .filter(|s| s.coast.len() >= 2)
            .map_or(ground_height, |s| ground_height.min(s.level))
    }

    /// The lightning flash lit at this moment of the animation clock (`set_time`), if the clouds
    /// are stormy and have lightning (`Clouds::lightning`).
    fn flash(&self) -> Option<lightning::Flash> {
        let (_, _, storm) = self.cloud_state();
        if storm <= 0.0 || matches!(self.sky_source, SkySource::Hdri { .. }) {
            return None;
        }
        lightning::flash_at(self.clock, self.clouds.lightning.clamp(0.0, 1.0) * storm)
    }

    pub fn set_clouds(&mut self, clouds: &Clouds) {
        if *clouds != self.clouds {
            let (a, b) = (&self.clouds, clouds);
            // An eased weather moves a little a frame; a jump starts the clouds' image over.
            self.cloud_jump |= (a.cover - b.cover).abs() > 0.15
                || (a.overcast - b.overcast).abs() > 0.3
                || (a.storm - b.storm).abs() > 0.3
                || (a.altitude_m - b.altitude_m).abs() > 200.0
                || a.volumetric != b.volumetric
                || a.sun_gap != b.sun_gap;
            self.clouds = *clouds;
            self.update_ambient(true);
        }
    }

    /// The display look on top of AgX, as in Filament's AgX looks: `power` above 1 deepens
    /// contrast (1.35 is their "punchy"), `saturation` above 1 lifts colour; (1, 1) is plain AgX.
    pub fn set_grade(&mut self, power: f32, saturation: f32) {
        self.grade = (power.clamp(0.5, 2.0), saturation.clamp(0.0, 2.0));
    }

    /// Bloom strength: the fraction of the image replaced by its blurred
    /// copy (0..1, default 0.025, subtle; `Quality::bloom` switches it).
    pub fn set_bloom(&mut self, strength: f32) {
        self.bloom_strength = strength.clamp(0.0, 1.0);
    }

    /// Counts and timings of the last recorded frame.
    pub fn stats(&self) -> RenderStats {
        self.last_stats.clone()
    }

    /// The globals bind groups and the environment's sky bind group, after
    /// a buffer or the sky source changed.
    fn rebind_globals(&mut self) {
        let sky_view = match &self.sky_source {
            SkySource::Hdri { view, .. } => view,
            SkySource::Physical => &self.physical.sky_view,
            SkySource::Preetham => &self.white,
        };
        for (buffer, bind) in once((&self.globals, &mut self.globals_bind)).chain(
            self.reflect_globals
                .iter()
                .zip(self.reflect_globals_bind.iter_mut()),
        ) {
            *bind = globals_bind_group(
                &self.device,
                &self.layouts.globals,
                buffer,
                &self.env.view,
                &self.env_sampler,
                &self.light_buffer,
                &self.grid_buffer,
                sky_view,
                [&self.room_volume.1, &self.occupancy.1, &self.sky_occ.1],
            );
        }
        self.sky_bind = sky_bind_group(
            &self.device,
            &self.layouts.sky_source,
            &self.globals,
            &self.env_sampler,
            sky_view,
        );
    }

    fn storage_buffer(&self, label: &str, size: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.next_power_of_two(),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn add_mesh(&mut self, mesh: &Mesh) -> Result<MeshId, String> {
        let gpu = self.store_mesh(pack_mesh(mesh)?, None)?;
        self.meshes.push(Some(gpu));
        Ok(MeshId(self.meshes.len() as u32 - 1))
    }

    /// Replaces a mesh in place: every item drawing it picks up the new data. Data of the same
    /// or a smaller size goes where the old was; more takes a new range of the arenas and the old
    /// one is freed.
    pub fn replace_mesh(&mut self, id: MeshId, mesh: &Mesh) -> Result<(), String> {
        self.replace_stored(id, pack_mesh(mesh)?, None)
    }

    /// Puts `packed` (and a terrain chunk's layer `weights`) where mesh `id`'s data was when
    /// it fits, else in a new range of the arenas (the old one is freed).
    fn replace_stored(
        &mut self,
        id: MeshId,
        packed: PackedMesh,
        weights: Option<Vec<u8>>,
    ) -> Result<(), String> {
        let old = match self.meshes.get(id.0 as usize) {
            Some(Some(m)) => m.place,
            Some(None) => return Err("mesh was removed".into()),
            None => return Err("unknown mesh".into()),
        };
        let (vertices, indices) = (
            packed.vertices.len() / VERTEX_STRIDE as usize,
            packed.indices.len() / 4,
        );
        let gpu =
            if vertices <= old.vertices.count as usize && indices <= old.indices.count as usize {
                let arena = &self.arenas[old.arena as usize];
                arena.write_vertices(&self.queue, old.vertices.first, &packed.vertices);
                arena.write_indices(&self.queue, old.indices.first, &packed.indices);
                if let Some(w) = &weights {
                    arena.write_weights(&self.queue, old.vertices.first, w);
                }
                GpuMesh {
                    place: old,
                    index_count: indices as u32,
                    bounds: packed.bounds,
                    wind: packed.wind,
                    weights: weights.is_some(),
                }
            } else {
                let gpu = self.store_mesh(packed, weights)?;
                self.mesh_ranges.release(&old);
                gpu
            };
        self.meshes[id.0 as usize] = Some(gpu);
        self.bounds_dirty = true;
        Ok(())
    }

    /// A mesh's packed data (and a terrain chunk's layer weights) into a range of the arenas.
    fn store_mesh(
        &mut self,
        packed: PackedMesh,
        weights: Option<Vec<u8>>,
    ) -> Result<GpuMesh, String> {
        let (vertices, indices) = (
            packed.vertices.len() as u32 / VERTEX_STRIDE as u32,
            packed.indices.len() as u32 / 4,
        );
        let place = self.place(vertices, indices)?;
        let arena = &self.arenas[place.arena as usize];
        arena.write_vertices(&self.queue, place.vertices.first, &packed.vertices);
        arena.write_indices(&self.queue, place.indices.first, &packed.indices);
        if let Some(w) = &weights {
            arena.write_weights(&self.queue, place.vertices.first, w);
        }
        Ok(GpuMesh {
            place,
            index_count: indices,
            bounds: packed.bounds,
            wind: packed.wind,
            weights: weights.is_some(),
        })
    }

    /// Where a mesh of `vertices` vertices and `indices` indices goes: a free range of an arena;
    /// else the last arena grows (its meshes keep their place: what it holds is copied in front
    /// of the larger buffers, in an encoder submitted here, before any frame draws with it);
    /// else a further arena, once that one is at the device's largest buffer.
    fn place(&mut self, vertices: u32, indices: u32) -> Result<batch::Placement, String> {
        if let Some(p) = self.mesh_ranges.alloc(vertices, indices) {
            return Ok(p);
        }
        let max_buffer = self.device.limits().max_buffer_size;
        let max = [(max_buffer / VERTEX_STRIDE) as u32, (max_buffer / 4) as u32];
        let last = self.mesh_ranges.len().checked_sub(1);
        let (arena, capacity) = match last.and_then(|i| {
            self.mesh_ranges
                .grow(i, vertices, indices, max)
                .map(|c| (i, c))
        }) {
            Some(grown) => grown,
            None => {
                if vertices > max[0] || indices > max[1] {
                    return Err(format!(
                        "a mesh of {vertices} vertices and {indices} indices is larger than the device's largest buffer"
                    ));
                }
                let i = self.mesh_ranges.push();
                let capacity = self
                    .mesh_ranges
                    .grow(i, vertices, indices, max)
                    .expect("a new arena takes the mesh");
                (i, capacity)
            }
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        if let Some(a) = self.arenas.get_mut(arena) {
            a.grow(&self.device, &mut encoder, capacity);
        } else {
            self.arenas
                .push(batch::MeshArena::new(&self.device, capacity, VERTEX_STRIDE));
        }
        self.mesh_ranges.set_capacity(arena, capacity.0, capacity.1);
        self.queue.submit([encoder.finish()]);
        self.mesh_ranges
            .alloc(vertices, indices)
            .ok_or_else(|| "no room in the arena that was just made for it".into())
    }

    /// A terrain chunk: a mesh (add it with `add_item` and any material,
    /// which is ignored) shaded by the layers of `set_terrain_layers`
    /// blended by its per-vertex weights. Drawn, shadowed and culled like
    /// any opaque mesh.
    pub fn add_terrain_chunk(&mut self, chunk: &TerrainChunk) -> Result<MeshId, String> {
        let gpu = self.upload_terrain(chunk)?;
        self.meshes.push(Some(gpu));
        Ok(MeshId(self.meshes.len() as u32 - 1))
    }

    /// Replaces a chunk in place (sculpting, painting).
    pub fn replace_terrain_chunk(
        &mut self,
        id: MeshId,
        chunk: &TerrainChunk,
    ) -> Result<(), String> {
        self.replace_stored(
            id,
            pack_mesh(&Mesh {
                positions: chunk.positions,
                normals: chunk.normals,
                indices: chunk.indices,
                ..Default::default()
            })?,
            Some(terrain::weight_bytes(chunk)?),
        )
    }

    fn upload_terrain(&mut self, chunk: &TerrainChunk) -> Result<GpuMesh, String> {
        let packed = pack_mesh(&Mesh {
            positions: chunk.positions,
            normals: chunk.normals,
            indices: chunk.indices,
            ..Default::default()
        })?;
        self.store_mesh(packed, Some(terrain::weight_bytes(chunk)?))
    }

    /// The terrain's material layers (1..=8; weight k of a chunk vertex
    /// selects layer k). Their maps are resampled to 512^2 into three
    /// texture arrays, so the source textures may be removed afterwards.
    pub fn set_terrain_layers(&mut self, layers: &[TerrainLayer]) -> Result<(), String> {
        let textures = &self.textures;
        self.terrain.set_layers(
            &self.device,
            &self.queue,
            &self.layouts.terrain,
            &self.mipmapper,
            layers,
            |id| Some(&textures.get(id?.0 as usize)?.as_ref()?.view),
        )
    }

    /// Bakes an impostor of a plant: its meshes and materials (one level of
    /// detail, usually the finest) in model space, seen from `views` (4..=16)
    /// azimuths around the vertical plus once from above, into a colour and
    /// a normal atlas of `Quality::impostor_resolution`^2 tiles (256 up to
    /// HIGH, 512 on ULTRA). `bounds` defaults to the meshes'. One
    /// per species variant and season: the colours are baked in.
    pub fn bake_impostor(
        &mut self,
        parts: &[(MeshId, MaterialId)],
        bounds: Option<(Vec3, Vec3)>,
        views: u32,
    ) -> Result<ImpostorId, String> {
        let mut bake = self.start_impostor_bake(parts, bounds, views)?;
        loop {
            if let Some(done) = self.impostor_bake_step(&mut bake, u32::MAX) {
                return done;
            }
        }
    }

    /// Makes and touches a texture of about `mb` megabytes, then lets it go: the GPU memory it
    /// took stays with the device for the textures a load makes next. Without it, the first use
    /// of a new texture mid-load (an impostor atlas, a streamed picture) could stall a frame by
    /// up to 100 ms on an integrated GPU while the driver took and cleared fresh memory.
    pub fn warm_texture_memory(&mut self, mb: u32) {
        let max = self.device.limits().max_texture_dimension_2d;
        let width = 4096.min(max);
        let height = ((mb as u64) << 20)
            .div_ceil(width as u64 * 4)
            .min(max as u64) as u32;
        if height == 0 {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("memory warm-up"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("memory warm-up"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    // Stored: a discarded clear may never touch the memory.
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.queue.submit([encoder.finish()]);
    }

    /// Makes `count` impostor atlases for `views` views at the quality's resolution ahead of a
    /// load, and touches them, so bakes during the load do not stall a frame on fresh memory.
    pub fn reserve_impostors(&mut self, views: u32, count: usize) {
        self.baker.reserve(
            &self.device,
            &self.queue,
            views.clamp(4, 16),
            self.quality.impostor_resolution,
            count,
        );
    }

    /// `bake_impostor` spread over frames: start it here, then call `impostor_bake_step` once
    /// a frame until it returns the impostor. Each step submits a few views (or the final
    /// dilation and mips), so no frame carries a whole atlas.
    pub fn start_impostor_bake(
        &mut self,
        parts: &[(MeshId, MaterialId)],
        bounds: Option<(Vec3, Vec3)>,
        views: u32,
    ) -> Result<ImpostorBake, String> {
        let (_, bounds) = self.bake_list(parts, bounds)?;
        let state = self.baker.begin(
            &self.device,
            &self.queue,
            &self.layouts,
            bounds,
            views.clamp(4, 16),
            self.quality.impostor_resolution,
        );
        Ok(ImpostorBake {
            parts: parts.to_vec(),
            state: Some(state),
        })
    }

    /// One step of a bake: `views` more views drawn (`u32::MAX`: all), or when all are, the
    /// dilation and mips. `Some` once the impostor is made (or failed: a part went away).
    pub fn impostor_bake_step(
        &mut self,
        bake: &mut ImpostorBake,
        views: u32,
    ) -> Option<Result<ImpostorId, String>> {
        let mut state = bake.state.take()?;
        if impostor::Baker::drawn(&state) {
            let gpu = self.baker.finish(
                &self.device,
                &self.queue,
                &self.layouts,
                &self.mipmapper,
                state,
            );
            self.impostors.push(Some(gpu));
            return Some(Ok(ImpostorId(self.impostors.len() as u32 - 1)));
        }
        let list = match self.bake_list(&bake.parts, None) {
            Ok((list, _)) => list,
            Err(e) => return Some(Err(e)),
        };
        self.baker
            .render(&self.device, &self.queue, &mut state, &list, views);
        bake.state = Some(state);
        None
    }

    /// The arena buffers, index ranges and bind groups of `parts`, and the bounds to bake
    /// (given, else theirs).
    fn bake_list(
        &self,
        parts: &[(MeshId, MaterialId)],
        bounds: Option<(Vec3, Vec3)>,
    ) -> Result<(Vec<impostor::BakePart<'_>>, (Vec3, Vec3)), String> {
        let mut list = Vec::new();
        let mut union_bounds: Option<Bounds> = None;
        for &(mesh, material) in parts {
            let m = self
                .meshes
                .get(mesh.0 as usize)
                .and_then(Option::as_ref)
                .ok_or("unknown or removed mesh")?;
            let mat = self
                .materials
                .get(material.0 as usize)
                .and_then(Option::as_ref)
                .ok_or("unknown or removed material")?;
            if let Some(b) = m.bounds {
                union_bounds = Some(union_bounds.map_or(b, |u| union(u, b)));
            }
            let dm = m.draw(mesh);
            let arena = &self.arenas[dm.arena as usize];
            let set = self
                .texture_sets
                .get(mat.set as usize)
                .and_then(Option::as_ref)
                .ok_or("material without a texture set")?;
            list.push(impostor::BakePart {
                vertices: &arena.vertices,
                indices: &arena.indices,
                mesh: dm,
                material: &set.bind,
                identity: &mat.identity,
                has_base: mat.has_base,
            });
        }
        let bounds = bounds.or(union_bounds).ok_or("nothing to bake")?;
        Ok((list, bounds))
    }

    /// Makes `impostor` the last level of detail of `item`: beyond its last
    /// mesh level's distance (which must be finite) up to `max_distance`
    /// (`f32::INFINITY`: never culled). Around the hand-over distance the
    /// mesh and the quads are dithered per instance over a band of 10 %
    /// (at least 2 m), so nothing pops.
    /// Shadows of this item use the level of detail it is drawn with, not a coarser one or its
    /// impostor (for hedges and other clipped plants, whose shadow is a continuous band).
    pub fn set_item_shadow_exact(&mut self, item: ItemId, exact: bool) {
        if let Some(Some(it)) = self.items.get_mut(item.0 as usize) {
            it.shadow_exact = exact;
        }
    }

    /// The item casts shadows into the cascades from index `first` on only: 0 (the default)
    /// all of them, a `first` past the last cascade none at all. For far cells of a large scene,
    /// which need no shadow in the metres a near cascade covers. Which cascades a frame
    /// refreshes does not depend on it; the snow cover map takes every item as before.
    pub fn set_item_shadow_cascades(&mut self, item: ItemId, first: u8) {
        if let Some(Some(it)) = self.items.get_mut(item.0 as usize) {
            it.shadow_from = first;
        }
    }

    /// A mesh that casts the item's shadow in every level past the finest (`add_mesh`, same
    /// vertex layout, `uv.x` the share of shadow texels it covers).
    pub fn set_item_shadow_caster(&mut self, item: ItemId, mesh: MeshId) {
        if let Some(Some(it)) = self.items.get_mut(item.0 as usize) {
            it.shadow_caster = Some(mesh);
        }
    }

    pub fn set_item_impostor(
        &mut self,
        item: ItemId,
        impostor: ImpostorId,
        max_distance: f32,
    ) -> Result<(), String> {
        if !matches!(self.impostors.get(impostor.0 as usize), Some(Some(_))) {
            return Err("unknown or removed impostor".into());
        }
        let slot = self.alloc_slot(MeshId(u32::MAX - 2), FROM_INSTANCE);
        self.impostor_slots_dirty = true;
        let Some(Some(it)) = self.items.get_mut(item.0 as usize) else {
            self.free_slots.push(slot);
            return Err("unknown or removed item".into());
        };
        let last = it.lods.last().copied().unwrap_or(f32::INFINITY);
        if !last.is_finite() {
            self.free_slots.push(slot);
            return Err("the item's last mesh level has no finite distance".into());
        }
        if let Some((_, old)) = it.impostor.take() {
            it.lods.pop();
            self.free_slots.push(old);
        }
        it.lods.push(max_distance.max(last + 1.0));
        it.impostor = Some((impostor, slot));
        for c in &mut it.clusters {
            c.lod = LOD_UNSET;
            c.target = LOD_UNSET;
            c.fade = 0;
        }
        Ok(())
    }

    /// Instances of `item` that cross its last level's (finite) distance
    /// dissolve over a few frames instead of vanishing at once (lawn tufts,
    /// ground cover); a parked camera shows no dither. Any `band_m` above 0
    /// switches it on (the value is no longer a distance band), 0 off.
    /// Items with an impostor dissolve mesh to impostor instead, and
    /// shadows never fade.
    pub fn set_item_cull_fade(&mut self, item: ItemId, band_m: f32) {
        if let Some(Some(it)) = self.items.get_mut(item.0 as usize) {
            it.cull_fade = band_m.max(0.0);
        }
    }

    /// Frees an impostor; items using it draw nothing at that level.
    pub fn remove_impostor(&mut self, id: ImpostorId) {
        if let Some(slot) = self.impostors.get_mut(id.0 as usize) {
            *slot = None;
        }
    }

    /// Whether the device takes BC-compressed textures (`bc::compress`, `add_compressed`).
    pub fn supports_bc(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    }

    /// Uploads a block-compressed image with its mip chain (`bc::compress`) at once. Fails
    /// when the device has no BC formats: the caller keeps RGBA8.
    pub fn add_compressed(
        &mut self,
        c: std::sync::Arc<bc::Compressed>,
        repeat: bool,
    ) -> Result<TextureId, String> {
        let mut job = UploadJob::new(c, repeat);
        loop {
            if let Some(r) = self.upload_step(&mut job, usize::MAX) {
                return r;
            }
        }
    }

    /// Uploads part of a block-compressed image: at most `max_bytes` (whole block rows, at
    /// least one), so a large map spreads over frames. `Some` once the texture is complete
    /// (only then is it usable), or failed.
    pub fn upload_step(
        &mut self,
        job: &mut UploadJob,
        max_bytes: usize,
    ) -> Option<Result<TextureId, String>> {
        let c = job.c.clone();
        if job.texture.is_none() {
            if !self.supports_bc() {
                return Some(Err("no BC texture formats on this device".into()));
            }
            let max = self.device.limits().max_texture_dimension_2d;
            if c.width == 0 || c.height == 0 || c.width > max || c.height > max {
                return Some(Err(format!(
                    "texture size {}x{} outside 1..={max}",
                    c.width, c.height
                )));
            }
            job.texture = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("compressed material texture"),
                size: wgpu::Extent3d {
                    width: c.width,
                    height: c.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: c.levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: c.format(),
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            }));
        }
        let texture = job.texture.as_ref()?;
        let block = bc::Compressed::block_bytes(c.kind) as u32;
        let mut budget = max_bytes;
        while job.level < c.levels.len() {
            let i = job.level;
            let (w, h) = ((c.width >> i).max(1), (c.height >> i).max(1));
            let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
            let row_bytes = (bw * block) as usize;
            let rows = ((budget / row_bytes.max(1)) as u32)
                .max(1)
                .min(bh - job.row);
            let start = job.row as usize * row_bytes;
            let data = &c.levels[i][start..start + rows as usize * row_bytes];
            let dst = wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: i as u32,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: job.row * 4,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            };
            let size = wgpu::Extent3d {
                width: bw * 4,
                height: rows * 4,
                depth_or_array_layers: 1,
            };
            // Streamed parts go through the belt; a whole image at once (`add_compressed`)
            // and the web, where the browser stages `writeTexture` itself, write directly.
            #[cfg(not(target_arch = "wasm32"))]
            if max_bytes != usize::MAX {
                let padded =
                    row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
                let total = (padded * rows as usize) as u64;
                let mut encoder = self.device.create_command_encoder(&Default::default());
                let slice = self.upload_belt.allocate(
                    wgpu::BufferSize::new(total)?,
                    wgpu::BufferSize::new(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64)?,
                );
                let offset = slice.offset();
                let buffer = slice.buffer().clone();
                {
                    let Ok(mut view) = slice.get_mapped_range_mut() else {
                        return Some(Err("the upload staging memory is not mapped".into()));
                    };
                    for (k, row) in data.chunks_exact(row_bytes).enumerate() {
                        view.slice(k * padded..k * padded + row_bytes)
                            .copy_from_slice(row);
                    }
                }
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset,
                            bytes_per_row: Some(padded as u32),
                            rows_per_image: Some(rows),
                        },
                    },
                    dst,
                    size,
                );
                self.upload_belt.finish_and_recall_on_submit(&encoder);
                self.queue.submit([encoder.finish()]);
            } else {
                self.queue.write_texture(
                    dst,
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bw * block),
                        rows_per_image: Some(rows),
                    },
                    size,
                );
            }
            #[cfg(target_arch = "wasm32")]
            self.queue.write_texture(
                dst,
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bw * block),
                    rows_per_image: Some(rows),
                },
                size,
            );
            budget = budget.saturating_sub(rows as usize * row_bytes);
            job.row += rows;
            if job.row == bh {
                job.level += 1;
                job.row = 0;
            }
            if budget == 0 {
                break;
            }
        }
        if job.level < c.levels.len() {
            return None;
        }
        let texture = job.texture.take()?;
        self.textures.push(Some(texture::GpuTexture {
            view: texture.create_view(&Default::default()),
            repeat: job.repeat,
            bytes: c.bytes(),
        }));
        Some(Ok(TextureId(self.textures.len() as u32 - 1)))
    }

    /// GPU memory of a texture with its mips (0 for a freed or unknown one).
    pub fn texture_bytes(&self, id: TextureId) -> u64 {
        self.textures
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .map_or(0, |t| t.bytes)
    }

    /// Frees a mesh. Items still referring to it draw nothing.
    pub fn remove_mesh(&mut self, id: MeshId) {
        if let Some(slot) = self.meshes.get_mut(id.0 as usize)
            && let Some(old) = slot.take()
        {
            self.mesh_ranges.release(&old.place);
            self.bounds_dirty = true;
        }
    }

    /// Uploads an RGBA8 image and generates its full mip chain on the GPU.
    pub fn add_texture(&mut self, desc: &TextureDesc) -> Result<TextureId, String> {
        let gpu = self.mipmapper.upload(&self.device, &self.queue, desc)?;
        self.textures.push(Some(gpu));
        Ok(TextureId(self.textures.len() as u32 - 1))
    }

    /// Frees a texture. Materials using it keep drawing with it until their
    /// next `set_material`, which then falls back to no map.
    pub fn remove_texture(&mut self, id: TextureId) {
        if let Some(slot) = self.textures.get_mut(id.0 as usize) {
            *slot = None;
        }
    }

    pub fn add_material(&mut self, material: &Material) -> MaterialId {
        let id = MaterialId(self.materials.len() as u32);
        self.ensure_rows(row(id) + 1);
        // Its set until `set_material` gives it the real one.
        let (set, _) = self.sets.acquire([None; 6]);
        // An instance that is the identity with the material's row in `m1.w`: what the bake
        // draws the material's parts with (`write_instances` does the same for the pool).
        let identity = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("material identity instance"),
            size: batch::INSTANCE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut instance = [0.0; 16];
        for i in [0, 5, 10, 15] {
            instance[i] = 1.0;
        }
        instance[7] = f32::from_bits(row(id));
        self.queue.write_buffer(&identity, 0, &f32_bytes(&instance));
        self.materials.push(Some(GpuMaterial {
            set,
            cpu: *material,
            has_base: false,
            identity,
        }));
        self.set_material(id, material);
        id
    }

    /// Room for `rows` rows of the material table: a larger buffer (twice this one, or just
    /// enough) with every row copied in front, and every texture set's bind group made again.
    fn ensure_rows(&mut self, rows: u32) {
        if rows <= self.material_capacity {
            return;
        }
        let size = table_size(
            rows,
            self.material_capacity,
            self.device.limits().max_storage_buffer_binding_size,
        );
        let table = self.storage_buffer("materials", size);
        self.queue.write_buffer(&table, 0, &self.material_bytes);
        self.material_table = table;
        self.material_capacity = u32::try_from(size / MATERIAL_SIZE).expect("rows fit a u32");
        self.material_bytes
            .resize((self.material_capacity * MATERIAL_SIZE as u32) as usize, 0);
        let maps: Vec<_> = self
            .texture_sets
            .iter()
            .map(|s| s.as_ref().map(|s| s.maps))
            .collect();
        let binds: Vec<_> = maps
            .iter()
            .map(|maps| maps.map(|maps| self.texture_set_bind(maps)))
            .collect();
        for (set, bind) in self.texture_sets.iter_mut().zip(binds) {
            if let Some((set, bind)) = set.as_mut().zip(bind) {
                set.bind = bind;
            }
        }
    }

    /// Writes a row of the material table (and its CPU copy).
    fn write_row(&mut self, row: u32, bytes: &[u8]) {
        let at = row as usize * MATERIAL_SIZE as usize;
        self.material_bytes[at..at + bytes.len()].copy_from_slice(bytes);
        self.queue
            .write_buffer(&self.material_table, at as u64, bytes);
    }

    /// Group 1 of the maps `maps`: the material table and the six maps with their samplers
    /// (absent ones: the white texture, repeating).
    fn texture_set_bind(&self, maps: [Option<TextureId>; 6]) -> wgpu::BindGroup {
        let maps =
            maps.map(
                |id| match id.and_then(|id| self.textures.get(id.0 as usize)?.as_ref()) {
                    Some(t) => (
                        &t.view,
                        if t.repeat {
                            &self.repeat_sampler
                        } else {
                            &self.clamp_sampler
                        },
                    ),
                    None => (&self.white, &self.repeat_sampler),
                },
            );
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: self.material_table.as_entire_binding(),
        }];
        for (i, (view, sampler)) in maps.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 7 + i as u32,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("texture set"),
            layout: &self.layouts.material,
            entries: &entries,
        })
    }

    /// Makes the bind group of the texture set `set` (newly acquired) over `maps`.
    fn put_set(&mut self, set: u32, maps: [Option<TextureId>; 6]) {
        let bind = self.texture_set_bind(maps);
        if set as usize == self.texture_sets.len() {
            self.texture_sets.push(None);
        }
        self.texture_sets[set as usize] = Some(GpuTextureSet { bind, maps });
    }

    pub fn set_material(&mut self, id: MaterialId, material: &Material) {
        let Some(Some(m)) = self.materials.get(id.0 as usize) else {
            return;
        };
        let (old_set, old_maps) = (m.set, m.cpu.maps());
        let maps = material.maps();
        let present = |t: Option<TextureId>| {
            t.and_then(|t| self.textures.get(t.0 as usize)?.as_ref())
                .is_some()
        };
        let bits = texture_bits(maps.map(present), material.occlusion_map.is_none());
        let (set, new) = self.sets.acquire(maps);
        if new {
            self.put_set(set, maps);
        }
        if self.sets.release(old_maps, old_set) {
            self.texture_sets[old_set as usize] = None;
        }
        self.write_row(row(id), &material_bytes(material, bits));
        let Some(Some(m)) = self.materials.get_mut(id.0 as usize) else {
            return;
        };
        m.set = set;
        m.cpu = *material;
        m.has_base = (bits & 1) != 0;
    }

    /// Frees a material. Items still using it draw nothing; the id is not
    /// reused. A ground drawn with it goes back to its own look.
    pub fn remove_material(&mut self, id: MaterialId) {
        let look = self.ground_look == Some(id);
        if let Some(slot) = self.materials.get_mut(id.0 as usize)
            && let Some(m) = slot.take()
            && self.sets.release(m.cpu.maps(), m.set)
        {
            self.texture_sets[m.set as usize] = None;
        }
        if look {
            self.write_ground_slot();
        }
    }

    /// A draw item: one mesh, one material, any number of instance
    /// transforms (model-to-world). One instance buffer and one draw call.
    pub fn add_item(&mut self, mesh: MeshId, material: MaterialId, instances: &[Mat4]) -> ItemId {
        self.add_lod_item(&[(mesh, material, f32::INFINITY)], instances)
    }

    /// An item of several (mesh, material, max distance) parts sharing one
    /// instance list. Parts with the same max distance form one level of
    /// detail (a plant's bark and foliage); levels are chosen per 8 m cell
    /// of instances by the distance from the camera to the cell, with 1 m of
    /// hysteresis, and cells beyond the largest distance are culled (use
    /// `f32::INFINITY` to never cull). Shadows draw one level coarser than
    /// the view; the cached far cascade draws the coarsest.
    pub fn add_lod_item(
        &mut self,
        parts: &[(MeshId, MaterialId, f32)],
        instances: &[Mat4],
    ) -> ItemId {
        let mut lods: Vec<f32> = parts.iter().map(|p| p.2.max(0.0)).collect();
        lods.sort_by(f32::total_cmp);
        lods.dedup();
        lods.truncate(LOD_UNSET as usize - 1);
        let capacity = self.draw_capacity;
        let parts: Vec<Part> = parts
            .iter()
            .map(|&(mesh, material, distance)| Part {
                mesh,
                material,
                lod: lods
                    .iter()
                    .position(|&d| d == distance.max(0.0))
                    .unwrap_or(lods.len() - 1) as u8,
                slot: self.alloc_slot(mesh, row(material)),
            })
            .collect();
        // Terrain chunks cast with the level they are drawn with: a coarser level lies above the
        // finer one wherever the ground is concave (the foot of a bank) and shadowed the beach
        // below it in wide sawtoothed wedges.
        let terrain = parts.iter().any(|p| {
            self.meshes
                .get(p.mesh.0 as usize)
                .and_then(Option::as_ref)
                .is_some_and(|m| m.weights)
        });
        let id = ItemId(self.items.len() as u32);
        self.visibility.dirty = true;
        self.items.push(Some(Item {
            parts,
            lods,
            transforms: Vec::new(),
            base: 0,
            cap: 0,
            bounds: None,
            clusters: Vec::new(),
            blocks: Vec::new(),
            block_of: Vec::new(),
            size: 0.0,
            impostor: None,
            shadow_exact: terrain,
            shadow_from: 0,
            shadow_caster: None,
            cull_fade: 0.0,
            follows_reach: false,
            conifer: false,
            grass: false,
            moved: false,
        }));
        // The slots again when a later part's `alloc_slot` regrew the draw-slot buffer: its
        // rewrite covers the parts of the items there are, and this one was not among them yet,
        // so its earlier parts read zero there (the ground's material, mesh 0) until it is.
        if self.draw_capacity != capacity
            && let Some(Some(item)) = self.items.get(id.0 as usize)
        {
            for p in &item.parts {
                let bytes = mesh_slot(p.mesh, row(p.material));
                self.queue
                    .write_buffer(&self.draw_buffer, u64::from(p.slot) * SLOT, &bytes);
            }
        }
        self.set_instances(id, instances);
        id
    }

    fn alloc_slot(&mut self, mesh: MeshId, material: u32) -> u32 {
        let slot = self.free_slots.pop().unwrap_or_else(|| {
            self.next_slot += 1;
            self.next_slot - 1
        });
        if slot as usize >= self.draw_capacity {
            self.draw_capacity = (slot as usize + 1).next_power_of_two();
            self.draw_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("draws"),
                size: SLOT * self.draw_capacity as u64,
                usage: draw_slot_usage(),
                mapped_at_creation: false,
            });
            self.draw_bind = draw_bind_group(&self.device, &self.layouts.draw, &self.draw_buffer);
            self.impostor_slots_dirty = true;
            // The ground's slot: the row it is shaded with now (a look's, else its own).
            self.write_ground_slot();
            // The gathered batches' slot too: a new buffer starts at zero, and row 0 is the
            // ground's material (every instance of a batch then shaded like the ground).
            self.queue.write_buffer(
                &self.draw_buffer,
                self.batch_slot as u64 * SLOT,
                &mesh_slot(MeshId(FROM_INSTANCE), FROM_INSTANCE),
            );
            for item in self.items.iter().flatten() {
                for p in &item.parts {
                    self.queue.write_buffer(
                        &self.draw_buffer,
                        p.slot as u64 * SLOT,
                        &mesh_slot(p.mesh, row(p.material)),
                    );
                }
            }
        }
        self.queue.write_buffer(
            &self.draw_buffer,
            slot as u64 * SLOT,
            &mesh_slot(mesh, material),
        );
        slot
    }

    pub fn set_instances(&mut self, id: ItemId, instances: &[Mat4]) {
        let n = instances.len() as u32;
        let Some(Some(item)) = self.items.get_mut(id.0 as usize) else {
            return;
        };
        // The last row but the wind's frequency (x's w) is spare: gathered instances carry their
        // material and mesh there (`Gather`).
        assert!(
            instances
                .iter()
                .all(|m| m.y_axis.w == 0.0 && m.z_axis.w == 0.0 && m.w_axis.w == 1.0),
            "instance matrices must be affine (last row 0, 0, 0, 1)"
        );
        // More than its range holds: a new range, or (none left) a larger pool.
        let mut repack = false;
        if n > item.cap {
            self.pool.ranges.release(item.base, item.cap);
            (item.base, item.cap) = match self.pool.ranges.alloc(n) {
                Some(base) => (base, n),
                None => {
                    repack = true;
                    (0, 0)
                }
            };
        }
        // Group by ground cell: sorted by (cell), each cluster is a contiguous range. This
        // reorders the instances on the GPU; fine while nothing reads `instance_index`
        // (per-instance data later must be permuted the same way, which `instance_order`
        // gives: the same cell, the same stable sort). The cells keep this order, not one by
        // 128 m block: it is the order the pictures were made in, and it decides which of two
        // equally distant leaves shows and which dissolving clusters get a fade slot when
        // more dissolve than there are slots.
        let mut sorted: Vec<_> = instances.iter().map(|m| (cluster_cell(m), *m)).collect();
        sorted.sort_by_key(|(p, _)| *p);
        let places: Vec<_> = sorted.iter().map(|(p, _)| *p).collect();
        item.clusters = same_ranges(&places, |a, b| *a == *b)
            .into_iter()
            .map(|range| Cluster {
                range,
                bounds: None,
                lod: LOD_UNSET,
                far: LOD_UNSET,
                target: LOD_UNSET,
                key: [0.0; 2],
                fade: 0,
                to_far: false,
                fading: false,
                fade_slot: 0,
            })
            .collect();
        // The 128 m blocks over the cells, in order of first appearance; each cluster knows
        // its block.
        item.blocks.clear();
        item.block_of.clear();
        item.block_of.reserve(item.clusters.len());
        let mut seen: std::collections::HashMap<
            (i32, i32),
            u32,
            std::hash::BuildHasherDefault<Fx>,
        > = Default::default();
        for c in &item.clusters {
            let p = places[c.range.start as usize];
            let key = (p.0.div_euclid(BLOCK_CELLS), p.1.div_euclid(BLOCK_CELLS));
            let next = item.blocks.len() as u32;
            item.block_of.push(*seen.entry(key).or_insert_with(|| {
                item.blocks.push(Block::default());
                next
            }));
        }
        item.transforms = sorted.into_iter().map(|(_, m)| m).collect();
        if repack {
            self.repack_pool();
        } else {
            self.write_instances(id);
        }
        self.visibility.dirty = true;
        self.bounds_dirty = true;
    }

    /// Where `set_instances` puts each of `matrices`: instance `i` of the list becomes
    /// instance `order[i]` of the item (its instances are sorted by ground cell, stably).
    pub fn instance_order(matrices: &[Mat4]) -> Vec<u32> {
        let mut order: Vec<u32> = (0..matrices.len() as u32).collect();
        order.sort_by_key(|&i| cluster_cell(&matrices[i as usize]));
        let mut at = vec![0u32; order.len()];
        for (pos, &i) in order.iter().enumerate() {
            at[i as usize] = pos as u32;
        }
        at
    }

    /// Replaces instance `index` of an item (numbered as [`Self::instance_order`] counts them)
    /// where it is: the same range in the pool, the same cluster. Its bounds, the scene's and
    /// the shadows are made again once when the frame is drawn, however many instances moved.
    /// For the few instances that move every frame (the parts of a model whose clip plays):
    /// nothing is sorted or allocated.
    pub fn set_instance(&mut self, id: ItemId, index: usize, model_to_world: Mat4) {
        // As `set_instances`: the last row but the wind's frequency (x's w) stays spare, a
        // gathered instance carries its material and mesh there (`Gather`).
        assert!(
            model_to_world.y_axis.w == 0.0
                && model_to_world.z_axis.w == 0.0
                && model_to_world.w_axis.w == 1.0,
            "instance matrices must be affine (last row 0, 0, 0, 1)"
        );
        let Some(mut item) = self.items.get_mut(id.0 as usize).and_then(Option::take) else {
            return;
        };
        if index >= item.transforms.len() {
            self.items[id.0 as usize] = Some(item);
            return;
        }
        item.transforms[index] = model_to_world;
        let mut m = model_to_world;
        if let Some(h) = self.wind_height(&item) {
            let hz = wind::plant_hz(h * m.x_axis.truncate().length(), item.conifer);
            m.x_axis.w = if item.grass { -hz } else { hz };
        }
        // The matrix as the pool takes it, on the stack: no buffer is allocated per frame.
        let mut bytes = [0u8; batch::INSTANCE_BYTES as usize];
        for (i, f) in m.to_cols_array().iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
        }
        self.queue.write_buffer(
            &self.pool.buffer,
            u64::from(item.base + index as u32) * batch::INSTANCE_BYTES,
            &bytes,
        );
        // Listed once until the frame takes the list, however many of its instances move.
        if !item.moved {
            item.moved = true;
            self.moved.push(id.0);
        }
        self.items[id.0 as usize] = Some(item);
    }

    /// The bounds of the items `set_instance` moved, and the scene's, made again (once a frame).
    fn update_moved(&mut self) {
        if self.moved.is_empty() {
            return;
        }
        let mut moved = std::mem::take(&mut self.moved);
        moved.sort_unstable();
        moved.dedup();
        for &i in &moved {
            let Some(mut item) = self.items.get_mut(i as usize).and_then(Option::take) else {
                continue;
            };
            let mesh_bounds = self.item_mesh_bounds(&item);
            // The clusters', the blocks' and the item's: a moved instance may have left its
            // cluster's old bounds, and a block that kept them would step over clusters that
            // should draw (`update_lods`).
            item_bounds(&mut item, mesh_bounds);
            item.moved = false;
            self.items[i as usize] = Some(item);
        }
        // Reused next frame: nothing is allocated while parts move.
        moved.clear();
        self.moved = moved;
        self.scene_bounds_of_all();
    }

    /// A larger pool for every item's instances, written again.
    fn repack_pool(&mut self) {
        let needed: u32 = self
            .items
            .iter()
            .flatten()
            .map(|i| i.transforms.len() as u32)
            .sum();
        self.pool.grow(&self.device, needed);
        for item in self.items.iter_mut().flatten() {
            let n = item.transforms.len() as u32;
            item.base = self.pool.ranges.alloc(n).unwrap_or(0);
            item.cap = n;
        }
        for i in 0..self.items.len() {
            self.write_instances(ItemId(i as u32));
        }
    }

    /// How tall the tallest wind-blown mesh an item draws is in its own space (`None` for an
    /// item nothing of which sways): from it and the scale drawn, each instance's sway
    /// frequency (`write_instances`).
    fn wind_height(&self, item: &Item) -> Option<f32> {
        item.parts
            .iter()
            .filter_map(|p| self.meshes.get(p.mesh.0 as usize).and_then(Option::as_ref))
            .filter(|m| m.wind)
            .filter_map(|m| m.bounds)
            .map(|(_, hi)| hi.y)
            .reduce(f32::max)
    }

    /// Uploads an item's instance matrices. The spare w of each matrix's first column carries
    /// the plant's own sway frequency (Hz; `wind::plant_hz` of its height in the world), which
    /// the vertex shaders read in every pass: no extra attribute, no per-draw uniform (the depth
    /// and shadow passes bind none).
    fn write_instances(&mut self, id: ItemId) {
        let Some(Some(item)) = self.items.get(id.0 as usize) else {
            return;
        };
        let height = self.wind_height(item);
        let mut b = Bytes::default();
        for m in &item.transforms {
            let mut m = *m;
            if let Some(h) = height {
                let hz = wind::plant_hz(h * m.x_axis.truncate().length(), item.conifer);
                m.x_axis.w = if item.grass { -hz } else { hz };
            }
            b.floats(&m.to_cols_array());
        }
        if !b.0.is_empty() {
            self.queue.write_buffer(
                &self.pool.buffer,
                u64::from(item.base) * batch::INSTANCE_BYTES,
                &b.0,
            );
        }
    }

    /// Where GPU grass grows (`grass.rs`), or none. Drawn when `Quality::grass` is above 0; the
    /// field stays across quality changes.
    pub fn set_grass(&mut self, field: Option<&GrassField>) {
        self.grass.set_field(&self.device, &self.queue, field);
    }

    /// A grass mask is set (`set_grass`).
    pub fn has_grass(&self) -> bool {
        self.grass.has_field()
    }

    /// An item's plants are lawn grass: with gusts on they bend far more than a stem of wood
    /// (`common.wgsl` `wind_sway`). Carried as a negative sway frequency.
    pub fn set_item_grass(&mut self, id: ItemId, grass: bool) {
        if let Some(Some(item)) = self.items.get_mut(id.0 as usize)
            && item.grass != grass
        {
            item.grass = grass;
            self.write_instances(id);
            // Grass in gusts needs more room in its culling bounds (`wind::bounds_pad`).
            self.bounds_dirty = true;
        }
    }

    /// An item's plants sway like conifers (stiffer: a higher natural frequency than a leafed
    /// broadleaf of the same height, `wind::plant_hz`).
    pub fn set_item_conifer(&mut self, id: ItemId, conifer: bool) {
        if let Some(Some(item)) = self.items.get_mut(id.0 as usize)
            && item.conifer != conifer
        {
            item.conifer = conifer;
            self.write_instances(id);
        }
    }

    /// Items drawn (for tests that nothing is left behind).
    pub fn item_count(&self) -> usize {
        self.items.iter().flatten().count()
    }

    pub fn remove_item(&mut self, id: ItemId) {
        if let Some(slot) = self.items.get_mut(id.0 as usize) {
            if let Some(item) = slot.take() {
                self.pool.ranges.release(item.base, item.cap);
                self.free_slots.extend(item.parts.iter().map(|p| p.slot));
                self.free_slots.extend(item.impostor.map(|(_, slot)| slot));
                self.visibility.dirty = true;
            }
            self.bounds_dirty = true;
        }
    }

    /// Tints one CAD face (by the mesh's per-vertex face ids), or none.
    pub fn set_highlight(&mut self, highlight: Option<(MeshId, u32)>) {
        self.highlight = highlight;
    }

    /// Overlay segments tested against the scene depth (grid, dimensions).
    pub fn set_overlay_lines(&mut self, lines: &[LineVertex]) {
        self.overlay_cpu.0 = lines.to_vec();
        self.upload_overlay();
    }

    /// Overlay segments drawn over everything (selection outlines, tools).
    /// The plan images drawn flat on the ground (replaces the last list; an empty one clears
    /// it). Upload each image with `add_texture` first.
    pub fn set_underlays(&mut self, quads: &[UnderlayQuad]) {
        let textures = &self.textures;
        let view = |id: TextureId| Some(&textures.get(id.0 as usize)?.as_ref()?.view);
        self.underlays = underlay::build(&self.device, &self.layouts.underlay, quads, view);
    }

    pub fn set_overlay_lines_on_top(&mut self, lines: &[LineVertex]) {
        self.overlay_cpu.1 = lines.to_vec();
        self.upload_overlay();
    }

    /// The pixel (top-left origin, physical pixels of the output) where a
    /// world position appears, for placing labels. `None` behind the camera.
    pub fn project(&self, world: Vec3) -> Option<Vec2> {
        let c = self.proj * self.view * world.extend(1.0);
        if c.w <= 1e-6 {
            return None;
        }
        let ndc = c.truncate() / c.w;
        Some(Vec2::new(
            (ndc.x * 0.5 + 0.5) * self.size.0 as f32,
            (0.5 - ndc.y * 0.5) * self.size.1 as f32,
        ))
    }

    /// The tone-mapped output texture, for readback (`COPY_SRC`).
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.targets.output
    }

    /// The sky's illuminance on an open horizontal surface (lux, linear RGB) as the last
    /// `render` left it in the ambient SH, without the sun's direct light. Waits for the GPU:
    /// for tests and measurements, not for a frame loop.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sky_illuminance(&self) -> Vec3 {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SH readback"),
            size: 9 * 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.env.sh.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: Default::default(),
            },
            wgpu::Extent3d {
                width: 9,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| ());
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(view) = buffer.slice(..).get_mapped_range() else {
            return Vec3::ZERO;
        };
        let f: Vec<f32> = view
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let sh: [Vec3; 9] =
            std::array::from_fn(|i| Vec3::new(f[4 * i], f[4 * i + 1], f[4 * i + 2]));
        sky::sh_eval(&sh, Vec3::Y) * 1000.0
    }

    /// The next `render` finishes the environment cube instead of spreading
    /// it over frames: a single-frame picture after a change of sky or sun
    /// would otherwise keep the old sky in its haze and reflections.
    ///
    /// The volumetric clouds of that frame start over too, with more jitters (clouds.rs).
    pub fn finish_environment(&mut self) {
        self.refresh_shadows();
        self.env.finish();
        self.cloud_picture = true;
    }

    /// The next `render` redraws every shadow cascade for the sun of the moment, instead of
    /// spreading the redraws of a moving sun over frames (`cascade_refresh`): for a picture.
    pub fn refresh_shadows(&mut self) {
        self.cascade_stale = [true; shadow::MAX_CASCADES];
        self.sun_moving = false;
    }

    /// Records every pass of a frame; returns the output view. Submit the
    /// encoder before the next `render` (GPU timings read back the previous
    /// frame's queries then).
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder) -> &wgpu::TextureView {
        self.render_part(encoder, FramePart::Whole);
        &self.targets.display_view
    }

    /// Renders `part` of a frame: all of it, the scene into the HDR image (a photo's sample),
    /// or the post chain on an HDR image a pass fills (a photo's mean, `still.rs`).
    fn render_part(&mut self, encoder: &mut wgpu::CommandEncoder, part: FramePart) {
        #[cfg(not(target_arch = "wasm32"))]
        let cpu_start = std::time::Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let lods_ms: f64;
        #[cfg(not(target_arch = "wasm32"))]
        let gather_ms: f64;
        #[cfg(not(target_arch = "wasm32"))]
        let record_ms: f64;
        if let Some(t) = &mut self.timer {
            t.begin();
        }
        self.counters = Counters::default();
        self.frame += 1;
        // The scene changed (or instances moved where they are): the cached cascades no
        // longer cover it.
        if self.bounds_dirty || !self.moved.is_empty() {
            self.invalidate_shadows();
        }
        self.update_moved();
        self.update_bounds();
        #[cfg(not(target_arch = "wasm32"))]
        let stage = std::time::Instant::now();
        self.update_lods();
        #[cfg(not(target_arch = "wasm32"))]
        {
            lods_ms = stage.elapsed().as_secs_f64() * 1000.0;
        }
        let mut cascades = shadow::compute(
            self.view,
            self.proj,
            (self.sun.direction + self.shadow_jitter).normalize(),
            self.quality.cascades as usize,
            self.quality.shadow_resolution,
            self.shadow_distance,
            self.scene_bounds,
        );
        // Reuse the camera-centred last cascade while it still covers us.
        let last = cascades.count - 1;
        let cached = cascades.margin > 0.0;
        let sun_lit = self.sun.illuminance > 0.0;
        let reuse = match &self.far_cache {
            Some(c) => c.proj == self.proj && c.eye.distance(self.eye) < cascades.margin,
            None => false,
        };
        // The near cascade every frame; with three or more, the middle ones
        // on alternate frames (sampled with the matrix they were rendered
        // with; points they no longer cover fall through to the next one).
        let sun_moving = std::mem::take(&mut self.sun_moving);
        let mut refresh = [false; shadow::MAX_CASCADES];
        for (c, r) in refresh.iter_mut().enumerate().take(cascades.count) {
            let far = cached && c == last;
            *r = cascade_refresh(
                c,
                cascades.count,
                far,
                reuse,
                self.cascade_rendered[c].is_some(),
                self.frame,
                sun_moving,
                self.cascade_stale[c],
                sun_lit,
            );
            if far {
                // A far cascade that draws uses the fresh matrix (the current sun, centred on
                // the current eye) and becomes the cache; one that does not keeps the cached.
                let (m, replace) = far_matrix(
                    *r,
                    cascades.view_proj[c],
                    self.far_cache.as_ref().map(|f| f.matrix),
                );
                cascades.view_proj[c] = m;
                if replace {
                    self.far_cache = Some(FarCache {
                        eye: self.eye,
                        proj: self.proj,
                        matrix: m,
                    });
                }
            } else if !*r && let Some(m) = self.cascade_rendered[c] {
                cascades.view_proj[c] = m;
            }
        }
        for (c, r) in refresh.iter().enumerate() {
            if *r {
                self.cascade_rendered[c] = Some(cascades.view_proj[c]);
                self.cascade_stale[c] = false;
            }
        }
        // The snow cover map: straight down over a square round the eye, in whole texels. The
        // sea reads it too, as the height of the bed and the beach under the water.
        self.snow_cover = (self.snow > 0.0
            || self.wetness.max(self.rain) > 0.0
            || self.sea().is_some())
        .then_some(self.scene_bounds)
        .flatten()
        .map(|(lo, hi)| snow_cover_matrix(self.eye, lo, hi, self.quality.shadow_resolution));
        let view_proj = self.proj * self.view;
        // The water planes of the frame, ranked (`rank_water`): those with a planar pass, in the
        // order of the reflection's layers.
        self.rank_water(view_proj);
        let mut planar = [(0usize, 0.0f32); water::MAX_PLANES as usize];
        let mut passes = 0usize;
        for i in 0..self.water.planes.len() {
            if let Some(layer) = self.water.layer_of(i) {
                planar[layer as usize] = (i, self.water.planes[i].height);
                passes = passes.max(layer as usize + 1);
            }
        }
        let planes: [(f32, u32); water::MAX_PLANES as usize] =
            std::array::from_fn(|k| (planar[k].1, k as u32));
        self.write_sea();
        // The physical sky's LUTs (its two fixed ones the first time) go before the globals are
        // written: the air's aerial perspective is on only once they are drawn
        // (`PhysicalSky::ready`), so a first frame written before them went without it, and its
        // surfaces differed from every later frame's (shafts.wgsl `fs_composite`). The passes
        // read the globals of this frame: their writes are staged before anything encoded runs.
        if self.sky_view_dirty && matches!(self.sky_source, SkySource::Physical) {
            self.physical
                .update(encoder, &self.globals_only_bind, self.timer.as_ref());
        }
        self.sky_view_dirty = false;
        let t = &self.targets;
        self.write_globals(
            &self.globals,
            &cascades,
            (self.view, self.proj, self.eye),
            t.size,
            &planes[..passes],
            None,
        );
        for (k, (_, h)) in planar.iter().enumerate().take(passes) {
            let m = mirror_matrix(*h);
            self.write_globals(
                &self.reflect_globals[k],
                &cascades,
                (self.view * m, self.proj, m.transform_point3(self.eye)),
                texture_size(&t.reflection),
                &[],
                Some(*h),
            );
        }
        let mut slots = vec![0u8; SLOT as usize * (shadow::MAX_CASCADES + 1)];
        let cover = self.snow_cover.unwrap_or(Mat4::IDENTITY);
        for (i, m) in cascades.view_proj.iter().chain([&cover]).enumerate() {
            let mut b = Bytes::default();
            b.floats(&m.to_cols_array());
            let far = (cached && i == last) || i == shadow::MAX_CASCADES;
            b.floats(&[if far { 0.0 } else { 1.0 }, 0.0, 0.0, 0.0]);
            slots[i * SLOT as usize..i * SLOT as usize + b.0.len()].copy_from_slice(&b.0);
        }
        self.queue.write_buffer(&self.cascade_buffer, 0, &slots);
        // A ground drawn in the terrain's own base material needs no blend band at the edge.
        let blend = self.ground.filter(|_| self.ground_look.is_none());
        self.terrain.write(&self.queue, blend.map(|g| g.color));
        // Settled impostors draw whole (fade 1); a buffer regrowth in `alloc_slot` writes plain
        // mesh slots, so after one (and after a new impostor), not every frame: a write per
        // planting a frame was hundreds of calls into the browser's WebGPU.
        if std::mem::take(&mut self.impostor_slots_dirty) {
            for item in self.items.iter().flatten() {
                if let Some((_, slot)) = item.impostor {
                    self.queue.write_buffer(
                        &self.draw_buffer,
                        slot as u64 * SLOT,
                        &fade_slot(MeshId(u32::MAX - 2), FROM_INSTANCE, 1.0),
                    );
                }
            }
        }
        let globals_all: [&wgpu::Buffer; 1 + water::MAX_PLANES as usize] =
            std::array::from_fn(|i| match i {
                0 => &self.globals,
                _ => &self.reflect_globals[i - 1],
            });
        self.env.update(
            encoder,
            &self.sky_bind,
            &globals_all,
            SH_OFFSET,
            self.timer.as_ref(),
        );

        // This frame's grass blades, for the depth pre-pass and the forward pass to draw.
        let develop = matches!(part, FramePart::Develop(_));
        let grass_level =
            if develop || shadow::is_orthographic(self.proj) || !self.grass.has_field() {
                0
            } else {
                self.quality.grass
            };
        let timestamps = (grass_level > 0)
            .then(|| self.timer.as_ref().and_then(|t| t.pass("grass")))
            .flatten();
        self.grass.generate(
            &self.device,
            &self.queue,
            encoder,
            grass::Frame {
                view_proj,
                eye: self.eye,
                pixels_per_m: self.proj.y_axis.y * self.targets.size.1 as f32 * 0.5,
                level: grass_level,
                samples: self.quality.samples(),
                fast_shading: self.quality.fast_shading,
                layouts: &self.layouts,
                timestamps,
            },
        );
        // Every pass's draws, planned before any records: the instances they share draws for
        // are gathered on the GPU first (`batch.rs`).
        let layers = self.targets.shadow_layers.len();
        let ssr = self.quality.screen_reflections > 0;
        let mut gather = batch::GatherList::default();
        self.visibility.follow(&self.items);
        let mut visibility = std::mem::take(&mut self.visibility);
        let mut plan = |cull: Mat4, near_far: bool, kind: Pass, small: Option<(Vec3, f32)>| {
            self.plan_items(cull, near_far, kind, small, &mut gather, &mut visibility)
        };
        let mut plans = (!develop).then(|| {
            let shadows: Vec<Vec<Draw>> = (0..layers)
                .map(|c| {
                    // The last layer is the snow cover map, drawn after the cascades.
                    if c >= refresh.len() || c + 1 == layers || !refresh[c] {
                        return Vec::new();
                    }
                    let far = cached && c == last;
                    plan(
                        cascades.view_proj[c],
                        false,
                        Pass::Shadow {
                            far,
                            coarse: c > 0,
                            cascade: c,
                        },
                        None,
                    )
                })
                .collect();
            // The view's own levels: a bare tree's coarsest level is a few large twig cards,
            // which kept the snow off a patch of lawn under a birch. Drawn when it moved or the
            // scene changed.
            let snow = self
                .snow_cover
                .filter(|m| layers > 0 && self.snow_drawn != Some(*m));
            let snow = snow.map(|m| {
                plan(
                    m,
                    false,
                    Pass::Shadow {
                        far: false,
                        coarse: false,
                        cascade: usize::MAX,
                    },
                    None,
                )
            });
            let depth = plan(view_proj, true, Pass::Depth, None);
            let coarsest = self.quality.water_reflection >= 4;
            // Things under a pixel or two in the reflection are left out of it (in perspective).
            let reflected_px =
                self.proj.y_axis.y.abs() * texture_size(&self.targets.reflection).1 as f32 * 0.5;
            let perspective = !shadow::is_orthographic(self.proj);
            // Each water plane with a pass: the scene mirrored about it (layer = its rank).
            let reflections: Vec<Vec<Draw>> = planar
                .iter()
                .take(passes)
                .map(|(_, h)| {
                    let m = mirror_matrix(*h);
                    let small = perspective.then(|| (m.transform_point3(self.eye), reflected_px));
                    plan(view_proj * m, true, Pass::Reflection { coarsest }, small)
                })
                .collect();
            let forward = plan(view_proj, true, Pass::Forward, None);
            let mirrors = ssr.then(|| plan(view_proj, true, Pass::Mirrors, None));
            FramePlans {
                shadows,
                snow,
                depth,
                reflections,
                forward,
                mirrors,
                args: Vec::new(),
            }
        });
        #[cfg(not(target_arch = "wasm32"))]
        let stage = std::time::Instant::now();
        self.gather
            .run(&self.device, &self.queue, encoder, &self.pool, &gather);
        #[cfg(not(target_arch = "wasm32"))]
        {
            gather_ms = stage.elapsed().as_secs_f64() * 1000.0;
        }
        self.visibility = visibility;
        if let Some(p) = plans.as_mut() {
            self.write_indirect(p);
        }
        if plans.as_ref().is_some_and(|p| p.snow.is_some()) {
            self.snow_drawn = self.snow_cover;
        }

        let t = &self.targets;
        #[cfg(not(target_arch = "wasm32"))]
        let stage = std::time::Instant::now();
        if let Some(plans) = &plans {
            // Where the passes after the planar reflections find their arguments
            // (`write_indirect`): a slot per water plane, also the ones without a pass.
            let after_reflections = layers + 2 + water::MAX_PLANES as usize;
            for (c, layer) in t.shadow_layers.iter().enumerate() {
                // The last layer is the snow cover map, drawn after the cascades.
                if c >= refresh.len() || c + 1 == t.shadow_layers.len() || !refresh[c] {
                    continue;
                }
                let mut pass = depth_pass(
                    encoder,
                    "shadow",
                    layer,
                    wgpu::StoreOp::Store,
                    self.ts("shadow"),
                );
                pass.set_bind_group(0, &self.globals_bind, &[]);
                pass.set_bind_group(2, &self.cascade_bind, &[(c as u64 * SLOT) as u32]);
                self.record(&mut pass, &plans.shadows[c], plans.args[c]);
            }
            if let (Some(draws), Some(layer)) = (&plans.snow, t.shadow_layers.last()) {
                let mut pass = depth_pass(encoder, "snow cover", layer, wgpu::StoreOp::Store, None);
                pass.set_bind_group(0, &self.globals_bind, &[]);
                pass.set_bind_group(
                    2,
                    &self.cascade_bind,
                    &[(shadow::MAX_CASCADES as u64 * SLOT) as u32],
                );
                self.record(&mut pass, draws, plans.args[layers]);
            }
            self.counters.split("shadow");
            {
                let mut pass = depth_pass(
                    encoder,
                    "depth pre-pass",
                    &t.depth,
                    wgpu::StoreOp::Store,
                    self.ts("depth"),
                );
                pass.set_bind_group(0, &self.globals_bind, &[]);
                self.record(&mut pass, &plans.depth, plans.args[layers + 1]);
                self.counters.calls(self.grass.draw_depth(&mut pass));
            }
            self.counters.split("depth");
            {
                let mut pass = color_pass(
                    encoder,
                    "depth resolve",
                    &t.linear_depth,
                    None,
                    None,
                    self.ts("resolve"),
                );
                pass.set_pipeline(&self.sampled.resolve);
                pass.set_bind_group(0, &self.globals_bind, &[]);
                pass.set_bind_group(1, &t.resolve_bind, &[]);
                pass.draw(0..3, 0..1);
                self.counters.call();
            }
            if self.quality.ao_samples > 0 || self.quality.contact_shadows {
                let mut pass = color_pass(
                    encoder,
                    "ssao",
                    &t.occlusion[0],
                    None,
                    None,
                    self.ts("ssao"),
                );
                pass.set_pipeline(&self.pipelines.ssao);
                pass.set_bind_group(0, &self.globals_bind, &[]);
                pass.set_bind_group(1, &t.ssao_bind, &[]);
                pass.draw(0..3, 0..1);
                self.counters.call();
                drop(pass);
                for (i, pipeline) in [&self.pipelines.blur_h, &self.pipelines.blur_v]
                    .into_iter()
                    .enumerate()
                {
                    let mut pass = color_pass(
                        encoder,
                        "blur",
                        &t.occlusion[1 - i],
                        None,
                        None,
                        self.ts("ssao"),
                    );
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &self.globals_bind, &[]);
                    pass.set_bind_group(1, &t.blur_bind[i], &[]);
                    pass.draw(0..3, 0..1);
                    self.counters.call();
                }
            } else {
                color_pass(
                    encoder,
                    "no ssao",
                    &t.occlusion[0],
                    Some(wgpu::Color::WHITE),
                    None,
                    None,
                );
            }
            // The volumetric clouds, for the sky pass to read.
            if self.volumetric_clouds() && !shadow::is_orthographic(self.proj) {
                let (steps, light_steps) = self.quality.cloud_steps();
                let ts = self.timer.as_ref().and_then(|t| t.pass("clouds"));
                self.cloud_volume.draw(
                    &self.device,
                    &self.queue,
                    encoder,
                    clouds::Draw {
                        layouts: &self.layouts,
                        globals: &self.globals_bind,
                        size: t.size,
                        div: self.quality.clouds,
                        steps,
                        light_steps,
                        view: self.view,
                        proj: self.proj,
                        jumped: self.cloud_jump,
                        picture: self.cloud_picture,
                        sun: self.sun.direction,
                        flash: self.flash().is_some(),
                    },
                    ts,
                );
                // One render pass, whatever it dispatches.
                self.counters.call();
                self.cloud_jump = false;
                self.cloud_picture = false;
            } else {
                self.cloud_volume.off();
            }
            self.counters.split("clouds");
            // One pass per water plane with a planar reflection, into its layer of the array:
            // scissored to where the plane shows on the render target (padded for the ripples
            // in `rank_water`), so its fill is only where that water is.
            let reflection_size = texture_size(&t.reflection);
            for (k, draws) in plans.reflections.iter().enumerate() {
                let (_, h) = planar[k];
                let rect = self.water.planes[planar[k].0].rect;
                let from = u64::from(t.size.0.max(1)).max(1);
                let from_y = u64::from(t.size.1.max(1));
                let scaled = |a: u32, b: u32, from: u64, to: u32| {
                    // Outward, so the scissor never cuts into the plane's rectangle.
                    let to = u64::from(to);
                    let lo = (u64::from(a) * to / from).min(to - 1);
                    let hi = (u64::from(a + b) * to).div_ceil(from).clamp(lo + 1, to);
                    (lo as u32, hi as u32)
                };
                let (x0, x1) = scaled(rect.x, rect.w, from, reflection_size.0);
                let (y0, y1) = scaled(rect.y, rect.h, from_y, reflection_size.1);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("reflection"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &t.reflection_layers[k],
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &t.reflection_depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: self.ts("reflection"),
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
                pass.set_bind_group(0, &self.reflect_globals_bind[k], &[]);
                pass.set_bind_group(3, &t.reflect_forward_bind, &[]);
                self.record(&mut pass, draws, plans.args[layers + 2 + k]);
                // The endless ground has no hole where the terrain is dug out:
                // seen from water below its height (a pond in the lawn) it would
                // cover the sky the water mirrors.
                if self.ground.is_some_and(|g| h >= g.height) {
                    self.draw_ground(&mut pass, &[&self.pipelines.reflect_ground]);
                }
                pass.set_pipeline(&self.pipelines.reflect_sky);
                pass.set_bind_group(2, self.cloud_volume.sky_bind(), &[]);
                pass.draw(0..3, 0..1);
                self.counters.call();
            }
            self.counters.split("reflection");
            {
                let (view, resolve) = match &t.hdr_msaa {
                    Some(msaa) => (msaa, Some(&t.hdr)),
                    None => (&t.hdr, None),
                };
                // With screen-space reflections the glass and the mirrors are drawn in a second pass,
                // after the opaque scene is copied for them to reflect (into `prev_hdr`): the first then
                // keeps its multisampled colour and depth for the second.
                let targets = (view, resolve, &t.depth);
                let mut pass =
                    forward_pass(encoder, "forward", targets, (true, ssr), self.ts("forward"));
                pass.set_bind_group(0, &self.globals_bind, &[]);
                pass.set_bind_group(3, &t.forward_bind, &[]);
                self.record(&mut pass, &plans.forward, plans.args[after_reflections]);
                let grass = self.grass.draw(
                    &mut pass,
                    &self.draw_bind,
                    (self.ground_slot as u64 * SLOT) as u32,
                );
                self.counters.calls(grass);
                if self.ground.is_some() {
                    // The cells near the sky volume only when there is one, and after the others:
                    // all ground lies at one depth, so of two cells over the same pixel the last
                    // drawn shows, and before the split that was the cell further out (the rings
                    // are drawn inside out), which the volume's are from where hills hide them.
                    let [near, away] = &self.sampled.ground;
                    let parts: &[&wgpu::RenderPipeline] = if self.sky_volume_box[7] > 0.5 {
                        &[away, near]
                    } else {
                        &[away]
                    };
                    self.draw_ground(&mut pass, parts);
                }
                pass.set_pipeline(&self.sampled.sky);
                pass.set_bind_group(2, self.cloud_volume.sky_bind(), &[]);
                pass.draw(0..3, 0..1);
                self.counters.call();
                let sea = self.draw_sea(&mut pass);
                self.counters.calls(sea);
                if ssr {
                    drop(pass);
                    encoder.copy_texture_to_texture(
                        t.hdr_texture.as_image_copy(),
                        t.prev_hdr.as_image_copy(),
                        t.prev_hdr.size(),
                    );
                    let label = "glass and mirrors";
                    pass = forward_pass(encoder, label, targets, (false, ssr), self.ts(label));
                    pass.set_bind_group(0, &self.globals_bind, &[]);
                    pass.set_bind_group(3, &t.forward_bind, &[]);
                    if let Some(draws) = &plans.mirrors {
                        self.record(&mut pass, draws, plans.args[after_reflections + 1]);
                    }
                }
                self.draw_transparent(&mut pass, view_proj);
                if self.rain > 0.0 && !shadow::is_orthographic(self.proj) {
                    pass.set_pipeline(&self.sampled.rain);
                    pass.set_bind_group(0, &self.globals_bind, &[]);
                    pass.set_bind_group(3, &t.forward_bind, &[]);
                    pass.draw(0..3, 0..1);
                    self.counters.call();
                }
            }
        }
        self.counters.split("forward");
        #[cfg(not(target_arch = "wasm32"))]
        {
            record_ms = stage.elapsed().as_secs_f64() * 1000.0;
            if let Some(log) = &mut self.stage_log {
                log.frame(
                    self.frame,
                    lods_ms,
                    self.counters.plan_ms.get(),
                    self.counters.visible_ms.get(),
                    gather_ms,
                    record_ms,
                );
            }
        }
        let post = |encoder: &mut wgpu::CommandEncoder,
                    label: &'static str,
                    target: &wgpu::TextureView,
                    load: Option<()>,
                    pipeline: &wgpu::RenderPipeline,
                    bind: &wgpu::BindGroup| {
            let mut pass = color_pass(encoder, label, target, None, load, self.ts(label));
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, bind, &[]);
            pass.draw(0..3, 0..1);
            self.counters.add(1, 1, false);
            self.counters.call();
        };
        // The air: light shafts, the sun's glare and the aerial perspective, onto the HDR image.
        // A photo's mean has them in already.
        let air = self.air(self.proj);
        if let FramePart::Develop(fill) = part {
            still::develop(self, encoder, fill);
        } else if air.any() {
            if air.aerial {
                self.physical
                    .update_aerial(encoder, &self.globals_only_bind, self.timer.as_ref());
            }
            let p = &self.pipelines;
            if air.shafts > 0.0 {
                post(
                    encoder,
                    "shafts",
                    &t.shafts[0],
                    None,
                    &p.shafts_march,
                    &t.air_bind[0],
                );
                let blur = &p.shafts_blur;
                post(
                    encoder,
                    "shafts blur",
                    &t.shafts[1],
                    None,
                    &blur[0],
                    &t.air_bind[1],
                );
                post(
                    encoder,
                    "shafts blur",
                    &t.shafts[0],
                    None,
                    &blur[1],
                    &t.air_bind[2],
                );
            }
            if air.glare > 0.0 {
                post(
                    encoder,
                    "sun glare",
                    &t.sun_seen,
                    None,
                    &p.sun_seen,
                    &t.air_bind[0],
                );
            }
            post(encoder, "air", &t.hdr, Some(()), &p.air, &t.air_bind[3]);
        }
        if matches!(part, FramePart::Scene) {
            self.counters.split("post");
            if let Some(t) = &mut self.timer {
                t.end(encoder);
            }
            #[cfg(not(target_arch = "wasm32"))]
            let cpu_encode_ms = cpu_start.elapsed().as_secs_f32() * 1000.0;
            #[cfg(target_arch = "wasm32")]
            let cpu_encode_ms = 0.0;
            self.update_stats(view_proj, cpu_encode_ms);
            return;
        }
        if self.quality.bloom && self.bloom_strength > 0.0 {
            for (i, target) in t.bloom.iter().enumerate() {
                let p = &self.pipelines.bloom[(i > 0) as usize];
                post(encoder, "bloom", target, None, p, &t.bloom_down_bind[i]);
            }
            for i in (0..t.bloom.len() - 1).rev() {
                let p = &self.pipelines.bloom[2];
                post(
                    encoder,
                    "bloom",
                    &t.bloom[i],
                    Some(()),
                    p,
                    &t.bloom_up_bind[i],
                );
            }
        }
        let upscale = t.size != self.size;
        let fxaa = self.quality.fxaa;
        let tonemap_target = if fxaa || upscale {
            &t.ldr[0]
        } else {
            &t.output_unorm
        };
        let tonemap = &self.pipelines.tonemap[(fxaa || upscale) as usize];
        post(
            encoder,
            "tonemap",
            tonemap_target,
            None,
            tonemap,
            &t.tonemap_bind,
        );
        if fxaa {
            let target = if upscale { &t.ldr[1] } else { &t.output_unorm };
            post(
                encoder,
                "fxaa",
                target,
                None,
                &self.pipelines.fxaa,
                &t.fxaa_bind,
            );
        }
        if upscale {
            let bind = &t.upscale_bind[fxaa as usize];
            post(
                encoder,
                "upscale",
                &t.output_unorm,
                None,
                &self.pipelines.upscale,
                bind,
            );
        }
        if let Some(buffer) = &self.underlays.buffer {
            let mut pass = color_pass(encoder, "underlay", &t.output_view, None, Some(()), None);
            pass.set_pipeline(&self.pipelines.underlay);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, &t.overlay_bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            for (i, bind) in self.underlays.binds.iter().enumerate() {
                pass.set_bind_group(2, bind, &[]);
                pass.draw(i as u32 * 6..i as u32 * 6 + 6, 0..1);
                self.counters.call();
            }
        }
        if let (Some(buffer), true) = (&self.overlay, self.overlay_count > 0) {
            let mut pass = color_pass(
                encoder,
                "overlay",
                &t.output_view,
                None,
                Some(()),
                self.ts("overlay"),
            );
            pass.set_pipeline(&self.pipelines.overlay);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, &t.overlay_bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..self.overlay_count);
            self.counters.call();
        }
        self.counters.split("post");
        if let Some(t) = &mut self.timer {
            t.end(encoder);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let cpu_encode_ms = cpu_start.elapsed().as_secs_f32() * 1000.0;
        #[cfg(target_arch = "wasm32")]
        let cpu_encode_ms = 0.0;
        self.update_stats(view_proj, cpu_encode_ms);
    }

    fn update_stats(&mut self, view_proj: Mat4, cpu_encode_ms: f32) {
        let visible_lights = self
            .lights_cpu
            .iter()
            .filter(|l| {
                !outside(
                    view_proj,
                    (l.position - l.range_m, l.position + l.range_m),
                    true,
                )
            })
            .count() as u32;
        let hdri = match &self.sky_source {
            SkySource::Hdri { bytes, .. } => *bytes,
            _ => 0,
        };
        let texture_bytes = self.textures.iter().flatten().map(|t| t.bytes).sum::<u64>()
            + self.targets.bytes
            + self.env.bytes
            + self.terrain.bytes
            + self
                .impostors
                .iter()
                .flatten()
                .map(|i| i.bytes)
                .sum::<u64>()
            + hdri;
        let shadow =
            // The cascades and the snow cover map.
            u64::from(self.quality.shadow_resolution).pow(2) * 4 * u64::from(self.quality.cascades + 1);
        let texture_breakdown = vec![
            (
                "textures",
                self.textures.iter().flatten().map(|t| t.bytes).sum::<u64>(),
            ),
            ("shadow maps", shadow),
            ("render targets", self.targets.bytes.saturating_sub(shadow)),
            ("environment", self.env.bytes),
            ("terrain", self.terrain.bytes),
            (
                "impostors",
                self.impostors
                    .iter()
                    .flatten()
                    .map(|i| i.bytes)
                    .sum::<u64>(),
            ),
            ("sky image", hdri),
        ];
        let gpu_passes = self.timer.as_ref().map_or(Vec::new(), |t| t.last.clone());
        // Native tools: VISIONCRAFT_GPU_TIMINGS=1 prints the pass times once
        // a second (at 60 frames), for profiling a host app's own scenes.
        #[cfg(not(target_arch = "wasm32"))]
        if self.frame.is_multiple_of(60)
            && !gpu_passes.is_empty()
            && std::env::var_os("VISIONCRAFT_GPU_TIMINGS").is_some()
        {
            let parts: Vec<String> = gpu_passes
                .iter()
                .map(|(l, ms)| format!("{l} {ms:.2}"))
                .collect();
            let total: f32 = gpu_passes.iter().map(|p| p.1).sum();
            eprintln!("gpu ms: {} (sum {total:.2})", parts.join(", "));
        }
        // The water surfaces of the frame and how each was reflected: by plane (height), and in
        // the order they were gathered within one. The list is the last frame's, filled again.
        let mut water = std::mem::take(&mut self.last_stats.water);
        water.clear();
        for (p, _) in self.water.planes.iter().enumerate() {
            for (s, seen) in self.water_seen.iter().enumerate() {
                if self.water.plane_of(s) != p {
                    continue;
                }
                water.push(WaterSurface {
                    item: seen.item.map(|(item, part, material)| WaterItem {
                        item,
                        part,
                        material: material.0,
                    }),
                    level_m: seen.surface.height,
                    tier: self.water.tier_of(s),
                });
            }
        }
        self.last_stats = RenderStats {
            draw_calls: self.counters.draws.get(),
            pass_draws: self.counters.passes.take(),
            pass_calls: self.counters.pass_calls.take(),
            pass_triangles: self.counters.pass_triangles.take(),
            triangles: self.counters.triangles.get(),
            instances: self.counters.instances.get(),
            visible_lights,
            texture_bytes,
            texture_breakdown,
            cpu_encode_ms,
            gpu_ms: (!gpu_passes.is_empty()).then(|| gpu_passes.iter().map(|p| p.1).sum()),
            gpu_passes,
            render_size: self.targets.size,
            water,
        };
    }

    /// The infinite ground (scene.wgsl `vs_ground`) with its own material, with each of
    /// `pipelines` (the parts of `ground_pipeline`).
    fn draw_ground(&self, pass: &mut wgpu::RenderPass, pipelines: &[&wgpu::RenderPipeline]) {
        const RINGS: u32 = 80;
        const SEGMENTS: u32 = 192;
        // A material of the scene (the terrain's base layer) when one is set: the ground then
        // looks like it, texture, tiling phase and variation alike, so their border vanishes:
        // its texture set here, and its row in the ground's draw slot (`ground_row`).
        let set = self.ground_look_material().map_or(0, |m| m.set);
        let Some(Some(texture_set)) = self.texture_sets.get(set as usize) else {
            return;
        };
        pass.set_bind_group(1, &texture_set.bind, &[]);
        pass.set_bind_group(
            2,
            &self.draw_bind,
            &[(self.ground_slot as u64 * SLOT) as u32],
        );
        for pipeline in pipelines {
            pass.set_pipeline(pipeline);
            pass.draw(0..RINGS * SEGMENTS * 6, 0..1);
            self.counters.add((RINGS * SEGMENTS * 2) as u64, 1, false);
            self.counters.call();
        }
    }

    /// Gathers the visible water surfaces of the frame and ranks their planes (`water.rs`):
    /// the sea when the eye is above it, and the parts with a water material whose clusters are
    /// in view, each where its visible clusters' bounds show on the render target. Nothing is
    /// allocated once the buffers have grown.
    fn rank_water(&mut self, cull: Mat4) {
        // The planar reflection off: nothing to rank, as before this ranked at all.
        let off = self.quality.water_reflection == 0;
        self.water_seen.clear();
        self.water_surfaces.clear();
        if off {
            self.water.rank(&[], 0, false);
            return;
        }
        let size = self.targets.size;
        // The ripples' most tilt: the wind's, more under rain's rings.
        let tilt = if self.rain > 0.0 {
            water::RAIN_TILT
        } else {
            water::WIND_TILT
        };
        // The sea when the eye is above it and some of it is in view.
        let sea = self.sea_surface(cull, size, tilt);
        let Self {
            water,
            water_seen,
            water_surfaces,
            items,
            materials,
            eye,
            quality,
            ..
        } = self;
        // Only surfaces that can be ranked (`water::Surface::usable`): a broken mesh or sea
        // level must not take a planar pass.
        if let Some(surface) = sea.filter(water::Surface::usable) {
            water_seen.push(Seen {
                item: None,
                surface,
            });
        }
        for (i, item) in items.iter().enumerate() {
            let Some(item) = item else {
                continue;
            };
            if item.bounds.is_none_or(|b| outside(cull, b, true)) {
                continue;
            }
            for (p, part) in item.parts.iter().enumerate() {
                let water_material = materials
                    .get(part.material.0 as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|m| m.cpu.surface == Surface::Water);
                if !water_material {
                    continue;
                }
                let visible = item
                    .clusters
                    .iter()
                    .filter(|c| Self::cluster_lod(item, c, Pass::Forward) == Some(part.lod))
                    .filter_map(|c| c.bounds)
                    .filter(|b| !outside(cull, *b, true))
                    .reduce(union);
                let Some((lo, hi)) = visible else {
                    continue;
                };
                let distance = eye.clamp(lo, hi).distance(*eye);
                let rect = water::screen_rect(cull, (lo, hi), size);
                let coverage = rect.area() as f32 / (u64::from(size.0) * u64::from(size.1)) as f32;
                // The shift over the view-space depth, as the shader divides it.
                let depth = water::view_depth(cull, (lo, hi));
                let rect = rect.padded(
                    water::ripple_pad(depth, water::POND_DISTORTION, tilt, size),
                    size,
                );
                let surface = water::Surface {
                    height: hi.y,
                    rect,
                    coverage,
                    distance,
                    sea: false,
                };
                if surface.usable() {
                    water_seen.push(Seen {
                        item: Some((i as u32, p as u32, part.material)),
                        surface,
                    });
                }
            }
        }
        water_surfaces.extend(water_seen.iter().map(|s| s.surface));
        water.rank(
            water_surfaces,
            quality.plane_budget(),
            quality.screen_reflections > 0,
        );
    }

    /// The level of detail a cluster draws at in a pass (`None`: culled).
    fn cluster_lod(item: &Item, cluster: &Cluster, pass: Pass) -> Option<u8> {
        let levels = item.lods.len() as u8;
        if cluster.lod >= levels {
            return None;
        }
        // A hedge's coarser levels are a few cards: its shadow would break into a row of dots.
        if item.shadow_exact && matches!(pass, Pass::Shadow { .. }) {
            return Some(cluster.lod.min(levels - 1));
        }
        if item.shadow_caster.is_some() && matches!(pass, Pass::Shadow { .. }) && cluster.lod >= 1 {
            return Some(SHADOW_CASTER_LEVEL);
        }
        // Plants with an impostor shadow with it beyond their finest level.
        if item.impostor.is_some() && matches!(pass, Pass::Shadow { .. }) && cluster.lod >= 1 {
            return Some(levels - 1);
        }
        Some(match pass {
            Pass::Depth | Pass::Forward | Pass::Mirrors => cluster.lod,
            Pass::Reflection { coarsest: true } => levels - 1,
            Pass::Reflection { coarsest: false }
            | Pass::Shadow {
                far: false,
                coarse: false,
                ..
            } => (cluster.lod + 1).min(levels - 1),
            // A tree at its finest level stands near the eye, and so does its shadow: its
            // impostor's two cards cast the side view's limbs metres long over the lawn there
            // (dark arcs like tyre tracks), so the far cascades take its last mesh level.
            Pass::Shadow { .. } if item.impostor.is_some() && levels >= 2 => levels - 2,
            Pass::Shadow { .. } => levels - 1,
        })
    }

    /// Per item, which of its clusters `cull` keeps, written into `seen` (`Visibility`): each
    /// cluster's own test, in one pass and without an allocation (the flags are the renderer's).
    /// Testing them per part, category and fade took a third of a coast frame's CPU time in the
    /// browser, and a list a pass 15 % of a town's. The 128 m blocks are not tested here: on the
    /// town that cost a sixth more than it saved (the cluster test is 4 ns, a lookup through the
    /// block more, and a wide view keeps most blocks in view); they are `update_lods`'.
    ///
    /// `small`: an eye and pixels per unit of size over distance; clusters whose items'
    /// instances would be under `MIN_REFLECTED_PX` from there are left out (the reflection).
    fn visible_clusters(
        &self,
        cull: Mat4,
        near_far: bool,
        small: Option<(Vec3, f32)>,
        visibility: &mut Visibility,
    ) {
        let Visibility { seen, offsets, .. } = visibility;
        let (seen, offsets): (&mut [bool], &[u32]) = (seen.as_mut_slice(), offsets.as_slice());
        for (i, item) in self.items.iter().enumerate() {
            let seen = &mut seen[offsets[i] as usize..offsets[i + 1] as usize];
            let Some(item) = item else {
                seen.fill(false);
                continue;
            };
            if item.bounds.is_none_or(|b| outside(cull, b, near_far)) {
                seen.fill(false);
                continue;
            }
            for (c, keep) in item.clusters.iter().zip(seen.iter_mut()) {
                *keep = (c.lod as usize) < item.lods.len()
                    && c.bounds.is_some_and(|b| {
                        !outside(cull, b, near_far)
                            && small.is_none_or(|(eye, px)| {
                                let d = eye.clamp(b.0, b.1).distance(eye);
                                item.size * px >= MIN_REFLECTED_PX * d
                            })
                    });
            }
        }
    }

    fn plan_items(
        &self,
        cull: Mat4,
        near_far: bool,
        kind: Pass,
        small: Option<(Vec3, f32)>,
        gather: &mut batch::GatherList,
        visibility: &mut Visibility,
    ) -> Vec<Draw> {
        let shadow = matches!(kind, Pass::Shadow { .. });
        let shaded = matches!(
            kind,
            Pass::Forward | Pass::Mirrors | Pass::Reflection { .. }
        );
        let fades = matches!(kind, Pass::Depth | Pass::Forward);
        let order = shadow as usize;
        let late = self.quality.screen_reflections > 0;
        let view = matches!(kind, Pass::Forward | Pass::Mirrors);
        // 0 opaque, 1 cutout, 2 glass: into shadow maps, and into the water's reflection as
        // an opaque pane (without it the reflection looked through every window into the dark
        // room behind it: the dark blobs in a pond).
        let glass = shadow || matches!(kind, Pass::Reflection { .. });
        let wanted = |category: usize| match kind {
            Pass::Mirrors => category == 3,
            _ => category < 2 + glass as usize,
        };
        #[cfg(not(target_arch = "wasm32"))]
        let plan_start = std::time::Instant::now();
        self.visible_clusters(cull, near_far, small, visibility);
        let Visibility { seen, offsets, .. } = visibility;
        let (seen, offsets): (&mut [bool], &[u32]) = (seen.as_mut_slice(), offsets.as_slice());
        #[cfg(not(target_arch = "wasm32"))]
        self.counters
            .visible_ms
            .set(self.counters.visible_ms.get() + stage::cpu_ms(plan_start));
        let mut draws = Vec::new();
        let mut batches = Batches::default();
        let mut levels: Vec<Option<u8>> = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            let Some(item) = item else { continue };
            // Out of this cascade altogether (`set_item_shadow_cascades`): its clusters are
            // not planned.
            if matches!(kind, Pass::Shadow { cascade, .. } if cascade < item.shadow_from as usize) {
                continue;
            }
            let seen = &seen[offsets[i] as usize..offsets[i + 1] as usize];
            if !seen.iter().any(|&s| s) {
                continue;
            }
            // This pass's level per cluster (`None`: not drawn), and whether any dissolves.
            levels.clear();
            levels.extend(
                item.clusters
                    .iter()
                    .zip(seen)
                    .map(|(c, &s)| s.then(|| Self::cluster_lod(item, c, kind)).flatten()),
            );
            let dissolving = fades
                && item
                    .clusters
                    .iter()
                    .zip(&levels)
                    .any(|(c, l)| l.is_some() && c.fading);
            let mut runs = RunCache::default();
            for (part_index, part) in item.parts.iter().enumerate() {
                let Some(Some(mesh)) = self.meshes.get(part.mesh.0 as usize) else {
                    continue;
                };
                let Some(material) = self
                    .materials
                    .get(part.material.0 as usize)
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                let m = &material.cpu;
                let category = if m.surface == Surface::Glass {
                    2
                } else if late && m.mirror() && matches!(kind, Pass::Forward | Pass::Mirrors) {
                    3
                } else if m.transparent() {
                    continue;
                } else {
                    m.cutout() as usize
                };
                if !wanted(category) {
                    continue;
                }
                let weights = mesh.weights && shaded;
                // Opaque and cutout mesh levels dissolve in fading clusters (into each other,
                // the impostor or nothing), through their own pipelines, one draw and fade slot
                // per cluster; the settled clusters leave them out.
                // Terrain chunks never dissolve: their forward pipeline has no dissolve, so they draw
                // their nearer level in every pass. Asked of the mesh, not of `weights` (none outside the
                // shaded passes): a depth pre-pass that dithered both levels left the forward pass's level
                // behind the coarser one, unshaded (black patches on the ground after a jump).
                let can_fade = fades && category <= 1 && !mesh.weights;
                let ds = m.double_sided as usize;
                let pipe = match kind {
                    Pass::Shadow { .. } => Pipe::Shadow(category),
                    Pass::Depth => Pipe::Depth(m.variant()),
                    Pass::Forward if weights => Pipe::Terrain,
                    Pass::Reflection { .. } if weights => Pipe::ReflectTerrain,
                    Pass::Forward | Pass::Mirrors => Pipe::Forward(m.variant()),
                    Pass::Reflection { .. } => Pipe::Reflect(m.variant()),
                };
                let g1 = if weights {
                    Some(Group1::Terrain)
                } else if shaded || category == 1 {
                    Some(Group1::Set(material.set))
                } else {
                    None
                };
                let draw = |pipe, g1, g2, mesh: DrawMesh, instances| Draw {
                    pipe,
                    g1,
                    g2,
                    vertices: mesh.range.count,
                    mesh: Some(mesh),
                    src: Src::Pool,
                    instances,
                    triangles: mesh.range.count as u64 / 3,
                    view,
                };
                let g2 = shaded.then_some((false, part.slot));
                let settled = runs.get(item, &levels, order, part.lod, can_fade && dissolving);
                let draw =
                    |pipe, g1, g2, instances| draw(pipe, g1, g2, mesh.draw(part.mesh), instances);
                if weights {
                    // A terrain chunk is its own mesh: nothing to share a draw with.
                    draws.extend(
                        settled
                            .iter()
                            .map(|(_, r)| draw(pipe, g1, g2, item.pool_range(r))),
                    );
                } else {
                    // A batch spans materials (one texture set): each instance names its row.
                    let batched = shaded.then_some((false, self.batch_slot));
                    batches.add(
                        category as u8,
                        draw(pipe, g1, batched, 0..0),
                        item,
                        row(part.material),
                        settled,
                    );
                }
                if can_fade && dissolving {
                    let pipe = match kind {
                        Pass::Depth => Pipe::DepthFade(ds),
                        _ => Pipe::Forward(2 + ds),
                    };
                    let g1 = Some(Group1::Set(material.set));
                    for (c, _) in item.clusters.iter().zip(&levels).filter(|(c, l)| {
                        l.is_some() && c.fading && (c.lod == part.lod || c.far == part.lod)
                    }) {
                        let g2 = Some((true, c.fade_slot + part_index as u32));
                        draws.push(draw(pipe, g1, g2, item.pool_range(&c.range)));
                    }
                }
            }
            if !matches!(kind, Pass::Mirrors) {
                self.plan_impostor(
                    item,
                    &levels,
                    dissolving,
                    &mut runs,
                    kind,
                    &mut draws,
                    &mut batches,
                );
            }
            // Trees' shadow casters (`set_item_shadow_caster`): their clusters past the finest
            // level, depth only, no material, the density dither of `fs_shadow_dither`.
            if shadow
                && let Some(id) = item.shadow_caster
                && let Some(Some(mesh)) = self.meshes.get(id.0 as usize)
            {
                let dm = mesh.draw(id);
                let draw = Draw {
                    pipe: Pipe::Shadow(3),
                    g1: None,
                    g2: None,
                    vertices: dm.range.count,
                    mesh: Some(dm),
                    src: Src::Pool,
                    instances: 0..0,
                    triangles: dm.range.count as u64 / 3,
                    view: false,
                };
                let settled = runs.get(item, &levels, 1, SHADOW_CASTER_LEVEL, false);
                batches.add(4, draw, item, FROM_INSTANCE, settled);
            }
        }
        batches.emit(&mut draws, gather);
        #[cfg(not(target_arch = "wasm32"))]
        self.counters
            .plan_ms
            .set(self.counters.plan_ms.get() + stage::cpu_ms(plan_start));
        draws
    }

    /// An item's impostor quads: clusters at the impostor level, and in the depth and forward
    /// passes also the dissolving ones whose far side it is.
    #[allow(clippy::too_many_arguments)]
    fn plan_impostor(
        &self,
        item: &Item,
        levels: &[Option<u8>],
        dissolving: bool,
        runs: &mut RunCache,
        kind: Pass,
        draws: &mut Vec<Draw>,
        batches: &mut Batches,
    ) {
        let Some((id, slot)) = item.impostor else {
            return;
        };
        if !matches!(self.impostors.get(id.0 as usize), Some(Some(_))) {
            return;
        }
        let level = item.lods.len() as u8 - 1;
        let shadow = matches!(kind, Pass::Shadow { .. });
        let pipe = match kind {
            Pass::Depth => Pipe::Impostor(0),
            Pass::Forward | Pass::Mirrors => Pipe::Impostor(1),
            Pass::Reflection { .. } => Pipe::ReflectImpostor,
            Pass::Shadow { .. } => Pipe::ImpostorShadow,
        };
        let draw = |g2, instances| Draw {
            pipe,
            g1: Some(Group1::Impostor(id.0)),
            g2,
            mesh: None,
            // Shadows: two cards per plant (impostor.wgsl, `impostor_shadow_towards`).
            vertices: if shadow { 12 } else { 6 },
            src: Src::Pool,
            instances,
            triangles: 2,
            view: kind == Pass::Forward,
        };
        // Settled impostors share one draw slot's contents (fade 1): batched by atlas.
        let settled = runs.get(item, levels, shadow as usize, level, dissolving);
        batches.add(
            1,
            draw((!shadow).then_some((false, slot)), 0..0),
            item,
            FROM_INSTANCE,
            settled,
        );
        // Dissolving clusters draw their impostor with their own fade slot when it is the far
        // side of their hand-over.
        if dissolving {
            for (c, _) in item
                .clusters
                .iter()
                .zip(levels)
                .filter(|(c, l)| l.is_some() && c.fading && c.far == level)
            {
                let slot = c.fade_slot + item.parts.len() as u32;
                draws.push(draw(Some((true, slot)), item.pool_range(&c.range)));
            }
        }
    }

    /// Runs of consecutive clusters (adjacent in the pool) at `level` in this pass (`levels`,
    /// per cluster), without the dissolving ones when `skip_fading`, each with the sort key of
    /// its nearest cluster (front to back seen from the camera, or from the sun with `order` 1).
    fn runs(
        item: &Item,
        order: usize,
        levels: &[Option<u8>],
        level: u8,
        skip_fading: bool,
    ) -> Vec<(f32, std::ops::Range<u32>)> {
        let mut out: Vec<(f32, std::ops::Range<u32>)> = Vec::new();
        let mut open = false;
        for (c, l) in item.clusters.iter().zip(levels) {
            if *l != Some(level) || (skip_fading && c.fading) {
                open = false;
                continue;
            }
            match out.last_mut() {
                Some((k, r)) if open => {
                    r.end = c.range.end;
                    *k = k.min(c.key[order]);
                }
                _ => out.push((c.key[order], c.range.clone())),
            }
            open = true;
        }
        out
    }

    fn pipe(&self, pipe: Pipe) -> &wgpu::RenderPipeline {
        match pipe {
            Pipe::Shadow(c) => &self.pipelines.shadow[c],
            Pipe::Depth(v) => &self.sampled.depth[v],
            Pipe::DepthFade(ds) => &self.sampled.depth_fade[ds],
            Pipe::Forward(v) => &self.sampled.forward[v],
            Pipe::Terrain => &self.sampled.terrain,
            Pipe::ReflectTerrain => &self.pipelines.reflect_terrain,
            Pipe::Reflect(v) => &self.pipelines.reflect[v],
            Pipe::Impostor(i) => &self.sampled.impostor[i],
            Pipe::ReflectImpostor => &self.pipelines.reflect_impostor,
            Pipe::ImpostorShadow => &self.pipelines.impostor_shadow,
        }
    }

    /// Packs the frame's indirect draws: per pass, the arguments of every batched mesh draw
    /// that `record` sends through the indirect buffer (runs of two or more with the same
    /// bindings), in their record order, into one buffer; where each pass's start is, is
    /// `plans.args`.
    fn write_indirect(&mut self, plans: &mut FramePlans) {
        self.indirect_args.clear();
        let FramePlans {
            shadows,
            snow,
            depth,
            reflections,
            forward,
            mirrors,
            args,
        } = plans;
        args.clear();
        args.reserve(shadows.len() + 4 + water::MAX_PLANES as usize);
        let mut base = |draws: &[Draw]| {
            let base = self.indirect_args.len() as u32 / INDIRECT_ARGS;
            pack_args(&mut self.indirect_args, draws, self.multi_draw);
            base
        };
        for shadow in shadows {
            args.push(base(shadow));
        }
        args.push(base(snow.as_deref().unwrap_or(&[])));
        args.push(base(depth));
        // A slot per water plane, also the ones without a pass this frame: a pass finds its
        // arguments by index.
        for k in 0..water::MAX_PLANES as usize {
            args.push(base(reflections.get(k).map_or(&[], Vec::as_slice)));
        }
        args.push(base(forward));
        args.push(base(mirrors.as_deref().unwrap_or(&[])));
        let n = self.indirect_args.len() as u32 / INDIRECT_ARGS;
        if n == 0 {
            return;
        }
        if n > self.indirect_capacity {
            self.indirect_capacity = n.next_power_of_two();
            self.indirect = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("indirect draws"),
                size: u64::from(self.indirect_capacity) * u64::from(INDIRECT_ARGS),
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue
            .write_buffer(&self.indirect, 0, &self.indirect_args);
    }

    /// Records planned draws (`plan_items`), setting only what changes from one to the next:
    /// in the browser every set is a call into its WebGPU. A run of two or more batched mesh
    /// draws with the same bindings is one indirect call (`Calls`; their arguments are at `args`
    /// in the buffer `write_indirect` packed); without the device's `INDIRECT_FIRST_INSTANCE`
    /// each is a direct draw of the same arguments. A draw whose bind group, mesh or arena went
    /// away since the plan is left out.
    fn record(&self, pass: &mut wgpu::RenderPass, draws: &[Draw], args: u32) {
        let mut pipe = None;
        let mut g1 = None;
        let mut g2 = None;
        let mut mesh = None;
        let mut arena = None;
        let mut src = None;
        // Binds what `d` needs and is not bound; false when something it needs is gone.
        let mut bind = |pass: &mut wgpu::RenderPass, d: &Draw| -> bool {
            if pipe != Some(d.pipe) {
                pass.set_pipeline(self.pipe(d.pipe));
                pipe = Some(d.pipe);
            }
            if let Some(g) = d.g1
                && g1 != Some(g)
            {
                let group = match g {
                    Group1::Set(set) => self
                        .texture_sets
                        .get(set as usize)
                        .and_then(Option::as_ref)
                        .map(|s| &s.bind),
                    Group1::Terrain => Some(&self.terrain.bind),
                    Group1::Impostor(id) => self
                        .impostors
                        .get(id as usize)
                        .and_then(Option::as_ref)
                        .map(|i| &i.bind),
                };
                let Some(group) = group else {
                    return false;
                };
                pass.set_bind_group(1, group, &[]);
                g1 = Some(g);
            }
            if let Some(g) = d.g2
                && g2 != Some(g)
            {
                let (fading, slot) = g;
                let slots = if fading {
                    &self.fade_bind
                } else {
                    &self.draw_bind
                };
                pass.set_bind_group(2, slots, &[(slot as u64 * SLOT) as u32]);
                g2 = Some(g);
            }
            // The mesh's vertex, index and (terrain) weight buffers are the arena's: bound
            // once per pass, and again only when another arena's mesh follows.
            if let Some(m) = d.mesh
                && mesh != Some(m)
            {
                let Some(a) = self.arenas.get(m.arena as usize) else {
                    return false;
                };
                if arena != Some(m.arena) {
                    pass.set_vertex_buffer(0, a.vertices.slice(..));
                    pass.set_index_buffer(a.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.set_vertex_buffer(2, a.weights.slice(..));
                    arena = Some(m.arena);
                }
                mesh = Some(m);
            }
            if src != Some(d.src) {
                let buffer = match d.src {
                    Src::Pool => &self.pool.buffer,
                    Src::Gathered => &self.gather.out,
                };
                pass.set_vertex_buffer(1, buffer.slice(..));
                src = Some(d.src);
            }
            true
        };
        let mut next_args = u64::from(args) * u64::from(INDIRECT_ARGS);
        for (i, n) in Calls::new(draws, self.multi_draw) {
            let d = &draws[i];
            let bound = bind(pass, d);
            // A run that could not bind (an arena that is gone) is not drawn, so not counted.
            if n > 1 {
                if bound {
                    pass.multi_draw_indexed_indirect(&self.indirect, next_args, n as u32);
                    for d in &draws[i..i + n] {
                        self.counters
                            .add(d.triangles, d.instances.len() as u32, d.view);
                    }
                    self.counters.call();
                }
                next_args += u64::from(n as u32) * u64::from(INDIRECT_ARGS);
            } else if bound {
                self.counters
                    .add(d.triangles, d.instances.len() as u32, d.view);
                self.counters.call();
                match d.mesh {
                    Some(m) => {
                        pass.draw_indexed(m.range.range(), m.base_vertex, d.instances.clone())
                    }
                    None => pass.draw(0..d.vertices, d.instances.clone()),
                }
            }
        }
    }

    /// Glass, water and blended materials, per visible cluster, back to
    /// front by the distance to the cluster's centre (instances inside one
    /// 8 m cluster are not sorted among themselves).
    fn draw_transparent(&self, pass: &mut wgpu::RenderPass, cull: Mat4) {
        let mut draws = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            let Some(item) = item else { continue };
            let Some(bounds) = item.bounds else { continue };
            if outside(cull, bounds, true) {
                continue;
            }
            for (p, part) in item.parts.iter().enumerate() {
                let transparent = self
                    .materials
                    .get(part.material.0 as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|m| m.cpu.transparent());
                if !transparent || !matches!(self.meshes.get(part.mesh.0 as usize), Some(Some(_))) {
                    continue;
                }
                for c in &item.clusters {
                    let Some(b) = c.bounds else { continue };
                    if Self::cluster_lod(item, c, Pass::Forward) != Some(part.lod)
                        || outside(cull, b, true)
                    {
                        continue;
                    }
                    let d = ((b.0 + b.1) * 0.5).distance_squared(self.eye);
                    draws.push((d, i, p, c.range.clone()));
                }
            }
        }
        draws.sort_by(|a, b| b.0.total_cmp(&a.0));
        pass.set_bind_group(0, &self.globals_bind, &[]);
        pass.set_bind_group(3, &self.targets.forward_bind, &[]);
        for (_, i, p, range) in draws {
            let Some(item) = &self.items[i] else { continue };
            let part = &item.parts[p];
            let (Some(Some(mesh)), Some(material)) = (
                self.meshes.get(part.mesh.0 as usize),
                self.materials
                    .get(part.material.0 as usize)
                    .and_then(Option::as_ref),
            ) else {
                continue;
            };
            let sided = material.cpu.double_sided as usize;
            let glass = [&self.sampled.glass[0][sided], &self.sampled.glass[1][sided]];
            let pipelines: &[&wgpu::RenderPipeline] = if material.cpu.surface == Surface::Glass {
                &glass
            } else {
                &[&self.sampled.transparent[sided]]
            };
            let dm = mesh.draw(part.mesh);
            let arena = &self.arenas[dm.arena as usize];
            let Some(Some(texture_set)) = self.texture_sets.get(material.set as usize) else {
                continue;
            };
            pass.set_bind_group(1, &texture_set.bind, &[]);
            pass.set_bind_group(2, &self.draw_bind, &[(part.slot as u64 * SLOT) as u32]);
            pass.set_vertex_buffer(0, arena.vertices.slice(..));
            pass.set_vertex_buffer(1, self.pool.buffer.slice(..));
            pass.set_index_buffer(arena.indices.slice(..), wgpu::IndexFormat::Uint32);
            for p in pipelines {
                pass.set_pipeline(p);
                self.counters
                    .add(mesh.index_count as u64 / 3, range.len() as u32, true);
                self.counters.call();
                pass.draw_indexed(dm.range.range(), dm.base_vertex, item.pool_range(&range));
            }
        }
    }

    fn update_bounds(&mut self) {
        if !self.bounds_dirty {
            return;
        }
        self.bounds_dirty = false;
        for i in 0..self.items.len() {
            let Some(mut item) = self.items[i].take() else {
                continue;
            };
            let mesh_bounds = self.item_mesh_bounds(&item);
            item_bounds(&mut item, mesh_bounds);
            self.items[i] = Some(item);
        }
        self.scene_bounds_of_all();
    }

    /// The bounds of an item's meshes with room for the wind's sway at any speed (the lean and
    /// the detail both saturate).
    fn item_mesh_bounds(&self, item: &Item) -> Option<Bounds> {
        let mut mesh_bounds: Option<Bounds> = None;
        let mut sways = false;
        for p in &item.parts {
            if let Some(Some(m)) = self.meshes.get(p.mesh.0 as usize) {
                sways |= m.wind;
                if let Some(b) = m.bounds {
                    mesh_bounds = Some(mesh_bounds.map_or(b, |s| union(s, b)));
                }
            }
        }
        match (mesh_bounds, sways) {
            (Some((lo, hi)), true) => {
                let pad = Vec3::splat(wind::bounds_pad(
                    (hi - lo).max_element(),
                    self.wind.speed_mps,
                    self.wind.gusts,
                    item.grass,
                ));
                Some((lo - pad, hi + pad))
            }
            (b, _) => b,
        }
    }

    /// The scene's bounds over every item's, and the terrain's edge over the terrain items'.
    fn scene_bounds_of_all(&mut self) {
        let mut scene: Option<Bounds> = None;
        let mut terrain: Option<Bounds> = None;
        for item in self.items.iter().flatten() {
            let Some(b) = item.bounds else {
                continue;
            };
            scene = Some(scene.map_or(b, |s| union(s, b)));
            let is_terrain = item.parts.iter().any(|p| {
                self.meshes
                    .get(p.mesh.0 as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|m| m.weights)
            });
            if is_terrain {
                terrain = Some(terrain.map_or(b, |s| union(s, b)));
            }
        }
        self.scene_bounds = scene;
        if let Some((lo, hi)) = terrain {
            self.terrain.edge = [lo.x, lo.z, hi.x, hi.z];
        }
    }

    fn lod_scale(&self) -> f32 {
        self.quality.lod_distance_percent.max(1) as f32 / 100.0 * self.view_distance
    }

    /// Per cluster level of detail from the camera distance to its bounds,
    /// and the front-to-back sort keys.
    /// With an orthographic camera every cluster takes the level for the
    /// view's height in metres (what a perspective view showing that much
    /// would use) and the order follows depth along the view direction.
    fn update_lods(&mut self) {
        let eye = self.eye;
        let sun = self.sun.direction;
        let scale = self.lod_scale();
        let ortho = shadow::is_orthographic(self.proj).then(|| {
            let forward = -self
                .view
                .inverse()
                .z_axis
                .truncate()
                .normalize_or(Vec3::NEG_Z);
            (2.0 / self.proj.y_axis.y.abs().max(1e-9), forward)
        });
        let reach = self.impostor_reach;
        let skip = self.lod_skip;
        let mut fade_bytes: Vec<u8> = Vec::new();
        let mut fade_used = 0usize;
        for item in self.items.iter_mut().flatten() {
            // The impostor's end, shortened or lengthened by the reach.
            let mut lods = item.lods.clone();
            if item.impostor.is_some()
                && item.follows_reach
                && let Some(last) = lods.last_mut()
            {
                let mesh_end = item.lods[item.lods.len() - 2];
                *last = (*last * reach).max(mesh_end + 1.0);
            }
            let levels = item.lods.len() as u8;
            // Past the last level (its hysteresis included: the nearest cluster of the block
            // could not come back), every cluster culled by distance and nothing dissolving:
            // no level there changes, so the block's clusters are left out below.
            for block in &mut item.blocks {
                block.beyond = skip
                    && ortho.is_none()
                    && block.bounds.is_some_and(|(lo, hi)| {
                        eye.clamp(lo, hi).distance(eye) / scale
                            > lods.last().copied().unwrap_or(f32::INFINITY) - lod::HYSTERESIS_M
                    });
            }
            if skip && ortho.is_none() {
                // Any cluster of the block not past the last level, or dissolving: not beyond.
                for (c, &b) in item.clusters.iter().zip(&item.block_of) {
                    if !(c.target == levels && c.lod == levels && c.fade == 0 && !c.fading) {
                        item.blocks[b as usize].beyond = false;
                    }
                }
            }
            // An item with an impostor draws the clusters at its last level, as impostors: they
            // get their sort keys, as every cluster does each frame (the draws' order). The
            // others there are culled by distance: nothing to do for them.
            let impostor = item.impostor.is_some();
            // In pool order, as without the blocks.
            for (c, &block) in item.clusters.iter_mut().zip(&item.block_of) {
                if item.blocks[block as usize].beyond {
                    if impostor && let Some((lo, hi)) = c.bounds {
                        c.key = [eye.clamp(lo, hi).distance(eye), -((lo + hi) * 0.5).dot(sun)];
                    }
                    continue;
                }
                let Some((lo, hi)) = c.bounds else {
                    continue;
                };
                // Scaling the distance instead of the thresholds: same choice.
                let (d, order) = match ortho {
                    Some((height, forward)) => (height, ((lo + hi) * 0.5 - eye).dot(forward)),
                    None => {
                        let d = eye.clamp(lo, hi).distance(eye);
                        (d, d)
                    }
                };
                let d = d / scale;
                let current = (c.target != LOD_UNSET).then_some(c.target as usize);
                let target = lod::select(d, &lods, current, lod::HYSTERESIS_M) as u8;
                // Every hand-over between two levels dissolves (mesh to
                // mesh, the last mesh level into the impostor, the last level
                // into nothing with a cull fade), so walking up to a tree
                // shows no pop. Beyond the impostor, or a last level without
                // a cull fade, it is culled at once.
                let levels = item.lods.len() as u8;
                let cull_fades = item.impostor.is_none()
                    && item.cull_fade > 0.0
                    && item.lods[levels as usize - 1].is_finite();
                let dissolves = |l: u8| l < levels || (l == levels && cull_fades);
                if ortho.is_none()
                    && c.target != LOD_UNSET
                    && target != c.target
                    && dissolves(c.target)
                    && dissolves(target)
                {
                    // Crossing a hand-over: dissolve, or turn a running
                    // dissolve between the same two levels around where it is.
                    let (a, b) = (c.target.min(target), c.target.max(target));
                    c.fade = if c.fade > 0 && c.lod == a && c.far == b {
                        FADE_FRAMES - c.fade
                    } else {
                        FADE_FRAMES
                    };
                    c.to_far = target > c.target;
                    c.lod = a;
                    c.far = b;
                } else if ortho.is_some() || !dissolves(target) {
                    c.fade = 0;
                }
                c.target = target;
                c.fading = false;
                if c.fade > 0 && fade_used + item.parts.len() < FADE_SLOTS {
                    let done = (FADE_FRAMES - c.fade) as f32 + 1.0;
                    let progress = done / (FADE_FRAMES as f32 + 1.0);
                    let fade = if c.to_far { progress } else { 1.0 - progress };
                    c.fade_slot = fade_used as u32;
                    for p in &item.parts {
                        // The far mesh level keeps the samples the near one gives
                        // up: a negative fade (impostor.wgsl `fade_keeps`).
                        let f = if p.lod == c.far { -fade } else { fade };
                        fade_bytes.extend_from_slice(&fade_slot(p.mesh, row(p.material), f));
                        fade_bytes.resize(fade_bytes.len() + SLOT as usize - 16, 0);
                    }
                    fade_bytes.extend_from_slice(&fade_slot(
                        MeshId(u32::MAX - 2),
                        FROM_INSTANCE,
                        fade,
                    ));
                    fade_bytes.resize(fade_bytes.len() + SLOT as usize - 16, 0);
                    fade_used += item.parts.len() + 1;
                    c.fading = true;
                    c.fade -= 1;
                } else {
                    c.fade = 0;
                    c.lod = target;
                }
                if ortho.is_some() {
                    // A plan or elevation shows everything: the coarsest
                    // level instead of culling by distance.
                    c.lod = c.lod.min(levels - 1);
                }
                c.key = [order, -((lo + hi) * 0.5).dot(sun)];
            }
        }
        if !fade_bytes.is_empty() {
            self.queue.write_buffer(&self.fade_buffer, 0, &fade_bytes);
        }
    }

    /// Timestamp writes for one pass when timing is on.
    fn ts(&self, label: &'static str) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.timer.as_ref()?.pass(label)
    }

    fn exposure(&self) -> f32 {
        // Lagarde & de Rousiers 2014, eq. 12: saturation-based sensitivity.
        1.0 / (1.2 * 2f32.powf(self.ev100))
    }

    /// The sun, sky or ground changed: the radiance below the horizon (CPU),
    /// then the sky-view LUT, SH and environment cube (GPU; spread over a few
    /// frames unless `all`).
    fn update_ambient(&mut self, all: bool) {
        let albedo = if self.clay {
            Vec3::splat(0.8) // clay surfaces' albedo (scene.wgsl `light`)
        } else {
            self.bounce_albedo.unwrap_or_else(|| {
                self.ground
                    .map_or(Vec3::splat(0.2), |g| Vec3::from(g.color))
            })
        };
        self.ground_kcd = match &self.sky_source {
            SkySource::Hdri {
                mean, intensity, ..
            } => {
                let sun = self.sun.color * self.sun.illuminance * self.sun.direction.y.max(0.0);
                albedo * (sun / std::f32::consts::PI + *mean * *intensity) / 1000.0
            }
            // The analytic skies: the albedo; the GPU lights it with the sun
            // and the drawn sky (sky.wgsl `ground_kcd`).
            _ => albedo,
        };
        self.env.invalidate(all);
        self.sky_view_dirty = true;
    }

    fn invalidate_shadows(&mut self) {
        #[cfg(test)]
        {
            self.shadow_invalidations += 1;
        }
        self.far_cache = None;
        self.snow_drawn = None;
        self.cascade_rendered = [None; shadow::MAX_CASCADES];
        self.cascade_stale = [false; shadow::MAX_CASCADES];
    }

    /// One globals block (common.wgsl `Globals`) without the SH, which the
    /// GPU writes: for the camera (`planes`: the water planes with a planar
    /// reflection this frame, each its height and its layer), or (`clip`: the
    /// water height, no planes) for the mirrored camera of one of them.
    fn write_globals(
        &self,
        buffer: &wgpu::Buffer,
        cascades: &shadow::Cascades,
        (view, proj, eye): (Mat4, Mat4, Vec3),
        (w, h): (u32, u32),
        planes: &[(f32, u32)],
        clip: Option<f32>,
    ) {
        let exposure = self.exposure();
        let view_proj = proj * view;
        let q = &self.quality;
        let mut b = Bytes::default();
        for m in [view_proj, view, proj, view_proj.inverse()] {
            b.floats(&m.to_cols_array());
        }
        for m in &cascades.view_proj {
            b.floats(&m.to_cols_array());
        }
        let sun_light = self.sun.color * self.sun.illuminance * exposure * self.sun_share();
        let disc_solid_angle = std::f32::consts::PI * sky::SUN_DISC_RADIUS * sky::SUN_DISC_RADIUS;
        let disc = (self.sun.illuminance / disc_solid_angle * exposure).min(64.0);
        b.floats(&[eye.x, eye.y, eye.z, self.atmosphere.clear_m]);
        b.floats(&self.sun.direction.extend(disc).to_array());
        b.floats(&sun_light.extend(sky::SUN_DISC_RADIUS.cos()).to_array());
        b.floats(&cascades.splits);
        b.floats(&cascades.texel);
        let res = q.shadow_resolution as f32;
        b.floats(&[cascades.count as f32, res, 1.0 / res, self.shadow_distance]);
        let [shadow_radius_m, shadow_mode] = q.shadow_filter_uniform();
        b.floats(&[shadow_radius_m, shadow_mode, 0.0, 0.0]);
        let (w, h) = (w as f32, h as f32);
        b.floats(&[w, h, 1.0 / w, 1.0 / h]);
        b.floats(&[
            q.ao_samples as f32,
            if q.contact_shadows { 1.0 } else { 0.0 },
            AO_RADIUS,
            AO_INTENSITY,
        ]);
        for p in &self.sky.perez {
            b.floats(&p.extend(0.0).to_array());
        }
        b.floats(&self.sky.zenith.extend(self.sky.day).to_array());
        b.floats(&(sky::NIGHT / 1000.0).extend(1000.0 * exposure).to_array());
        let (mesh, face) = self
            .highlight
            .map_or((u32::MAX, u32::MAX), |(m, f)| (m.0, f));
        for u in [mesh, face, self.overlay_on_top, 0] {
            b.0.extend_from_slice(&u.to_le_bytes());
        }
        let wind = wind::Uniforms::new(&self.wind, &self.wind_phases);
        b.floats(&wind.wind);
        b.floats(&wind.phase);
        b.floats(&wind.amp);
        b.floats(&[
            self.time,
            exposure,
            (env::MIPS - 1) as f32,
            if self.clay { 1.0 } else { 0.0 },
        ]);
        let g = &self.grid;
        b.floats(&[
            g.origin.x,
            g.origin.y,
            1.0 / g.cell,
            self.light_count as f32,
        ]);
        for u in [g.dims.0, g.dims.1, 0, 0] {
            b.0.extend_from_slice(&u.to_le_bytes());
        }
        let (ow, oh) = (self.size.0 as f32, self.size.1 as f32);
        let scale = self.targets.size.0 as f32 / ow;
        b.floats(&[ow, oh, scale, 1.0 / scale]);
        let a = &self.atmosphere;
        let ground_height = self.ground.map_or(0.0, |g| g.height);
        // A plan or elevation has no horizon and no meaningful distance:
        // no haze.
        let haze = if shadow::is_orthographic(proj) {
            0.0
        } else {
            a.haze
        };
        // The haze thins upwards from the lowest open ground: with a coast, the sea.
        let haze_base = self.haze_base();
        b.floats(&[haze / 1000.0, 1.0 / a.haze_height_m, haze_base, a.sun_glow]);
        let (source, rotation, scale, mips) = match &self.sky_source {
            SkySource::Preetham => (0.0, 0.0, 1.0, 1.0),
            SkySource::Physical => (1.0, 0.0, SUN_TOP_KLX * self.sky_scale, 1.0),
            SkySource::Hdri {
                rotation,
                intensity,
                mips,
                ..
            } => (2.0, *rotation, intensity / 1000.0, *mips as f32),
        };
        b.floats(&[source, rotation, scale, mips]);
        let hdri_below = matches!(self.sky_source, SkySource::Hdri { .. })
            && self.ground.is_none()
            && !self.clay;
        b.floats(
            &self
                .ground_kcd
                .extend(if hdri_below { 1.0 } else { 0.0 })
                .to_array(),
        );
        let bloom = if q.bloom { self.bloom_strength } else { 0.0 };
        b.floats(&[
            q.shadow_taps() as f32,
            bloom,
            if q.ao_full_resolution { 1.0 } else { 2.0 },
            ground_height,
        ]);
        b.floats(&[
            0.0,
            0.0,
            if clip.is_some() { 1.0 } else { 0.0 },
            clip.unwrap_or(0.0),
        ]);
        for slot in 0..water::MAX_PLANES as usize {
            let (height, layer, on) = planes
                .get(slot)
                .map_or((0.0, 0.0, 0.0), |&(h, l)| (h, l as f32, 1.0));
            b.floats(&[height, layer, on, 0.0]);
        }
        b.floats(&[
            self.grade.0,
            self.grade.1,
            if self.indoors { 1.0 } else { 0.0 },
            // The mirrored camera of the planar reflection gathers no bounce light.
            if clip.is_some() { 0.0 } else { self.photo_seed },
        ]);
        // Clouds drift with the wind, about 3 m/s in a moderate breeze, by the unwrapped clock
        // (the shader clock wraps every 68 minutes): continuous for weeks of running.
        let wind = Vec2::new(self.wind.direction.x, self.wind.direction.z).normalize_or_zero();
        let travelled = (1.0 + 2.0 * f64::from(self.wind.strength())) * self.clock;
        let drift = Vec2::new(
            (f64::from(wind.x) * travelled) as f32,
            (f64::from(wind.y) * travelled) as f32,
        );
        let (cloud_cover, overcast, storm) = self.cloud_state();
        b.floats(&[cloud_cover, overcast, drift.x, drift.y]);
        b.floats(&self.snow_cover.unwrap_or(Mat4::ZERO).to_cols_array());
        b.floats(&[
            self.snow,
            self.quality.cascades as f32,
            if self.snow_cover.is_some() { 1.0 } else { 0.0 },
            self.grain,
        ]);
        b.floats(&self.sky_volume_box);
        // The sea's coastline: the endless ground stays land on its left (sea.rs).
        let coast = self.sea().map_or(&[][..], |s| &s.coast[..]);
        let n = if coast.len() >= 2 {
            coast.len().min(sea::COAST)
        } else {
            0
        };
        let level = self.sea().map_or(0.0, |s| s.level);
        let surroundings = self.ground.map_or(0.0, |g| g.surroundings as u32 as f32);
        b.floats(&[n as f32, level, level - 20.0, surroundings]);
        for i in 0..sea::COAST {
            b.floats(&coast.get(i).filter(|_| i < n).copied().unwrap_or([0.0; 2]));
        }
        b.floats(&self.ground.map_or([0.0; 4], |g| g.plot));
        // Screen-space reflections: on, the step (pixels), the most steps, the thickness (m).
        let step = self.quality.screen_reflections as f32;
        b.floats(&[
            // Not in the water's mirrored view, which has no depth or scene of its own to read.
            (step > 0.0 && clip.is_none()) as u32 as f32,
            step.max(1.0),
            if step > 1.0 { 40.0 } else { 80.0 },
            0.25,
        ]);
        // Wet surfaces and rain: the wetness drawn is the larger of the two.
        b.floats(&[
            self.wetness.max(self.rain),
            self.rain,
            self.sun_share(),
            if self.mist > 0.0 {
                atmosphere::mist_sun_share(self.mist, self.sun.direction.y)
            } else {
                1.0
            },
        ]);
        // Mist and rain fog: one low layer of droplets (common.wgsl `mist_depth`); none in a
        // plan or an elevation.
        let mist = if shadow::is_orthographic(proj) {
            [0.0; 4]
        } else {
            atmosphere::mist_uniform(self.mist, self.rain, haze_base, a.sun_glow)
        };
        b.floats(&mist);
        b.floats(&wind::Uniforms::new(&self.wind, &self.wind_phases).gust);
        // The air (shafts.wgsl); none of it in the water's mirrored view.
        let air = if clip.is_some() {
            Air {
                shafts: 0.0,
                glare: 0.0,
                aerial: false,
            }
        } else {
            self.air(proj)
        };
        let steps = if q.fast_shading { 12.0 } else { 16.0 };
        b.floats(&[
            air.shafts,
            steps,
            self.shadow_distance.min(SHAFTS_REACH_M),
            SHAFTS_DIV as f32,
        ]);
        let [share, narrow, wide, halo] = SUN_GLARE;
        b.floats(&[air.glare * share, narrow, wide, halo]);
        b.floats(&[
            if air.aerial { 1.0 } else { 0.0 },
            atmosphere::AERIAL_KM,
            0.002 + eye.y.max(0.0) / 1000.0,
            self.vignette,
        ]);
        let (white, reference) = match self.white_balance {
            WhiteBalance::Daylight => ([0.0; 4], [1.0, 1.0, 1.0, 0.0]),
            WhiteBalance::Auto => (
                [0.0, 0.0, 0.0, 2.0],
                [1.0, 1.0, 1.0, tonemap::AUTO_ADAPTATION],
            ),
            WhiteBalance::Kelvin(k) => (
                tonemap::kelvin_white(k).extend(1.0).to_array(),
                tonemap::kelvin_white(tonemap::DAYLIGHT_KELVIN)
                    .extend(1.0)
                    .to_array(),
            ),
        };
        b.floats(&white);
        b.floats(&reference);
        let (mut m, mut contrast) = tonemap::grade_matrix(self.color_grade);
        let mut on = if self.color_grade == ColorGrade::Neutral {
            0.0
        } else {
            1.0
        };
        if let Some((other, t)) = self.color_grade_mix {
            let t = t.clamp(0.0, 1.0);
            let (m2, c2) = tonemap::grade_matrix(other);
            m = m * (1.0 - t) + m2 * t;
            contrast = contrast * (1.0 - t) + c2 * t;
            on = if self.color_grade == ColorGrade::Neutral && other == ColorGrade::Neutral {
                0.0
            } else {
                1.0
            };
        }
        b.floats(&m.x_axis.extend(contrast).to_array());
        b.floats(&m.y_axis.extend(on).to_array());
        b.floats(&m.z_axis.extend(0.0).to_array());
        // The baked sky visibility: off unless switched on and given (not in the water's
        // mirrored view, whose surfaces are seen from below the water).
        b.floats(
            &self
                .sky_occ_box
                .filter(|_| self.sky_occlusion && clip.is_none())
                .unwrap_or([0.0; 8]),
        );
        // The volumetric clouds (clouds.wgsl, common.wgsl `cloud_shadow`): drawn in this view
        // (not in the water's mirrored one, which keeps the flat layer, nor in a plan); their
        // base, lower for a closed or stormy sky; the layer's thickness and the clouds' type.
        let ortho = shadow::is_orthographic(proj);
        let drawn = self.volumetric_clouds() && clip.is_none() && !ortho;
        let closed = overcast.max(storm);
        let base = self.clouds.altitude_m.max(100.0) * (1.0 - 0.45 * closed);
        let thickness = 2500.0 + 4500.0 * storm;
        let fair_type = 0.55 + 0.1 * cloud_cover;
        let kind = fair_type + (0.15 - fair_type) * overcast;
        let kind = kind + (1.0 - kind) * storm;
        b.floats(&[f32::from(u8::from(drawn)), base, thickness, kind]);
        // Their shadows, and the gap over the site: where the sun's ray through the site's
        // origin crosses the clouds' lower part (common.wgsl `cloud_coverage`).
        let shade = self.cloud_shadows(proj);
        let sun = self.sun.direction;
        let gap = if self.clouds.sun_gap && shade > 0.0 && sun.y > 0.02 {
            let mid = base + 0.15 * thickness;
            let centre = (Vec2::new(sun.x, sun.z) / sun.y * mid).clamp_length_max(30_000.0);
            [1.0 - closed, centre.x, centre.y]
        } else {
            [0.0; 3]
        };
        b.floats(&[shade, gap[0], gap[1], gap[2]]);
        let extinction = 0.03 * (1.0 + 0.25 * overcast + storm);
        b.floats(&[
            self.cloud_drift[0] as f32,
            self.cloud_drift[1] as f32,
            extinction,
            storm,
        ]);
        // Lightning in a storm (lightning.rs): the flash in the cloud, its bolt and its light on
        // the scene; none in a plan or an elevation.
        let flash = self.flash().filter(|_| !ortho);
        match flash {
            Some(f) => {
                let (sin, cos) = f.azimuth.sin_cos();
                let at = Vec3::new(
                    eye.x + cos * f.distance_m,
                    base + f.height * thickness,
                    eye.z + sin * f.distance_m,
                );
                b.floats(&at.extend(lightning::FLASH_KCD * f.strength).to_array());
                b.floats(&[
                    f.bolt.map_or(0.0, |seed| (seed >> 8) as f32),
                    if f.bolt.is_some() { f.strength } else { 0.0 },
                    lightning::fill_kcd(&f, self.sun.illuminance * self.sun.direction.y.max(0.0)),
                    ground_height,
                ]);
            }
            None => {
                b.floats(&[0.0; 4]);
                b.floats(&[0.0, 0.0, 0.0, ground_height]);
            }
        }
        debug_assert_eq!(b.0.len() as u64, SH_OFFSET);
        self.queue.write_buffer(buffer, 0, &b.0);
    }

    fn upload_overlay(&mut self) {
        let (tested, on_top) = &self.overlay_cpu;
        let mut b = Vec::with_capacity((tested.len() + on_top.len()) / 2 * 40);
        for list in [tested, on_top] {
            for v in list.as_chunks::<2>().0.iter().flatten() {
                for f in v.position {
                    b.extend_from_slice(&f.to_le_bytes());
                }
                b.extend_from_slice(&v.color);
                b.extend_from_slice(&v.width.to_le_bytes());
            }
        }
        self.overlay_on_top = (tested.len() / 2) as u32;
        self.overlay_count = (b.len() / 40) as u32;
        if b.is_empty() {
            return;
        }
        if self
            .overlay
            .as_ref()
            .is_none_or(|o| o.size() < b.len() as u64)
        {
            self.overlay = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay"),
                size: (b.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(o) = &self.overlay {
            self.queue.write_buffer(o, 0, &b);
        }
    }

    fn rebuild_targets(&mut self) {
        self.invalidate_shadows();
        self.targets = Targets::new(
            &self.device,
            &self.layouts,
            self.output_format,
            self.quality,
            self.size,
            &self.shadow_sampler,
            &self.post_sampler,
            &self.white,
            &self.physical.aerial,
            false,
        );
    }
}

/// A mesh's vertices and indices as the arenas hold them (`Renderer::store_mesh`).
struct PackedMesh {
    vertices: Vec<u8>,
    indices: Vec<u8>,
    bounds: Option<Bounds>,
    /// Any vertex with a bend weight: bounds get padded for the sway.
    wind: bool,
}

/// A mesh's data, packed as the vertex and index arenas hold it; Err (nothing is made) when its
/// arrays disagree.
fn pack_mesh(mesh: &Mesh) -> Result<PackedMesh, String> {
    let n = mesh.positions.len();
    if mesh.normals.len() != n {
        return Err(format!("{} normals for {n} positions", mesh.normals.len()));
    }
    for (name, len) in [
        ("face ids", mesh.face_ids.map(<[u32]>::len)),
        ("uvs", mesh.uvs.map(<[[f32; 2]]>::len)),
        ("wind weights", mesh.wind.map(<[[f32; 2]]>::len)),
    ] {
        if let Some(len) = len
            && len != n
        {
            return Err(format!("{len} {name} for {n} positions"));
        }
    }
    if !mesh.indices.len().is_multiple_of(3) {
        return Err(format!(
            "{} indices is not a whole number of triangles",
            mesh.indices.len()
        ));
    }
    if let Some(i) = mesh.indices.iter().find(|&&i| i as usize >= n) {
        return Err(format!("index {i} out of range for {n} vertices"));
    }
    let mut v = Bytes::default();
    let mut wind = false;
    for i in 0..n {
        v.floats(&mesh.positions[i]);
        v.floats(&mesh.normals[i]);
        let face = mesh.face_ids.map_or(u32::MAX, |f| f[i]);
        v.0.extend_from_slice(&face.to_le_bytes());
        v.floats(&mesh.uvs.map_or([0.0; 2], |u| u[i]));
        let w = mesh.wind.map_or([0.0; 2], |w| w[i]);
        wind |= w[0] > 0.0;
        v.floats(&[w[0].clamp(0.0, 1.0), w[1]]);
    }
    let mut ix = Vec::with_capacity(mesh.indices.len() * 4);
    for i in mesh.indices {
        ix.extend_from_slice(&i.to_le_bytes());
    }
    let bounds = mesh
        .positions
        .iter()
        .map(|p| (Vec3::from(*p), Vec3::from(*p)))
        .reduce(union);
    Ok(PackedMesh {
        vertices: v.0,
        indices: ix,
        bounds,
        wind,
    })
}

fn union(a: Bounds, b: Bounds) -> Bounds {
    (a.0.min(b.0), a.1.max(b.1))
}

fn corners((lo, hi): Bounds) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        )
    })
}

/// The bounds of a cluster's instances: the item's mesh bounds under each of them.
fn cluster_bounds(item: &Item, c: &Cluster, mesh_bounds: Option<Bounds>) -> Option<Bounds> {
    mesh_bounds.and_then(|b| {
        item.transforms[c.range.start as usize..c.range.end as usize]
            .iter()
            .map(|m| transform_bounds(*m, b))
            .reduce(union)
    })
}

/// An item's bounds from where its instances are: each cluster's (`cluster_bounds`), each
/// 128 m block's over the clusters in it (`update_lods` steps over those), its own, and its
/// size. For every item when the scene changed (`update_bounds`) and for an item an instance
/// of which moved (`update_moved`): a block that kept the bounds of before the move would step
/// over clusters that should draw.
fn item_bounds(item: &mut Item, mesh_bounds: Option<Bounds>) {
    for c in 0..item.clusters.len() {
        item.clusters[c].bounds = cluster_bounds(&*item, &item.clusters[c], mesh_bounds);
    }
    for block in &mut item.blocks {
        block.bounds = None;
    }
    for (c, &b) in item.clusters.iter().zip(&item.block_of) {
        if let Some(cb) = c.bounds {
            let block = &mut item.blocks[b as usize];
            block.bounds = Some(match block.bounds {
                Some(bb) => union(bb, cb),
                None => cb,
            });
        }
    }
    item.bounds = item.clusters.iter().filter_map(|c| c.bounds).reduce(union);
    item.size = item_size(item, mesh_bounds);
}

/// The bounding radius (m) of an item's largest instance: what the reflection leaves out when
/// it would be under `MIN_REFLECTED_PX`.
fn item_size(item: &Item, mesh_bounds: Option<Bounds>) -> f32 {
    let scale = item
        .transforms
        .iter()
        .map(|m| {
            m.x_axis
                .truncate()
                .length()
                .max(m.y_axis.truncate().length())
                .max(m.z_axis.truncate().length())
        })
        .fold(0.0, f32::max);
    mesh_bounds.map_or(0.0, |(lo, hi)| (hi - lo).length() * 0.5 * scale)
}

fn transform_bounds(m: Mat4, b: Bounds) -> Bounds {
    corners(b)
        .map(|c| m.transform_point3(c))
        .into_iter()
        .map(|p| (p, p))
        .reduce(union)
        .unwrap_or(b)
}

/// True when the box is entirely outside one clip plane of `view_proj`.
fn outside(view_proj: Mat4, b: Bounds, near_far: bool) -> bool {
    // Every corner beyond one clip plane (x < -w, x > w, ...) is the box's corner furthest
    // inside it still beyond: its centre plus the extents along the plane's normal. Six dot
    // products instead of eight corners through the matrix (the culling of every cluster, every
    // pass, was a tenth of a frame's CPU time in the browser).
    let (c, e) = ((b.0 + b.1) * 0.5, (b.1 - b.0) * 0.5);
    let m = view_proj.transpose();
    let (x, y, z, w) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
    let beyond = |n: Vec4| n.truncate().dot(c) + n.w + n.truncate().abs().dot(e) < 0.0;
    beyond(w + x)
        || beyond(w - x)
        || beyond(w + y)
        || beyond(w - y)
        || (near_far && (beyond(z) || beyond(w - z)))
}

/// Little-endian byte packing for uniforms and vertex data (no bytemuck,
/// no unsafe).
#[derive(Default)]
struct Bytes(Vec<u8>);

impl Bytes {
    fn floats(&mut self, f: &[f32]) {
        for x in f {
            self.0.extend_from_slice(&x.to_le_bytes());
        }
    }
}

/// The material table's size in bytes for `rows` rows, from `capacity` rows before: twice the
/// old capacity, or just enough when that is more, at most what one binding holds (`limit`:
/// the device's largest storage buffer binding, `limit / MATERIAL_SIZE` rows). A power of two
/// (`storage_buffer` rounds up to one) that never passes the limit, where the rounding up of
/// the clamped size could.
fn table_size(rows: u32, capacity: u32, limit: u64) -> u64 {
    let max = limit / MATERIAL_SIZE;
    assert!(
        u64::from(rows) <= max,
        "{rows} materials: more than the device's largest buffer holds"
    );
    let wanted = rows
        .max(capacity.saturating_mul(2))
        .min(u32::try_from(max).unwrap_or(u32::MAX));
    (u64::from(wanted) * MATERIAL_SIZE)
        .next_power_of_two()
        .min(limit)
}

/// scene.wgsl's texture bits of the six map slots; a slot 6 filled while the material has no
/// occlusion map holds the height map (HAS_HEIGHT, 64, instead of 32).
fn texture_bits(present: [bool; 6], no_occlusion: bool) -> u32 {
    let bits = (0..6).fold(0u32, |acc, i| acc | ((present[i] as u32) << i));
    if no_occlusion && bits & 32 != 0 {
        bits ^ (32 | 64)
    } else {
        bits
    }
}

/// The material uniform (scene.wgsl `Material`); `bits` flags the maps
/// that are present.
fn material_bytes(material: &Material, bits: u32) -> Vec<u8> {
    let mut b = Bytes::default();
    b.floats(&material.base_color);
    b.floats(&[
        material.metallic.clamp(0.0, 1.0),
        material.roughness.clamp(0.0, 1.0),
        material.relief.max(0.0),
        material.gloss,
    ]);
    let e = Vec3::from(material.emissive) * material.emissive_strength.max(0.0);
    b.floats(&e.extend(material.weathering.clamp(0.0, 1.0)).to_array());
    let threshold = match material.alpha_mode {
        AlphaMode::Cutout { threshold } => threshold.clamp(0.0, 1.0),
        _ => 0.0,
    };
    let inv_scale = match material.projection {
        Projection::Uv => 0.0,
        Projection::Triplanar { scale_m } => 1.0 / scale_m.max(1e-3),
    };
    let surface = match material.surface {
        Surface::Standard => 0.0,
        Surface::Glass => 1.0,
        Surface::Water => 2.0,
    };
    b.floats(&[
        threshold,
        inv_scale,
        surface,
        material.foliage as u32 as f32,
    ]);
    let variation = material.variation.clamp(0.0, 1.0).to_bits();
    for u in [
        bits,
        material.double_sided as u32,
        variation,
        material.stripes,
    ] {
        b.0.extend_from_slice(&u.to_le_bytes());
    }
    let north = glam::Vec2::from(material.aging_north)
        .try_normalize()
        .unwrap_or(glam::Vec2::NEG_Y);
    b.floats(&[
        material.parallax_depth_m.clamp(0.0, 0.1),
        north.x,
        north.y,
        material.puddles as u32 as f32,
    ]);
    b.floats(&material.aging.map(|k| k.clamp(0.0, 1.0)));
    debug_assert_eq!(b.0.len() as u64, MATERIAL_SIZE);
    b.0
}

fn f32_bytes(f: &[f32]) -> Vec<u8> {
    let mut b = Bytes::default();
    b.floats(f);
    b.0
}

fn u32_bytes(u: &[u32]) -> Vec<u8> {
    u.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// The snow cover map's camera: straight down from over the scene onto a square of
/// `SNOW_COVER_M` round the eye, its centre moved in whole texels so the cover does not crawl.
fn snow_cover_matrix(eye: Vec3, lo: Vec3, hi: Vec3, resolution: u32) -> Mat4 {
    let half = SNOW_COVER_M / 2.0;
    // Centred on the eye in steps of eight whole texels (1.25 m at 1024): the map is drawn
    // again only when it moves (`snow_drawn`).
    let step = 8.0 * SNOW_COVER_M / resolution.max(1) as f32;
    let c = (glam::Vec2::new(eye.x, eye.z) / step).round() * step;
    let top = hi.y + 1.0;
    let view = Mat4::look_to_rh(Vec3::new(c.x, top, c.y), Vec3::NEG_Y, Vec3::NEG_Z);
    Mat4::orthographic_rh(-half, half, -half, half, 0.0, (top - lo.y + 1.0).max(1.0)) * view
}

/// The snow cover map spans this square round the eye (metres); beyond, snow lies everywhere
/// that faces up.
const SNOW_COVER_M: f32 = 160.0;

fn globals_size() -> u64 {
    // 9 matrices, 67 vec4s, the 9 SH vec4s last; see common.wgsl.
    SH_OFFSET + 9 * 16
}

/// The clouds over the analytic skies: a flat procedural layer (fractal noise on a plane 1.5 km
/// up, lit by the sun and the sky), or volumetric clouds (clouds.wgsl) where `volumetric` asks
/// for them and the quality has them (`Quality::clouds`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clouds {
    /// 0 clear .. 1 covered.
    pub cover: f32,
    /// 0 separate fair-weather clouds .. 1 a closed grey layer; in between a sky closing in.
    pub overcast: f32,
    /// 0 .. 1 storm clouds: towering cumulonimbus, dark and dense (and overcast).
    pub storm: f32,
    /// Volumetric clouds with their shadows on the ground (off: the flat layer).
    pub volumetric: bool,
    /// The volumetric clouds' base over the ground (m); a closed or stormy sky lowers it.
    pub altitude_m: f32,
    /// The clouds' drift (m/s, downwind); `None`: from the wind at the ground.
    pub speed_mps: Option<f32>,
    /// A gap in the broken clouds in front of the sun over the site, so the design stays in
    /// the sun; off, cloud shadows drift over it.
    pub sun_gap: bool,
    /// How often lightning flashes in storm clouds, 0..1 (times `storm`): flashes light the
    /// clouds from inside and the scene for a moment, some with a bolt far off, all at fixed
    /// moments of the animation clock (`Renderer::set_time`).
    pub lightning: f32,
}

impl Default for Clouds {
    fn default() -> Self {
        Clouds {
            cover: 0.0,
            overcast: 0.0,
            storm: 0.0,
            volumetric: false,
            altitude_m: 1500.0,
            speed_mps: None,
            sun_gap: true,
            lightning: 0.5,
        }
    }
}

impl Clouds {
    /// How much of the sun's direct light comes through: fair-weather clouds pass most of it
    /// on average, an overcast layer little (soft shadows, a low sun to sky ratio), a storm
    /// least.
    pub fn sun_share(&self) -> f32 {
        let cover = self.cover.clamp(0.0, 1.0);
        let storm = self.storm.clamp(0.0, 1.0);
        let overcast = self.overcast.clamp(0.0, 1.0).max(storm);
        let fair = 1.0 - 0.15 * cover;
        let closed = 1.0 - cover * (0.85 + 0.12 * storm);
        fair + (closed - fair) * overcast
    }
}

/// An impostor bake in progress (`Renderer::start_impostor_bake`).
pub struct ImpostorBake {
    parts: Vec<(MeshId, MaterialId)>,
    state: Option<impostor::BakeState>,
}

/// The bloom levels: half the render size, halved again per level (the
/// downsample pass maps each output pixel to a 2x2 block of its source).
fn bloom_sizes((w, h): (u32, u32)) -> [(u32, u32); BLOOM_LEVELS] {
    let mut size = (w, h);
    std::array::from_fn(|_| {
        size = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        size
    })
}

/// Reflection about the horizontal plane y = h.
fn mirror_matrix(h: f32) -> Mat4 {
    Mat4::from_cols(
        Vec4::X,
        Vec4::NEG_Y,
        Vec4::Z,
        Vec4::new(0.0, 2.0 * h, 0.0, 1.0),
    )
}

fn texture_size(view: &wgpu::TextureView) -> (u32, u32) {
    let t = view.texture();
    (t.width(), t.height())
}

/// A draw slot that also carries a dissolve's progress, 0 the last mesh
/// level .. 1 the impostor or nothing (impostor.wgsl `lod_fade`).
fn fade_slot(mesh: MeshId, material: u32, fade: f32) -> [u8; 16] {
    let mut out = mesh_slot(mesh, material);
    out[8..12].copy_from_slice(&fade.to_le_bytes());
    out
}

/// What the draw-slot buffer is for: a uniform the queue writes; in a test also one to copy
/// from, to read a slot back (`tests::slots`).
fn draw_slot_usage() -> wgpu::BufferUsages {
    #[cfg(not(test))]
    let usage = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
    // In a test the buffer is copied out for `tests::slots`.
    #[cfg(test)]
    let usage =
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
    usage
}

/// A draw slot (`scene.wgsl` `Draw`): x the mesh id for the face highlight, y the material's
/// row; `FROM_INSTANCE` where a gathered instance carries it.
fn mesh_slot(mesh: MeshId, material: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&mesh.0.to_le_bytes());
    out[4..8].copy_from_slice(&material.to_le_bytes());
    out
}

fn slot_binding(buffer: &wgpu::Buffer, size: u64) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer,
        offset: 0,
        size: NonZeroU64::new(size),
    })
}

fn draw_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("draw"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: slot_binding(buffer, 16),
        }],
    })
}

#[allow(clippy::too_many_arguments)]
fn globals_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    env: &wgpu::TextureView,
    env_sampler: &wgpu::Sampler,
    lights: &wgpu::Buffer,
    grid: &wgpu::Buffer,
    sky: &wgpu::TextureView,
    [room_volume, occupancy, sky_occ]: [&wgpu::TextureView; 3],
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(env),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(env_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: grid.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(sky),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(sky_occ),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(room_volume),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(occupancy),
            },
        ],
    })
}

/// A baked sky visibility grid for `Renderer::set_sky_visibility` (visioncraft-core's
/// `SkyVisibility`, `texels()`): columns `cell` apart from `origin` (x, z), `dims` columns along
/// x, layers and columns along z, layer k at `lift + height (k / (layers - 1))^2` over the ground,
/// and the probes as three tiles side by side along x (x fastest, then the layer, then z).
#[derive(Clone, Copy, Debug)]
pub struct SkyVisibilityGrid<'a> {
    pub origin: [f32; 2],
    pub cell: f32,
    pub dims: [u32; 3],
    pub lift: f32,
    pub height: f32,
    pub texels: &'a [[f32; 4]],
}

/// The baked sky visibility as an `Rgba16Float` 3D texture three tiles wide (common.wgsl
/// `sky_occ_tex`), one level.
fn sky_occ_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dims: [u32; 3],
    texels: &[[f32; 4]],
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: 3 * dims[0],
        height: dims[1],
        depth_or_array_layers: dims[2],
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sky visibility"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes: Vec<u8> = texels
        .iter()
        .flat_map(|s| s.map(|v| hdri::f16_bits(v.clamp(-60000.0, 60000.0))))
        .flat_map(u16::to_le_bytes)
        .collect();
    queue.write_texture(
        texture.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8 * 3 * dims[0]),
            rows_per_image: Some(dims[1]),
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// A sky visibility volume for `Renderer::set_sky_volume`: origin (m), cell (m), cells per
/// axis, samples, room light, and its occluders' voxels (0.2 m from the same origin, nonzero
/// solid) with their count per axis.
pub type SkyVolume<'a> = (
    [f32; 3],
    f32,
    [u32; 3],
    &'a [[f32; 4]],
    &'a [[f32; 4]],
    (&'a [u8], [u32; 3]),
);

/// The occluders' voxels of a sky volume as an `R8Unorm` 3D texture.
fn occupancy_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dims: [u32; 3],
    solid: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: dims[0],
        height: dims[1],
        depth_or_array_layers: dims[2],
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sky volume voxels"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        solid,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(dims[0]),
            rows_per_image: Some(dims[1]),
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// A sky visibility volume as an `Rgba16Float` 3D texture, one level, two tiles side by side
/// along x (common.wgsl `room_vis`): the room light, rgb the lamps' and the sun's light passed
/// round the rooms (lux) and a the sky's; then the sky's visibility, r the visible fraction and
/// gba the mean visible direction.
fn room_volume_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dims: [u32; 3],
    room: &[[f32; 4]],
    sky: &[[f32; 4]],
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: 2 * dims[0],
        height: dims[1],
        depth_or_array_layers: dims[2],
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("room volume"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes = pack_room_volume(dims[0] as usize, room, sky);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16 * dims[0]),
            rows_per_image: Some(dims[1]),
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// The room tile of a sky volume's texels (`room_volume_texture`): per sample rgba as f16, little
/// endian, clamped as there.
fn pack_room_light(room: &[[f32; 4]]) -> Vec<u8> {
    room.iter()
        .flat_map(texel_bits)
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// One rgba texel as f16 bits, clamped to what the texture's format and the shader take.
fn texel_bits(s: &[f32; 4]) -> [u16; 4] {
    s.map(|v| hdri::f16_bits(v.clamp(-1.0, 60000.0)))
}

/// A whole sky volume's texels, `row` samples wide: per row the room tile, then the sky tile.
fn pack_room_volume(row: usize, room: &[[f32; 4]], sky: &[[f32; 4]]) -> Vec<u8> {
    room.chunks(row)
        .zip(sky.chunks(row))
        .flat_map(|(r, s)| r.iter().chain(s))
        .flat_map(texel_bits)
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn sky_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    sky: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sky source"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(sky),
            },
        ],
    })
}

fn depth_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    store: wgpu::StoreOp,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A single-target colour pass. `clear: None` with `load: None` clears to
/// black for fullscreen passes; `load: Some(())` keeps the contents.
fn color_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
    load: Option<()>,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites>,
) -> wgpu::RenderPass<'e> {
    let load = match (load, clear) {
        (Some(()), _) => wgpu::LoadOp::Load,
        (None, Some(c)) => wgpu::LoadOp::Clear(c),
        (None, None) => wgpu::LoadOp::Clear(wgpu::Color::BLACK),
    };
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn texture_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    view_dimension: wgpu::TextureViewDimension,
    multisampled: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension,
            multisampled,
        },
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dynamic: Option<u64>,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic.is_some(),
            min_binding_size: dynamic.and_then(NonZeroU64::new),
        },
        count: None,
    }
}

fn sampler_entry(binding: u32, ty: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(ty),
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Layouts {
    fn new(device: &wgpu::Device) -> Self {
        use wgpu::SamplerBindingType as B;
        use wgpu::TextureSampleType as S;
        use wgpu::TextureViewDimension as D;
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let unfilterable = S::Float { filterable: false };
        let filterable = S::Float { filterable: true };
        let fs = wgpu::ShaderStages::FRAGMENT;
        let vfs = wgpu::ShaderStages::VERTEX_FRAGMENT;
        // Binding 0: every material's parameters (scene.wgsl `materials`), read-only storage.
        let mut material = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: fs,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];
        material.extend((1..=6).map(|b| texture_entry(b, filterable, D::D2, false)));
        material.extend((7..=12).map(|b| sampler_entry(b, B::Filtering)));
        Self {
            globals: layout(
                "globals",
                &[
                    uniform_entry(0, vfs, None),
                    texture_entry(1, filterable, D::Cube, false),
                    sampler_entry(2, B::Filtering),
                    storage_entry(3),
                    storage_entry(4),
                    texture_entry(5, filterable, D::D2, false),
                    texture_entry(6, filterable, D::D3, false),
                    texture_entry(7, filterable, D::D3, false),
                    texture_entry(8, filterable, D::D3, false),
                ],
            ),
            globals_only: layout("globals only", &[uniform_entry(0, vfs, None)]),
            sky_source: layout(
                "sky source",
                &[
                    uniform_entry(0, vfs, None),
                    sampler_entry(2, B::Filtering),
                    texture_entry(5, filterable, D::D2, false),
                ],
            ),
            terrain: terrain::layout(device),
            impostor: impostor::layout(device),
            sea: sea::layout(device),
            post: layout(
                "post",
                &[
                    texture_entry(0, filterable, D::D2, false),
                    sampler_entry(1, B::Filtering),
                    texture_entry(2, filterable, D::D2, false),
                ],
            ),
            material: layout("material", &material),
            draw: layout("draw", &[uniform_entry(0, vfs, Some(16))]),
            cascade: layout(
                "cascade",
                &[uniform_entry(
                    1,
                    wgpu::ShaderStages::VERTEX,
                    Some(CASCADE_SIZE),
                )],
            ),
            forward: layout(
                "forward",
                &[
                    // The sea's vertices read the cover map's heights.
                    wgpu::BindGroupLayoutEntry {
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ..texture_entry(0, S::Depth, D::D2Array, false)
                    },
                    sampler_entry(1, B::Comparison),
                    texture_entry(2, unfilterable, D::D2, false),
                    texture_entry(3, unfilterable, D::D2, false),
                    // The planar reflection: one layer per water plane with a pass.
                    texture_entry(4, filterable, D::D2Array, false),
                    texture_entry(5, unfilterable, D::D2, false),
                ],
            ),
            depth_ms: layout("depth ms", &[texture_entry(0, S::Depth, D::D2, true)]),
            depth_1x: layout("depth 1x", &[texture_entry(1, S::Depth, D::D2, false)]),
            texture: layout("texture", &[texture_entry(0, unfilterable, D::D2, false)]),
            blur: layout(
                "blur",
                &[
                    texture_entry(0, unfilterable, D::D2, false),
                    texture_entry(1, unfilterable, D::D2, false),
                ],
            ),
            underlay: layout(
                "underlay",
                &[
                    texture_entry(0, filterable, D::D2, false),
                    sampler_entry(1, B::Filtering),
                ],
            ),
            shafts: layout(
                "air",
                &[
                    texture_entry(0, S::Depth, D::D2Array, false),
                    sampler_entry(1, B::Comparison),
                    texture_entry(2, unfilterable, D::D2, false),
                    texture_entry(3, unfilterable, D::D2, false),
                    texture_entry(4, filterable, D::D2, false),
                    sampler_entry(5, B::Filtering),
                    texture_entry(6, unfilterable, D::D2, false),
                ],
            ),
            sky_clouds: layout(
                "sky clouds",
                &[
                    texture_entry(0, filterable, D::D2, false),
                    sampler_entry(1, B::Filtering),
                ],
            ),
        }
    }
}

/// Mesh and instance buffer layouts; `depth_only` declares just what the
/// depth and shadow passes fetch (position, uv, wind).
fn vertex_layouts(depth_only: bool) -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
    const MESH: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Uint32, 3 => Float32x2, 4 => Float32x2
    ];
    const DEPTH_MESH: [wgpu::VertexAttribute; 3] = [MESH[0], MESH[3], MESH[4]];
    const INSTANCE: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4];
    [
        Some(wgpu::VertexBufferLayout {
            array_stride: VERTEX_STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: if depth_only { &DEPTH_MESH } else { &MESH },
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: 64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE,
        }),
    ]
}

struct PipelineDesc<'a> {
    label: &'a str,
    module: &'a wgpu::ShaderModule,
    vs: &'a str,
    fs: Option<&'a str>,
    groups: &'a [Option<&'a wgpu::BindGroupLayout>],
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    target: Option<wgpu::ColorTargetState>,
    depth: Option<wgpu::DepthStencilState>,
    cull: Option<wgpu::Face>,
    samples: u32,
    constants: &'a [(&'a str, f64)],
    /// Clockwise front faces: the mirrored reflection view flips winding.
    front_cw: bool,
}

fn pipeline(device: &wgpu::Device, d: PipelineDesc) -> wgpu::RenderPipeline {
    pipeline_with(device, d, wgpu::PrimitiveTopology::TriangleList)
}

/// `pipeline` drawing `topology` (the grass's blades are triangle strips).
fn pipeline_with(
    device: &wgpu::Device,
    d: PipelineDesc,
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    let options = wgpu::PipelineCompilationOptions {
        constants: d.constants,
        ..Default::default()
    };
    let has_target = d.target.is_some();
    let targets = [d.target];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(d.label),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(d.label),
                bind_group_layouts: d.groups,
                immediate_size: 0,
            }),
        ),
        vertex: wgpu::VertexState {
            module: d.module,
            entry_point: Some(d.vs),
            compilation_options: options.clone(),
            buffers: d.buffers,
        },
        fragment: d.fs.map(|fs| wgpu::FragmentState {
            module: d.module,
            entry_point: Some(fs),
            compilation_options: options,
            targets: &targets[..usize::from(has_target)],
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            cull_mode: d.cull,
            front_face: if d.front_cw {
                wgpu::FrontFace::Cw
            } else {
                wgpu::FrontFace::Ccw
            },
            ..Default::default()
        },
        depth_stencil: d.depth,
        multisample: wgpu::MultisampleState {
            count: d.samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn depth_state(
    write: bool,
    compare: wgpu::CompareFunction,
    bias: wgpu::DepthBiasState,
) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias,
    }
}

/// The line of common.wgsl that `Quality::fast_shading` sets to true.
const FAST_SHADING_LINE: &str = "const FAST_SHADING: bool = false;";

/// The line of common.wgsl that sizes `scratch_floor`'s array.
const SCRATCH_FLOOR_LINE: &str = "const SCRATCH_FLOOR_VEC4S: u32 = 17u;";

/// Whether `info`'s driver may give fragment shaders of 1 KB and 2 KB of scratch one buffer with
/// two strides (common.wgsl `scratch_floor`): Mesa's Intel Vulkan driver, which does before Gfx
/// 12.5, and a browser's WebGPU unless its description names another vendor (Chrome on Linux runs on that
/// driver). The floor costs Iris Xe 1 to 4 % of a LOW or MEDIUM frame (its spills spread over
/// more scratch), so other drivers go without it.
fn needs_scratch_floor(info: &wgpu::AdapterInfo) -> bool {
    let mesa = |s: &str| s.to_ascii_lowercase().contains("mesa");
    match info.backend {
        wgpu::Backend::BrowserWebGpu => browser_gpu_may_be_intel(info),
        wgpu::Backend::Vulkan => {
            info.vendor == 0x8086 && (mesa(&info.driver) || mesa(&info.driver_info))
        }
        _ => false,
    }
}

/// Whether a browser's WebGPU adapter may be an Intel one: wgpu only fills the adapter's
/// description there (no vendor id, no driver), so a description naming another vendor rules it
/// out, and an Intel, empty or unknown one keeps the floor.
fn browser_gpu_may_be_intel(info: &wgpu::AdapterInfo) -> bool {
    if info.vendor != 0 && info.vendor != 0x8086 {
        return false;
    }
    let text = format!("{} {} {}", info.name, info.driver, info.driver_info).to_ascii_lowercase();
    if text.contains("intel") {
        return true;
    }
    const OTHERS: [&str; 8] = [
        "nvidia", "amd", "radeon", "apple", "qualcomm", "adreno", "mali", "arm",
    ];
    !OTHERS.iter().any(|vendor| text.contains(vendor))
}

/// `source` for `device`: without `scratch_floor`'s scratch (an array of one, kept in registers)
/// where the driver does not need it.
fn for_device(source: String, device: &wgpu::Device) -> String {
    if needs_scratch_floor(&device.adapter_info()) {
        source
    } else {
        source.replace(SCRATCH_FLOOR_LINE, "const SCRATCH_FLOOR_VEC4S: u32 = 1u;")
    }
}

/// One shader module's source (`SHADERS` entry), for the fast shading of MEDIUM and LOW or the
/// full one.
fn shader_source(parts: &[&str], fast_shading: bool) -> String {
    let source = parts.concat();
    if fast_shading {
        source.replace(FAST_SHADING_LINE, "const FAST_SHADING: bool = true;")
    } else {
        source
    }
}

/// The source of the `SHADERS` module `name`: on Vulkan the scene module also has the
/// per-sample coverage stages (`coverage.rs`), elsewhere it is as written.
fn module_source(
    name: &str,
    parts: &[&str],
    fast_shading: bool,
    coverage: coverage::Coverage,
) -> String {
    let mut source = shader_source(parts, fast_shading);
    if name == "scene" && coverage == coverage::Coverage::PerSample {
        source.push_str(coverage::PER_SAMPLE_SOURCE);
    }
    source
}

fn shader_modules(
    device: &wgpu::Device,
    fast_shading: bool,
    coverage: coverage::Coverage,
) -> Vec<wgpu::ShaderModule> {
    SHADERS
        .iter()
        .map(|(name, parts)| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(
                    for_device(module_source(name, parts, fast_shading, coverage), device).into(),
                ),
            })
        })
        .collect()
}

fn module<'a>(modules: &'a [wgpu::ShaderModule], name: &str) -> &'a wgpu::ShaderModule {
    &modules[SHADERS
        .iter()
        .position(|(n, _)| *n == name)
        .expect("shader name")]
}

fn cull(double_sided: bool) -> Option<wgpu::Face> {
    (!double_sided).then_some(wgpu::Face::Back)
}

/// The forward pass over the colour target (multisampled `view` resolving into `resolve`) and
/// the depth of the pre-pass; `(first, split)`: with screen-space reflections the opaque scene
/// (`first`) and the glass and mirrors are two passes, so the first keeps its multisampled
/// colour and depth for the second.
fn forward_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    label: &'static str,
    (view, resolve, depth): (
        &wgpu::TextureView,
        Option<&wgpu::TextureView>,
        &wgpu::TextureView,
    ),
    (first, split): (bool, bool),
    timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'a> {
    let keep = |kept: bool| {
        if kept {
            wgpu::StoreOp::Store
        } else {
            wgpu::StoreOp::Discard
        }
    };
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: resolve,
            ops: wgpu::Operations {
                load: if first {
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                } else {
                    wgpu::LoadOp::Load
                },
                store: keep(resolve.is_none() || (first && split)),
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: keep(first && split),
            }),
            stencil_ops: None,
        }),
        timestamp_writes: timestamps,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A pipeline drawing scene meshes with the forward layout (groups 0-3).
struct Forward<'a> {
    label: &'a str,
    fs: &'a str,
    variant: usize,
    target: wgpu::ColorTargetState,
    depth: wgpu::DepthStencilState,
    samples: u32,
    front_cw: bool,
}

fn forward_pipeline(
    device: &wgpu::Device,
    l: &Layouts,
    modules: &[wgpu::ShaderModule],
    f: Forward,
) -> wgpu::RenderPipeline {
    let groups = [
        Some(&l.globals),
        Some(&l.material),
        Some(&l.draw),
        Some(&l.forward),
    ];
    pipeline(
        device,
        PipelineDesc {
            label: f.label,
            module: module(modules, "scene"),
            vs: "vs_main",
            fs: Some(f.fs),
            groups: &groups,
            buffers: &vertex_layouts(false),
            target: Some(f.target),
            depth: Some(f.depth),
            cull: cull(f.variant % 2 == 1),
            samples: f.samples,
            constants: &[],
            front_cw: f.front_cw,
        },
    )
}

/// The infinite ground (no vertex buffers, forward layout).
/// `part`: common.wgsl `VOLUME_PART` (0 the whole ground, 1 its cells near the sky volume, 2
/// the others, compiled without the volume's lookup).
fn ground_pipeline(
    device: &wgpu::Device,
    l: &Layouts,
    modules: &[wgpu::ShaderModule],
    fs: &str,
    samples: u32,
    part: u32,
) -> wgpu::RenderPipeline {
    let constants = [("VOLUME_PART", part as f64)];
    pipeline(
        device,
        PipelineDesc {
            label: "ground",
            module: module(modules, "scene"),
            vs: "vs_ground",
            fs: Some(fs),
            groups: &[
                Some(&l.globals),
                Some(&l.material),
                Some(&l.draw),
                Some(&l.forward),
            ],
            buffers: &[],
            target: Some(HDR.into()),
            // In front of the far plane only: fills what nothing covered,
            // and writes depth so the sky leaves it alone.
            depth: Some(depth_state(
                true,
                wgpu::CompareFunction::LessEqual,
                Default::default(),
            )),
            cull: None,
            samples,
            constants: &constants,
            front_cw: false,
        },
    )
}

/// Impostor quads: no mesh, the instance buffer in slot 1.
fn impostor_pipeline(
    device: &wgpu::Device,
    l: &Layouts,
    modules: &[wgpu::ShaderModule],
    fs: &str,
    samples: u32,
    shadow: bool,
) -> wgpu::RenderPipeline {
    let [_, instances] = vertex_layouts(true);
    let samples_constant = [("SAMPLES", samples as f64)];
    // Depth only: the pre-pass (with MSAA its alpha-to-coverage stages, `coverage.rs`) and the
    // shadow maps.
    let depth_only = fs != "fs_impostor" && fs != "fs_impostor_reflect";
    let (compare, write) = match fs {
        "fs_impostor" => (wgpu::CompareFunction::Equal, false),
        _ => (wgpu::CompareFunction::Less, true),
    };
    let bias = if shadow {
        wgpu::DepthBiasState {
            constant: 1,
            slope_scale: 1.5,
            clamp: 0.0,
        }
    } else {
        Default::default()
    };
    let groups: &[Option<&wgpu::BindGroupLayout>] = if shadow {
        &[Some(&l.globals), Some(&l.impostor), Some(&l.cascade)]
    } else {
        &[
            Some(&l.globals),
            Some(&l.impostor),
            Some(&l.draw),
            Some(&l.forward),
        ]
    };
    pipeline(
        device,
        PipelineDesc {
            label: "impostor",
            module: module(modules, "scene"),
            vs: if shadow {
                "vs_impostor_shadow"
            } else {
                "vs_impostor"
            },
            fs: Some(fs),
            groups: if depth_only && !shadow {
                &groups[..3]
            } else {
                groups
            },
            buffers: &[None, instances],
            target: (!depth_only).then(|| HDR.into()),
            depth: Some(depth_state(write, compare, bias)),
            cull: None,
            samples,
            constants: if depth_only && samples > 1 {
                &samples_constant
            } else {
                &[]
            },
            front_cw: false,
        },
    )
}

fn sky_pipeline(
    device: &wgpu::Device,
    l: &Layouts,
    modules: &[wgpu::ShaderModule],
    samples: u32,
) -> wgpu::RenderPipeline {
    pipeline(
        device,
        PipelineDesc {
            label: "sky",
            module: module(modules, "sky"),
            vs: "vs_sky",
            fs: Some("fs_sky"),
            groups: &[Some(&l.globals), None, Some(&l.sky_clouds)],
            buffers: &[],
            target: Some(HDR.into()),
            depth: Some(depth_state(
                false,
                wgpu::CompareFunction::LessEqual,
                Default::default(),
            )),
            cull: None,
            samples,
            constants: &[],
            front_cw: false,
        },
    )
}

/// The rain's streaks (wet.wgsl `fs_rain`): premultiplied alpha over the HDR scene, tested
/// against its depth (each streak's fragments take their layer's depth), nothing written.
fn rain_pipeline(
    device: &wgpu::Device,
    l: &Layouts,
    modules: &[wgpu::ShaderModule],
    samples: u32,
) -> wgpu::RenderPipeline {
    pipeline(
        device,
        PipelineDesc {
            label: "rain",
            module: module(modules, "scene"),
            vs: "vs_rain",
            fs: Some("fs_rain"),
            groups: &[Some(&l.globals), None, None, Some(&l.forward)],
            buffers: &[],
            target: Some(wgpu::ColorTargetState {
                format: HDR,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            }),
            depth: Some(depth_state(
                false,
                wgpu::CompareFunction::LessEqual,
                Default::default(),
            )),
            cull: None,
            samples,
            constants: &[],
            front_cw: false,
        },
    )
}

impl SampledPipelines {
    fn new(
        device: &wgpu::Device,
        l: &Layouts,
        modules: &[wgpu::ShaderModule],
        samples: u32,
        coverage: coverage::Coverage,
    ) -> Self {
        let scene = module(modules, "scene");
        let depth_buffers = vertex_layouts(true);
        let samples_constant = [("SAMPLES", samples as f64)];
        let depth_groups = [Some(&l.globals), Some(&l.material)];
        let depth = std::array::from_fn(|variant| {
            let cutout = variant >= 2;
            pipeline(
                device,
                PipelineDesc {
                    label: "depth pre-pass",
                    module: scene,
                    vs: "vs_depth",
                    fs: cutout.then_some(if samples > 1 {
                        coverage.stage("fs_depth_cutout")
                    } else {
                        "fs_depth_cutout_1x"
                    }),
                    groups: &depth_groups[..1 + cutout as usize],
                    buffers: &depth_buffers,
                    target: None,
                    depth: Some(depth_state(
                        true,
                        wgpu::CompareFunction::Less,
                        Default::default(),
                    )),
                    cull: cull(variant % 2 == 1),
                    samples,
                    constants: if cutout && samples > 1 {
                        &samples_constant
                    } else {
                        &[]
                    },
                    front_cw: false,
                },
            )
        });
        let forward = std::array::from_fn(|variant| {
            // Cutout: the pre-pass already decided coverage; Equal keeps
            // exactly those samples and needs no alpha test here.
            let compare = if variant >= 2 {
                wgpu::CompareFunction::Equal
            } else {
                wgpu::CompareFunction::LessEqual
            };
            forward_pipeline(
                device,
                l,
                modules,
                Forward {
                    label: "forward",
                    fs: "fs_main",
                    variant,
                    target: HDR.into(),
                    depth: depth_state(false, compare, Default::default()),
                    samples,
                    front_cw: false,
                },
            )
        });
        let transparent = std::array::from_fn(|double_sided| {
            forward_pipeline(
                device,
                l,
                modules,
                Forward {
                    label: "transparent",
                    fs: "fs_transparent",
                    variant: double_sided,
                    target: wgpu::ColorTargetState {
                        format: HDR,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    },
                    depth: depth_state(false, wgpu::CompareFunction::LessEqual, Default::default()),
                    samples,
                    front_cw: false,
                },
            )
        });
        let blend = |src_factor, dst_factor| wgpu::ColorTargetState {
            format: HDR,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor,
                    dst_factor,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::ALL,
        };
        let glass = [
            (
                "fs_glass_transmit",
                blend(wgpu::BlendFactor::Zero, wgpu::BlendFactor::Src),
            ),
            (
                "fs_glass_reflect",
                blend(wgpu::BlendFactor::One, wgpu::BlendFactor::One),
            ),
        ]
        .map(|(fs, target)| {
            std::array::from_fn(|double_sided| {
                forward_pipeline(
                    device,
                    l,
                    modules,
                    Forward {
                        label: "glass",
                        fs,
                        variant: double_sided,
                        target: target.clone(),
                        depth: depth_state(
                            false,
                            wgpu::CompareFunction::LessEqual,
                            Default::default(),
                        ),
                        samples,
                        front_cw: false,
                    },
                )
            })
        });
        let multisampled = samples > 1;
        let resolve = pipeline(
            device,
            PipelineDesc {
                label: "depth resolve",
                module: module(modules, "depth"),
                vs: "vs_fullscreen",
                fs: Some(if multisampled {
                    "fs_resolve_ms"
                } else {
                    "fs_resolve_1x"
                }),
                groups: &[
                    Some(&l.globals),
                    Some(if multisampled {
                        &l.depth_ms
                    } else {
                        &l.depth_1x
                    }),
                ],
                buffers: &[],
                target: Some(LINEAR_DEPTH.into()),
                depth: None,
                cull: None,
                samples: 1,
                constants: &[],
                front_cw: false,
            },
        );
        Self {
            depth,
            forward,
            transparent,
            glass,
            terrain: terrain::pipeline_for(device, l, modules, samples, false),
            impostor: [
                impostor_pipeline(
                    device,
                    l,
                    modules,
                    if samples > 1 {
                        coverage.stage("fs_impostor_depth_ms")
                    } else {
                        "fs_impostor_depth"
                    },
                    samples,
                    false,
                ),
                impostor_pipeline(device, l, modules, "fs_impostor", samples, false),
            ],
            depth_fade: std::array::from_fn(|double_sided| {
                pipeline(
                    device,
                    PipelineDesc {
                        label: "depth fade",
                        module: module(modules, "scene"),
                        vs: "vs_depth_fade",
                        fs: Some(if samples > 1 {
                            coverage.stage("fs_depth_fade_ms")
                        } else {
                            "fs_depth_fade"
                        }),
                        groups: &[Some(&l.globals), Some(&l.material), Some(&l.draw)],
                        buffers: &vertex_layouts(true),
                        target: None,
                        depth: Some(depth_state(
                            true,
                            wgpu::CompareFunction::Less,
                            Default::default(),
                        )),
                        cull: cull(double_sided == 1),
                        samples,
                        constants: if samples > 1 { &samples_constant } else { &[] },
                        front_cw: false,
                    },
                )
            }),
            ground: [1, 2]
                .map(|part| ground_pipeline(device, l, modules, "fs_ground", samples, part)),
            sky: sky_pipeline(device, l, modules, samples),
            rain: rain_pipeline(device, l, modules, samples),
            resolve,
            sea: sea::SeaPipelines::new(device, l, modules, samples),
        }
    }
}

impl Pipelines {
    fn new(
        device: &wgpu::Device,
        l: &Layouts,
        modules: &[wgpu::ShaderModule],
        format: wgpu::TextureFormat,
    ) -> Self {
        let buffers = vertex_layouts(true);
        let unorm = format.remove_srgb_suffix();
        let fullscreen = |label,
                          name,
                          fs,
                          groups: &[Option<&wgpu::BindGroupLayout>],
                          target: wgpu::ColorTargetState,
                          constants| {
            pipeline(
                device,
                PipelineDesc {
                    label,
                    module: module(modules, name),
                    vs: "vs_fullscreen",
                    fs: Some(fs),
                    groups,
                    buffers: &[],
                    target: Some(target),
                    depth: None,
                    cull: None,
                    samples: 1,
                    constants,
                    front_cw: false,
                },
            )
        };
        let segment = wgpu::VertexBufferLayout {
            array_stride: 40,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Unorm8x4, 2 => Float32, 3 => Float32x3, 4 => Unorm8x4, 5 => Float32
            ],
        };
        let shadow = |fs: Option<&str>, material: bool| {
            pipeline(
                device,
                PipelineDesc {
                    label: "shadow",
                    module: module(modules, "scene"),
                    vs: "vs_shadow",
                    fs,
                    groups: &[
                        Some(&l.globals),
                        material.then_some(&l.material),
                        Some(&l.cascade),
                    ],
                    buffers: &buffers,
                    target: None,
                    // Slope-scaled bias against acne on surfaces at grazing
                    // sun angles; the normal offset in scene.wgsl does the rest.
                    depth: Some(depth_state(
                        true,
                        wgpu::CompareFunction::Less,
                        wgpu::DepthBiasState {
                            constant: 1,
                            slope_scale: 1.5,
                            clamp: 0.0,
                        },
                    )),
                    // Both sides: CAD shells are not always closed.
                    cull: None,
                    samples: 1,
                    constants: &[],
                    front_cw: false,
                },
            )
        };
        let reflect = std::array::from_fn(|variant| {
            forward_pipeline(
                device,
                l,
                modules,
                Forward {
                    label: "reflection",
                    fs: "fs_reflect",
                    variant,
                    target: HDR.into(),
                    depth: depth_state(true, wgpu::CompareFunction::Less, Default::default()),
                    samples: 1,
                    front_cw: true,
                },
            )
        });
        let post = [Some(&l.globals), Some(&l.post)];
        let air = [Some(&l.globals), Some(&l.shafts)];
        let add = wgpu::ColorTargetState {
            format: HDR,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::REPLACE,
            }),
            write_mask: wgpu::ColorWrites::ALL,
        };
        Self {
            shadow: [
                shadow(None, false),
                shadow(Some("fs_shadow_cutout"), true),
                shadow(Some("fs_shadow_glass"), false),
                shadow(Some("fs_shadow_dither"), false),
            ],
            reflect,
            reflect_ground: ground_pipeline(device, l, modules, "fs_reflect_ground", 1, 0),
            reflect_terrain: terrain::pipeline_for(device, l, modules, 1, true),
            reflect_impostor: impostor_pipeline(
                device,
                l,
                modules,
                "fs_impostor_reflect",
                1,
                false,
            ),
            impostor_shadow: impostor_pipeline(device, l, modules, "fs_impostor_shadow", 1, true),
            reflect_sky: sky_pipeline(device, l, modules, 1),
            ssao: fullscreen(
                "ssao",
                "ssao",
                "fs_main",
                &[Some(&l.globals), Some(&l.texture)],
                OCCLUSION.into(),
                &[],
            ),
            blur_h: fullscreen(
                "blur h",
                "blur",
                "fs_main",
                &[Some(&l.globals), Some(&l.blur)],
                OCCLUSION.into(),
                &[("HORIZONTAL", 1.0)],
            ),
            blur_v: fullscreen(
                "blur v",
                "blur",
                "fs_main",
                &[Some(&l.globals), Some(&l.blur)],
                OCCLUSION.into(),
                &[("HORIZONTAL", 0.0)],
            ),
            bloom: [
                fullscreen(
                    "bloom",
                    "post",
                    "fs_bloom_down",
                    &post,
                    HDR.into(),
                    &[("KARIS", 1.0)],
                ),
                fullscreen("bloom", "post", "fs_bloom_down", &post, HDR.into(), &[]),
                fullscreen("bloom", "post", "fs_bloom_up", &post, add, &[]),
            ],
            tonemap: [
                fullscreen("tonemap", "tonemap", "fs_tonemap", &post, unorm.into(), &[]),
                fullscreen(
                    "tonemap",
                    "tonemap",
                    "fs_tonemap",
                    &post,
                    unorm.into(),
                    &[("LUMA_IN_ALPHA", 1.0)],
                ),
            ],
            fxaa: fullscreen("fxaa", "post", "fs_fxaa", &post, unorm.into(), &[]),
            shafts_march: fullscreen("shafts", "shafts", "fs_march", &air, HDR.into(), &[]),
            shafts_blur: [
                fullscreen(
                    "shafts blur",
                    "shafts",
                    "fs_blur",
                    &air,
                    HDR.into(),
                    &[("HORIZONTAL", 1.0)],
                ),
                fullscreen(
                    "shafts blur",
                    "shafts",
                    "fs_blur",
                    &air,
                    HDR.into(),
                    &[("HORIZONTAL", 0.0)],
                ),
            ],
            sun_seen: fullscreen("sun seen", "shafts", "fs_sun_seen", &air, HDR.into(), &[]),
            air: fullscreen(
                "air",
                "shafts",
                "fs_composite",
                &air,
                wgpu::ColorTargetState {
                    format: HDR,
                    // The image times the air's transmittance (alpha), plus its light.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::SrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                },
                &[],
            ),
            upscale: fullscreen("upscale", "post", "fs_upscale", &post, unorm.into(), &[]),
            underlay: pipeline(
                device,
                PipelineDesc {
                    label: "underlay",
                    module: module(modules, "underlay"),
                    vs: "vs_main",
                    fs: Some("fs_main"),
                    groups: &[Some(&l.globals), Some(&l.texture), Some(&l.underlay)],
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: underlay::VERTEX_BYTES,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3, 1 => Float32x2, 2 => Float32
                        ],
                    })],
                    target: Some(wgpu::ColorTargetState {
                        format: format.add_srgb_suffix(),
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    depth: None,
                    cull: None,
                    samples: 1,
                    constants: &[],
                    front_cw: false,
                },
            ),
            overlay: pipeline(
                device,
                PipelineDesc {
                    label: "overlay",
                    module: module(modules, "overlay"),
                    vs: "vs_main",
                    fs: Some("fs_main"),
                    groups: &[Some(&l.globals), Some(&l.texture)],
                    buffers: &[Some(segment)],
                    target: Some(wgpu::ColorTargetState {
                        format: format.add_srgb_suffix(),
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    depth: None,
                    cull: None,
                    samples: 1,
                    constants: &[],
                    front_cw: false,
                },
            ),
        }
    }
}

impl Targets {
    #[allow(clippy::too_many_arguments)]
    fn new(
        device: &wgpu::Device,
        l: &Layouts,
        format: wgpu::TextureFormat,
        quality: Quality,
        (out_w, out_h): (u32, u32),
        shadow_sampler: &wgpu::Sampler,
        post_sampler: &wgpu::Sampler,
        white: &wgpu::TextureView,
        aerial: &wgpu::TextureView,
        photo: bool,
    ) -> Self {
        let samples = quality.samples();
        let scale = quality.render_scale.clamp(0.5, 1.0);
        let (width, height) = (
            ((out_w as f32 * scale).round() as u32).clamp(1, out_w),
            ((out_h as f32 * scale).round() as u32).clamp(1, out_h),
        );
        let bytes = Cell::new(0u64);
        let texture_sized =
            |label, format: wgpu::TextureFormat, samples, usage, (w, h): (u32, u32)| {
                let block = format.block_copy_size(None).unwrap_or(4) as u64;
                bytes.set(bytes.get() + w as u64 * h as u64 * block * samples as u64);
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
        let texture = |label, format, samples, usage| {
            texture_sized(label, format, samples, usage, (width, height))
        };
        let view = |t: wgpu::Texture| t.create_view(&Default::default());
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let sampled = attach | wgpu::TextureUsages::TEXTURE_BINDING;
        let depth = view(texture("depth", DEPTH, samples, sampled));
        let hdr_msaa = (samples > 1).then(|| view(texture("hdr msaa", HDR, samples, attach)));
        let hdr_texture = texture("hdr", HDR, 1, sampled | wgpu::TextureUsages::COPY_SRC);
        let hdr = view(hdr_texture.clone());
        let prev_hdr = texture_sized(
            "previous hdr",
            HDR,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            // Screen-space reflections read the opaque scene from it too (`Pass::Mirrors`).
            if photo || quality.screen_reflections > 0 {
                (width, height)
            } else {
                (1, 1)
            },
        );
        let prev_view = view(prev_hdr.clone());
        let linear_depth_texture = texture(
            "linear depth",
            LINEAR_DEPTH,
            1,
            sampled | wgpu::TextureUsages::COPY_SRC,
        );
        let linear_depth = view(linear_depth_texture.clone());
        // Half resolution unless the quality asks for full: AO and contact
        // shadows are low-frequency.
        let ao_div = if quality.ao_full_resolution { 1 } else { 2 };
        let half = (width.div_ceil(ao_div), height.div_ceil(ao_div));
        let occlusion =
            [0, 1].map(|_| view(texture_sized("occlusion", OCCLUSION, 1, sampled, half)));
        let unorm = format.remove_srgb_suffix();
        let srgb = format.add_srgb_suffix();
        let ldr = [0, 1].map(|_| view(texture("ldr", unorm, 1, sampled)));
        let bloom: Vec<_> = bloom_sizes((width, height))
            .into_iter()
            .map(|size| view(texture_sized("bloom", HDR, 1, sampled, size)))
            .collect();
        let divisor = quality.water_reflection;
        let reflection_size = width
            .checked_div(divisor)
            .zip(height.checked_div(divisor))
            .map_or((1, 1), |(w, h)| (w.max(1), h.max(1)));
        // A one-layer array standing in for the planar reflection in the reflection passes' own
        // bind group (group 3 binding 4 is an array there too, and is not read).
        let white_array = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("reflection stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        // One layer per planar water pass (a single 1x1 layer while the reflection is off).
        let reflection_layers = quality.plane_budget().max(1);
        let reflection_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reflection"),
            size: wgpu::Extent3d {
                width: reflection_size.0,
                height: reflection_size.1,
                depth_or_array_layers: reflection_layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR,
            usage: sampled,
            view_formats: &[],
        });
        let reflection = reflection_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("reflection array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let reflection_layer_views: Vec<_> = (0..reflection_layers)
            .map(|i| {
                reflection_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("reflection layer"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        bytes.set(
            bytes.get()
                + u64::from(reflection_size.0)
                    * u64::from(reflection_size.1)
                    * 8
                    * u64::from(reflection_layers),
        );
        let reflection_depth = view(texture_sized(
            "reflection depth",
            DEPTH,
            1,
            attach,
            reflection_size,
        ));
        bytes.set(bytes.get() + out_w as u64 * out_h as u64 * 4);
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: wgpu::Extent3d {
                width: out_w,
                height: out_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: sampled | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[srgb, unorm],
        });
        let output_view = output.create_view(&wgpu::TextureViewDescriptor {
            format: Some(srgb),
            ..Default::default()
        });
        let output_unorm = output.create_view(&wgpu::TextureViewDescriptor {
            format: Some(unorm),
            ..Default::default()
        });
        let display_view = output.create_view(&Default::default());

        let res = quality.shadow_resolution;
        bytes.set(bytes.get() + res as u64 * res as u64 * 4 * quality.cascades as u64);
        let shadow = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d {
                width: res,
                height: res,
                // The cascades and the snow cover map.
                depth_or_array_layers: quality.cascades + 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: sampled,
            view_formats: &[],
        });
        let shadow_layers = (0..=quality.cascades)
            .map(|i| {
                shadow.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("shadow layer"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_array = shadow.create_view(&wgpu::TextureViewDescriptor {
            label: Some("shadow array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        let bind = |label, layout, entries: &[(u32, wgpu::BindingResource)]| {
            let entries: Vec<_> = entries
                .iter()
                .map(|(binding, resource)| wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: resource.clone(),
                })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        let tv = wgpu::BindingResource::TextureView;
        let post = |a: &wgpu::TextureView, b: &wgpu::TextureView| {
            bind(
                "post",
                &l.post,
                &[
                    (0, wgpu::BindingResource::TextureView(a)),
                    (1, wgpu::BindingResource::Sampler(post_sampler)),
                    (2, wgpu::BindingResource::TextureView(b)),
                ],
            )
        };
        // The light shafts at half size (rounded up, as the occlusion), the sun's openness.
        let shafts_size = (width.div_ceil(SHAFTS_DIV), height.div_ceil(SHAFTS_DIV));
        let shafts = [0, 1].map(|_| view(texture_sized("shafts", HDR, 1, sampled, shafts_size)));
        let sun_seen = view(texture_sized("sun seen", HDR, 1, sampled, (1, 1)));
        let air = |source: &wgpu::TextureView, seen: &wgpu::TextureView| {
            bind(
                "air",
                &l.shafts,
                &[
                    (0, wgpu::BindingResource::TextureView(&shadow_array)),
                    (1, wgpu::BindingResource::Sampler(shadow_sampler)),
                    (2, wgpu::BindingResource::TextureView(&linear_depth)),
                    (3, wgpu::BindingResource::TextureView(source)),
                    (4, wgpu::BindingResource::TextureView(aerial)),
                    (5, wgpu::BindingResource::Sampler(post_sampler)),
                    (6, wgpu::BindingResource::TextureView(seen)),
                ],
            )
        };
        let air_bind = [
            air(white, white),
            air(&shafts[0], white),
            air(&shafts[1], white),
            air(&shafts[0], &sun_seen),
        ];
        let resolve_bind = if samples > 1 {
            bind("resolve", &l.depth_ms, &[(0, tv(&depth))])
        } else {
            bind("resolve", &l.depth_1x, &[(1, tv(&depth))])
        };
        let forward = |occlusion: &wgpu::TextureView,
                       linear_depth: &wgpu::TextureView,
                       reflection: &wgpu::TextureView,
                       previous: &wgpu::TextureView| {
            bind(
                "forward",
                &l.forward,
                &[
                    (0, wgpu::BindingResource::TextureView(&shadow_array)),
                    (1, wgpu::BindingResource::Sampler(shadow_sampler)),
                    (2, wgpu::BindingResource::TextureView(occlusion)),
                    (3, wgpu::BindingResource::TextureView(linear_depth)),
                    (4, wgpu::BindingResource::TextureView(reflection)),
                    (5, wgpu::BindingResource::TextureView(previous)),
                ],
            )
        };
        Targets {
            size: (width, height),
            ssao_bind: bind("ssao", &l.texture, &[(0, tv(&linear_depth))]),
            blur_bind: [
                bind(
                    "blur h",
                    &l.blur,
                    &[(0, tv(&occlusion[0])), (1, tv(&linear_depth))],
                ),
                bind(
                    "blur v",
                    &l.blur,
                    &[(0, tv(&occlusion[1])), (1, tv(&linear_depth))],
                ),
            ],
            bloom_down_bind: (0..BLOOM_LEVELS)
                .map(|i| post(if i == 0 { &hdr } else { &bloom[i - 1] }, white))
                .collect(),
            bloom_up_bind: (0..BLOOM_LEVELS - 1)
                .map(|i| post(&bloom[i + 1], white))
                .collect(),
            tonemap_bind: post(&hdr, &bloom[0]),
            fxaa_bind: post(&ldr[0], white),
            upscale_bind: [post(&ldr[0], white), post(&ldr[1], white)],
            overlay_bind: bind("overlay", &l.texture, &[(0, tv(&linear_depth))]),
            forward_bind: forward(&occlusion[0], &linear_depth, &reflection, &prev_view),
            reflect_forward_bind: forward(white, white, &white_array, white),
            resolve_bind,
            depth,
            hdr_msaa,
            hdr,
            hdr_texture,
            prev_hdr,
            linear_depth,
            linear_depth_texture,
            occlusion,
            output,
            output_view,
            output_unorm,
            display_view,
            ldr,
            bloom,
            reflection,
            reflection_layers: reflection_layer_views,
            reflection_depth,
            shadow_layers,
            shafts,
            sun_seen,
            air_bind,
            bytes: bytes.get(),
        }
    }
}

/// Adapter and device without a surface, for tests and the offscreen bench.
/// Fails (never falls back silently) when no adapter exists.
pub async fn headless_device() -> Result<(wgpu::Device, wgpu::Queue), String> {
    // One instance at a time: the Vulkan loader crashes (null call in
    // `vkEnumerateInstanceExtensionProperties`) when threads create instances at once.
    static CREATING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // No debug names: naming an object makes the loader walk its list of devices
    // unlocked, which crashes while another thread creates or drops one.
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.flags.remove(wgpu::InstanceFlags::DEBUG);
    let instance = {
        let _one_at_a_time = CREATING.lock().unwrap_or_else(|e| e.into_inner());
        wgpu::Instance::new(descriptor)
    };
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| format!("no wgpu adapter: {e}"))?;
    eprintln!("adapter: {:?}", adapter.get_info());
    // Timestamp queries when the adapter has them (headless only: tools and
    // benches), so `stats()` reports GPU time per pass.
    // Indirect first instance: batched draws go through one indirect buffer per frame
    // (`record`); without it every draw is direct.
    let required_features = adapter.features()
        & (wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::TEXTURE_COMPRESSION_BC
            | wgpu::Features::INDIRECT_FIRST_INSTANCE);
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features,
            ..Default::default()
        })
        .await
        .map_err(|e| format!("no wgpu device: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The in-place room light sends the bytes the full upload puts in the room tile: per row of
    /// the 2*dims[0]-wide texture, the room tile first and the sky tile after.
    #[test]
    fn in_place_room_light_equals_the_room_tile_of_the_full_packing() {
        let dims = [3usize, 2, 2];
        let n: usize = dims.iter().product();
        // Values over the clamp's ends and between, so the clamp and the f16 rounding both count.
        let room: Vec<[f32; 4]> = (0..n)
            .map(|i| {
                let f = i as f32;
                [f * 0.37 - 1.5, f * 5000.1, 0.1 + f / 7.0, 70000.0 - f]
            })
            .collect();
        let sky: Vec<[f32; 4]> = (0..n).map(|i| [i as f32 / 3.0, 0.5, -0.5, 1.0]).collect();
        let full = pack_room_volume(dims[0], &room, &sky);
        let in_place = pack_room_light(&room);
        assert_eq!(in_place.len(), n * 8);
        let (tile, row) = (dims[0] * 8, dims[0] * 16);
        for r in 0..dims[1] * dims[2] {
            assert_eq!(
                &full[r * row..r * row + tile],
                &in_place[r * tile..(r + 1) * tile],
                "row {r}"
            );
        }
    }

    /// `instance_order` says where each given instance lands (sorted by ground cell), and
    /// `set_instance` moves one in place: its bounds and the scene's follow once the frame is
    /// drawn, the other instances stay where they are, and nothing else is made dirty.
    #[test]
    fn an_instance_moves_in_place() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let at = |x: f32, y: f32| Mat4::from_translation(Vec3::new(x, y, 0.0));
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = r
            .add_mesh(&Mesh {
                positions: &p,
                normals: &[[0.0, 0.0, 1.0]; 3],
                indices: &[0, 1, 2],
                ..Default::default()
            })
            .unwrap();
        let mat = r.add_material(&Material::default());
        // Three cells apart, given from east to west: sorted west to east.
        let given = [at(16.0, 0.0), at(0.0, 0.0), at(8.0, 0.0)];
        let item = r.add_item(mesh, mat, &given);
        let order = Renderer::instance_order(&given);
        assert_eq!(order, vec![2, 0, 1], "the first given lands last");
        fn of(r: &Renderer, id: ItemId) -> &Item {
            r.items[id.0 as usize].as_ref().unwrap()
        }
        assert_eq!(
            of(&r, item)
                .transforms
                .iter()
                .map(|m| m.w_axis.x)
                .collect::<Vec<_>>(),
            vec![0.0, 8.0, 16.0]
        );
        r.update_bounds();
        assert!(!r.bounds_dirty);
        let b = of(&r, item).bounds.unwrap();
        assert_eq!((b.0.x, b.1.x, b.1.y), (0.0, 17.0, 1.0));
        // The first given instance, the one furthest east, jumps up and away.
        let m = at(100.0, 50.0);
        r.set_instance(item, order[0] as usize, m);
        assert_eq!(of(&r, item).transforms[order[0] as usize], m);
        assert_eq!(of(&r, item).transforms[order[1] as usize], given[1]);
        assert_eq!(of(&r, item).transforms[order[2] as usize], given[2]);
        assert!(!r.bounds_dirty, "nothing else is made again");
        // Its bounds and the scene's are made again when the frame is drawn.
        r.update_moved();
        let b = of(&r, item).bounds.unwrap();
        assert_eq!((b.0.x, b.1.x, b.1.y), (0.0, 101.0, 51.0));
        assert_eq!(r.scene_bounds.unwrap().1.x, 101.0);
        assert_eq!(
            of(&r, item).clusters.len(),
            3,
            "still one instance per cell"
        );
        // Out of range, or a removed item: nothing happens.
        r.set_instance(item, 9, m);
        assert_eq!(of(&r, item).transforms.len(), 3);
        r.set_instance(ItemId(item.0 + 1), 0, m);
        r.set_instance(item, order[0] as usize, given[0]);
        r.update_moved();
        let b = of(&r, item).bounds.unwrap();
        assert_eq!((b.1.x, b.1.y), (17.0, 1.0), "and back");
    }

    /// Posing moves a few instances every frame: however many move, the shadow caches are
    /// thrown away once, when the frame is drawn, not once per instance.
    #[test]
    fn moved_instances_invalidate_the_shadows_once_a_frame() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(64, 36);
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = r
            .add_mesh(&Mesh {
                positions: &p,
                normals: &[[0.0, 0.0, 1.0]; 3],
                indices: &[0, 1, 2],
                ..Default::default()
            })
            .unwrap();
        let mat = r.add_material(&Material::default());
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let item = r.add_item(mesh, mat, &[at(0.0), at(8.0)]);
        let frame = |r: &mut Renderer| {
            let mut e = device.create_command_encoder(&Default::default());
            r.render(&mut e);
            queue.submit([e.finish()]);
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
        };
        frame(&mut r);
        let before = r.shadow_invalidations;
        r.set_instance(item, 0, at(1.0));
        r.set_instance(item, 1, at(9.0));
        frame(&mut r);
        assert_eq!(r.shadow_invalidations - before, 1, "once for the frame");
        assert_eq!(
            r.scene_bounds.unwrap().1.x,
            10.0,
            "the scene's bounds follow"
        );
        // A frame in which nothing moved keeps them.
        let before = r.shadow_invalidations;
        frame(&mut r);
        assert_eq!(r.shadow_invalidations, before);
    }

    /// A still taken right after instances moved: the window's own shadow caches (swapped out
    /// for the still) are thrown away too, not only the still's.
    #[test]
    fn a_still_after_moved_instances_invalidates_the_window_s_shadows() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(64, 36);
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = r
            .add_mesh(&Mesh {
                positions: &p,
                normals: &[[0.0, 0.0, 1.0]; 3],
                indices: &[0, 1, 2],
                ..Default::default()
            })
            .unwrap();
        let mat = r.add_material(&Material::default());
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let item = r.add_item(mesh, mat, &[at(0.0), at(8.0)]);
        let mut e = device.create_command_encoder(&Default::default());
        r.render(&mut e);
        queue.submit([e.finish()]);
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        assert!(
            r.cascade_rendered.iter().any(Option::is_some),
            "a frame caches its cascades"
        );
        r.set_instance(item, 0, at(1.0));
        let (view, proj, eye) = (r.view, r.proj, r.eye);
        r.render_still(32, 18, view, proj, eye).expect("a still");
        assert!(
            r.cascade_rendered.iter().all(Option::is_none) && r.far_cache.is_none(),
            "the window's cascades no longer cover the scene"
        );
    }

    /// `instance_order` is the order the item keeps its instances in: instance `i` as given is
    /// instance `order[i]` once kept, those of one cell in the order they were given.
    #[test]
    fn instance_order_is_the_order_the_item_keeps() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = r
            .add_mesh(&Mesh {
                positions: &p,
                normals: &[[0.0, 0.0, 1.0]; 3],
                indices: &[0, 1, 2],
                ..Default::default()
            })
            .unwrap();
        let mat = r.add_material(&Material::default());
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        // Given east to west, two to a cell and one west of them all.
        let given = [at(8.5), at(0.5), at(8.0), at(0.0), at(-8.0)];
        let order = Renderer::instance_order(&given);
        assert_eq!(order, [3, 1, 4, 2, 0], "west to east, ties as given");
        let item = r.add_item(mesh, mat, &given);
        let kept = &r.items[item.0 as usize].as_ref().unwrap().transforms;
        for (i, &pos) in order.iter().enumerate() {
            assert_eq!(
                kept[pos as usize], given[i],
                "instance {i} as given is kept at {pos}"
            );
        }
    }

    /// Every 128 m block holds the bounds of its clusters, also once an instance moved: a moved
    /// instance stays in its cluster and so in its block, and the block's bounds follow it.
    /// `update_lods` steps over the clusters of a block past the item's last level when the
    /// block is culled, so a block that kept the bounds of before the move would step over
    /// clusters that should draw.
    #[test]
    fn the_blocks_follow_a_moved_instance() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = r
            .add_mesh(&Mesh {
                positions: &p,
                normals: &[[0.0, 0.0, 1.0]; 3],
                indices: &[0, 1, 2],
                ..Default::default()
            })
            .unwrap();
        let mat = r.add_material(&Material::default());
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        // Two cells 300 m apart (two blocks), given east first.
        let given = [at(300.0), at(0.0)];
        let order = Renderer::instance_order(&given);
        let item = r.add_item(mesh, mat, &given);
        r.update_bounds();
        let blocks_hold = |r: &Renderer| {
            let item = r.items[item.0 as usize].as_ref().unwrap();
            assert_eq!(item.block_of.len(), item.clusters.len());
            for (b, block) in item.blocks.iter().enumerate() {
                let clusters = item
                    .clusters
                    .iter()
                    .zip(&item.block_of)
                    .filter(|(_, in_b)| **in_b == b as u32)
                    .filter_map(|(c, _)| c.bounds)
                    .reduce(union);
                assert_eq!(block.bounds, clusters, "block {b}");
            }
        };
        blocks_hold(&r);
        let x_of = |r: &Renderer, cluster: usize| {
            let item = r.items[item.0 as usize].as_ref().unwrap();
            item.blocks[item.block_of[cluster] as usize].bounds.unwrap()
        };
        let (lo, hi) = (x_of(&r, 0).0.x, x_of(&r, 1).0.x);
        assert_eq!((lo, hi), (0.0, 300.0), "a block a cluster, where they are");
        // The western instance jumps 300 m further west.
        r.set_instance(item, order[1] as usize, at(-300.0));
        r.update_moved();
        blocks_hold(&r);
        let item = r.items[item.0 as usize].as_ref().unwrap();
        assert_eq!(
            (item.blocks.len(), item.clusters.len()),
            (2, 2),
            "a move keeps the clusters and the blocks"
        );
        let b = item.bounds.unwrap();
        assert_eq!((b.0.x, b.1.x), (-300.0, 301.0));
        assert_eq!(r.scene_bounds.unwrap().0.x, -300.0);
        let (lo, hi) = (x_of(&r, 0).0.x, x_of(&r, 1).0.x);
        assert_eq!(
            (lo, hi),
            (-300.0, 300.0),
            "its block's bounds are where it is now, not where it was"
        );
    }

    /// Each moved item is listed once until the frame takes the list, however its moves
    /// interleave with other items' (a host that prepares without drawing keeps it short).
    #[test]
    fn a_moved_item_is_listed_once() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = r
            .add_mesh(&Mesh {
                positions: &p,
                normals: &[[0.0, 0.0, 1.0]; 3],
                indices: &[0, 1, 2],
                ..Default::default()
            })
            .unwrap();
        let mat = r.add_material(&Material::default());
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let (a, b) = (
            r.add_item(mesh, mat, &[at(0.0)]),
            r.add_item(mesh, mat, &[at(8.0)]),
        );
        for k in 0..50 {
            r.set_instance(a, 0, at(k as f32));
            r.set_instance(b, 0, at(8.0 + k as f32));
        }
        assert_eq!(r.moved.len(), 2);
        r.update_moved();
        assert!(r.moved.is_empty());
        r.set_instance(a, 0, at(0.0));
        assert_eq!(r.moved.len(), 1, "listed again after the frame");
    }

    /// `set_sky_volume_light` needs the volume it changes the light of.
    #[test]
    fn set_sky_volume_light_needs_a_volume_of_the_same_size() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let room = vec![[1.0, 2.0, 3.0, 4.0]; 12];
        assert!(!r.set_sky_volume_light(&room), "no volume yet");
        let sky = vec![[1.0, 0.0, 0.0, 0.0]; 12];
        let solid = vec![0u8; 12];
        r.set_sky_volume(Some((
            [0.0; 3],
            1.0,
            [3, 2, 2],
            &sky,
            &room,
            (&solid, [3, 2, 2]),
        )));
        assert!(r.set_sky_volume_light(&room));
        assert!(!r.set_sky_volume_light(&room[..11]), "another size");
        r.set_sky_volume(None);
        assert!(!r.set_sky_volume_light(&room[..1]), "volume removed");
    }

    /// Cascade `c` of `count` (the last one cached when `cached`), the camera inside the far
    /// cascade's margin, everything drawn once and stale when `stale`.
    fn refresh(
        c: usize,
        count: usize,
        cached: bool,
        frame: u64,
        moving: bool,
        stale: bool,
    ) -> bool {
        cascade_refresh(
            c,
            count,
            cached && c == count - 1,
            true,
            true,
            frame,
            moving,
            stale,
            true,
        )
    }

    #[test]
    fn a_moving_sun_redraws_the_near_cascade_every_frame() {
        for f in 1..=16 {
            assert!(refresh(0, 4, true, f, true, true));
        }
    }

    #[test]
    fn a_moving_sun_redraws_the_far_cascade_once_in_four_frames() {
        let n = (1..=16)
            .filter(|&f| refresh(3, 4, true, f, true, true))
            .count();
        assert_eq!(n, 4);
        // With two cascades the second is the far cached one: the same rule.
        let n = (1..=16)
            .filter(|&f| refresh(1, 2, true, f, true, true))
            .count();
        assert_eq!(n, 4);
    }

    #[test]
    fn a_moving_sun_alternates_the_middle_cascades_instead_of_forcing_them_together() {
        for c in [1, 2] {
            let n = (1..=16)
                .filter(|&f| refresh(c, 4, true, f, true, true))
                .count();
            assert_eq!(n, 8);
        }
        assert!(
            (1..=16)
                .all(|f| refresh(1, 4, true, f, true, true) != refresh(2, 4, true, f, true, true))
        );
    }

    #[test]
    fn a_sun_that_stopped_redraws_every_stale_cascade_at_once() {
        for f in 1..=4 {
            for c in 0..4 {
                assert!(refresh(c, 4, true, f, false, true), "cascade {c} frame {f}");
            }
        }
    }

    #[test]
    fn a_settled_sun_keeps_the_cached_far_cascade_and_the_alternating_middle_ones() {
        assert!((1..=16).all(|f| !refresh(3, 4, true, f, false, false)));
        assert!(refresh(0, 4, true, 3, false, false));
        let n = (1..=16)
            .filter(|&f| refresh(1, 4, true, f, false, false))
            .count();
        assert_eq!(n, 8);
        // Without a cached far cascade the last one draws every frame.
        assert!((1..=16).all(|f| refresh(1, 2, false, f, false, false)));
    }

    #[test]
    fn a_scene_change_redraws_everything() {
        for c in 0..4 {
            // Nothing drawn yet, no cached far cascade to reuse.
            assert!(cascade_refresh(
                c,
                4,
                c == 3,
                false,
                false,
                5,
                true,
                false,
                true
            ));
            assert!(cascade_refresh(
                c,
                4,
                c == 3,
                false,
                false,
                6,
                false,
                false,
                true
            ));
        }
        // The camera left the far cascade's margin.
        assert!(cascade_refresh(
            3, 4, true, false, true, 5, true, false, true
        ));
    }

    #[test]
    fn a_stale_far_refresh_inside_the_margin_takes_the_fresh_matrix_into_the_cache() {
        let (old, fresh) = (
            Mat4::from_scale(Vec3::splat(2.0)),
            Mat4::from_scale(Vec3::splat(3.0)),
        );
        // Within the margin (reuse), stale, sun stopped: it draws, so the fresh matrix.
        let draws = cascade_refresh(3, 4, true, true, true, 5, false, true, true);
        assert!(draws);
        assert_eq!(far_matrix(draws, fresh, Some(old)), (fresh, true));
        // A frame that does not draw keeps the cached matrix and the cache.
        let draws = cascade_refresh(3, 4, true, true, true, 5, true, true, true);
        assert!(!draws);
        assert_eq!(far_matrix(draws, fresh, Some(old)), (old, false));
        // Nothing cached yet: the fresh one is the only choice.
        assert_eq!(far_matrix(false, fresh, None), (fresh, false));
    }

    #[test]
    fn no_sun_draws_no_shadows() {
        for c in 0..4 {
            for (moving, stale, rendered) in [
                (true, true, true),
                (false, true, false),
                (false, false, false),
            ] {
                assert!(!cascade_refresh(
                    c,
                    4,
                    c == 3,
                    false,
                    rendered,
                    4,
                    moving,
                    stale,
                    false
                ));
            }
        }
    }

    /// The binary operators of `src` (comments stripped) that WGSL forbids mixing without
    /// parentheses, as (line, text) pairs where a bitwise one (`&`, `|`, `^`) shares a level with
    /// an arithmetic one or another bitwise one. naga accepts the mix; Chrome's compiler (Tint)
    /// rejects the whole module, which threw away every frame of the grass compute pass.
    fn mixed_bitwise(src: &str) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for (n, raw) in src.lines().enumerate() {
            let line = raw.split("//").next().unwrap_or("");
            let b = line.as_bytes();
            // Per open bracket: the operators seen since it opened or since the last `,`.
            let mut levels: Vec<Vec<u8>> = vec![Vec::new()];
            let mut prev_operand = false;
            let mut i = 0;
            while i < b.len() {
                let c = b[i];
                if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
                {
                    // A number, exponent sign included (1e-3).
                    i += 1;
                    while i < b.len()
                        && (b[i].is_ascii_alphanumeric()
                            || b[i] == b'.'
                            || ((b[i] == b'-' || b[i] == b'+') && matches!(b[i - 1], b'e' | b'E')))
                    {
                        i += 1;
                    }
                    prev_operand = true;
                    continue;
                }
                match c {
                    b'(' | b'[' => {
                        levels.push(Vec::new());
                        prev_operand = false;
                    }
                    b')' | b']' => {
                        if levels.len() > 1 {
                            levels.pop();
                        }
                        prev_operand = true;
                    }
                    b',' | b';' | b'{' | b'}' | b'=' => {
                        levels.last_mut().unwrap().clear();
                        prev_operand = false;
                    }
                    b'&' | b'|' if b.get(i + 1) == Some(&c) => {
                        levels.last_mut().unwrap().clear();
                        prev_operand = false;
                        i += 1;
                    }
                    b'&' | b'|' | b'^' | b'*' | b'/' | b'%' | b'+' | b'-' if prev_operand => {
                        if b.get(i + 1) == Some(&b'=') {
                            levels.last_mut().unwrap().clear();
                            i += 1;
                        } else {
                            let ops = levels.last_mut().unwrap();
                            let bitwise = |o: u8| matches!(o, b'&' | b'|' | b'^');
                            if ops.iter().any(|&o| o != c && (bitwise(o) || bitwise(c))) {
                                out.push((n + 1, raw.trim().to_string()));
                            }
                            ops.push(c);
                        }
                        prev_operand = false;
                    }
                    b'<' | b'>' if b.get(i + 1) == Some(&c) && prev_operand => {
                        // A shift counts as one more kind of operator for the mix rule.
                        let ops = levels.last_mut().unwrap();
                        if !ops.is_empty() && !ops.iter().all(|&o| o == c) {
                            out.push((n + 1, raw.trim().to_string()));
                        }
                        ops.push(c);
                        prev_operand = false;
                        i += 1;
                    }
                    b'<' | b'>' | b'!' => {
                        // Comparisons and generics end an operand chain without mixing.
                        levels.last_mut().unwrap().clear();
                        prev_operand = c == b'>';
                    }
                    c if c.is_ascii_alphanumeric() || c == b'_' => prev_operand = true,
                    c if c.is_ascii_whitespace() => {}
                    _ => prev_operand = false,
                }
                i += 1;
            }
        }
        out.dedup();
        out
    }

    #[test]
    fn no_shader_mixes_bitwise_operators_without_parentheses() {
        assert_eq!(
            mixed_bitwise("let h = a * 7u ^ b;").len(),
            1,
            "the scan finds the mix"
        );
        assert!(mixed_bitwise("let h = (a * 7u) ^ b ^ c; let x = 1e-3 * y - z;").is_empty());
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("src") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|e| e == "wgsl") {
                let src = std::fs::read_to_string(&path).expect("reads");
                for (line, text) in mixed_bitwise(&src) {
                    found.push(format!("{}:{line}: {text}", path.display()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "WGSL needs parentheses here (Chrome rejects the module):\n{}",
            found.join("\n")
        );
    }

    type ShaderEntry = (&'static str, &'static [&'static str]);

    /// Every module of `SHADERS`, with fast shading and without, and the scene module also in
    /// its Vulkan form (`coverage.rs`).
    fn shader_variants() -> Vec<(&'static ShaderEntry, bool, coverage::Coverage)> {
        let mut all = Vec::new();
        for shader in &SHADERS {
            for fast in [false, true] {
                all.push((shader, fast, coverage::Coverage::Masked));
                if shader.0 == "scene" {
                    all.push((shader, fast, coverage::Coverage::PerSample));
                }
            }
        }
        all
    }

    /// Whether SPIR-V `words` decorate anything `BuiltIn SampleMask`.
    fn writes_sample_mask(words: &[u32]) -> bool {
        const OP_DECORATE: u32 = 71;
        const OP_MEMBER_DECORATE: u32 = 72;
        const BUILT_IN: u32 = 11;
        const SAMPLE_MASK: u32 = 20;
        let mut at = 5; // the header
        while at < words.len() {
            let (count, op) = ((words[at] >> 16) as usize, words[at] & 0xffff);
            let count = count.max(1);
            let operands = &words[at + 1..(at + count).min(words.len())];
            let decoration = match op {
                OP_DECORATE if !operands.is_empty() => &operands[1..],
                OP_MEMBER_DECORATE if operands.len() >= 2 => &operands[2..],
                _ => &[][..],
            };
            if decoration == [BUILT_IN, SAMPLE_MASK] {
                return true;
            }
            at += count;
        }
        false
    }

    /// No fragment stage a Vulkan pipeline takes writes `@builtin(sample_mask)`: naga 30 writes
    /// it as a scalar, which Vulkan forbids (VUID-SampleMask-SampleMask-04359) and Intel's and
    /// AMD's drivers on Windows crash on (`coverage.rs`). Every fragment entry point of every
    /// module is written with naga's SPIR-V back end as wgpu does on Vulkan (overrides set, the
    /// entry point alone); only the three masked stages carry a sample mask, and on Vulkan each
    /// is replaced by a stage that does not.
    #[test]
    fn no_vulkan_stage_writes_a_sample_mask() {
        let mut masked = std::collections::BTreeSet::new();
        let mut written = std::collections::BTreeSet::new();
        for ((name, parts), fast, coverage) in shader_variants() {
            let source = module_source(name, parts, fast, coverage);
            let module = naga::front::wgsl::parse_str(&source).expect("parses");
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect("validates");
            let mut constants = naga::back::PipelineConstants::default();
            if *name == "scene" {
                constants.insert("SAMPLES".into(), MSAA_SAMPLES as f64);
            }
            for ep in &module.entry_points {
                if ep.stage != naga::ShaderStage::Fragment {
                    continue;
                }
                let (module, info) = naga::back::pipeline_constants::process_overrides(
                    &module,
                    &info,
                    Some((ep.stage, &ep.name)),
                    &constants,
                )
                .unwrap_or_else(|e| panic!("{name} {}: {e}", ep.name));
                let words = naga::back::spv::write_vec(
                    &module,
                    &info,
                    &naga::back::spv::Options::default(),
                    Some(&naga::back::spv::PipelineOptions {
                        shader_stage: ep.stage,
                        entry_point: ep.name.clone(),
                    }),
                )
                .unwrap_or_else(|e| panic!("{name} {}: {e}", ep.name));
                written.insert(ep.name.clone());
                if writes_sample_mask(&words) {
                    masked.insert(ep.name.clone());
                }
            }
        }
        let expected: std::collections::BTreeSet<String> = coverage::STAGES
            .iter()
            .map(|(m, _)| m.to_string())
            .collect();
        assert_eq!(
            masked, expected,
            "the stages with a sample mask are exactly the ones coverage.rs replaces on Vulkan"
        );
        for (stage, _) in coverage::STAGES {
            let vulkan = coverage::Coverage::PerSample.stage(stage);
            assert!(
                written.contains(vulkan),
                "{vulkan} is in the Vulkan scene module"
            );
            assert!(!masked.contains(vulkan), "{vulkan} writes no sample mask");
        }
    }

    /// The functions `block` calls, and the ones those call, into `out`.
    fn calls(
        module: &naga::Module,
        block: &naga::Block,
        out: &mut Vec<naga::Handle<naga::Function>>,
    ) {
        for statement in block.iter() {
            match statement {
                naga::Statement::Call { function, .. } => {
                    if !out.contains(function) {
                        out.push(*function);
                        calls(module, &module.functions[*function].body, out);
                    }
                }
                naga::Statement::Block(b) => calls(module, b, out),
                naga::Statement::If { accept, reject, .. } => {
                    calls(module, accept, out);
                    calls(module, reject, out);
                }
                naga::Statement::Switch { cases, .. } => {
                    for case in cases {
                        calls(module, &case.body, out);
                    }
                }
                naga::Statement::Loop {
                    body, continuing, ..
                } => {
                    calls(module, body, out);
                    calls(module, continuing, out);
                }
                _ => {}
            }
        }
    }

    /// Every fragment stage that lights a surface (calls `light` or `sun_shadow`, directly or
    /// not) also calls `scratch_floor` (common.wgsl: no lit shader may spill exactly 1 KB per
    /// thread on Mesa's Intel driver), and the floor's array is too big for registers.
    #[test]
    fn every_lit_fragment_stage_has_the_scratch_floor() {
        assert!(
            COMMON.contains(SCRATCH_FLOOR_LINE),
            "the floor's array: just over 256 bytes (Mesa's limit for arrays in registers), over 2 KB at SIMD8"
        );
        let mut lit = std::collections::BTreeSet::new();
        let modules = shader_variants()
            .into_iter()
            .map(|((name, parts), fast, coverage)| {
                (*name, module_source(name, parts, fast, coverage))
            })
            .chain([false, true].map(|fast| ("grass", grass::draw_source(fast))));
        for (name, source) in modules {
            let module = naga::front::wgsl::parse_str(&source).expect("parses");
            let named = |n: &str| {
                module
                    .functions
                    .iter()
                    .find(|(_, f)| f.name.as_deref() == Some(n))
                    .map(|(h, _)| h)
            };
            let lights = [named("light"), named("sun_shadow")];
            let floor = named("scratch_floor");
            for ep in &module.entry_points {
                if ep.stage != naga::ShaderStage::Fragment {
                    continue;
                }
                let mut reached = Vec::new();
                calls(&module, &ep.function.body, &mut reached);
                if !lights.iter().flatten().any(|h| reached.contains(h)) {
                    continue;
                }
                lit.insert(ep.name.clone());
                assert!(
                    floor.is_some_and(|f| reached.contains(&f)),
                    "{name} {} lights a surface without `scratch_floor`",
                    ep.name
                );
            }
        }
        for stage in [
            "fs_main",
            "fs_ground",
            "fs_impostor",
            "fs_terrain",
            "fs_transparent",
            "fs_grass",
        ] {
            assert!(lit.contains(stage), "{stage} is checked");
        }
    }

    /// The scratch floor is for Mesa's Intel Vulkan driver and for browsers, which hide theirs.
    #[test]
    fn the_scratch_floor_is_for_mesa_on_intel_and_browsers() {
        let info = |backend, vendor, driver: &str, driver_info: &str| wgpu::AdapterInfo {
            name: String::new(),
            vendor,
            device: 0,
            device_type: wgpu::DeviceType::IntegratedGpu,
            device_pci_bus_id: String::new(),
            driver: driver.into(),
            driver_info: driver_info.into(),
            backend,
            subgroup_min_size: 8,
            subgroup_max_size: 32,
            transient_saves_memory: None,
            limit_bucket: None,
        };
        let vk = wgpu::Backend::Vulkan;
        let mesa_intel = info(vk, 0x8086, "Intel open-source Mesa driver", "Mesa 26.2.6");
        assert!(needs_scratch_floor(&mesa_intel));
        assert!(needs_scratch_floor(&info(
            wgpu::Backend::BrowserWebGpu,
            0,
            "",
            ""
        )));
        let named = |name: &str| {
            let mut browser = info(wgpu::Backend::BrowserWebGpu, 0, "", "");
            browser.name = name.into();
            needs_scratch_floor(&browser)
        };
        assert!(named("Intel(R) Iris(R) Xe Graphics"));
        assert!(named("some future gpu"));
        assert!(!named("NVIDIA GeForce RTX 4090"));
        assert!(!named("AMD Radeon RX 7900"));
        assert!(!named("Apple M3"));
        assert!(!needs_scratch_floor(&info(
            vk,
            0x8086,
            "Intel Corporation",
            "101.6130"
        )));
        assert!(!needs_scratch_floor(&info(
            vk,
            0x1002,
            "radv",
            "Mesa 26.2.6"
        )));
        assert!(!needs_scratch_floor(&info(
            wgpu::Backend::Dx12,
            0x8086,
            "",
            ""
        )));
    }

    /// Every WGSL module parses and validates with naga, without a GPU.
    #[test]
    fn height_takes_the_occlusion_slot_only_without_an_occlusion_map() {
        let all = [true; 6];
        assert_eq!(texture_bits(all, false), 63);
        assert_eq!(texture_bits(all, true), 31 | 64);
        assert_eq!(
            texture_bits([true, true, true, false, false, false], true),
            7
        );
    }

    #[test]
    fn shaders_validate() {
        assert!(COMMON.contains(FAST_SHADING_LINE));
        for ((name, parts), fast, coverage) in shader_variants() {
            let source = module_source(name, parts, fast, coverage);
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{name}.wgsl: {}", e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}.wgsl: {}", e.emit_to_string(&source)));
        }
    }

    /// The scene module's functions that take a screen derivative or sample a texture by implicit
    /// derivatives. Each does so in uniform control flow, at the top of its fragment stage and
    /// before any branch on the material: a material's values are not uniform (the material table:
    /// a batch spans materials), and Chrome's shader compiler rejects a derivative taken after a
    /// branch on one ("must only be called from uniform control flow") where naga accepts it, so
    /// the gates on the build server cannot see it. A function not listed here takes none.
    #[test]
    fn derivatives_are_taken_only_where_the_flow_is_uniform() {
        let allowed: std::collections::BTreeSet<&str> = [
            "alpha_samples",
            "cutout_alpha",
            "depth_fade_mask",
            "derivs",
            "edge_fade",
            "fs_bake",
            "fs_glass_transmit",
            "fs_sea",
            "impostor_alpha",
            "impostor_sample",
            "terrain_surface",
        ]
        .into_iter()
        .collect();
        let takes_derivatives = |function: &naga::Function| {
            function.expressions.iter().any(|(_, e)| {
                matches!(e, naga::Expression::Derivative { .. })
                    || matches!(
                        e,
                        naga::Expression::ImageSample {
                            level: naga::SampleLevel::Auto,
                            ..
                        }
                    )
            })
        };
        let (_, parts) = SHADERS.iter().find(|(name, _)| *name == "scene").unwrap();
        for fast in [false, true] {
            let source = module_source("scene", parts, fast, coverage::Coverage::Masked);
            let module = naga::front::wgsl::parse_str(&source).expect("parses");
            let mut names: std::collections::BTreeSet<&str> = module
                .functions
                .iter()
                .filter(|&(_, f)| takes_derivatives(f))
                .filter_map(|(_, f)| f.name.as_deref())
                .collect();
            names.extend(
                module
                    .entry_points
                    .iter()
                    .filter(|ep| takes_derivatives(&ep.function))
                    .map(|ep| ep.name.as_str()),
            );
            assert_eq!(names, allowed, "fast shading {fast}");
        }
    }

    #[test]
    fn photo_shader_validates() {
        let source = include_str!("photo.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("photo.wgsl: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("photo.wgsl: {}", e.emit_to_string(source)));
    }

    /// Each entry point also validates as wgpu builds its pipelines: compacted to what the entry
    /// point reaches, the overrides replaced by their values (the defaults here).
    #[test]
    fn entry_points_validate_with_their_overrides() {
        for ((name, parts), fast, coverage) in shader_variants() {
            let source = module_source(name, parts, fast, coverage);
            let module = naga::front::wgsl::parse_str(&source).expect("parses");
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect("validates");
            for ep in &module.entry_points {
                let constants = Default::default();
                if let Err(e) = naga::back::pipeline_constants::process_overrides(
                    &module,
                    &info,
                    Some((ep.stage, &ep.name)),
                    &constants,
                ) {
                    let detail = match &e {
                        naga::back::pipeline_constants::PipelineConstantError::ValidationError(
                            v,
                        ) => v.emit_to_string(&source),
                        other => other.to_string(),
                    };
                    panic!("{name} {} (fast {fast}): {detail}", ep.name);
                }
            }
        }
    }

    /// Materials with the same maps share a texture set; a set no material uses is freed, and
    /// its index used again; the set without maps (index 0) never is.
    #[test]
    fn texture_sets_are_shared_and_freed() {
        let mut sets = TextureSets::default();
        let (a, new) = sets.acquire([None; 6]);
        assert!(new && a == 0);
        assert_eq!(
            sets.acquire([None; 6]).0,
            0,
            "untextured materials share set 0"
        );
        let tex = TextureId(3);
        let maps = [Some(tex), None, None, None, None, None];
        let (b, new) = sets.acquire(maps);
        assert!(new && b == 1);
        assert_eq!(sets.acquire(maps), (1, false));
        assert!(!sets.release(maps, 1), "one user is left");
        assert!(sets.release(maps, 1), "the last user frees it");
        let other = [None, None, None, None, None, Some(tex)];
        assert_eq!(sets.acquire(other), (1, true), "the freed index again");
        assert!(!sets.release([None; 6], 0));
        assert!(!sets.release([None; 6], 0), "set 0 is never given back");
        assert_eq!(sets.acquire([None; 6]), (0, false), "set 0 stays");
    }

    /// What the GPU's draw-slot buffer holds once the queue's writes are in (the test's copy
    /// of it, read back): the first 16 bytes of every slot of `SLOT` bytes.
    fn slots(r: &Renderer, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<[u8; 16]> {
        let mut encoder = device.create_command_encoder(&Default::default());
        let size = SLOT * r.draw_capacity as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&r.draw_buffer, 0, &buffer, 0, size);
        queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |res| res.expect("map"));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let bytes = buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped")
            .to_vec();
        let (slots, rest) = bytes.as_chunks::<{ SLOT as usize }>();
        assert!(rest.is_empty(), "a whole number of slots");
        slots
            .iter()
            .map(|s| s[..16].try_into().expect("a slot"))
            .collect()
    }

    /// A draw slot's mesh id and material row (`mesh_slot`).
    fn slot_words(bytes: [u8; 16]) -> (u32, u32) {
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("word"));
        (word(0), word(4))
    }

    /// One box on the ground, as a mesh (`tests`).
    fn box_mesh(r: &mut Renderer) -> MeshId {
        let mut m = demo::MeshData::default();
        m.add_box(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
        r.add_mesh(&m.mesh()).expect("mesh")
    }

    /// Draw slots in use (the ground's and the batches' included).
    fn live_slots(r: &Renderer) -> usize {
        r.next_slot as usize - r.free_slots.len()
    }

    /// The ground's draw slot in a renderer read back: its mesh id and material row.
    fn ground_words(r: &Renderer, device: &wgpu::Device, queue: &wgpu::Queue) -> (u32, u32) {
        slot_words(slots(r, device, queue)[r.ground_slot as usize])
    }

    /// The endless ground with a look (`set_ground_material`) names the look's row in its
    /// draw slot, a row whose bits say the maps are there, so the ground is shaded with the
    /// material's parameters under its texture set and the terrain's edge vanishes; without a
    /// look, and once the look is removed, the slot names the ground's own row; a draw-slot
    /// buffer that regrew writes whichever holds.
    #[test]
    fn the_ground_with_a_look_material_names_its_row() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.set_ground(Some(Ground::default()));
        let grass = r
            .add_texture(&TextureDesc {
                width: 16,
                height: 16,
                rgba8: &[96; 16 * 16 * 4],
                color_space: ColorSpace::Srgb,
                repeat: true,
            })
            .expect("texture");
        let lawn = r.add_material(&Material {
            base_color_map: Some(grass),
            projection: Projection::Triplanar { scale_m: 2.0 },
            variation: 0.5,
            ..Default::default()
        });
        assert_eq!(
            ground_words(&r, &device, &queue),
            (u32::MAX - 1, GROUND_ROW),
            "no look: the ground's own row"
        );
        r.set_ground_material(Some(lawn));
        assert_eq!(
            ground_words(&r, &device, &queue),
            (u32::MAX - 1, row(lawn)),
            "the slot names the look's row"
        );
        // The look's row (the CPU copy, what was written): its texture bits (scene.wgsl
        // `Material.flags.x`) say the base map is bound, where the ground's own say none.
        let bits = |of: u32| {
            let at = of as usize * MATERIAL_SIZE as usize + 64;
            u32::from_le_bytes(r.material_bytes[at..at + 4].try_into().expect("bits"))
        };
        assert_eq!(bits(row(lawn)) & 1, 1, "the look's row is textured");
        assert_eq!(bits(GROUND_ROW), 0, "the ground's own row has no map");
        // Items until the draw-slot buffer regrows: the rewrite keeps the look's row.
        let (mesh, material) = (box_mesh(&mut r), r.add_material(&Material::default()));
        let one = [Mat4::from_translation(Vec3::X)];
        let capacity = r.draw_capacity;
        while r.draw_capacity == capacity {
            r.add_item(mesh, material, &one);
        }
        assert_eq!(
            ground_words(&r, &device, &queue),
            (u32::MAX - 1, row(lawn)),
            "a regrown buffer keeps the look's row"
        );
        r.set_ground_material(None);
        assert_eq!(
            ground_words(&r, &device, &queue),
            (u32::MAX - 1, GROUND_ROW),
            "no look again: the ground's own row"
        );
        r.set_ground_material(Some(lawn));
        r.remove_material(lawn);
        assert_eq!(
            ground_words(&r, &device, &queue),
            (u32::MAX - 1, GROUND_ROW),
            "the look removed: the ground's own row"
        );
    }

    /// An item whose later part makes the draw-slot buffer regrow keeps every part's slot: the
    /// rewrite after a regrow covers the items there are, and the item being added was not one
    /// yet, so its earlier parts read zero (the ground's material, mesh 0) and a glass part was
    /// shaded as ground.
    #[test]
    fn an_item_added_as_the_slot_buffer_regrows_keeps_its_slots() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let plain = r.add_material(&Material::default());
        let glass = r.add_material(&Material {
            surface: Surface::Glass,
            ..Default::default()
        });
        // Mesh 0 is what a zero slot names: a spare mesh first, so the parts use later ones.
        let spare = box_mesh(&mut r);
        let (bark, leaves) = (box_mesh(&mut r), box_mesh(&mut r));
        assert_ne!(bark.0, 0);
        assert_ne!(leaves.0, 0);
        let one = [Mat4::from_translation(Vec3::X)];
        let capacity = r.draw_capacity;
        while live_slots(&r) < capacity - 1 {
            r.add_item(spare, plain, &one);
        }
        assert_eq!(
            live_slots(&r),
            capacity - 1,
            "one slot left before the buffer doubles"
        );
        let item = r.add_lod_item(
            &[(bark, plain, f32::INFINITY), (leaves, glass, f32::INFINITY)],
            &one,
        );
        assert_eq!(r.draw_capacity, capacity * 2, "the second part regrew it");
        let parts = &r.items[item.0 as usize].as_ref().expect("item").parts;
        assert_eq!(parts.len(), 2);
        let all = slots(&r, &device, &queue);
        for p in parts {
            assert_eq!(
                slot_words(all[p.slot as usize]),
                (p.mesh.0, row(p.material)),
                "the slot of mesh {} keeps its mesh and row",
                p.mesh.0
            );
        }
    }

    /// The material table's size: twice the old capacity or just enough when that is more,
    /// always room for the rows; a power of two; and never past the largest storage binding,
    /// where a limit that is no power of two of bytes used to be passed by the rounding up.
    #[test]
    fn the_material_table_stays_within_one_binding() {
        let mib = 1 << 20;
        assert_eq!(
            table_size(65, 64, 128 * mib),
            16384,
            "65 rows from 64: twice"
        );
        assert_eq!(table_size(1, 0, 128 * mib), 128, "one row, rounded up");
        assert_eq!(
            table_size(1, 64, 128 * mib),
            16384,
            "twice the old capacity, rounded up"
        );
        // A limit of 2^31 - 1 bytes holds 19 173 952 rows; their bytes rounded up are 2^31.
        assert_eq!(
            table_size(19_173_952, 64, (1u64 << 31) - 1),
            (1u64 << 31) - 1,
            "the rounding up stays under the limit"
        );
        for limit in [
            112 * 1000 + 7,
            1 << 27,
            (1u64 << 31) - 1,
            112 * 4096,
            112u64 * 3000 - 1,
        ] {
            let last = (limit / MATERIAL_SIZE) as u32;
            let mut samples = vec![1, 2, 63, 64, 65, 100, 146, 147, 999, 1000];
            samples.extend([last / 2, last.saturating_sub(1), last]);
            for capacity in [0u32, 1, 64, 146, 1000] {
                for rows in samples.iter().copied().filter(|&r| r <= last) {
                    let size = table_size(rows, capacity, limit);
                    assert!(
                        size <= limit,
                        "{rows} rows from {capacity} under {limit}: {size}"
                    );
                    assert!(
                        size / MATERIAL_SIZE >= u64::from(rows),
                        "{rows} rows from {capacity} under {limit}: {size}"
                    );
                }
            }
        }
    }

    /// A row of boxes, each its own untextured material (the material table grows on the way):
    /// one batch draws them all, each shaded with its own row.
    #[test]
    fn many_materials_draw_their_own_colour_in_one_batch() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let (w, h) = (320u32, 60u32);
        let n = 150u32;
        let scene = |own: bool| {
            let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
            let mut m = demo::MeshData::default();
            m.add_box(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0));
            let mesh = r.add_mesh(&m.mesh()).expect("mesh");
            let first = r.add_material(&Material {
                base_color: [1.0, 0.2, 0.0, 1.0],
                roughness: 0.9,
                ..Default::default()
            });
            for k in 0..n {
                let t = k as f32 / n as f32;
                let material = if own {
                    r.add_material(&Material {
                        base_color: [1.0 - t, 0.2, t, 1.0],
                        roughness: 0.9,
                        ..Default::default()
                    })
                } else {
                    first
                };
                r.add_item(
                    mesh,
                    material,
                    &[Mat4::from_translation(Vec3::new(2.5 * k as f32, 0.0, 0.0))],
                );
            }
            r.resize(w, h);
            r.set_quality(Quality::LOW);
            let (half, eye) = (
                2.5 * n as f32 * 0.5,
                Vec3::new(2.5 * n as f32 * 0.5 - 1.25, 1.0, 20.0),
            );
            let view = Mat4::look_at_rh(eye, Vec3::new(eye.x, 1.0, 0.0), Vec3::Y);
            let proj = Mat4::orthographic_rh(-half, half, -1.5, 1.5, 0.1, 100.0);
            r.set_camera(view, proj, eye);
            r
        };
        let forward = |r: &Renderer| {
            r.stats()
                .pass_draws
                .iter()
                .find(|(l, _)| *l == "forward")
                .map(|(_, n)| *n)
                .expect("a forward pass")
        };
        let mut one = scene(false);
        pixels(&mut one, &device, &queue, w, h);
        let (one_px, one_forward) = (pixels(&mut one, &device, &queue, w, h), forward(&one));
        assert!(one_forward <= 3, "{one_forward} forward draws");
        let mut own = scene(true);
        pixels(&mut own, &device, &queue, w, h);
        let own_px = pixels(&mut own, &device, &queue, w, h);
        assert_eq!(
            forward(&own),
            one_forward,
            "colour alone does not split draws"
        );
        assert_ne!(own_px, one_px, "the colours differ");
        // Red minus blue, averaged over the middle row's left and right quarter (at LOW the
        // boxes are under two pixels wide and upscaled, so one pixel mixes its neighbours): the
        // row's left stays red, as with one material, and its right turns blue, with the
        // materials past the table's growth from 64 rows (boxes 113 on use rows 115 on). The
        // light is the sky's, blue: a material with less than about three times more red than
        // blue (box 40 on) reads blue itself, so the left quarter is the red one.
        let quarter = |px: &[u8], from: usize| {
            let mut sum = [0i64; 3];
            for x in from..from + w as usize / 4 {
                let i = ((h as usize / 2) * w as usize + x) * 4;
                for (c, s) in sum.iter_mut().enumerate() {
                    *s += i64::from(px[i + c]);
                }
            }
            let n = (w / 4) as f32;
            (sum[0] as f32 / n, sum[2] as f32 / n)
        };
        let (right_from, left_from) = (w as usize / 4 * 3, 0);
        let (one_left, one_right) = (quarter(&one_px, left_from), quarter(&one_px, right_from));
        let (own_left, own_right) = (quarter(&own_px, left_from), quarter(&own_px, right_from));
        let red = |q: (f32, f32)| q.0 - q.1;
        assert!(
            red(one_left) > 40.0 && red(one_right) > 40.0,
            "one red material, both quarters red: {one_left:?} {one_right:?}"
        );
        assert!(
            red(own_left) > 40.0,
            "the left quarter stays red: {own_left:?}"
        );
        assert!(
            red(own_right) < -10.0,
            "the right quarter is blue: {own_right:?}"
        );
        assert!(
            red(own_right) < red(one_right) - 40.0,
            "the right quarter differs from one material's: {own_right:?} against {one_right:?}"
        );
    }

    /// How planned draws are issued: batched mesh draws with the same bindings run as one
    /// indirect call, everything between them one draw each; the arguments packed are those of
    /// the runs' draws, and are what their direct draws name.
    #[test]
    fn runs_of_batched_draws_with_the_same_bindings_share_a_call() {
        let mesh = |first: u32, id: u32| DrawMesh {
            id,
            arena: 0,
            range: batch::Span::new(first, 12),
            base_vertex: 0,
        };
        let draw = |mesh, g1, src| Draw {
            pipe: Pipe::Depth(0),
            g1,
            g2: None,
            mesh,
            vertices: 12,
            src,
            instances: 0..3,
            triangles: 4,
            view: false,
        };
        let set = Some(Group1::Set(0));
        let elsewhere = |first: u32, id: u32| DrawMesh {
            arena: 1,
            ..mesh(first, id)
        };
        let draws = vec![
            draw(Some(mesh(0, 0)), set, Src::Gathered),
            draw(Some(mesh(12, 1)), set, Src::Gathered),
            draw(None, set, Src::Gathered),
            draw(Some(mesh(24, 2)), set, Src::Gathered),
            draw(Some(mesh(36, 3)), None, Src::Gathered),
            draw(Some(mesh(48, 4)), None, Src::Gathered),
            draw(Some(mesh(60, 5)), None, Src::Pool),
            draw(Some(elsewhere(72, 6)), None, Src::Gathered),
            draw(Some(elsewhere(84, 7)), None, Src::Gathered),
            draw(Some(mesh(96, 8)), None, Src::Gathered),
        ];
        let calls = |indirect: bool| Calls::new(&draws, indirect).collect::<Vec<_>>();
        assert_eq!(
            calls(true),
            vec![(0, 2), (2, 1), (3, 1), (4, 2), (6, 1), (7, 2), (9, 1)],
            "a run stays in one arena"
        );
        assert_eq!(
            calls(false),
            (0..draws.len()).map(|i| (i, 1)).collect::<Vec<_>>()
        );
        let mut args = Vec::new();
        pack_args(&mut args, &draws, true);
        assert_eq!(args.len(), INDIRECT_ARGS as usize * 6, "three runs of two");
        assert_eq!(&args[..20], &u32_bytes(&[12, 3, 0, 0, 0]));
        assert_eq!(&args[20..40], &u32_bytes(&[12, 3, 12, 0, 0]));
        assert_eq!(&args[40..60], &u32_bytes(&[12, 3, 36, 0, 0]));
        assert_eq!(&args[60..80], &u32_bytes(&[12, 3, 48, 0, 0]));
        assert_eq!(&args[80..100], &u32_bytes(&[12, 3, 72, 0, 0]));
        assert_eq!(&args[100..120], &u32_bytes(&[12, 3, 84, 0, 0]));
        args.clear();
        pack_args(&mut args, &draws, false);
        assert!(args.is_empty(), "every draw direct");
    }

    /// Forty meshes of one texture set: the forward pass issues one call for them all (a run
    /// through the indirect buffer), so `pass_calls` is below `pass_draws`; with the direct
    /// fallback every draw is a call, and the picture is the same.
    #[test]
    fn many_meshes_draw_in_one_indirect_call_and_the_fallback_draws_the_same() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let (w, h) = (320u32, 60u32);
        let scene = || {
            let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
            let material = r.add_material(&Material::default());
            let mut rng = demo::Rng::new(5);
            for k in 0..40 {
                let mut m = demo::MeshData::default();
                let a = 0.4 + rng.unit() * 0.5;
                let b = 0.6 + rng.unit() * 1.5;
                m.add_box(Vec3::new(-a, 0.0, -a), Vec3::new(a, b, a));
                let mesh = r.add_mesh(&m.mesh()).expect("mesh");
                r.add_item(
                    mesh,
                    material,
                    &[Mat4::from_translation(Vec3::new(2.5 * k as f32, 0.0, 0.0))],
                );
            }
            r.resize(w, h);
            r.set_quality(Quality::LOW);
            let (half, eye) = (50.0f32, Vec3::new(48.75, 1.0, 20.0));
            let view = Mat4::look_at_rh(eye, Vec3::new(eye.x, 1.0, 0.0), Vec3::Y);
            let proj = Mat4::orthographic_rh(-half, half, -1.5, 1.5, 0.1, 100.0);
            r.set_camera(view, proj, eye);
            r
        };
        // The calls of one pass group (the forward pass's: the batches' and the sky's).
        let calls_of = |r: &Renderer, key: &str| -> u32 {
            r.stats()
                .pass_calls
                .iter()
                .filter(|(l, _)| *l == key)
                .map(|(_, n)| *n)
                .sum()
        };
        let draw_count = |r: &Renderer, key: &str| -> u32 {
            r.stats()
                .pass_draws
                .iter()
                .find(|(l, _)| *l == key)
                .map(|(_, n)| *n)
                .expect("the pass")
        };
        let mut indirect = scene();
        let with_feature = indirect.multi_draw;
        pixels(&mut indirect, &device, &queue, w, h);
        let indirect_px = pixels(&mut indirect, &device, &queue, w, h);
        let (draws, calls) = (
            draw_count(&indirect, "forward"),
            calls_of(&indirect, "forward"),
        );
        let mut direct = scene();
        direct.multi_draw = false;
        pixels(&mut direct, &device, &queue, w, h);
        let direct_px = pixels(&mut direct, &device, &queue, w, h);
        assert_eq!(indirect_px, direct_px, "the same picture either way");
        let direct_calls = calls_of(&direct, "forward");
        // A call per draw, and the sky's, which `pass_draws` does not count.
        assert_eq!(
            direct_calls,
            draw_count(&direct, "forward") + 1,
            "a call per draw"
        );
        if with_feature {
            assert!(calls < draws, "{calls} calls for {draws} draws");
        } else {
            eprintln!(
                "the adapter has no INDIRECT_FIRST_INSTANCE: {calls} calls for {draws} draws"
            );
            assert_eq!(calls, direct_calls);
        }
    }

    /// An item's clusters partition its instances (by 8 m cell), and its blocks its clusters
    /// (by 128 m block of the ground grid: each cluster in exactly one), wherever the instances
    /// stand.
    #[test]
    fn blocks_partition_the_clusters_and_the_clusters_the_instances() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut m = demo::MeshData::default();
        m.add_box(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
        let mesh = r.add_mesh(&m.mesh()).expect("mesh");
        let material = r.add_material(&Material::default());
        let mut rng = demo::Rng::new(3);
        let at: Vec<Mat4> = (0..5000)
            .map(|_| {
                Mat4::from_translation(Vec3::new(
                    rng.unit() * 600.0 - 300.0,
                    0.0,
                    rng.unit() * 600.0 - 300.0,
                ))
            })
            .collect();
        let item = r.add_lod_item(&[(mesh, material, f32::INFINITY)], &at);
        let item = r.items[item.0 as usize].as_ref().expect("item");
        let cell = |m: &Mat4| {
            (
                (m.w_axis.x / CLUSTER_CELL).floor() as i32,
                (m.w_axis.z / CLUSTER_CELL).floor() as i32,
            )
        };
        let cells = same_ranges(&item.transforms, |a, b| cell(a) == cell(b));
        assert_eq!(
            cells,
            item.clusters
                .iter()
                .map(|c| c.range.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(item.clusters.last().map(|c| c.range.end), Some(5000));
        let block = |c: &Cluster| {
            let m = &item.transforms[c.range.start as usize];
            (
                (m.w_axis.x / 128.0).floor() as i32,
                (m.w_axis.z / 128.0).floor() as i32,
            )
        };
        assert_eq!(item.block_of.len(), item.clusters.len());
        let mut first: Vec<Option<(i32, i32)>> = vec![None; item.blocks.len()];
        for (ci, &b) in item.block_of.iter().enumerate() {
            let k = block(&item.clusters[ci]);
            match &mut first[b as usize] {
                Some(f) => assert_eq!(*f, k, "cluster {ci}"),
                None => first[b as usize] = Some(k),
            }
        }
        assert!(first.iter().all(Option::is_some), "no empty block");
        assert!(item.blocks.len() > 20, "{} blocks", item.blocks.len());
    }

    /// Every block's bounds hold those of each of its clusters (so a block outside a view has
    /// no cluster inside it), over many items with and without levels, some removed.
    #[test]
    fn a_block_holds_its_clusters() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(64, 36);
        let mut rng = demo::Rng::new(9);
        for k in 0..40 {
            let s = 0.5 + rng.unit();
            let mut m = demo::MeshData::default();
            m.add_box(Vec3::new(-s, 0.0, -s), Vec3::new(s, 2.0 * s, s));
            let mesh = r.add_mesh(&m.mesh()).expect("mesh");
            let material = r.add_material(&Material::default());
            let n = 1 + (rng.unit() * 300.0) as usize;
            let at: Vec<Mat4> = (0..n)
                .map(|_| {
                    Mat4::from_translation(Vec3::new(
                        rng.unit() * 800.0 - 400.0,
                        0.0,
                        rng.unit() * 800.0 - 400.0,
                    ))
                })
                .collect();
            let item = if k % 3 == 0 {
                r.add_lod_item(&[(mesh, material, 120.0)], &at)
            } else {
                r.add_item(mesh, material, &at)
            };
            if k % 5 == 0 {
                r.remove_item(item);
            }
        }
        r.update_bounds();
        let mut checked = 0usize;
        for item in r.items.iter().flatten() {
            for (c, &b) in item.clusters.iter().zip(&item.block_of) {
                let (Some(cb), Some(bb)) = (c.bounds, item.blocks[b as usize].bounds) else {
                    continue;
                };
                for axis in 0..3 {
                    assert!(
                        bb.0[axis] <= cb.0[axis] && bb.1[axis] >= cb.1[axis],
                        "block {b} of a cluster: {bb:?} against {cb:?}"
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 5_000, "{checked}");
    }

    /// Over a camera path of 300 frames, skipping the blocks past the last level leaves every
    /// cluster's level, target and dissolve as visiting them all does, while hand-overs do
    /// dissolve on the way.
    #[test]
    fn skipped_blocks_leave_the_levels_unchanged() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let scene = |skip: bool| {
            let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
            r.resize(64, 36);
            r.lod_skip = skip;
            let mut rng = demo::Rng::new(21);
            for k in 0..12 {
                let mut m = demo::MeshData::default();
                m.add_box(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.5, 0.5));
                let mesh = r.add_mesh(&m.mesh()).expect("mesh");
                let material = r.add_material(&Material::default());
                let at: Vec<Mat4> = (0..400)
                    .map(|_| {
                        Mat4::from_translation(Vec3::new(
                            rng.unit() * 600.0 - 300.0,
                            0.0,
                            rng.unit() * 600.0 - 300.0,
                        ))
                    })
                    .collect();
                let item = r.add_lod_item(
                    &[
                        (mesh, material, 25.0),
                        (mesh, material, 70.0),
                        (mesh, material, 160.0),
                    ],
                    &at,
                );
                if k % 4 == 0 {
                    r.set_item_cull_fade(item, 8.0);
                }
            }
            r
        };
        let (mut full, mut skip) = (scene(false), scene(true));
        let mut dissolving = 0usize;
        let mut skipped = 0usize;
        for f in 0..300 {
            let t = f as f32 / 10.0;
            let eye = Vec3::new(
                t * 3.0 - 40.0,
                1.7 + (t * 0.2).sin() * 2.0,
                (t * 0.7).cos() * 120.0,
            );
            let target = eye + Vec3::new((t * 0.3).sin(), -0.1, 1.0);
            for r in [&mut full, &mut skip] {
                let (view, proj, eye) = demo::camera(eye, target, 16.0 / 9.0);
                r.set_camera(view, proj, eye);
                r.update_bounds();
                r.update_lods();
            }
            for (a, b) in full.items.iter().zip(&skip.items) {
                let (Some(a), Some(b)) = (a, b) else {
                    continue;
                };
                for (ca, cb) in a.clusters.iter().zip(&b.clusters) {
                    assert_eq!(
                        (ca.lod, ca.target, ca.fade, ca.fading, ca.far, ca.to_far),
                        (cb.lod, cb.target, cb.fade, cb.fading, cb.far, cb.to_far),
                        "frame {f}"
                    );
                    dissolving += ca.fading as usize;
                    skipped += (ca.target == a.lods.len() as u8) as usize;
                }
            }
        }
        assert!(
            dissolving > 100,
            "hand-overs dissolved on the way: {dissolving}"
        );
        assert!(skipped > 1000, "clusters past the last level: {skipped}");
    }

    /// An item that casts from cascade 1 on adds no draw to cascade 0 and the same draws as
    /// before to the later ones; an item without the setting is as it was.
    #[test]
    fn an_item_can_stay_out_of_the_near_cascades() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(64, 36);
        let mut m = demo::MeshData::default();
        m.add_box(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.5, 0.5));
        let mesh = r.add_mesh(&m.mesh()).expect("mesh");
        let material = r.add_material(&Material::default());
        let at: Vec<Mat4> = (0..40)
            .map(|i| Mat4::from_translation(Vec3::new(3.0 * i as f32, 0.0, 0.0)))
            .collect();
        let plain = r.add_item(mesh, material, &at);
        let late = r.add_item(mesh, material, &at);
        r.set_item_shadow_cascades(late, 1);
        // Among the boxes, so the near cascade has them in view.
        let (view, proj, eye) =
            demo::camera(Vec3::new(60.0, 3.0, 4.0), Vec3::new(60.0, 0.5, 0.0), 1.0);
        r.set_camera(view, proj, eye);
        r.update_bounds();
        r.update_lods();
        let draws = |r: &mut Renderer, cascade: usize| {
            r.visibility.follow(&r.items);
            let mut visibility = std::mem::take(&mut r.visibility);
            let n = r
                .plan_items(
                    proj * view,
                    false,
                    Pass::Shadow {
                        far: false,
                        coarse: cascade > 0,
                        cascade,
                    },
                    None,
                    &mut batch::GatherList::default(),
                    &mut visibility,
                )
                .len();
            r.visibility = visibility;
            n
        };
        let (far0, far1) = (draws(&mut r, 0), draws(&mut r, 1));
        assert_eq!(
            draws(&mut r, 2),
            far1,
            "the setting changes no later cascade"
        );
        // Both items share mesh and texture set: one batch per cascade.
        assert_eq!((far0, far1), (1, 1));
        r.remove_item(plain);
        let (late0, late1) = (draws(&mut r, 0), draws(&mut r, 1));
        assert_eq!(late0, 0, "nothing in cascade 0");
        assert_eq!(late1, far1, "the later cascades as before");
        r.set_item_shadow_cascades(late, u8::MAX);
        assert_eq!(
            draws(&mut r, 1),
            0,
            "past the last cascade: no shadow at all"
        );
    }

    /// The plane test culls exactly what testing the eight corners culled.
    #[test]
    fn culling_by_planes_matches_the_corners() {
        let by_corners = |m: Mat4, b: Bounds, near_far: bool| {
            let c = corners(b).map(|p| m * p.extend(1.0));
            let all = |f: &dyn Fn(Vec4) -> bool| c.iter().all(|&p| f(p));
            all(&|p| p.x < -p.w)
                || all(&|p| p.x > p.w)
                || all(&|p| p.y < -p.w)
                || all(&|p| p.y > p.w)
                || (near_far && (all(&|p| p.z < 0.0) || all(&|p| p.z > p.w)))
        };
        let view = Mat4::look_at_rh(Vec3::new(3.0, 2.0, 9.0), Vec3::ZERO, Vec3::Y);
        let views = [
            Mat4::perspective_rh(1.0, 1.6, 0.1, 50.0) * view,
            Mat4::orthographic_rh(-8.0, 8.0, -5.0, 5.0, 0.1, 40.0) * view,
        ];
        let mut seed = 7u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        let (mut culled, mut kept) = (0, 0);
        for _ in 0..4000 {
            let lo = Vec3::new(next(), next(), next()) * 80.0 - 40.0;
            let b = (lo, lo + Vec3::new(next(), next(), next()) * 6.0);
            for (m, near_far) in views.iter().flat_map(|m| [(*m, false), (*m, true)]) {
                let want = by_corners(m, b, near_far);
                assert_eq!(outside(m, b, near_far), want, "{b:?}");
                if want { culled += 1 } else { kept += 1 }
            }
        }
        assert!(culled > 1000 && kept > 1000, "{culled} {kept}");
    }

    /// A user editing: 2000 random adds, replacements and removals of
    /// meshes, textures, materials and items with frames in between; when
    /// everything is removed again, memory and slots are back at baseline
    /// and no frame stalled.
    #[test]
    fn edit_soak_returns_to_baseline() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        r.resize(64, 36);
        r.set_quality(Quality::MEDIUM);
        let worst = Cell::new(0f64);
        let frame = |r: &mut Renderer| {
            let t = std::time::Instant::now();
            let mut e = device.create_command_encoder(&Default::default());
            r.render(&mut e);
            queue.submit([e.finish()]);
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            worst.set(worst.get().max(t.elapsed().as_secs_f64()));
        };
        frame(&mut r);
        let live = |r: &Renderer| {
            (
                r.stats().texture_bytes,
                r.meshes.iter().flatten().count(),
                r.textures.iter().flatten().count(),
                r.materials.iter().flatten().count(),
                r.items.iter().flatten().count(),
                r.next_slot as usize - r.free_slots.len(),
                r.mesh_ranges.used(),
            )
        };
        let baseline = live(&r);
        let mut rng = demo::Rng::new(7);
        let mut pick = |n: usize| (rng.unit() * n as f32) as usize % n.max(1);
        let (mut meshes, mut textures, mut materials, mut items) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let cube = |s: f32| {
            let mut m = demo::MeshData::default();
            m.add_box(Vec3::ZERO, Vec3::splat(0.1 + s));
            m
        };
        for step in 0..2000 {
            match pick(8) {
                0 => meshes.push(r.add_mesh(&cube(pick(10) as f32).mesh()).expect("mesh")),
                1 if !meshes.is_empty() => {
                    let m = meshes[pick(meshes.len())];
                    r.replace_mesh(m, &cube(pick(5) as f32).mesh())
                        .expect("replace");
                }
                2 if !meshes.is_empty() => r.remove_mesh(meshes.swap_remove(pick(meshes.len()))),
                3 => textures.push(
                    r.add_texture(&TextureDesc {
                        width: 16,
                        height: 16,
                        rgba8: &[128; 16 * 16 * 4],
                        color_space: ColorSpace::Srgb,
                        repeat: true,
                    })
                    .expect("texture"),
                ),
                4 if !textures.is_empty() => {
                    r.remove_texture(textures.swap_remove(pick(textures.len())))
                }
                5 => {
                    let map = (!textures.is_empty()).then(|| textures[pick(textures.len())]);
                    materials.push(r.add_material(&Material {
                        base_color_map: map,
                        ..Default::default()
                    }));
                }
                6 if !materials.is_empty() => {
                    r.remove_material(materials.swap_remove(pick(materials.len())))
                }
                _ if !meshes.is_empty() && !materials.is_empty() && pick(2) == 0 => {
                    let (m, mat) = (meshes[pick(meshes.len())], materials[pick(materials.len())]);
                    items.push(r.add_item(
                        m,
                        mat,
                        &[Mat4::from_translation(Vec3::X * step as f32 * 0.01)],
                    ));
                }
                _ if !items.is_empty() => r.remove_item(items.swap_remove(pick(items.len()))),
                _ => {}
            }
            if step % 25 == 0 {
                frame(&mut r);
            }
        }
        for i in items {
            r.remove_item(i);
        }
        for m in meshes {
            r.remove_mesh(m);
        }
        for m in materials {
            r.remove_material(m);
        }
        for t in textures {
            r.remove_texture(t);
        }
        frame(&mut r);
        frame(&mut r);
        assert_eq!(
            live(&r),
            baseline,
            "(bytes, meshes, textures, materials, items, slots, arena elements)"
        );
        // Guards against a runaway frame, not speed: a software rasteriser on a shared
        // machine has taken 2.1 s here.
        assert!(worst.get() < 6.0, "a frame took {:.2} s", worst.get());
    }

    /// The presets budget 0, 1, 2 and 3 planar passes; the globals hold four planes at most.
    #[test]
    fn the_presets_budget_their_planar_water_passes() {
        let budgets: Vec<u32> = Quality::PRESETS
            .iter()
            .map(|(_, q)| q.plane_budget())
            .collect();
        assert_eq!(budgets, [0, 1, 2, 3]);
        assert_eq!(Quality::LOW.water_reflection, 0);
        // The planar reflection off: no budget, whatever the setting.
        assert_eq!(
            (Quality {
                water_reflection: 0,
                water_planes: 4,
                ..Quality::ULTRA
            })
            .plane_budget(),
            0
        );
        assert_eq!(
            (Quality {
                water_planes: 9,
                ..Quality::ULTRA
            })
            .plane_budget(),
            water::MAX_PLANES
        );
    }

    /// Renders one frame and reads the output back as tightly packed RGBA8.
    fn pixels(
        r: &mut Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        w: u32,
        h: u32,
    ) -> Vec<u8> {
        let mut encoder = device.create_command_encoder(&Default::default());
        r.render(&mut encoder);
        let stride = (w * 4).next_multiple_of(256);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (stride * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            r.output_texture().as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |res| res.expect("map"));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let padded = buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped")
            .to_vec();
        padded
            .chunks(stride as usize)
            .flat_map(|row| &row[..(w * 4) as usize])
            .copied()
            .collect()
    }

    /// The meshes of `replaced_and_removed_meshes_draw_the_same_as_fresh_ones`: a row of boxes,
    /// the third of three before or after (more data), the sixth of two before and one after
    /// (less), the eighth removed, two more after it. `end`: as they end up.
    fn boxes(end: bool) -> Vec<demo::MeshData> {
        (0..12)
            .map(|i| {
                let mut m = demo::MeshData::default();
                let count = match (end, i) {
                    (true, 3) => 3,
                    (false, 5) => 2,
                    _ => 1,
                };
                for k in 0..count {
                    let x = 0.9 * k as f32;
                    m.add_box(Vec3::new(x - 0.3, 0.0, -0.3), Vec3::new(x + 0.3, 0.5, 0.3));
                }
                m
            })
            .collect()
    }

    /// Ten meshes in the arenas, then one replaced by more data (a new range, the old one
    /// freed), one by less (in place), one removed (its range freed) and two more added: the
    /// picture is the one of the same final meshes uploaded fresh, pixel for pixel.
    #[test]
    fn replaced_and_removed_meshes_draw_the_same_as_fresh_ones() {
        let (device, queue) = pollster::block_on(headless_device()).expect("device");
        let (w, h) = (96, 54);
        let (before, boxes) = (boxes(false), boxes(true));
        let at = |i: usize| Mat4::from_translation(Vec3::new(2.5 * i as f32, 0.0, 0.0));
        let look = |r: &mut Renderer| {
            let aspect = w as f32 / h as f32;
            let (view, proj, eye) = demo::camera(
                Vec3::new(13.0, 9.0, 26.0),
                Vec3::new(13.0, 0.5, 0.0),
                aspect,
            );
            r.resize(w, h);
            r.set_quality(Quality::MEDIUM);
            r.set_camera(view, proj, eye);
        };
        // Edited: the first ten, then the edits, then two more.
        let mut a = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut ids: Vec<MeshId> = before[..10]
            .iter()
            .map(|m| a.add_mesh(&m.mesh()).expect("mesh"))
            .collect();
        let material = a.add_material(&Material::default());
        let items: Vec<ItemId> = ids
            .iter()
            .enumerate()
            .map(|(i, &m)| a.add_item(m, material, &[at(i)]))
            .collect();
        look(&mut a);
        pixels(&mut a, &device, &queue, w, h);
        let (before3, before5) = (
            a.meshes[3].as_ref().expect("3").place,
            a.meshes[5].as_ref().expect("5").place,
        );
        a.replace_mesh(ids[3], &boxes[3].mesh())
            .expect("replace, more data");
        a.replace_mesh(ids[5], &boxes[5].mesh())
            .expect("replace, less data");
        a.remove_mesh(ids[7]);
        a.remove_item(items[7]);
        ids.extend(
            boxes[10..]
                .iter()
                .map(|m| a.add_mesh(&m.mesh()).expect("mesh")),
        );
        for (i, &m) in ids[10..].iter().enumerate() {
            a.add_item(m, material, &[at(10 + i)]);
        }
        let after = (
            a.meshes[3].as_ref().expect("3").place,
            a.meshes[5].as_ref().expect("5").place,
        );
        assert_ne!(after.0, before3, "more data: a new range");
        assert_eq!(after.1, before5, "less data: in place");
        assert!(a.meshes[7].is_none());
        // Fresh: the same final meshes, in one go.
        let mut b = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let material = b.add_material(&Material::default());
        for (i, m) in boxes.iter().enumerate() {
            if i == 7 {
                continue;
            }
            let mesh = b.add_mesh(&m.mesh()).expect("mesh");
            b.add_item(mesh, material, &[at(i)]);
        }
        look(&mut b);
        pixels(&mut b, &device, &queue, w, h);
        let (pa, pb) = (
            pixels(&mut a, &device, &queue, w, h),
            pixels(&mut b, &device, &queue, w, h),
        );
        assert_eq!(pa, pb, "the edited scene draws what the fresh one does");
        assert!(
            pa.chunks(4).any(|p| p[0] != p[1] || p[1] != p[2]),
            "something is drawn"
        );
    }

    #[test]
    fn mirror_reflects_about_the_water_plane() {
        let h = -0.04;
        let m = mirror_matrix(h);
        let p = Vec3::new(1.5, 2.0, -3.0);
        let q = m.transform_point3(p);
        assert!(
            (q - Vec3::new(1.5, 2.0 * h - 2.0, -3.0)).length() < 1e-6,
            "{q}"
        );
        assert!((m * m).abs_diff_eq(Mat4::IDENTITY, 1e-6), "an involution");
        assert!(
            m.determinant() < 0.0,
            "flips winding: front faces become clockwise"
        );
        // The mirrored camera sees the reflected point where the real camera
        // sees the water point on the line to the mirror image.
        let eye = Vec3::new(0.0, 1.7, 8.0);
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let proj = Mat4::perspective_rh(1.0, 1.5, 0.1, 100.0);
        let real = (proj * view).project_point3(m.transform_point3(p));
        let mirrored = (proj * view * m).project_point3(p);
        assert!((real - mirrored).length() < 1e-5);
        // The clip plane keeps what is above the water only.
        assert!(p.y > h && q.y < h);
    }

    #[test]
    fn bloom_levels_halve_down_to_one_pixel() {
        assert_eq!(
            bloom_sizes((1536, 864)),
            [(768, 432), (384, 216), (192, 108), (96, 54)]
        );
        assert_eq!(bloom_sizes((3, 1)), [(1, 1); 4]);
        assert_eq!(bloom_sizes((1921, 1081))[0], (960, 540));
    }

    /// `ShadowFilter::sanitized` keeps `Quality: Eq` total: taps become the nearest of 4, 9 and
    /// 16, a NaN radius the presets' (PCSS: its own default), and a radius stays within 0..=1.
    #[test]
    fn shadow_filter_sanitizes_taps_and_radius() {
        let pcf = |taps, radius_m| ShadowFilter::Pcf { taps, radius_m };
        let pcss = |max_radius_m| ShadowFilter::Pcss { max_radius_m };
        assert_eq!(pcf(7, 0.05).sanitized(), pcf(9, 0.05));
        assert_eq!(pcf(1, 0.05).sanitized(), pcf(4, 0.05));
        assert_eq!(pcf(12, 0.05).sanitized(), pcf(9, 0.05));
        assert_eq!(pcf(13, 0.05).sanitized(), pcf(16, 0.05));
        assert_eq!(
            pcf(9, f32::NAN).sanitized(),
            pcf(9, DEFAULT_SHADOW_RADIUS_M)
        );
        assert_eq!(pcf(9, 5.0).sanitized(), pcf(9, 1.0));
        assert_eq!(pcf(9, -1.0).sanitized(), pcf(9, 0.0));
        assert_eq!(pcf(9, -0.0).sanitized(), pcf(9, 0.0));
        assert_eq!(pcf(9, f32::INFINITY).sanitized(), pcf(9, 1.0));
        assert_eq!(ShadowFilter::Hard.sanitized(), ShadowFilter::Hard);
        assert_eq!(pcss(f32::NAN).sanitized(), pcss(DEFAULT_PCSS_MAX_RADIUS_M));
        assert_eq!(pcss(5.0).sanitized(), pcss(1.0));
        assert_eq!(pcss(-1.0).sanitized(), pcss(0.0));
        assert_eq!(pcss(-0.0).sanitized(), pcss(0.0));
        assert_eq!(pcss(f32::INFINITY).sanitized(), pcss(1.0));
        assert_eq!(pcss(0.15).sanitized(), pcss(0.15));
        // Idempotent, so a preset equals its own sanitized self (the renderer compares).
        for f in [
            pcf(9, 0.05),
            pcf(16, 0.2),
            pcss(0.2),
            pcss(f32::NAN),
            ShadowFilter::Hard,
        ] {
            assert_eq!(f.sanitized().sanitized(), f.sanitized());
        }
        // A NaN radius would break `==` (and so `Eq`): sanitized, a filter equals itself.
        let f = pcf(9, f32::NAN).sanitized();
        assert_eq!(f, f);
        let f = pcss(f32::NAN).sanitized();
        assert_eq!(f, f);
    }

    /// Every preset filters the sun's shadows with PCF at today's tap counts (LOW and MEDIUM
    /// 4, HIGH 9, ULTRA 16); the radius is the 16-tap disk's alone, and under `fast_shading` the
    /// shaders run 4 taps at most, PCSS included.
    #[test]
    fn every_preset_filters_shadows_with_pcf() {
        for (name, q) in Quality::PRESETS {
            assert!(
                matches!(q.shadow_filter, ShadowFilter::Pcf { .. }),
                "{name}"
            );
            assert_eq!(
                q.shadow_filter.radius_m(),
                DEFAULT_SHADOW_RADIUS_M,
                "{name}"
            );
            assert_eq!(q.shadow_filter, q.shadow_filter.sanitized(), "{name}");
        }
        assert_eq!(Quality::LOW.shadow_filter.taps(), 4);
        assert_eq!(Quality::MEDIUM.shadow_filter.taps(), 4);
        assert_eq!(Quality::HIGH.shadow_filter.taps(), 9);
        assert_eq!(Quality::ULTRA.shadow_filter.taps(), 16);
        assert_eq!(Quality::MEDIUM.shadow_taps(), 4);
        assert_eq!(Quality::HIGH.shadow_taps(), 9);
        assert_eq!(Quality::ULTRA.shadow_taps(), 16);
        assert_eq!(
            Quality {
                shadow_filter: ShadowFilter::Hard,
                ..Quality::MEDIUM
            }
            .shadow_taps(),
            1
        );
        // PCSS is the 16-tap disk: 16 taps, and 4 under fast shading, as pcf16.
        let pcss = ShadowFilter::Pcss {
            max_radius_m: DEFAULT_PCSS_MAX_RADIUS_M,
        };
        assert_eq!(pcss.taps(), 16);
        assert_eq!(pcss.radius_m(), DEFAULT_PCSS_MAX_RADIUS_M);
        assert_eq!(
            Quality {
                shadow_filter: pcss,
                ..Quality::HIGH
            }
            .shadow_taps(),
            16
        );
        assert_eq!(
            Quality {
                shadow_filter: pcss,
                ..Quality::MEDIUM
            }
            .shadow_taps(),
            4
        );
    }

    /// `Globals.shadow_filter` carries the penumbra's radius and the 16-tap branch's mode: PCSS
    /// (1) for `ShadowFilter::Pcss`, PCF (0) for every other filter and under `fast_shading`,
    /// where the branch is not compiled.
    #[test]
    fn the_shadow_filter_uniform_switches_to_pcss() {
        assert_eq!(
            Quality::ULTRA.shadow_filter_uniform(),
            [DEFAULT_SHADOW_RADIUS_M, 0.0]
        );
        assert_eq!(
            Quality {
                shadow_filter: ShadowFilter::Hard,
                ..Quality::HIGH
            }
            .shadow_filter_uniform(),
            [0.0, 0.0]
        );
        let pcss = Quality {
            shadow_filter: ShadowFilter::Pcss { max_radius_m: 0.15 },
            ..Quality::HIGH
        };
        assert_eq!(pcss.shadow_filter_uniform(), [0.15, 1.0]);
        // A 16-tap disk is PCF at any radius.
        assert_eq!(
            Quality {
                shadow_filter: ShadowFilter::Pcf {
                    taps: 16,
                    radius_m: 0.2,
                },
                ..Quality::ULTRA
            }
            .shadow_filter_uniform(),
            [0.2, 0.0]
        );
        // Under fast shading the 16-tap branch is not compiled: PCSS runs as the 4-tap grid.
        assert_eq!(
            Quality {
                shadow_filter: pcss.shadow_filter,
                ..Quality::MEDIUM
            }
            .shadow_filter_uniform(),
            [0.15, 0.0]
        );
    }

    /// The 4- and 9-tap filters are the fixed texel-sized grids of before (scene.wgsl
    /// `cascade_shadow`: the offsets `g.shadow.z * 1.5`), so LOW, MEDIUM and HIGH shade as they
    /// did: they never read the penumbra's radius, which only the 16-tap disk spreads over.
    #[test]
    fn the_grid_filters_ignore_the_radius_and_stay_texel_sized() {
        let src = include_str!("scene.wgsl");
        let body = &src[src.find("fn cascade_shadow(").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let (disk, grids) = body.split_once("taps > 8.5").expect("the 9-tap branch");
        assert!(
            disk.contains("g.shadow_filter.x"),
            "the 16-tap disk spreads over the radius:\n{disk}"
        );
        // From the 9-tap branch on: the 9- and 4-tap paths.
        assert!(
            !grids.contains("g.shadow_filter") && !grids.contains("spread"),
            "the grids take no radius:\n{grids}"
        );
        for offsets in [
            "let o = vec2<f32>(f32(x), f32(y)) * g.shadow.z * 1.5;",
            "let o = (vec2<f32>(f32(i & 1), f32(i >> 1u)) - 0.5) * g.shadow.z * 1.5;",
        ] {
            assert!(grids.contains(offsets), "a texel-sized grid: {offsets}");
        }
    }

    /// The 16-tap disk takes its turn from the noise of its place in the shadow map (two cells
    /// per texel), not from the fragment's pixel: a pattern locked to the screen swims over the
    /// ground in a walkthrough (there is no temporal antialiasing to average it away), one
    /// locked to the map stays on it. So no `sun_shadow` caller passes a pixel, as on main. And
    /// the disk compares each tap against the receiver plane's own depth where the tap lands, a
    /// quarter texel less: not against the receiver's depth `p.z` (the taps of a slope towards
    /// the light then shade the surface), nor that slackened by the worst-case slope `r * tan`
    /// for every tap (the taps away from the light then lift real shadows off the ground). The
    /// loop itself is `shadow_disk`, the one disk both the PCF and the PCSS modes filter with.
    #[test]
    fn the_disk_is_turned_by_the_map_and_follows_the_receiver_plane() {
        let scene = include_str!("scene.wgsl");
        let branch = wgsl_fn(scene, "fn cascade_shadow(");
        let (disk, _) = branch.split_once("taps > 8.5").expect("the 9-tap branch");
        assert!(
            disk.contains("ign_sampled(uv * g.shadow.y * 2.0)"),
            "the turn comes from the place in the map:\n{disk}"
        );
        assert!(
            disk.contains("let small = 0.25 * g.texel[c] * dz_per_m;"),
            "a quarter texel of depth:\n{disk}"
        );
        let filter = wgsl_fn(scene, "fn shadow_disk(");
        assert!(
            filter.contains("z + dz - small"),
            "a tap is compared against the receiver plane's depth:\n{filter}"
        );
        assert!(
            filter.contains("for (var i = 0u; i < 16u; i++)"),
            "16 taps:\n{filter}"
        );
        assert!(
            !disk.contains("tan_theta") && !filter.contains("tan_theta"),
            "no worst-case slope bias:\n{disk}\n{filter}"
        );
        // The plane maths `receiver_plane` mirrors (the uv's y runs against the clip's y).
        for line in [
            "let slope_max = 4.0 * dz_per_m / length(a0);",
            "let plane_z = -select(1.0, max(-plane_n.z, length(plane_n.xy) / slope_max), facing);",
            "let k = select(vec2<f32>(0.0), -plane_n.xy / plane_z, facing);",
        ] {
            assert!(
                disk.contains(line),
                "the receiver plane as mirrored: {line}"
            );
        }
        // Each tap's depth on that plane, by the one line both loops share (the uv's y runs
        // against the clip's y).
        assert!(
            wgsl_fn(scene, "fn plane_dz(").contains("dot(k, vec2<f32>(2.0 * o.x, -2.0 * o.y))"),
            "the plane's depth change at a tap"
        );
        for helper in ["fn shadow_disk(", "fn shadow_blockers("] {
            assert!(
                wgsl_fn(scene, helper).contains("let dz = plane_dz(o, k);"),
                "{helper} compares against the receiver plane"
            );
        }
        for (name, src) in [
            ("scene", scene),
            ("grass", include_str!("grass.wgsl")),
            ("sea", include_str!("sea.wgsl")),
        ] {
            for line in src
                .lines()
                .filter(|l| l.contains("sun_shadow(") || l.contains("cascade_shadow("))
            {
                assert!(
                    !line.contains("frag") && !line.contains("clip"),
                    "{name}.wgsl passes no pixel to the shadow:\n{line}"
                );
            }
        }
    }

    /// The text of the WGSL function that starts at `signature`, to its closing brace.
    fn wgsl_fn<'a>(src: &'a str, signature: &str) -> &'a str {
        let start = src
            .find(signature)
            .unwrap_or_else(|| panic!("{signature} not in the shader"));
        let body = &src[start..];
        &body[..body.find("\n}\n").expect("the function's end")]
    }

    /// PCSS (`ShadowFilter::Pcss`; Fernando, "Percentage-Closer Soft Shadows", NVIDIA,
    /// SIGGRAPH 2005 sketch) runs in the 16-tap branch of `cascade_shadow` when
    /// `g.shadow_filter.y` is 1, and changes nothing else: a blocker search over the same turned
    /// Vogel disk as the filter, reading raw depths and taking a tap for a blocker when it lies
    /// before the receiver plane where the tap lands (as the filter's taps are compared, a
    /// quarter texel less, so a sloped receiver is not its own occluder); lit at once without a
    /// blocker; a penumbra of the receiver's distance to the blockers along the light, times the
    /// tangent of the sun's disc (`g.sun_light.w`: its cosine), no more than the filter's
    /// radius; then the one disk, `shadow_disk`, at that spread, in texels and no wider than
    /// 6, as the plain disk, and never narrower than half a texel (its floor is 1.5: at the
    /// contact that one is the plain disk's softness). The plain disk keeps its floor of 1.5
    /// texels.
    #[test]
    fn pcss_searches_blockers_on_the_disk_and_widens_with_their_distance() {
        let scene = include_str!("scene.wgsl");
        let branch = wgsl_fn(scene, "fn cascade_shadow(");
        let (disk, _) = branch.split_once("taps > 8.5").expect("the 9-tap branch");
        assert!(
            disk.contains("if g.shadow_filter.y > 0.5 {"),
            "PCSS only in mode 1:\n{disk}"
        );
        for line in [
            "let search = clamp(g.shadow_filter.x / g.texel[c], 1.5, 6.0);",
            "let blocker = shadow_blockers(c, uv, p.z, k, small, search, turn);",
            "if blocker < 0.0 {",
            "return 1.0;",
            "let tan_sun = sqrt(max(1.0 - g.sun_light.w * g.sun_light.w, 0.0)) / g.sun_light.w;",
            "let w = clamp((p.z - blocker) / dz_per_m * tan_sun, 0.0, g.shadow_filter.x);",
            "return shadow_disk(c, uv, p.z, k, small, clamp(w / g.texel[c], 0.5, 6.0), turn);",
            "let spread = clamp(g.shadow_filter.x / g.texel[c], 1.5, 6.0);",
            "return shadow_disk(c, uv, p.z, k, small, spread, turn);",
        ] {
            assert!(
                disk.contains(line),
                "the PCSS branch as written:\n{line}\n{disk}"
            );
        }
        assert_eq!(
            disk.matches("return shadow_disk(").count(),
            2,
            "one disk, for PCSS and for PCF:\n{disk}"
        );
        let search = wgsl_fn(scene, "fn shadow_blockers(");
        for line in [
            "for (var i = 0u; i < 16u; i++)",
            "let o = disk_offset(i, search, turn);",
            "let dz = plane_dz(o, k);",
            "let t = clamp(vec2<i32>((uv + o) * vec2<f32>(dims)), vec2<i32>(0), vec2<i32>(dims) - 1);",
            "let stored = textureLoad(shadow_map, t, i32(c), 0);",
            "if stored < z + dz - small {",
        ] {
            assert!(
                search.contains(line),
                "the blocker search as written:\n{line}\n{search}"
            );
        }
        // The filter walks the same disk.
        let filter = wgsl_fn(scene, "fn shadow_disk(");
        assert!(
            filter.contains("let o = disk_offset(i, spread, turn);"),
            "the filter's taps:\n{filter}"
        );
        let offset = wgsl_fn(scene, "fn disk_offset(");
        assert!(
            offset.contains("let r_texels = sqrt((f32(i) + 0.5) / 16.0) * spread;"),
            "the Vogel disk:\n{offset}"
        );
        assert!(
            offset.contains("let a = f32(i) * 2.399963 + turn;"),
            "turned:\n{offset}"
        );
        assert_eq!(
            scene.matches("disk_offset(").count(),
            3,
            "the disk's offset is defined once and walked by the search and the filter"
        );
    }

    /// Scene.wgsl `cascade_shadow`'s PCSS penumbra, mirrored: a receiver `d` metres behind its
    /// blockers along the light gets `d * tan(sun disc)` (the disc as drawn,
    /// `sky::SUN_DISC_RADIUS`), between 0 and the filter's radius.
    fn pcss_penumbra_m(receiver: f32, blocker: f32, dz_per_m: f32, max_radius_m: f32) -> f32 {
        let cos_sun = sky::SUN_DISC_RADIUS.cos();
        let tan_sun = (1.0 - cos_sun * cos_sun).max(0.0).sqrt() / cos_sun;
        ((receiver - blocker) / dz_per_m * tan_sun).clamp(0.0, max_radius_m)
    }

    /// The blocker search's depths are the shadow map's: a receiver's depth minus its blocker's,
    /// over `dz_per_m`, is the distance between the two along the light, whatever cascade is
    /// read. A pole's shadow: the receiver on the ground `s` metres from the foot and the pole
    /// at height `s * tan(elevation)` lie on one light ray (the same place in every cascade's
    /// map), `s / cos(elevation)` apart. So PCSS's penumbra there is millimetres at the foot
    /// (sharp: the contact, where the filter pass's disk is floored at half a texel) and
    /// centimetres further on, capped by the filter's radius.
    #[test]
    fn a_blockers_depth_is_its_distance_along_the_light() {
        let view = Mat4::look_at_rh(
            Vec3::new(0.0, 7.5, -3.25),
            Vec3::new(0.0, 0.0, -3.25),
            Vec3::Z,
        );
        let proj = Mat4::perspective_rh(50f32.to_radians(), 16.0 / 9.0, 0.1, 1000.0);
        let elevation = 30.0f32;
        let sun = Sun::from_angles(180.0, elevation).direction;
        let cascades = shadow::compute(view, proj, sun, 4, 2048, 80.0, None);
        let angle = elevation.to_radians();
        let (tan, cos) = (angle.tan(), angle.cos());
        let cos_sun = sky::SUN_DISC_RADIUS.cos();
        let tan_sun = (1.0 - cos_sun * cos_sun).sqrt() / cos_sun;
        for (c, m) in cascades
            .view_proj
            .iter()
            .copied()
            .enumerate()
            .take(cascades.count)
        {
            let depth = |x: Vec3| (m * x.extend(1.0)).z;
            let uv = |x: Vec3| {
                let p = m * x.extend(1.0);
                Vec2::new(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5)
            };
            // The depth row (`a2` in the shader): a metre along the light, in depth.
            let dz_per_m = Vec3::new(m.col(0).z, m.col(1).z, m.col(2).z).length();
            // Metres from the pole's foot: at the contact, mid-shadow, and far enough for the
            // cap (0.15 m is reached 17 m along the light).
            let cap = 0.15;
            for s in [0.5f32, 6.0, 20.0] {
                let receiver = Vec3::new(0.0, 0.0, -s);
                let blocker = Vec3::new(0.0, s * tan, 0.0);
                assert!(
                    (uv(receiver) - uv(blocker)).length() * 2048.0 < 0.01,
                    "cascade {c}, {s} m: the blocker is over its shadow in the map"
                );
                let along = (depth(receiver) - depth(blocker)) / dz_per_m;
                assert!(
                    (along - s / cos).abs() < 1e-3 * s,
                    "cascade {c}, {s} m: depth over dz_per_m is {along}, the distance {}",
                    s / cos
                );
                let w = pcss_penumbra_m(depth(receiver), depth(blocker), dz_per_m, cap);
                assert!(
                    (w - (s / cos * tan_sun).min(cap)).abs() < 1e-5,
                    "cascade {c}, {s} m: penumbra {w}"
                );
            }
        }
    }

    /// Scene.wgsl `cascade_shadow`'s receiver plane for the 16-tap disk, step by step as the
    /// shader has it: `k`, the depth change of the receiver's plane per unit of clip x and y
    /// (zero: no plane, for a receiver facing away from the light), and `dz_per_m`, a metre
    /// along the light in depth.
    fn receiver_plane(cascade: Mat4, n: Vec3) -> (Vec2, f32) {
        // The rows of the linear part: `vec3<f32>(m[0].x, m[1].x, m[2].x)` and so on in WGSL.
        let row = |i: usize| Vec3::new(cascade.col(0)[i], cascade.col(1)[i], cascade.col(2)[i]);
        let (a0, a1, a2) = (row(0), row(1), row(2));
        let dz_per_m = a2.length();
        let plane_n = Vec3::new(
            a0.dot(n) / a0.dot(a0),
            a1.dot(n) / a1.dot(a1),
            a2.dot(n) / a2.dot(a2),
        );
        let slope_max = 4.0 * dz_per_m / a0.length();
        let facing = plane_n.z < 0.0;
        let plane_z = -if facing {
            (-plane_n.z).max(Vec2::new(plane_n.x, plane_n.y).length() / slope_max)
        } else {
            1.0
        };
        let k = if facing {
            -Vec2::new(plane_n.x, plane_n.y) / plane_z
        } else {
            Vec2::ZERO
        };
        (k, dz_per_m)
    }

    /// The 16-tap disk compares each tap against the depth of the receiver's own plane where
    /// the tap lands (scene.wgsl `cascade_shadow`: Isidoro, "Shadow Mapping: GPU-based Tips
    /// and Techniques", GDC 2006), with the maths mirrored in `receiver_plane`. The plane point
    /// itself is found here along the light ray through the tap, with the cascade matrix's
    /// inverse: for every cascade of a garden view, banks at several slopes, suns high and low
    /// and taps out to 6 texels in eight directions, `p.z + dz` is the depth of the point of the
    /// plane that projects to `uv + o`. A receiver that faces away from the light gets no plane,
    /// and one the light grazes is clamped to the old worst-case bias: 4 m of depth per metre
    /// in the light's plane.
    #[test]
    fn a_tap_is_compared_against_the_receiver_plane() {
        let view = Mat4::look_at_rh(Vec3::new(0.0, 1.7, 0.0), Vec3::new(0.0, 1.0, 40.0), Vec3::Y);
        let proj = Mat4::perspective_rh(50f32.to_radians(), 16.0 / 9.0, 0.1, 1000.0);
        let map = 2048.0;
        let (mut exact, mut clamped, mut back) = (0, 0, 0);
        for elevation in [60.0f32, 15.0] {
            let sun = Sun::from_angles(180.0, elevation).direction;
            let cascades = shadow::compute(view, proj, sun, 4, map as u32, 80.0, None);
            for c in 0..cascades.count {
                let m = cascades.view_proj[c];
                let inv = m.inverse();
                // A bank through this point, tilted `tilt` degrees towards the sun (+Z).
                let x0 = Vec3::new(-2.0, 0.5, 35.0);
                let p = m * x0.extend(1.0);
                let uv = Vec2::new(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
                for tilt in [-13.0f32, -20.0, 0.0, 30.0, 70.0] {
                    let (sin, cos) = tilt.to_radians().sin_cos();
                    let n = Vec3::new(0.0, cos, sin);
                    let (k, dz_per_m) = receiver_plane(m, n);
                    // `tilt + elevation` is how far the light is from perpendicular to the bank:
                    // its sine is `n . sun` (not positive: the bank faces away), its cosine the sine
                    // of the angle between the normal and the light, whose tangent must stay under
                    // 4 for the plane to be taken unclamped.
                    let (to_sun, sine) = (tilt + elevation).to_radians().sin_cos();
                    if to_sun <= 0.0 {
                        assert_eq!(k, Vec2::ZERO, "cascade {c}, tilt {tilt}: no plane");
                        back += 1;
                        continue;
                    }
                    for steps in 0..8 {
                        let angle = steps as f32 * std::f32::consts::FRAC_PI_4;
                        for r_texels in [1.5, 3.0, 6.0] {
                            let dir = Vec2::new(angle.cos(), angle.sin());
                            let o = dir * (r_texels / map);
                            let dz = k.dot(Vec2::new(2.0 * o.x, -2.0 * o.y));
                            if sine / to_sun < 3.9 {
                                // The point of the plane on the light ray through `uv + o`, by
                                // depth t: x(t) = inv * (q.x, q.y, t, 1) = x(0) + t * w.
                                let q = Vec4::new(
                                    2.0 * (uv.x + o.x) - 1.0,
                                    1.0 - 2.0 * (uv.y + o.y),
                                    0.0,
                                    1.0,
                                );
                                let at = (inv * q).truncate();
                                let w = (inv * Vec4::new(0.0, 0.0, 1.0, 0.0)).truncate();
                                let t = n.dot(x0 - at) / n.dot(w);
                                let depth = (m * (at + w * t).extend(1.0)).z;
                                assert!(
                                    (depth - t).abs() < 1e-6,
                                    "clip z is the depth: {depth} vs {t}"
                                );
                                assert!(
                                    (p.z + dz - t).abs() < 1e-5,
                                    "cascade {c}, sun {elevation}, tilt {tilt}, tap \
                                     {r_texels} texels at {angle:.2}: p.z + dz {} vs the plane's {t}",
                                    p.z + dz
                                );
                                exact += 1;
                            } else {
                                // Clamped as the worst-case bias was: at most 4 m of depth per
                                // metre in the light's plane, so `4 * r * dz_per_m` at r metres,
                                // and no less straight down the slope.
                                let bound = 4.0 * (r_texels * cascades.texel[c]) * dz_per_m;
                                assert!(
                                    dz.abs() <= bound * (1.0 + 1e-4),
                                    "cascade {c}, sun {elevation}, tilt {tilt}: dz {dz} over {bound}"
                                );
                                if dir.x.abs() < 0.5 {
                                    assert!(
                                        (dz.abs() - bound).abs() <= 1e-4 * bound,
                                        "cascade {c}, sun {elevation}, tilt {tilt}: the clamp is \
                                         reached straight down the slope: {dz} vs {bound}"
                                    );
                                }
                                clamped += 1;
                            }
                        }
                    }
                }
            }
        }
        // 2 suns x 4 cascades x 5 banks x 8 directions x 3 radii: 864 taps checked, 768 within
        // the clamp (exact) and 96 on the bank the low sun grazes (clamped); `back` counts the
        // 4 (sun, cascade, bank) cases facing away from the light (no plane), whose taps are
        // skipped.
        assert_eq!((exact, clamped, back), (768, 96, 4));
    }

    /// `Quality::sanitized` is how any quality enters the renderer (`set_quality`, still.rs
    /// `render_offscreen` and `photo_begin`): the fields it indexes or divides by in range, the
    /// shadow filter sanitized, and a preset its own sanitized self.
    #[test]
    fn a_quality_is_sanitized_whenever_it_enters_the_renderer() {
        let out_of_range = Quality {
            cascades: 0,
            impostor_resolution: 1,
            render_scale: f32::NAN,
            shadow_filter: ShadowFilter::Pcf {
                taps: 7,
                radius_m: f32::NAN,
            },
            ..Quality::HIGH
        };
        let clean = out_of_range.sanitized();
        assert_eq!(
            (
                clean.cascades,
                clean.impostor_resolution,
                clean.render_scale
            ),
            (1, 64, 1.0)
        );
        assert_eq!(
            clean.shadow_filter,
            ShadowFilter::Pcf {
                taps: 9,
                radius_m: DEFAULT_SHADOW_RADIUS_M
            }
        );
        assert_eq!(
            Quality {
                cascades: 4,
                impostor_resolution: 1024,
                render_scale: 1.0,
                ..Quality::LOW
            },
            Quality {
                cascades: 99,
                impostor_resolution: 9999,
                render_scale: 3.0,
                ..Quality::LOW
            }
            .sanitized()
        );
        // Idempotent, and a preset is its own sanitized self: set again, it changes nothing.
        assert_eq!(clean.sanitized(), clean);
        for (name, q) in Quality::PRESETS {
            assert_eq!(q.sanitized(), q, "{name}");
        }
    }

    #[test]
    fn globals_match_the_shader_layout() {
        // Read the layout from the WGSL itself, so a field added on one side only fails here.
        let module = naga::front::wgsl::parse_str(COMMON).unwrap();
        let (span, members) = module
            .types
            .iter()
            .find_map(|(_, t)| match &t.inner {
                naga::TypeInner::Struct { members, span }
                    if t.name.as_deref() == Some("Globals") =>
                {
                    Some((*span, members.clone()))
                }
                _ => None,
            })
            .expect("Globals in common.wgsl");
        let sh = members.iter().find(|m| m.name.as_deref() == Some("sh"));
        assert_eq!(u64::from(sh.unwrap().offset), SH_OFFSET);
        assert_eq!(SH_OFFSET % 16, 0, "SH copy needs 16-byte texel alignment");
        assert_eq!(globals_size(), u64::from(span));
        assert_eq!(globals_size(), SH_OFFSET + 144);
    }

    /// The straight-line SH product in scene.wgsl (`sh_product9`) is the triple-product table:
    /// each `T * (v[j] * c[k] ...)` term against the integral of the basis, found here by
    /// quadrature, and no non-zero integral left out.
    #[test]
    fn sky_product_matches_the_triple_product() {
        let basis = |x: f64, y: f64, z: f64| {
            [
                0.282_095,
                0.488_603 * y,
                0.488_603 * z,
                0.488_603 * x,
                1.092_548 * x * y,
                1.092_548 * y * z,
                0.315_392 * (3.0 * z * z - 1.0),
                1.092_548 * x * z,
                0.546_274 * (x * x - y * y),
            ]
        };
        // Midpoint in the cosine and round the axis, fine enough for these polynomials.
        let (rows, cols) = (400, 64);
        let mut t = [[[0.0f64; 9]; 9]; 9];
        for a in 0..rows {
            let mu = -1.0 + (a as f64 + 0.5) * 2.0 / rows as f64;
            let s = (1.0 - mu * mu).sqrt();
            for b in 0..cols {
                let phi = (b as f64 + 0.5) * std::f64::consts::TAU / cols as f64;
                let y = basis(s * phi.cos(), s * phi.sin(), mu);
                let w = 2.0 / rows as f64 * std::f64::consts::TAU / cols as f64;
                for i in 0..9 {
                    for j in 0..9 {
                        for k in 0..9 {
                            t[i][j][k] += w * y[i] * y[j] * y[k];
                        }
                    }
                }
            }
        }
        let src = include_str!("scene.wgsl");
        let body = &src[src.find("fn sh_product9").unwrap()..];
        let body = &body[..body.find("return w;").unwrap()];
        let mut seen = std::collections::HashSet::new();
        let mut i = 0usize;
        for line in body.lines().map(str::trim) {
            let line = line.trim_start_matches("+ ").trim_end_matches(';');
            let rest = match line.strip_prefix("w[") {
                Some(r) => {
                    i = r[..1].parse().unwrap();
                    r[r.find("= ").unwrap() + 2..].to_string()
                }
                None if line.contains("* v[") || line.contains("* (v[") => line.to_string(),
                None => continue,
            };
            let coef: f64 = rest[..rest.find(" *").unwrap()].parse().unwrap();
            let idx: Vec<usize> = rest
                .match_indices("v[")
                .map(|(p, _)| rest[p + 2..p + 3].parse().unwrap())
                .collect();
            let jdx: Vec<usize> = rest
                .match_indices("c[")
                .map(|(p, _)| rest[p + 2..p + 3].parse().unwrap())
                .collect();
            let (j, k) = (idx[0], jdx[0]);
            assert!(
                (coef - t[i][j][k]).abs() < 2e-4,
                "w[{i}] v[{j}] c[{k}]: {coef} vs {}",
                t[i][j][k]
            );
            seen.insert((i, j.min(k), j.max(k)));
        }
        for (i, ti) in t.iter().enumerate() {
            for (j, tij) in ti.iter().enumerate() {
                for (k, v) in tij.iter().enumerate().skip(j) {
                    if v.abs() > 1e-3 {
                        assert!(seen.contains(&(i, j, k)), "T[{i}][{j}][{k}] missing");
                    }
                }
            }
        }
    }

    #[test]
    fn culling_keeps_visible_boxes() {
        let vp = Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0)
            * Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let b = |c: Vec3| (c - Vec3::ONE, c + Vec3::ONE);
        assert!(!outside(vp, b(Vec3::new(0.0, 0.0, -10.0)), true));
        assert!(outside(vp, b(Vec3::new(0.0, 0.0, 10.0)), true)); // behind
        assert!(outside(vp, b(Vec3::new(50.0, 0.0, -10.0)), true)); // right
        assert!(!outside(vp, b(Vec3::new(0.0, 0.0, 0.5)), true)); // around the eye
    }

    #[test]
    fn water_absorption_curve() {
        assert!(
            (water_transmittance(0.0, 0.5) - 1.0).abs() < 1e-6,
            "no water, no tint"
        );
        let mut last = 1.0;
        for i in 1..50 {
            let t = water_transmittance(i as f32 * 0.05, 0.3);
            assert!(t < last && t > 0.0, "monotonic");
            last = t;
        }
        // A 10 cm edge stays mostly clear, 2 m of murky water nearly opaque.
        assert!(water_transmittance(0.1, 0.3) > 0.85);
        assert!(water_transmittance(2.0, 1.0) < 0.01);
        assert!(water_transmittance(1.0, 0.0) > water_transmittance(1.0, 1.0));
    }

    #[test]
    fn material_defaults_keep_the_old_meaning() {
        let m = Material::default();
        assert_eq!(
            (m.base_color, m.metallic, m.roughness),
            ([0.8, 0.8, 0.8, 1.0], 0.0, 0.7)
        );
        assert_eq!(m.variant(), 0);
        assert!(!m.transparent());
        let glass = Material {
            surface: Surface::Glass,
            alpha_mode: AlphaMode::Cutout { threshold: 0.5 },
            ..m
        };
        assert!(glass.transparent() && !glass.cutout());
    }
}
