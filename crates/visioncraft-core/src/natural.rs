//! Natural ground: automatic terrain materials, rocks, deadwood and the ground cover near the
//! camera.
//!
//! - **Automatic materials** (`terrain.set_auto`): per vertex, masks for rock, scree, moss and
//!   soil from the slope, the altitude, the curvature (a hollow collects soil and moisture, a
//!   convex edge shows rock) and a moisture/shade term (concavity plus faces turned to the north),
//!   each threshold moved by low-frequency value noise so the boundaries wander over tens of
//!   metres. They share out the base layer's paint when the chunks are meshed; layers painted by
//!   hand stay. The renderer then blends the layers by their height maps (rock pokes through grass
//!   where its relief is high) and hex-tiles their maps (`terrain.wgsl`). The approach follows
//!   the slope/height/curvature splatting of the public talks on procedural terrain (Andersson,
//!   "Terrain Rendering in Frostbite", SIGGRAPH 2007; J. Moore, "Terrain Rendering in Far Cry 5",
//!   GDC 2018).
//! - **Rocks** (`rock.*`): boulders, slabs, outcrops and cobbles as icospheres cut by random
//!   planes (fracture faces) and roughened by 3D value noise; or a CC0 scan from the library
//!   (`model`). Partly sunk, tilted towards the slope, at a random turn and size from a seed.
//! - **Deadwood** (`deadwood.*`): fallen logs, stumps with root flare, snags (dead standing
//!   trunks with a broken top) and branches, as generated tubes; or a CC0 scan.
//! - **Ground cover** (`terrain.set_cover`): stones on scree, moss patches in shade, twigs on soil,
//!   placed per square metre from a hash of the cell (the same spots whenever they are made
//!   again), only round the camera.
//!
//! Meshes are made in unit size, so a few variants serve every instance.

use crate::planting::{Rng, mix, point_in_polygon};
use crate::terrain::{Terrain, value_noise};
use serde::{Deserialize, Serialize};

/// How many cells round a vertex its automatic weights look at (slope and curvature).
pub const AUTO_REACH: u32 = 4;

fn sstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---- automatic materials ----

/// Material ids for each class (absent: that class is not used) and the rules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoMaterials {
    /// Bare rock on steep and convex ground (and above `rock_above_m`).
    pub rock: Option<String>,
    /// Loose stones on slopes between `scree_slope_deg` and the rock.
    pub scree: Option<String>,
    /// Moss in hollows and on faces turned away from the sun.
    pub moss: Option<String>,
    /// Soil and leaf litter collecting in hollows.
    pub soil: Option<String>,
    /// Slope (degrees) where rock takes over.
    pub rock_slope_deg: f32,
    /// Slope (degrees) where scree starts.
    pub scree_slope_deg: f32,
    /// Half the width of each transition, degrees.
    pub blend_deg: f32,
    /// Above this height (metres) rock shows on gentler slopes too (a mountain top).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rock_above_m: Option<f32>,
    /// 0..3: how strongly curvature counts (convex: rock, concave: soil and moss).
    pub curvature: f32,
    /// 0..3: how readily moss grows in shade and hollows.
    pub moisture: f32,
    /// The size of the macro noise that moves the boundaries, metres.
    pub macro_scale_m: f32,
    /// 0..1: how far the noise moves them.
    pub macro_strength: f32,
    /// The drawing bearing of true north (degrees clockwise from -Z): which faces are shaded.
    pub north_deg: f32,
    pub seed: u64,
}

impl Default for AutoMaterials {
    fn default() -> Self {
        Self {
            rock: Some("rock".into()),
            scree: Some("scree".into()),
            moss: Some("moss".into()),
            soil: Some("forest-floor".into()),
            rock_slope_deg: 38.0,
            scree_slope_deg: 27.0,
            blend_deg: 5.0,
            rock_above_m: None,
            curvature: 1.0,
            moisture: 1.0,
            macro_scale_m: 40.0,
            macro_strength: 0.5,
            north_deg: 0.0,
            seed: 1,
        }
    }
}

/// The share of the base layer each class takes at one spot, 0..1, summing to at most 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Masks {
    pub rock: f32,
    pub scree: f32,
    pub moss: f32,
    pub soil: f32,
}

impl AutoMaterials {
    pub fn validate(&self) -> Result<(), String> {
        let within = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        if !(within(self.rock_slope_deg, 1.0, 89.0) && within(self.scree_slope_deg, 1.0, 89.0)) {
            return Err("rock_slope_deg and scree_slope_deg must lie in 1..89".into());
        }
        if self.scree_slope_deg > self.rock_slope_deg {
            return Err("scree_slope_deg must not be above rock_slope_deg".into());
        }
        if !within(self.blend_deg, 0.5, 20.0) {
            return Err("blend_deg must lie in 0.5..20".into());
        }
        if !(within(self.curvature, 0.0, 3.0) && within(self.moisture, 0.0, 3.0)) {
            return Err("curvature and moisture must lie in 0..3".into());
        }
        if !(within(self.macro_scale_m, 2.0, 2000.0) && within(self.macro_strength, 0.0, 1.0)) {
            return Err("macro_scale_m must lie in 2..2000 and macro_strength in 0..1".into());
        }
        if !self.north_deg.is_finite() || self.rock_above_m.is_some_and(|h| !h.is_finite()) {
            return Err("north_deg and rock_above_m must be finite".into());
        }
        Ok(())
    }

    /// The material ids in use, in class order (rock, scree, moss, soil).
    pub fn materials(&self) -> Vec<&str> {
        [&self.rock, &self.scree, &self.moss, &self.soil]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect()
    }

    /// The masks at vertex `(ix, iz)` of `t`.
    pub fn masks(&self, t: &Terrain, ix: u32, iz: u32) -> Masks {
        let h = |dx: i64, dz: i64| t.height_clamped(i64::from(ix) + dx, i64::from(iz) + dz);
        let c = t.cell_m;
        // Slope and curvature over about a metre (one cell at least), so the 0.25 m grid of a
        // garden and the 2 m grid of a hillside read alike.
        let r = ((1.0 / c).round() as i64).clamp(1, i64::from(AUTO_REACH));
        let span = 2.0 * r as f32 * c;
        let (gx, gz) = ((h(r, 0) - h(-r, 0)) / span, (h(0, r) - h(0, -r)) / span);
        let grad = (gx * gx + gz * gz).sqrt();
        let slope = grad.atan().to_degrees();
        let h0 = h(0, 0);
        let lap = (h(r, 0) + h(-r, 0) + h(0, r) + h(0, -r) - 4.0 * h0) / ((r as f32 * c).powi(2));
        // A curvature of 0.15 per metre (a seven metre radius) counts fully.
        let concave = (lap * 6.5 * self.curvature).clamp(0.0, 1.0);
        let convex = (-lap * 6.5 * self.curvature).clamp(0.0, 1.0);
        let [x, z] = t.xz(ix, iz);
        let s = self.macro_scale_m;
        let n1 = value_noise(self.seed, x / s, z / s) * 0.65
            + value_noise(self.seed ^ 0x51, x / (s * 0.3), z / (s * 0.3)) * 0.35;
        let n2 = value_noise(self.seed ^ 0xA7, x / (s * 0.5) + 17.0, z / (s * 0.5));
        let m = self.macro_strength;
        let e = slope + n1 * 14.0 * m + convex * 10.0 - concave * 6.0;
        let b = self.blend_deg;
        let mut rock = sstep(self.rock_slope_deg - b, self.rock_slope_deg + b, e);
        if let Some(above) = self.rock_above_m {
            let alpine = sstep(above - 3.0, above + 3.0, h0 + n1 * 6.0 * m);
            let gentle = self.rock_slope_deg * 0.45;
            rock = rock.max(alpine * sstep(gentle - b, gentle + b, e));
        }
        // Scree lies below the rock and gathers at the foot of a slope.
        let scree = sstep(
            self.scree_slope_deg - b,
            self.scree_slope_deg + b,
            e + concave * 6.0,
        ) * (1.0 - rock);
        // Shade: a slope falling towards north (in the drawing's bearing) sees less sun.
        let nb = self.north_deg.to_radians();
        let north = [nb.sin(), -nb.cos()];
        let facing = if grad > 1e-4 {
            (-gx * north[0] - gz * north[1]) / grad
        } else {
            0.0
        };
        let shade = facing.max(0.0) * (grad * 2.0).min(1.0);
        let wet = (concave * 0.8 + shade * 0.9 + n2 * 0.25 * m) * self.moisture;
        let moss = sstep(0.35, 0.75, wet) * (1.0 - scree) * (1.0 - 0.4 * rock);
        let soil = sstep(0.65, 1.0, concave) * (1.0 - rock) * (1.0 - scree);
        // Small shares are dropped (and the rest rescaled): a few per cent of a layer whose
        // height map stands high would show through as flecks all over the others.
        let keep = |v: f32| ((v - 0.12) / 0.88).max(0.0);
        let mut out = Masks {
            rock: keep(rock),
            scree: keep(scree),
            moss: keep(moss),
            soil: keep(soil * (1.0 - moss)),
        };
        let total = out.rock + out.scree + out.moss + out.soil;
        if total > 1.0 {
            out = Masks {
                rock: out.rock / total,
                scree: out.scree / total,
                moss: out.moss / total,
                soil: out.soil / total,
            };
        }
        out
    }
}

