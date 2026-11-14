//! How plants grow over the years after planting: one table of estimated growth data per
//! built-in species (`GROWTH`), a Chapman-Richards curve through it, and the sizes it gives.
//!
//! The numbers are estimates from common horticultural knowledge (nursery catalogues, tree
//! guides): mature height and width as the species catalogue states them, the years a plant
//! takes from germination to about 95 % of that size, and the age it usually has when planted.
//! They are all in the one table below so they are easy to correct.
//!
//! A clipped plant (hedge, topiary) grows the same way until it reaches its size and is then
//! kept there. Herbaceous plants reach full size in two or three years.

use serde::Serialize;

/// What kind of grower a species is: decides the curve's shape and how it is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grower {
    Tree,
    Conifer,
    Palm,
    Shrub,
    /// Hedges and topiary: clipped at their size once they reach it.
    Clipped,
    Grass,
    Perennial,
    GroundCover,
    Climber,
    /// The lawn's tufts: mown, never grow.
    Lawn,
}

use Grower::*;

/// Estimated growth data: (species id, grower, mature height m, mature width m, years to
/// maturity, usual age when planted in years). All values estimated.
#[rustfmt::skip]
pub const GROWTH: &[(&str, Grower, f32, f32, f32, f32)] = &[
    ("quercus-robur", Tree, 22.0, 26.0, 60.0, 8.0),
    ("fagus-sylvatica", Tree, 25.0, 18.0, 50.0, 8.0),
    ("betula-pendula", Tree, 18.0, 8.0, 18.0, 8.0),
    ("tilia-cordata", Tree, 22.0, 14.0, 35.0, 8.0),
    ("acer-campestre", Tree, 10.0, 8.0, 30.0, 8.0),
    ("acer-palmatum", Tree, 5.0, 5.0, 30.0, 8.0),
    ("salix-sepulcralis", Tree, 15.0, 15.0, 15.0, 8.0),
    ("populus-nigra-italica", Tree, 25.0, 4.0, 15.0, 8.0),
    ("carpinus-betulus", Tree, 16.0, 12.0, 35.0, 8.0),
    ("malus-domestica", Tree, 6.0, 6.0, 15.0, 4.0),
    ("prunus-serrulata", Tree, 8.0, 8.0, 20.0, 8.0),
    ("magnolia-soulangeana", Tree, 6.0, 6.0, 25.0, 8.0),
    ("sorbus-aucuparia", Tree, 10.0, 6.0, 20.0, 8.0),
    ("juglans-regia", Tree, 20.0, 18.0, 40.0, 8.0),
    ("platanus-hispanica", Tree, 30.0, 20.0, 40.0, 8.0),
    ("olea-europaea", Tree, 7.0, 6.0, 50.0, 4.0),
    ("pinus-sylvestris", Conifer, 20.0, 9.0, 40.0, 6.0),
    ("picea-abies", Conifer, 25.0, 8.0, 40.0, 6.0),
    ("abies-nordmanniana", Conifer, 20.0, 7.0, 45.0, 6.0),
    ("taxus-baccata", Conifer, 10.0, 8.0, 60.0, 6.0),
    ("thuja-smaragd", Conifer, 4.0, 1.2, 15.0, 6.0),
    ("cupressus-sempervirens", Conifer, 15.0, 2.0, 30.0, 6.0),
    ("trachycarpus-fortunei", Palm, 6.0, 2.6, 30.0, 6.0),
    ("buxus-sempervirens", Shrub, 1.5, 1.5, 20.0, 3.0),
    ("hydrangea-macrophylla", Shrub, 1.5, 1.5, 5.0, 3.0),
    ("rhododendron", Shrub, 2.0, 2.5, 12.0, 3.0),
    ("prunus-laurocerasus", Shrub, 3.0, 3.5, 8.0, 3.0),
    ("lavandula-angustifolia", Shrub, 0.6, 0.7, 3.0, 3.0),
    ("rosa-shrub", Shrub, 1.2, 1.0, 4.0, 3.0),
    ("buddleja-davidii", Shrub, 3.0, 3.0, 3.0, 3.0),
    ("cornus-alba", Shrub, 2.5, 2.5, 5.0, 3.0),
    ("phyllostachys", Shrub, 5.0, 2.5, 6.0, 3.0),
    ("buxus-ball", Clipped, 0.6, 0.6, 12.0, 4.0),
    ("taxus-cone", Clipped, 1.8, 1.0, 15.0, 4.0),
    ("carpinus-hedge", Clipped, 1.8, 0.6, 6.0, 2.0),
    ("fagus-hedge", Clipped, 1.8, 0.6, 7.0, 2.0),
    ("taxus-hedge", Clipped, 1.6, 0.7, 12.0, 4.0),
    ("ligustrum-hedge", Clipped, 1.6, 0.6, 5.0, 2.0),
    ("lawn-tuft", Lawn, 0.08, 0.1, 0.0, 0.0),
    ("miscanthus-sinensis", Grass, 1.8, 1.3, 3.0, 1.0),
    ("cortaderia-selloana", Grass, 2.5, 1.8, 3.0, 1.0),
    ("pennisetum-alopecuroides", Grass, 0.9, 1.0, 3.0, 1.0),
    ("festuca-glauca", Grass, 0.25, 0.3, 2.0, 1.0),
    ("calamagrostis-acutiflora", Grass, 1.6, 0.6, 3.0, 1.0),
    ("hosta", Perennial, 0.7, 0.9, 3.0, 1.0),
    ("hemerocallis", Perennial, 0.9, 0.8, 3.0, 1.0),
    ("geranium-rozanne", Perennial, 0.5, 0.7, 3.0, 1.0),
    ("nepeta-faassenii", Perennial, 0.5, 0.6, 3.0, 1.0),
    ("echinacea-purpurea", Perennial, 0.9, 0.5, 3.0, 1.0),
    ("dryopteris-filix-mas", Perennial, 1.0, 1.0, 3.0, 1.0),
    ("hedera-ground", GroundCover, 0.15, 0.6, 3.0, 1.0),
    ("hedera-wall", Climber, 2.5, 2.0, 6.0, 2.0),
    ("acer-platanoides", Tree, 20.0, 15.0, 30.0, 8.0),
    ("aesculus-hippocastanum", Tree, 25.0, 18.0, 40.0, 8.0),
    ("fraxinus-excelsior", Tree, 25.0, 15.0, 30.0, 8.0),
    ("alnus-glutinosa", Tree, 18.0, 8.0, 18.0, 8.0),
    ("salix-alba", Tree, 20.0, 14.0, 15.0, 8.0),
    ("castanea-sativa", Tree, 25.0, 18.0, 40.0, 8.0),
    ("robinia-pseudoacacia", Tree, 20.0, 12.0, 20.0, 8.0),
    ("pyrus-communis", Tree, 10.0, 6.0, 20.0, 4.0),
    ("prunus-domestica", Tree, 7.0, 6.0, 15.0, 4.0),
    ("ficus-carica", Tree, 5.0, 6.0, 15.0, 4.0),
    ("ginkgo-biloba", Tree, 18.0, 9.0, 50.0, 8.0),
    ("liquidambar-styraciflua", Tree, 18.0, 10.0, 35.0, 8.0),
    ("larix-decidua", Conifer, 25.0, 8.0, 30.0, 6.0),
    ("cupressocyparis-hedge", Clipped, 2.5, 0.9, 4.0, 2.0),
    ("pinus-mugo", Conifer, 1.5, 2.5, 25.0, 6.0),
    ("phoenix-canariensis", Palm, 10.0, 8.0, 40.0, 6.0),
    ("forsythia", Shrub, 2.5, 2.5, 5.0, 3.0),
    ("syringa-vulgaris", Shrub, 4.0, 3.0, 10.0, 3.0),
    ("sambucus-nigra", Shrub, 5.0, 4.0, 6.0, 3.0),
    ("ilex-aquifolium", Shrub, 6.0, 4.0, 25.0, 3.0),
    ("viburnum-opulus", Shrub, 3.5, 3.5, 7.0, 3.0),
    ("photinia-red-robin", Shrub, 3.0, 3.0, 8.0, 3.0),
    ("spiraea-vanhouttei", Shrub, 2.0, 2.5, 5.0, 3.0),
    ("weigela-florida", Shrub, 2.5, 2.5, 5.0, 3.0),
    ("mahonia-aquifolium", Shrub, 1.5, 1.5, 8.0, 3.0),
    ("skimmia-japonica", Shrub, 1.0, 1.2, 10.0, 3.0),
    ("hibiscus-syriacus", Shrub, 3.0, 2.0, 10.0, 3.0),
    ("pieris-japonica", Shrub, 2.0, 2.0, 12.0, 3.0),
    ("fargesia-hedge", Shrub, 3.0, 1.5, 6.0, 3.0),
    ("laurocerasus-hedge", Clipped, 2.0, 1.0, 5.0, 2.0),
    ("thuja-hedge", Clipped, 2.0, 0.6, 6.0, 2.0),
    ("photinia-hedge", Clipped, 1.8, 0.6, 5.0, 2.0),
    ("buxus-edging", Clipped, 0.3, 0.25, 6.0, 2.0),
    ("laurus-standard", Clipped, 2.2, 0.8, 10.0, 4.0),
    ("tilia-pleached", Clipped, 4.0, 0.7, 10.0, 4.0),
    ("carex-oshimensis", Grass, 0.35, 0.5, 3.0, 1.0),
    ("hakonechloa-macra", Grass, 0.4, 0.6, 3.0, 1.0),
    ("panicum-virgatum", Grass, 1.5, 0.8, 3.0, 1.0),
    ("typha-latifolia", Grass, 2.0, 0.8, 3.0, 1.0),
    ("paeonia-lactiflora", Perennial, 0.8, 0.9, 4.0, 1.0),
    ("delphinium-elatum", Perennial, 1.6, 0.6, 3.0, 1.0),
    ("lupinus-polyphyllus", Perennial, 1.0, 0.6, 3.0, 1.0),
    ("sedum-herbstfreude", Perennial, 0.5, 0.5, 3.0, 1.0),
    ("heuchera", Perennial, 0.4, 0.4, 3.0, 1.0),
    ("astilbe-arendsii", Perennial, 0.6, 0.5, 3.0, 1.0),
    ("salvia-nemorosa", Perennial, 0.5, 0.45, 3.0, 1.0),
    ("rudbeckia-fulgida", Perennial, 0.7, 0.6, 3.0, 1.0),
    ("phlox-paniculata", Perennial, 1.0, 0.6, 3.0, 1.0),
    ("iris-germanica", Perennial, 0.8, 0.5, 3.0, 1.0),
    ("alchemilla-mollis", Perennial, 0.4, 0.6, 3.0, 1.0),
    ("bergenia-cordifolia", Perennial, 0.4, 0.5, 3.0, 1.0),
    ("anemone-hupehensis", Perennial, 1.0, 0.6, 3.0, 1.0),
    ("tulipa-clump", Perennial, 0.45, 0.3, 1.0, 1.0),
    ("narcissus-clump", Perennial, 0.4, 0.3, 1.0, 1.0),
    ("allium-purple-sensation", Perennial, 0.8, 0.3, 1.0, 1.0),
    ("vinca-minor", GroundCover, 0.15, 0.6, 3.0, 1.0),
    ("pachysandra-terminalis", GroundCover, 0.2, 0.6, 3.0, 1.0),
    ("thymus-serpyllum", GroundCover, 0.08, 0.5, 3.0, 1.0),
    ("wisteria-wall", Climber, 3.0, 3.0, 10.0, 2.0),
    ("rosa-climbing-wall", Climber, 2.5, 2.0, 6.0, 2.0),
    ("clematis-wall", Climber, 2.5, 1.5, 6.0, 2.0),
    ("parthenocissus-wall", Climber, 3.0, 2.5, 6.0, 2.0),
    ("solanum-lycopersicum", Perennial, 1.6, 0.6, 1.0, 1.0),
    ("lactuca-sativa", Perennial, 0.2, 0.3, 1.0, 1.0),
    ("rheum-rhabarbarum", Perennial, 1.0, 1.5, 3.0, 1.0),
    ("fragaria-patch", GroundCover, 0.2, 0.6, 3.0, 1.0),
    ("nymphaea-patch", GroundCover, 0.12, 1.2, 3.0, 1.0),
    ("iris-pseudacorus", Perennial, 1.2, 0.8, 3.0, 1.0),
];

