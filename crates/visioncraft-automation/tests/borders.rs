//! The species of the border styles all exist in the built-in catalogue, and the mature size
//! the layout uses (the core's growth table) matches the species' own.

use visioncraft_core::border::{STYLES, style};

#[test]
fn every_species_of_every_style_is_in_the_catalogue() {
    let all = visioncraft_flora::species();
    for name in STYLES {
        let (species, [front, back]) = style(name).unwrap();
        assert!(front < back, "{name}");
        assert!(species.iter().any(|s| s.matrix), "{name} has no matrix");
        for s in &species {
            let id = s.asset.strip_prefix("flora:").unwrap();
            let found = all
                .iter()
                .find(|f| f.id == id)
                .unwrap_or_else(|| panic!("{name}: no species `{id}`"));
            let growth = visioncraft_core::growth::growth_of(&s.asset, None);
            assert!(growth.from_table, "{name}: `{id}` has no growth data");
            // The growth table and the catalogue say about the same.
            assert!(
                (growth.mature_height_m - found.mature_height_m).abs()
                    < 0.35 * found.mature_height_m,
                "{id}: {} vs {}",
                growth.mature_height_m,
                found.mature_height_m
            );
        }
    }
}
