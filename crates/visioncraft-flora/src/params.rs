//! Plain, documented parameter structs. Users and third parties create their own species by
//! editing these. With the optional `serde` feature they serialise; every struct has a
//! `Default` and `#[serde(default)]`, so stored species survive fields added later.

use crate::Lod;

/// Everything that defines a species' look.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct PlantParams {
    /// Mature height in metres (in flower, for perennials whose stalks rise above the leaves).
    pub height_m: f32,
    /// Mature width (crown or clump diameter) in metres. Hedge segments: the hedge's thickness;
    /// climbers and hedges have their own length/width fields as well.
    pub width_m: f32,
    /// Growth stage, (0, 1]; 1 is mature. Geometry scales linearly with `age`, leaves keep their
    /// real size, so a young plant simply carries fewer of them.
    pub age: f32,
    /// Drops its leaves in winter.
    pub deciduous: bool,
    /// Keeps dry brown leaves through winter (young beech and hornbeam, and their hedges).
    pub marcescent: bool,
    /// Months (1..=12) with flowers.
    pub bloom_months: Vec<u8>,
    /// Triangle budget class.
    pub budget: BudgetClass,
    pub form: Form,
    pub leaves: LeafParams,
    /// `None` for plants whose flowers are not ornamentally visible.
    pub flowers: Option<FlowerParams>,
    pub bark: BarkParams,
}

/// Hard triangle limits per level of detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BudgetClass {
    LargeTree,
    /// Small trees, conifers and palms.
    #[default]
    SmallTree,
    /// Shrubs, clipped forms, hedge segments, wall climbers.
    Shrub,
    /// Perennials, ferns, ornamental grass clumps.
    Herb,
    /// Lawn tufts and ground-cover patches.
    Tuft,
}

impl BudgetClass {
    pub fn triangles(self, lod: Lod) -> usize {
        let t = match self {
            BudgetClass::LargeTree => [12_000, 3_500, 600],
            BudgetClass::SmallTree => [8_000, 2_500, 400],
            BudgetClass::Shrub => [2_500, 800, 150],
            BudgetClass::Herb => [600, 200, 40],
            BudgetClass::Tuft => [60, 24, 8],
        };
        t[lod.index()]
    }
}

/// The growth archetype, each with its own parameters.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Form {
    /// Trees, conifers, multi-stem shrubs, bamboo: recursive branching after Weber & Penn.
    Branching(BranchingParams),
    /// Single-trunk fan or feather palm.
    Palm(PalmParams),
    /// Topiary and hedges: foliage cards on a clipped surface around an opaque leafy core.
    Clipped(ClippedParams),
    /// Grass clump of arching blades, with optional plumes.
    Tussock(TussockParams),
    /// Basal leaves in a spiral (hosta, daylily, fern) or a leafy mound, with flower stalks.
    Rosette(RosetteParams),
    /// A low patch of leaves on the ground.
    GroundCover(GroundCoverParams),
    /// A rectangular patch of a self-clinging climber on a wall in the XY plane (wall at z = 0,
    /// leaves towards +Z).
    Climber(ClimberParams),
    /// Excurrent conifer with whorled tiers (spruce, fir, larch): a dedicated cone builder so
    /// the silhouette reads as a clean triangle at every distance.
    Conifer(ConiferParams),
}

/// Crown envelopes of Weber & Penn (1995): the length of a primary branch as a function of its
/// height in the crown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CrownShape {
    Conical,
    #[default]
    Spherical,
    Hemispherical,
    Cylindrical,
    TaperedCylindrical,
    /// Widest high up, like a flame.
    Flame,
    /// Widest at the top.
    InverseConical,
    TendFlame,
}

