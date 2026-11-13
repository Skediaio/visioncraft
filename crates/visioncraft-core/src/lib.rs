//! Headless model data for VisionCraft: designs read into triangle meshes.

use std::fmt;

/// A design as triangles, every triangle tagged with the B-rep face it came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedModel {
    /// Metres, in the source's own axes.
    pub positions: Vec<[f32; 3]>,
    /// Unit length, one per position.
    pub normals: Vec<[f32; 3]>,
    /// Counter-clockwise seen from outside.
    pub triangles: Vec<[u32; 3]>,
    /// One per triangle: index into `faces`.
    pub triangle_faces: Vec<u32>,
    pub faces: Vec<Face>,
    pub bodies: Vec<Body>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// The persistent name, stable across edits upstream, e.g. `F2:side(e7)`.
    pub name: String,
    /// Index into `bodies`.
    pub body: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    /// The id of the feature that made the body, e.g. `F2`.
    pub id: String,
    /// That feature's name in the history, e.g. `Plate`.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImportError {
    /// Not a readable `.3dcraft` file.
    Format(String),
    /// A `.3dcraftasm` assembly; only parts are read.
    Assembly,
    /// A body that could not be meshed.
    Tessellation { body: String, reason: String },
    /// A face without a persistent name (3dcraft names every face it builds; this means it did not).
    UnnamedFace { body: String, face: usize },
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(reason) => write!(f, "not a readable .3dcraft file: {reason}"),
            Self::Assembly => f.write_str("an assembly cannot be read, only a part"),
            Self::Tessellation { body, reason } => {
                write!(f, "body {body} cannot be meshed: {reason}")
            }
            Self::UnnamedFace { body, face } => write!(f, "face {face} of body {body} has no name"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Reads a `.3dcraft` file, rebuilds its feature history and meshes every body it ends with.
///
/// Axes are 3dcraft's, unchanged: right-handed with **Z up** (3dcraft `docs/guide/reference.md`:
/// "X, Y and Z, with Z up"; a sketch on `xy` extrudes along +Z). Positions are converted from the
/// document's length unit to metres. Bodies hidden in the view and features that fail to build
/// are left out, as 3dcraft's own view leaves them out.
pub fn import_3dcraft(bytes: &[u8]) -> Result<ImportedModel, ImportError> {
    let doc = craft3d_format::from_bytes(bytes).map_err(|e| ImportError::Format(e.to_string()))?;
    if doc.is_assembly() {
        return Err(ImportError::Assembly);
    }
    let metres = craft3d_regen::metres_per_unit(doc.units.length);
    let regen = craft3d_regen::regenerate(&doc);
    let mut model = ImportedModel::default();
    for (id, body) in regen.bodies().filter(|(id, _)| !doc.is_hidden(*id)) {
        let body_index = model.bodies.len() as u32;
        model.bodies.push(Body {
            id: id.to_string(),
            name: doc.feature(id).map(|f| f.name.clone()).unwrap_or_default(),
        });
        let failed = |reason: String| ImportError::Tessellation {
            body: id.to_string(),
            reason,
        };
        // shortcut: a thousandth of the body's size, as 3dcraft's automatic tolerance; expose quality when large scenes need it.
        let size = body.bbox().map_err(|e| failed(e.to_string()))?.diagonal();
        let params = craft3d_tess::TessParams {
            chordal_tolerance: if size.is_finite() && size > 0.0 {
                size * 1e-3
            } else {
                1e-3
            },
            max_angle: Some(10f64.to_radians()),
            ..Default::default()
        };
        let mesh =
            craft3d_tess::tessellate_body(body, &params).map_err(|e| failed(e.to_string()))?;

        let names = regen.feature(id).and_then(|f| f.names.as_ref());
        let first_face = model.faces.len() as u32;
        for face in 0..body.faces.len() {
            let name = names
                .and_then(|n| n.faces.get(face))
                .ok_or(ImportError::UnnamedFace {
                    body: id.to_string(),
                    face,
                })?;
            model.faces.push(Face {
                name: name.to_string(),
                body: body_index,
            });
        }

        let base = model.positions.len() as u32;
        let first_normal = model.normals.len();
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            model.positions.push([
                (p.x * metres) as f32,
                (p.y * metres) as f32,
                (p.z * metres) as f32,
            ]);
            model.normals.push([n.x as f32, n.y as f32, n.z as f32]);
        }
        for (t, face) in mesh.triangles.iter().zip(&mesh.face_ids) {
            model.triangles.push(t.map(|i| i + base));
            model.triangle_faces.push(first_face + face);
        }
        // tess marks "no normal here" with a zero vector: use the triangle's own.
        for (k, t) in mesh.triangles.iter().enumerate() {
            let Some([a, b, c]) = mesh.corners(k) else {
                continue;
            };
            let Some(n) = (b - a).cross(c - a).normalized() else {
                continue;
            };
            for i in t {
                if let Some(slot) = model.normals.get_mut(first_normal + *i as usize)
                    && *slot == [0.0; 3]
                {
                    *slot = [n.x as f32, n.y as f32, n.z as f32];
                }
            }
        }
    }
    Ok(model)
}
