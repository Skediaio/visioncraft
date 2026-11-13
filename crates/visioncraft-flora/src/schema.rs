//! A small, UI-oriented description of the parameters a gardener most often wants to change,
//! addressed by dotted paths, with typed getters and setters. Enumerations (crown shape, leaf
//! shape, bark pattern, flower shape, clip shape) are integer choices indexing `choices`.

use crate::{
    BarkPattern, ClipShape, CrownShape, FlowerParams, FlowerShape, Form, LeafShape, PlantParams,
};

/// One editable parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamInfo {
    pub path: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub help: &'static str,
    /// Names of the choices for enumerations (value = index); empty for numbers.
    pub choices: &'static [&'static str],
}

const fn num(
    path: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    step: f32,
    help: &'static str,
) -> ParamInfo {
    ParamInfo {
        path,
        label,
        min,
        max,
        step,
        help,
        choices: &[],
    }
}

const fn choice(
    path: &'static str,
    label: &'static str,
    choices: &'static [&'static str],
    help: &'static str,
) -> ParamInfo {
    ParamInfo {
        path,
        label,
        min: 0.0,
        max: (choices.len() - 1) as f32,
        step: 1.0,
        help,
        choices,
    }
}

pub const CROWN_SHAPES: &[&str] = &[
    "conical",
    "spherical",
    "hemispherical",
    "cylindrical",
    "tapered cylindrical",
    "flame",
    "inverse conical",
    "tend flame",
];
pub const LEAF_SHAPES: &[&str] = &[
    "ovate",
    "elliptic",
    "lanceolate",
    "linear",
    "cordate",
    "deltoid",
    "lobed",
    "palmate",
    "pinnate",
    "needles",
    "scale",
    "fan frond",
    "feather frond",
    "fern frond",
    "orbicular",
    "digitate",
    "flabellate",
];
pub const BARK_PATTERNS: &[&str] = &[
    "smooth", "birch", "furrowed", "flaky", "fluted", "fibrous", "stem", "scaly",
];
pub const FLOWER_SHAPES: &[&str] = &[
    "daisy",
    "blossom",
    "mophead",
    "rose",
    "lily",
    "goblet",
    "truss",
    "spike",
    "panicle",
    "plume",
    "bottlebrush",
    "saucer",
    "fruit",
    "raceme",
    "trumpet",
];
pub const CLIP_SHAPES: &[&str] = &["ball", "cone", "column", "flame", "hedge segment"];
const LEAF_HABIT: &[&str] = &["evergreen", "deciduous"];

