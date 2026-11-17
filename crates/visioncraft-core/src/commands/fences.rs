//! Fences and screens along a line (`crate::fence`): an open extrusion with a fence.

use super::shapes::add;
use super::{fail, house::assign_defaults};
use crate::fence::{Fence, FenceKind, Gate};
use crate::project::Project;
use crate::session::{CommandError, Session, parse};
use crate::shape::{Extrusion, Shape, shape_mesh};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

/// The fields both `fence.add` and `fence.set` take.
fn fields() -> Vec<crate::spec::ParamSpec> {
    vec![
        param("kind", T::String, "slats (vertical boards), horizontal_slats, panels (closed board panels between posts), picket (low pointed pickets), mesh (a thin wire grid on posts, see-through) or screen (a solid privacy screen).").default(json!("slats")),
        param("slat_width_m", T::Number, "Width of a slat or picket, the height of a horizontal board, the wire's thickness (mesh). Unused by panels and screens. Default by kind.").unit("m"),
        param("gap_m", T::Number, "Gap between slats, boards, pickets or wires. Default by kind.").unit("m"),
        param("post_spacing_m", T::Number, "The most two neighbouring posts stand apart, 0.5..6. Default 1.8 (2.4 for mesh and screen).").unit("m"),
        param("post_size_m", T::Number, "A post's square side, 0.02..0.5. Default by kind.").unit("m"),
        param("rails", T::Boolean, "Rails behind the infill (slats, pickets, boards) or framing it (panels, mesh). Default by kind."),
        param("thickness", T::Number, "Depth of the line, 0.005..1. Default by kind.").unit("m"),
    ]
}

pub(super) fn register(s: &mut Session) {
    let id = || param("id", T::String, "The fence (an open extrusion).").required();
    let mut add_params = vec![
        param("name", T::String, "Display name.").default(json!("Fence")),
        param("line", T::Outline, "The fence line as [x, z] pairs in metres, at least 2 points, not self-intersecting; posts stand at every point.").required(),
        param("height", T::Number, "Height of the fence, 0.3..10.").unit("m").required(),
        param("base", T::Number, "Height of its bottom (on terrain, above the lowest ground under the line).").unit("m").default(json!(0)),
        param("layer", T::String, "Layer id.").default(json!("shapes")),
        param("material", T::String, "One material for the whole fence (a screen in corten, say) instead of the usual timber; faces can still be assigned later."),
        param("gates", T::Array, "Gates: [{offset_m, width_m}], the centre along the line from its start and the gap. Each lies on one straight stretch."),
    ];
    add_params.extend(fields());
    s.register_with_spec(
        spec(
            "fence.add",
            "Add Fence",
            "Adds a fence or screen along a line: posts at every corner, both ends and evenly between, with slats, boards, panels, pickets, wire mesh or a solid screen between them, and gates. An open extrusion; the faces are `<id>:posts`, `<id>:infill`, `<id>:rails` and `<id>:gate`, so a material can be assigned per part. It starts with timber (a dark timber for the posts) where the project has none assigned. On terrain it stands on the lowest ground under its line.",
            add_params,
            "{id} (S1, S2...)",
            json!({"name": "Garden fence", "line": [[0, 0], [8, 0], [8, 6]], "height": 1.8, "kind": "slats"}),
        ),
        add_fence,
    );
    let mut set_params = vec![
        id(),
        param("name", T::String, "Display name."),
        param("line", T::Outline, "New line as [x, z] pairs in metres."),
        param("height", T::Number, "New height.").unit("m"),
        param("base", T::Number, "New base.").unit("m"),
        param(
            "gates",
            T::Array,
            "Replaces all gates: [{offset_m, width_m}].",
        ),
    ];
    set_params.extend(fields());
    s.register_with_spec(
        spec(
            "fence.set",
            "Set Fence",
            "Changes any fields of a fence; the others stay. Changing `kind` starts from the new kind's sizes (keeping the gates) unless you give them. On an open wall that is not a fence yet it turns the wall into one, with the kind's usual depth.",
            set_params,
            "null",
            json!({"id": "S3", "kind": "picket", "height": 1.0}),
        ),
        set_fence,
    );
    s.register_with_spec(
        spec(
            "fence.add_gate",
            "Add Fence Gate",
            "Opens a gap in a fence run with a post on each side and a gate leaf (`<id>:gate`). Refused outside the line, round a corner, over another gate or closer to a corner than a post's width.",
            vec![
                id(),
                param("offset_m", T::Number, "Centre of the gate, along the line from its start.").unit("m").required(),
                param("width_m", T::Number, "Width of the gap, 0.3..6.").unit("m").default(json!(1.0)),
            ],
            "{gate} (the gate's index)",
            json!({"id": "S3", "offset_m": 7.0, "width_m": 1.0}),
        ),
        add_gate,
    );
    s.register_with_spec(
        spec(
            "fence.remove_gate",
            "Remove Fence Gate",
            "Closes a gate: the run is filled in again.",
            vec![
                id(),
                param(
                    "index",
                    T::Integer,
                    "The gate's index, from 0 (see document.inspect).",
                )
                .required(),
            ],
            "null",
            json!({"id": "S3", "index": 0}),
        ),
        remove_gate,
    );
    s.register_with_spec(
        spec(
            "fence.clear",
            "Clear Fence",
            "Turns a fence back into a plain wall of the same line, height and thickness.",
            vec![id()],
            "null",
            json!({"id": "S3"}),
        ),
        clear,
    );
}

