//! `query.bloom_calendar`: what flowers each month, its plants counted as `query.quantities`
//! counts them and its months shifted for the site's hemisphere. The core alone knows no
//! species (the flora library is a host's), so these tests say when each species flower
//! themselves: the table below is made up for the layout, not botanical data.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use visioncraft_core::bloom::Bloom;
use visioncraft_core::quantities::Filter;
use visioncraft_core::{Project, Session, evaluate};

/// When the species of these tests flower: (months in the north, flower colour). Invented.
fn flowering(asset: &str) -> Option<Bloom> {
    let (months, colour): (&[u8], Option<[f32; 3]>) = match asset {
        "flora:delphinium-elatum" => (&[6, 7], Some([0.25, 0.3, 0.85])),
        "flora:phlox-paniculata" => (&[7, 8], Some([0.9, 0.5, 0.9])),
        "flora:lupinus-polyphyllus" => (&[5, 6], Some([0.6, 0.4, 0.8])),
        "flora:anemone-hupehensis" => (&[8, 9, 10], Some([0.95, 0.95, 0.9])),
        "flora:paeonia-lactiflora" => (&[5, 6], Some([0.95, 0.85, 0.9])),
        "flora:echinacea-purpurea" => (&[7, 8, 9], Some([0.8, 0.4, 0.7])),
        "flora:rudbeckia-fulgida" => (&[8, 9, 10], Some([0.95, 0.8, 0.2])),
        "flora:iris-germanica" => (&[5, 6], Some([0.5, 0.4, 0.8])),
        "flora:salvia-nemorosa" => (&[6, 7, 8, 9], Some([0.5, 0.5, 0.9])),
        "flora:lavandula-angustifolia" => (&[6, 7, 8], Some([0.6, 0.5, 0.85])),
        "flora:nepeta-faassenii" => (&[5, 6, 9], Some([0.6, 0.6, 0.9])),
        "flora:geranium-rozanne" => (&[6, 7, 8, 9], Some([0.5, 0.6, 0.9])),
        // A species the table has an entry for, without bloom months.
        "flora:alchemilla-mollis" => (&[], None),
        // A custom species of the project.
        "sp1" => (&[3, 4], Some([1.0, 0.0, 0.0])),
        // Nothing is known for anything else: an oak, and the lamp is no plant at all.
        _ => return None,
    };
    Some(Bloom {
        months: months.to_vec(),
        colour,
        name: None,
    })
}

const OAK: &str = "flora:quercus-robur";

/// A cottage border (deterministic by its seed), a single oak, a custom species and two lamps.
fn garden() -> Session {
    let mut s = Session::new(Project::default());
    let run = |s: &mut Session, name: &str, p: Value| s.run(name, p).unwrap();
    run(
        &mut s,
        "planting.add_border",
        json!({"name": "Cottage border", "outline": [[0, 0], [10, 0], [10, 2], [0, 2]], "style": "cottage", "facing": [0, 1], "seed": 7}),
    );
    run(
        &mut s,
        "planting.add_single",
        json!({"name": "Oak", "asset": OAK, "position": [5, 0, 8]}),
    );
    run(
        &mut s,
        "species.add",
        json!({"name": "Red thing", "base": "geranium-rozanne", "params": {"height_m": 0.4}}),
    );
    run(
        &mut s,
        "planting.add_single",
        json!({"name": "Custom", "asset": "sp1", "position": [6, 0, 8]}),
    );
    run(
        &mut s,
        "planting.add_spline",
        json!({"name": "Path lights", "asset_palette": [{"asset": "vc-path-light", "weight": 1}], "points": [[0, 5], [6, 5]], "spacing": {"count": 2}, "seed": 1}),
    );
    s
}

fn calendar(s: &Session, filter: &Filter) -> Value {
    visioncraft_core::bloom::calendar(s.project(), filter, None, &flowering).unwrap()
}

