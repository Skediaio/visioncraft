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
//!    cached far cascade); with 3+ cascades the middle ones alternate frames
//! 2. depth pre-pass, `Depth32Float` (4x MSAA when enabled); cutout
//!    materials write their coverage through the sample mask
//! 3. depth resolve: sample 0 -> linear view depth, `R32Float`
//! 4. AO + contact shadows at half (ULTRA: full) render size, `Rg8Unorm`, depth-aware blur (H, V)
//! 5. planar reflection of the largest visible water plane (when enabled):
//!    the scene mirrored, coarser LODs, no screen-space effects,
//!    `Rgba16Float` + `Depth32Float` at render size / divisor
//! 6. forward PBR, `Rgba16Float` (MSAA, resolved into a 1x target): opaque
//!    and cutout geometry (cutout with an Equal depth test), the infinite
//!    ground, the sky, then glass, water and alpha-blended materials sorted
//!    back to front (not in the depth pre-pass, so not in AO and not
//!    depth-writing); distance haze on everything
//! 7. bloom: 4 half-size `Rgba16Float` levels down, then back up
//! 8. AgX tone map (bloom mixed in) to 8-bit sRGB-encoded values: straight
//!    into the output, or into an LDR target at render size for
//! 9. FXAA (when enabled) and
//! 10. the upscale to the output size with RCAS sharpening (render scale < 1)
//! 11. overlay lines and points, alpha-blended onto the output (`Rgba8UnormSrgb`)
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
use std::num::NonZeroU64;

use glam::{Mat4, Vec2, Vec3, Vec4};

mod atmosphere;
pub mod bc;
pub use bc::UploadJob;
pub mod demo;
mod env;
pub mod hardware;
mod hdri;
mod impostor;
pub mod lights;
pub mod lod;
mod sea;
pub mod shadow;
pub mod sky;
mod stats;
mod still;
mod terrain;
mod texture;
mod timing;
pub mod tonemap;

pub use atmosphere::{Atmosphere, fog_transmittance};
pub use hardware::{GpuKind, HardwareProbe, Score, calibrate, relative_cost, starting_quality};
pub use hdri::{HdriCalibration, HdriImage, OVERCAST_LUX, calibrate_hdri, estimate_sun};
pub use impostor::{ImpostorId, atlas_layout, impostor_views};
pub use lights::{Light, LightKind};
pub use sea::Sea;
pub use sky::Sun;
pub use stats::{RenderStats, TARGET_MS, ULTRA_HEADROOM, suggest_quality};
pub use still::Photo;
pub use still::Still;
pub use terrain::{TerrainChunk, TerrainLayer};
pub use texture::{ColorSpace, TextureDesc, TextureId, mip_count};

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const LINEAR_DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
const OCCLUSION: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;
const MSAA_SAMPLES: u32 = 4;
/// Uniform slot size: WebGPU's default `minUniformBufferOffsetAlignment`.
const SLOT: u64 = 256;
const VERTEX_STRIDE: u64 = 44; // position, normal, face id, uv, wind
const MATERIAL_SIZE: u64 = 80;
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
/// Solar illuminance at the top of the atmosphere (lux; Filament's
/// documentation, "Physically based units"), in kcd/m2 per unit of the
/// physical sky's LUTs.
const SUN_TOP_KLX: f32 = 128.0;
/// Instances are grouped into square ground cells of this size (metres) so
/// shadow cascades and the camera can skip the ones they do not see, and
/// levels of detail switch per cell.
const CLUSTER_CELL: f32 = 8.0;
/// `set_time` wraps here (seconds) to keep f32 precision in the shaders.
const TIME_WRAP: f64 = 4096.0;
/// Byte offset of the SH in the globals (written by the GPU, see env.rs).
const SH_OFFSET: u64 = 9 * 64 + 29 * 16;

const COMMON: &str = include_str!("common.wgsl");
const SKY: &str = include_str!("sky.wgsl");
const POST: &str = include_str!("post.wgsl");
/// Every shader module as (name, source parts); WGSL has no includes, so the
/// shared declarations are prepended. Also validated with naga in a unit test.
const SHADERS: [(&str, &[&str]); 12] = [
    (
        "scene",
        &[
            COMMON,
            include_str!("scene.wgsl"),
            include_str!("terrain.wgsl"),
            include_str!("impostor.wgsl"),
            include_str!("sea.wgsl"),
        ],
    ),
    ("sky", &[COMMON, SKY]),
    ("depth", &[COMMON, include_str!("depth.wgsl")]),
    ("ssao", &[COMMON, include_str!("ssao.wgsl")]),
    ("blur", &[COMMON, include_str!("blur.wgsl")]),
    ("tonemap", &[COMMON, POST, include_str!("tonemap.wgsl")]),
    ("overlay", &[COMMON, include_str!("overlay.wgsl")]),
    ("env", &[COMMON, SKY, include_str!("env.wgsl")]),
    ("mip", &[include_str!("mip.wgsl")]),
    (
        "atmosphere",
        &[COMMON, SKY, include_str!("atmosphere.wgsl")],
    ),
    ("post", &[COMMON, POST]),
    ("dilate", &[include_str!("dilate.wgsl")]),
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
            emissive: [0.0; 3],
            emissive_strength: 0.0,
            projection: Projection::Uv,
            relief: 1.0,
            gloss: 0.0,
            weathering: 0.0,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
            foliage: false,
            surface: Surface::Standard,
            variation: 0.0,
            stripes: 0,
        }
    }
}

impl Material {
    fn transparent(&self) -> bool {
        self.surface != Surface::Standard || self.alpha_mode == AlphaMode::Blend
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
            self.occlusion_map,
        ]
    }
}

/// Wind for the vegetation animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wind {
    /// Where the wind blows to (world; only the horizontal part is used).
    pub direction: Vec3,
    /// 0 calm, 1 a moderate breeze, 2 strong wind (relative, not m/s).
    pub strength: f32,
    /// 0..1: how much the strength varies in travelling gusts.
    pub gustiness: f32,
}

