use super::{fail, merge_update, next_id};
use crate::material::{Assignment, Material, MaterialMaps, SurfaceKind, Target};
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    let name = || param("name", T::String, "Display name.");
    let kind = || {
        param(
            "kind",
            T::Object,
            "Surface shading: {type: \"standard_triplanar_pbr\", map_scale_m, relief, gloss, weathering} | {type: \"architectural_glass\", tint: [r,g,b], roughness} | {type: \"animated_water\", flow_dir_deg, speed, tint: [r,g,b], depth_m} | {type: \"lawn3d\", density_per_m2, blade_height_m, seed}.",
        )
    };
    let color = || {
        param(
            "base_color",
            T::Color,
            "Linear RGBA, each 0..1 (alpha 1 if three numbers).",
        )
    };
    let maps = || {
        param(
            "maps",
            T::Object,
            "Texture sources {base_color, normal (OpenGL), roughness, metallic, ambient_occlusion, height}: a path relative to the project, or a URL.",
        )
    };
    let target = || {
        param(
            "target",
            T::Object,
            "What to cover: {type: \"face\", owner, face} (a persistent face name), {type: \"body\", owner, body} (a body id) or {type: \"owner\", owner}; owner is a model or shape id.",
        )
        .required()
    };
    s.register_with_spec(
        spec(
            "material.add",
            "Add Material",
            "Adds a material to the project.",
            vec![
                name().required(),
                kind().default(json!({"type": "standard_triplanar_pbr", "map_scale_m": 1, "relief": 0, "gloss": 0, "weathering": 0})),
                color().default(json!([1, 1, 1, 1])),
                param("metallic", T::Number, "0 (dielectric) to 1 (metal).").default(json!(0)),
                param("roughness", T::Number, "0 (mirror) to 1 (matte).").default(json!(0.5)),
                maps().default(json!({})),
            ],
            "{id} (mat1, mat2...)",
            json!({"name": "Red render", "base_color": [0.6, 0.1, 0.08, 1.0], "roughness": 0.8}),
        ),
        add,
    );
    s.register_with_spec(
        spec(
            "material.update",
            "Update Material",
            "Changes fields of a material; fields given replace the old ones whole.",
            vec![
                param("id", T::String, "Material id.").required(),
                name(),
                kind(),
                color(),
                param("metallic", T::Number, "0 to 1."),
                param("roughness", T::Number, "0 to 1."),
                maps(),
            ],
            "null",
            json!({"id": "glass", "roughness": 0.2}),
        ),
        update,
    );
    s.register_with_spec(
        spec(
            "material.remove",
            "Remove Material",
            "Removes a material; refused while anything is assigned it or a terrain layer uses it.",
            vec![param("id", T::String, "Material id.").required()],
            "null",
            json!({"id": "mat1"}),
        ),
        remove,
    );
    s.register_with_spec(
        spec(
            "material.assign",
            "Assign Material",
            "Assigns a material to a face, body or whole owner, replacing what was there. A face beats a body beats the owner. The face or body need not exist now (it may come back after a re-import).",
            vec![
                target(),
                param("material", T::String, "Material id.").required(),
            ],
            "null",
            json!({"target": {"type": "face", "owner": "S2", "face": "S2:top"}, "material": "roof"}),
        ),
        assign,
    );
    s.register_with_spec(
        spec(
            "material.unassign",
            "Clear Material",
            "Clears the assignment of exactly this target.",
            vec![target()],
            "null",
            json!({"target": {"type": "owner", "owner": "S1"}}),
        ),
        unassign,
    );
}

fn add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        #[serde(default)]
        kind: SurfaceKind,
        #[serde(default = "white")]
        base_color: [f32; 4],
        #[serde(default)]
        metallic: f32,
        #[serde(default = "half")]
        roughness: f32,
        #[serde(default)]
        maps: MaterialMaps,
    }
    fn white() -> [f32; 4] {
        [1.0; 4]
    }
    fn half() -> f32 {
        0.5
    }
    let p: P = parse(params)?;
    let id = next_id("mat", |id| project.materials.iter().any(|m| m.id == id));
    let m = Material {
        id: id.clone(),
        name: p.name,
        kind: p.kind,
        base_color: p.base_color,
        metallic: p.metallic,
        roughness: p.roughness,
        maps: p.maps,
        extensions: Default::default(),
    };
    m.validate().map_err(|e| bad(&e))?;
    project.materials.push(m);
    Ok(json!({ "id": id }))
}

fn update(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    let at = project
        .materials
        .iter()
        .position(|m| m.id == id)
        .ok_or_else(|| fail(format!("no material `{id}`")))?;
    let m = merge_update(&project.materials[at], params)?;
    m.validate().map_err(|e| bad(&e))?;
    project.materials[at] = m;
    Ok(Value::Null)
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    if !project.materials.iter().any(|m| m.id == id) {
        return Err(fail(format!("no material `{id}`")));
    }
    if project.assignments.iter().any(|a| a.material == id) {
        return Err(fail(format!("material `{id}` is still assigned")));
    }
    if project
        .terrain
        .as_ref()
        .is_some_and(|t| t.layers.iter().any(|l| l.material == id))
    {
        return Err(fail(format!("material `{id}` is a terrain layer")));
    }
    project.materials.retain(|m| m.id != id);
    Ok(Value::Null)
}

fn assign(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        target: Target,
        material: String,
    }
    let p: P = parse(params)?;
    if !project.materials.iter().any(|m| m.id == p.material) {
        return Err(fail(format!("no material `{}`", p.material)));
    }
    let owner = p.target.owner();
    if !project.models.iter().any(|m| m.id == owner)
        && !project.shapes.iter().any(|s| s.id() == owner)
    {
        return Err(fail(format!("no model or shape `{owner}`")));
    }
    // The face or body is not checked: after a re-import it may not exist for a while.
    project.assignments.retain(|a| a.target != p.target);
    project.assignments.push(Assignment {
        target: p.target,
        material: p.material,
    });
    Ok(Value::Null)
}

fn unassign(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        target: Target,
    }
    let p: P = parse(params)?;
    let before = project.assignments.len();
    project.assignments.retain(|a| a.target != p.target);
    if project.assignments.len() == before {
        return Err(fail("nothing is assigned to that target"));
    }
    Ok(Value::Null)
}
