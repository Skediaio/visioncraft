//! The built-in species library: plants common in Belgian and Dutch gardens. Every parameter
//! value here is chosen for this crate from the plants' botany (mature height and spread,
//! crown shape, branching habit, leaf morphology, colours); none is taken from another
//! generator's presets.

use std::sync::OnceLock;

use crate::{
    BarkParams, BarkPattern, BranchingParams, BudgetClass, ClimberParams, ClipShape, ClippedParams,
    CrownShape, FlowerParams, FlowerShape, Form, GroundCoverParams, LeafParams, LeafShape,
    LevelParams, PalmParams, PlantParams, RosetteParams, TussockParams,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Tree,
    Conifer,
    Palm,
    Shrub,
    Hedge,
    Perennial,
    Fern,
    Grass,
    GroundCover,
    Climber,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Exposure {
    Sun,
    HalfShade,
    Shade,
}

/// A catalogue entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Species {
    pub id: &'static str,
    pub common_name: &'static str,
    pub botanical_name: &'static str,
    pub category: Category,
    pub mature_height_m: f32,
    pub mature_width_m: f32,
    pub evergreen: bool,
    pub bloom_months: &'static [u8],
    pub exposure: Exposure,
    pub params: PlantParams,
}

/// The built-in library, in a fixed order.
pub fn species() -> &'static [Species] {
    static LIB: OnceLock<Vec<Species>> = OnceLock::new();
    LIB.get_or_init(build)
}

// ---- small constructors -------------------------------------------------------------------

fn lv(
    count: u16,
    length: f32,
    down: f32,
    rotate: f32,
    whorl: u8,
    curve: f32,
    curve_var: f32,
    gravity: f32,
    segments: u8,
    radius_ratio: f32,
    start: f32,
) -> LevelParams {
    LevelParams {
        count,
        length,
        down_angle_deg: down,
        down_angle_var_deg: down * 0.22,
        rotate_deg: rotate,
        whorl,
        curve_deg: curve,
        curve_var_deg: curve_var,
        gravity,
        segments,
        radius_ratio,
        start,
    }
}

const NO: LevelParams = LevelParams::NONE;

fn leaf(
    shape: LeafShape,
    leaf_m: f32,
    aspect: f32,
    serration: f32,
    colour: [f32; 3],
    autumn: [f32; 3],
    card_m: f32,
    density: f32,
) -> LeafParams {
    LeafParams {
        shape,
        leaf_m,
        aspect,
        serration,
        colour,
        colour_var: 0.3,
        autumn_colour: autumn,
        card_m,
        density,
    }
}

fn bark(pattern: BarkPattern, colour: [f32; 3], accent: [f32; 3]) -> BarkParams {
    BarkParams {
        pattern,
        colour,
        accent,
    }
}

fn flower(
    shape: FlowerShape,
    colour: [f32; 3],
    centre: [f32; 3],
    card_m: f32,
    density: f32,
) -> Option<FlowerParams> {
    Some(FlowerParams {
        shape,
        colour,
        centre,
        card_m,
        density,
    })
}

struct Habit {
    stems: u16,
    spread: f32,
    base_r: f32,
    ratio: f32,
    leader: f32,
    crown_base: f32,
    shape: CrownShape,
    levels: [LevelParams; 3],
    leaf_level: u8,
    leaf_start: f32,
    leaf_up: f32,
    flare: f32,
    wobble: f32,
    segs: u8,
}

impl Habit {
    fn single(
        ratio: f32,
        leader: f32,
        crown_base: f32,
        shape: CrownShape,
        levels: [LevelParams; 3],
    ) -> Habit {
        Habit {
            stems: 1,
            spread: 0.0,
            base_r: 0.0,
            ratio,
            leader,
            crown_base,
            shape,
            levels,
            leaf_level: 3,
            leaf_start: 0.2,
            leaf_up: 0.3,
            flare: 0.35,
            wobble: 2.5,
            segs: 12,
        }
    }
    fn multi(
        stems: u16,
        spread: f32,
        base_r: f32,
        ratio: f32,
        leader: f32,
        crown_base: f32,
        shape: CrownShape,
        levels: [LevelParams; 3],
    ) -> Habit {
        Habit {
            stems,
            spread,
            base_r,
            flare: 0.1,
            segs: 6,
            ..Habit::single(ratio, leader, crown_base, shape, levels)
        }
    }
    fn leaves_on(mut self, level: u8, start: f32) -> Habit {
        self.leaf_level = level;
        self.leaf_start = start;
        self
    }
    fn form(self) -> Form {
        Form::Branching(BranchingParams {
            stems: self.stems,
            stem_spread_deg: self.spread,
            stem_base_radius_m: self.base_r,
            trunk_radius_ratio: self.ratio,
            trunk_flare: self.flare,
            trunk_wobble_deg: self.wobble,
            leader: self.leader,
            crown_base: self.crown_base,
            crown_shape: self.shape,
            trunk_segments: self.segs,
            levels: self.levels,
            leaf_level: self.leaf_level,
            leaf_start: self.leaf_start,
            leaf_up: self.leaf_up,
        })
    }
}

fn sp(
    id: &'static str,
    common_name: &'static str,
    botanical_name: &'static str,
    category: Category,
    h: f32,
    w: f32,
    evergreen: bool,
    bloom_months: &'static [u8],
    exposure: Exposure,
    form: Form,
    leaves: LeafParams,
    flowers: Option<FlowerParams>,
    bark: BarkParams,
) -> Species {
    let budget = match category {
        Category::Tree if h >= 12.0 => BudgetClass::LargeTree,
        Category::Tree | Category::Conifer | Category::Palm => BudgetClass::SmallTree,
        Category::Shrub | Category::Hedge | Category::Climber => BudgetClass::Shrub,
        Category::GroundCover => BudgetClass::Tuft,
        Category::Grass if h < 0.15 => BudgetClass::Tuft,
        Category::Perennial | Category::Fern | Category::Grass => BudgetClass::Herb,
    };
    Species {
        id,
        common_name,
        botanical_name,
        category,
        mature_height_m: h,
        mature_width_m: w,
        evergreen,
        bloom_months,
        exposure,
        params: PlantParams {
            height_m: h,
            width_m: w,
            age: 1.0,
            deciduous: !evergreen && !matches!(category, Category::Grass),
            marcescent: false,
            bloom_months: if flowers.is_some() {
                bloom_months.to_vec()
            } else {
                Vec::new()
            },
            budget,
            form,
            leaves,
            flowers,
            bark,
        },
    }
}

fn marcescent(mut s: Species) -> Species {
    s.params.marcescent = true;
    s
}

// ---- colours ------------------------------------------------------------------------------

const GREEN_BARK: [f32; 3] = [0.32, 0.42, 0.18];
const DARK_STEM: [f32; 3] = [0.22, 0.3, 0.12];
const YELLOW: [f32; 3] = [0.85, 0.72, 0.2];