impl CrownShape {
    /// Relative length for `ratio` = 0 at the crown base to 1 at the top.
    pub fn factor(self, ratio: f32) -> f32 {
        let r = ratio.clamp(0.0, 1.0);
        match self {
            CrownShape::Conical => 0.2 + 0.8 * (1.0 - r),
            CrownShape::Spherical => 0.25 + 0.75 * crate::math::sin(crate::math::PI * r),
            CrownShape::Hemispherical => {
                0.25 + 0.75 * crate::math::sin(crate::math::PI * 0.5 * (1.0 - r) + 0.25)
            }
            CrownShape::Cylindrical => 1.0,
            CrownShape::TaperedCylindrical => 0.5 + 0.5 * (1.0 - r),
            CrownShape::Flame => {
                if r <= 0.7 {
                    0.15 + 0.85 * r / 0.7
                } else {
                    0.15 + 0.85 * (1.0 - r) / 0.3
                }
            }
            CrownShape::InverseConical => 0.3 + 0.7 * r,
            CrownShape::TendFlame => 0.5 + 0.5 * CrownShape::Flame.factor(r),
        }
    }
}

/// One level of the branch hierarchy (children of the trunk, of the limbs, of the branches).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct LevelParams {
    /// Children per parent (0 ends the hierarchy).
    pub count: u16,
    /// Length relative to the plant height (level 1) or to the parent's length (deeper).
    pub length: f32,
    /// Angle between child and parent in degrees.
    pub down_angle_deg: f32,
    pub down_angle_var_deg: f32,
    /// Azimuth step between successive nodes in degrees (137.5 spiral, 180 two-ranked).
    pub rotate_deg: f32,
    /// Children per node: 1 alternate, 2 opposite (decussate with `rotate_deg` 90), 3+ whorled.
    pub whorl: u8,
    /// Total bend along the length, degrees (positive bends towards the parent's up side).
    pub curve_deg: f32,
    /// Random bend per segment, degrees (tortuous oak limbs high, straight poplar low).
    pub curve_var_deg: f32,
    /// Tropism per unit length: > 0 bends towards +Y (fastigiate), < 0 hangs (weeping).
    pub gravity: f32,
    /// Skeleton segments.
    pub segments: u8,
    /// Base radius relative to the parent's radius where it attaches.
    pub radius_ratio: f32,
    /// Fraction of the parent's length below which no children grow.
    pub start: f32,
}