const COMMON: &[ParamInfo] = &[
    num(
        "height_m",
        "Height",
        0.05,
        40.0,
        0.05,
        "Mature height in metres (in flower for perennials).",
    ),
    num(
        "width_m",
        "Width",
        0.05,
        30.0,
        0.05,
        "Mature crown or clump diameter in metres (hedges: thickness).",
    ),
    num(
        "age",
        "Age",
        0.1,
        1.0,
        0.05,
        "Growth stage; 1 is mature. Scales the plant, not its leaves.",
    ),
    choice(
        "deciduous",
        "Leaf habit",
        LEAF_HABIT,
        "Deciduous plants are bare in winter.",
    ),
    choice(
        "leaves.shape",
        "Leaf shape",
        LEAF_SHAPES,
        "Botanical leaf outline.",
    ),
    num(
        "leaves.leaf_m",
        "Leaf size",
        0.005,
        1.5,
        0.005,
        "Length of one leaf in metres.",
    ),
    num(
        "leaves.card_m",
        "Foliage card size",
        0.05,
        2.0,
        0.01,
        "Size of one foliage card (a spray of leaves) in metres.",
    ),
    num(
        "leaves.density",
        "Foliage density",
        0.5,
        5.0,
        0.1,
        "How closed the foliage is.",
    ),
    num(
        "leaves.colour.r",
        "Leaf colour red",
        0.0,
        1.0,
        0.01,
        "Leaf colour, sRGB.",
    ),
    num(
        "leaves.colour.g",
        "Leaf colour green",
        0.0,
        1.0,
        0.01,
        "Leaf colour, sRGB.",
    ),
    num(
        "leaves.colour.b",
        "Leaf colour blue",
        0.0,
        1.0,
        0.01,
        "Leaf colour, sRGB.",
    ),
    num(
        "leaves.autumn_colour.r",
        "Autumn colour red",
        0.0,
        1.0,
        0.01,
        "Autumn leaf colour, sRGB.",
    ),
    num(
        "leaves.autumn_colour.g",
        "Autumn colour green",
        0.0,
        1.0,
        0.01,
        "Autumn leaf colour, sRGB.",
    ),
    num(
        "leaves.autumn_colour.b",
        "Autumn colour blue",
        0.0,
        1.0,
        0.01,
        "Autumn leaf colour, sRGB.",
    ),
    choice(
        "flowers.shape",
        "Flower shape",
        FLOWER_SHAPES,
        "Flower or inflorescence type.",
    ),
    num(
        "flowers.colour.r",
        "Flower colour red",
        0.0,
        1.0,
        0.01,
        "Flower colour, sRGB.",
    ),
    num(
        "flowers.colour.g",
        "Flower colour green",
        0.0,
        1.0,
        0.01,
        "Flower colour, sRGB.",
    ),
    num(
        "flowers.colour.b",
        "Flower colour blue",
        0.0,
        1.0,
        0.01,
        "Flower colour, sRGB.",
    ),
    num(
        "flowers.card_m",
        "Flower size",
        0.02,
        1.0,
        0.01,
        "Size of one flower card in metres.",
    ),
    num(
        "bloom_mask",
        "Bloom months",
        0.0,
        4095.0,
        1.0,
        "Bit k set = flowers in month k + 1 (January = bit 0).",
    ),
    choice(
        "bark.pattern",
        "Bark pattern",
        BARK_PATTERNS,
        "Bark texture pattern.",
    ),
    num(
        "bark.colour.r",
        "Bark colour red",
        0.0,
        1.0,
        0.01,
        "Bark colour, sRGB.",
    ),
    num(
        "bark.colour.g",
        "Bark colour green",
        0.0,
        1.0,
        0.01,
        "Bark colour, sRGB.",
    ),
    num(
        "bark.colour.b",
        "Bark colour blue",
        0.0,
        1.0,
        0.01,
        "Bark colour, sRGB.",
    ),
];