/// The layer index of each class in `t` (rock, scree, moss, soil): the automatic materials'
/// ids, else the built-in ones (`rock`, `scree`, `moss`, `forest-floor` or `soil`).
pub fn class_layers(t: &Terrain) -> [Option<usize>; 4] {
    let find = |id: &str| t.layers.iter().position(|l| l.material == id);
    match &t.auto {
        Some(a) => [&a.rock, &a.scree, &a.moss, &a.soil].map(|m| m.as_deref().and_then(find)),
        None => [
            find("rock"),
            find("scree"),
            find("moss"),
            find("forest-floor").or_else(|| find("soil")),
        ],
    }
}

impl Terrain {
    /// The weights the meshes draw at vertex `(ix, iz)`: the painted ones, with the base layer's
    /// share given out to the automatic materials (when set), padded to eight.
    pub fn vertex_weights(&self, ix: u32, iz: u32) -> [u8; 8] {
        let n = self.layers.len();
        let i = self.at(ix, iz) * n;
        let mut w = [0u8; 8];
        w[..n].copy_from_slice(&self.weights[i..i + n]);
        let Some(a) = &self.auto else {
            return w;
        };
        if w[0] == 0 {
            return w;
        }
        let m = a.masks(self, ix, iz);
        let layers = class_layers(self);
        let base = f32::from(w[0]);
        let mut left = i32::from(w[0]);
        for (layer, share) in layers.into_iter().zip([m.rock, m.scree, m.moss, m.soil]) {
            if let Some(l) = layer.filter(|l| *l != 0) {
                let v = ((base * share).round() as i32).clamp(0, left);
                w[l] = (i32::from(w[l]) + v) as u8;
                left -= v;
            }
        }
        w[0] = left as u8;
        w
    }

    /// [`Terrain::vertex_weights`] at the vertex nearest world `(x, z)`; `None` outside.
    pub fn drawn_weights_at(&self, x: f32, z: f32) -> Option<[u8; 8]> {
        let (u, v) = (
            (x - self.origin[0]) / self.cell_m,
            (z - self.origin[1]) / self.cell_m,
        );
        if !(u >= 0.0 && v >= 0.0 && u <= self.width as f32 && v <= self.depth as f32) {
            return None;
        }
        Some(self.vertex_weights(u.round() as u32, v.round() as u32))
    }
}

/// The materials of natural ground, added to a project when a command first uses them: the
/// library's CC0 maps from ambientCG (Lennart Demes): Rock 044 (a cliff face), Rocks 006 (sharp
/// loose stones), Moss 001 and Bark 016. Each tile is drawn larger than the scan's own size: from
/// a few metres away the scans' repeat is what shows, and the hex tiling hides the rest.
pub fn natural_materials() -> Vec<crate::Material> {
    let textured = |id: &str, name: &str, folder: &str, scale: f32, relief: f32, variation: f32| {
        let mut m = crate::Material {
            id: id.into(),
            name: name.into(),
            kind: crate::SurfaceKind::StandardTriplanarPbr {
                map_scale_m: scale,
                relief,
                gloss: 0.0,
                weathering: 0.0,
                parallax_depth_m: 0.0,
            },
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            maps: Default::default(),
            variation,
            weathering: None,
            extensions: Default::default(),
        };
        let map = |kind: &str| Some(format!("materials/{folder}/{kind}.jpg"));
        m.maps = crate::material::MaterialMaps {
            base_color: map("basecolor"),
            normal: map("normal"),
            orm: map("orm"),
            height: map("height"),
            ..Default::default()
        };
        m
    };
    // The two stone scans are pale (a sunlit cliff, light limestone chips): toned down to the
    // grey of weathered rock in shade and light.
    let tinted = |mut m: crate::Material, k: f32| {
        m.base_color = [k * 0.9, k * 0.93, k, 1.0];
        m
    };
    vec![
        tinted(textured("rock", "Rock", "rock044", 3.0, 0.8, 0.4), 0.42),
        tinted(textured("scree", "Scree", "rocks006", 1.2, 0.8, 0.3), 0.45),
        // The moss scan is a bright yellow green in full light: darker, as moss in shade is.
        {
            let mut m = textured("moss", "Moss", "moss001", 0.9, 0.5, 0.5);
            m.base_color = [0.45, 0.5, 0.35, 1.0];
            m
        },
        textured("bark", "Bark", "bark016", 1.0, 0.9, 0.3),
    ]
}

/// Adds the natural material `id` to `project` when it lacks it and it is one of
/// [`natural_materials`] (or of the built-in palette).
pub fn ensure_material(project: &mut crate::Project, id: &str) -> bool {
    if project.materials.iter().any(|m| m.id == id) {
        return true;
    }
    match natural_materials()
        .into_iter()
        .chain(crate::builtin_palette())
        .find(|m| m.id == id)
    {
        Some(m) => {
            project.materials.push(m);
            true
        }
        None => false,
    }
}

// ---- ground cover ----

/// Densities of the ground cover near the camera, as multiples of the built-in ones (0: none).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GroundCover {
    /// Loose stones: many on scree, a few on rock ledges and soil.
    pub stones: f32,
    /// Twigs and small branches on soil and moss.
    pub twigs: f32,
    /// Cushions of moss in the shade.
    pub moss: f32,
    /// Leaf litter under deciduous crowns the whole year, not only in autumn.
    pub litter: f32,
    pub seed: u64,
}

impl Default for GroundCover {
    fn default() -> Self {
        Self {
            stones: 1.0,
            twigs: 1.0,
            moss: 1.0,
            litter: 1.0,
            seed: 1,
        }
    }
}

impl GroundCover {
    pub fn validate(&self) -> Result<(), String> {
        if [self.stones, self.twigs, self.moss, self.litter]
            .iter()
            .all(|v| v.is_finite() && (0.0..=4.0).contains(v))
        {
            Ok(())
        } else {
            Err("stones, twigs, moss and litter must lie in 0..4".into())
        }
    }
}

