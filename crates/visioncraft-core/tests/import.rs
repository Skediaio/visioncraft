//! `tests/data/plate.3dcraft`: written by hand for this crate (format version 69, modelled on
//! 3dcraft's `crates/format/tests/fixtures/versions/v21.3dcraft`): sketch F1 on xy, a 40 x 20 mm
//! rectangle (lines e5..e8), extruded by F2 "Plate" by the variable `height` = 10 mm.

use std::collections::BTreeSet;
use visioncraft_core::{ImportedModel, import_3dcraft, import_obj};

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
    // 3dcraft is Z up, the world Y up: (x, y, z) -> (x, z, -y), so the plate lies in XZ.
    assert_close(lo, [0.0, 0.0, -0.02]);
    assert_close(hi, [0.04, 0.01, 0.0]);
    assert!(model.normals.iter().all(|n| {
        let len = n.iter().map(|c| c * c).sum::<f32>();
        (len - 1.0).abs() < 1e-4
    }));
    assert_eq!(model.bodies[0].triangles, 0..model.triangles.len() as u32);
    assert!(model.warnings.is_empty());
    assert_eq!(face_names(&model), expected_names());
}

#[test]
fn face_names_survive_an_edit() {
    let mut doc = craft3d_format::from_bytes(PLATE).unwrap();
    doc.variables[0].expr = craft3d_doc::Expr::new("25 mm");
    let model = import_3dcraft(&craft3d_format::to_bytes(&doc).unwrap()).unwrap();
    let (_, hi) = bbox(&model);
    assert_close(hi, [0.04, 0.025, 0.0]);
    assert_eq!(face_names(&model), expected_names());
}

const CUBE: &str = "o Cube\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 4 3 2\nf 5 6 7 8\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\n";

#[test]
fn obj_cube_without_normals_gets_flat_normals() {
    let m = import_obj(CUBE.as_bytes()).unwrap();
    assert_eq!(m.triangles.len(), 12); // six quads fanned
    assert_eq!(m.bodies.len(), 1);
    assert_eq!(
        (m.bodies[0].id.as_str(), m.faces[0].name.as_str()),
        ("Cube", "Cube")
    );
    assert_eq!(m.faces.len(), 1);
    assert_eq!(m.normals.len(), m.positions.len());
    // The first quad (1 4 3 2) lies at z = 0 and faces -Z.
    for &i in &m.triangles[0] {
        assert_close(m.normals[i as usize], [0.0, 0.0, -1.0]);
    }
    let (lo, hi) = bbox(&m);
    assert_close(lo, [0.0; 3]);
    assert_close(hi, [1.0; 3]);
}

#[test]
fn obj_with_normals_shares_vertices_and_splits_groups() {
    let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nvn 0 0 1\nvn 0 1 0\n\
g A\nf 1//1 2//1 3//1\ng B\nf -4//2 -3//2 -1//2\n";
    let m = import_obj(obj.as_bytes()).unwrap();
    assert_eq!(m.bodies.len(), 2);
    assert_eq!(
        m.faces.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert_eq!(m.triangle_faces, [0, 1]);
    assert_eq!(m.normals[m.triangles[0][0] as usize], [0.0, 0.0, 1.0]);
    assert_eq!(m.normals[m.triangles[1][0] as usize], [0.0, 1.0, 0.0]);
    assert_eq!(m.bodies[1].triangles, 1..2);
    assert!(import_obj(b"v 0 0 0\nf 1 2 3\n").is_err());
    assert!(import_obj(b"v 0 0 0\n").is_err());
}

#[test]
fn ship_propeller_imports_or_is_reported() {
    // Failed with a single fixed tolerance; the retry ladder must not lose the whole file.
    let bytes = include_bytes!("data/ship-propeller.3dcraft");
    let m = import_3dcraft(bytes).unwrap();
    eprintln!(
        "bodies {} triangles {} warnings {:?}",
        m.bodies.len(),
        m.triangles.len(),
        m.warnings
    );
    assert!(!m.bodies.is_empty() || !m.warnings.is_empty());
    assert_eq!(m.triangle_faces.len(), m.triangles.len());
}
