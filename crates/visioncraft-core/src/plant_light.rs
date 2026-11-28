//! How much sun a plant wants: one estimated range of direct-sun hours per built-in species
//! (`LIGHT`, keyed like `growth::GROWTH`), and the ranking of species for a spot by the hours of
//! sun it gets (`query.plants_for_light`, `query.light_check`).
//!
//! A range is the hours of direct sun on a summer day the plant thrives in. The usual classes
//! are full sun (6 h and more), part shade (3 to 6 h) and full shade (under 3 h); many plants
//! span two of them.
//!
//! All values are estimated from general horticultural knowledge (nursery labels, planting
//! guides) and are not yet checked against open data. A species that is not in the table gets a
//! guess from its grower type, flagged `from_table: false`.

use crate::growth::{GROWTH, Grower, growth_of};
use crate::sun_hours::{FULL_SUN_HOURS, PART_SHADE_HOURS};
use serde::Serialize;
use serde_json::{Value, json};

/// The hours of direct sun a plant thrives in, on a summer day.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Light {
    pub min_hours: f32,
    pub max_hours: f32,
}

const fn l(min_hours: f32, max_hours: f32) -> Light {
    Light {
        min_hours,
        max_hours,
    }
}

/// Full sun: 6 hours and more.
const SUN: Light = l(6.0, 24.0);
/// Sun to part shade: 3 hours and more.
const SUN_PART: Light = l(3.0, 24.0);
/// Part shade: 3 to 6 hours.
const PART: Light = l(3.0, 6.0);
/// Part shade to full shade: up to 6 hours.
const PART_SHADE: Light = l(0.0, 6.0);
/// Full shade: under 3 hours.
const SHADE: Light = l(0.0, 3.0);
/// Anything from deep shade to full sun.
const ANY: Light = l(0.0, 24.0);

use Grower::*;

