//! Denser borders: `planting.add_border` plants a layered, drifted perennial border that
//! closes up as it grows.

use serde_json::{Value, json};
use visioncraft_core::growth::growth_of;
use visioncraft_core::planting::{Generator, Placement, evaluate};
use visioncraft_core::{Project, Session};

/// The garden example's border S4 (x 0.8..3, z 1..29), its hedge on the west side.
const OUTLINE: [[f32; 2]; 4] = [[0.8, 1.0], [3.0, 1.0], [3.0, 29.0], [0.8, 29.0]];

fn garden_border(style: &str, extra: Value) -> (Session, String) {
    let mut s = Session::new(Project::default());
    let mut p =
        json!({"name": "Border plants", "outline": OUTLINE, "style": style, "facing": [1, 0]});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    let r = s.run("planting.add_border", p).unwrap();
    let id = r["id"].as_str().unwrap().to_string();
    (s, id)
}

fn plants(s: &Session, id: &str) -> Vec<Placement> {
    let g = s
        .project()
        .generators
        .iter()
        .find(|g| g.id() == id)
        .unwrap();
    evaluate(g, &|_, _| None).placements
}

/// Share of the border (inset 0.3 m) under a crown `years` after planting.
fn coverage(ps: &[Placement], years: f32) -> f32 {
    let crowns: Vec<([f32; 2], f32)> = ps
        .iter()
        .map(|p| {
            let g = growth_of(&p.asset, None);
            let w = g.mature_width_m * g.size(g.planted_age_years + years).width * p.scale;
            ([p.position[0], p.position[2]], w / 2.0)
        })
        .collect();
    let (mut covered, mut total) = (0, 0);
    let mut x = 1.1;
    while x < 2.7 {
        let mut z = 1.3;
        while z < 28.7 {
            total += 1;
            if crowns.iter().any(|(c, r)| (c[0] - x).hypot(c[1] - z) <= *r) {
                covered += 1;
            }
            z += 0.1;
        }
        x += 0.1;
    }
    covered as f32 / total as f32
}

#[test]
fn a_border_is_made_of_catalogue_species_and_undone() {
    let (mut s, id) = garden_border("cottage", json!({}));
    let ps = plants(&s, &id);
    assert!(ps.len() > 150, "{}", ps.len());
    for p in &ps {
        let sp = p.asset.strip_prefix("flora:").unwrap();
        assert!(
            visioncraft_core::growth::GROWTH.iter().any(|r| r.0 == sp),
            "{sp}"
        );
        assert!((0.8..=3.0).contains(&p.position[0]) && (1.0..=29.0).contains(&p.position[2]));
    }
    assert!(s.undo());
    assert!(s.project().generators.is_empty());
    // Unknown styles and species are refused.
    let mut s = Session::new(Project::default());
    let bad = |s: &mut Session, extra: Value| {
        let mut p = json!({"outline": OUTLINE, "style": "cottage"});
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        s.run("planting.add_border", p).is_err()
    };
    assert!(bad(&mut s, json!({"style": "jungle"})));
    assert!(bad(&mut s, json!({"species": ["flora:no-such-plant"]})));
    assert!(bad(&mut s, json!({"height_range": [2.0, 1.0]})));
    assert!(s.project().generators.is_empty());
}

#[test]
fn heights_rise_from_the_front_to_the_back() {
    for style in ["cottage", "prairie", "shade", "dry"] {
        let (s, id) = garden_border(style, json!({}));
        let ps = plants(&s, &id);
        // Facing +x: the front is at x = 3, the back at 0.8.
        let mean = |lo: f32, hi: f32| {
            let h: Vec<f32> = ps
                .iter()
                .filter(|p| (lo..hi).contains(&p.position[0]))
                .map(|p| growth_of(&p.asset, None).mature_height_m * p.scale)
                .collect();
            assert!(!h.is_empty());
            h.iter().sum::<f32>() / h.len() as f32
        };
        let (front, middle, back) = (mean(2.27, 3.1), mean(1.53, 2.27), mean(0.7, 1.53));
        assert!(
            front + 0.05 < middle && middle + 0.05 < back,
            "{style}: front {front}, middle {middle}, back {back}"
        );
    }
}

