use std::time::Instant;

use visioncraft_flora::{
    ATLAS_SIZE, BARK_SIZE, BudgetClass, Category, ClipShape, Form, Lod, Material, PlantMesh,
    Season, Species, generate, generate_in_season, species, textures,
};

fn height(m: &PlantMesh) -> f32 {
    m.bounds().1[1]
}

/// Catalogue width: crown diameter, or the thickness (Z) of a hedge segment.
fn width(s: &Species, m: &PlantMesh) -> f32 {
    let (lo, hi) = m.bounds();
    if s.category == Category::Hedge {
        hi[2] - lo[2]
    } else {
        (hi[0] - lo[0]).max(hi[2] - lo[2])
    }
}

fn expected_budget(s: &Species) -> BudgetClass {
    match s.category {
        Category::Tree if s.mature_height_m >= 12.0 => BudgetClass::LargeTree,
        Category::Tree | Category::Conifer | Category::Palm => BudgetClass::SmallTree,
        Category::Shrub | Category::Hedge | Category::Climber => BudgetClass::Shrub,
        Category::GroundCover => BudgetClass::Tuft,
        Category::Grass if s.id == "lawn-tuft" => BudgetClass::Tuft,
        Category::Perennial | Category::Fern | Category::Grass => BudgetClass::Herb,
    }
}

#[test]
fn library_is_complete() {
    let lib = species();
    assert!(lib.len() >= 48, "{} species", lib.len());
    let mut ids: Vec<_> = lib.iter().map(|s| s.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), lib.len(), "duplicate ids");
    for s in lib {
        assert!(!s.common_name.is_empty() && !s.botanical_name.is_empty());
        assert_eq!(s.params.budget, expected_budget(s), "{}", s.id);
        assert!(s.bloom_months.iter().all(|m| (1..=12).contains(m)));
    }
}

#[test]
fn meshes_are_valid_and_within_budget() {
    for s in species() {
        for lod in Lod::ALL {
            let m = generate(&s.params, 7, lod);
            let tag = format!("{} {lod:?}", s.id);
            let budget = s.params.budget.triangles(lod);
            assert!(
                m.triangle_count() <= budget,
                "{tag}: {} > {budget}",
                m.triangle_count()
            );
            assert!(m.triangle_count() > 0, "{tag}: empty");
            let n = m.positions.len();
            assert_eq!(m.normals.len(), n);
            assert_eq!(m.uvs.len(), n);
            assert_eq!(m.wind.len(), n);
            assert!(
                m.indices.iter().all(|&i| (i as usize) < n),
                "{tag}: index out of range"
            );
            assert_eq!(m.indices.len() % 3, 0);
            let covered: usize = m.parts.iter().map(|p| p.index_range.len()).sum();
            assert_eq!(
                covered,
                m.indices.len(),
                "{tag}: parts do not cover indices"
            );
            for p in &m.parts {
                assert_eq!(p.index_range.len() % 3, 0);
            }
            for k in 0..n {
                assert!(
                    m.positions[k].iter().all(|v| v.is_finite()),
                    "{tag}: position"
                );
                let nn = m.normals[k];
                assert!(nn.iter().all(|v| v.is_finite()), "{tag}: normal");
                let l = (nn[0] * nn[0] + nn[1] * nn[1] + nn[2] * nn[2]).sqrt();
                assert!((l - 1.0).abs() < 1e-3, "{tag}: normal length {l}");
                assert!(
                    m.uvs[k].iter().all(|v| (0.0..=1.0).contains(v)),
                    "{tag}: uv {:?}",
                    m.uvs[k]
                );
                assert!(
                    (0.0..=1.0).contains(&m.wind[k][0]),
                    "{tag}: wind {:?}",
                    m.wind[k]
                );
                assert!(m.wind[k][1].is_finite());
            }
            // Wind is zero at the base: every bark vertex at the ground, or the lowest vertices.
            let ground: Vec<usize> = (0..n).filter(|&k| m.positions[k][1] < 0.005).collect();
            if !ground.is_empty() {
                let min_w = ground
                    .iter()
                    .map(|&k| m.wind[k][0])
                    .fold(f32::MAX, f32::min);
                assert_eq!(min_w, 0.0, "{tag}: no zero wind weight at the base");
            }
            if matches!(s.params.form, Form::Branching(_)) {
                let bark_ground = m
                    .parts
                    .iter()
                    .filter(|p| p.material == Material::Bark)
                    .flat_map(|p| {
                        m.indices[p.index_range.start as usize..p.index_range.end as usize].iter()
                    });
                for &i in bark_ground {
                    if m.positions[i as usize][1] < 0.005 {
                        assert_eq!(m.wind[i as usize][0], 0.0, "{tag}: trunk base sways");
                    }
                }
            }
        }
    }
}

