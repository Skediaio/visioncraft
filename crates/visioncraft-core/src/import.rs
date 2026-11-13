use crate::{Body, Face, ImportError, ImportedModel};
use std::collections::HashMap;

/// Z up (3dcraft) to Y up (world): a proper rotation, so winding is kept.
fn y_up(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Meshes one body, retrying as 3dcraft's own viewer does. Returns the mesh and whether the
/// tolerance had to be coarsened.
///
/// Ported in outline from 3dcraft (MIT) `crates/engine/src/scene.rs` (`build`, around lines
/// 1700-1715: the angle criterion is a wish, a body that fails with it is meshed without it) and
/// `crates/tess/src/body.rs` (`tessellate_within_budget`, used as is: a body too fine for the
/// tessellator at the tolerance is meshed at the finest coarser tolerance it can do).
/// `automatic_tolerance` (`crates/engine/src/commands.rs`) is by total area of all bodies; ours is
/// per body by its diagonal, a thousandth, as before.
fn mesh_body(body: &craft3d_topo::Body) -> Result<(craft3d_tess::WithinBudget, bool), String> {
    let size = body
        .bbox()
        .map(|b| b.diagonal())
        .map_err(|e| e.to_string())?;
    let tolerance = if size.is_finite() && size > 0.0 {
        size * 1e-3
    } else {
        1e-3
    };
    let mut last = String::new();
    for max_angle in [Some(10f64.to_radians()), None] {
        let params = craft3d_tess::TessParams {
            chordal_tolerance: tolerance,
            max_angle,
            ..Default::default()
        };
        match craft3d_tess::tessellate_within_budget(body, &params) {
            Ok(within) => {
                let coarsened = within.tolerance > tolerance;
                return Ok((within, coarsened));
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// Reads a `.3dcraft` file, rebuilds its feature history and meshes every body it ends with.
///
/// Axes: 3dcraft is Z up (`docs/guide/reference.md`: "X, Y and Z, with Z up"); positions and
/// normals are converted with `(x, y, z) -> (x, z, -y)` to the world's Y up. Lengths are
/// converted from the document's unit to metres. Bodies hidden in the view are left out. A feature
/// that fails to build, or a body that cannot be meshed, does not fail the import: it is skipped
/// and named in `warnings`.
pub fn import_3dcraft(bytes: &[u8]) -> Result<ImportedModel, ImportError> {
    let doc = craft3d_format::from_bytes(bytes).map_err(|e| ImportError::Format(e.to_string()))?;
    if doc.is_assembly() {
        return Err(ImportError::Assembly);
    }
    let metres = craft3d_regen::metres_per_unit(doc.units.length);
    let regen = craft3d_regen::regenerate(&doc);
    let mut model = ImportedModel::default();
    let label = |id: craft3d_doc::FeatureId| {
        let name = doc.feature(id).map(|f| f.name.as_str()).unwrap_or_default();
        format!("{id} {name}")
    };
    if !regen.is_ok() {
        for f in &regen.features {
            if let craft3d_regen::Status::Failed(reason) = &f.status {
                model.warnings.push(format!(
                    "feature {} failed to rebuild: {reason}",
                    label(f.id)
                ));
            }
        }
    }
    for (id, body) in regen.bodies().filter(|(id, _)| !doc.is_hidden(*id)) {
        let (within, coarsened) = match mesh_body(body) {
            Ok(ok) => ok,
            Err(reason) => {
                model.warnings.push(format!(
                    "body {} skipped, cannot be meshed: {reason}",
                    label(id)
                ));
                continue;
            }
        };
        if coarsened {
            model.warnings.push(format!(
                "body {} meshed at a coarser tolerance than asked",
                label(id)
            ));
        }
        let mesh = within.mesh;
        let names = regen.feature(id).and_then(|f| f.names.as_ref());
        let body_index = model.bodies.len() as u32;
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
        let first_triangle = model.triangles.len() as u32;
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            model.positions.push(y_up([
                (p.x * metres) as f32,
                (p.y * metres) as f32,
                (p.z * metres) as f32,
            ]));
            model
                .normals
                .push(y_up([n.x as f32, n.y as f32, n.z as f32]));
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
                    *slot = y_up([n.x as f32, n.y as f32, n.z as f32]);
                }
            }
        }

        // Colour: a fixed or evaluable body colour; one that does not evaluate is left unset.
        let color = doc
            .body_properties(id)
            .and_then(|p| p.appearance.as_ref())
            .and_then(|a| a.resolve(None).ok())
            .map(|a| {
                let [r, g, b] = a.color.map(srgb_to_linear);
                [r, g, b, 1.0]
            });
        let triangles = first_triangle..model.triangles.len() as u32;
        let name = doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
        let body = Body::new(&model, (id.to_string(), name), color, triangles);
        model.bodies.push(body);
    }
    Ok(model)
}

/// Reads a Wavefront OBJ: `v`, `vn` and `f` (any of `v`, `v/vt`, `v//vn`, `v/vt/vn`, negative
/// indices allowed, polygons fanned into triangles); one body and one face per `o`/`g` group,
/// both named after it. OBJ has no unit: metres and Y up are assumed. Corners without a normal
/// get the flat normal of their polygon.
pub fn import_obj(bytes: &[u8]) -> Result<ImportedModel, ImportError> {
    let err = |line: usize, msg: &str| ImportError::Obj(format!("line {}: {msg}", line + 1));
    let text = std::str::from_utf8(bytes).map_err(|e| ImportError::Obj(e.to_string()))?;
    let (mut vs, mut vns): (Vec<[f32; 3]>, Vec<[f32; 3]>) = (Vec::new(), Vec::new());
    let mut model = ImportedModel::default();
    // The current group: its name and where its triangles start.
    let mut group: Option<(String, u32)> = None;
    let mut dedupe: HashMap<(usize, usize), u32> = HashMap::new();
    let mut groups: Vec<(String, u32, u32)> = Vec::new(); // name, first triangle, end triangle

    let close = |group: &mut Option<(String, u32)>, groups: &mut Vec<(String, u32, u32)>, end| {
        if let Some((name, start)) = group.take()
            && end > start
        {
            groups.push((name, start, end));
        }
    };
    for (n, line) in text.lines().enumerate() {
        let mut it = line.split_whitespace();
        let Some(key) = it.next() else { continue };
        match key {
            "v" | "vn" => {
                let c: Vec<f32> = it
                    .take(3)
                    .map(|s| s.parse::<f32>().map_err(|_| err(n, "bad number")))
                    .collect::<Result<_, _>>()?;
                let c: [f32; 3] = c.try_into().map_err(|_| err(n, "expected 3 numbers"))?;
                if key == "v" { &mut vs } else { &mut vns }.push(c);
            }
            "o" | "g" => {
                close(&mut group, &mut groups, model.triangles.len() as u32);
                let name = it.collect::<Vec<_>>().join(" ");
                group = Some((name, model.triangles.len() as u32));
            }
            "f" => {
                let corners: Vec<(usize, Option<usize>)> = it
                    .map(|c| {
                        let mut p = c.split('/');
                        let index = |s: &str, len: usize| -> Result<usize, ImportError> {
                            let i: i64 = s.parse().map_err(|_| err(n, "bad index"))?;
                            let i = if i < 0 { len as i64 + i } else { i - 1 };
                            usize::try_from(i)
                                .ok()
                                .filter(|&i| i < len)
                                .ok_or_else(|| err(n, "index out of range"))
                        };
                        let v = index(p.next().unwrap_or(""), vs.len())?;
                        let vn = match p.nth(1) {
                            Some(s) if !s.is_empty() => Some(index(s, vns.len())?),
                            _ => None,
                        };
                        Ok((v, vn))
                    })
                    .collect::<Result<_, _>>()?;
                if corners.len() < 3 {
                    return Err(err(n, "a face needs at least 3 corners"));
                }
                group.get_or_insert_with(|| ("default".into(), model.triangles.len() as u32));
                let flat = polygon_normal(corners.iter().map(|c| vs[c.0]));
                let mut ids = Vec::with_capacity(corners.len());
                for &(v, vn) in &corners {
                    let id = match vn {
                        Some(vn) => *dedupe.entry((v, vn)).or_insert_with(|| {
                            model.positions.push(vs[v]);
                            model.normals.push(vns[vn]);
                            model.positions.len() as u32 - 1
                        }),
                        None => {
                            model.positions.push(vs[v]);
                            model.normals.push(flat);
                            model.positions.len() as u32 - 1
                        }
                    };
                    ids.push(id);
                }
                for k in 1..ids.len() - 1 {
                    model.triangles.push([ids[0], ids[k], ids[k + 1]]);
                }
            }
            _ => {}
        }
    }
    close(&mut group, &mut groups, model.triangles.len() as u32);
    if groups.is_empty() {
        return Err(ImportError::Obj("no faces".into()));
    }
    for (name, start, end) in groups {
        let body = model.bodies.len() as u32;
        model.faces.push(Face {
            name: name.clone(),
            body,
        });
        model
            .triangle_faces
            .extend((start..end).map(|_| model.faces.len() as u32 - 1));
        let body = Body::new(&model, (name.clone(), name), None, start..end);
        model.bodies.push(body);
    }
    Ok(model)
}

/// Unit normal of a polygon by Newell's method; zero for a degenerate one.
fn polygon_normal(points: impl Iterator<Item = [f32; 3]>) -> [f32; 3] {
    let p: Vec<_> = points.collect();
    let mut n = [0.0f32; 3];
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > 0.0 {
        n.map(|c| c / len)
    } else {
        [0.0; 3]
    }
}