/// The growth data of an asset (`flora:<id>`), else a guess from its size: a plant taller than
/// 5 m grows as a tree. `size` is the asset's (height, width) at full size when known.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Growth {
    pub grower: Grower,
    pub mature_height_m: f32,
    pub mature_width_m: f32,
    pub years_to_maturity: f32,
    pub planted_age_years: f32,
    /// Whether the numbers come from the table (all estimated) or a guess from the size.
    pub from_table: bool,
}

pub fn growth_of(asset: &str, size: Option<(f32, f32)>) -> Growth {
    let id = asset.strip_prefix("flora:").unwrap_or(asset);
    if let Some(&(_, grower, h, w, years, planted)) = GROWTH.iter().find(|r| r.0 == id) {
        return Growth {
            grower,
            mature_height_m: h,
            mature_width_m: w,
            years_to_maturity: years,
            planted_age_years: planted,
            from_table: true,
        };
    }
    let (h, w) = size.unwrap_or((1.0, 1.0));
    let (grower, years, planted) = if h > 5.0 {
        (Tree, 30.0, 8.0)
    } else if h > 1.0 {
        (Shrub, 8.0, 3.0)
    } else {
        (Perennial, 3.0, 1.0)
    };
    Growth {
        grower,
        mature_height_m: h,
        mature_width_m: w,
        years_to_maturity: years,
        planted_age_years: planted,
        from_table: false,
    }
}