const BRANCHING: &[ParamInfo] = &[
    num(
        "branching.stems",
        "Stems",
        1.0,
        30.0,
        1.0,
        "1 for a single trunk, more for multi-stemmed shrubs.",
    ),
    num(
        "branching.leader",
        "Trunk fraction",
        0.1,
        1.0,
        0.05,
        "How far up the trunk continues: 1 for a central leader.",
    ),
    num(
        "branching.crown_base",
        "Crown base",
        0.0,
        0.8,
        0.05,
        "Height fraction of the lowest limbs.",
    ),
    choice(
        "branching.crown_shape",
        "Crown shape",
        CROWN_SHAPES,
        "Envelope of the main limbs.",
    ),
    num(
        "branching.limbs",
        "Main branches",
        1.0,
        120.0,
        1.0,
        "Limbs on the trunk.",
    ),
    num(
        "branching.branches",
        "Branch density",
        0.0,
        40.0,
        1.0,
        "Side branches per limb.",
    ),
    num(
        "branching.spread_deg",
        "Limb angle",
        10.0,
        90.0,
        1.0,
        "Angle of the limbs from the trunk.",
    ),
    num(
        "branching.droop",
        "Droop",
        -5.0,
        2.0,
        0.1,
        "Negative values make branches hang (weeping).",
    ),
    num(
        "branching.trunk_radius_ratio",
        "Trunk thickness",
        0.005,
        0.08,
        0.001,
        "Trunk radius divided by height.",
    ),
];
const CONIFER: &[ParamInfo] = &[
    num(
        "conifer.tiers",
        "Whorls",
        4.0,
        120.0,
        1.0,
        "Branch tiers from the crown base to the top.",
    ),
    num(
        "conifer.per_whorl",
        "Branches per whorl",
        1.0,
        12.0,
        1.0,
        "Branches in each tier.",
    ),
    num(
        "conifer.crown_base",
        "Crown base",
        0.0,
        0.8,
        0.05,
        "Height fraction of the lowest branches.",
    ),
    num(
        "conifer.taper",
        "Taper",
        0.3,
        3.0,
        0.05,
        "1 straight cone, below 1 blunter top, above 1 slender spire.",
    ),
    num(
        "conifer.droop",
        "Droop",
        0.0,
        1.5,
        0.05,
        "Sag of the main branches.",
    ),
    num(
        "conifer.upsweep",
        "Upswept tips",
        0.0,
        1.5,
        0.05,
        "Upward sweep of the branch tips.",
    ),
    num(
        "conifer.pendulous",
        "Hanging branchlets",
        0.0,
        1.0,
        0.05,
        "Curtains of hanging shoots (Norway spruce).",
    ),
    num(
        "conifer.layered",
        "Layered tiers",
        0.0,
        1.0,
        0.05,
        "Flat horizontal tiers (fir) versus rounded sprays.",
    ),
];
const PALM: &[ParamInfo] = &[
    num(
        "palm.fronds",
        "Fronds",
        1.0,
        80.0,
        1.0,
        "Living fronds in the crown.",
    ),
    num(
        "palm.dead_fronds",
        "Dead fronds",
        0.0,
        60.0,
        1.0,
        "Hanging skirt of dead fronds.",
    ),
    num(
        "palm.blade_m",
        "Frond size",
        0.1,
        6.0,
        0.05,
        "Frond blade length in metres.",
    ),
    num(
        "palm.petiole_m",
        "Leaf stalk",
        0.0,
        5.0,
        0.05,
        "Petiole length in metres.",
    ),
];
const CLIPPED: &[ParamInfo] = &[
    choice(
        "clipped.shape",
        "Clipped shape",
        CLIP_SHAPES,
        "Topiary form or hedge segment.",
    ),
    num(
        "clipped.length_m",
        "Segment length",
        0.2,
        10.0,
        0.1,
        "Hedge segment length along X in metres.",
    ),
    num(
        "clipped.roughness",
        "Roughness",
        0.0,
        0.3,
        0.005,
        "0.01 freshly clipped, 0.08 shaggy.",
    ),
    num(
        "clipped.stem_m",
        "Clear stem",
        0.0,
        4.0,
        0.05,
        "Bare stem under the foliage (standards, pleached hedges).",
    ),
];
const TUSSOCK: &[ParamInfo] = &[
    num(
        "tussock.blades",
        "Blades",
        1.0,
        400.0,
        1.0,
        "Individual blades at close range.",
    ),
    num(
        "tussock.fans",
        "Clump fullness",
        0.0,
        64.0,
        1.0,
        "Fan cards giving the clump its mass.",
    ),
    num(
        "tussock.droop",
        "Arching",
        0.0,
        2.0,
        0.05,
        "0 stiff, 1 strongly arching blades.",
    ),
    num(
        "tussock.foliage_height",
        "Foliage height",
        0.05,
        1.0,
        0.05,
        "Leaf mound height relative to the plant height.",
    ),
    num(
        "tussock.plumes",
        "Plumes",
        0.0,
        200.0,
        1.0,
        "Flowering plumes.",
    ),
    num(
        "tussock.plume_m",
        "Plume length",
        0.0,
        3.0,
        0.01,
        "Plume length in metres.",
    ),
];
const ROSETTE: &[ParamInfo] = &[
    num(
        "rosette.leaves",
        "Leaves",
        1.0,
        300.0,
        1.0,
        "Leaves or leafy shoots.",
    ),
    num(
        "rosette.leaf_m",
        "Leaf length",
        0.01,
        5.0,
        0.01,
        "Leaf length including stalk, metres.",
    ),
    num(
        "rosette.arch_deg",
        "Arching",
        -90.0,
        180.0,
        1.0,
        "Bend of the leaves towards the ground.",
    ),
    num(
        "rosette.max_elev_deg",
        "Leaf angle",
        -30.0,
        90.0,
        1.0,
        "Elevation of the inner leaves.",
    ),
    num(
        "rosette.stalks",
        "Flower stalks",
        0.0,
        300.0,
        1.0,
        "Flower stalks.",
    ),
    num(
        "rosette.foliage_height",
        "Foliage height",
        0.05,
        1.0,
        0.05,
        "Leaf mound height relative to the plant height.",
    ),
];
const GROUND: &[ParamInfo] = &[
    num(
        "ground_cover.cards",
        "Leaf clusters",
        1.0,
        2000.0,
        1.0,
        "Leaf cards in the patch.",
    ),
    num(
        "ground_cover.depth_m",
        "Depth",
        0.0,
        1.0,
        0.01,
        "Height of the leaf layer in metres.",
    ),
    num(
        "ground_cover.tilt_deg",
        "Leaf tilt",
        0.0,
        90.0,
        1.0,
        "0 lies flat (floating pads).",
    ),
];
const CLIMBER: &[ParamInfo] = &[
    num(
        "climber.patch_width_m",
        "Patch width",
        0.2,
        20.0,
        0.1,
        "Width of the wall patch in metres.",
    ),
    num(
        "climber.stems",
        "Stems",
        0.0,
        40.0,
        1.0,
        "Main climbing stems.",
    ),
    num(
        "climber.depth_m",
        "Depth",
        0.0,
        1.0,
        0.01,
        "How far the leaves stand off the wall.",
    ),
];