#[allow(clippy::vec_init_then_push)] // one push per species reads best as a catalogue
fn build() -> Vec<Species> {
    use Category::*;
    use CrownShape::*;
    use Exposure::*;
    use LeafShape::*;
    let mut v = Vec::new();

    // ---- trees ----
    v.push(sp(
        "quercus-robur",
        "Pedunculate oak",
        "Quercus robur",
        Tree,
        25.0,
        22.0,
        false,
        &[],
        Sun,
        Habit::single(
            0.026,
            0.45,
            0.18,
            Spherical,
            [
                lv(9, 0.55, 55.0, 137.5, 1, 18.0, 10.0, 0.0, 6, 0.65, 0.35),
                lv(9, 0.45, 45.0, 137.5, 1, -8.0, 10.0, -0.1, 4, 0.5, 0.2),
                lv(6, 0.45, 40.0, 137.5, 1, 0.0, 12.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Lobed { lobes: 4 },
            0.1,
            0.55,
            0.0,
            [0.26, 0.37, 0.13],
            [0.55, 0.4, 0.15],
            0.9,
            2.4,
        ),
        None,
        bark(
            BarkPattern::Furrowed,
            [0.42, 0.38, 0.33],
            [0.15, 0.13, 0.11],
        ),
    ));
    v.push(sp(
        "fagus-sylvatica",
        "European beech",
        "Fagus sylvatica",
        Tree,
        25.0,
        18.0,
        false,
        &[],
        HalfShade,
        Habit::single(
            0.022,
            0.7,
            0.15,
            Spherical,
            [
                lv(12, 0.5, 50.0, 137.5, 1, 25.0, 5.0, 0.0, 6, 0.6, 0.25),
                lv(10, 0.45, 50.0, 180.0, 1, -15.0, 6.0, -0.15, 4, 0.5, 0.2),
                lv(6, 0.45, 45.0, 180.0, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Ovate,
            0.08,
            0.6,
            0.1,
            [0.3, 0.42, 0.12],
            [0.7, 0.4, 0.12],
            0.85,
            2.6,
        ),
        None,
        bark(BarkPattern::Smooth, [0.56, 0.56, 0.53], [0.42, 0.48, 0.36]),
    ));
    v.push(sp(
        "betula-pendula",
        "Silver birch",
        "Betula pendula",
        Tree,
        18.0,
        8.0,
        false,
        &[],
        Sun,
        Habit::single(
            0.014,
            1.0,
            0.25,
            Spherical,
            [
                lv(16, 0.35, 45.0, 137.5, 1, 20.0, 5.0, 0.0, 5, 0.5, 0.25),
                lv(8, 0.6, 35.0, 137.5, 1, -20.0, 6.0, -1.6, 4, 0.4, 0.2),
                lv(5, 0.5, 25.0, 137.5, 1, 0.0, 6.0, -0.8, 2, 0.4, 0.25),
            ],
        )
        .form(),
        leaf(
            Deltoid,
            0.05,
            0.7,
            0.6,
            [0.36, 0.5, 0.17],
            [0.8, 0.7, 0.2],
            0.6,
            1.8,
        ),
        None,
        bark(BarkPattern::Birch, [0.9, 0.89, 0.85], [0.12, 0.11, 0.1]),
    ));
    v.push(sp(
        "tilia-cordata",
        "Small-leaved lime",
        "Tilia cordata",
        Tree,
        22.0,
        14.0,
        false,
        &[6, 7],
        Sun,
        Habit::single(
            0.022,
            0.85,
            0.15,
            Spherical,
            [
                lv(14, 0.4, 60.0, 137.5, 1, 25.0, 6.0, 0.0, 5, 0.55, 0.2),
                lv(9, 0.5, 50.0, 180.0, 1, -10.0, 6.0, -0.3, 4, 0.5, 0.2),
                lv(6, 0.45, 45.0, 180.0, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Cordate,
            0.06,
            0.9,
            0.3,
            [0.3, 0.45, 0.15],
            [0.75, 0.65, 0.2],
            0.8,
            2.6,
        ),
        None,
        bark(BarkPattern::Furrowed, [0.42, 0.4, 0.36], [0.22, 0.2, 0.18]),
    ));
    v.push(sp(
        "acer-campestre",
        "Field maple",
        "Acer campestre",
        Tree,
        10.0,
        8.0,
        false,
        &[],
        Sun,
        Habit::single(
            0.025,
            0.6,
            0.2,
            Spherical,
            [
                lv(10, 0.5, 55.0, 137.5, 1, 20.0, 8.0, 0.0, 5, 0.6, 0.3),
                lv(8, 0.5, 45.0, 90.0, 2, -5.0, 6.0, 0.0, 3, 0.5, 0.2),
                lv(6, 0.45, 40.0, 90.0, 2, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Palmate {
                lobes: 5,
                depth: 0.5,
            },
            0.07,
            1.0,
            0.0,
            [0.3, 0.45, 0.15],
            YELLOW,
            0.55,
            2.4,
        ),
        None,
        bark(BarkPattern::Furrowed, [0.47, 0.4, 0.32], [0.25, 0.2, 0.16]),
    ));
    v.push(sp(
        "acer-palmatum",
        "Japanese maple",
        "Acer palmatum 'Atropurpureum'",
        Tree,
        5.0,
        5.0,
        false,
        &[],
        HalfShade,
        Habit {
            leaf_up: 0.6,
            ..Habit::multi(
                3,
                25.0,
                0.15,
                0.02,
                0.55,
                0.25,
                Hemispherical,
                [
                    lv(8, 0.55, 65.0, 90.0, 2, 15.0, 6.0, 0.0, 5, 0.6, 0.3),
                    lv(8, 0.5, 50.0, 90.0, 2, 0.0, 6.0, 0.0, 3, 0.5, 0.2),
                    lv(6, 0.45, 45.0, 90.0, 2, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
                ],
            )
        }
        .form(),
        leaf(
            Palmate {
                lobes: 7,
                depth: 0.8,
            },
            0.06,
            1.0,
            0.3,
            [0.45, 0.1, 0.12],
            [0.75, 0.12, 0.08],
            0.45,
            2.2,
        ),
        None,
        bark(BarkPattern::Smooth, [0.5, 0.45, 0.42], [0.35, 0.3, 0.3]),
    ));
    v.push(sp(
        "salix-sepulcralis",
        "Weeping willow",
        "Salix x sepulcralis 'Chrysocoma'",
        Tree,
        15.0,
        15.0,
        false,
        &[],
        Sun,
        Habit::single(
            0.035,
            0.4,
            0.2,
            Spherical,
            [
                lv(10, 0.6, 55.0, 137.5, 1, 30.0, 6.0, 0.0, 6, 0.65, 0.35),
                lv(40, 1.0, 40.0, 137.5, 1, -20.0, 4.0, -4.0, 6, 0.25, 0.35),
                NO,
            ],
        )
        .leaves_on(2, 0.15)
        .form(),
        leaf(
            Lanceolate,
            0.1,
            0.15,
            0.3,
            [0.45, 0.55, 0.22],
            [0.75, 0.7, 0.25],
            0.8,
            2.2,
        ),
        None,
        bark(BarkPattern::Furrowed, [0.45, 0.4, 0.33], [0.2, 0.17, 0.13]),
    ));
    v.push(sp(
        "populus-nigra-italica",
        "Lombardy poplar",
        "Populus nigra 'Italica'",
        Tree,
        25.0,
        4.0,
        false,
        &[],
        Sun,
        Habit::single(
            0.025,
            1.0,
            0.05,
            TaperedCylindrical,
            [
                lv(50, 0.22, 25.0, 137.5, 1, 5.0, 4.0, 0.6, 4, 0.4, 0.03),
                lv(6, 0.5, 25.0, 137.5, 1, 0.0, 5.0, 0.4, 2, 0.5, 0.2),
                NO,
            ],
        )
        .leaves_on(1, 0.3)
        .form(),
        leaf(Deltoid, 0.07, 0.9, 0.3, [0.3, 0.45, 0.15], YELLOW, 0.8, 2.4),
        None,
        bark(BarkPattern::Furrowed, [0.42, 0.4, 0.36], [0.18, 0.16, 0.14]),
    ));
    v.push(sp(
        "carpinus-betulus",
        "Hornbeam",
        "Carpinus betulus",
        Tree,
        16.0,
        12.0,
        false,
        &[],
        HalfShade,
        Habit::single(
            0.022,
            0.7,
            0.12,
            Spherical,
            [
                lv(14, 0.45, 50.0, 137.5, 1, 30.0, 6.0, 0.0, 5, 0.55, 0.2),
                lv(9, 0.5, 45.0, 180.0, 1, -5.0, 6.0, -0.1, 3, 0.5, 0.2),
                lv(6, 0.45, 40.0, 180.0, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Ovate,
            0.08,
            0.5,
            0.6,
            [0.28, 0.42, 0.14],
            [0.8, 0.65, 0.2],
            0.8,
            2.6,
        ),
        None,
        bark(BarkPattern::Fluted, [0.56, 0.56, 0.53], [0.38, 0.38, 0.36]),
    ));
    v.push(sp(
        "malus-domestica",
        "Apple",
        "Malus domestica",
        Tree,
        6.0,
        6.0,
        false,
        &[4, 5],
        Sun,
        Habit {
            wobble: 4.0,
            ..Habit::single(
                0.03,
                0.35,
                0.25,
                Hemispherical,
                [
                    lv(6, 0.6, 55.0, 137.5, 1, 15.0, 10.0, 0.0, 5, 0.7, 0.6),
                    lv(9, 0.5, 50.0, 137.5, 1, -5.0, 8.0, -0.15, 3, 0.5, 0.2),
                    lv(6, 0.4, 40.0, 137.5, 1, 0.0, 10.0, 0.0, 2, 0.5, 0.25),
                ],
            )
        }
        .form(),
        leaf(
            Ovate,
            0.07,
            0.6,
            0.3,
            [0.3, 0.43, 0.17],
            [0.6, 0.55, 0.2],
            0.5,
            2.2,
        ),
        flower(
            FlowerShape::Blossom,
            [0.97, 0.9, 0.92],
            [0.9, 0.8, 0.3],
            0.3,
            0.2,
        ),
        bark(BarkPattern::Flaky, [0.45, 0.4, 0.35], [0.3, 0.27, 0.24]),
    ));
    v.push(sp(
        "prunus-serrulata",
        "Japanese flowering cherry (in blossom)",
        "Prunus serrulata 'Kanzan'",
        Tree,
        8.0,
        8.0,
        false,
        &[4],
        Sun,
        Habit::single(
            0.025,
            0.35,
            0.3,
            InverseConical,
            [
                lv(6, 0.65, 35.0, 137.5, 1, -10.0, 6.0, 0.0, 5, 0.7, 0.6),
                lv(8, 0.5, 45.0, 137.5, 1, 0.0, 6.0, 0.0, 3, 0.5, 0.2),
                lv(6, 0.4, 40.0, 137.5, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Ovate,
            0.1,
            0.45,
            0.4,
            [0.3, 0.38, 0.16],
            [0.8, 0.4, 0.15],
            0.55,
            2.2,
        ),
        flower(
            FlowerShape::Blossom,
            [0.95, 0.62, 0.76],
            [0.85, 0.5, 0.6],
            0.5,
            0.65,
        ),
        bark(BarkPattern::Birch, [0.45, 0.26, 0.21], [0.24, 0.15, 0.12]),
    ));
    v.push(sp(
        "magnolia-soulangeana",
        "Saucer magnolia",
        "Magnolia x soulangeana",
        Tree,
        6.0,
        6.0,
        false,
        &[4],
        Sun,
        Habit::multi(
            3,
            25.0,
            0.2,
            0.02,
            0.6,
            0.2,
            Spherical,
            [
                lv(8, 0.5, 45.0, 137.5, 1, 20.0, 6.0, 0.0, 4, 0.6, 0.3),
                lv(6, 0.45, 40.0, 137.5, 1, 0.0, 6.0, 0.0, 3, 0.5, 0.2),
                lv(4, 0.4, 35.0, 137.5, 1, 0.0, 6.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Elliptic,
            0.15,
            0.5,
            0.0,
            [0.3, 0.42, 0.17],
            [0.6, 0.5, 0.25],
            0.5,
            2.0,
        ),
        flower(
            FlowerShape::Goblet,
            [0.97, 0.9, 0.92],
            [0.8, 0.4, 0.6],
            0.2,
            0.6,
        ),
        bark(BarkPattern::Smooth, [0.55, 0.53, 0.5], [0.42, 0.44, 0.38]),
    ));
    v.push(sp(
        "sorbus-aucuparia",
        "Rowan",
        "Sorbus aucuparia",
        Tree,
        10.0,
        6.0,
        false,
        &[5],
        Sun,
        Habit::single(
            0.02,
            0.75,
            0.25,
            Spherical,
            [
                lv(10, 0.4, 45.0, 137.5, 1, 25.0, 6.0, 0.0, 4, 0.55, 0.25),
                lv(7, 0.5, 40.0, 137.5, 1, 0.0, 6.0, 0.0, 3, 0.5, 0.2),
                lv(5, 0.4, 35.0, 137.5, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Pinnate { leaflets: 13 },
            0.2,
            0.3,
            0.5,
            [0.3, 0.45, 0.17],
            [0.75, 0.25, 0.1],
            0.6,
            2.2,
        ),
        flower(
            FlowerShape::Blossom,
            [0.95, 0.93, 0.85],
            [0.85, 0.8, 0.5],
            0.2,
            0.2,
        ),
        bark(BarkPattern::Smooth, [0.55, 0.53, 0.5], [0.4, 0.4, 0.37]),
    ));
    v.push(sp(
        "juglans-regia",
        "Walnut",
        "Juglans regia",
        Tree,
        20.0,
        18.0,
        false,
        &[],
        Sun,
        Habit::single(
            0.03,
            0.45,
            0.18,
            Spherical,
            [
                lv(8, 0.55, 55.0, 137.5, 1, 20.0, 6.0, 0.0, 6, 0.65, 0.4),
                lv(7, 0.5, 45.0, 137.5, 1, -5.0, 6.0, 0.0, 4, 0.5, 0.2),
                lv(5, 0.4, 40.0, 137.5, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Pinnate { leaflets: 7 },
            0.35,
            0.4,
            0.0,
            [0.28, 0.4, 0.15],
            [0.55, 0.45, 0.2],
            0.9,
            2.0,
        ),
        None,
        bark(BarkPattern::Furrowed, [0.6, 0.58, 0.55], [0.3, 0.29, 0.27]),
    ));
    v.push(sp(
        "platanus-hispanica",
        "London plane",
        "Platanus x hispanica",
        Tree,
        30.0,
        20.0,
        false,
        &[],
        Sun,
        Habit::single(
            0.025,
            0.6,
            0.15,
            Spherical,
            [
                lv(10, 0.5, 55.0, 137.5, 1, 25.0, 8.0, 0.0, 6, 0.6, 0.3),
                lv(8, 0.45, 50.0, 137.5, 1, -5.0, 6.0, 0.0, 4, 0.5, 0.2),
                lv(6, 0.4, 40.0, 137.5, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
            ],
        )
        .form(),
        leaf(
            Palmate {
                lobes: 5,
                depth: 0.45,
            },
            0.18,
            1.0,
            0.2,
            [0.3, 0.45, 0.16],
            [0.6, 0.45, 0.2],
            1.0,
            2.2,
        ),
        None,
        bark(BarkPattern::Flaky, [0.78, 0.75, 0.6], [0.45, 0.45, 0.35]),
    ));
    v.push(sp(
        "olea-europaea",
        "Olive",
        "Olea europaea",
        Tree,
        7.0,
        6.0,
        true,
        &[],
        Sun,
        Habit {
            flare: 0.5,
            wobble: 8.0,
            ..Habit::single(
                0.045,
                0.35,
                0.25,
                Spherical,
                [
                    lv(6, 0.55, 45.0, 137.5, 1, 15.0, 15.0, 0.0, 5, 0.7, 0.6),
                    lv(9, 0.5, 45.0, 90.0, 2, 0.0, 10.0, -0.1, 3, 0.5, 0.2),
                    lv(7, 0.4, 40.0, 90.0, 2, 0.0, 8.0, 0.0, 2, 0.5, 0.25),
                ],
            )
        }
        .form(),
        leaf(
            Lanceolate,
            0.06,
            0.18,
            0.0,
            [0.42, 0.48, 0.36],
            [0.42, 0.48, 0.36],
            0.45,
            2.0,
        ),
        None,
        bark(BarkPattern::Furrowed, [0.5, 0.48, 0.44], [0.28, 0.27, 0.25]),
    ));

    // ---- conifers ----
    v.push(sp(
        "pinus-sylvestris",
        "Scots pine",
        "Pinus sylvestris",
        Conifer,
        20.0,
        9.0,
        true,
        &[],
        Sun,
        Habit::single(
            0.022,
            0.95,
            0.55,
            Hemispherical,
            [
                lv(15, 0.3, 70.0, 40.0, 3, 15.0, 8.0, 0.1, 4, 0.45, 0.5),
                lv(8, 0.45, 50.0, 137.5, 1, 5.0, 8.0, 0.15, 3, 0.5, 0.2),
                NO,
            ],
        )
        .leaves_on(1, 0.4)
        .form(),
        leaf(
            Needles {
                length_m: 0.05,
                bundle: 2,
            },
            0.05,
            0.1,
            0.0,
            [0.22, 0.35, 0.24],
            [0.22, 0.35, 0.24],
            0.6,
            2.2,
        ),
        None,
        bark(BarkPattern::Flaky, [0.7, 0.42, 0.25], [0.4, 0.3, 0.25]),
    ));
    v.push(sp(
        "picea-abies",
        "Norway spruce",
        "Picea abies",
        Conifer,
        25.0,
        8.0,
        true,
        &[],
        Sun,
        Habit::single(
            0.02,
            1.0,
            0.05,
            Conical,
            [
                lv(72, 0.4, 80.0, 30.0, 4, 25.0, 4.0, -0.1, 5, 0.35, 0.04),
                lv(14, 0.45, 75.0, 137.5, 1, 0.0, 5.0, -0.6, 3, 0.4, 0.15),
                NO,
            ],
        )
        .leaves_on(1, 0.5)
        .form(),
        leaf(
            Needles {
                length_m: 0.018,
                bundle: 1,
            },
            0.018,
            0.1,
            0.0,
            [0.12, 0.25, 0.12],
            [0.12, 0.25, 0.12],
            0.6,
            2.8,
        ),
        None,
        bark(BarkPattern::Scaly, [0.45, 0.3, 0.22], [0.3, 0.22, 0.17]),
    ));
    v.push(sp(
        "abies-nordmanniana",
        "Nordmann fir",
        "Abies nordmanniana",
        Conifer,
        20.0,
        7.0,
        true,
        &[],
        HalfShade,
        Habit::single(
            0.02,
            1.0,
            0.03,
            Conical,
            [
                lv(60, 0.4, 75.0, 30.0, 5, 15.0, 3.0, 0.0, 4, 0.35, 0.03),
                lv(8, 0.4, 60.0, 180.0, 1, 0.0, 4.0, 0.0, 2, 0.4, 0.2),
                NO,
            ],
        )
        .leaves_on(1, 0.5)
        .form(),
        leaf(
            Needles {
                length_m: 0.025,
                bundle: 0,
            },
            0.025,
            0.1,
            0.0,
            [0.1, 0.25, 0.13],
            [0.1, 0.25, 0.13],
            0.55,
            2.8,
        ),
        None,
        bark(BarkPattern::Smooth, [0.5, 0.5, 0.48], [0.35, 0.35, 0.33]),
    ));
    v.push(sp(
        "taxus-baccata",
        "Yew",
        "Taxus baccata",
        Conifer,
        10.0,
        8.0,
        true,
        &[],
        Shade,
        Habit::multi(
            4,
            25.0,
            0.3,
            0.04,
            0.6,
            0.05,
            Hemispherical,
            [
                lv(14, 0.55, 55.0, 137.5, 1, 20.0, 6.0, 0.0, 4, 0.55, 0.1),
                lv(8, 0.45, 50.0, 180.0, 1, 0.0, 6.0, -0.1, 3, 0.5, 0.2),
                NO,
            ],
        )
        .leaves_on(1, 0.3)
        .form(),
        leaf(
            Needles {
                length_m: 0.025,
                bundle: 0,
            },
            0.025,
            0.1,
            0.0,
            [0.09, 0.19, 0.08],
            [0.09, 0.19, 0.08],
            0.6,
            3.0,
        ),
        None,
        bark(BarkPattern::Flaky, [0.5, 0.28, 0.2], [0.35, 0.22, 0.2]),
    ));
    v.push(sp(
        "thuja-smaragd",
        "Arborvitae 'Smaragd'",
        "Thuja occidentalis 'Smaragd'",
        Conifer,
        4.0,
        1.2,
        true,
        &[],
        Sun,
        Form::Clipped(ClippedParams {
            shape: ClipShape::Column,
            roughness: 0.05,
            tuft_m: 0.12,
            stem_m: 0.0,
        }),
        leaf(
            Scale,
            0.1,
            0.3,
            0.0,
            [0.22, 0.42, 0.12],
            [0.22, 0.42, 0.12],
            0.3,
            3.0,
        ),
        None,
        bark(BarkPattern::Fibrous, [0.5, 0.3, 0.2], [0.3, 0.18, 0.12]),
    ));
    v.push(sp(
        "cupressus-sempervirens",
        "Italian cypress",
        "Cupressus sempervirens 'Stricta'",
        Conifer,
        15.0,
        2.0,
        true,
        &[],
        Sun,
        Form::Clipped(ClippedParams {
            shape: ClipShape::Flame,
            roughness: 0.08,
            tuft_m: 0.2,
            stem_m: 0.3,
        }),
        leaf(
            Scale,
            0.1,
            0.3,
            0.0,
            [0.14, 0.26, 0.11],
            [0.14, 0.26, 0.11],
            0.5,
            3.0,
        ),
        None,
        bark(BarkPattern::Fibrous, [0.45, 0.35, 0.28], [0.25, 0.2, 0.16]),
    ));

    // ---- palm ----
    v.push(sp(
        "trachycarpus-fortunei",
        "Chusan palm",
        "Trachycarpus fortunei",
        Palm,
        6.0,
        2.6,
        true,
        &[],
        Sun,
        Form::Palm(PalmParams {
            fronds: 26,
            dead_fronds: 10,
            blade_m: 0.8,
            petiole_m: 0.45,
            trunk_radius_m: 0.13,
            lean_deg: 6.0,
        }),
        leaf(
            FanFrond,
            1.0,
            1.0,
            0.0,
            [0.2, 0.36, 0.14],
            [0.2, 0.36, 0.14],
            1.0,
            1.0,
        ),
        None,
        bark(BarkPattern::Fibrous, [0.38, 0.27, 0.16], [0.14, 0.1, 0.07]),
    ));

    // ---- shrubs ----
    let shrub = |stems,
                 spread,
                 base,
                 ratio,
                 leader,
                 cb,
                 shape,
                 l: [LevelParams; 3],
                 leaf_level,
                 leaf_start| {
        Habit::multi(stems, spread, base, ratio, leader, cb, shape, l)
            .leaves_on(leaf_level, leaf_start)
            .form()
    };
    v.push(sp(
        "buxus-sempervirens",
        "Boxwood",
        "Buxus sempervirens",
        Shrub,
        1.5,
        1.5,
        true,
        &[],
        HalfShade,
        shrub(
            6,
            30.0,
            0.15,
            0.02,
            0.75,
            0.1,
            Spherical,
            [
                lv(10, 0.4, 45.0, 137.5, 1, 10.0, 8.0, 0.0, 3, 0.6, 0.15),
                lv(6, 0.5, 40.0, 137.5, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.2),
                NO,
            ],
            1,
            0.2,
        ),
        leaf(
            Elliptic,
            0.02,
            0.55,
            0.0,
            [0.17, 0.32, 0.1],
            [0.17, 0.32, 0.1],
            0.2,
            3.0,
        ),
        None,
        bark(BarkPattern::Smooth, [0.6, 0.55, 0.45], [0.45, 0.4, 0.32]),
    ));
    v.push(sp(
        "hydrangea-macrophylla",
        "Bigleaf hydrangea",
        "Hydrangea macrophylla",
        Shrub,
        1.5,
        1.5,
        false,
        &[7, 8, 9],
        HalfShade,
        shrub(
            9,
            30.0,
            0.2,
            0.02,
            0.9,
            0.3,
            Spherical,
            [
                lv(5, 0.3, 40.0, 90.0, 2, 10.0, 6.0, 0.0, 3, 0.6, 0.4),
                NO,
                NO,
            ],
            0,
            0.4,
        ),
        leaf(
            Ovate,
            0.15,
            0.6,
            0.5,
            [0.3, 0.45, 0.18],
            [0.5, 0.45, 0.2],
            0.35,
            2.4,
        ),
        flower(
            FlowerShape::Mophead,
            [0.55, 0.58, 0.88],
            [0.5, 0.5, 0.8],
            0.25,
            0.25,
        ),
        bark(BarkPattern::Stem, [0.5, 0.45, 0.32], [0.4, 0.35, 0.25]),
    ));
    v.push(sp(
        "rhododendron",
        "Rhododendron",
        "Rhododendron catawbiense hybrid",
        Shrub,
        2.0,
        2.5,
        true,
        &[5],
        HalfShade,
        shrub(
            5,
            35.0,
            0.2,
            0.03,
            0.6,
            0.1,
            Hemispherical,
            [
                lv(6, 0.4, 50.0, 40.0, 3, 15.0, 10.0, 0.0, 3, 0.6, 0.3),
                lv(5, 0.45, 40.0, 40.0, 3, 0.0, 8.0, 0.0, 2, 0.5, 0.3),
                NO,
            ],
            2,
            0.4,
        ),
        leaf(
            Elliptic,
            0.12,
            0.35,
            0.0,
            [0.15, 0.28, 0.12],
            [0.15, 0.28, 0.12],
            0.35,
            2.8,
        ),
        flower(
            FlowerShape::Truss,
            [0.75, 0.35, 0.65],
            [0.95, 0.85, 0.5],
            0.25,
            0.35,
        ),
        bark(BarkPattern::Smooth, [0.45, 0.38, 0.32], [0.35, 0.3, 0.25]),
    ));
    v.push(sp(
        "prunus-laurocerasus",
        "Cherry laurel",
        "Prunus laurocerasus",
        Shrub,
        3.0,
        3.5,
        true,
        &[],
        HalfShade,
        shrub(
            6,
            40.0,
            0.3,
            0.025,
            0.6,
            0.05,
            Spherical,
            [
                lv(7, 0.45, 50.0, 137.5, 1, 15.0, 6.0, 0.0, 4, 0.6, 0.2),
                lv(6, 0.45, 45.0, 137.5, 1, 0.0, 6.0, 0.0, 2, 0.5, 0.2),
                NO,
            ],
            2,
            0.2,
        ),
        leaf(
            Elliptic,
            0.15,
            0.35,
            0.1,
            [0.15, 0.32, 0.1],
            [0.15, 0.32, 0.1],
            0.5,
            2.8,
        ),
        None,
        bark(BarkPattern::Smooth, [0.4, 0.36, 0.3], [0.3, 0.27, 0.22]),
    ));
    v.push(sp(
        "lavandula-angustifolia",
        "English lavender",
        "Lavandula angustifolia",
        Shrub,
        0.6,
        0.7,
        true,
        &[6, 7, 8],
        Sun,
        Form::Rosette(RosetteParams {
            leaves: 60,
            leaf_m: 0.3,
            min_elev_deg: 20.0,
            max_elev_deg: 80.0,
            arch_deg: 15.0,
            shoots: true,
            stalks: 60,
            stalk_spread_deg: 35.0,
            foliage_height: 0.5,
        }),
        leaf(
            Linear,
            0.04,
            0.12,
            0.0,
            [0.5, 0.58, 0.46],
            [0.5, 0.58, 0.46],
            0.25,
            3.0,
        ),
        flower(
            FlowerShape::Spike,
            [0.5, 0.42, 0.78],
            [0.4, 0.45, 0.35],
            0.1,
            1.0,
        ),
        bark(BarkPattern::Stem, [0.45, 0.52, 0.4], [0.4, 0.45, 0.35]),
    ));
    v.push(sp(
        "rosa-shrub",
        "Shrub rose",
        "Rosa (shrub hybrid)",
        Shrub,
        1.2,
        1.0,
        false,
        &[6, 7, 8, 9],
        Sun,
        shrub(
            6,
            25.0,
            0.1,
            0.012,
            0.95,
            0.3,
            Spherical,
            [
                lv(5, 0.3, 45.0, 137.5, 1, -10.0, 8.0, -0.2, 3, 0.6, 0.35),
                lv(3, 0.4, 40.0, 137.5, 1, 0.0, 8.0, 0.0, 2, 0.5, 0.3),
                NO,
            ],
            1,
            0.1,
        ),
        leaf(
            Pinnate { leaflets: 5 },
            0.12,
            0.5,
            0.6,
            [0.2, 0.36, 0.14],
            [0.5, 0.35, 0.15],
            0.3,
            3.0,
        ),
        flower(
            FlowerShape::Rose,
            [0.8, 0.12, 0.18],
            [0.95, 0.8, 0.3],
            0.18,
            0.18,
        ),
        bark(BarkPattern::Stem, [0.35, 0.42, 0.2], [0.45, 0.3, 0.2]),
    ));
    v.push(sp(
        "buddleja-davidii",
        "Butterfly bush",
        "Buddleja davidii",
        Shrub,
        3.0,
        3.0,
        false,
        &[7, 8, 9],
        Sun,
        shrub(
            8,
            35.0,
            0.3,
            0.015,
            0.8,
            0.2,
            InverseConical,
            [
                lv(6, 0.35, 40.0, 90.0, 2, -10.0, 6.0, -0.6, 4, 0.6, 0.3),
                lv(3, 0.4, 40.0, 90.0, 2, 0.0, 6.0, -0.3, 2, 0.5, 0.3),
                NO,
            ],
            1,
            0.2,
        ),
        leaf(
            Lanceolate,
            0.2,
            0.25,
            0.3,
            [0.38, 0.48, 0.32],
            [0.38, 0.45, 0.3],
            0.5,
            2.0,
        ),
        flower(
            FlowerShape::Panicle,
            [0.55, 0.35, 0.75],
            [0.95, 0.6, 0.2],
            0.35,
            0.3,
        ),
        bark(BarkPattern::Furrowed, [0.5, 0.45, 0.38], [0.3, 0.26, 0.2]),
    ));
    v.push(sp(
        "cornus-alba",
        "Red-barked dogwood",
        "Cornus alba 'Sibirica'",
        Shrub,
        2.5,
        2.5,
        false,
        &[],
        Sun,
        shrub(
            16,
            22.0,
            0.25,
            0.01,
            1.0,
            0.4,
            Cylindrical,
            [
                lv(4, 0.25, 35.0, 90.0, 2, 0.0, 6.0, 0.0, 2, 0.6, 0.5),
                NO,
                NO,
            ],
            0,
            0.5,
        ),
        leaf(
            Ovate,
            0.08,
            0.55,
            0.0,
            [0.3, 0.45, 0.18],
            [0.55, 0.15, 0.15],
            0.35,
            2.0,
        ),
        None,
        bark(BarkPattern::Stem, [0.72, 0.12, 0.1], [0.5, 0.08, 0.08]),
    ));
    v.push(sp(
        "phyllostachys",
        "Bamboo",
        "Phyllostachys aureosulcata",
        Shrub,
        5.0,
        2.5,
        true,
        &[],
        HalfShade,
        Habit {
            segs: 8,
            ..Habit::multi(
                24,
                12.0,
                0.4,
                0.006,
                1.0,
                0.35,
                Cylindrical,
                [
                    lv(14, 0.12, 55.0, 137.5, 1, 0.0, 6.0, -0.3, 2, 0.5, 0.35),
                    NO,
                    NO,
                ],
            )
            .leaves_on(1, 0.1)
        }
        .form(),
        leaf(
            Lanceolate,
            0.12,
            0.15,
            0.0,
            [0.3, 0.45, 0.15],
            [0.3, 0.45, 0.15],
            0.4,
            2.0,
        ),
        None,
        bark(BarkPattern::Stem, [0.58, 0.62, 0.26], [0.42, 0.45, 0.2]),
    ));

    // ---- clipped ----
    let box_leaf = leaf(
        Elliptic,
        0.02,
        0.55,
        0.0,
        [0.18, 0.33, 0.1],
        [0.18, 0.33, 0.1],
        0.12,
        3.0,
    );
    let yew_leaf = leaf(
        Needles {
            length_m: 0.025,
            bundle: 0,
        },
        0.025,
        0.1,
        0.0,
        [0.09, 0.2, 0.08],
        [0.09, 0.2, 0.08],
        0.18,
        3.0,
    );
    let clip = |shape, roughness, tuft_m| {
        Form::Clipped(ClippedParams {
            shape,
            roughness,
            tuft_m,
            stem_m: 0.0,
        })
    };
    let hedge = ClipShape::HedgeSegment { length_m: 1.0 };
    v.push(sp(
        "buxus-ball",
        "Box ball",
        "Buxus sempervirens (clipped)",
        Shrub,
        0.6,
        0.6,
        true,
        &[],
        HalfShade,
        clip(ClipShape::Ball, 0.012, 0.04),
        box_leaf.clone(),
        None,
        bark(BarkPattern::Smooth, [0.6, 0.55, 0.45], [0.45, 0.4, 0.32]),
    ));
    v.push(sp(
        "taxus-cone",
        "Yew cone",
        "Taxus baccata (clipped)",
        Shrub,
        1.8,
        1.0,
        true,
        &[],
        Shade,
        clip(ClipShape::Cone, 0.015, 0.05),
        yew_leaf.clone(),
        None,
        bark(BarkPattern::Flaky, [0.5, 0.28, 0.2], [0.35, 0.22, 0.2]),
    ));
    v.push(marcescent(sp(
        "carpinus-hedge",
        "Hornbeam hedge (1 m segment)",
        "Carpinus betulus (hedge)",
        Hedge,
        1.8,
        0.6,
        false,
        &[],
        HalfShade,
        clip(hedge, 0.02, 0.06),
        leaf(
            Ovate,
            0.08,
            0.5,
            0.6,
            [0.28, 0.42, 0.14],
            [0.8, 0.65, 0.2],
            0.22,
            3.0,
        ),
        None,
        bark(BarkPattern::Fluted, [0.56, 0.56, 0.53], [0.38, 0.38, 0.36]),
    )));
    v.push(marcescent(sp(
        "fagus-hedge",
        "Beech hedge (1 m segment)",
        "Fagus sylvatica (hedge)",
        Hedge,
        1.8,
        0.6,
        false,
        &[],
        HalfShade,
        clip(hedge, 0.02, 0.06),
        leaf(
            Ovate,
            0.08,
            0.6,
            0.1,
            [0.3, 0.42, 0.12],
            [0.7, 0.4, 0.12],
            0.22,
            3.0,
        ),
        None,
        bark(BarkPattern::Smooth, [0.56, 0.56, 0.53], [0.42, 0.48, 0.36]),
    )));
    v.push(sp(
        "taxus-hedge",
        "Yew hedge (1 m segment)",
        "Taxus baccata (hedge)",
        Hedge,
        1.6,
        0.7,
        true,
        &[],
        Shade,
        clip(hedge, 0.015, 0.05),
        yew_leaf,
        None,
        bark(BarkPattern::Flaky, [0.5, 0.28, 0.2], [0.35, 0.22, 0.2]),
    ));
    v.push(sp(
        "ligustrum-hedge",
        "Privet hedge (1 m segment)",
        "Ligustrum ovalifolium (hedge)",
        Hedge,
        1.6,
        0.6,
        true,
        &[],
        Sun,
        clip(hedge, 0.02, 0.05),
        leaf(
            Elliptic,
            0.05,
            0.45,
            0.0,
            [0.2, 0.38, 0.12],
            [0.2, 0.38, 0.12],
            0.18,
            3.0,
        ),
        None,
        bark(BarkPattern::Smooth, [0.5, 0.48, 0.42], [0.4, 0.38, 0.32]),
    ));

    // ---- grasses ----
    let grass = |blades, fans, bw, base, a0, a1, droop, fh, plumes, plume_m, spread| {
        Form::Tussock(TussockParams {
            blades,
            fans,
            blade_width_m: bw,
            base_radius_m: base,
            min_angle_deg: a0,
            max_angle_deg: a1,
            droop,
            foliage_height: fh,
            plumes,
            plume_m,
            plume_spread_deg: spread,
        })
    };
    let blade = |colour, autumn| leaf(Linear, 0.5, 0.05, 0.0, colour, autumn, 0.1, 1.0);
    let stem = bark(BarkPattern::Stem, GREEN_BARK, DARK_STEM);
    v.push(sp(
        "lawn-tuft",
        "Lawn grass tuft",
        "Lolium perenne / Festuca rubra",
        Grass,
        0.08,
        0.1,
        true,
        &[],
        Sun,
        grass(10, 0, 0.004, 0.015, 5.0, 35.0, 0.2, 1.0, 0, 0.0, 0.0),
        blade([0.25, 0.45, 0.14], [0.25, 0.45, 0.14]),
        None,
        stem.clone(),
    ));
    v.push(sp(
        "miscanthus-sinensis",
        "Chinese silver grass",
        "Miscanthus sinensis",
        Grass,
        1.8,
        1.3,
        false,
        &[8, 9, 10],
        Sun,
        grass(60, 14, 0.012, 0.15, 10.0, 55.0, 0.8, 0.7, 12, 0.3, 15.0),
        blade([0.3, 0.45, 0.2], [0.7, 0.55, 0.3]),
        flower(
            FlowerShape::Plume,
            [0.85, 0.76, 0.7],
            [0.6, 0.45, 0.4],
            0.16,
            1.0,
        ),
        stem.clone(),
    ));
    v.push(sp(
        "cortaderia-selloana",
        "Pampas grass",
        "Cortaderia selloana",
        Grass,
        2.5,
        1.8,
        true,
        &[8, 9, 10, 11],
        Sun,
        grass(70, 16, 0.01, 0.25, 10.0, 60.0, 0.9, 0.55, 14, 0.5, 18.0),
        blade([0.35, 0.45, 0.25], [0.35, 0.45, 0.25]),
        flower(
            FlowerShape::Plume,
            [0.95, 0.92, 0.82],
            [0.85, 0.8, 0.65],
            0.22,
            1.0,
        ),
        stem.clone(),
    ));
    v.push(sp(
        "pennisetum-alopecuroides",
        "Fountain grass",
        "Pennisetum alopecuroides",
        Grass,
        0.9,
        1.0,
        false,
        &[8, 9, 10],
        Sun,
        grass(55, 14, 0.006, 0.12, 15.0, 70.0, 0.9, 0.65, 20, 0.15, 40.0),
        blade([0.32, 0.47, 0.18], [0.75, 0.6, 0.25]),
        flower(
            FlowerShape::Bottlebrush,
            [0.75, 0.66, 0.62],
            [0.55, 0.45, 0.4],
            0.07,
            1.0,
        ),
        stem.clone(),
    ));
    v.push(sp(
        "festuca-glauca",
        "Blue fescue",
        "Festuca glauca",
        Grass,
        0.25,
        0.3,
        true,
        &[],
        Sun,
        grass(70, 10, 0.0025, 0.04, 0.0, 75.0, 0.3, 1.0, 0, 0.0, 0.0),
        blade([0.45, 0.6, 0.68], [0.45, 0.6, 0.68]),
        None,
        stem.clone(),
    ));
    v.push(sp(
        "calamagrostis-acutiflora",
        "Feather reed grass",
        "Calamagrostis x acutiflora 'Karl Foerster'",
        Grass,
        1.6,
        0.6,
        false,
        &[6, 7, 8, 9],
        Sun,
        grass(45, 10, 0.008, 0.1, 5.0, 35.0, 0.5, 0.45, 30, 0.3, 10.0),
        blade([0.3, 0.46, 0.18], [0.75, 0.65, 0.35]),
        flower(
            FlowerShape::Plume,
            [0.8, 0.7, 0.5],
            [0.6, 0.5, 0.35],
            0.05,
            1.0,
        ),
        stem,
    ));

    // ---- perennials and ferns ----
    let ros = |leaves, leaf_m, e0, e1, arch, shoots, stalks, spread, fh| {
        Form::Rosette(RosetteParams {
            leaves,
            leaf_m,
            min_elev_deg: e0,
            max_elev_deg: e1,
            arch_deg: arch,
            shoots,
            stalks,
            stalk_spread_deg: spread,
            foliage_height: fh,
        })
    };
    let green_stem = bark(BarkPattern::Stem, GREEN_BARK, DARK_STEM);
    v.push(sp(
        "hosta",
        "Hosta",
        "Hosta sieboldiana",
        Perennial,
        0.7,
        0.9,
        false,
        &[7, 8],
        Shade,
        ros(22, 0.4, 15.0, 55.0, 35.0, false, 6, 12.0, 0.5),
        leaf(
            Cordate,
            0.4,
            0.8,
            0.0,
            [0.25, 0.42, 0.24],
            [0.7, 0.6, 0.25],
            0.4,
            1.0,
        ),
        flower(
            FlowerShape::Spike,
            [0.78, 0.74, 0.92],
            [0.35, 0.45, 0.3],
            0.25,
            1.0,
        ),
        green_stem.clone(),
    ));
    v.push(sp(
        "hemerocallis",
        "Daylily",
        "Hemerocallis fulva",
        Perennial,
        0.9,
        0.8,
        false,
        &[6, 7, 8],
        Sun,
        ros(30, 0.65, 40.0, 80.0, 70.0, false, 6, 15.0, 0.55),
        leaf(
            Linear,
            0.65,
            0.04,
            0.0,
            [0.3, 0.46, 0.18],
            [0.65, 0.55, 0.25],
            0.65,
            1.0,
        ),
        flower(
            FlowerShape::Lily,
            [0.95, 0.5, 0.12],
            [0.95, 0.8, 0.3],
            0.13,
            2.0,
        ),
        green_stem.clone(),
    ));
    v.push(sp(
        "geranium-rozanne",
        "Cranesbill 'Rozanne'",
        "Geranium 'Gerwat'",
        Perennial,
        0.5,
        0.7,
        false,
        &[6, 7, 8, 9, 10],
        HalfShade,
        ros(40, 0.25, 10.0, 70.0, 20.0, true, 14, 30.0, 0.85),
        leaf(
            Palmate {
                lobes: 5,
                depth: 0.7,
            },
            0.08,
            1.0,
            0.3,
            [0.25, 0.42, 0.16],
            [0.6, 0.3, 0.2],
            0.25,
            2.5,
        ),
        flower(
            FlowerShape::Saucer,
            [0.45, 0.4, 0.85],
            [0.9, 0.9, 0.95],
            0.05,
            2.0,
        ),
        green_stem.clone(),
    ));
    v.push(sp(
        "nepeta-faassenii",
        "Catmint",
        "Nepeta x faassenii",
        Perennial,
        0.5,
        0.6,
        false,
        &[5, 6, 7, 8, 9],
        Sun,
        ros(36, 0.2, 10.0, 75.0, 20.0, true, 24, 35.0, 0.6),
        leaf(
            Ovate,
            0.03,
            0.7,
            0.4,
            [0.45, 0.55, 0.42],
            [0.45, 0.5, 0.4],
            0.2,
            2.5,
        ),
        flower(
            FlowerShape::Spike,
            [0.55, 0.55, 0.85],
            [0.4, 0.45, 0.4],
            0.2,
            1.0,
        ),
        green_stem.clone(),
    ));
    v.push(sp(
        "echinacea-purpurea",
        "Purple coneflower",
        "Echinacea purpurea",
        Perennial,
        0.9,
        0.5,
        false,
        &[7, 8, 9],
        Sun,
        ros(16, 0.22, 20.0, 65.0, 25.0, false, 8, 15.0, 0.4),
        leaf(
            Lanceolate,
            0.22,
            0.3,
            0.3,
            [0.26, 0.4, 0.16],
            [0.4, 0.35, 0.2],
            0.22,
            1.0,
        ),
        flower(
            FlowerShape::Daisy { droop: true },
            [0.85, 0.4, 0.6],
            [0.65, 0.35, 0.12],
            0.12,
            1.0,
        ),
        green_stem.clone(),
    ));
    v.push(sp(
        "dryopteris-filix-mas",
        "Male fern",
        "Dryopteris filix-mas",
        Fern,
        1.0,
        1.0,
        false,
        &[],
        Shade,
        ros(14, 1.0, 45.0, 75.0, 50.0, false, 0, 0.0, 1.0),
        leaf(
            FernFrond,
            1.0,
            0.3,
            0.0,
            [0.25, 0.42, 0.14],
            [0.55, 0.42, 0.2],
            1.0,
            1.0,
        ),
        None,
        green_stem,
    ));

    // ---- ground cover and climbers ----
    let ivy = leaf(
        Palmate {
            lobes: 5,
            depth: 0.35,
        },
        0.06,
        1.0,
        0.0,
        [0.13, 0.27, 0.1],
        [0.13, 0.27, 0.1],
        0.16,
        1.6,
    );
    v.push(sp(
        "hedera-ground",
        "Ivy ground cover (patch)",
        "Hedera helix",
        GroundCover,
        0.15,
        0.6,
        true,
        &[],
        Shade,
        Form::GroundCover(GroundCoverParams {
            cards: 30,
            depth_m: 0.1,
        }),
        ivy.clone(),
        None,
        bark(BarkPattern::Stem, [0.35, 0.3, 0.2], [0.25, 0.2, 0.15]),
    ));
    v.push(sp(
        "hedera-wall",
        "Ivy on a wall (2 x 2.5 m patch)",
        "Hedera helix",
        Climber,
        2.5,
        2.0,
        true,
        &[],
        Shade,
        Form::Climber(ClimberParams {
            patch_width_m: 2.0,
            stems: 4,
            depth_m: 0.12,
        }),
        LeafParams {
            card_m: 0.2,
            density: 3.5,
            ..ivy
        },
        None,
        bark(BarkPattern::Fibrous, [0.4, 0.33, 0.24], [0.25, 0.2, 0.15]),
    ));
    v
}