/// What the quantities count as placed, plant or not: (asset, plants), per planting.
fn placed(s: &Session) -> Vec<(String, u64)> {
    visioncraft_core::quantities::quantities(s.project(), &Filter::default()).unwrap()["plantings"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["assets"].as_array().unwrap().iter())
        .map(|a| {
            (
                a["asset"].as_str().unwrap().to_string(),
                a["count"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// The plants the quantities count (everything placed but the lamps).
fn counted_plants(s: &Session) -> u64 {
    placed(s)
        .iter()
        .filter(|(a, _)| a != "vc-path-light")
        .map(|(_, n)| n)
        .sum()
}

fn months_of(v: &Value) -> Vec<u64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_u64().unwrap())
        .collect()
}

fn ids(v: &Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| e["species"].as_str().unwrap())
        .collect()
}

fn plants(v: &Value) -> u64 {
    v["plants"].as_u64().unwrap()
}

fn shift(m: u8) -> u8 {
    (m + 5) % 12 + 1
}

/// A species' row in `out`.
fn row<'a>(out: &'a Value, id: &str) -> &'a Value {
    out["species"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["species"] == id)
        .unwrap()
}

/// The layout, whichever hemisphere `out` is for: 12 months in order, each month's `plants`
/// the sum of its `flowering` sorted by plants then id, every species in exactly the months
/// of its data and in the species list with the same plants, the gaps and the unknown right.
fn check_layout(out: &Value, north: bool, total_plants: u64) {
    let months = out["months"].as_array().unwrap();
    assert_eq!(months.len(), 12);
    assert_eq!(out["hemisphere"], if north { "north" } else { "south" });
    let mut in_flower: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for (i, m) in months.iter().enumerate() {
        assert_eq!(m["month"], (i + 1) as u64);
        let flowering = m["flowering"].as_array().unwrap();
        assert_eq!(m["species"], flowering.len() as u64);
        assert_eq!(plants(m), flowering.iter().map(plants).sum::<u64>());
        let rows: Vec<(u64, &str)> = flowering
            .iter()
            .map(|f| (plants(f), f["species"].as_str().unwrap()))
            .collect();
        let mut sorted = rows.clone();
        sorted.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        assert_eq!(rows, sorted, "month {}", m["month"]);
        for f in flowering {
            assert!(plants(f) > 0, "{f}");
            in_flower
                .entry(f["species"].as_str().unwrap())
                .or_default()
                .push(m["month"].as_u64().unwrap());
        }
    }
    let species = out["species"].as_array().unwrap();
    let mut listed = 0;
    for sp in species {
        let id = sp["species"].as_str().unwrap();
        let data = flowering(id).unwrap();
        let mut want: Vec<u64> = data
            .months
            .iter()
            .map(|&m| if north { m } else { shift(m) })
            .map(u64::from)
            .collect();
        want.sort_unstable();
        let mut got = in_flower.remove(id).unwrap_or_default();
        got.sort_unstable();
        assert_eq!(got, want, "{id} flowers in exactly the months of its data");
        assert_eq!(months_of(&sp["bloom_months"]), want, "{id}");
        assert!(plants(sp) > 0, "{id}");
        listed += plants(sp);
        // The colour is the data's, or the key is left out.
        match data.colour {
            Some(c) => assert_eq!(sp["colour"], json!(c), "{id}"),
            None => assert!(sp.get("colour").is_none(), "{id}"),
        }
        for m in months {
            for f in m["flowering"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|f| f["species"] == id)
            {
                assert_eq!(f["plants"], sp["plants"], "{id}");
                assert_eq!(f.get("colour"), sp.get("colour"), "{id}");
                assert!(f.get("name").is_none(), "this table gives no name");
            }
        }
    }
    assert!(
        in_flower.is_empty(),
        "in a month but not in the species list"
    );
    assert_eq!(
        ids(&out["species"]),
        {
            let mut v = ids(&out["species"]);
            v.sort_unstable();
            v
        },
        "species by id"
    );
    // Gaps: exactly the months with nothing in flower.
    let gaps: Vec<u64> = months
        .iter()
        .filter(|m| m["flowering"].as_array().unwrap().is_empty())
        .map(|m| m["month"].as_u64().unwrap())
        .collect();
    assert_eq!(months_of(&out["gaps"]), gaps);
    // The unknown: species without bloom data, by id, with their plants; nothing else.
    let unknown = out["unknown"].as_array().unwrap();
    assert_eq!(ids(&out["unknown"]), {
        let mut v = ids(&out["unknown"]);
        v.sort_unstable();
        v
    });
    for u in unknown {
        let id = u["species"].as_str().unwrap();
        assert!(
            flowering(id).is_none_or(|b| b.months.is_empty()),
            "{id} has bloom data"
        );
        assert!(plants(u) > 0 && u.as_object().unwrap().len() == 2, "{u}");
        assert!(species.iter().all(|s| s["species"] != id));
    }
    // Every plant counted is in flower somewhere or unknown.
    assert_eq!(
        listed + unknown.iter().map(plants).sum::<u64>(),
        total_plants
    );
}