#[test]
fn sizes_match_catalogue_and_lods_agree() {
    let mut bad = Vec::new();
    for s in species() {
        let m0 = generate(&s.params, 7, Lod::Lod0);
        let (h, w) = (height(&m0), width(s, &m0));
        if m0.bounds().0[1] < -0.05 {
            bad.push(format!("{}: below ground {}", s.id, m0.bounds().0[1]));
        }
        if (h / s.mature_height_m - 1.0).abs() > 0.25 {
            bad.push(format!("{}: height {h:.2} vs {}", s.id, s.mature_height_m));
        }
        if (w / s.mature_width_m - 1.0).abs() > 0.25 {
            bad.push(format!("{}: width {w:.2} vs {}", s.id, s.mature_width_m));
        }
        let m2 = generate(&s.params, 7, Lod::Lod2);
        let (h2, w2) = (height(&m2), width(s, &m2));
        if (h2 / h - 1.0).abs() > 0.15 {
            bad.push(format!("{}: LOD2 height {h2:.2} vs {h:.2}", s.id));
        }
        if (w2 / w - 1.0).abs() > 0.15 {
            bad.push(format!("{}: LOD2 width {w2:.2} vs {w:.2}", s.id));
        }
        let (lo0, hi0) = m0.bounds();
        let (lo2, hi2) = m2.bounds();
        for k in [0, 2] {
            let ext = (hi0[k] - lo0[k]).max(0.1);
            if (lo2[k] - lo0[k]).abs() > 0.15 * ext || (hi2[k] - hi0[k]).abs() > 0.15 * ext {
                bad.push(format!("{}: LOD2 box shifted on axis {k}", s.id));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn deterministic_and_seed_dependent() {
    for s in species() {
        for lod in Lod::ALL {
            assert_eq!(
                generate(&s.params, 42, lod),
                generate(&s.params, 42, lod),
                "{} {lod:?}",
                s.id
            );
        }
        let (a, b) = (
            generate(&s.params, 1, Lod::Lod0),
            generate(&s.params, 2, Lod::Lod0),
        );
        assert_ne!(
            a.positions, b.positions,
            "{}: seeds give the same plant",
            s.id
        );
    }
}

fn fnv(m: &PlantMesh) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u32| {
        for byte in b.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    m.positions
        .iter()
        .flatten()
        .chain(m.normals.iter().flatten())
        .chain(m.uvs.iter().flatten())
        .chain(m.wind.iter().flatten())
        .for_each(|v| eat(v.to_bits()));
    m.indices.iter().for_each(|&i| eat(i));
    h
}

/// Bit-exact output across platforms and optimisation levels: these hashes were produced on
/// x86_64 Linux; the same test on another IEEE-754 target (aarch64, wasm32) must match. Any
/// intended change to a generator or the library changes them: update the constants then.
#[test]
fn golden_hashes() {
    let lib = species();
    for (id, lod, want) in [
        ("quercus-robur", Lod::Lod0, 0xacf6_d35f_963a_b551_u64),
        ("hosta", Lod::Lod1, 0xa334_e697_a230_f815),
        ("carpinus-hedge", Lod::Lod2, 0x8873_77e0_ca08_cc73),
    ] {
        let s = lib.iter().find(|s| s.id == id).unwrap();
        let h = fnv(&generate(&s.params, 1234, lod));
        assert_eq!(h, want, "{id} {lod:?}: got {h:#018x}");
    }
}

#[test]
fn hedge_segments_tile() {
    for s in species().iter().filter(|s| s.category == Category::Hedge) {
        let Form::Clipped(c) = &s.params.form else {
            panic!("{} is not clipped", s.id)
        };
        let ClipShape::HedgeSegment { length_m } = c.shape else {
            panic!()
        };
        for lod in Lod::ALL {
            let m = generate(&s.params, 3, lod);
            let end = |x: f32| {
                let mut v: Vec<(u32, u32, u32)> = m
                    .positions
                    .iter()
                    .zip(&m.wind)
                    .filter(|(p, _)| (p[0] - x).abs() < 1e-5)
                    .map(|(p, w)| (p[1].to_bits(), p[2].to_bits(), w[0].to_bits()))
                    .collect();
                v.sort();
                v.dedup();
                v
            };
            let (a, b) = (end(-length_m * 0.5), end(length_m * 0.5));
            assert!(a.len() >= 4, "{} {lod:?}: no end face", s.id);
            assert_eq!(a, b, "{} {lod:?}: end cross-sections differ", s.id);
            // And the overall boundary extents agree.
            let ext = |v: &[(u32, u32, u32)]| {
                let ys: Vec<f32> = v.iter().map(|t| f32::from_bits(t.0)).collect();
                let zs: Vec<f32> = v.iter().map(|t| f32::from_bits(t.1)).collect();
                (
                    ys.iter().cloned().fold(f32::MAX, f32::min),
                    ys.iter().cloned().fold(f32::MIN, f32::max),
                    zs.iter().cloned().fold(f32::MAX, f32::min),
                    zs.iter().cloned().fold(f32::MIN, f32::max),
                )
            };
            assert_eq!(ext(&a), ext(&b));
        }
    }
}

#[test]
fn winter_strips_deciduous_trees_only() {
    let lib = species();
    let foliage = |id: &str, month: u8| {
        let s = lib.iter().find(|s| s.id == id).unwrap();
        generate_in_season(&s.params, 7, Lod::Lod0, Season { month })
            .triangles_of(Material::Foliage)
    };
    for id in [
        "quercus-robur",
        "fagus-sylvatica",
        "betula-pendula",
        "acer-palmatum",
    ] {
        let (summer, winter) = (foliage(id, 7), foliage(id, 1));
        assert!(summer > 100, "{id}");
        assert!(
            winter * 10 < summer,
            "{id}: winter {winter} vs summer {summer}"
        );
    }
    for id in [
        "taxus-baccata",
        "prunus-laurocerasus",
        "picea-abies",
        "buxus-ball",
        "olea-europaea",
    ] {
        assert_eq!(
            foliage(id, 1),
            foliage(id, 7),
            "{id}: evergreen lost leaves"
        );
    }
    // Spring flush and autumn colour tint the foliage.
    let oak = &lib.iter().find(|s| s.id == "quercus-robur").unwrap().params;
    assert_eq!(
        generate_in_season(oak, 7, Lod::Lod1, Season { month: 7 }).foliage_tint,
        [1.0; 3]
    );
    assert_ne!(
        generate_in_season(oak, 7, Lod::Lod1, Season { month: 10 }).foliage_tint,
        [1.0; 3]
    );
    // Flowers only in bloom months.
    let cherry = &lib
        .iter()
        .find(|s| s.id == "prunus-serrulata")
        .unwrap()
        .params;
    assert!(
        generate_in_season(cherry, 7, Lod::Lod0, Season { month: 4 })
            .triangles_of(Material::Flower)
            > 0
    );
    assert_eq!(
        generate_in_season(cherry, 7, Lod::Lod0, Season { month: 8 })
            .triangles_of(Material::Flower),
        0
    );
}

#[test]
fn textures_have_cutouts_and_sizes() {
    for s in species() {
        let t = textures(&s.params);
        assert_eq!((t.atlas.width, t.atlas.height), (ATLAS_SIZE, ATLAS_SIZE));
        assert_eq!(t.atlas.rgba.len(), (ATLAS_SIZE * ATLAS_SIZE * 4) as usize);
        const { assert!(ATLAS_SIZE <= 1024 && BARK_SIZE <= 256) };
        for img in [&t.bark, &t.bark_normal] {
            assert_eq!((img.width, img.height), (BARK_SIZE, BARK_SIZE));
            assert!(img.rgba.chunks(4).all(|p| p[3] == 255));
        }
        let alpha = t.atlas.rgba.chunks(4).map(|p| p[3]);
        let opaque = alpha.clone().filter(|&a| a == 255).count();
        let clear = alpha.filter(|&a| a == 0).count();
        assert!(
            opaque > 10_000 && clear > 10_000,
            "{}: opaque {opaque}, clear {clear}",
            s.id
        );
        // Colour bleeding: transparent texels carry leaf colour, not black.
        let dark_clear = t
            .atlas
            .rgba
            .chunks(4)
            .filter(|p| p[3] == 0 && p[0] as u32 + p[1] as u32 + p[2] as u32 == 0)
            .count();
        assert!(
            dark_clear < clear / 100,
            "{}: {dark_clear} black transparent texels",
            s.id
        );
    }
}

#[test]
fn young_plants_are_smaller() {
    let s = species()
        .iter()
        .find(|s| s.id == "fagus-sylvatica")
        .unwrap();
    let mut young = s.params.clone();
    young.age = 0.3;
    let (m, y) = (
        generate(&s.params, 7, Lod::Lod0),
        generate(&young, 7, Lod::Lod0),
    );
    assert!(height(&y) < 0.45 * height(&m));
    assert!(y.triangle_count() <= m.triangle_count());
}

#[test]
fn generation_time_report() {
    // Non-failing measurement; meaningful only in an optimised build.
    for s in species()
        .iter()
        .filter(|s| s.params.budget == BudgetClass::LargeTree)
    {
        let t = Instant::now();
        let m = generate(&s.params, 7, Lod::Lod0);
        println!(
            "{:<26} LOD0 {:>6} tris in {:>7.2} ms",
            s.id,
            m.triangle_count(),
            t.elapsed().as_secs_f64() * 1e3
        );
    }
}
