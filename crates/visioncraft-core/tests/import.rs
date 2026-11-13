//! `tests/data/plate.3dcraft`: written by hand for this crate (format version 69, modelled on
//! 3dcraft's `crates/format/tests/fixtures/versions/v21.3dcraft`): sketch F1 on xy, a 40 x 20 mm
//! rectangle (lines e5..e8), extruded by F2 "Plate" by the variable `height` = 10 mm.

use std::collections::BTreeSet;
use visioncraft_core::{ImportedModel, import_3dcraft};

const PLATE: &[u8] = include_bytes!("data/plate.3dcraft");

fn bbox(model: &ImportedModel) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &model.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn face_names(model: &ImportedModel) -> BTreeSet<String> {
    model
        .triangle_faces
        .iter()
        .map(|&f| model.faces[f as usize].name.clone())
        .collect()
}

fn expected_names() -> BTreeSet<String> {
    [
        "F2:side(e5)",
        "F2:side(e6)",
        "F2:side(e7)",
        "F2:side(e8)",
        "F2:startCap(e5,e6,e7,e8)",
        "F2:endCap(e5,e6,e7,e8)",
    ]
    .map(String::from)
    .into()
}

fn assert_close(got: [f32; 3], want: [f32; 3]) {
    for k in 0..3 {
        assert!((got[k] - want[k]).abs() < 1e-6, "{got:?} != {want:?}");
    }
}

#[test]
fn plate_imports_with_named_faces_in_metres() {
    let model = import_3dcraft(PLATE).unwrap();
    assert!(!model.triangles.is_empty());
    assert_eq!(model.triangle_faces.len(), model.triangles.len());
    assert!(
        model
            .triangle_faces
            .iter()
            .all(|&f| !model.faces[f as usize].name.is_empty())
    );
    assert_eq!(model.bodies.len(), 1);
    assert_eq!(
        (model.bodies[0].id.as_str(), model.bodies[0].name.as_str()),
        ("F2", "Plate")
    );
    assert!(model.faces.iter().all(|f| f.body == 0));
    let (lo, hi) = bbox(&model);
    assert_close(lo, [0.0, 0.0, 0.0]);
    assert_close(hi, [0.04, 0.02, 0.01]); // Z up: the extrusion runs along +Z
    assert_eq!(face_names(&model), expected_names());
}

#[test]
fn face_names_survive_an_edit() {
    let mut doc = craft3d_format::from_bytes(PLATE).unwrap();
    doc.variables[0].expr = craft3d_doc::Expr::new("25 mm");
    let model = import_3dcraft(&craft3d_format::to_bytes(&doc).unwrap()).unwrap();
    let (_, hi) = bbox(&model);
    assert_close(hi, [0.04, 0.02, 0.025]);
    assert_eq!(face_names(&model), expected_names());
}
