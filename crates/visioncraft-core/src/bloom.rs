//! What flowers when: the bloom calendar of a design's plants (`query.bloom_calendar`). The
//! core counts the plants of the plantings as [`quantities`] does and lays each species' bloom
//! months over the year, six months on south of the equator. When a species flowers, in what
//! colour and how it is called is the species library's to say, which the core does not link:
//! a host answers for every species ([`Bloom`]); run by the core alone, every species is
//! `unknown`.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::project::Project;
use crate::quantities::{Filter, Scene, is_plant, planting_counts, selection};
use crate::season::shift_month;

/// Said in the command's description when only the core answers; a host replaces it with
/// [`HOST_NOTE`].
pub const CORE_NOTE: &str = "Run by the core alone, every species is in `unknown` and nothing flowers: the bloom months are the species library's, which a host reads (the window, `visioncraft-cli` or the MCP server).";

/// What a host's description of the command says in place of [`CORE_NOTE`].
pub const HOST_NOTE: &str = "A built-in species' bloom months and flower colour come from the species library, a custom species' from its parameters; the core alone knows neither.";

/// What a host knows of a species for the calendar.
#[derive(Clone, Debug, PartialEq)]
pub struct Bloom {
    /// The months with flowers, 1 = January, as the species data states them: written for the
    /// northern hemisphere. The calendar shifts them for the site.
    pub months: Vec<u8>,
    /// The flower colour, sRGB 0..1; `None` when the species data has none.
    pub colour: Option<[f32; 3]>,
    /// What the species is called; `None` when the host has no name for it.
    pub name: Option<String>,
}

/// A species of the design: a built-in one (`flora:`), or one the project made itself.
fn is_species(project: &Project, asset: &str) -> bool {
    is_plant(asset) || project.species.iter().any(|c| c.id == asset)
}

/// The species a calendar row is about.
fn species_id(row: &Value) -> &str {
    row["species"].as_str().unwrap_or_default()
}

/// The bloom calendar of the plantings `filter` selects: `{hemisphere, months, species, gaps,
/// unknown}`. Their plants are counted as [`crate::quantities::quantities_in`] counts them
/// (`scene`: as the host draws them); lights and objects a planting places are not species and
/// are left out. A species flowers in a month when that month is in its bloom months, shifted
/// for the site's hemisphere ([`shift_month`]); `bloom` says when a species flowers, and a
/// species it knows nothing about, or whose data holds no month of the year, is `unknown`.
/// Within a month the species stand by their plants, the most first, then by id. An error on
/// an unknown planting or layer.
pub fn calendar(
    project: &Project,
    filter: &Filter,
    scene: Option<&Scene>,
    bloom: &dyn Fn(&str) -> Option<Bloom>,
) -> Result<Value, String> {
    for id in filter.ids.unwrap_or_default() {
        if !project.generators.iter().any(|g| g.id() == id) {
            return Err(format!("no planting `{id}`"));
        }
    }
    let sel = selection(project, filter)?;
    let latitude = project.site.latitude;
    let (mut flowering, mut per_month) = (<[Vec<Value>; 12]>::default(), [0usize; 12]);
    let (mut species, mut unknown) = (Vec::new(), Vec::new());
    // By asset id, so each month's list starts in the order the tie-break wants.
    for (asset, plants) in &planting_counts(project, &sel, scene).by_asset {
        if !is_species(project, asset) {
            continue;
        }
        let info = bloom(asset);
        let months: Vec<u8> = info
            .iter()
            .flat_map(|b| b.months.iter().copied())
            .filter(|m| (1..=12).contains(m))
            .map(|m| {
                u8::try_from(shift_month(u32::from(m), latitude)).expect("a month of the year")
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if months.is_empty() {
            unknown.push(json!({"species": asset, "plants": plants}));
            continue;
        }
        let mut row = json!({"species": asset, "plants": plants, "bloom_months": months});
        let o = row.as_object_mut().expect("a row is an object");
        if let Some(colour) = info.as_ref().and_then(|b| b.colour) {
            o.insert("colour".into(), json!(colour));
        }
        if let Some(name) = info.as_ref().and_then(|b| b.name.clone()) {
            o.insert("name".into(), json!(name));
        }
        for m in &months {
            flowering[usize::from(m - 1)].push(row.clone());
            per_month[usize::from(m - 1)] += *plants;
        }
        species.push(row);
    }
    let (mut months, mut gaps) = (Vec::with_capacity(12), Vec::new());
    for (i, (list, plants)) in flowering.into_iter().zip(per_month).enumerate() {
        let mut list = list;
        list.sort_by(|a, b| {
            b["plants"]
                .as_u64()
                .cmp(&a["plants"].as_u64())
                .then_with(|| species_id(a).cmp(species_id(b)))
        });
        if list.is_empty() {
            gaps.push(i as u64 + 1);
        }
        let count = list.len();
        months.push(json!({
            "month": i as u64 + 1, "flowering": list, "plants": plants, "species": count,
        }));
    }
    Ok(json!({
        "hemisphere": if latitude < 0.0 { "south" } else { "north" },
        "months": months,
        "species": species,
        "gaps": gaps,
        "unknown": unknown,
    }))
}
