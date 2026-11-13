//! Ready-made layered plantings, as data. Asset ids are `flora:<species id>` of the built-in
//! plant generator. Densities follow common forestry and planting practice, stated per preset.

use crate::planting::{PlantLayer, Weighted};

#[derive(Clone, Debug, PartialEq)]
pub struct PlantingPreset {
    pub id: String,
    pub name: String,
    pub layers: Vec<PlantLayer>,
}

/// A layer from `(species, weight)` pairs; scale 0.85-1.15 unless it is ground cover, which varies more.
fn layer(
    name: &str,
    species: &[(&str, f32)],
    density_per_m2: f32,
    min_distance_m: f32,
    keep_clear_m: f32,
) -> PlantLayer {
    PlantLayer {
        name: name.into(),
        asset_palette: species
            .iter()
            .map(|(s, w)| Weighted {
                asset: format!("flora:{s}"),
                weight: *w,
            })
            .collect(),
        density_per_m2,
        min_distance_m,
        scale_min: 0.85,
        scale_max: 1.15,
        random_rotation: true,
        keep_clear_m,
    }
}

fn preset(id: &str, name: &str, layers: Vec<PlantLayer>) -> PlantingPreset {
    PlantingPreset {
        id: id.into(),
        name: name.into(),
        layers,
    }
}