impl LevelParams {
    pub const NONE: LevelParams = LevelParams {
        count: 0,
        length: 0.0,
        down_angle_deg: 0.0,
        down_angle_var_deg: 0.0,
        rotate_deg: 137.5,
        whorl: 1,
        curve_deg: 0.0,
        curve_var_deg: 0.0,
        gravity: 0.0,
        segments: 2,
        radius_ratio: 0.5,
        start: 0.3,
    };
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct BranchingParams {
    /// 1 for a single trunk; more for multi-stemmed shrubs and small trees.
    pub stems: u16,
    /// Lean of the stems away from vertical, degrees.
    pub stem_spread_deg: f32,
    /// Radius of the circle the stems emerge from, metres.
    pub stem_base_radius_m: f32,
    /// Trunk base radius divided by plant height.
    pub trunk_radius_ratio: f32,
    /// Extra radius at the root flare, relative.
    pub trunk_flare: f32,
    /// Random lean and wobble of the trunk, degrees per segment.
    pub trunk_wobble_deg: f32,
    /// How far up the trunk continues as a leader, as a fraction of the height: 1 for excurrent
    /// growth (spruce, poplar), 0.3-0.6 for decurrent growth where the trunk dissolves into
    /// limbs (oak, apple).
    pub leader: f32,
    /// Height fraction of the lowest limbs.
    pub crown_base: f32,
    pub crown_shape: CrownShape,
    /// Trunk segments.
    pub trunk_segments: u8,
    /// Levels 1..=3 below the trunk.
    pub levels: [LevelParams; 3],
    /// Lowest branch level that carries foliage (1 = limbs, 3 = twigs).
    pub leaf_level: u8,
    /// Foliage starts at this fraction along the leafy branches.
    pub leaf_start: f32,
    /// How much foliage cards orient towards the sky (0 random, 1 flat).
    pub leaf_up: f32,
    /// Height of a support stake next to the stem (tomato), metres; 0 for none.
    pub stake_m: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ConiferParams {
    /// Branch tiers (whorls) from the crown base to the top.
    pub tiers: u16,
    /// Branches per whorl.
    pub per_whorl: u8,
    /// Height fraction of the lowest live branches.
    pub crown_base: f32,
    /// Trunk base radius divided by height.
    pub trunk_radius_ratio: f32,
    /// Branch length along the crown is `(1 - f)^taper`: 1 a straight cone, < 1 a blunter,
    /// fuller top (fir), > 1 a slender spire.
    pub taper: f32,
    /// Sag of the main branches, 0 horizontal to 1 strongly drooping.
    pub droop: f32,
    /// Upward sweep of the branch tips, 0..1.
    pub upsweep: f32,
    /// Hanging secondary branchlets (Norway spruce curtains), 0..1.
    pub pendulous: f32,
    /// Foliage in flat horizontal tiers (fir 1) rather than rounded sprays (0).
    pub layered: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct PalmParams {
    /// Number of living fronds.
    pub fronds: u16,
    /// Dead fronds hanging as a skirt under the crown.
    pub dead_fronds: u16,
    /// Frond blade (fan) length, metres.
    pub blade_m: f32,
    /// Petiole length, metres.
    pub petiole_m: f32,
    /// Trunk base radius, metres.
    pub trunk_radius_m: f32,
    /// Lean of the trunk, degrees.
    pub lean_deg: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ClipShape {
    /// Sphere of diameter `width_m`, resting on the ground.
    #[default]
    Ball,
    /// Cone of base `width_m` and height `height_m`.
    Cone,
    /// Narrow natural column with a rounded top (thuja 'Smaragd').
    Column,
    /// Flame/spindle, widest at a third of the height (Italian cypress).
    Flame,
    /// A straight hedge segment along X from `-length_m/2` to `length_m/2`, `width_m` thick and
    /// `height_m` high, with a slight batter. Segments tile end to end.
    HedgeSegment { length_m: f32 },
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ClippedParams {
    pub shape: ClipShape,
    /// Surface irregularity, metres (0.01 freshly clipped, 0.08 shaggy natural column).
    pub roughness: f32,
    /// How far foliage cards stand out of the core surface, metres.
    pub tuft_m: f32,
    /// Short bare stem(s) visible under the foliage (ball on legs, cypress trunk), metres.
    pub stem_m: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TussockParams {
    /// Blades at LOD0.
    pub blades: u16,
    /// Blade width, metres.
    pub blade_width_m: f32,
    /// Radius of the clump's base, metres.
    pub base_radius_m: f32,
    /// Blade angle from vertical: inner blades `min`, outer blades `max`, degrees.
    pub min_angle_deg: f32,
    pub max_angle_deg: f32,
    /// Droop of the tips (0 stiff, 1 strongly arching).
    pub droop: f32,
    /// Height of the foliage mound relative to `height_m` (plumes reach `height_m`).
    pub foliage_height: f32,
    /// Vertical fan cards through the clump's axis, each showing many arching blades (the
    /// clump's mass); 0 for single-blade tufts.
    pub fans: u16,
    /// Flowering plumes (colour and shape from `flowers`).
    pub plumes: u16,
    /// Plume length, metres.
    pub plume_m: f32,
    /// Spread of the plume stalks from vertical, degrees.
    pub plume_spread_deg: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct RosetteParams {
    /// Leaves (or leafy shoots) at LOD0.
    pub leaves: u16,
    /// Leaf length including petiole, metres.
    pub leaf_m: f32,
    /// Elevation of the leaves above horizontal: inner `max`, outer `min`, degrees.
    pub min_elev_deg: f32,
    pub max_elev_deg: f32,
    /// Bend of the leaf towards the ground along its length, degrees.
    pub arch_deg: f32,
    /// Cards show a leafy shoot (spray, small-leaved mounds) instead of one leaf.
    pub shoots: bool,
    /// Flower stalks at LOD0.
    pub stalks: u16,
    /// Spread of the stalks from vertical, degrees.
    pub stalk_spread_deg: f32,
    /// Height of the leaf mound relative to `height_m`.
    pub foliage_height: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct GroundCoverParams {
    /// Leaf cards at LOD0.
    pub cards: u16,
    /// Height of the leaf layer, metres.
    pub depth_m: f32,
    /// Largest tilt of the leaf cards from horizontal, degrees (0 for floating pads).
    pub tilt_deg: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ClimberParams {
    /// Patch width along X, metres (height is `height_m`).
    pub patch_width_m: f32,
    /// Main stems climbing the patch.
    pub stems: u16,
    /// How far the leaves stand off the wall, metres.
    pub depth_m: f32,
}

/// Leaf silhouettes (standard botanical terms).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LeafShape {
    /// Egg-shaped, widest below the middle (beech, apple, cherry).
    #[default]
    Ovate,
    /// Widest in the middle (box, laurel, rhododendron).
    Elliptic,
    /// Long and narrow, tapering both ends (willow, olive, bamboo).
    Lanceolate,
    /// Strap or grass blade.
    Linear,
    /// Heart-shaped base (lime, hosta).
    Cordate,
    /// Triangular with a pointed tip (birch).
    Deltoid,
    /// Rounded sinuate lobes (oak), `lobes` per side.
    Lobed { lobes: u8 },
    /// Palmately lobed (maple, plane, ivy, cranesbill); `depth` 0..1 of the incisions.
    Palmate { lobes: u8, depth: f32 },
    /// Pinnately compound with `leaflets` leaflets (rowan, walnut, rose).
    Pinnate { leaflets: u8 },
    /// Needle sprays; `length_m` per needle, `bundle` needles per fascicle (pine 2, fir 1).
    Needles { length_m: f32, bundle: u8 },
    /// Overlapping scale leaves in flattened sprays (thuja, cypress).
    Scale,
    /// Palmate palm frond.
    FanFrond,
    /// Pinnate palm frond.
    FeatherFrond,
    /// Bipinnate fern frond.
    FernFrond,
    /// Round with a narrow slit to the centre (water lily pad, nasturtium-like).
    Orbicular,
    /// Palmately compound: `leaflets` leaflets radiating from one point (horse chestnut).
    Digitate { leaflets: u8 },
    /// Fan-shaped with a notched rim (ginkgo).
    Flabellate,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct LeafParams {
    pub shape: LeafShape,
    /// Length of one leaf (a whole compound leaf or frond for those shapes), metres.
    pub leaf_m: f32,
    /// Width / length of a simple leaf (or leaflet).
    pub aspect: f32,
    /// Edge serration amplitude, 0..1.
    pub serration: f32,
    /// Upper-side colour, sRGB 0..1.
    pub colour: [f32; 3],
    /// Random colour variation between leaves and cells, 0..1.
    pub colour_var: f32,
    /// Autumn colour (deciduous), sRGB 0..1.
    pub autumn_colour: [f32; 3],
    /// Edge length of one foliage card at LOD0, metres: a twig spray with several leaves for
    /// trees, one leaf for rosettes.
    pub card_m: f32,
    /// Coverage multiplier. Branching forms: 1 means the cards' total area covers the crown
    /// envelope's surface 1.5 times; clipped forms and climbers: card area per surface area.
    pub density: f32,
}

/// Flower and inflorescence types.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FlowerShape {
    /// Ray florets around a disc (coneflower; `droop` the rays hang).
    Daisy { droop: bool },
    /// Clusters of small five-petalled flowers along a twig (cherry, apple, rowan corymbs).
    #[default]
    Blossom,
    /// Dome of four-sepalled florets (mophead hydrangea).
    Mophead,
    /// Layered petals (rose).
    Rose,
    /// Six-tepalled star/trumpet (daylily).
    Lily,
    /// Large upright goblet (magnolia).
    Goblet,
    /// Rounded truss of funnel flowers (rhododendron).
    Truss,
    /// Narrow spike of whorled florets (lavender, catmint, hosta).
    Spike,
    /// Long conical, arching panicle (butterfly bush).
    Panicle,
    /// Feathery grass plume (miscanthus, pampas, feather reed grass).
    Plume,
    /// Cylindrical bristly grass spike (fountain grass).
    Bottlebrush,
    /// Saucer of five petals (cranesbill).
    Saucer,
    /// Round fruits in small clusters (tomato, holly and skimmia berries, strawberries).
    Fruit,
    /// Hanging raceme of pea-like flowers (wisteria, robinia, pieris).
    Raceme,
    /// Six tepals around a central trumpet (daffodil).
    Trumpet,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct FlowerParams {
    pub shape: FlowerShape,
    pub colour: [f32; 3],
    /// Centre/disc or stalk colour.
    pub centre: [f32; 3],
    /// Size of one flower card at LOD0, metres (grass plumes: the plume's width).
    pub card_m: f32,
    /// Branching forms: flower cards per foliage card. Rosettes/tussocks: flowers per stalk.
    pub density: f32,
}

/// Bark patterns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BarkPattern {
    /// Smooth grey (beech, young maple, magnolia).
    Smooth,
    /// White with dark horizontal lenticels and black fissured patches (birch).
    Birch,
    /// Deep vertical furrows and ridges (oak, walnut, lime, poplar, willow).
    #[default]
    Furrowed,
    /// Flaking plates (Scots pine upper trunk, plane camouflage, yew).
    Flaky,
    /// Smooth with vertical muscle-like flutes (hornbeam).
    Fluted,
    /// Fibrous hairy mat (Chusan palm).
    Fibrous,
    /// Herbaceous or young-shoot stem, with fine longitudinal lines (bamboo culm, dogwood).
    Stem,
    /// Scaly plates of spruce and fir.
    Scaly,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct BarkParams {
    pub pattern: BarkPattern,
    /// Main colour, sRGB 0..1.
    pub colour: [f32; 3],
    /// Furrow / lenticel / accent colour.
    pub accent: [f32; 3],
}

// ---- defaults: a generic small deciduous tree and neutral archetype settings ---------------

impl Default for PlantParams {
    fn default() -> Self {
        PlantParams {
            height_m: 8.0,
            width_m: 6.0,
            age: 1.0,
            deciduous: true,
            marcescent: false,
            bloom_months: Vec::new(),
            budget: BudgetClass::SmallTree,
            form: Form::default(),
            leaves: LeafParams::default(),
            flowers: None,
            bark: BarkParams::default(),
        }
    }
}

impl Default for Form {
    fn default() -> Self {
        Form::Branching(BranchingParams::default())
    }
}

impl Default for LevelParams {
    fn default() -> Self {
        LevelParams::NONE
    }
}

impl Default for BranchingParams {
    fn default() -> Self {
        let lv = |count, length, down, segments| LevelParams {
            count,
            length,
            down_angle_deg: down,
            down_angle_var_deg: 10.0,
            segments,
            ..LevelParams::NONE
        };
        BranchingParams {
            stems: 1,
            stem_spread_deg: 0.0,
            stem_base_radius_m: 0.0,
            trunk_radius_ratio: 0.025,
            trunk_flare: 0.3,
            trunk_wobble_deg: 2.5,
            leader: 0.6,
            crown_base: 0.25,
            crown_shape: CrownShape::Spherical,
            trunk_segments: 10,
            levels: [
                lv(9, 0.5, 50.0, 5),
                lv(8, 0.5, 45.0, 3),
                lv(5, 0.4, 40.0, 2),
            ],
            leaf_level: 3,
            leaf_start: 0.2,
            leaf_up: 0.3,
            stake_m: 0.0,
        }
    }
}

impl Default for ConiferParams {
    fn default() -> Self {
        ConiferParams {
            tiers: 30,
            per_whorl: 5,
            crown_base: 0.05,
            trunk_radius_ratio: 0.02,
            taper: 1.0,
            droop: 0.3,
            upsweep: 0.4,
            pendulous: 0.3,
            layered: 0.3,
        }
    }
}

impl Default for PalmParams {
    fn default() -> Self {
        PalmParams {
            fronds: 24,
            dead_fronds: 6,
            blade_m: 0.8,
            petiole_m: 0.5,
            trunk_radius_m: 0.15,
            lean_deg: 5.0,
        }
    }
}

impl Default for ClippedParams {
    fn default() -> Self {
        ClippedParams {
            shape: ClipShape::Ball,
            roughness: 0.02,
            tuft_m: 0.05,
            stem_m: 0.0,
        }
    }
}

impl Default for TussockParams {
    fn default() -> Self {
        TussockParams {
            blades: 50,
            blade_width_m: 0.008,
            base_radius_m: 0.1,
            min_angle_deg: 10.0,
            max_angle_deg: 55.0,
            droop: 0.6,
            foliage_height: 0.8,
            fans: 10,
            plumes: 0,
            plume_m: 0.2,
            plume_spread_deg: 15.0,
        }
    }
}

impl Default for RosetteParams {
    fn default() -> Self {
        RosetteParams {
            leaves: 20,
            leaf_m: 0.25,
            min_elev_deg: 15.0,
            max_elev_deg: 60.0,
            arch_deg: 25.0,
            shoots: false,
            stalks: 0,
            stalk_spread_deg: 15.0,
            foliage_height: 0.6,
        }
    }
}

impl Default for GroundCoverParams {
    fn default() -> Self {
        GroundCoverParams {
            cards: 30,
            depth_m: 0.1,
            tilt_deg: 45.0,
        }
    }
}

impl Default for ClimberParams {
    fn default() -> Self {
        ClimberParams {
            patch_width_m: 2.0,
            stems: 3,
            depth_m: 0.12,
        }
    }
}

impl Default for LeafParams {
    fn default() -> Self {
        LeafParams {
            shape: LeafShape::Ovate,
            leaf_m: 0.08,
            aspect: 0.6,
            serration: 0.2,
            colour: [0.3, 0.43, 0.16],
            colour_var: 0.3,
            autumn_colour: [0.75, 0.6, 0.2],
            card_m: 0.6,
            density: 2.2,
        }
    }
}

impl Default for FlowerParams {
    fn default() -> Self {
        FlowerParams {
            shape: FlowerShape::Blossom,
            colour: [0.95, 0.9, 0.92],
            centre: [0.9, 0.8, 0.3],
            card_m: 0.25,
            density: 0.2,
        }
    }
}

impl Default for BarkParams {
    fn default() -> Self {
        BarkParams {
            pattern: BarkPattern::Furrowed,
            colour: [0.45, 0.4, 0.35],
            accent: [0.22, 0.2, 0.17],
        }
    }
}

// ---- validation -------------------------------------------------------------------------------

struct Check(Vec<String>);

impl Check {
    fn range(&mut self, name: &str, v: f32, lo: f32, hi: f32) {
        if !v.is_finite() || v < lo || v > hi {
            self.0.push(format!("{name} = {v} is outside [{lo}, {hi}]"));
        }
    }
    fn count(&mut self, name: &str, v: u32, lo: u32, hi: u32) {
        if v < lo || v > hi {
            self.0.push(format!("{name} = {v} is outside [{lo}, {hi}]"));
        }
    }
    fn colour(&mut self, name: &str, c: [f32; 3]) {
        for (k, v) in c.into_iter().enumerate() {
            self.range(&format!("{name}[{k}]"), v, 0.0, 1.0);
        }
    }
}

impl PlantParams {
    /// Report every out-of-range value (nothing is clamped). Generation never panics or runs
    /// away on parameters that pass; the ranges also bound the work per plant.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut c = Check(Vec::new());
        c.range("height_m", self.height_m, 0.02, 60.0);
        c.range("width_m", self.width_m, 0.02, 40.0);
        c.range("age", self.age, 0.05, 1.0);
        for &m in &self.bloom_months {
            c.count("bloom_months[]", m as u32, 1, 12);
        }
        let l = &self.leaves;
        c.range("leaves.leaf_m", l.leaf_m, 0.003, 6.0);
        c.range("leaves.aspect", l.aspect, 0.02, 2.0);
        c.range("leaves.serration", l.serration, 0.0, 1.0);
        c.colour("leaves.colour", l.colour);
        c.colour("leaves.autumn_colour", l.autumn_colour);
        c.range("leaves.colour_var", l.colour_var, 0.0, 1.0);
        c.range("leaves.card_m", l.card_m, 0.03, 6.0);
        c.range("leaves.density", l.density, 0.1, 8.0);
        match l.shape {
            LeafShape::Lobed { lobes } => c.count("leaves.shape.lobes", lobes as u32, 1, 9),
            LeafShape::Palmate { lobes, depth } => {
                c.count("leaves.shape.lobes", lobes as u32, 1, 11);
                c.range("leaves.shape.depth", depth, 0.0, 1.0);
            }
            LeafShape::Pinnate { leaflets } | LeafShape::Digitate { leaflets } => {
                c.count("leaves.shape.leaflets", leaflets as u32, 1, 31)
            }
            LeafShape::Needles { length_m, bundle } => {
                c.range("leaves.shape.length_m", length_m, 0.003, 0.4);
                c.count("leaves.shape.bundle", bundle as u32, 0, 5);
            }
            _ => {}
        }
        if let Some(f) = &self.flowers {
            c.colour("flowers.colour", f.colour);
            c.colour("flowers.centre", f.centre);
            c.range("flowers.card_m", f.card_m, 0.01, 2.0);
            c.range("flowers.density", f.density, 0.0, 10.0);
        }
        c.colour("bark.colour", self.bark.colour);
        c.colour("bark.accent", self.bark.accent);
        match &self.form {
            Form::Branching(b) => {
                c.count("branching.stems", b.stems as u32, 1, 60);
                c.range("branching.stem_spread_deg", b.stem_spread_deg, 0.0, 80.0);
                c.range(
                    "branching.stem_base_radius_m",
                    b.stem_base_radius_m,
                    0.0,
                    10.0,
                );
                c.range(
                    "branching.trunk_radius_ratio",
                    b.trunk_radius_ratio,
                    0.001,
                    0.2,
                );
                c.range("branching.trunk_flare", b.trunk_flare, 0.0, 2.0);
                c.range("branching.trunk_wobble_deg", b.trunk_wobble_deg, 0.0, 30.0);
                c.range("branching.leader", b.leader, 0.05, 1.0);
                c.range("branching.crown_base", b.crown_base, 0.0, 0.95);
                c.count("branching.trunk_segments", b.trunk_segments as u32, 2, 30);
                c.count("branching.leaf_level", b.leaf_level as u32, 0, 3);
                c.range("branching.leaf_start", b.leaf_start, 0.0, 1.0);
                c.range("branching.leaf_up", b.leaf_up, 0.0, 2.0);
                c.range("branching.stake_m", b.stake_m, 0.0, self.height_m * 1.5);
                let maxc = [150, 60, 40];
                let mut total: u64 = b.stems as u64;
                for (k, lv) in b.levels.iter().enumerate() {
                    let p = format!("branching.levels.{k}");
                    c.count(&format!("{p}.count"), lv.count as u32, 0, maxc[k]);
                    c.range(&format!("{p}.length"), lv.length, 0.0, 3.0);
                    c.range(
                        &format!("{p}.down_angle_deg"),
                        lv.down_angle_deg,
                        -180.0,
                        180.0,
                    );
                    c.range(
                        &format!("{p}.down_angle_var_deg"),
                        lv.down_angle_var_deg,
                        0.0,
                        90.0,
                    );
                    c.range(&format!("{p}.rotate_deg"), lv.rotate_deg, -360.0, 360.0);
                    c.count(&format!("{p}.whorl"), lv.whorl as u32, 1, 8);
                    c.range(&format!("{p}.curve_deg"), lv.curve_deg, -360.0, 360.0);
                    c.range(&format!("{p}.curve_var_deg"), lv.curve_var_deg, 0.0, 90.0);
                    c.range(&format!("{p}.gravity"), lv.gravity, -10.0, 10.0);
                    c.count(&format!("{p}.segments"), lv.segments as u32, 1, 20);
                    c.range(&format!("{p}.radius_ratio"), lv.radius_ratio, 0.05, 1.0);
                    c.range(&format!("{p}.start"), lv.start, 0.0, 1.0);
                    total = total.saturating_mul((lv.count as u64).max(1));
                }
                if total > 60_000 {
                    c.0.push(format!(
                        "branching: stems x level counts = {total} branches, more than 60000"
                    ));
                }
            }
            Form::Conifer(k) => {
                c.count("conifer.tiers", k.tiers as u32, 2, 200);
                c.count("conifer.per_whorl", k.per_whorl as u32, 1, 12);
                c.range("conifer.crown_base", k.crown_base, 0.0, 0.9);
                c.range(
                    "conifer.trunk_radius_ratio",
                    k.trunk_radius_ratio,
                    0.001,
                    0.1,
                );
                c.range("conifer.taper", k.taper, 0.3, 3.0);
                c.range("conifer.droop", k.droop, 0.0, 1.5);
                c.range("conifer.upsweep", k.upsweep, 0.0, 1.5);
                c.range("conifer.pendulous", k.pendulous, 0.0, 1.0);
                c.range("conifer.layered", k.layered, 0.0, 1.0);
            }
            Form::Palm(p) => {
                c.count("palm.fronds", p.fronds as u32, 1, 80);
                c.count("palm.dead_fronds", p.dead_fronds as u32, 0, 60);
                c.range("palm.blade_m", p.blade_m, 0.1, 6.0);
                c.range("palm.petiole_m", p.petiole_m, 0.0, 5.0);
                c.range("palm.trunk_radius_m", p.trunk_radius_m, 0.02, 1.0);
                c.range("palm.lean_deg", p.lean_deg, 0.0, 45.0);
            }
            Form::Clipped(k) => {
                if let ClipShape::HedgeSegment { length_m } = k.shape {
                    c.range("clipped.length_m", length_m, 0.2, 10.0);
                }
                c.range("clipped.roughness", k.roughness, 0.0, 0.5);
                c.range("clipped.tuft_m", k.tuft_m, 0.0, 1.0);
                c.range("clipped.stem_m", k.stem_m, 0.0, self.height_m * 0.9);
            }
            Form::Tussock(t) => {
                c.count("tussock.blades", t.blades as u32, 1, 400);
                c.range("tussock.blade_width_m", t.blade_width_m, 0.0005, 0.5);
                c.range(
                    "tussock.base_radius_m",
                    t.base_radius_m,
                    0.0,
                    self.width_m.max(0.02),
                );
                c.range("tussock.min_angle_deg", t.min_angle_deg, 0.0, 90.0);
                c.range("tussock.max_angle_deg", t.max_angle_deg, 0.0, 90.0);
                c.range("tussock.droop", t.droop, 0.0, 2.0);
                c.range("tussock.foliage_height", t.foliage_height, 0.05, 1.0);
                c.count("tussock.fans", t.fans as u32, 0, 64);
                c.count("tussock.plumes", t.plumes as u32, 0, 200);
                c.range("tussock.plume_m", t.plume_m, 0.0, 3.0);
                c.range("tussock.plume_spread_deg", t.plume_spread_deg, 0.0, 80.0);
            }
            Form::Rosette(r) => {
                c.count("rosette.leaves", r.leaves as u32, 1, 300);
                c.range("rosette.leaf_m", r.leaf_m, 0.01, 5.0);
                c.range("rosette.min_elev_deg", r.min_elev_deg, -30.0, 90.0);
                c.range("rosette.max_elev_deg", r.max_elev_deg, -30.0, 90.0);
                c.range("rosette.arch_deg", r.arch_deg, -90.0, 180.0);
                c.count("rosette.stalks", r.stalks as u32, 0, 300);
                c.range("rosette.stalk_spread_deg", r.stalk_spread_deg, 0.0, 80.0);
                c.range("rosette.foliage_height", r.foliage_height, 0.05, 1.0);
            }
            Form::GroundCover(g) => {
                c.count("ground_cover.cards", g.cards as u32, 1, 2000);
                c.range("ground_cover.depth_m", g.depth_m, 0.0, 1.0);
                c.range("ground_cover.tilt_deg", g.tilt_deg, 0.0, 90.0);
            }
            Form::Climber(k) => {
                c.range("climber.patch_width_m", k.patch_width_m, 0.2, 20.0);
                c.count("climber.stems", k.stems as u32, 0, 40);
                c.range("climber.depth_m", k.depth_m, 0.0, 1.0);
            }
        }
        if c.0.is_empty() { Ok(()) } else { Err(c.0) }
    }
}