impl Default for Wind {
    fn default() -> Self {
        Self {
            direction: Vec3::X,
            strength: 0.0,
            gustiness: 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialId(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(u32);

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
    /// 3x3 PCF taps for sun shadows (otherwise 2x2: softer edges cost ~2x).
    pub soft_shadows: bool,
    /// Ambient occlusion and contact shadows at the full render size
    /// (otherwise half size, upsampled depth-aware).
    pub ao_full_resolution: bool,
    /// Side of one view in impostor atlases baked from now on (texels,
    /// 64..=1024); impostors baked earlier keep theirs.
    pub impostor_resolution: u32,
}

// `render_scale` is clamped to 0.5..=1 everywhere it is used, never NaN, so
// equality is total (presets are compared with `==`).
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
        soft_shadows: false,
        ao_full_resolution: false,
        impostor_resolution: 256,
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
        soft_shadows: false,
        ao_full_resolution: false,
        impostor_resolution: 256,
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
        soft_shadows: true,
        ao_full_resolution: false,
        impostor_resolution: 256,
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
        soft_shadows: true,
        ao_full_resolution: true,
        impostor_resolution: 512,
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
}

impl Default for Ground {
    fn default() -> Self {
        Self {
            height: 0.0,
            color: [0.1, 0.16, 0.05],
            roughness: 0.95,
            variation: 0.6,
        }
    }
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
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    bounds: Option<Bounds>,
    /// Any vertex with a bend weight: bounds get padded for the sway.
    wind: bool,
    /// Terrain chunks: eight u8 layer weights per vertex.
    weights: Option<wgpu::Buffer>,
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
    /// Sorted by `CLUSTER_CELL` so each cluster is a contiguous range.
    transforms: Vec<Mat4>,
    instances: wgpu::Buffer,
    bounds: Option<Bounds>,
    clusters: Vec<Cluster>,
    /// The baked impostor drawn as the last level, and its draw slot.
    impostor: Option<(ImpostorId, u32)>,
    /// Metres before the last level's distance over which instances dither
    /// out (0: they are culled at once).
    cull_fade: f32,
    /// The impostor's end follows `set_impostor_reach` (small plants).
    follows_reach: bool,
    /// Casts shadows with the level it is drawn with (`set_item_shadow_exact`).
    shadow_exact: bool,
}

/// Instances in one ground cell: culled as a unit, drawn as one range, one
/// level of detail.
struct Cluster {
    range: std::ops::Range<u32>,
    bounds: Option<Bounds>,
    /// View level of detail drawn (`lods.len()` = culled by distance;
    /// `LOD_UNSET` before the first frame).
    lod: u8,
    /// The level the distance asks for (with hysteresis); differs from
    /// `lod` while a hand-over dissolves.
    target: u8,
    /// Front-to-back sort keys seen from the camera and from the sun.
    key: [f32; 2],
    /// Frames left of a dissolve between the last mesh level (`lod`) and
    /// the impostor (or nothing, for a cull fade); 0 when settled.
    fade: u8,
    /// The dissolve goes towards the far side (impostor or culled).
    to_far: bool,
    /// Dissolving this frame: both sides draw, dithered by `fade_slot`.
    fading: bool,
    /// First of this frame's fade slots (one per part, then the impostor).
    fade_slot: u32,
}

const LOD_UNSET: u8 = u8::MAX;
/// Frames a hand-over between the last mesh level and the impostor (or a
/// cull fade) dissolves over: the dither shows only while it runs, never
/// as a steady stipple on a parked camera.
const FADE_FRAMES: u8 = 16;
/// Draw slots for this frame's dissolving clusters; more dissolving at once
/// switch without one.
const FADE_SLOTS: usize = 1024;

struct FarCache {
    eye: Vec3,
    proj: Mat4,
    matrix: Mat4,
}

struct GpuMaterial {
    buffer: wgpu::Buffer,
    bind: wgpu::BindGroup,
    cpu: Material,
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
    ground: wgpu::RenderPipeline,
    terrain: wgpu::RenderPipeline,
    /// Impostor quads: depth pre-pass (alpha test, dither), forward (Equal).
    impostor: [wgpu::RenderPipeline; 2],
    /// The last mesh level handing over to an impostor (by double-sided).
    depth_fade: [wgpu::RenderPipeline; 2],
    sky: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
    sea: sea::SeaPipelines,
}

struct Pipelines {
    /// Opaque, cutout, glass (stippled).
    shadow: [wgpu::RenderPipeline; 3],
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
    /// light of the next sample gathers.
    prev_hdr: wgpu::Texture,
    linear_depth: wgpu::TextureView,
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
    reflection: wgpu::TextureView,
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
    bytes: u64,
}

/// Which geometry pass `draw_items` records.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// `far`: the cached, camera-independent last cascade; `coarse`: a
    /// middle cascade (draws the coarsest level of detail).
    Shadow {
        far: bool,
        coarse: bool,
    },
    Depth,
    Forward,
    Reflection,
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

/// Per-frame counters for `stats` (`draw_items` takes `&self`).
#[derive(Default)]
struct Counters {
    draws: Cell<u32>,
    triangles: Cell<u64>,
    instances: Cell<u32>,
    /// Draws per pass group, closed by `split`.
    passes: std::cell::RefCell<Vec<(&'static str, u32)>>,
    mark: Cell<u32>,
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

    /// Attributes the draws since the last split to `label`.
    fn split(&self, label: &'static str) {
        let n = self.draws.get() - self.mark.replace(self.draws.get());
        let mut passes = self.passes.borrow_mut();
        match passes.iter_mut().find(|(l, _)| *l == label) {
            Some(e) => e.1 += n,
            None => passes.push((label, n)),
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
    targets: Targets,
    size: (u32, u32),
    quality: Quality,

    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    /// The mirrored camera's globals for the reflection pass.
    reflect_globals: wgpu::Buffer,
    reflect_globals_bind: wgpu::BindGroup,
    globals_only_bind: wgpu::BindGroup,
    /// Globals, sampler and sky source for the environment passes.
    sky_bind: wgpu::BindGroup,
    cascade_buffer: wgpu::Buffer,
    cascade_bind: wgpu::BindGroup,
    draw_buffer: wgpu::Buffer,
    draw_bind: wgpu::BindGroup,
    draw_capacity: usize,
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
    ground_material: GpuMaterial,
    /// A scene material the ground is drawn with (`set_ground_material`).
    ground_look: Option<MaterialId>,
    ground_slot: u32,
    terrain: terrain::TerrainGpu,
    sea: sea::SeaGpu,
    baker: impostor::Baker,
    impostors: Vec<Option<impostor::GpuImpostor>>,
    bloom_strength: f32,
    /// The display look (`set_grade`): contrast power and saturation.
    grade: (f32, f32),
    /// The camera is in a room (`set_indoors`): glass mirrors the room, not the sky.
    indoors: bool,
    /// The procedural cloud layer of the analytic skies (`set_clouds`).
    clouds: Clouds,
    /// Snow cover 0..1 (`set_snow`), and the top-down depth map of what lies over the ground
    /// around the eye (the last shadow layer): where it shows something overhead, no snow.
    snow: f32,
    snow_cover: Option<Mat4>,
    /// A photo's sample (`still::Photo`): the shadow's light direction moved within the sun's
    /// disc, and the noise seed of ambient occlusion; zero for live frames.
    shadow_jitter: Vec3,
    photo_seed: f32,
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

    meshes: Vec<Option<GpuMesh>>,
    materials: Vec<Option<GpuMaterial>>,
    textures: Vec<Option<texture::GpuTexture>>,
    items: Vec<Option<Item>>,
    bounds_dirty: bool,
    scene_bounds: Option<Bounds>,

    view: Mat4,
    proj: Mat4,
    eye: Vec3,
    sun: Sun,
    sky: sky::Sky,
    ev100: f32,
    shadow_distance: f32,
    /// Multiplies every level-of-detail distance (with the quality's percentage).
    view_distance: f32,
    /// Multiplies how far impostors draw (their last distance only).
    impostor_reach: f32,
    wind: Wind,
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
    frame: u64,
    /// Depth-tested and on-top segments; kept so either list can be
    /// replaced alone.
    overlay_cpu: (Vec<LineVertex>, Vec<LineVertex>),
    overlay: Option<wgpu::Buffer>,
    overlay_count: u32,
    overlay_on_top: u32,
    timer: Option<timing::Timer>,
    counters: Counters,
    last_stats: RenderStats,
}

impl Renderer {
    /// `format` is the output texture's format: `Rgba8UnormSrgb`, or
    /// `Rgba8Unorm` for sRGB-encoded bytes in a plain texture (what egui
    /// expects of user textures). Either way the passes write sRGB-encoded
    /// values (the overlay blends through an sRGB view, in hardware).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layouts = Layouts::new(device);
        let modules: Vec<_> = SHADERS
            .iter()
            .map(|(name, parts)| {
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(name),
                    source: wgpu::ShaderSource::Wgsl(parts.concat().into()),
                })
            })
            .collect();
        let quality = Quality::HIGH;
        let pipelines = Pipelines::new(device, &layouts, &modules, format);
        let sampled = SampledPipelines::new(device, &layouts, &modules, quality.samples());

        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = uniform("globals", globals_size());
        let reflect_globals = uniform("reflection globals", globals_size());
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
        let draw_buffer = uniform("draws", SLOT * draw_capacity as u64);
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
        );
        let reflect_globals_bind = globals_bind_group(
            device,
            &layouts.globals,
            &reflect_globals,
            &env.view,
            &env_sampler,
            &light_buffer,
            &grid_buffer,
            &physical.sky_view,
        );
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
            false,
        );
        let mipmapper = texture::Mipmapper::new(device, module(&modules, "mip"));
        let sun = Sun::from_angles(180.0, 45.0);
        let sky = sky::Sky::new(sun.direction);
        let ground_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ground material"),
            size: MATERIAL_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // The ground's material has no maps: white textures, repeat sampler.
        let ground_entries: Vec<_> = (0..13)
            .map(|b| wgpu::BindGroupEntry {
                binding: b,
                resource: match b {
                    0 => ground_buffer.as_entire_binding(),
                    1..=6 => wgpu::BindingResource::TextureView(&white),
                    _ => wgpu::BindingResource::Sampler(&repeat_sampler),
                },
            })
            .collect();
        let ground_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ground material"),
            layout: &layouts.material,
            entries: &ground_entries,
        });
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
            modules,
            pipelines,
            sampled,
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
            ground_material: GpuMaterial {
                buffer: ground_buffer,
                bind: ground_bind,
                cpu: Material::default(),
            },
            ground_slot: 0,
            terrain: terrain::TerrainGpu::new(device, queue, &layouts_terrain),
            sea: sea::SeaGpu::new(device, &layouts_sea),
            baker,
            impostors: Vec::new(),
            bloom_strength: 0.025,
            grade: (1.0, 1.0),
            indoors: false,
            clouds: Clouds::default(),
            snow: 0.0,
            snow_cover: None,
            shadow_jitter: Vec3::ZERO,
            photo_seed: 0.0,
            photo_pipes: None,
            light_buffer,
            grid_buffer,
            grid,
            light_count: 0,
            lights_cpu: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            upload_belt: wgpu::util::StagingBelt::new(device.clone(), UPLOAD_CHUNK),
            meshes: Vec::new(),
            materials: Vec::new(),
            textures: Vec::new(),
            items: Vec::new(),
            bounds_dirty: false,
            scene_bounds: None,
            view: Mat4::look_at_rh(Vec3::new(0.0, 1.7, 10.0), Vec3::ZERO, Vec3::Y),
            proj: Mat4::perspective_rh(1.0, 1.0, 0.1, 1000.0),
            eye: Vec3::new(0.0, 1.7, 10.0),
            sun,
            sky,
            ev100: 15.0,
            shadow_distance: 80.0,
            view_distance: 1.0,
            impostor_reach: 1.0,
            wind: Wind::default(),
            time: 0.0,
            clock: 0.0,
            clay: false,
            highlight: None,
            far_cache: None,
            cascade_rendered: [None; shadow::MAX_CASCADES],
            frame: 0,
            overlay_cpu: (Vec::new(), Vec::new()),
            overlay: None,
            overlay_count: 0,
            overlay_on_top: 0,
            timer: timing::Timer::new(device, queue),
            counters: Counters::default(),
            last_stats: RenderStats::default(),
        };
        renderer.ground_slot = renderer.alloc_slot(MeshId(u32::MAX - 1));
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

    pub fn set_quality(&mut self, quality: Quality) {
        let quality = Quality {
            cascades: quality.cascades.clamp(1, shadow::MAX_CASCADES as u32),
            impostor_resolution: quality.impostor_resolution.clamp(64, 1024),
            render_scale: if quality.render_scale.is_nan() {
                1.0
            } else {
                quality.render_scale.clamp(0.5, 1.0)
            },
            ..quality
        };
        if quality == self.quality {
            return;
        }
        if quality.msaa != self.quality.msaa {
            self.sampled = SampledPipelines::new(
                &self.device,
                &self.layouts,
                &self.modules,
                quality.samples(),
            );
        }
        self.quality = quality;
        self.rebuild_targets();
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

    pub fn set_sun(&mut self, sun: &Sun) {
        let sun = Sun {
            direction: sun.direction.normalize(),
            ..*sun
        };
        if sun == self.sun {
            return; // keeps the cached cascade, the ambient and the environment
        }
        self.sun = sun;
        self.invalidate_shadows();
        self.sky = sky::Sky::new(self.sun.direction);
        self.update_ambient(false);
    }

    /// Camera exposure as EV at ISO 100. Manual only: the renderer never
    /// adapts it. Sensible values for this sky model and lights:
    /// sunny noon 15, overcast or sun low (10 deg) 13-14, sunset 11-12,
    /// dusk (sun 2-4 deg below the horizon) 7-9, late dusk with garden
    /// lights 4-6, night lit by garden lamps 1-3, moonlight alone -2..0.
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
        self.wind = *wind;
    }

    /// Animation time in seconds (wind, water). Wraps every 4096 s.
    pub fn set_time(&mut self, seconds: f64) {
        self.time = seconds.rem_euclid(TIME_WRAP) as f32;
        self.clock = seconds;
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
            self.queue.write_buffer(
                &self.ground_material.buffer,
                0,
                &material_bytes(&material, 0),
            );
            self.ground_material.cpu = material;
        }
        self.update_ambient(true);
    }

    /// Draws the endless ground with `material` (its maps, scale and variation) instead of the
    /// plain colour of `set_ground`, which still lights the environment below the horizon.
    pub fn set_ground_material(&mut self, material: Option<MaterialId>) {
        self.ground_look = material;
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

    /// The cloud layer over the analytic skies (an HDRI brings its own). It shows in the sky,
    /// in reflections and in the ambient light, and takes its share of the sun.
    /// Snow over the up-facing surfaces, 0 (none) to 1, except where something lies over them.
    pub fn set_snow(&mut self, amount: f32) {
        self.snow = amount.clamp(0.0, 1.0);
    }

    pub fn set_clouds(&mut self, clouds: &Clouds) {
        if *clouds != self.clouds {
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
        for (buffer, bind) in [
            (&self.globals, &mut self.globals_bind),
            (&self.reflect_globals, &mut self.reflect_globals_bind),
        ] {
            *bind = globals_bind_group(
                &self.device,
                &self.layouts.globals,
                buffer,
                &self.env.view,
                &self.env_sampler,
                &self.light_buffer,
                &self.grid_buffer,
                sky_view,
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
        let gpu = self.upload_mesh(mesh)?;
        self.meshes.push(Some(gpu));
        Ok(MeshId(self.meshes.len() as u32 - 1))
    }

    /// Replaces a mesh in place: every item drawing it picks up the new data.
    pub fn replace_mesh(&mut self, id: MeshId, mesh: &Mesh) -> Result<(), String> {
        let slot = self.meshes.get_mut(id.0 as usize).ok_or("unknown mesh")?;
        if slot.is_none() {
            return Err("mesh was removed".into());
        }
        let gpu = upload_mesh(&self.device, &self.queue, mesh)?;
        self.meshes[id.0 as usize] = Some(gpu);
        self.bounds_dirty = true;
        Ok(())
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
        match self.meshes.get(id.0 as usize) {
            Some(Some(_)) => {}
            Some(None) => return Err("mesh was removed".into()),
            None => return Err("unknown mesh".into()),
        }
        let gpu = self.upload_terrain(chunk)?;
        self.meshes[id.0 as usize] = Some(gpu);
        self.bounds_dirty = true;
        Ok(())
    }

    fn upload_terrain(&self, chunk: &TerrainChunk) -> Result<GpuMesh, String> {
        let weights = terrain::weight_bytes(chunk)?;
        let mut gpu = upload_mesh(
            &self.device,
            &self.queue,
            &Mesh {
                positions: chunk.positions,
                normals: chunk.normals,
                indices: chunk.indices,
                ..Default::default()
            },
        )?;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain weights"),
            size: (weights.len() as u64).max(8),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buffer, 0, &weights);
        gpu.weights = Some(buffer);
        Ok(gpu)
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

    /// The buffers and bind groups of `parts`, and the bounds to bake (given, else theirs).
    #[allow(clippy::type_complexity)]
    fn bake_list(
        &self,
        parts: &[(MeshId, MaterialId)],
        bounds: Option<(Vec3, Vec3)>,
    ) -> Result<
        (
            Vec<(&wgpu::Buffer, &wgpu::Buffer, u32, &wgpu::BindGroup)>,
            (Vec3, Vec3),
        ),
        String,
    > {
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
            list.push((&m.vertices, &m.indices, m.index_count, &mat.bind));
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

    pub fn set_item_impostor(
        &mut self,
        item: ItemId,
        impostor: ImpostorId,
        max_distance: f32,
    ) -> Result<(), String> {
        if !matches!(self.impostors.get(impostor.0 as usize), Some(Some(_))) {
            return Err("unknown or removed impostor".into());
        }
        let slot = self.alloc_slot(MeshId(u32::MAX - 2));
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
        if let Some(slot) = self.meshes.get_mut(id.0 as usize) {
            *slot = None;
            self.bounds_dirty = true;
        }
    }

    fn upload_mesh(&self, mesh: &Mesh) -> Result<GpuMesh, String> {
        upload_mesh(&self.device, &self.queue, mesh)
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
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("material"),
            size: MATERIAL_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = self.material_bind(&buffer, material);
        self.materials.push(Some(GpuMaterial {
            buffer,
            bind,
            cpu: *material,
        }));
        let id = MaterialId(self.materials.len() as u32 - 1);
        self.set_material(id, material);
        id
    }

    fn material_bind(&self, buffer: &wgpu::Buffer, material: &Material) -> wgpu::BindGroup {
        let maps = material.maps().map(|id| {
            match id.and_then(|id| self.textures.get(id.0 as usize)?.as_ref()) {
                Some(t) => (
                    &t.view,
                    if t.repeat {
                        &self.repeat_sampler
                    } else {
                        &self.clamp_sampler
                    },
                ),
                None => (&self.white, &self.repeat_sampler),
            }
        });
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
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
            label: Some("material"),
            layout: &self.layouts.material,
            entries: &entries,
        })
    }

    pub fn set_material(&mut self, id: MaterialId, material: &Material) {
        let Some(Some(m)) = self.materials.get(id.0 as usize) else {
            return;
        };
        let bind = self.material_bind(&m.buffer, material);
        let present = |t: Option<TextureId>| {
            t.and_then(|t| self.textures.get(t.0 as usize)?.as_ref())
                .is_some()
        };
        let bits = material
            .maps()
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, t)| acc | ((present(*t) as u32) << i));
        self.queue
            .write_buffer(&m.buffer, 0, &material_bytes(material, bits));
        if let Some(Some(m)) = self.materials.get_mut(id.0 as usize) {
            m.bind = bind;
            m.cpu = *material;
        }
    }

    /// Frees a material. Items still using it draw nothing; the id is not
    /// reused.
    pub fn remove_material(&mut self, id: MaterialId) {
        if let Some(slot) = self.materials.get_mut(id.0 as usize) {
            *slot = None;
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
        let parts: Vec<Part> = parts
            .iter()
            .map(|&(mesh, material, distance)| Part {
                mesh,
                material,
                lod: lods
                    .iter()
                    .position(|&d| d == distance.max(0.0))
                    .unwrap_or(lods.len() - 1) as u8,
                slot: self.alloc_slot(mesh),
            })
            .collect();
        let id = ItemId(self.items.len() as u32);
        self.items.push(Some(Item {
            parts,
            lods,
            transforms: Vec::new(),
            instances: self.instance_buffer(0),
            bounds: None,
            clusters: Vec::new(),
            impostor: None,
            shadow_exact: false,
            cull_fade: 0.0,
            follows_reach: false,
        }));
        self.set_instances(id, instances);
        id
    }

    fn alloc_slot(&mut self, mesh: MeshId) -> u32 {
        let slot = self.free_slots.pop().unwrap_or_else(|| {
            self.next_slot += 1;
            self.next_slot - 1
        });
        if slot as usize >= self.draw_capacity {
            self.draw_capacity = (slot as usize + 1).next_power_of_two();
            self.draw_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("draws"),
                size: SLOT * self.draw_capacity as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.draw_bind = draw_bind_group(&self.device, &self.layouts.draw, &self.draw_buffer);
            self.queue.write_buffer(
                &self.draw_buffer,
                self.ground_slot as u64 * SLOT,
                &mesh_slot(MeshId(u32::MAX - 1)),
            );
            for item in self.items.iter().flatten() {
                for p in &item.parts {
                    self.queue.write_buffer(
                        &self.draw_buffer,
                        p.slot as u64 * SLOT,
                        &mesh_slot(p.mesh),
                    );
                }
            }
        }
        self.queue
            .write_buffer(&self.draw_buffer, slot as u64 * SLOT, &mesh_slot(mesh));
        slot
    }

    pub fn set_instances(&mut self, id: ItemId, instances: &[Mat4]) {
        let grow = match self.items.get(id.0 as usize) {
            Some(Some(item)) => item.instances.size() < (instances.len() as u64 * 64).max(64),
            _ => return,
        };
        let buffer = grow.then(|| self.instance_buffer(instances.len()));
        let Some(Some(item)) = self.items.get_mut(id.0 as usize) else {
            return;
        };
        if let Some(buffer) = buffer {
            item.instances = buffer;
        }
        // Group by ground cell. This reorders the instances on the GPU; fine
        // while nothing reads `instance_index` (per-instance data later must
        // be permuted the same way).
        let cell = |m: &Mat4| {
            (
                (m.w_axis.x / CLUSTER_CELL).floor() as i32,
                (m.w_axis.z / CLUSTER_CELL).floor() as i32,
            )
        };
        let mut sorted = instances.to_vec();
        sorted.sort_by_key(cell);
        item.clusters.clear();
        for (i, m) in sorted.iter().enumerate() {
            match item.clusters.last_mut() {
                Some(c) if cell(&sorted[c.range.start as usize]) == cell(m) => c.range.end += 1,
                _ => item.clusters.push(Cluster {
                    range: i as u32..i as u32 + 1,
                    bounds: None,
                    lod: LOD_UNSET,
                    target: LOD_UNSET,
                    key: [0.0; 2],
                    fade: 0,
                    to_far: false,
                    fading: false,
                    fade_slot: 0,
                }),
            }
        }
        let mut b = Bytes::default();
        for m in &sorted {
            b.floats(&m.to_cols_array());
        }
        self.queue.write_buffer(&item.instances, 0, &b.0);
        item.transforms = sorted;
        self.bounds_dirty = true;
    }

    /// Items drawn (for tests that nothing is left behind).
    pub fn item_count(&self) -> usize {
        self.items.iter().flatten().count()
    }

    pub fn remove_item(&mut self, id: ItemId) {
        if let Some(slot) = self.items.get_mut(id.0 as usize) {
            if let Some(item) = slot.take() {
                self.free_slots.extend(item.parts.iter().map(|p| p.slot));
                self.free_slots.extend(item.impostor.map(|(_, slot)| slot));
            }
            self.bounds_dirty = true;
        }
    }

    fn instance_buffer(&self, count: usize) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (count as u64 * 64).max(64),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
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

    /// Records every pass of a frame; returns the output view. Submit the
    /// encoder before the next `render` (GPU timings read back the previous
    /// frame's queries then).
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder) -> &wgpu::TextureView {
        #[cfg(not(target_arch = "wasm32"))]
        let cpu_start = std::time::Instant::now();
        if let Some(t) = &mut self.timer {
            t.begin();
        }
        self.counters = Counters::default();
        self.frame += 1;
        if self.bounds_dirty {
            self.invalidate_shadows(); // the scene changed
        }
        self.update_bounds();
        self.update_lods();
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
        if reuse {
            cascades.view_proj[last] = self.far_cache.as_ref().map_or(Mat4::IDENTITY, |c| c.matrix);
        } else if cached && sun_lit {
            self.far_cache = Some(FarCache {
                eye: self.eye,
                proj: self.proj,
                matrix: cascades.view_proj[last],
            });
        }
        // The near cascade every frame; with three or more, the middle ones
        // on alternate frames (sampled with the matrix they were rendered
        // with; points they no longer cover fall through to the next one).
        let mut refresh = [false; shadow::MAX_CASCADES];
        for (c, r) in refresh.iter_mut().enumerate().take(cascades.count) {
            let far = cached && c == last;
            *r = sun_lit
                && if far {
                    !reuse
                } else {
                    c == 0
                        || cascades.count < 3
                        || self.cascade_rendered[c].is_none()
                        || (self.frame + c as u64).is_multiple_of(2)
                };
            if !*r
                && !far
                && let Some(m) = self.cascade_rendered[c]
            {
                cascades.view_proj[c] = m;
            }
        }
        for (c, r) in refresh.iter().enumerate() {
            if *r {
                self.cascade_rendered[c] = Some(cascades.view_proj[c]);
            }
        }
        // The snow cover map: straight down over a square round the eye, in whole texels.
        self.snow_cover = (self.snow > 0.0)
            .then_some(self.scene_bounds)
            .flatten()
            .map(|(lo, hi)| snow_cover_matrix(self.eye, lo, hi, self.quality.shadow_resolution));
        let view_proj = self.proj * self.view;
        // The sea, seen from above, takes the planar reflection over any pond.
        let water = (self.quality.water_reflection > 0)
            .then(|| self.sea_plane().or_else(|| self.water_plane(view_proj)))
            .flatten();
        self.write_sea();
        let t = &self.targets;
        self.write_globals(
            &self.globals,
            &cascades,
            (self.view, self.proj, self.eye),
            t.size,
            water,
            None,
        );
        let mirror = water.map(mirror_matrix);
        if let (Some(m), Some(h)) = (mirror, water) {
            self.write_globals(
                &self.reflect_globals,
                &cascades,
                (self.view * m, self.proj, m.transform_point3(self.eye)),
                texture_size(&t.reflection),
                water,
                Some(h),
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
        // Settled impostors draw whole (fade 1); a buffer regrowth in
        // `alloc_slot` writes plain mesh slots, so every frame.
        for item in self.items.iter().flatten() {
            if let Some((_, slot)) = item.impostor {
                self.queue.write_buffer(
                    &self.draw_buffer,
                    slot as u64 * SLOT,
                    &fade_slot(MeshId(u32::MAX - 2), 1.0),
                );
            }
        }
        if self.sky_view_dirty && matches!(self.sky_source, SkySource::Physical) {
            self.physical
                .update(encoder, &self.globals_only_bind, self.timer.as_ref());
        }
        self.sky_view_dirty = false;
        self.env.update(
            encoder,
            &self.sky_bind,
            &[&self.globals, &self.reflect_globals],
            SH_OFFSET,
            self.timer.as_ref(),
        );

        let t = &self.targets;
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
            let far = cached && c == last;
            self.draw_items(
                &mut pass,
                cascades.view_proj[c],
                false,
                Pass::Shadow { far, coarse: c > 0 },
            );
        }
        if let (Some(m), Some(layer)) = (self.snow_cover, t.shadow_layers.last()) {
            let mut pass = depth_pass(encoder, "snow cover", layer, wgpu::StoreOp::Store, None);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(
                2,
                &self.cascade_bind,
                &[(shadow::MAX_CASCADES as u64 * SLOT) as u32],
            );
            self.draw_items(
                &mut pass,
                m,
                false,
                // The view's own levels: a bare tree's coarsest level is a few large twig
                // cards, which kept the snow off a patch of lawn under a birch.
                Pass::Shadow {
                    far: false,
                    coarse: false,
                },
            );
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
            self.draw_items(&mut pass, view_proj, true, Pass::Depth);
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
        if let Some(m) = mirror {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reflection"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.reflection,
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
            pass.set_bind_group(0, &self.reflect_globals_bind, &[]);
            pass.set_bind_group(3, &t.reflect_forward_bind, &[]);
            self.draw_items(&mut pass, view_proj * m, true, Pass::Reflection);
            // The endless ground has no hole where the terrain is dug out:
            // seen from water below its height (a pond in the lawn) it would
            // cover the sky the water mirrors.
            if self
                .ground
                .is_some_and(|g| water.is_some_and(|h| h >= g.height))
            {
                self.draw_ground(&mut pass, &self.pipelines.reflect_ground);
            }
            pass.set_pipeline(&self.pipelines.reflect_sky);
            pass.draw(0..3, 0..1);
        }
        self.counters.split("reflection");
        {
            let (view, resolve) = match &t.hdr_msaa {
                Some(msaa) => (msaa, Some(&t.hdr)),
                None => (&t.hdr, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forward"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: if resolve.is_some() {
                            wgpu::StoreOp::Discard
                        } else {
                            wgpu::StoreOp::Store
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.ts("forward"),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(3, &t.forward_bind, &[]);
            self.draw_items(&mut pass, view_proj, true, Pass::Forward);
            if self.ground.is_some() {
                self.draw_ground(&mut pass, &self.sampled.ground);
            }
            pass.set_pipeline(&self.sampled.sky);
            pass.draw(0..3, 0..1);
            self.draw_sea(&mut pass);
            self.draw_transparent(&mut pass, view_proj);
        }
        self.counters.split("forward");
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
        };
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
        &self.targets.display_view
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
        self.last_stats = RenderStats {
            draw_calls: self.counters.draws.get(),
            pass_draws: self.counters.passes.take(),
            triangles: self.counters.triangles.get(),
            instances: self.counters.instances.get(),
            visible_lights,
            texture_bytes,
            texture_breakdown,
            cpu_encode_ms,
            gpu_ms: (!gpu_passes.is_empty()).then(|| gpu_passes.iter().map(|p| p.1).sum()),
            gpu_passes,
            render_size: self.targets.size,
        };
    }

    /// The infinite ground (scene.wgsl `vs_ground`) with its own material.
    fn draw_ground(&self, pass: &mut wgpu::RenderPass, pipeline: &wgpu::RenderPipeline) {
        const RINGS: u32 = 48;
        const SEGMENTS: u32 = 64;
        pass.set_pipeline(pipeline);
        // A material of the scene (the terrain's base layer) when one is set: the ground then
        // looks like it, texture, tiling phase and variation alike, so their border vanishes.
        let bind = self
            .ground_look
            .and_then(|id| self.materials.get(id.0 as usize)?.as_ref())
            .map_or(&self.ground_material.bind, |m| &m.bind);
        pass.set_bind_group(1, bind, &[]);
        pass.set_bind_group(
            2,
            &self.draw_bind,
            &[(self.ground_slot as u64 * SLOT) as u32],
        );
        pass.draw(0..RINGS * SEGMENTS * 6, 0..1);
        self.counters.add((RINGS * SEGMENTS * 2) as u64, 1, false);
    }

    /// The height of the largest visible water surface (by the area of its
    /// visible clusters' bounds), for the planar reflection.
    fn water_plane(&self, cull: Mat4) -> Option<f32> {
        let mut best: Option<(f32, f32)> = None;
        for item in self.items.iter().flatten() {
            if item.bounds.is_none_or(|b| outside(cull, b, true)) {
                continue;
            }
            for part in &item.parts {
                let water = self
                    .materials
                    .get(part.material.0 as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|m| m.cpu.surface == Surface::Water);
                if !water {
                    continue;
                }
                let visible = item
                    .clusters
                    .iter()
                    .filter(|c| Self::cluster_lod(item, c, Pass::Forward) == Some(part.lod))
                    .filter_map(|c| c.bounds)
                    .filter(|b| !outside(cull, *b, true))
                    .reduce(union);
                if let Some((lo, hi)) = visible {
                    let area = (hi.x - lo.x) * (hi.z - lo.z);
                    if best.is_none_or(|(a, _)| area > a) {
                        best = Some((area, hi.y));
                    }
                }
            }
        }
        best.map(|(_, h)| h)
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
        // Plants with an impostor shadow with it beyond their finest level.
        if item.impostor.is_some() && matches!(pass, Pass::Shadow { .. }) && cluster.lod >= 1 {
            return Some(levels - 1);
        }
        Some(match pass {
            Pass::Depth | Pass::Forward => cluster.lod,
            Pass::Reflection
            | Pass::Shadow {
                far: false,
                coarse: false,
            } => (cluster.lod + 1).min(levels - 1),
            Pass::Shadow { .. } => levels - 1,
        })
    }

    /// Runs of consecutive clusters (adjacent in the instance buffer) that
    /// pass `keep` and are inside `cull`, one draw each, ordered front to
    /// back by their nearest cluster (so early depth testing rejects hidden
    /// foliage layers while a run of cells stays one draw).
    fn runs(
        item: &Item,
        order: usize,
        cull: Mat4,
        near_far: bool,
        keep: impl Fn(&Cluster) -> bool,
    ) -> Vec<std::ops::Range<u32>> {
        let mut out: Vec<(f32, std::ops::Range<u32>)> = Vec::new();
        let mut open = false;
        for c in &item.clusters {
            if !keep(c) || c.bounds.is_none_or(|b| outside(cull, b, near_far)) {
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
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().map(|(_, r)| r).collect()
    }

    /// Draws every opaque and cutout part (and, into shadow maps and the water reflection, glass)
    /// whose bounds overlap `cull` (a view-projection), opaque parts first,
    /// clusters front to back, then all impostor quads. `near_far` also culls
    /// on depth.
    fn draw_items(&self, pass: &mut wgpu::RenderPass, cull: Mat4, near_far: bool, kind: Pass) {
        let mut current: Option<usize> = None;
        let shadow = matches!(kind, Pass::Shadow { .. });
        let shaded = matches!(kind, Pass::Forward | Pass::Reflection);
        let fades = matches!(kind, Pass::Depth | Pass::Forward);
        let order = shadow as usize;
        // 0 opaque, 1 cutout, 2 glass: into shadow maps, and into the water's reflection as
        // an opaque pane (without it the reflection looked through every window into the dark
        // room behind it: the dark blobs in a pond).
        let glass = shadow || matches!(kind, Pass::Reflection);
        for category in 0..2 + glass as usize {
            for item in self.items.iter().flatten() {
                let Some(bounds) = item.bounds else { continue };
                if outside(cull, bounds, near_far) {
                    continue;
                }
                let levels = item.lods.len() as u8;
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
                    let this = if m.surface == Surface::Glass {
                        2
                    } else if m.transparent() {
                        continue;
                    } else {
                        m.cutout() as usize
                    };
                    if this != category {
                        continue;
                    }
                    let weights = mesh.weights.as_ref().filter(|_| shaded);
                    // The last mesh level of an item with an impostor (or a
                    // cull fade) dissolves in fading clusters, through its
                    // own pipelines, one draw and fade slot per cluster.
                    let can_fade = fades
                        && if item.impostor.is_some() {
                            part.lod + 2 == levels
                        } else {
                            item.cull_fade > 0.0 && part.lod + 1 == levels
                        };
                    for fading in [false, true] {
                        if fading && !can_fade {
                            continue;
                        }
                        let ds = m.double_sided as usize;
                        let (key, pipeline) = match kind {
                            Pass::Shadow { .. } => (category, &self.pipelines.shadow[category]),
                            Pass::Depth if fading => (300 + ds, &self.sampled.depth_fade[ds]),
                            Pass::Forward if fading => (2 + ds, &self.sampled.forward[2 + ds]),
                            Pass::Depth => (m.variant(), &self.sampled.depth[m.variant()]),
                            Pass::Forward if weights.is_some() => (100, &self.sampled.terrain),
                            Pass::Reflection if weights.is_some() => {
                                (100, &self.pipelines.reflect_terrain)
                            }
                            Pass::Forward => (m.variant(), &self.sampled.forward[m.variant()]),
                            Pass::Reflection => (m.variant(), &self.pipelines.reflect[m.variant()]),
                        };
                        let keep = |c: &Cluster| {
                            Self::cluster_lod(item, c, kind) == Some(part.lod)
                                && (!can_fade || c.fading == fading)
                        };
                        let runs: Vec<(std::ops::Range<u32>, u32)> = if fading {
                            item.clusters
                                .iter()
                                .filter(|c| {
                                    keep(c) && c.bounds.is_some_and(|b| !outside(cull, b, near_far))
                                })
                                .map(|c| (c.range.clone(), c.fade_slot + part_index as u32))
                                .collect()
                        } else {
                            Self::runs(item, order, cull, near_far, keep)
                                .into_iter()
                                .map(|r| (r, part.slot))
                                .collect()
                        };
                        if runs.is_empty() {
                            continue;
                        }
                        if current != Some(key) {
                            pass.set_pipeline(pipeline);
                            current = Some(key);
                        }
                        if let Some(w) = weights {
                            pass.set_bind_group(1, &self.terrain.bind, &[]);
                            pass.set_vertex_buffer(2, w.slice(..));
                        } else if shaded || category == 1 || fading {
                            pass.set_bind_group(1, &material.bind, &[]);
                        }
                        let slots = if fading {
                            &self.fade_bind
                        } else {
                            &self.draw_bind
                        };
                        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                        pass.set_vertex_buffer(1, item.instances.slice(..));
                        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                        for (i, (r, slot)) in runs.into_iter().enumerate() {
                            if (shaded || fading) && (fading || i == 0) {
                                pass.set_bind_group(2, slots, &[(slot as u64 * SLOT) as u32]);
                            }
                            self.counters.add(
                                mesh.index_count as u64 / 3,
                                r.len() as u32,
                                kind == Pass::Forward,
                            );
                            pass.draw_indexed(0..mesh.index_count, 0, r);
                        }
                    }
                }
            }
            // All impostors after the cutout meshes: one pipeline switch.
            if category == 1 {
                for item in self.items.iter().flatten() {
                    if item.bounds.is_some_and(|b| !outside(cull, b, near_far)) {
                        self.draw_impostor(pass, item, cull, near_far, kind, &mut current);
                    }
                }
            }
        }
    }

    /// An item's impostor quads: clusters at the impostor level, and in the
    /// depth and forward passes also the fading ones.
    fn draw_impostor(
        &self,
        pass: &mut wgpu::RenderPass,
        item: &Item,
        cull: Mat4,
        near_far: bool,
        kind: Pass,
        current: &mut Option<usize>,
    ) {
        let Some((id, slot)) = item.impostor else {
            return;
        };
        let Some(Some(gpu)) = self.impostors.get(id.0 as usize) else {
            return;
        };
        let level = item.lods.len() as u8 - 1;
        let order = matches!(kind, Pass::Shadow { .. }) as usize;
        let fades = matches!(kind, Pass::Depth | Pass::Forward);
        let runs = Self::runs(item, order, cull, near_far, |c| {
            Self::cluster_lod(item, c, kind) == Some(level) && !(fades && c.fading)
        });
        // Dissolving clusters draw their impostor with their own fade slot.
        let fading: Vec<(std::ops::Range<u32>, u32)> = if fades {
            item.clusters
                .iter()
                .filter(|c| c.fading && c.bounds.is_some_and(|b| !outside(cull, b, near_far)))
                .map(|c| (c.range.clone(), c.fade_slot + item.parts.len() as u32))
                .collect()
        } else {
            Vec::new()
        };
        if runs.is_empty() && fading.is_empty() {
            return;
        }
        let (key, pipeline) = match kind {
            Pass::Depth => (400, &self.sampled.impostor[0]),
            Pass::Forward => (401, &self.sampled.impostor[1]),
            Pass::Reflection => (402, &self.pipelines.reflect_impostor),
            Pass::Shadow { .. } => (403, &self.pipelines.impostor_shadow),
        };
        if *current != Some(key) {
            pass.set_pipeline(pipeline);
            *current = Some(key);
        }
        pass.set_bind_group(1, &gpu.bind, &[]);
        if !matches!(kind, Pass::Shadow { .. }) {
            pass.set_bind_group(2, &self.draw_bind, &[(slot as u64 * SLOT) as u32]);
        }
        pass.set_vertex_buffer(1, item.instances.slice(..));
        for r in runs {
            self.counters.add(2, r.len() as u32, kind == Pass::Forward);
            pass.draw(0..6, r);
        }
        for (r, slot) in fading {
            pass.set_bind_group(2, &self.fade_bind, &[(slot as u64 * SLOT) as u32]);
            self.counters.add(2, r.len() as u32, kind == Pass::Forward);
            pass.draw(0..6, r);
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
            pass.set_bind_group(1, &material.bind, &[]);
            pass.set_bind_group(2, &self.draw_bind, &[(part.slot as u64 * SLOT) as u32]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_vertex_buffer(1, item.instances.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            for p in pipelines {
                pass.set_pipeline(p);
                self.counters
                    .add(mesh.index_count as u64 / 3, range.len() as u32, true);
                pass.draw_indexed(0..mesh.index_count, 0, range.clone());
            }
        }
    }

    fn update_bounds(&mut self) {
        if !self.bounds_dirty {
            return;
        }
        self.bounds_dirty = false;
        let mut scene: Option<Bounds> = None;
        let mut terrain: Option<Bounds> = None;
        for item in self.items.iter_mut().flatten() {
            let is_terrain = item.parts.iter().any(|p| {
                self.meshes
                    .get(p.mesh.0 as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|m| m.weights.is_some())
            });
            let meshes: Vec<&GpuMesh> = item
                .parts
                .iter()
                .filter_map(|p| self.meshes.get(p.mesh.0 as usize)?.as_ref())
                .collect();
            let mesh_bounds = meshes.iter().filter_map(|m| m.bounds).reduce(union);
            // Room for the wind's sway (tips lean a few percent of their size).
            let mesh_bounds = match (mesh_bounds, meshes.iter().any(|m| m.wind)) {
                (Some((lo, hi)), true) => {
                    let pad = Vec3::splat(0.1 * (hi - lo).max_element() + 0.1);
                    Some((lo - pad, hi + pad))
                }
                (b, _) => b,
            };
            for c in &mut item.clusters {
                c.bounds = mesh_bounds.and_then(|b| {
                    item.transforms[c.range.start as usize..c.range.end as usize]
                        .iter()
                        .map(|m| transform_bounds(*m, b))
                        .reduce(union)
                });
            }
            item.bounds = item.clusters.iter().filter_map(|c| c.bounds).reduce(union);
            if let Some(b) = item.bounds {
                scene = Some(scene.map_or(b, |s| union(s, b)));
                if is_terrain {
                    terrain = Some(terrain.map_or(b, |s| union(s, b)));
                }
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
            for c in &mut item.clusters {
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
                // The level next to the hand-over that dissolves: the last
                // mesh level into the impostor, or the last level into
                // nothing with a cull fade. None: levels switch at once.
                let levels = item.lods.len() as u8;
                let near = if ortho.is_some() {
                    None
                } else if item.impostor.is_some() {
                    Some(levels - 2)
                } else if item.cull_fade > 0.0 && item.lods[levels as usize - 1].is_finite() {
                    Some(levels - 1)
                } else {
                    None
                };
                let far = |l: u8| near.is_some_and(|n| l > n);
                // Beyond the impostor everything is culled at once.
                let past = |l: u8| item.impostor.is_some() && l >= levels;
                if let Some(n) = near
                    && c.target != LOD_UNSET
                    && far(c.target) != far(target)
                    && !past(c.target)
                    && !past(target)
                {
                    // Crossing the hand-over: dissolve, or turn a running
                    // dissolve around where it is.
                    c.fade = if c.fade > 0 {
                        FADE_FRAMES - c.fade
                    } else {
                        FADE_FRAMES
                    };
                    c.to_far = far(target);
                    c.lod = n;
                } else if near.is_none() || past(target) {
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
                        fade_bytes.extend_from_slice(&fade_slot(p.mesh, fade));
                        fade_bytes.resize(fade_bytes.len() + SLOT as usize - 16, 0);
                    }
                    fade_bytes.extend_from_slice(&fade_slot(MeshId(u32::MAX - 2), fade));
                    fade_bytes.resize(fade_bytes.len() + SLOT as usize - 16, 0);
                    fade_used += item.parts.len() + 1;
                    c.fading = true;
                    c.lod = near.unwrap_or(target);
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
        self.far_cache = None;
        self.cascade_rendered = [None; shadow::MAX_CASCADES];
    }

    /// One globals block (common.wgsl `Globals`) without the SH, which the
    /// GPU writes: for the camera, or (`clip`: the water height) for the
    /// mirrored camera of the planar reflection.
    fn write_globals(
        &self,
        buffer: &wgpu::Buffer,
        cascades: &shadow::Cascades,
        (view, proj, eye): (Mat4, Mat4, Vec3),
        (w, h): (u32, u32),
        water: Option<f32>,
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
        let sun_light = self.sun.color * self.sun.illuminance * exposure * self.clouds.sun_share();
        let disc_solid_angle = std::f32::consts::PI * sky::SUN_DISC_RADIUS * sky::SUN_DISC_RADIUS;
        let disc = (self.sun.illuminance / disc_solid_angle * exposure).min(64.0);
        b.floats(&[eye.x, eye.y, eye.z, self.atmosphere.clear_m]);
        b.floats(&self.sun.direction.extend(disc).to_array());
        b.floats(&sun_light.extend(sky::SUN_DISC_RADIUS.cos()).to_array());
        b.floats(&cascades.splits);
        b.floats(&cascades.texel);
        let res = q.shadow_resolution as f32;
        b.floats(&[cascades.count as f32, res, 1.0 / res, self.shadow_distance]);
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
        let dir = Vec2::new(self.wind.direction.x, self.wind.direction.z).normalize_or(Vec2::X);
        b.floats(&[
            dir.x,
            dir.y,
            self.wind.strength.max(0.0),
            self.wind.gustiness.clamp(0.0, 1.0),
        ]);
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
        b.floats(&[
            haze / 1000.0,
            1.0 / a.haze_height_m,
            ground_height,
            a.sun_glow,
        ]);
        let (source, rotation, scale, mips) = match &self.sky_source {
            SkySource::Preetham => (0.0, 0.0, 1.0, 1.0),
            SkySource::Physical => (1.0, 0.0, SUN_TOP_KLX, 1.0),
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
            if q.soft_shadows { 9.0 } else { 4.0 },
            bloom,
            if q.ao_full_resolution { 1.0 } else { 2.0 },
            ground_height,
        ]);
        b.floats(&[
            if water.is_some() { 1.0 } else { 0.0 },
            water.unwrap_or(0.0),
            if clip.is_some() { 1.0 } else { 0.0 },
            clip.unwrap_or(0.0),
        ]);
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
        let travelled = (1.0 + 2.0 * f64::from(self.wind.strength)) * self.clock;
        let drift = Vec2::new(
            (f64::from(wind.x) * travelled) as f32,
            (f64::from(wind.y) * travelled) as f32,
        );
        b.floats(&[
            self.clouds.cover.clamp(0.0, 1.0),
            if self.clouds.overcast { 1.0 } else { 0.0 },
            drift.x,
            drift.y,
        ]);
        b.floats(&self.snow_cover.unwrap_or(Mat4::ZERO).to_cols_array());
        b.floats(&[
            self.snow,
            self.quality.cascades as f32,
            if self.snow_cover.is_some() { 1.0 } else { 0.0 },
            0.0,
        ]);
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
            false,
        );
    }
}

fn upload_mesh(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) -> Result<GpuMesh, String> {
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
    let buffer = |label, bytes: &[u8], usage| {
        let b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (bytes.len() as u64).max(4).next_multiple_of(4),
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&b, 0, bytes);
        b
    };
    let bounds = mesh
        .positions
        .iter()
        .map(|p| (Vec3::from(*p), Vec3::from(*p)))
        .reduce(union);
    Ok(GpuMesh {
        vertices: buffer("vertices", &v.0, wgpu::BufferUsages::VERTEX),
        indices: buffer("indices", &ix, wgpu::BufferUsages::INDEX),
        index_count: mesh.indices.len() as u32,
        bounds,
        wind,
        weights: None,
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
    let c = corners(b).map(|p| view_proj * p.extend(1.0));
    let all = |f: &dyn Fn(Vec4) -> bool| c.iter().all(|&p| f(p));
    all(&|p| p.x < -p.w)
        || all(&|p| p.x > p.w)
        || all(&|p| p.y < -p.w)
        || all(&|p| p.y > p.w)
        || (near_far && (all(&|p| p.z < 0.0) || all(&|p| p.z > p.w)))
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
    let texel = SNOW_COVER_M / resolution.max(1) as f32;
    let c = (glam::Vec2::new(eye.x, eye.z) / texel).round() * texel;
    let top = hi.y + 1.0;
    let view = Mat4::look_to_rh(Vec3::new(c.x, top, c.y), Vec3::NEG_Y, Vec3::NEG_Z);
    Mat4::orthographic_rh(-half, half, -half, half, 0.0, (top - lo.y + 1.0).max(1.0)) * view
}

/// The snow cover map spans this square round the eye (metres); beyond, snow lies everywhere
/// that faces up.
const SNOW_COVER_M: f32 = 160.0;

fn globals_size() -> u64 {
    // 8 matrices, 28 vec4s, the 9 SH vec4s last; see common.wgsl.
    SH_OFFSET + 9 * 16
}

/// A procedural cloud layer (fractal noise on a plane 1.5 km up, lit by the sun and the sky).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clouds {
    /// 0 clear .. 1 covered.
    pub cover: f32,
    /// A closed grey layer instead of separate fair-weather clouds.
    pub overcast: bool,
}

impl Clouds {
    /// How much of the sun's direct light comes through: fair-weather clouds pass most of it
    /// on average, an overcast layer little (soft shadows, a low sun to sky ratio).
    pub fn sun_share(&self) -> f32 {
        let cover = self.cover.clamp(0.0, 1.0);
        if self.overcast {
            1.0 - 0.85 * cover
        } else {
            1.0 - 0.15 * cover
        }
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
fn fade_slot(mesh: MeshId, fade: f32) -> [u8; 16] {
    let mut out = mesh_slot(mesh);
    out[8..12].copy_from_slice(&fade.to_le_bytes());
    out
}

fn mesh_slot(mesh: MeshId) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&mesh.0.to_le_bytes());
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
        ],
    })
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
        let mut material = vec![uniform_entry(0, fs, None)];
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
                    texture_entry(0, S::Depth, D::D2Array, false),
                    sampler_entry(1, B::Comparison),
                    texture_entry(2, unfilterable, D::D2, false),
                    texture_entry(3, unfilterable, D::D2, false),
                    texture_entry(4, filterable, D::D2, false),
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

fn module<'a>(modules: &'a [wgpu::ShaderModule], name: &str) -> &'a wgpu::ShaderModule {
    &modules[SHADERS
        .iter()
        .position(|(n, _)| *n == name)
        .expect("shader name")]
}

fn cull(double_sided: bool) -> Option<wgpu::Face> {
    (!double_sided).then_some(wgpu::Face::Back)
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
fn ground_pipeline(
    device: &wgpu::Device,
    l: &Layouts,
    modules: &[wgpu::ShaderModule],
    fs: &str,
    samples: u32,
) -> wgpu::RenderPipeline {
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
            constants: &[],
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
    // With MSAA the pre-pass writes alpha-to-coverage sample masks.
    let fs = if fs == "fs_impostor_depth" && samples > 1 {
        "fs_impostor_depth_ms"
    } else {
        fs
    };
    let samples_constant = [("SAMPLES", samples as f64)];
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
            constants: if fs == "fs_impostor_depth_ms" {
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
            groups: &[Some(&l.globals)],
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

impl SampledPipelines {
    fn new(
        device: &wgpu::Device,
        l: &Layouts,
        modules: &[wgpu::ShaderModule],
        samples: u32,
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
                        "fs_depth_cutout"
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
                impostor_pipeline(device, l, modules, "fs_impostor_depth", samples, false),
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
                            "fs_depth_fade_ms"
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
            ground: ground_pipeline(device, l, modules, "fs_ground", samples),
            sky: sky_pipeline(device, l, modules, samples),
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
            ],
            reflect,
            reflect_ground: ground_pipeline(device, l, modules, "fs_reflect", 1),
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
            upscale: fullscreen("upscale", "post", "fs_upscale", &post, unorm.into(), &[]),
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
            if photo { (width, height) } else { (1, 1) },
        );
        let prev_view = view(prev_hdr.clone());
        let linear_depth = view(texture("linear depth", LINEAR_DEPTH, 1, sampled));
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
        let reflection = view(texture_sized(
            "reflection",
            HDR,
            1,
            sampled,
            reflection_size,
        ));
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
            reflect_forward_bind: forward(white, white, white, white),
            resolve_bind,
            depth,
            hdr_msaa,
            hdr,
            hdr_texture,
            prev_hdr,
            linear_depth,
            occlusion,
            output,
            output_view,
            output_unorm,
            display_view,
            ldr,
            bloom,
            reflection,
            reflection_depth,
            shadow_layers,
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
    let required_features = adapter.features()
        & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TEXTURE_COMPRESSION_BC);
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

    /// Every WGSL module parses and validates with naga, without a GPU.
    #[test]
    fn shaders_validate() {
        for (name, parts) in SHADERS {
            let source = parts.concat();
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
            "(bytes, meshes, textures, materials, items, slots)"
        );
        // Guards against a runaway frame, not speed: a software rasteriser on a shared
        // machine has taken 2.1 s here.
        assert!(worst.get() < 6.0, "a frame took {:.2} s", worst.get());
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

    #[test]
    fn globals_match_the_shader_layout() {
        // common.wgsl: 8 mat4, 28 vec4, the snow cover's mat4 and the snow vec4, then the 9 SH vec4.
        assert_eq!(SH_OFFSET, 9 * 64 + 29 * 16);
        assert_eq!(SH_OFFSET % 16, 0, "SH copy needs 16-byte texel alignment");
        assert_eq!(globals_size(), SH_OFFSET + 144);
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