pub fn planting_presets() -> Vec<PlantingPreset> {
    vec![
        // Mature broadleaf stands carry 100-200 canopy stems per hectare (crowns 8-10 m), with
        // an understorey a few times denser and a dense forest floor.
        preset(
            "broadleaf-woodland",
            "Broadleaf woodland",
            vec![
                layer(
                    "Canopy",
                    &[
                        ("quercus-robur", 3.0),
                        ("fagus-sylvatica", 3.0),
                        ("carpinus-betulus", 2.0),
                        ("tilia-cordata", 2.0),
                    ],
                    0.012,
                    5.0,
                    0.0,
                ),
                layer(
                    "Understorey",
                    &[("ilex-aquifolium", 2.0), ("sambucus-nigra", 1.0)],
                    0.06,
                    2.0,
                    1.5,
                ),
                layer(
                    "Ground",
                    &[("dryopteris-filix-mas", 2.0), ("hedera-ground", 1.0)],
                    0.8,
                    0.5,
                    1.0,
                ),
            ],
        ),
        // Spruce, fir, pine and larch stands are planted tighter than broadleaves (2-3 m
        // spacing when young, thinned to 300-600 stems/ha); under dense conifers little grows.
        preset(
            "conifer-woodland",
            "Conifer woodland",
            vec![
                layer(
                    "Canopy",
                    &[
                        ("picea-abies", 3.0),
                        ("abies-nordmanniana", 2.0),
                        ("pinus-sylvestris", 2.0),
                        ("larix-decidua", 1.0),
                    ],
                    0.03,
                    3.0,
                    0.0,
                ),
                layer("Understorey", &[("taxus-baccata", 1.0)], 0.02, 3.0, 2.0),
                layer("Ground", &[("dryopteris-filix-mas", 1.0)], 0.3, 0.7, 1.2),
            ],
        ),
        // A mix of both stands, canopy density between the two.
        preset(
            "mixed-woodland",
            "Mixed woodland",
            vec![
                layer(
                    "Canopy",
                    &[
                        ("quercus-robur", 2.0),
                        ("picea-abies", 2.0),
                        ("betula-pendula", 2.0),
                        ("sorbus-aucuparia", 1.0),
                    ],
                    0.02,
                    4.0,
                    0.0,
                ),
                layer(
                    "Understorey",
                    &[
                        ("ilex-aquifolium", 2.0),
                        ("taxus-baccata", 1.0),
                        ("sambucus-nigra", 1.0),
                    ],
                    0.06,
                    2.0,
                    1.5,
                ),
                layer(
                    "Ground",
                    &[("dryopteris-filix-mas", 2.0), ("hedera-ground", 1.0)],
                    0.6,
                    0.5,
                    1.0,
                ),
            ],
        ),
        // The edge is open and light: fewer trees (a tree per 10 m x 10 m), shrubs at a
        // 3-4 m spacing in groups, and a herb layer of grasses and perennials.
        preset(
            "woodland-edge",
            "Woodland edge",
            vec![
                layer(
                    "Canopy",
                    &[
                        ("sorbus-aucuparia", 2.0),
                        ("betula-pendula", 2.0),
                        ("acer-campestre", 2.0),
                    ],
                    0.01,
                    5.0,
                    0.0,
                ),
                layer(
                    "Shrubs",
                    &[("cornus-alba", 1.0), ("viburnum-opulus", 1.0)],
                    0.08,
                    1.5,
                    1.5,
                ),
                layer(
                    "Herbs",
                    &[
                        ("calamagrostis-acutiflora", 2.0),
                        ("geranium-rozanne", 1.0),
                        ("alchemilla-mollis", 1.0),
                    ],
                    1.0,
                    0.4,
                    0.8,
                ),
            ],
        ),
        // Standard orchard trees stand 8-10 m apart (about 100-150/ha) over a mown sward.
        preset(
            "orchard",
            "Orchard",
            vec![
                layer(
                    "Trees",
                    &[
                        ("malus-domestica", 3.0),
                        ("pyrus-communis", 2.0),
                        ("prunus-domestica", 2.0),
                        ("prunus-serrulata", 1.0),
                    ],
                    0.011,
                    7.0,
                    0.0,
                ),
                layer("Lawn", &[("lawn-tuft", 1.0)], 3.0, 0.2, 0.6),
            ],
        ),
        // Meadows are mostly grass (hundreds of tillers per m2) with 10-20 percent forbs.
        preset(
            "wildflower-meadow",
            "Wildflower meadow",
            vec![
                layer(
                    "Grasses",
                    &[
                        ("calamagrostis-acutiflora", 1.0),
                        ("festuca-glauca", 2.0),
                        ("panicum-virgatum", 1.0),
                    ],
                    3.0,
                    0.2,
                    0.0,
                ),
                layer(
                    "Flowers",
                    &[
                        ("echinacea-purpurea", 1.0),
                        ("rudbeckia-fulgida", 1.0),
                        ("salvia-nemorosa", 1.0),
                        ("geranium-rozanne", 1.0),
                    ],
                    1.0,
                    0.25,
                    0.1,
                ),
            ],
        ),
        // Border perennials go in at 3-5 per m2; grasses are accents at about 1 per 3 m2.
        preset(
            "perennial-border",
            "Perennial border",
            vec![
                layer(
                    "Grasses",
                    &[
                        ("miscanthus-sinensis", 1.0),
                        ("calamagrostis-acutiflora", 1.0),
                        ("pennisetum-alopecuroides", 1.0),
                    ],
                    0.35,
                    1.0,
                    0.0,
                ),
                layer(
                    "Perennials",
                    &[
                        ("salvia-nemorosa", 1.0),
                        ("echinacea-purpurea", 1.0),
                        ("rudbeckia-fulgida", 1.0),
                        ("nepeta-faassenii", 1.0),
                        ("geranium-rozanne", 1.0),
                        ("alchemilla-mollis", 1.0),
                    ],
                    4.0,
                    0.25,
                    0.5,
                ),
            ],
        ),
        // A mixed native hedge is planted 3-5 per running metre in a double row; over a 2 m wide
        // strip that is about 2 plants per m2, lowered to 1.5 here to let the shapes read.
        preset(
            "hedgerow",
            "Hedgerow mix",
            vec![layer(
                "Shrubs",
                &[
                    ("carpinus-betulus", 2.0),
                    ("acer-campestre", 2.0),
                    ("cornus-alba", 1.0),
                    ("viburnum-opulus", 1.0),
                ],
                1.5,
                0.4,
                0.0,
            )],
        ),
        // Marginal plants stand close: 4-6 per m2 for reeds and bulrush, sparser irises.
        preset(
            "pond-margin",
            "Pond margin",
            vec![
                layer(
                    "Reeds",
                    &[("typha-latifolia", 2.0), ("carex-oshimensis", 1.0)],
                    3.0,
                    0.2,
                    0.0,
                ),
                layer("Irises", &[("iris-pseudacorus", 1.0)], 0.8, 0.5, 0.3),
            ],
        ),
        // Mediterranean gardens are sparse: one olive per 80-100 m2, cypress as accents, and
        // lavender at about 3 per m2 as a low drift, thyme between.
        preset(
            "mediterranean",
            "Mediterranean",
            vec![
                layer(
                    "Trees",
                    &[
                        ("olea-europaea", 3.0),
                        ("cupressus-sempervirens", 2.0),
                        ("trachycarpus-fortunei", 1.0),
                    ],
                    0.012,
                    4.0,
                    0.0,
                ),
                layer("Shrubs", &[("lavandula-angustifolia", 1.0)], 1.2, 0.35, 0.8),
                layer("Ground", &[("thymus-serpyllum", 1.0)], 2.0, 0.25, 0.3),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every species id of `crates/visioncraft-flora` (species.rs and species/more.rs), copied;
    /// the app tests the real list.
    const FLORA_IDS: &[&str] = &[
        "abies-nordmanniana",
        "acer-campestre",
        "acer-palmatum",
        "acer-platanoides",
        "aesculus-hippocastanum",
        "alchemilla-mollis",
        "allium-purple-sensation",
        "alnus-glutinosa",
        "anemone-hupehensis",
        "astilbe-arendsii",
        "bergenia-cordifolia",
        "betula-pendula",
        "buddleja-davidii",
        "buxus-ball",
        "buxus-edging",
        "buxus-sempervirens",
        "calamagrostis-acutiflora",
        "carex-oshimensis",
        "carpinus-betulus",
        "carpinus-hedge",
        "castanea-sativa",
        "clematis-wall",
        "cornus-alba",
        "cortaderia-selloana",
        "cupressocyparis-hedge",
        "cupressus-sempervirens",
        "delphinium-elatum",
        "dryopteris-filix-mas",
        "echinacea-purpurea",
        "fagus-hedge",
        "fagus-sylvatica",
        "fargesia-hedge",
        "festuca-glauca",
        "ficus-carica",
        "forsythia",
        "fragaria-patch",
        "fraxinus-excelsior",
        "geranium-rozanne",
        "ginkgo-biloba",
        "hakonechloa-macra",
        "hedera-ground",
        "hedera-wall",
        "hemerocallis",
        "heuchera",
        "hibiscus-syriacus",
        "hosta",
        "hydrangea-macrophylla",
        "ilex-aquifolium",
        "iris-germanica",
        "iris-pseudacorus",
        "juglans-regia",
        "lactuca-sativa",
        "larix-decidua",
        "laurocerasus-hedge",
        "laurus-standard",
        "lavandula-angustifolia",
        "lawn-tuft",
        "ligustrum-hedge",
        "liquidambar-styraciflua",
        "lupinus-polyphyllus",
        "magnolia-soulangeana",
        "mahonia-aquifolium",
        "malus-domestica",
        "miscanthus-sinensis",
        "narcissus-clump",
        "nepeta-faassenii",
        "nymphaea-patch",
        "olea-europaea",
        "pachysandra-terminalis",
        "paeonia-lactiflora",
        "panicum-virgatum",
        "parthenocissus-wall",
        "pennisetum-alopecuroides",
        "phlox-paniculata",
        "phoenix-canariensis",
        "photinia-hedge",
        "photinia-red-robin",
        "phyllostachys",
        "picea-abies",
        "pieris-japonica",
        "pinus-mugo",
        "pinus-sylvestris",
        "platanus-hispanica",
        "populus-nigra-italica",
        "prunus-domestica",
        "prunus-laurocerasus",
        "prunus-serrulata",
        "pyrus-communis",
        "quercus-robur",
        "rheum-rhabarbarum",
        "rhododendron",
        "robinia-pseudoacacia",
        "rosa-climbing-wall",
        "rosa-shrub",
        "rudbeckia-fulgida",
        "salix-alba",
        "salix-sepulcralis",
        "salvia-nemorosa",
        "sambucus-nigra",
        "sedum-herbstfreude",
        "skimmia-japonica",
        "solanum-lycopersicum",
        "sorbus-aucuparia",
        "spiraea-vanhouttei",
        "syringa-vulgaris",
        "taxus-baccata",
        "taxus-cone",
        "taxus-hedge",
        "thuja-hedge",
        "thuja-smaragd",
        "thymus-serpyllum",
        "tilia-cordata",
        "tilia-pleached",
        "trachycarpus-fortunei",
        "tulipa-clump",
        "typha-latifolia",
        "viburnum-opulus",
        "vinca-minor",
        "weigela-florida",
        "wisteria-wall",
    ];

    #[test]
    fn presets_use_existing_species_and_valid_layers() {
        let presets = planting_presets();
        assert_eq!(presets.len(), 10);
        for p in &presets {
            assert!(!p.layers.is_empty());
            for l in &p.layers {
                assert!(l.density_per_m2 > 0.0 && l.scale_min <= l.scale_max);
                for w in &l.asset_palette {
                    let id = w.asset.strip_prefix("flora:").unwrap();
                    assert!(FLORA_IDS.contains(&id), "{}: unknown species `{id}`", p.id);
                }
            }
        }
    }
}