/// Estimated light needs: (species id, hours range). One row for each species of `GROWTH`.
/// All values estimated from general horticultural knowledge, not yet checked against open data.
#[rustfmt::skip]
pub const LIGHT: &[(&str, Light)] = &[
    ("quercus-robur", SUN_PART), ("fagus-sylvatica", SUN_PART), ("betula-pendula", SUN),
    ("tilia-cordata", SUN_PART), ("acer-campestre", SUN_PART), ("acer-palmatum", SUN_PART),
    ("salix-sepulcralis", SUN), ("populus-nigra-italica", SUN), ("carpinus-betulus", ANY),
    ("malus-domestica", SUN), ("prunus-serrulata", SUN), ("magnolia-soulangeana", SUN_PART),
    ("sorbus-aucuparia", SUN_PART), ("juglans-regia", SUN), ("platanus-hispanica", SUN),
    ("olea-europaea", SUN), ("pinus-sylvestris", SUN), ("picea-abies", SUN_PART),
    ("abies-nordmanniana", SUN_PART), ("taxus-baccata", ANY), ("thuja-smaragd", SUN_PART),
    ("cupressus-sempervirens", SUN), ("trachycarpus-fortunei", SUN_PART),
    ("buxus-sempervirens", ANY), ("hydrangea-macrophylla", PART), ("rhododendron", PART),
    ("prunus-laurocerasus", ANY), ("lavandula-angustifolia", SUN), ("rosa-shrub", SUN),
    ("buddleja-davidii", SUN), ("cornus-alba", SUN_PART), ("phyllostachys", SUN_PART),
    ("buxus-ball", ANY), ("taxus-cone", ANY), ("carpinus-hedge", ANY), ("fagus-hedge", SUN_PART),
    ("taxus-hedge", ANY), ("ligustrum-hedge", SUN_PART),
    // The lawn wants 4 hours and more.
    ("lawn-tuft", l(4.0, 24.0)),
    ("miscanthus-sinensis", SUN), ("cortaderia-selloana", SUN), ("pennisetum-alopecuroides", SUN),
    ("festuca-glauca", SUN), ("calamagrostis-acutiflora", SUN_PART),
    ("hosta", PART_SHADE), ("hemerocallis", SUN_PART), ("narcissus", SUN_PART),
    ("helleborus", PART_SHADE), ("geranium-rozanne", SUN_PART), ("nepeta-faassenii", SUN),
    ("echinacea-purpurea", SUN), ("dryopteris-filix-mas", SHADE),
    ("hedera-ground", ANY), ("hedera-wall", ANY),
    ("acer-platanoides", SUN_PART), ("aesculus-hippocastanum", SUN_PART), ("fraxinus-excelsior", SUN),
    ("alnus-glutinosa", SUN_PART), ("salix-alba", SUN), ("castanea-sativa", SUN),
    ("robinia-pseudoacacia", SUN), ("pyrus-communis", SUN), ("prunus-domestica", SUN),
    ("ficus-carica", SUN), ("ginkgo-biloba", SUN_PART), ("liquidambar-styraciflua", SUN),
    ("larix-decidua", SUN), ("cupressocyparis-hedge", SUN_PART), ("pinus-mugo", SUN),
    ("phoenix-canariensis", SUN), ("phoenix-dactylifera", SUN),
    ("forsythia", SUN_PART), ("syringa-vulgaris", SUN), ("sambucus-nigra", SUN_PART),
    ("ilex-aquifolium", ANY), ("viburnum-opulus", SUN_PART), ("photinia-red-robin", SUN_PART),
    ("spiraea-vanhouttei", SUN), ("weigela-florida", SUN), ("mahonia-aquifolium", PART_SHADE),
    ("skimmia-japonica", PART_SHADE), ("hibiscus-syriacus", SUN), ("pieris-japonica", PART),
    ("fargesia-hedge", SUN_PART), ("laurocerasus-hedge", ANY), ("thuja-hedge", SUN_PART),
    ("photinia-hedge", SUN_PART), ("buxus-edging", ANY), ("laurus-standard", SUN_PART),
    ("tilia-pleached", SUN_PART),
    ("carex-oshimensis", PART_SHADE), ("hakonechloa-macra", PART_SHADE), ("panicum-virgatum", SUN),
    ("typha-latifolia", SUN),
    ("paeonia-lactiflora", SUN), ("delphinium-elatum", SUN), ("lupinus-polyphyllus", SUN),
    ("sedum-herbstfreude", SUN), ("heuchera", PART_SHADE), ("astilbe-arendsii", PART_SHADE),
    ("salvia-nemorosa", SUN), ("rudbeckia-fulgida", SUN), ("phlox-paniculata", SUN_PART),
    ("iris-germanica", SUN), ("alchemilla-mollis", SUN_PART), ("bergenia-cordifolia", ANY),
    ("anemone-hupehensis", SUN_PART), ("tulipa-clump", SUN), ("narcissus-clump", SUN_PART),
    ("allium-purple-sensation", SUN), ("vinca-minor", ANY), ("pachysandra-terminalis", PART_SHADE),
    ("thymus-serpyllum", SUN), ("wisteria-wall", SUN), ("rosa-climbing-wall", SUN),
    ("clematis-wall", SUN_PART), ("parthenocissus-wall", ANY),
    ("solanum-lycopersicum", SUN), ("lactuca-sativa", SUN_PART), ("rheum-rhabarbarum", SUN_PART),
    ("fragaria-patch", SUN_PART), ("nymphaea-patch", SUN), ("iris-pseudacorus", SUN_PART),
];

/// The light a species wants, with where it comes from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LightNeed {
    pub min_hours: f32,
    pub max_hours: f32,
    /// Whether the range is from the table (estimated) or a guess from the grower type.
    pub from_table: bool,
    /// Always true for now: no value is checked against open data yet.
    pub estimated: bool,
}

impl LightNeed {
    pub fn light(&self) -> Light {
        l(self.min_hours, self.max_hours)
    }
}

/// The guess for a species that is not in the table: from what kind of grower it is.
pub fn guess_for(grower: Grower) -> Light {
    match grower {
        Palm | Grass => SUN,
        Lawn => l(4.0, 24.0),
        GroundCover => ANY,
        Tree | Conifer | Shrub | Clipped | Perennial | Climber => SUN_PART,
    }
}