/// The editable parameters for a plant of this form: the common ones plus the form's own.
pub fn param_schema(form: &Form) -> Vec<ParamInfo> {
    let own = match form {
        Form::Branching(_) => BRANCHING,
        Form::Conifer(_) => CONIFER,
        Form::Palm(_) => PALM,
        Form::Clipped(_) => CLIPPED,
        Form::Tussock(_) => TUSSOCK,
        Form::Rosette(_) => ROSETTE,
        Form::GroundCover(_) => GROUND,
        Form::Climber(_) => CLIMBER,
    };
    COMMON.iter().chain(own).copied().collect()
}

enum Slot<'a> {
    F(&'a mut f32),
    U16(&'a mut u16),
    U8(&'a mut u8),
}

fn rgb<'a>(c: &'a mut [f32; 3], k: &str) -> Option<Slot<'a>> {
    match k {
        "r" => Some(Slot::F(&mut c[0])),
        "g" => Some(Slot::F(&mut c[1])),
        "b" => Some(Slot::F(&mut c[2])),
        _ => None,
    }
}

/// Numeric fields addressed by path (choices are handled separately).
fn slot<'a>(p: &'a mut PlantParams, path: &str) -> Option<Slot<'a>> {
    let (head, tail) = path.rsplit_once('.').unwrap_or(("", path));
    match (head, tail) {
        ("", "height_m") => return Some(Slot::F(&mut p.height_m)),
        ("", "width_m") => return Some(Slot::F(&mut p.width_m)),
        ("", "age") => return Some(Slot::F(&mut p.age)),
        ("leaves", "leaf_m") => return Some(Slot::F(&mut p.leaves.leaf_m)),
        ("leaves", "card_m") => return Some(Slot::F(&mut p.leaves.card_m)),
        ("leaves", "density") => return Some(Slot::F(&mut p.leaves.density)),
        ("leaves.colour", k) => return rgb(&mut p.leaves.colour, k),
        ("leaves.autumn_colour", k) => return rgb(&mut p.leaves.autumn_colour, k),
        ("flowers.colour", k) => return p.flowers.as_mut().and_then(|f| rgb(&mut f.colour, k)),
        ("flowers", "card_m") => return p.flowers.as_mut().map(|f| Slot::F(&mut f.card_m)),
        ("bark.colour", k) => return rgb(&mut p.bark.colour, k),
        _ => {}
    }
    match (&mut p.form, head, tail) {
        (Form::Branching(b), "branching", f) => match f {
            "stems" => Some(Slot::U16(&mut b.stems)),
            "leader" => Some(Slot::F(&mut b.leader)),
            "crown_base" => Some(Slot::F(&mut b.crown_base)),
            "limbs" => Some(Slot::U16(&mut b.levels[0].count)),
            "branches" => Some(Slot::U16(&mut b.levels[1].count)),
            "spread_deg" => Some(Slot::F(&mut b.levels[0].down_angle_deg)),
            "droop" => Some(Slot::F(&mut b.levels[1].gravity)),
            "trunk_radius_ratio" => Some(Slot::F(&mut b.trunk_radius_ratio)),
            _ => None,
        },
        (Form::Conifer(c), "conifer", f) => match f {
            "tiers" => Some(Slot::U16(&mut c.tiers)),
            "per_whorl" => Some(Slot::U8(&mut c.per_whorl)),
            "crown_base" => Some(Slot::F(&mut c.crown_base)),
            "taper" => Some(Slot::F(&mut c.taper)),
            "droop" => Some(Slot::F(&mut c.droop)),
            "upsweep" => Some(Slot::F(&mut c.upsweep)),
            "pendulous" => Some(Slot::F(&mut c.pendulous)),
            "layered" => Some(Slot::F(&mut c.layered)),
            _ => None,
        },
        (Form::Palm(c), "palm", f) => match f {
            "fronds" => Some(Slot::U16(&mut c.fronds)),
            "dead_fronds" => Some(Slot::U16(&mut c.dead_fronds)),
            "blade_m" => Some(Slot::F(&mut c.blade_m)),
            "petiole_m" => Some(Slot::F(&mut c.petiole_m)),
            _ => None,
        },
        (Form::Clipped(c), "clipped", f) => match f {
            "length_m" => match &mut c.shape {
                ClipShape::HedgeSegment { length_m } => Some(Slot::F(length_m)),
                _ => None,
            },
            "roughness" => Some(Slot::F(&mut c.roughness)),
            "stem_m" => Some(Slot::F(&mut c.stem_m)),
            _ => None,
        },
        (Form::Tussock(c), "tussock", f) => match f {
            "blades" => Some(Slot::U16(&mut c.blades)),
            "fans" => Some(Slot::U16(&mut c.fans)),
            "droop" => Some(Slot::F(&mut c.droop)),
            "foliage_height" => Some(Slot::F(&mut c.foliage_height)),
            "plumes" => Some(Slot::U16(&mut c.plumes)),
            "plume_m" => Some(Slot::F(&mut c.plume_m)),
            _ => None,
        },
        (Form::Rosette(c), "rosette", f) => match f {
            "leaves" => Some(Slot::U16(&mut c.leaves)),
            "leaf_m" => Some(Slot::F(&mut c.leaf_m)),
            "arch_deg" => Some(Slot::F(&mut c.arch_deg)),
            "max_elev_deg" => Some(Slot::F(&mut c.max_elev_deg)),
            "stalks" => Some(Slot::U16(&mut c.stalks)),
            "foliage_height" => Some(Slot::F(&mut c.foliage_height)),
            _ => None,
        },
        (Form::GroundCover(c), "ground_cover", f) => match f {
            "cards" => Some(Slot::U16(&mut c.cards)),
            "depth_m" => Some(Slot::F(&mut c.depth_m)),
            "tilt_deg" => Some(Slot::F(&mut c.tilt_deg)),
            _ => None,
        },
        (Form::Climber(c), "climber", f) => match f {
            "patch_width_m" => Some(Slot::F(&mut c.patch_width_m)),
            "stems" => Some(Slot::U16(&mut c.stems)),
            "depth_m" => Some(Slot::F(&mut c.depth_m)),
            _ => None,
        },
        _ => None,
    }
}

