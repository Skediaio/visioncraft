//! Plain, documented parameter structs. Users and third parties create their own species by
//! editing these; nothing here needs serde.

use crate::Lod;

/// Everything that defines a species' look.
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetClass {
    LargeTree,
    /// Small trees, conifers and palms.
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
}

/// Crown envelopes of Weber & Penn (1995): the length of a primary branch as a function of its
/// height in the crown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrownShape {
    Conical,
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
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClipShape {
    /// Sphere of diameter `width_m`, resting on the ground.
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
pub struct GroundCoverParams {
    /// Leaf cards at LOD0.
    pub cards: u16,
    /// Height of the leaf layer, metres.
    pub depth_m: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClimberParams {
    /// Patch width along X, metres (height is `height_m`).
    pub patch_width_m: f32,
    /// Main stems climbing the patch.
    pub stems: u16,
    /// How far the leaves stand off the wall, metres.
    pub depth_m: f32,
}

/// Leaf silhouettes (standard botanical terms).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LeafShape {
    /// Egg-shaped, widest below the middle (beech, apple, cherry).
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
}

#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlowerShape {
    /// Ray florets around a disc (coneflower; `droop` the rays hang).
    Daisy { droop: bool },
    /// Clusters of small five-petalled flowers along a twig (cherry, apple, rowan corymbs).
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
}

#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarkPattern {
    /// Smooth grey (beech, young maple, magnolia).
    Smooth,
    /// White with dark horizontal lenticels and black fissured patches (birch).
    Birch,
    /// Deep vertical furrows and ridges (oak, walnut, lime, poplar, willow).
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
pub struct BarkParams {
    pub pattern: BarkPattern,
    /// Main colour, sRGB 0..1.
    pub colour: [f32; 3],
    /// Furrow / lenticel / accent colour.
    pub accent: [f32; 3],
}