/// The light needs of an asset (`flora:<id>`), else a guess from its grower type (see
/// `growth_of`: `size` is the asset's height and width when known).
pub fn light_of(asset: &str, size: Option<(f32, f32)>) -> LightNeed {
    let id = asset.strip_prefix("flora:").unwrap_or(asset);
    let (light, from_table) = match LIGHT.iter().find(|r| r.0 == id) {
        Some(r) => (r.1, true),
        None => (guess_for(growth_of(asset, size).grower), false),
    };
    LightNeed {
        min_hours: light.min_hours,
        max_hours: light.max_hours,
        from_table,
        estimated: true,
    }
}

/// How well a spot's hours suit a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// The hours lie inside the range.
    Full,
    /// Outside the range, but within one class (shade, part shade, full sun) of it.
    Partial,
}

/// 0 shade (under 3 h), 1 part shade, 2 full sun.
fn class(hours: f32) -> i32 {
    if hours >= FULL_SUN_HOURS as f32 {
        2
    } else if hours >= PART_SHADE_HOURS as f32 {
        1
    } else {
        0
    }
}

/// The fit of `hours` to `light`; `None` when the spot is two classes away (shade for a sun
/// lover, full sun for a shade plant).
pub fn fit(light: &Light, hours: f32) -> Option<Fit> {
    if hours >= light.min_hours && hours <= light.max_hours {
        return Some(Fit::Full);
    }
    // The classes the range covers (its top is exclusive: 3 h ends the shade class) against
    // the spot's class.
    let (lo, hi) = (class(light.min_hours), class(light.max_hours - 1e-3));
    let spot = class(hours);
    let distance = if spot < lo {
        lo - spot
    } else {
        (spot - hi).max(0)
    };
    (distance <= 1).then_some(Fit::Partial)
}

/// How far (hours) outside the range a plant still counts as well placed: sun hours are
/// estimates and the thresholds are not sharp.
pub const TOLERANCE_HOURS: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    TooSunny,
    TooShady,
}

/// Whether `hours` suit the range, within [`TOLERANCE_HOURS`].
pub fn verdict(light: &Light, hours: f32) -> Verdict {
    if hours < light.min_hours - TOLERANCE_HOURS {
        Verdict::TooShady
    } else if hours > light.max_hours + TOLERANCE_HOURS {
        Verdict::TooSunny
    } else {
        Verdict::Ok
    }
}

/// The grower types by their names in commands.
pub const GROWERS: &[(&str, Grower)] = &[
    ("tree", Tree),
    ("conifer", Conifer),
    ("palm", Palm),
    ("shrub", Shrub),
    ("clipped", Clipped),
    ("hedge", Clipped),
    ("grass", Grass),
    ("perennial", Perennial),
    ("ground_cover", GroundCover),
    ("climber", Climber),
    ("lawn", Lawn),
];

pub fn parse_grower(name: &str) -> Option<Grower> {
    GROWERS
        .iter()
        .find(|g| g.0 == name.to_lowercase())
        .map(|g| g.1)
}

fn range_label(light: &Light) -> String {
    let max = if light.max_hours >= 24.0 {
        "+".to_string()
    } else {
        format!("-{}", light.max_hours)
    };
    format!("{}{} h", light.min_hours, max)
}

fn reason(light: &Light, fit: Fit, hours: f32) -> String {
    let range = range_label(light);
    match fit {
        Fit::Full => format!("thrives in {range} of sun, the spot gets {hours:.1} h"),
        Fit::Partial if hours < light.min_hours => format!(
            "wants {range} of sun, the spot gets {hours:.1} h: {:.1} h short",
            light.min_hours - hours
        ),
        Fit::Partial => format!(
            "thrives in {range} of sun, the spot gets {hours:.1} h: {:.1} h too much",
            hours - light.max_hours
        ),
    }
}

/// A species ranked for a spot.
#[derive(Clone, Debug, PartialEq)]
pub struct Ranked {
    pub id: &'static str,
    pub grower: Grower,
    pub mature_height_m: f32,
    pub mature_width_m: f32,
    pub light: Light,
    pub fit: Fit,
    pub reason: String,
    /// Hours short of or over the range (0 inside it).
    pub gap_hours: f32,
}