fn default_materials(project: &mut Project, id: &str, kind: FenceKind) {
    let faces: [(&str, &str); 4] = if kind == FenceKind::Mesh {
        [
            ("posts", "metal"),
            ("infill", "metal"),
            ("rails", "metal"),
            ("gate", "metal"),
        ]
    } else {
        [
            ("posts", "timber-dark"),
            ("infill", "timber"),
            ("rails", "timber-dark"),
            ("gate", "timber"),
        ]
    };
    assign_defaults(project, id, &faces);
}

/// The fields of `fence.add` and `fence.set`.
#[derive(Deserialize, Default)]
struct Fields {
    kind: Option<FenceKind>,
    slat_width_m: Option<f32>,
    gap_m: Option<f32>,
    post_spacing_m: Option<f32>,
    post_size_m: Option<f32>,
    rails: Option<bool>,
    thickness: Option<f32>,
    gates: Option<Vec<Gate>>,
}

impl Fields {
    /// `fence` with the given fields applied; a different kind starts from its own sizes.
    fn apply(&self, fence: Option<Fence>) -> (Fence, f32, bool) {
        let kind = self
            .kind
            .or(fence.as_ref().map(|f| f.kind))
            .unwrap_or_default();
        let fresh = fence.as_ref().is_none_or(|f| f.kind != kind);
        let mut f = Fence::new(kind);
        if let Some(old) = fence {
            f.gates = old.gates;
            if !fresh {
                f = Fence {
                    gates: f.gates,
                    ..old
                };
            }
        }
        if let Some(v) = self.slat_width_m {
            f.slat_width_m = v;
        }
        if let Some(v) = self.gap_m {
            f.gap_m = v;
        }
        if let Some(v) = self.post_spacing_m {
            f.post_spacing_m = v;
        }
        if let Some(v) = self.post_size_m {
            f.post_size_m = v;
        }
        if let Some(v) = self.rails {
            f.rails = v;
        }
        if let Some(g) = &self.gates {
            f.gates = g.clone();
        }
        let thickness = self.thickness.unwrap_or(kind.default_thickness());
        (f, thickness, fresh)
    }
}

fn add_fence(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: Option<String>,
        line: Vec<[f32; 2]>,
        height: f32,
        #[serde(default)]
        base: f32,
        layer: Option<String>,
        material: Option<String>,
        #[serde(flatten)]
        fields: Fields,
    }
    let p: P = parse(params)?;
    let (fence, thickness, _) = p.fields.apply(None);
    let kind = fence.kind;
    let owned = p.material.is_some();
    let r = add(project, (p.layer, p.material), |id, layer| {
        Shape::Extrusion(Extrusion {
            id,
            name: p.name.unwrap_or_else(|| "Fence".into()),
            layer,
            outline: p.line,
            closed: false,
            base: p.base,
            height: p.height,
            thickness,
            extensions: Default::default(),
            house: None,
            fence: Some(fence),
            stairs: None,
            raised_bed: None,
        })
    })?;
    if !owned && let Some(id) = r["id"].as_str() {
        default_materials(project, id, kind);
    }
    Ok(r)
}

fn set_fence(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: Option<String>,
        line: Option<Vec<[f32; 2]>>,
        height: Option<f32>,
        base: Option<f32>,
        #[serde(flatten)]
        fields: Fields,
    }
    let p: P = parse(params)?;
    let kind = edit(project, &p.id, true, |e| {
        let (fence, thickness, fresh) = p.fields.apply(e.fence.take());
        if (fresh && p.fields.thickness.is_none()) || p.fields.thickness.is_some() {
            e.thickness = thickness;
        }
        e.fence = Some(fence);
        if let Some(n) = p.name {
            e.name = n;
        }
        if let Some(l) = p.line {
            e.outline = l;
        }
        if let Some(h) = p.height {
            e.height = h;
        }
        if let Some(b) = p.base {
            e.base = b;
        }
        Ok(Value::Null)
    })?;
    default_materials(project, &p.id, kind);
    Ok(Value::Null)
}