#[test]
fn twelve_months_and_every_species_in_its_own_months() {
    let s = garden();
    let out = calendar(&s, &Filter::default());
    check_layout(&out, true, counted_plants(&s));
    // The cottage border flowers from May to October and the custom species in March and
    // April: winter is the gap.
    assert_eq!(months_of(&out["gaps"]), [1, 2, 11, 12]);
    let june = &out["months"][5];
    assert!(june["species"].as_u64().unwrap() >= 5, "{june}");
    assert!(plants(june) > 20, "{june}");
    // The oak has no bloom data here, the lady's mantle none in its table; the custom species
    // flowers in March and April.
    assert_eq!(ids(&out["unknown"]), ["flora:alchemilla-mollis", OAK]);
    assert_eq!(out["unknown"][1]["plants"], 1);
    let sp1 = out["species"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["species"] == "sp1")
        .unwrap();
    assert_eq!(months_of(&sp1["bloom_months"]), [3, 4]);
    assert_eq!(plants(sp1), 1);
    assert_eq!(out["months"][2]["flowering"][0]["species"], "sp1");
    // No lamp anywhere.
    assert!(!out.to_string().contains("vc-path-light"));
}

#[test]
fn south_of_the_equator_every_species_flowers_six_months_on() {
    let mut s = garden();
    let north = calendar(&s, &Filter::default());
    s.run("site.set", json!({"latitude": -33.9})).unwrap();
    let south = calendar(&s, &Filter::default());
    check_layout(&south, false, counted_plants(&s));
    assert_eq!(ids(&north["species"]).len(), ids(&south["species"]).len());
    for sp in north["species"].as_array().unwrap() {
        let id = sp["species"].as_str().unwrap();
        // Months that cross the new year come back in calendar order (December before January).
        let mut want: Vec<u64> = months_of(&sp["bloom_months"])
            .into_iter()
            .map(|m| u64::from(shift(m as u8)))
            .collect();
        want.sort_unstable();
        assert_eq!(months_of(&row(&south, id)["bloom_months"]), want, "{id}");
        assert_eq!(row(&south, id)["plants"], sp["plants"], "{id}");
    }
    assert_eq!(south["unknown"], north["unknown"]);
    // What flowers in June and July flowers in December and January.
    assert_eq!(
        ids(&north["months"][5]["flowering"]),
        ids(&south["months"][11]["flowering"])
    );
    assert_eq!(
        ids(&north["months"][6]["flowering"]),
        ids(&south["months"][0]["flowering"])
    );
    // March and April become September and October.
    assert_eq!(row(&south, "sp1")["bloom_months"], json!([9, 10]));
    assert_eq!(months_of(&south["gaps"]), [5, 6, 7, 8]);
}