/// Size at an age, as fractions of the mature size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Size {
    /// 0..1 of the mature height.
    pub height: f32,
    /// 0..1 of the mature width: a young tree is narrow for its height.
    pub width: f32,
}

impl Growth {
    /// The Chapman-Richards curve `(1 - e^(-k t))^p`, k such that the plant has 95 % of its
    /// size at `years_to_maturity`; p > 1 gives woody plants their slow start.
    pub fn fraction(&self, age_years: f32) -> f32 {
        if self.grower == Lawn || self.years_to_maturity <= 0.0 {
            return 1.0;
        }
        let p = match self.grower {
            Tree | Conifer | Palm => 1.6,
            Shrub | Clipped | Climber => 1.3,
            _ => 1.0,
        };
        let k = -(1.0 - 0.95f32.powf(1.0 / p)).ln() / self.years_to_maturity;
        let s = (1.0 - (-k * age_years.max(0.0)).exp()).powf(p);
        // Clipped plants are kept at their size once there; others creep on to 1.
        if self.grower == Clipped {
            (s / 0.95).min(1.0)
        } else {
            s.min(1.0)
        }
    }

    pub fn size(&self, age_years: f32) -> Size {
        let s = self.fraction(age_years).max(0.02);
        let width = match self.grower {
            // Young trees are slender: the crown widens later than the tree rises.
            Tree | Conifer | Palm => s.powf(1.25),
            _ => s,
        };
        Size { height: s, width }
    }