fn leaf_shape_index(s: LeafShape) -> usize {
    match s {
        LeafShape::Ovate => 0,
        LeafShape::Elliptic => 1,
        LeafShape::Lanceolate => 2,
        LeafShape::Linear => 3,
        LeafShape::Cordate => 4,
        LeafShape::Deltoid => 5,
        LeafShape::Lobed { .. } => 6,
        LeafShape::Palmate { .. } => 7,
        LeafShape::Pinnate { .. } => 8,
        LeafShape::Needles { .. } => 9,
        LeafShape::Scale => 10,
        LeafShape::FanFrond => 11,
        LeafShape::FeatherFrond => 12,
        LeafShape::FernFrond => 13,
        LeafShape::Orbicular => 14,
        LeafShape::Digitate { .. } => 15,
        LeafShape::Flabellate => 16,
    }
}

fn leaf_shape_of(i: usize) -> Option<LeafShape> {
    Some(match i {
        0 => LeafShape::Ovate,
        1 => LeafShape::Elliptic,
        2 => LeafShape::Lanceolate,
        3 => LeafShape::Linear,
        4 => LeafShape::Cordate,
        5 => LeafShape::Deltoid,
        6 => LeafShape::Lobed { lobes: 4 },
        7 => LeafShape::Palmate {
            lobes: 5,
            depth: 0.5,
        },
        8 => LeafShape::Pinnate { leaflets: 7 },
        9 => LeafShape::Needles {
            length_m: 0.025,
            bundle: 1,
        },
        10 => LeafShape::Scale,
        11 => LeafShape::FanFrond,
        12 => LeafShape::FeatherFrond,
        13 => LeafShape::FernFrond,
        14 => LeafShape::Orbicular,
        15 => LeafShape::Digitate { leaflets: 7 },
        16 => LeafShape::Flabellate,
        _ => return None,
    })
}