#[test]
fn one_planting_or_one_layer_and_what_is_unknown() {
    let mut s = garden();
    let all = calendar(&s, &Filter::default());
    // Only the border: its species, no oak and no custom species.
    let border = s.project().generators[0].id().to_string();
    let only_border = Filter {
        ids: Some(std::slice::from_ref(&border)),
        layer: None,
    };
    let one = calendar(&s, &only_border);
    assert!(ids(&one["unknown"]) == ["flora:alchemilla-mollis"]);
    assert_eq!(ids(&one["species"]).len(), ids(&all["species"]).len() - 1);
    for (a, b) in all["species"]
        .as_array()
        .unwrap()
        .iter()
        .zip(one["species"].as_array().unwrap())
    {
        assert_eq!(
            (a["species"].clone(), a["plants"].clone()),
            (b["species"].clone(), b["plants"].clone())
        );
    }
    // The same through the command, and what the bare core answers: no bloom data, so every
    // species is unknown and every month a gap.
    let bare = s.run("query.bloom_calendar", json!({})).unwrap();
    assert_eq!(bare["hemisphere"], "north");
    assert_eq!(bare["months"].as_array().unwrap().len(), 12);
    assert!(bare["species"].as_array().unwrap().is_empty());
    assert_eq!(months_of(&bare["gaps"]), (1..=12).collect::<Vec<u64>>());
    let mut every: Vec<String> = placed(&s)
        .into_iter()
        .map(|(a, _)| a)
        .filter(|a| a != "vc-path-light")
        .collect();
    every.sort_unstable();
    let every: Vec<&str> = every.iter().map(String::as_str).collect();
    assert_eq!(ids(&bare["unknown"]), every);
    let total: u64 = bare["unknown"].as_array().unwrap().iter().map(plants).sum();
    assert_eq!(total, counted_plants(&s));
    assert!(
        bare["months"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["plants"] == 0)
    );
    assert!(!bare.to_string().contains("vc-path-light"));
    // A query leaves the history alone. Through the command with one planting named: the
    // core's own answer for the border, every species of it unknown and nothing else (no oak,
    // no custom species).
    let (revision, could_undo) = (s.revision(), s.can_undo());
    let cmd = s
        .run("query.bloom_calendar", json!({"planting": border}))
        .unwrap();
    assert!(cmd["species"].as_array().unwrap().is_empty());
    assert_eq!((s.revision(), s.can_undo()), (revision, could_undo));
    let mut want: Vec<&str> = ids(&one["species"]);
    want.extend(ids(&one["unknown"]));
    want.sort_unstable();
    assert_eq!(ids(&cmd["unknown"]), want);
    // A layer of its own with one lavender: only it there, with the rest by default.
    let far = s.run("layer.add", json!({"name": "Far bed"})).unwrap();
    let far = far["id"].as_str().unwrap().to_string();
    s.run(
        "planting.add_single",
        json!({"name": "Lavender", "asset": "flora:lavandula-angustifolia", "position": [20, 0, 20], "layer": far}),
    )
    .unwrap();
    let on_far = calendar(
        &s,
        &Filter {
            ids: None,
            layer: Some(far.as_str()),
        },
    );
    assert_eq!(ids(&on_far["species"]), ["flora:lavandula-angustifolia"]);
    assert!(on_far["unknown"].as_array().unwrap().is_empty());
    assert_eq!(months_of(&on_far["gaps"]), [1, 2, 3, 4, 5, 9, 10, 11, 12]);
    let all = calendar(&s, &Filter::default());
    check_layout(&all, true, counted_plants(&s));
    // One species more than the border alone (the custom one): the lavender on the far layer
    // is one of the border's species and joins its row with more plants.
    assert_eq!(ids(&all["species"]).len(), ids(&one["species"]).len() + 1);
    let lavender = |out: &Value| row(out, "flora:lavandula-angustifolia")["plants"].clone();
    assert_eq!(
        lavender(&all).as_u64().unwrap(),
        lavender(&one).as_u64().unwrap() + 1
    );
    // A planting and a layer together: the planting, when it is on that layer.
    let both = calendar(
        &s,
        &Filter {
            ids: Some(std::slice::from_ref(&border)),
            layer: Some("planting"),
        },
    );
    assert_eq!(both["species"], one["species"]);
    let none = calendar(
        &s,
        &Filter {
            ids: Some(std::slice::from_ref(&border)),
            layer: Some(far.as_str()),
        },
    );
    assert!(none["species"].as_array().unwrap().is_empty());
    assert_eq!(months_of(&none["gaps"]), (1..=12).collect::<Vec<u64>>());
    // Unknown ids are errors that name them.
    let err = s
        .run("query.bloom_calendar", json!({"planting": "g9"}))
        .unwrap_err();
    assert_eq!(err.to_string(), "no planting `g9`");
    let err = s
        .run("query.bloom_calendar", json!({"layer": "nope"}))
        .unwrap_err();
    assert_eq!(err.to_string(), "no layer `nope`");
    assert!(s.run("query.bloom_calendar", json!({"month": 6})).is_err());
}

