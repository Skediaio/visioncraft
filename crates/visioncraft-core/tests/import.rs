//! `tests/data/plate.3dcraft`: written by hand for this crate (format version 69, modelled on
//! 3dcraft's `crates/format/tests/fixtures/versions/v21.3dcraft`): sketch F1 on xy, a 40 x 20 mm
//! rectangle (lines e5..e8), extruded by F2 "Plate" by the variable `height` = 10 mm.
//!
//! `tests/data/plane-cut-house.3dcraft`: 3dcraft's own fixture of format version 78 (what 0.3.0
//! saves), copied unchanged from `crates/format/tests/fixtures/versions/v78.3dcraft` at tag
//! v0.3.0 (MIT, Johan Coppens): written by its `craft3d-cli`, never by hand. A block "House"
//! 40 x 30 x 20 mm is cut by three planes in one feature, "Roof" (`feature.planeCut`, the reason
//! for version 78): two slopes rising 8 over 15 from eaves at z = 12 mm to a ridge at y = 15 mm,
//! z = 20 mm, and the side x = 0 moved in by 2 mm. "Upper" is a kept copy of it, cut below
//! z = 6 mm. In closed form: Roof 38 x (30 x 12 + 30 x 8 / 2) = 18 240 mm3, Upper
//! 18 240 - 38 x 30 x 6 = 11 400 mm3.

use std::collections::BTreeSet;
use visioncraft_core::{ImportedModel, import_3dcraft, import_obj};

const PLATE: &[u8] = include_bytes!("data/plate.3dcraft");
const PLANE_CUT_HOUSE: &[u8] = include_bytes!("data/plane-cut-house.3dcraft");

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

fn bodies(model: &ImportedModel) -> Vec<(String, String)> {
    model
        .bodies
        .iter()
        .map(|b| (b.id.clone(), b.name.clone()))
        .collect()
}

fn body_faces(model: &ImportedModel, body: u32) -> BTreeSet<String> {
    model
        .faces
        .iter()
        .filter(|f| f.body == body)
        .map(|f| f.name.clone())
        .collect()
}

/// The volume a body's triangles enclose, in m3, positive when they wind counter-clockwise seen
/// from outside (the promise of `ImportedModel::triangles`; the turn to Y up is a rotation).
fn volume(model: &ImportedModel, body: u32) -> f64 {
    let b = &model.bodies[body as usize];
    let p = |i: u32| model.positions[i as usize].map(f64::from);
    let range = b.triangles.start as usize..b.triangles.end as usize;
    model.triangles[range]
        .iter()
        .map(|t| {
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum()
}

#[test]
fn a_format_78_design_opens() {
    // The 3dcraft crates at v0.1.3 refused it: "this file has format version 78, and this build
    // of 3DCraft reads up to version 69: a newer 3DCraft is needed to open it".
    let model = import_3dcraft(PLANE_CUT_HOUSE).unwrap();
    assert!(model.warnings.is_empty(), "{:?}", model.warnings);
    assert_eq!(
        bodies(&model),
        [("F3".into(), "Roof".into()), ("F4".into(), "Upper".into())]
    );
    assert_eq!(model.faces.len(), 14);
    // The faces the cuts left are named after their plane; the ones kept keep the name they had.
    let kept = [
        "F1:face(2)",
        "F1:face(3)",
        "F1:face(5)",
        "F3:cut(6)",
        "F3:cut(5)",
        "F3:cut(7)",
    ];
    assert_eq!(
        body_faces(&model, 0),
        [
            "F3:copy(0)/F1:face(0)",
            "F3:copy(0)/F1:face(2)",
            "F3:copy(0)/F1:face(3)",
            "F3:copy(0)/F1:face(5)",
            "F3:cut(6)",
            "F3:cut(5)",
            "F3:cut(7)",
        ]
        .map(String::from)
        .into()
    );
    let mut upper: BTreeSet<String> = kept.iter().map(|f| format!("F4:copy(0)/{f}")).collect();
    upper.insert("F4:cut(1)".into());
    assert_eq!(body_faces(&model, 1), upper);
    // 3dcraft is Z up; (x, y, z) -> (x, z, -y), in metres: x 2..40 mm, y 0..20 mm (z 0..20).
    assert_close(model.bodies[0].bounds[0], [0.002, 0.0, -0.03]);
    assert_close(model.bodies[0].bounds[1], [0.04, 0.02, 0.0]);
    assert_close(model.bodies[1].bounds[0], [0.002, 0.006, -0.03]);
    assert_close(model.bodies[1].bounds[1], [0.04, 0.02, 0.0]);
    // Each body: 7 faces, 10 vertices, 15 edges, so 2 x 15 - 2 x 7 = 16 triangles, and no point
    // is added inside a face.
    assert_eq!(model.bodies[0].triangles, 0..16);
    assert_eq!(model.bodies[1].triangles, 16..32);
    assert_eq!(model.triangle_faces.len(), 32);
    assert!(
        (volume(&model, 0) - 18.24e-6).abs() < 1e-9,
        "{}",
        volume(&model, 0)
    );
    assert!(
        (volume(&model, 1) - 11.4e-6).abs() < 1e-9,
        "{}",
        volume(&model, 1)
    );
}

#[test]
fn a_newer_format_is_refused_by_its_number() {
    // VisionCraft keeps no version of its own: the 3dcraft crates read up to theirs, and say so.
    assert_eq!(craft3d_format::FORMAT_VERSION, 78);
    let entries = craft3d_zipstore::read_stored(PLANE_CUT_HOUSE).unwrap();
    let manifest = entries
        .iter()
        .find(|e| e.name == "manifest.json")
        .unwrap()
        .data;
    let newer = String::from_utf8_lossy(manifest).replacen(
        "\"format_version\": 78",
        "\"format_version\": 79",
        1,
    );
    assert!(
        newer.contains("\"format_version\": 79"),
        "the manifest is pretty JSON"
    );
    let bytes = craft3d_zipstore::write_stored(&[("manifest.json", newer.as_bytes())]).unwrap();
    let err = import_3dcraft(&bytes).unwrap_err().to_string();
    assert!(
        err.contains("format version 79")
            && err.contains(&format!(
                "reads up to version {}",
                craft3d_format::FORMAT_VERSION
            )),
        "{err}"
    );
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