/// Per square metre at a full mask and a multiple of 1.
// shortcut: densities judged from photographs of talus slopes and woodland floors (no source);
// tune against more.
const STONES_ON_SCREE: f32 = 14.0;
const STONES_ON_ROCK: f32 = 2.0;
const STONES_ON_SOIL: f32 = 1.5;
const MOSS_PATCHES: f32 = 3.0;
const TWIGS_ON_SOIL: f32 = 1.5;
const TWIGS_ON_MOSS: f32 = 0.4;
/// At most this many spots of the cover in one go.
pub const MAX_COVER_SPOTS: usize = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoverKind {
    Stone,
    Twig,
    Moss,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverSpot {
    pub kind: CoverKind,
    pub x: f32,
    pub z: f32,
    pub yaw: f32,
    /// Metres: a stone's width, a twig's length, a moss cushion's width.
    pub size: f32,
    /// 0..1, for picking a variant.
    pub pick: f32,
}

/// The cover inside the square of half side `reach` round `center`, by whole metre cells
/// (deterministic per cell: the same spots each time an area is made again).
pub fn cover_spots(
    t: &Terrain,
    cover: &GroundCover,
    center: [f32; 2],
    reach: f32,
) -> Vec<CoverSpot> {
    let layers = class_layers(t);
    let mut out = Vec::new();
    let [[x0, z0], [x1, z1]] = t.extent();
    let lo = [(center[0] - reach).max(x0), (center[1] - reach).max(z0)].map(f32::floor);
    let hi = [(center[0] + reach).min(x1), (center[1] + reach).min(z1)].map(f32::ceil);
    let (mut cz, end_z) = (lo[1] as i64, hi[1] as i64);
    while cz < end_z {
        let mut cx = lo[0] as i64;
        while cx < hi[0] as i64 {
            let (px, pz) = (cx as f32 + 0.5, cz as f32 + 0.5);
            if let Some(w) = t.drawn_weights_at(px, pz) {
                let share = |l: Option<usize>| l.map_or(0.0, |l| f32::from(w[l]) / 255.0);
                let [rock, scree, moss, soil] = layers.map(share);
                let mut rng = Rng::new(mix(mix(cover.seed ^ 0xC0FE, cx), cz));
                let mut emit = |kind: CoverKind, density: f32, rng: &mut Rng| {
                    let count = density + rng.f32();
                    for _ in 0..count as u32 {
                        let (x, z) = (cx as f32 + rng.f32(), cz as f32 + rng.f32());
                        let u = rng.f32();
                        let size = match kind {
                            // Mostly small, a few bigger: a talus spread.
                            CoverKind::Stone => 0.03 + 0.22 * u * u * u,
                            CoverKind::Twig => 0.15 + 0.45 * u,
                            CoverKind::Moss => 0.25 + 0.5 * u,
                        };
                        out.push(CoverSpot {
                            kind,
                            x,
                            z,
                            yaw: rng.f32() * std::f32::consts::TAU,
                            size,
                            pick: rng.f32(),
                        });
                    }
                };
                let stones =
                    (STONES_ON_SCREE * scree + STONES_ON_ROCK * rock + STONES_ON_SOIL * soil)
                        * cover.stones;
                emit(CoverKind::Stone, stones, &mut rng);
                // Cushions on soft ground, not on the rock face.
                emit(
                    CoverKind::Moss,
                    MOSS_PATCHES * moss * (1.0 - rock) * cover.moss,
                    &mut rng,
                );
                let twigs = (TWIGS_ON_SOIL * soil + TWIGS_ON_MOSS * moss) * cover.twigs;
                emit(CoverKind::Twig, twigs, &mut rng);
                if out.len() >= MAX_COVER_SPOTS {
                    out.truncate(MAX_COVER_SPOTS);
                    return out;
                }
            }
            cx += 1;
        }
        cz += 1;
    }
    out
}

// ---- rocks and deadwood ----

fn layer_default() -> String {
    crate::project::LAYER_SHAPES.into()
}

fn rock_material() -> String {
    "rock".into()
}

fn bark_material() -> String {
    "bark".into()
}

fn one_metre() -> f32 {
    1.0
}

fn sink_default() -> f32 {
    0.25
}

fn align_default() -> f32 {
    0.7
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RockShape {
    /// Rounded, about as deep as wide.
    #[default]
    Boulder,
    /// Flat and wide.
    Slab,
    /// Tall and craggy, with flat fracture faces.
    Outcrop,
    /// A small rounded stone.
    Cobble,
}

impl RockShape {
    pub const ALL: [Self; 4] = [Self::Boulder, Self::Slab, Self::Outcrop, Self::Cobble];

    pub fn name(self) -> &'static str {
        match self {
            Self::Boulder => "boulder",
            Self::Slab => "slab",
            Self::Outcrop => "outcrop",
            Self::Cobble => "cobble",
        }
    }

    /// Height and depth as shares of the width.
    fn proportions(self) -> [f32; 2] {
        match self {
            Self::Boulder => [0.7, 0.8],
            Self::Slab => [0.3, 0.7],
            Self::Outcrop => [1.0, 0.6],
            Self::Cobble => [0.55, 0.75],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rock {
    pub id: String,
    #[serde(default = "layer_default")]
    pub layer: String,
    /// `[x, z]` of its centre on the ground.
    pub position: [f32; 2],
    /// Width (the longest side across), metres.
    #[serde(default = "one_metre")]
    pub size_m: f32,
    #[serde(default)]
    pub shape: RockShape,
    /// Turn about the vertical, degrees.
    #[serde(default)]
    pub yaw_deg: f32,
    #[serde(default)]
    pub seed: u64,
    /// 0..0.9: the share of its height sunk into the ground.
    #[serde(default = "sink_default")]
    pub sink: f32,
    /// 0..1: how far it tilts with the slope (0 upright, 1 square to the ground).
    #[serde(default = "align_default")]
    pub align: f32,
    #[serde(default = "rock_material")]
    pub material: String,
    /// A catalogue asset (a CC0 scan such as `boulder_01`) drawn instead of the generated rock,
    /// scaled to `size_m`; the generated one stands in while it loads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The scatter that made it (`rock.scatter`), to remove them together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadwoodKind {
    /// A fallen trunk lying on the ground.
    #[default]
    Log,
    /// A cut or broken trunk standing on its roots.
    Stump,
    /// A dead standing tree with a broken top and branch stubs.
    Snag,
    /// A fallen branch with a fork or two.
    Branch,
}

impl DeadwoodKind {
    pub const ALL: [Self; 4] = [Self::Log, Self::Stump, Self::Snag, Self::Branch];

    pub fn name(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Stump => "stump",
            Self::Snag => "snag",
            Self::Branch => "branch",
        }
    }

    /// A length and diameter to begin with, metres.
    pub fn default_size(self) -> (f32, f32) {
        match self {
            Self::Log => (4.0, 0.35),
            Self::Stump => (0.5, 0.45),
            Self::Snag => (6.0, 0.35),
            Self::Branch => (1.6, 0.06),
        }
    }

    fn lies(self) -> bool {
        matches!(self, Self::Log | Self::Branch)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deadwood {
    pub id: String,
    #[serde(default = "layer_default")]
    pub layer: String,
    #[serde(default)]
    pub kind: DeadwoodKind,
    /// `[x, z]`: a lying piece's middle, a standing one's foot.
    pub position: [f32; 2],
    /// Metres: a log's or branch's length, a stump's or snag's height.
    pub length_m: f32,
    pub diameter_m: f32,
    #[serde(default)]
    pub yaw_deg: f32,
    #[serde(default)]
    pub seed: u64,
    #[serde(default = "bark_material")]
    pub material: String,
    /// A catalogue asset (a CC0 scan such as `tree_stump_01` or `dead_tree_trunk`) drawn
    /// instead, scaled to `length_m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

impl Rock {
    pub fn validate(&self) -> Result<(), String> {
        if !(self.size_m.is_finite() && (0.05..=30.0).contains(&self.size_m)) {
            return Err("a rock's size_m must lie in 0.05..30".into());
        }
        if !(self.position.iter().all(|v| v.is_finite()) && self.yaw_deg.is_finite()) {
            return Err("position and yaw_deg must be finite".into());
        }
        if !((0.0..=0.9).contains(&self.sink) && (0.0..=1.0).contains(&self.align)) {
            return Err("sink must lie in 0..0.9 and align in 0..1".into());
        }
        Ok(())
    }

    /// Width, height and depth in metres (the seed varies the shape's proportions by up to 15 %).
    pub fn dimensions(&self) -> [f32; 3] {
        let [h, d] = self.shape.proportions();
        let mut rng = Rng::new(mix(self.seed, 77));
        let j = |rng: &mut Rng| 0.85 + 0.3 * rng.f32();
        [
            self.size_m,
            self.size_m * h * j(&mut rng),
            self.size_m * d * j(&mut rng),
        ]
    }
}

impl Deadwood {
    pub fn validate(&self) -> Result<(), String> {
        if !(self.length_m.is_finite() && (0.1..=40.0).contains(&self.length_m)) {
            return Err("length_m must lie in 0.1..40".into());
        }
        if !(self.diameter_m.is_finite() && (0.01..=3.0).contains(&self.diameter_m)) {
            return Err("diameter_m must lie in 0.01..3".into());
        }
        if self.diameter_m > self.length_m {
            return Err("diameter_m must not be above length_m".into());
        }
        if !(self.position.iter().all(|v| v.is_finite()) && self.yaw_deg.is_finite()) {
            return Err("position and yaw_deg must be finite".into());
        }
        Ok(())
    }

    /// The generated mesh it uses: its kind, a variant and the aspect bucket (length over
    /// diameter, rounded to a power of the square root of two).
    pub fn mesh_key(&self) -> (DeadwoodKind, u32, u32) {
        let aspect = (self.length_m / self.diameter_m).max(1.0);
        let bucket = (aspect.log2() * 2.0).round().clamp(0.0, 14.0) as u32;
        (self.kind, (self.seed % VARIANTS as u64) as u32, bucket)
    }
}

/// Generated variants per rock shape and per deadwood kind.
pub const VARIANTS: u32 = 4;

/// A placement as a column-major 4x4 matrix (the glam / WGSL layout).
pub type Matrix = [f32; 16];

fn matrix(axes: [[f32; 3]; 3], at: [f32; 3]) -> Matrix {
    [
        axes[0][0], axes[0][1], axes[0][2], 0.0, axes[1][0], axes[1][1], axes[1][2], 0.0,
        axes[2][0], axes[2][1], axes[2][2], 0.0, at[0], at[1], at[2], 1.0,
    ]
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-9 {
        v.map(|c| c / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The ground's normal round `(x, z)` over `reach` metres from `ground` (up where it is missing).
fn ground_normal(ground: &dyn Fn(f32, f32) -> Option<f32>, x: f32, z: f32, reach: f32) -> [f32; 3] {
    let h = |dx: f32, dz: f32| ground(x + dx, z + dz);
    match (h(reach, 0.0), h(-reach, 0.0), h(0.0, reach), h(0.0, -reach)) {
        (Some(a), Some(b), Some(c), Some(d)) => {
            norm([-(a - b) / (2.0 * reach), 1.0, -(c - d) / (2.0 * reach)])
        }
        _ => [0.0, 1.0, 0.0],
    }
}

/// The frame of a thing standing at `(x, z)` turned by `yaw` and tilted `align` of the way to
/// the ground's normal `n`: (right, up, forward).
fn frame(yaw: f32, n: [f32; 3], align: f32) -> [[f32; 3]; 3] {
    let up = norm([n[0] * align, 1.0 - align + n[1] * align, n[2] * align]);
    let fwd0 = [yaw.sin(), 0.0, yaw.cos()];
    let right = norm(cross(up, fwd0));
    let fwd = cross(right, up);
    [right, up, fwd]
}

/// Where a rock goes: unit mesh (`rock_mesh`, x and z in -0.5..0.5, y in 0..1) to world, on the
/// lowest ground under its footprint and sunk by `sink` of its height, so no edge floats.
pub fn rock_matrix(r: &Rock, ground: &dyn Fn(f32, f32) -> Option<f32>) -> Matrix {
    let [w, h, d] = r.dimensions();
    let [x, z] = r.position;
    let yaw = r.yaw_deg.to_radians();
    let reach = (w.max(d) * 0.5).max(0.1);
    let n = ground_normal(ground, x, z, reach);
    let [right, up, fwd] = frame(yaw, n, r.align);
    // The footprint's corners and edge midpoints: the lowest of them sets the height.
    let y0 = ground(x, z).unwrap_or(0.0);
    let mut low = y0;
    for (a, b) in [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (0.7, 0.7),
        (-0.7, -0.7),
        (0.7, -0.7),
        (-0.7, 0.7),
    ] {
        let px = x + right[0] * a * w * 0.5 + fwd[0] * b * d * 0.5;
        let pz = z + right[2] * a * w * 0.5 + fwd[2] * b * d * 0.5;
        if let Some(g) = ground(px, pz) {
            // What the tilt already lowers that side by.
            let tilt =
                dot(right, [0.0, 1.0, 0.0]) * a * w * 0.5 + dot(fwd, [0.0, 1.0, 0.0]) * b * d * 0.5;
            low = low.min(g - tilt);
        }
    }
    let y = low - r.sink * h;
    let s = |v: [f32; 3], k: f32| v.map(|c| c * k);
    matrix([s(right, w), s(up, h), s(fwd, d)], [x, y, z])
}

/// Where a deadwood piece goes, for its generated mesh (`deadwood_mesh` of its `mesh_key`): a
/// lying piece along its yaw, pitched to the ground under its two ends and sunk a sixth of its
/// diameter; a standing one upright on the lowest ground round its foot.
pub fn deadwood_matrix(p: &Deadwood, ground: &dyn Fn(f32, f32) -> Option<f32>) -> Matrix {
    let (_, _, bucket) = p.mesh_key();
    let aspect = 2f32.powf(bucket as f32 * 0.5);
    let d = p.diameter_m;
    // The mesh is `aspect` long and 1 thick: stretch the length to the asked one.
    let along = p.length_m / (aspect * d);
    let [x, z] = p.position;
    let yaw = p.yaw_deg.to_radians();
    if p.kind.lies() {
        let dir = [yaw.cos(), 0.0, -yaw.sin()];
        let half = p.length_m * 0.45;
        let g = |s: f32| ground(x + dir[0] * s, z + dir[2] * s);
        let y0 = ground(x, z).unwrap_or(0.0);
        let (a, b) = (g(-half).unwrap_or(y0), g(half).unwrap_or(y0));
        let axis = norm([dir[0] * 2.0 * half, b - a, dir[2] * 2.0 * half]);
        let side = norm(cross(axis, [0.0, 1.0, 0.0]));
        let up = cross(side, axis);
        // The middle rests on the higher of the line between the ends and the ground there.
        let mid = ((a + b) * 0.5).max(y0 - d * 0.3);
        let s = |v: [f32; 3], k: f32| v.map(|c| c * k);
        // Mesh: x along the length, y up (0..1 diameter), z across.
        matrix(
            [s(axis, d * along), s(up, d), s(side, d)],
            [x, mid - d * 0.17, z],
        )
    } else {
        let reach = d * 0.75;
        let mut low = ground(x, z).unwrap_or(0.0);
        for (a, b) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            if let Some(g) = ground(x + a * reach, z + b * reach) {
                low = low.min(g);
            }
        }
        let n = ground_normal(ground, x, z, reach);
        let [right, up, fwd] = frame(yaw, n, 0.15);
        let s = |v: [f32; 3], k: f32| v.map(|c| c * k);
        matrix(
            [s(right, d), s(up, d * along), s(fwd, d)],
            [x, low - d * 0.15, z],
        )
    }
}

/// The scale a catalogue model of `size` (width, height, depth in metres) gets to stand for a
/// rock of `r`'s width, or a deadwood piece of its length.
pub fn model_scale(size: [f32; 3], longest_m: f32) -> f32 {
    longest_m / size[0].max(size[1]).max(size[2]).max(1e-3)
}

/// Placement of a catalogue model standing in for a rock or a deadwood piece: its base on the
/// lowest ground round it, sunk `sink` of its height, turned and tilted as the generated one.
pub fn model_matrix_at(
    position: [f32; 2],
    yaw_deg: f32,
    align: f32,
    size: [f32; 3],
    scale: f32,
    sink: f32,
    ground: &dyn Fn(f32, f32) -> Option<f32>,
) -> Matrix {
    let [x, z] = position;
    let reach = (size[0].max(size[2]) * scale * 0.5).max(0.1);
    let n = ground_normal(ground, x, z, reach);
    let [right, up, fwd] = frame(yaw_deg.to_radians(), n, align);
    let mut low = ground(x, z).unwrap_or(0.0);
    for (a, b) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        if let Some(g) = ground(x + a * reach, z + b * reach) {
            low = low.min(g);
        }
    }
    let s = |v: [f32; 3]| v.map(|c| c * scale);
    matrix(
        [s(right), s(up), s(fwd)],
        [x, low - sink * size[1] * scale, z],
    )
}

// ---- meshes ----

/// A generated mesh: `indices[..split]` take the main material (rock, bark), the rest the
/// second (the cut wood of a stump's top and a log's ends; none for a rock).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NaturalMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub split: usize,
}

fn hash3(seed: u64, x: i64, y: i64, z: i64) -> f32 {
    Rng::new(mix(mix(mix(seed, x), y), z)).f32() * 2.0 - 1.0
}

/// 3D value noise in -1..1.
fn noise3(seed: u64, p: [f32; 3]) -> f32 {
    let i = p.map(|v| v.floor());
    let f = [0, 1, 2].map(|k| {
        let t = p[k] - i[k];
        t * t * (3.0 - 2.0 * t)
    });
    let c = |dx: i64, dy: i64, dz: i64| {
        hash3(seed, i[0] as i64 + dx, i[1] as i64 + dy, i[2] as i64 + dz)
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(c(0, 0, 0), c(1, 0, 0), f[0]);
    let x10 = lerp(c(0, 1, 0), c(1, 1, 0), f[0]);
    let x01 = lerp(c(0, 0, 1), c(1, 0, 1), f[0]);
    let x11 = lerp(c(0, 1, 1), c(1, 1, 1), f[0]);
    lerp(lerp(x00, x10, f[1]), lerp(x01, x11, f[1]), f[2])
}

fn fbm3(seed: u64, p: [f32; 3], octaves: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut total) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += noise3(seed.wrapping_add(u64::from(o) * 101), p.map(|v| v * freq)) * amp;
        total += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / total
}

/// An icosphere of radius 1 with `subdivisions` (0: 12 vertices, 3: 642).
fn icosphere(subdivisions: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut p: Vec<[f32; 3]> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .map(norm)
    .to_vec();
    let mut f: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdivisions {
        let mut mid = std::collections::HashMap::new();
        let mut half = |a: u32, b: u32, p: &mut Vec<[f32; 3]>| {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let (u, v) = (p[a as usize], p[b as usize]);
                p.push(norm([
                    (u[0] + v[0]) * 0.5,
                    (u[1] + v[1]) * 0.5,
                    (u[2] + v[2]) * 0.5,
                ]));
                p.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(f.len() * 4);
        for [a, b, c] in f {
            let (ab, bc, ca) = (half(a, b, &mut p), half(b, c, &mut p), half(c, a, &mut p));
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = next;
    }
    (p, f.into_iter().flatten().collect())
}

fn smooth_normals(p: &[[f32; 3]], idx: &[u32]) -> Vec<[f32; 3]> {
    let mut n = vec![[0.0f32; 3]; p.len()];
    for t in idx.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| p[i as usize]);
        let f = cross(
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        for &i in t {
            for k in 0..3 {
                n[i as usize][k] += f[k];
            }
        }
    }
    n.into_iter().map(norm).collect()
}

/// A rock of `shape`, variant `variant`, in unit size: x and z in -0.5..0.5, y in 0..1 (its
/// bottom flat, to be sunk). `fine`: 642 vertices, else 162.
pub fn rock_mesh(shape: RockShape, variant: u32, fine: bool) -> NaturalMesh {
    let (mut p, idx) = icosphere(if fine { 3 } else { 2 });
    let seed = mix(0x0B0C_u64 ^ (shape as u64 * 977), i64::from(variant));
    let mut rng = Rng::new(seed);
    // Fracture planes: each flattens the side it cuts (an outcrop has more, cut deeper).
    let (cuts, depth) = match shape {
        RockShape::Outcrop => (11, 0.62),
        RockShape::Slab => (8, 0.7),
        RockShape::Boulder => (7, 0.78),
        RockShape::Cobble => (4, 0.88),
    };
    let planes: Vec<([f32; 3], f32)> = (0..cuts)
        .map(|_| {
            let n = norm([
                rng.f32() * 2.0 - 1.0,
                (rng.f32() * 2.0 - 1.0) * 0.8,
                rng.f32() * 2.0 - 1.0,
            ]);
            (n, depth + rng.f32() * 0.18)
        })
        .collect();
    let rough = match shape {
        RockShape::Cobble => 0.05,
        RockShape::Outcrop => 0.12,
        _ => 0.09,
    };
    for v in &mut p {
        // Low-frequency lumps, then the cuts, then fine roughness.
        let lump = 1.0 + fbm3(seed, v.map(|c| c * 1.3), 2) * 0.18;
        let mut q = v.map(|c| c * lump);
        for (n, d) in &planes {
            let s = dot(q, *n) - d;
            if s > 0.0 {
                q = [q[0] - n[0] * s, q[1] - n[1] * s, q[2] - n[2] * s];
            }
        }
        let fine_noise = fbm3(seed ^ 0xF1, q.map(|c| c * 4.0), 3) * rough;
        let dir = norm(q);
        q = [
            q[0] + dir[0] * fine_noise,
            q[1] + dir[1] * fine_noise,
            q[2] + dir[2] * fine_noise,
        ];
        // A flat underside.
        q[1] = q[1].max(-0.55);
        *v = q;
    }
    // Into the unit box.
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for v in &p {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    for v in &mut p {
        *v = [
            (v[0] - lo[0]) / (hi[0] - lo[0]) - 0.5,
            (v[1] - lo[1]) / (hi[1] - lo[1]),
            (v[2] - lo[2]) / (hi[2] - lo[2]) - 0.5,
        ];
    }
    let normals = smooth_normals(&p, &idx);
    let split = idx.len();
    NaturalMesh {
        positions: p,
        normals,
        indices: idx,
        split,
    }
}

/// A tube along a centre line: rings of `sides` vertices at `centres` with radii `radii`,
/// each ring perpendicular to the line. Appends to `m` and returns the first ring's index.
fn tube(
    m: &mut NaturalMesh,
    centres: &[[f32; 3]],
    radii: &[f32],
    sides: u32,
    seed: u64,
    bark: f32,
) -> u32 {
    let first = m.positions.len() as u32;
    let n = centres.len();
    let mut along_total = 0.0;
    for i in 0..n {
        let a = centres[i.saturating_sub(1)];
        let b = centres[(i + 1).min(n - 1)];
        let axis = norm([b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
        let helper = if axis[1].abs() < 0.9 {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let u = norm(cross(axis, helper));
        let v = cross(axis, u);
        if i > 0 {
            let p = centres[i - 1];
            let c = centres[i];
            along_total +=
                ((c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2) + (c[2] - p[2]).powi(2)).sqrt();
        }
        for s in 0..sides {
            let ang = s as f32 / sides as f32 * std::f32::consts::TAU;
            let (sn, cs) = ang.sin_cos();
            // Bark: ridged plates between furrows running along the trunk (the furrows where
            // the noise crosses zero), finer cracks over them, and slow bulges.
            let plates = 0.5 - noise3(seed, [cs * 2.5, sn * 2.5, along_total * 0.5]).abs();
            let cracks = noise3(seed ^ 0xBA, [cs * 5.0, sn * 5.0, along_total * 1.6]);
            let bulge = noise3(seed ^ 0x51, [cs, sn, along_total * 0.25]);
            let ridge = (plates * 1.4 + cracks * 0.35 + bulge * 0.5) * bark;
            let r = radii[i] * (1.0 + ridge);
            let d = [
                u[0] * cs + v[0] * sn,
                u[1] * cs + v[1] * sn,
                u[2] * cs + v[2] * sn,
            ];
            let c = centres[i];
            m.positions
                .push([c[0] + d[0] * r, c[1] + d[1] * r, c[2] + d[2] * r]);
            m.normals.push(d);
        }
    }
    for i in 0..n as u32 - 1 {
        for s in 0..sides {
            let a = first + i * sides + s;
            let b = first + i * sides + (s + 1) % sides;
            let (c, d) = (a + sides, b + sides);
            m.indices.extend([a, b, c, b, d, c]);
        }
    }
    first
}

/// Closes the ring at `ring` (of `sides` vertices) with a fan to `centre`, facing `out`.
fn cap(
    m: &mut NaturalMesh,
    ring: u32,
    sides: u32,
    centre: [f32; 3],
    out: [f32; 3],
    jag: f32,
    seed: u64,
) -> Vec<u32> {
    let mut tris = Vec::new();
    let mut rng = Rng::new(seed);
    // A broken end: the rim and the middle stand proud by a random amount.
    let c = m.positions.len() as u32;
    let lift = jag * (0.3 + rng.f32());
    m.positions.push([
        centre[0] + out[0] * lift,
        centre[1] + out[1] * lift,
        centre[2] + out[2] * lift,
    ]);
    m.normals.push(out);
    let rim0 = m.positions.len() as u32;
    for s in 0..sides {
        let p = m.positions[(ring + s) as usize];
        let k = jag * rng.f32();
        m.positions
            .push([p[0] + out[0] * k, p[1] + out[1] * k, p[2] + out[2] * k]);
        m.normals.push(out);
    }
    // Wound to face `out`, whichever way the ring runs.
    let (p0, p1) = (m.positions[rim0 as usize], m.positions[rim0 as usize + 1]);
    let q = m.positions[c as usize];
    let face = cross(
        [p0[0] - q[0], p0[1] - q[1], p0[2] - q[2]],
        [p1[0] - q[0], p1[1] - q[1], p1[2] - q[2]],
    );
    let flip = dot(face, out) < 0.0;
    for s in 0..sides {
        let (a, b) = (rim0 + s, rim0 + (s + 1) % sides);
        tris.extend(if flip { [c, b, a] } else { [c, a, b] });
    }
    tris
}

/// A deadwood piece in unit thickness: lying kinds run along +x from 0 to `aspect` with their
/// axis 0.5 above y = 0; standing kinds rise along +y from 0 to `aspect`, centred on x = z = 0.
pub fn deadwood_mesh(kind: DeadwoodKind, variant: u32, bucket: u32, fine: bool) -> NaturalMesh {
    let aspect = 2f32.powf(bucket as f32 * 0.5);
    let seed = mix(0xDEAD ^ (kind as u64 * 31), i64::from(variant));
    let mut rng = Rng::new(seed);
    // Fine enough round for the bark's plates to show in the outline and the shading.
    let sides = if fine { 16 } else { 6 };
    let mut m = NaturalMesh::default();
    let mut ends: Vec<u32> = Vec::new();
    let rings = |len: f32| ((len * if fine { 4.0 } else { 1.0 }).ceil() as usize).clamp(3, 64);
    match kind {
        DeadwoodKind::Log | DeadwoodKind::Branch => {
            let branch = kind == DeadwoodKind::Branch;
            let n = rings(aspect);
            // A trunk is seldom straight: a sideways bow over its length with a second, shorter
            // wave on it, and a little sag into the ground (never a bow up: it would float).
            let pi = std::f32::consts::PI;
            let bow = (0.04 + 0.05 * rng.f32()) * aspect * if rng.f32() < 0.5 { -1.0 } else { 1.0 };
            let wave = (rng.f32() - 0.5) * 0.04 * aspect;
            let phase = rng.f32() * pi;
            let sag = -rng.f32() * 0.012 * aspect;
            let centres: Vec<[f32; 3]> = (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    let curve = (t * pi).sin();
                    let side = bow * curve + wave * (t * 2.0 * pi + phase).sin() * curve;
                    [t * aspect - aspect * 0.5, 0.5 + sag * curve, side]
                })
                .collect();
            // Tapering towards the top, the butt flaring into the root collar of a log.
            let taper = if branch { 0.45 } else { 0.62 };
            let radii: Vec<f32> = (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    let collar = if branch {
                        0.0
                    } else {
                        0.22 * (-t * aspect / 0.6).exp()
                    };
                    0.5 * (1.0 - (1.0 - taper) * t) * (1.0 + collar)
                })
                .collect();
            let bark = if branch { 0.05 } else { 0.09 };
            let first = tube(&mut m, &centres, &radii, sides, seed, bark);
            let last = first + n as u32 * sides;
            // Broken ends: each rim slanted across the trunk (a break is never square), the top
            // more than the butt; `cap` then splinters it.
            for (ring, slant) in [(first, 0.25 * rng.f32()), (last, 0.5 + 0.6 * rng.f32())] {
                let turn = rng.f32() * std::f32::consts::TAU;
                let (ts, tc) = turn.sin_cos();
                for s in 0..sides {
                    let p = &mut m.positions[(ring + s) as usize];
                    let c = if ring == first {
                        centres[0]
                    } else {
                        centres[n]
                    };
                    let across = (p[1] - c[1]) * tc + (p[2] - c[2]) * ts;
                    let along = if ring == first { -1.0 } else { 1.0 };
                    p[0] += across * slant * along;
                }
            }
            ends.extend(cap(
                &mut m,
                first,
                sides,
                centres[0],
                [-1.0, 0.0, 0.0],
                if branch { 0.1 } else { 0.3 },
                seed ^ 1,
            ));
            ends.extend(cap(
                &mut m,
                last,
                sides,
                centres[n],
                [1.0, 0.0, 0.0],
                0.8,
                seed ^ 2,
            ));
            // A branch forks; a log keeps the broken stubs of its branches, short and jagged,
            // up or to the side, never into the ground.
            let stubs = if branch {
                2
            } else {
                2 + (rng.f32() * 3.0) as u32
            };
            for k in 0..stubs {
                let t = 0.2 + 0.7 * rng.f32();
                let i = ((t * n as f32) as usize).min(n);
                let c = centres[i];
                let ang = rng.f32() * std::f32::consts::TAU;
                let dir = norm([
                    0.35 + rng.f32() * 0.5,
                    ang.sin().abs() * 0.9 + 0.1,
                    ang.cos(),
                ]);
                let (len, r0) = if branch {
                    (aspect * (0.25 + 0.2 * rng.f32()), 0.3)
                } else {
                    (
                        0.35 + 0.7 * rng.f32(),
                        (0.1 + 0.08 * rng.f32()) * radii[i] * 2.0,
                    )
                };
                let steps = if fine { 4 } else { 2 };
                let cs: Vec<[f32; 3]> = (0..=steps)
                    .map(|j| {
                        let s = len * j as f32 / steps as f32;
                        [c[0] + dir[0] * s, c[1] + dir[1] * s, c[2] + dir[2] * s]
                    })
                    .collect();
                let rs: Vec<f32> = (0..=steps)
                    .map(|j| r0 * (1.0 - 0.45 * j as f32 / steps as f32))
                    .collect();
                let s0 = tube(&mut m, &cs, &rs, sides.min(6), seed ^ (k as u64 + 9), 0.05);
                ends.extend(cap(
                    &mut m,
                    s0 + steps * sides.min(6),
                    sides.min(6),
                    cs[steps as usize],
                    dir,
                    if branch { 0.05 } else { 0.25 },
                    seed ^ (k as u64 + 20),
                ));
            }
        }
        DeadwoodKind::Stump | DeadwoodKind::Snag => {
            let n = rings(aspect).max(if fine { 8 } else { 4 });
            let lean = [(rng.f32() - 0.5) * 0.06, (rng.f32() - 0.5) * 0.06];
            let centres: Vec<[f32; 3]> = (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    let y = t * aspect;
                    [lean[0] * y, y, lean[1] * y]
                })
                .collect();
            let taper = if kind == DeadwoodKind::Snag {
                0.5
            } else {
                0.95
            };
            let radii: Vec<f32> = (0..=n)
                .map(|i| {
                    let y = i as f32 / n as f32 * aspect;
                    // Root flare: half again as wide at the ground, gone by 0.6 diameters up.
                    let flare = 1.0 + 0.6 * (-y / 0.25).exp();
                    0.5 * flare * (1.0 - (1.0 - taper) * i as f32 / n as f32)
                })
                .collect();
            let first = tube(&mut m, &centres, &radii, sides, seed, 0.1);
            // Root buttresses: a few ribs bulging at the foot.
            let ribs = 4 + (rng.f32() * 2.0) as u32;
            let phase = rng.f32() * std::f32::consts::TAU;
            for i in 0..=n as u32 {
                let y = m.positions[(first + i * sides) as usize][1];
                let fade = (-y / 0.35).exp();
                for s in 0..sides {
                    let k = (first + i * sides + s) as usize;
                    let ang = s as f32 / sides as f32 * std::f32::consts::TAU;
                    let rib = ((ang * ribs as f32 + phase).cos().max(0.0)).powi(3) * 0.35 * fade;
                    let c = centres[i as usize];
                    let p = m.positions[k];
                    m.positions[k] = [p[0] + (p[0] - c[0]) * rib, p[1], p[2] + (p[2] - c[2]) * rib];
                }
            }
            let top = first + n as u32 * sides;
            let jag = if kind == DeadwoodKind::Snag {
                1.2
            } else {
                0.08
            };
            ends.extend(cap(
                &mut m,
                top,
                sides,
                centres[n],
                [0.0, 1.0, 0.0],
                jag,
                seed ^ 3,
            ));
            if kind == DeadwoodKind::Snag {
                let stubs = 3 + (rng.f32() * 4.0) as u32;
                for k in 0..stubs {
                    let y = aspect * (0.35 + 0.55 * rng.f32());
                    let ang = rng.f32() * std::f32::consts::TAU;
                    let dir = norm([ang.cos(), 0.3 + 0.5 * rng.f32(), ang.sin()]);
                    let c = [lean[0] * y, y, lean[1] * y];
                    let len = 0.8 + 2.5 * rng.f32();
                    let steps = if fine { 4 } else { 2 };
                    let cs: Vec<[f32; 3]> = (0..=steps)
                        .map(|j| {
                            let s = len * j as f32 / steps as f32;
                            [
                                c[0] + dir[0] * s,
                                c[1] + dir[1] * s - 0.05 * s * s,
                                c[2] + dir[2] * s,
                            ]
                        })
                        .collect();
                    let rs: Vec<f32> = (0..=steps)
                        .map(|j| 0.14 * (1.0 - 0.7 * j as f32 / steps as f32))
                        .collect();
                    let s0 = tube(&mut m, &cs, &rs, sides.min(6), seed ^ (k as u64 + 40), 0.05);
                    ends.extend(cap(
                        &mut m,
                        s0 + steps * sides.min(6),
                        sides.min(6),
                        cs[steps as usize],
                        dir,
                        0.1,
                        seed ^ (k as u64 + 60),
                    ));
                }
            }
        }
    }
    let normals = smooth_normals(&m.positions, &m.indices);
    // The caps keep their flat normals; the tubes get smooth ones from their triangles.
    for (i, n) in normals.into_iter().enumerate() {
        if dot(n, n) > 0.5 {
            m.normals[i] = n;
        }
    }
    m.split = m.indices.len();
    m.indices.extend(ends);
    m
}

// ---- scattering ----

/// Points in `outline` at about `per_100m2` per hundred square metres, from `seed`, thinned by
/// `keep(x, z)` (0..1). Clustered: points come in groups of two to five round seed points.
pub fn scatter_points(
    outline: &[[f32; 2]],
    per_100m2: f32,
    seed: u64,
    clustered: bool,
    keep: &dyn Fn(f32, f32) -> f32,
) -> Vec<[f32; 2]> {
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for p in outline {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if outline.len() < 3 || per_100m2 <= 0.0 || lo[0] >= hi[0] || lo[1] >= hi[1] {
        return Vec::new();
    }
    let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
    let mut rng = Rng::new(mix(seed, 0x5CA7));
    let tries = ((area * per_100m2 / 100.0) as usize).min(20_000);
    let mut out = Vec::new();
    if clustered {
        let groups = tries.div_ceil(3);
        for _ in 0..groups {
            let c = [
                lo[0] + rng.f32() * (hi[0] - lo[0]),
                lo[1] + rng.f32() * (hi[1] - lo[1]),
            ];
            let n = 2 + (rng.f32() * 4.0) as usize;
            let spread = 0.8 + rng.f32() * 2.0;
            for _ in 0..n {
                let a = rng.f32() * std::f32::consts::TAU;
                let r = spread * rng.f32().sqrt();
                let p = [c[0] + a.cos() * r, c[1] + a.sin() * r];
                if point_in_polygon(p, outline) && rng.f32() < keep(p[0], p[1]) {
                    out.push(p);
                }
            }
        }
    } else {
        for _ in 0..tries {
            let p = [
                lo[0] + rng.f32() * (hi[0] - lo[0]),
                lo[1] + rng.f32() * (hi[1] - lo[1]),
            ];
            if point_in_polygon(p, outline) && rng.f32() < keep(p[0], p[1]) {
                out.push(p);
            }
        }
    }
    out
}

/// How much a scatter of rocks keeps at `(x, z)` when it follows the terrain: everywhere a
/// little, much more where the automatic (or painted) rock and scree are and on steep ground.
pub fn rockiness(t: &Terrain, x: f32, z: f32) -> f32 {
    let Some(w) = t.drawn_weights_at(x, z) else {
        return 0.15;
    };
    let [rock, scree, _, _] = class_layers(t).map(|l| l.map_or(0.0, |l| f32::from(w[l]) / 255.0));
    let n = t.normal_at(x, z);
    let steep = sstep(0.95, 0.75, n[1]);
    (0.15 + 0.85 * (rock + scree).max(steep * 0.6)).min(1.0)
}

/// A pseudo-random number in 0..1 for item `k` of a scatter.
pub fn unit(seed: u64, k: i64) -> f32 {
    Rng::new(mix(seed, k)).f32()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slope() -> Terrain {
        let mut t = Terrain::flat([0.0, 0.0], 40.0, 40.0, 0.5, "lawn").unwrap();
        // A ramp up to 20 m over 20 m along x: 45 degrees at the steep end.
        for iz in 0..=t.depth {
            for ix in 0..=t.width {
                let x = ix as f32 * t.cell_m;
                let i = t.at(ix, iz);
                t.heights[i] = if x > 20.0 { (x - 20.0) * 1.2 } else { 0.0 };
            }
        }
        for m in ["rock", "scree", "moss", "forest-floor"] {
            t.add_layer(m);
        }
        t
    }

    #[test]
    fn steep_ground_turns_to_rock_and_flat_ground_stays_the_base() {
        let mut t = slope();
        t.auto = Some(AutoMaterials {
            macro_strength: 0.0,
            ..Default::default()
        });
        let flat = t.drawn_weights_at(5.0, 20.0).unwrap();
        assert_eq!(flat[0], 255, "{flat:?}");
        let steep = t.drawn_weights_at(30.0, 20.0).unwrap();
        assert!(steep[1] > 200, "rock on 50 degrees: {steep:?}");
        assert_eq!(steep.iter().map(|v| u32::from(*v)).sum::<u32>(), 255);
        // The foot of the slope is concave: soil or moss collects there.
        let foot = t.drawn_weights_at(20.0, 20.0).unwrap();
        assert!(foot[0] < 128, "{foot:?}");
    }

    #[test]
    fn painted_layers_keep_their_paint() {
        let mut t = slope();
        t.auto = Some(AutoMaterials::default());
        let n = t.layers.len();
        let v = t.at(70, 40);
        t.weights[v * n..(v + 1) * n].copy_from_slice(&[0, 0, 255, 0, 0]);
        assert_eq!(t.vertex_weights(70, 40)[..n], [0, 0, 255, 0, 0]);
    }

    #[test]
    fn auto_materials_survive_the_project_file_and_change_the_whole_mesh() {
        let mut t = slope();
        let old = t.clone();
        t.auto = Some(AutoMaterials::default());
        t.hex_tiling = true;
        t.cover = Some(GroundCover::default());
        assert_eq!(t.changed_since(&old), Some(t.whole()));
        let json = serde_json::to_string(&t).unwrap();
        let back: Terrain = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
        // A flat terrain without them writes none of the new fields.
        let plain = serde_json::to_string(&old).unwrap();
        assert!(!plain.contains("auto") && !plain.contains("hex") && !plain.contains("cover"));
    }

    #[test]
    fn a_rock_never_floats_on_a_slope() {
        let t = slope();
        let ground = |x: f32, z: f32| t.height_at(x, z);
        for k in 0..20 {
            let r = Rock {
                id: format!("r{k}"),
                layer: "shapes".into(),
                position: [24.0 + k as f32 * 0.6, 10.0],
                size_m: 1.5,
                shape: RockShape::ALL[k % 4],
                yaw_deg: k as f32 * 37.0,
                seed: k as u64,
                sink: 0.2,
                align: 0.7,
                material: "rock".into(),
                model: None,
                group: None,
                hidden: false,
            };
            let m = rock_matrix(&r, &ground);
            let mesh = rock_mesh(r.shape, (r.seed % 4) as u32, false);
            // Every bottom vertex lies at or under the ground.
            for p in mesh.positions.iter().filter(|p| p[1] < 0.02) {
                let w = [
                    m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
                    m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
                    m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
                ];
                let g = t.height_at(w[0], w[2]).unwrap();
                assert!(
                    w[1] <= g + 0.05,
                    "rock {k} floats {} over the ground",
                    w[1] - g
                );
            }
        }
    }

    #[test]
    fn meshes_are_in_unit_size_with_valid_indices() {
        for shape in RockShape::ALL {
            let m = rock_mesh(shape, 1, true);
            assert_eq!(m.positions.len(), 642);
            assert!(m.indices.iter().all(|i| (*i as usize) < m.positions.len()));
            let top = m.positions.iter().map(|p| p[1]).fold(0.0, f32::max);
            assert!((top - 1.0).abs() < 1e-4);
        }
        for kind in DeadwoodKind::ALL {
            let m = deadwood_mesh(kind, 2, 6, true);
            assert!(m.split > 0 && m.split < m.indices.len(), "{kind:?}");
            assert!(m.indices.iter().all(|i| (*i as usize) < m.positions.len()));
            assert!(m.normals.iter().all(|n| n.iter().all(|c| c.is_finite())));
        }
    }

    #[test]
    fn cover_follows_the_masks_and_is_the_same_each_time() {
        let mut t = slope();
        t.auto = Some(AutoMaterials::default());
        let cover = GroundCover::default();
        let a = cover_spots(&t, &cover, [20.0, 20.0], 15.0);
        let b = cover_spots(&t, &cover, [20.0, 20.0], 15.0);
        assert_eq!(a, b);
        let stones = |lo: f32, hi: f32| {
            a.iter()
                .filter(|s| s.kind == CoverKind::Stone && s.x >= lo && s.x < hi)
                .count()
        };
        // Scree and rock on the slope, bare base on the flat.
        assert!(
            stones(22.0, 34.0) > 10 * stones(6.0, 18.0).max(1),
            "{} vs {}",
            stones(22.0, 34.0),
            stones(6.0, 18.0)
        );
    }

    #[test]
    fn a_scatter_follows_the_rock_and_stays_inside() {
        let mut t = slope();
        t.auto = Some(AutoMaterials::default());
        let outline = [[0.0, 0.0], [40.0, 0.0], [40.0, 40.0], [0.0, 40.0]];
        let pts = scatter_points(&outline, 4.0, 7, true, &|x, z| rockiness(&t, x, z));
        assert!(!pts.is_empty());
        let steep = pts.iter().filter(|p| p[0] > 22.0).count();
        let flat = pts.iter().filter(|p| p[0] < 18.0).count();
        assert!(steep > flat, "{steep} on the slope, {flat} on the flat");
        assert!(pts.iter().all(|p| point_in_polygon(*p, &outline)));
    }

    #[test]
    fn a_log_bows_tapers_and_breaks_unevenly() {
        // Bucket 6: eight diameters long; the trunk is the first tube, 16 sides round.
        let sides = 16;
        for v in 0..VARIANTS {
            let m = deadwood_mesh(DeadwoodKind::Log, v, 6, true);
            let rings = 33;
            let ring = |i: usize| &m.positions[i * sides..(i + 1) * sides];
            let centre = |i: usize| {
                let r = ring(i);
                let n = r.len() as f32;
                [0, 1, 2].map(|k| r.iter().map(|p| p[k]).sum::<f32>() / n)
            };
            let radius = |i: usize| {
                let c = centre(i);
                ring(i)
                    .iter()
                    .map(|p| ((p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt())
                    .sum::<f32>()
                    / sides as f32
            };
            let (a, mid, b) = (centre(0), centre(rings / 2), centre(rings - 1));
            let bow = (mid[2] - (a[2] + b[2]) * 0.5).abs();
            assert!(bow > 0.25, "variant {v}: bow {bow}");
            assert!(mid[1] <= 0.5 + 0.05, "variant {v} arches up: {mid:?}");
            assert!(radius(0) > 1.4 * radius(rings - 1), "variant {v}: no taper");
            // The top's rim is slanted and jagged, not a square cut.
            let xs = ring(rings - 1).iter().map(|p| p[0]);
            let spread = xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min);
            assert!(spread > 0.2, "variant {v}: square end {spread}");
        }
    }

    #[test]
    fn deadwood_lies_along_the_ground() {
        let mut t = Terrain::flat([0.0, 0.0], 20.0, 20.0, 0.5, "lawn").unwrap();
        for iz in 0..=t.depth {
            for ix in 0..=t.width {
                let i = t.at(ix, iz);
                t.heights[i] = ix as f32 * t.cell_m * 0.25;
            }
        }
        let log = Deadwood {
            id: "d1".into(),
            layer: "shapes".into(),
            kind: DeadwoodKind::Log,
            position: [10.0, 10.0],
            length_m: 4.0,
            diameter_m: 0.4,
            yaw_deg: 0.0,
            seed: 3,
            material: "bark".into(),
            model: None,
            group: None,
            hidden: false,
        };
        let ground = |x: f32, z: f32| t.height_at(x, z);
        let m = deadwood_matrix(&log, &ground);
        // Its axis runs along x and climbs with the ground.
        assert!(m[0] > 0.0 && m[1] > 0.0, "{m:?}");
        let mid_y = m[13];
        let g = t.height_at(10.0, 10.0).unwrap();
        assert!(
            (mid_y - (g - 0.4 * 0.17)).abs() < 0.2,
            "{mid_y} vs ground {g}"
        );
    }
}