#[test]
fn a_host_counts_the_plants_as_it_draws_them() {
    let s = garden();
    let plan = calendar(&s, &Filter::default());
    // A host that draws one plant less of the border.
    let project = s.project();
    let border = project.generators[0].id();
    let scene = visioncraft_core::quantities::Scene {
        models: &|_| None,
        placements: &|g| {
            let mut p = evaluate(g, &|_, _| None).placements;
            if g.id() == border && !p.is_empty() {
                p.pop();
            }
            (p, false)
        },
    };
    let drawn =
        visioncraft_core::bloom::calendar(project, &Filter::default(), Some(&scene), &flowering)
            .unwrap();
    let total = |out: &Value| {
        out["species"]
            .as_array()
            .unwrap()
            .iter()
            .chain(out["unknown"].as_array().unwrap())
            .map(plants)
            .sum::<u64>()
    };
    assert_eq!(total(&plan), total(&drawn) + 1);
    assert_eq!(ids(&plan["species"]), ids(&drawn["species"]));
}

#[test]
fn names_come_from_the_host_and_months_out_of_range_are_dropped() {
    let s = garden();
    let named = |asset: &str| {
        flowering(asset).map(|b| Bloom {
            name: Some(format!("Name of {asset}")),
            ..b
        })
    };
    let out =
        visioncraft_core::bloom::calendar(s.project(), &Filter::default(), None, &named).unwrap();
    for sp in out["species"].as_array().unwrap() {
        let id = sp["species"].as_str().unwrap();
        assert_eq!(sp["name"], format!("Name of {id}"));
    }
    for m in out["months"].as_array().unwrap() {
        for f in m["flowering"].as_array().unwrap() {
            let id = f["species"].as_str().unwrap();
            assert_eq!(f["name"], format!("Name of {id}"));
            let sp = out["species"]
                .as_array()
                .unwrap()
                .iter()
                .find(|sp| sp["species"] == id)
                .unwrap();
            for k in ["plants", "colour", "name"] {
                assert_eq!(f.get(k), sp.get(k), "{id}: {k}");
            }
        }
    }
    // Months out of 1..12 in the data are ignored, not an error.
    let odd = |asset: &str| {
        flowering(asset).map(|b| Bloom {
            months: b.months.iter().copied().chain([0, 13]).collect(),
            ..b
        })
    };
    let odd_out =
        visioncraft_core::bloom::calendar(s.project(), &Filter::default(), None, &odd).unwrap();
    let plain = calendar(&s, &Filter::default());
    assert_eq!(odd_out["months"], plain["months"]);
    assert_eq!(odd_out["species"], plain["species"]);
}
