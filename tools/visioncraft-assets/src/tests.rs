use crate::{
    glb::write_glb,
    library,
    scene::{Mesh, Model, pack_orm, read_glb, read_gltf_variant},
};
use image::{GrayImage, Luma};
use std::path::PathBuf;
use visioncraft_core::{
    Asset, Catalog,
    catalog::{Category, Lod, Origin},
    validate_open_licences,
};

fn sample() -> Model {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/sample_model_outdoor_table_chair_set_01/1k/outdoor_table_chair_set_01_1k.gltf"
    );
    read_gltf_variant(&PathBuf::from(p), 0).unwrap()
}

#[test]
fn glb_round_trips_through_the_gltf_reader() {
    let mut m = sample();
    m.normalise();
    let bytes = write_glb(&m, "sample", 256).unwrap();
    gltf::Gltf::from_slice(&bytes).unwrap();
    let (doc, _, images) = gltf::import_slice(&bytes).unwrap();
    assert_eq!(doc.materials().len(), 2);
    assert!(images.iter().all(|i| i.width <= 256 && i.height <= 256));
    let back = read_glb(&bytes).unwrap();
    assert_eq!(back.tris(), m.tris());
    assert_eq!(back.tris(), 9828);
    let (a, b) = (m.size(), back.size());
    assert!((0..3).all(|k| (a[k] - b[k]).abs() < 1e-4));
}

#[test]
fn normalise_puts_min_y_at_zero_and_centres_the_footprint() {
    let mut m = sample();
    let before = m.size();
    m.normalise();
    let (lo, hi) = m.bounds();
    assert!(lo[1].abs() < 1e-5);
    assert!((lo[0] + hi[0]).abs() < 1e-5 && (lo[2] + hi[2]).abs() < 1e-5);
    assert!((0..3).all(|k| (m.size()[k] - before[k]).abs() < 1e-5));
}

#[test]
fn orm_packs_separate_maps() {
    let g = |v| GrayImage::from_pixel(4, 4, Luma([v]));
    let p = pack_orm(Some(&g(10)), Some(&g(20)), Some(&g(30)));
    assert_eq!(p.get_pixel(1, 1).0, [10, 20, 30]);
    let p = pack_orm(None, Some(&g(20)), None);
    assert_eq!(p.get_pixel(0, 0).0, [255, 20, 0]);
    // a map of another size is resized to the largest
    let small = GrayImage::from_pixel(2, 2, Luma([7]));
    assert_eq!(
        pack_orm(Some(&small), Some(&g(20)), None).dimensions(),
        (4, 4)
    );
}

#[test]
fn simplification_keeps_the_box_and_hits_the_target() {
    let mut m = sample();
    m.normalise();
    let (lo, hi) = m.bounds();
    let t0 = m.tris();
    for frac in [0.35, 0.08] {
        let target = (t0 as f64 * frac) as usize;
        let s = m.simplified(target);
        let (slo, shi) = s.bounds();
        for k in 0..3 {
            let extent = hi[k] - lo[k];
            assert!(
                (slo[k] - lo[k]).abs() <= 0.05 * extent && (shi[k] - hi[k]).abs() <= 0.05 * extent,
                "axis {k} at {frac}"
            );
        }
        let got = s.tris() as f64;
        assert!(
            (got - target as f64).abs() <= 0.2 * target as f64,
            "{got} triangles for target {target}"
        );
    }
}

fn cube_model() -> Model {
    let pos = vec![
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, -1.0],
        [1.0, 2.0, -1.0],
        [-1.0, 2.0, 1.0],
    ];
    let mesh = Mesh {
        nrm: vec![[0.0, 1.0, 0.0]; 4],
        uv: vec![[0.0; 2]; 4],
        idx: vec![0, 1, 2, 0, 2, 3],
        pos,
        mat: 0,
    };
    Model {
        meshes: vec![mesh],
        mats: vec![Default::default()],
    }
}

fn asset(id: &str, license: &str) -> Asset {
    Asset {
        id: id.into(),
        name: "Thing".into(),
        category: Category::Furniture,
        tags: vec![],
        license: license.into(),
        origin: Origin::BuiltIn,
        author: "Someone".into(),
        source_url: "https://example.org/thing".into(),
        size_m: [2.0, 2.0, 2.0],
        lods: vec![Lod {
            file: format!("models/{id}/lod0.glb"),
            max_distance_m: 30.0,
        }],
        thumbnail: format!("models/{id}/thumb.png"),
        plant: None,
        light: None,
    }
}

#[test]
fn catalogue_entry_validates() {
    let c = Catalog {
        assets: vec![asset("a", "CC0-1.0")],
    };
    let c = Catalog::from_json_bytes(&c.to_json_bytes()).unwrap();
    assert!(validate_open_licences(&c).is_ok());
    assert!(
        validate_open_licences(&Catalog {
            assets: vec![asset("b", "private")]
        })
        .is_err()
    );
}

#[test]
fn check_fails_on_a_missing_file() {
    let out = std::env::temp_dir().join(format!("vca-check-{}", std::process::id()));
    let dir = out.join("models/a");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("lod0.glb"),
        write_glb(&cube_model(), "a", 64).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("thumb.png"), b"x").unwrap();
    std::fs::write(
        dir.join("entry.json"),
        serde_json::to_vec(&asset("a", "CC0-1.0")).unwrap(),
    )
    .unwrap();
    library::catalog(&out).unwrap();
    assert_eq!(library::check(&out), Vec::<String>::new());
    std::fs::remove_file(dir.join("lod0.glb")).unwrap();
    let errors = library::check(&out);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("missing file models/a/lod0.glb")),
        "{errors:?}"
    );
    std::fs::remove_dir_all(&out).unwrap();
}