const FLOWERS: [FlowerShape; 15] = [
    FlowerShape::Daisy { droop: false },
    FlowerShape::Blossom,
    FlowerShape::Mophead,
    FlowerShape::Rose,
    FlowerShape::Lily,
    FlowerShape::Goblet,
    FlowerShape::Truss,
    FlowerShape::Spike,
    FlowerShape::Panicle,
    FlowerShape::Plume,
    FlowerShape::Bottlebrush,
    FlowerShape::Saucer,
    FlowerShape::Fruit,
    FlowerShape::Raceme,
    FlowerShape::Trumpet,
];
const CROWNS: [CrownShape; 8] = [
    CrownShape::Conical,
    CrownShape::Spherical,
    CrownShape::Hemispherical,
    CrownShape::Cylindrical,
    CrownShape::TaperedCylindrical,
    CrownShape::Flame,
    CrownShape::InverseConical,
    CrownShape::TendFlame,
];
const BARKS: [BarkPattern; 8] = [
    BarkPattern::Smooth,
    BarkPattern::Birch,
    BarkPattern::Furrowed,
    BarkPattern::Flaky,
    BarkPattern::Fluted,
    BarkPattern::Fibrous,
    BarkPattern::Stem,
    BarkPattern::Scaly,
];

fn index_of<T: PartialEq>(list: &[T], v: &T) -> Option<usize> {
    list.iter().position(|x| x == v)
}