    /// Trunk diameter at breast height (1.3 m), roughly: 2.5 % of the mature height for trees,
    /// growing a little faster than the height once the tree is tall; 0 for herbaceous plants.
    pub fn trunk_diameter_m(&self, age_years: f32) -> f32 {
        match self.grower {
            Tree | Conifer | Palm => {
                let s = self.fraction(age_years);
                self.mature_height_m * 0.035 * s.powf(1.4)
            }
            Shrub | Clipped | Climber => self.mature_height_m * 0.01 * self.fraction(age_years),
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn birch_outgrows_oak_and_hedges_stop_at_their_size() {
        let birch = growth_of("flora:betula-pendula", None);
        let oak = growth_of("flora:quercus-robur", None);
        assert!(birch.fraction(10.0) > oak.fraction(10.0) + 0.2);
        assert!((birch.fraction(birch.years_to_maturity) - 0.95).abs() < 0.01);
        assert_eq!(birch.fraction(0.0), 0.0);
        let hedge = growth_of("flora:carpinus-hedge", None);
        assert_eq!(hedge.fraction(40.0), 1.0);
        assert!(hedge.fraction(2.0) < 0.8);
        let perennial = growth_of("flora:nepeta-faassenii", None);
        assert!(perennial.fraction(3.0) > 0.9);
        // A young tree is narrow for its height, the mature one is not.
        let young = birch.size(3.0);
        assert!(young.width < young.height);
        assert!(birch.trunk_diameter_m(30.0) > birch.trunk_diameter_m(10.0));
        // Unknown assets are guessed from their size.
        assert_eq!(
            growth_of("pine_tree_01", Some((12.0, 6.0))).grower,
            Grower::Tree
        );
        assert!(GROWTH.iter().all(|r| r.2 > 0.0 && r.3 > 0.0 && r.4 >= 0.0));
    }
}