impl Ranked {
    pub fn to_json(&self) -> Value {
        json!({
            "species": format!("flora:{}", self.id), "grower": self.grower,
            "fit": self.fit, "reason": self.reason,
            "light": {"min_hours": self.light.min_hours, "max_hours": self.light.max_hours},
            "mature_height_m": self.mature_height_m, "mature_width_m": self.mature_width_m,
            "from_table": true, "estimated": true,
        })
    }
}

/// How wide a range is, the open top counted as a 12 hour day: specialists rank before plants
/// that take anything.
fn width(light: &Light) -> f32 {
    light.max_hours.min(12.0) - light.min_hours.min(12.0)
}

/// The table's species that suit a spot with `hours` of sun, best first: those whose range holds
/// the hours (the narrowest range first: the plants that want exactly this light), then those
/// within one class (the smallest gap first). `grower` keeps one type only.
pub fn rank(hours: f32, grower: Option<Grower>) -> Vec<Ranked> {
    let mut out: Vec<Ranked> = GROWTH
        .iter()
        .filter(|g| grower.is_none_or(|w| g.1 == w))
        .filter_map(|g| {
            let light = LIGHT.iter().find(|r| r.0 == g.0)?.1;
            let f = fit(&light, hours)?;
            let gap = if hours < light.min_hours {
                light.min_hours - hours
            } else {
                (hours - light.max_hours).max(0.0)
            };
            Some(Ranked {
                id: g.0,
                grower: g.1,
                mature_height_m: g.2,
                mature_width_m: g.3,
                light,
                fit: f,
                reason: reason(&light, f, hours),
                gap_hours: gap,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        (a.fit as u8)
            .cmp(&(b.fit as u8))
            .then(a.gap_hours.total_cmp(&b.gap_hours))
            .then(width(&a.light).total_cmp(&width(&b.light)))
            .then(a.id.cmp(b.id))
    });
    out
}

/// The answer of `query.plants_for_light`: the spot (as the caller measured it, or just the
/// hours given), the hours the ranking is for and the best `limit` species.
pub fn plants_for_light_json(
    spot: Value,
    hours: f32,
    grower: Option<Grower>,
    limit: usize,
) -> Value {
    let ranked = rank(hours, grower);
    json!({
        "spot": spot,
        "hours": hours,
        "classes": {"full_sun": FULL_SUN_HOURS, "part_shade": PART_SHADE_HOURS},
        "grower": grower,
        "matches": ranked.len(),
        "full": ranked.iter().filter(|r| r.fit == Fit::Full).count(),
        "species": ranked.iter().take(limit).map(Ranked::to_json).collect::<Vec<_>>(),
        "note": "light ranges are estimated from general horticultural knowledge, not yet checked against open data",
    })
}

/// Up to `n` species of the same grower type as `asset` that suit `hours`, for swapping a
/// misfit: those that hold the hours first, the closest in mature height first.
pub fn suggest(asset: &str, hours: f32, n: usize) -> Vec<Ranked> {
    let me = growth_of(asset, None);
    let id = asset.strip_prefix("flora:").unwrap_or(asset);
    let mut v: Vec<Ranked> = rank(hours, Some(me.grower))
        .into_iter()
        .filter(|r| r.id != id)
        .collect();
    // Stable: the ranking order holds within one height distance.
    v.sort_by(|a, b| {
        (a.fit as u8).cmp(&(b.fit as u8)).then(
            (a.mature_height_m - me.mature_height_m)
                .abs()
                .total_cmp(&(b.mature_height_m - me.mature_height_m).abs()),
        )
    });
    v.truncate(n);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_growth_species_has_a_sane_range_and_no_stray_rows() {
        for g in GROWTH {
            let row = LIGHT.iter().find(|r| r.0 == g.0);
            let (_, light) = row.unwrap_or_else(|| panic!("no light for {}", g.0));
            assert!(
                light.min_hours >= 0.0
                    && light.min_hours <= light.max_hours
                    && light.max_hours <= 24.0,
                "{}",
                g.0
            );
            assert!(light_of(g.0, None).from_table);
        }
        for r in LIGHT {
            assert!(GROWTH.iter().any(|g| g.0 == r.0), "stray light row {}", r.0);
            assert_eq!(LIGHT.iter().filter(|x| x.0 == r.0).count(), 1, "{}", r.0);
        }
        // The lawn wants 4 hours and more.
        assert_eq!(light_of("flora:lawn-tuft", None).min_hours, 4.0);
    }

    #[test]
    fn unknown_species_are_guessed_from_the_grower() {
        let n = light_of("pine_tree_01", Some((12.0, 6.0)));
        assert!(!n.from_table && n.estimated);
        assert_eq!((n.min_hours, n.max_hours), (3.0, 24.0));
        let small = light_of("mystery", Some((0.3, 0.3)));
        assert!(!small.from_table);
    }

    #[test]
    fn fit_is_full_inside_partial_within_a_class_and_none_beyond() {
        let sun = SUN;
        assert_eq!(fit(&sun, 8.0), Some(Fit::Full));
        assert_eq!(fit(&sun, 6.0), Some(Fit::Full));
        assert_eq!(fit(&sun, 4.0), Some(Fit::Partial));
        assert_eq!(fit(&sun, 2.0), None);
        assert_eq!(fit(&SHADE, 2.0), Some(Fit::Full));
        assert_eq!(fit(&SHADE, 4.5), Some(Fit::Partial));
        assert_eq!(fit(&SHADE, 8.0), None);
        assert_eq!(fit(&PART, 8.0), Some(Fit::Partial));
        assert_eq!(verdict(&SUN, 2.0), Verdict::TooShady);
        assert_eq!(verdict(&SUN, 5.6), Verdict::Ok);
        assert_eq!(verdict(&SHADE, 9.0), Verdict::TooSunny);
    }

    #[test]
    fn sun_lovers_lead_at_eight_hours_and_shade_plants_at_two() {
        let at8 = rank(8.0, None);
        assert_eq!(at8[0].fit, Fit::Full);
        for id in ["lavandula-angustifolia", "echinacea-purpurea"] {
            let pos = at8.iter().position(|r| r.id == id).unwrap();
            assert!(at8[pos].fit == Fit::Full && pos < 40, "{id} at {pos}");
        }
        // No shade-only plant at 8 h.
        assert!(at8.iter().all(|r| r.id != "dryopteris-filix-mas"));
        let first_wide = at8.iter().position(|r| width(&r.light) > 11.0).unwrap();
        let lavender = at8
            .iter()
            .position(|r| r.id == "lavandula-angustifolia")
            .unwrap();
        assert!(lavender < first_wide);
        let at2 = rank(2.0, None);
        assert_eq!(at2[0].fit, Fit::Full);
        assert_eq!(at2[0].light, SHADE);
        let hosta = at2.iter().position(|r| r.id == "hosta").unwrap();
        let lavender = at2.iter().position(|r| r.id == "lavandula-angustifolia");
        assert!(lavender.is_none(), "a sun lover is two classes off at 2 h");
        assert!(at2[hosta].fit == Fit::Full);
        // The grower filter.
        assert!(
            rank(8.0, Some(Grower::Tree))
                .iter()
                .all(|r| r.grower == Grower::Tree)
        );
    }

    #[test]
    fn suggestions_stay_with_the_grower_and_leave_the_plant_out() {
        let s = suggest("flora:rosa-shrub", 2.0, 3);
        assert!(!s.is_empty() && s.len() <= 3);
        assert!(
            s.iter()
                .all(|r| r.grower == Grower::Shrub && r.id != "rosa-shrub")
        );
        assert_eq!(s[0].fit, Fit::Full);
    }

    #[test]
    fn growers_parse() {
        assert_eq!(parse_grower("Tree"), Some(Grower::Tree));
        assert_eq!(parse_grower("ground_cover"), Some(Grower::GroundCover));
        assert_eq!(parse_grower("fern"), None);
    }
}