/// Read a parameter by schema path; `None` for unknown paths or ones that do not apply.
pub fn get_param(p: &PlantParams, path: &str) -> Option<f32> {
    match path {
        "deciduous" => return Some(p.deciduous as u8 as f32),
        "leaves.shape" => return Some(leaf_shape_index(p.leaves.shape) as f32),
        "bark.pattern" => return index_of(&BARKS, &p.bark.pattern).map(|i| i as f32),
        "flowers.shape" => {
            return p.flowers.as_ref().and_then(|f| {
                let s = if let FlowerShape::Daisy { .. } = f.shape {
                    FlowerShape::Daisy { droop: false }
                } else {
                    f.shape
                };
                index_of(&FLOWERS, &s).map(|i| i as f32)
            });
        }
        "bloom_mask" => {
            return Some(
                p.bloom_months
                    .iter()
                    .filter(|m| (1..=12).contains(*m))
                    .fold(0u32, |a, m| a | 1 << (m - 1)) as f32,
            );
        }
        "branching.crown_shape" => {
            return match &p.form {
                Form::Branching(b) => index_of(&CROWNS, &b.crown_shape).map(|i| i as f32),
                _ => None,
            };
        }
        "clipped.shape" => {
            return match &p.form {
                Form::Clipped(c) => Some(match c.shape {
                    ClipShape::Ball => 0.0,
                    ClipShape::Cone => 1.0,
                    ClipShape::Column => 2.0,
                    ClipShape::Flame => 3.0,
                    ClipShape::HedgeSegment { .. } => 4.0,
                }),
                _ => None,
            };
        }
        _ => {}
    }
    let mut copy = p.clone();
    slot(&mut copy, path).map(|s| match s {
        Slot::F(v) => *v,
        Slot::U16(v) => *v as f32,
        Slot::U8(v) => *v as f32,
    })
}

/// Write a parameter by schema path. Returns false for unknown paths, paths that do not apply
/// to this form, non-finite values and out-of-range choices. Numbers are not clamped; use
/// [`PlantParams::validate`] before generating.
pub fn set_param(p: &mut PlantParams, path: &str, value: f32) -> bool {
    if !value.is_finite() {
        return false;
    }
    let idx = if value >= 0.0 {
        value.round() as usize
    } else {
        usize::MAX
    };
    match path {
        "deciduous" => {
            p.deciduous = value >= 0.5;
            return true;
        }
        "leaves.shape" => {
            return match leaf_shape_of(idx) {
                Some(s) => {
                    if leaf_shape_index(p.leaves.shape) != idx {
                        p.leaves.shape = s;
                    }
                    true
                }
                None => false,
            };
        }
        "bark.pattern" => {
            return BARKS.get(idx).map(|b| p.bark.pattern = *b).is_some();
        }
        "flowers.shape" => {
            let Some(s) = FLOWERS.get(idx) else {
                return false;
            };
            p.flowers.get_or_insert_with(FlowerParams::default).shape = *s;
            return true;
        }
        "bloom_mask" => {
            let m = value.round().clamp(0.0, 4095.0) as u32;
            p.bloom_months = (1..=12u8).filter(|k| m & (1 << (k - 1)) != 0).collect();
            return true;
        }
        "branching.crown_shape" => {
            return match (&mut p.form, CROWNS.get(idx)) {
                (Form::Branching(b), Some(c)) => {
                    b.crown_shape = *c;
                    true
                }
                _ => false,
            };
        }
        "clipped.shape" => {
            let Form::Clipped(c) = &mut p.form else {
                return false;
            };
            let length_m = if let ClipShape::HedgeSegment { length_m } = c.shape {
                length_m
            } else {
                1.0
            };
            c.shape = match idx {
                0 => ClipShape::Ball,
                1 => ClipShape::Cone,
                2 => ClipShape::Column,
                3 => ClipShape::Flame,
                4 => ClipShape::HedgeSegment { length_m },
                _ => return false,
            };
            return true;
        }
        _ => {}
    }
    if matches!(
        path,
        "flowers.colour.r" | "flowers.colour.g" | "flowers.colour.b" | "flowers.card_m"
    ) && p.flowers.is_none()
    {
        p.flowers = Some(FlowerParams::default());
    }
    match slot(p, path) {
        Some(Slot::F(v)) => *v = value,
        Some(Slot::U16(v)) => *v = value.round().clamp(0.0, u16::MAX as f32) as u16,
        Some(Slot::U8(v)) => *v = value.round().clamp(0.0, u8::MAX as f32) as u8,
        None => return false,
    }
    true
}