#[test]
fn the_front_follows_facing_front_edges_and_the_nearest_hedge() {
    let (s, id) = garden_border("cottage", json!({"facing": [-1, 0]}));
    let ps = plants(&s, &id);
    let tall = |lo: f32, hi: f32| {
        let h: Vec<f32> = ps
            .iter()
            .filter(|p| (lo..hi).contains(&p.position[0]))
            .map(|p| growth_of(&p.asset, None).mature_height_m)
            .collect();
        h.iter().sum::<f32>() / h.len() as f32
    };
    assert!(tall(0.7, 1.5) < tall(2.3, 3.1));
    // Edge 1 of the outline is the east one (x = 3): the same front as facing +x.
    let (_, a) = garden_border("cottage", json!({"front": [1]}));
    let (s2, b) = garden_border("cottage", json!({}));
    let (s3, _) = garden_border("cottage", json!({"front": [1]}));
    assert_eq!(plants(&s3, &a), plants(&s2, &b));
    // A hedge on the west side: the front is derived as east, away from it.
    let mut s = Session::new(Project::default());
    s.run(
        "planting.add_spline",
        json!({"name": "Hedge", "asset_palette": [{"asset": "flora:carpinus-hedge", "weight": 1}],
               "points": [[0.2, 0.2], [0.2, 29.8]], "spacing": {"every": 0.8}}),
    )
    .unwrap();
    let r = s
        .run(
            "planting.add_border",
            json!({"outline": OUTLINE, "style": "cottage"}),
        )
        .unwrap();
    assert_eq!(r["facing"], json!([1.0, 0.0]), "{r}");
}

#[test]
fn a_border_closes_up_with_the_years() {
    for style in ["cottage", "prairie", "shade", "dry"] {
        let (s, id) = garden_border(style, json!({}));
        let ps = plants(&s, &id);
        let (y0, y1, y3) = (coverage(&ps, 0.0), coverage(&ps, 1.0), coverage(&ps, 3.0));
        eprintln!(
            "{style}: {} plants, cover {y0:.2} {y1:.2} {y3:.2}",
            ps.len()
        );
        assert!(y0 <= 0.6, "{style}: year 0 {y0}");
        assert!(y1 <= 0.85, "{style}: year 1 {y1}");
        assert!(y3 >= 0.95, "{style}: year 3 {y3}");
    }
}

#[test]
fn plants_stand_in_drifts() {
    for style in ["cottage", "prairie", "shade", "dry"] {
        let (s, id) = garden_border(style, json!({}));
        let ps = plants(&s, &id);
        let mut counts = std::collections::HashMap::new();
        for p in &ps {
            *counts.entry(p.asset.clone()).or_insert(0usize) += 1;
        }
        let n = ps.len() as f32;
        // The chance that two plants picked at random are the same species.
        let random: f32 = counts.values().map(|&c| (c as f32 / n).powi(2)).sum();
        let same = ps
            .iter()
            .filter(|p| {
                let near = ps
                    .iter()
                    .filter(|q| !std::ptr::eq(*p, *q))
                    .min_by(|a, b| d(p, a).total_cmp(&d(p, b)))
                    .unwrap();
                near.asset == p.asset
            })
            .count() as f32
            / n;
        eprintln!("{style}: nearest neighbour the same {same:.2}, at random {random:.2}");
        assert!(same > random + 0.2, "{style}: {same} vs {random}");
    }
}

fn d(a: &Placement, b: &Placement) -> f32 {
    (a.position[0] - b.position[0]).hypot(a.position[2] - b.position[2])
}

#[test]
fn a_border_is_deterministic_by_seed() {
    let (s1, a) = garden_border("prairie", json!({"seed": 5}));
    let (s2, b) = garden_border("prairie", json!({"seed": 5}));
    let (s3, c) = garden_border("prairie", json!({"seed": 6}));
    assert_eq!(plants(&s1, &a), plants(&s2, &b));
    assert_ne!(plants(&s1, &a), plants(&s3, &c));
}

#[test]
fn a_border_survives_a_file_and_updates() {
    let (mut s, id) = garden_border(
        "dry",
        json!({"species": ["lavandula-angustifolia", "festuca-glauca"]}),
    );
    let n = plants(&s, &id).len();
    let back = Project::from_json_bytes(&s.project().to_json_bytes()).unwrap();
    assert_eq!(back.generators, s.project().generators);
    assert!(
        plants(&s, &id)
            .iter()
            .all(|p| p.asset == "flora:lavandula-angustifolia" || p.asset == "flora:festuca-glauca")
    );
    // A new seed through planting.update.
    s.run("planting.update", json!({"id": id, "seed": 99}))
        .unwrap();
    assert_ne!(plants(&s, &id).len() + 1, 0);
    let Generator::AreaScatter(a) = &s.project().generators[0] else {
        panic!()
    };
    assert_eq!(a.seed, 99);
    assert!(n > 100);
}