fn add_gate(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        offset_m: f32,
        width_m: Option<f32>,
    }
    let p: P = parse(params)?;
    let mut index = 0;
    edit(project, &p.id, false, |e| {
        let f = e.fence.as_mut().expect("checked by edit");
        f.gates.push(Gate {
            offset_m: p.offset_m,
            width_m: p.width_m.unwrap_or(1.0),
        });
        index = f.gates.len() - 1;
        Ok(Value::Null)
    })?;
    Ok(json!({ "gate": index }))
}

fn remove_gate(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        index: usize,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, false, |e| {
        let f = e.fence.as_mut().expect("checked by edit");
        if p.index >= f.gates.len() {
            return Err(fail(format!(
                "no gate {}; the fence has {}",
                p.index,
                f.gates.len()
            )));
        }
        f.gates.remove(p.index);
        Ok(Value::Null)
    })?;
    Ok(Value::Null)
}

fn clear(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| fail("`id`: a fence"))?;
    edit(project, id, false, |e| {
        e.fence = None;
        Ok(Value::Null)
    })?;
    Ok(Value::Null)
}

/// Applies `f` to a copy of open extrusion `id` and keeps it only if it still meshes. A fence is
/// required unless `make`. Returns the fence's kind afterwards (the plain wall's: slats).
fn edit(
    project: &mut Project,
    id: &str,
    make: bool,
    f: impl FnOnce(&mut Extrusion) -> Result<Value, CommandError>,
) -> Result<FenceKind, CommandError> {
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == id)
        .ok_or_else(|| fail(format!("no shape `{id}`")))?;
    let mut shape = project.shapes[at].clone();
    let Shape::Extrusion(e) = &mut shape else {
        return Err(fail(format!("`{id}` is an area, not a fence")));
    };
    if e.closed {
        return Err(fail(format!("`{id}` is a closed block, not a fence line")));
    }
    if e.fence.is_none() && !make {
        return Err(fail(format!(
            "`{id}` is a plain wall, not a fence; use fence.set"
        )));
    }
    f(e)?;
    let kind = e.fence.as_ref().map(|f| f.kind).unwrap_or_default();
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use crate::{Project, Session};
    use serde_json::json;

    fn fence(s: &mut Session) -> String {
        s.run(
            "fence.add",
            json!({"line": [[0, 0], [10, 0], [10, 6]], "height": 1.8, "kind": "slats"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn get(s: &Session, id: &str) -> crate::shape::Extrusion {
        match s.project().shapes.iter().find(|x| x.id() == id).unwrap() {
            crate::Shape::Extrusion(e) => e.clone(),
            _ => panic!("not an extrusion"),
        }
    }

    #[test]
    fn a_fence_is_added_with_defaults_and_materials_and_can_be_undone() {
        let mut s = Session::new(Project::default());
        let id = fence(&mut s);
        let e = get(&s, &id);
        assert!(!e.closed && e.fence.is_some());
        assert_eq!(e.name, "Fence");
        assert_eq!(e.fence.as_ref().unwrap().post_spacing_m, 1.8);
        let faces = crate::shape_mesh(&s.project().shapes[0]).unwrap().faces;
        assert_eq!(faces[1].name, format!("{id}:infill"));
        let mats: Vec<_> = s
            .project()
            .assignments
            .iter()
            .map(|a| a.material.as_str())
            .collect();
        assert!(
            mats.contains(&"timber") && mats.contains(&"timber-dark"),
            "{mats:?}"
        );
        assert!(s.undo());
        assert!(s.project().shapes.is_empty() && s.project().assignments.is_empty());
    }

    #[test]
    fn set_changes_fields_and_a_new_kind_takes_its_own_sizes() {
        let mut s = Session::new(Project::default());
        let id = fence(&mut s);
        s.run("fence.set", json!({"id": id, "height": 2.0, "gap_m": 0.04}))
            .unwrap();
        let e = get(&s, &id);
        assert_eq!(e.height, 2.0);
        assert_eq!(e.fence.as_ref().unwrap().gap_m, 0.04);
        s.run("fence.add_gate", json!({"id": id, "offset_m": 4.0}))
            .unwrap();
        s.run("fence.set", json!({"id": id, "kind": "mesh"}))
            .unwrap();
        let e = get(&s, &id);
        let f = e.fence.as_ref().unwrap();
        assert_eq!(f.post_spacing_m, 2.4);
        assert_eq!(f.gates.len(), 1);
        assert_eq!(
            e.thickness,
            crate::fence::FenceKind::Mesh.default_thickness()
        );
        let err = s
            .run("fence.set", json!({"id": id, "post_spacing_m": 0.1}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("post_spacing_m"), "{err}");
        assert!(s.run("fence.set", json!({"id": id, "colour": 1})).is_err());
        assert!(s.undo());
        assert_eq!(get(&s, &id).fence.unwrap().post_spacing_m, 1.8);
    }

    #[test]
    fn a_plain_wall_becomes_a_fence_and_back() {
        let mut s = Session::new(Project::default());
        let id = s
            .run(
                "shape.add_extrusion",
                json!({"name": "Wall", "outline": [[0, 0], [6, 0]], "closed": false, "height": 1.5, "thickness": 0.2}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            s.run("fence.add_gate", json!({"id": id, "offset_m": 3}))
                .is_err()
        );
        s.run("fence.set", json!({"id": id, "kind": "picket"}))
            .unwrap();
        let e = get(&s, &id);
        assert!(e.fence.is_some() && e.thickness == 0.04);
        s.run("fence.clear", json!({"id": id})).unwrap();
        let e = get(&s, &id);
        assert!(e.fence.is_none() && e.height == 1.5);
        assert!(s.run("fence.clear", json!({"id": id})).is_err());
        assert!(s.undo());
        assert!(get(&s, &id).fence.is_some());
    }

    #[test]
    fn gates_are_added_and_removed_with_undo_and_bad_ones_refused() {
        let mut s = Session::new(Project::default());
        let id = fence(&mut s);
        let g = s
            .run(
                "fence.add_gate",
                json!({"id": id, "offset_m": 5.0, "width_m": 1.2}),
            )
            .unwrap();
        assert_eq!(g["gate"], 0);
        assert_eq!(get(&s, &id).fence.unwrap().gates.len(), 1);
        let mesh = crate::shape_mesh(&s.project().shapes[0]).unwrap();
        let gate = mesh
            .faces
            .iter()
            .position(|f| f.name == format!("{id}:gate"))
            .unwrap() as u32;
        assert!(mesh.triangle_faces.contains(&gate));
        for bad in [
            json!({"id": id, "offset_m": 30.0}),
            json!({"id": id, "offset_m": 10.0}),
            json!({"id": id, "offset_m": 5.5}),
        ] {
            assert!(s.run("fence.add_gate", bad).is_err());
        }
        assert!(
            s.run("fence.remove_gate", json!({"id": id, "index": 3}))
                .is_err()
        );
        s.run("fence.remove_gate", json!({"id": id, "index": 0}))
            .unwrap();
        assert!(get(&s, &id).fence.unwrap().gates.is_empty());
        assert!(s.undo());
        assert_eq!(get(&s, &id).fence.unwrap().gates.len(), 1);
    }

    #[test]
    fn a_fence_with_gates_given_at_creation_and_files_without_a_fence_still_load() {
        let mut s = Session::new(Project::default());
        let r = s.run(
            "fence.add",
            json!({"line": [[0, 0], [6, 0]], "height": 1.0, "kind": "picket", "gates": [{"offset_m": 3.0, "width_m": 1.0}]}),
        );
        assert!(r.is_ok(), "{r:?}");
        let before = serde_json::to_value(&s.project().shapes[0]).unwrap();
        assert!(before["fence"]["gates"].is_array());
        let wall: crate::Shape = serde_json::from_value(json!({
            "type": "extrusion", "id": "S9", "name": "W", "layer": "shapes",
            "outline": [[0.0, 0.0], [1.0, 0.0]], "closed": false, "base": 0.0, "height": 1.0, "thickness": 0.2
        }))
        .unwrap();
        assert!(serde_json::to_value(&wall).unwrap().get("fence").is_none());
    }

    #[test]
    fn quantities_count_the_parts_and_use_the_real_volume() {
        let mut s = Session::new(Project::default());
        let id = s
            .run(
                "fence.add",
                json!({"line": [[0, 0], [2, 0]], "height": 1.8, "gates": []}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let q = s.run("query.quantities", json!({})).unwrap();
        let row = q["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id.as_str())
            .unwrap();
        assert_eq!(row["fence"]["posts"], 3);
        assert_eq!(row["fence"]["slats"], 16);
        assert_eq!(row["fence"]["gates"], 0);
        let v = row["volume_m3"].as_f64().unwrap();
        // Far less than the slab 2 m * 0.05 m * 1.8 m = 0.18 m3, and not nothing.
        assert!(v > 0.02 && v < 0.12, "{v}");
    }
}
