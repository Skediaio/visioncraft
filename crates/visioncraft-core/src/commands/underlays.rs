//! Plan images under the drawing: add, change, calibrate, remove.

use super::{check_layer, fail, next_id};
use crate::project::{Layer, Project};
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use crate::underlay::{LAYER_UNDERLAY, Placement, Underlay};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "underlay.add",
            "Add Underlay",
            "Places a plan image (PNG or JPEG: a floor plan, a site plan) flat under the drawing to trace over. The host reads the image's size when it can (`size_px`); the scale is a guess until `underlay.calibrate` or `underlay.scale_by_distance` sets it. The image is drawn see-through and unlit, casts no shadow and is not picked.",
            vec![
                param("file", T::String, "Path relative to the project file, or `underlays/<name>` inside a bundle.").required(),
                param("name", T::String, "Display name.").default(json!("the file's name")),
                param("layer", T::String, "Layer id; the `underlay` layer (made when missing) when not given, so it can be hidden alone.").default(json!("underlay")),
                param("origin", T::Point2, "Where the image's top-left corner lands on the ground, [x, z].").unit("m").default(json!([0, 0])),
                param("metres_per_pixel", T::Number, "Scale: the ground length of one pixel, above zero.").unit("m").default(json!(0.05)),
                param("rotation_deg", T::Number, "Turn about the origin, counter-clockwise seen from above (like a model's rotation).").unit("deg").default(json!(0)),
                param("opacity", T::Number, "0 (invisible) to 1 (opaque).").default(json!(0.5)),
                param("elevation", T::Number, "Height above the ground plane, a little above so it does not flicker with it.").unit("m").default(json!(0.02)),
                param("size_px", T::Array, "[width, height] of the image in pixels; filled in by the host when it can read the file, required otherwise.").unit("px"),
            ],
            "{id} (u1, u2...)",
            json!({"file": "plan.png", "name": "Site plan", "origin": [0, 0], "metres_per_pixel": 0.05, "opacity": 0.6, "size_px": [800, 600]}),
        ),
        add,
    );
    s.register_with_spec(
        spec(
            "underlay.set",
            "Change Underlay",
            "Changes an underlay; only the fields given change.",
            vec![
                param("id", T::String, "Underlay id.").required(),
                param("name", T::String, "Display name."),
                param(
                    "file",
                    T::String,
                    "Another image file (give `size_px` too unless the host can read it).",
                ),
                param("layer", T::String, "Layer id."),
                param(
                    "origin",
                    T::Point2,
                    "Where the image's top-left corner lands, [x, z].",
                )
                .unit("m"),
                param("metres_per_pixel", T::Number, "Ground length of one pixel.").unit("m"),
                param(
                    "rotation_deg",
                    T::Number,
                    "Turn about the origin, counter-clockwise seen from above.",
                )
                .unit("deg"),
                param("opacity", T::Number, "0 to 1."),
                param("elevation", T::Number, "Height above the ground plane.").unit("m"),
                param(
                    "size_px",
                    T::Array,
                    "[width, height] of the image in pixels.",
                )
                .unit("px"),
            ],
            "null",
            json!({"id": "u1", "opacity": 0.35, "rotation_deg": 12.5}),
        ),
        set,
    );
    s.register_with_spec(
        spec(
            "underlay.calibrate",
            "Calibrate Underlay",
            "Sets scale, rotation and origin exactly from two image pixels and the two drawing points they must land on: measure a known dimension on the plan (a wall of 10 m), say where its ends are in the drawing. The image is never mirrored or skewed.",
            vec![
                param("id", T::String, "Underlay id.").required(),
                param("pixel_a", T::Point2, "First image position [x, y] in pixels (top-left is 0, 0; y runs down).").unit("px").required(),
                param("pixel_b", T::Point2, "Second image position, not the same.").unit("px").required(),
                param("world_a", T::Point2, "Where pixel_a must land in the drawing, [x, z].").unit("m").required(),
                param("world_b", T::Point2, "Where pixel_b must land, not the same point.").unit("m").required(),
            ],
            "{origin, metres_per_pixel, rotation_deg}",
            json!({"id": "u1", "pixel_a": [100, 200], "pixel_b": [500, 200], "world_a": [0, 0], "world_b": [10, 0]}),
        ),
        calibrate,
    );
    s.register_with_spec(
        spec(
            "underlay.scale_by_distance",
            "Scale Underlay by a Distance",
            "Sets the scale so that two image pixels are `distance_m` apart on the ground; origin and rotation stay.",
            vec![
                param("id", T::String, "Underlay id.").required(),
                param("pixel_a", T::Point2, "First image position [x, y] in pixels.").unit("px").required(),
                param("pixel_b", T::Point2, "Second image position, not the same.").unit("px").required(),
                param("distance_m", T::Number, "The real distance between them, above zero.").unit("m").required(),
            ],
            "{metres_per_pixel}",
            json!({"id": "u1", "pixel_a": [100, 200], "pixel_b": [500, 200], "distance_m": 10}),
        ),
        scale_by_distance,
    );
    s.register_with_spec(
        spec(
            "underlay.remove",
            "Remove Underlay",
            "Removes an underlay from the project (the image file stays where it is).",
            vec![param("id", T::String, "Underlay id.").required()],
            "null",
            json!({"id": "u1"}),
        ),
        remove,
    );
}

fn find<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut Underlay, CommandError> {
    project
        .underlays
        .iter_mut()
        .find(|u| u.id == id)
        .ok_or_else(|| fail(format!("no underlay `{id}`")))
}

/// The file's name without folders and extension, for a default display name.
fn stem(file: &str) -> String {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_string()
}

fn add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        file: String,
        name: Option<String>,
        layer: Option<String>,
        origin: Option<[f32; 2]>,
        metres_per_pixel: Option<f32>,
        rotation_deg: Option<f32>,
        opacity: Option<f32>,
        elevation: Option<f32>,
        size_px: Option<[u32; 2]>,
    }
    let p: P = parse(params)?;
    let size_px = p
        .size_px
        .ok_or_else(|| bad("size_px is needed: the image's width and height in pixels"))?;
    let id = next_id("u", |id| project.underlays.iter().any(|u| u.id == id));
    let defaults = Placement::default();
    let underlay = Underlay {
        id: id.clone(),
        name: p.name.unwrap_or_else(|| stem(&p.file)),
        file: p.file,
        layer: p.layer.clone().unwrap_or_else(|| LAYER_UNDERLAY.into()),
        placement: Placement {
            origin: p.origin.unwrap_or(defaults.origin),
            metres_per_pixel: p.metres_per_pixel.unwrap_or(defaults.metres_per_pixel),
            rotation_deg: p.rotation_deg.unwrap_or(defaults.rotation_deg),
        },
        size_px,
        opacity: p.opacity.unwrap_or(0.5),
        elevation: p.elevation.unwrap_or(0.02),
        extensions: Default::default(),
    };
    underlay.validate().map_err(|e| bad(&e))?;
    match p.layer {
        Some(layer) => check_layer(project, &layer)?,
        None if !project.layers.iter().any(|l| l.id == LAYER_UNDERLAY) => {
            project.layers.push(Layer {
                id: LAYER_UNDERLAY.into(),
                name: "Underlay".into(),
                visible: true,
                context: false,
            });
        }
        None => {}
    }
    project.underlays.push(underlay);
    Ok(json!({ "id": id }))
}

fn set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: Option<String>,
        file: Option<String>,
        layer: Option<String>,
        origin: Option<[f32; 2]>,
        metres_per_pixel: Option<f32>,
        rotation_deg: Option<f32>,
        opacity: Option<f32>,
        elevation: Option<f32>,
        size_px: Option<[u32; 2]>,
    }
    let p: P = parse(params)?;
    if let Some(layer) = &p.layer {
        check_layer(project, layer)?;
    }
    let mut u = find(project, &p.id)?.clone();
    if let Some(v) = p.name {
        u.name = v;
    }
    if let Some(v) = p.file {
        u.file = v;
    }
    if let Some(v) = p.layer {
        u.layer = v;
    }
    if let Some(v) = p.origin {
        u.placement.origin = v;
    }
    if let Some(v) = p.metres_per_pixel {
        u.placement.metres_per_pixel = v;
    }
    if let Some(v) = p.rotation_deg {
        u.placement.rotation_deg = v;
    }
    if let Some(v) = p.opacity {
        u.opacity = v;
    }
    if let Some(v) = p.elevation {
        u.elevation = v;
    }
    if let Some(v) = p.size_px {
        u.size_px = v;
    }
    u.validate().map_err(|e| bad(&e))?;
    *find(project, &p.id)? = u;
    Ok(Value::Null)
}

fn calibrate(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        pixel_a: [f32; 2],
        pixel_b: [f32; 2],
        world_a: [f32; 2],
        world_b: [f32; 2],
    }
    let p: P = parse(params)?;
    let placement =
        Placement::calibrated(p.pixel_a, p.pixel_b, p.world_a, p.world_b).map_err(|e| bad(&e))?;
    let u = find(project, &p.id)?;
    let before = u.placement;
    u.placement = placement;
    if let Err(e) = u.validate() {
        u.placement = before;
        return Err(bad(&e));
    }
    Ok(json!({
        "origin": placement.origin,
        "metres_per_pixel": placement.metres_per_pixel,
        "rotation_deg": placement.rotation_deg,
    }))
}

fn scale_by_distance(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        pixel_a: [f32; 2],
        pixel_b: [f32; 2],
        distance_m: f32,
    }
    let p: P = parse(params)?;
    let u = find(project, &p.id)?;
    let placement = u
        .placement
        .scaled_by_distance(p.pixel_a, p.pixel_b, p.distance_m)
        .map_err(|e| bad(&e))?;
    let before = u.placement;
    u.placement = placement;
    if let Err(e) = u.validate() {
        u.placement = before;
        return Err(bad(&e));
    }
    Ok(json!({ "metres_per_pixel": placement.metres_per_pixel }))
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    find(project, id)?;
    project.underlays.retain(|u| u.id != id);
    Ok(Value::Null)
}
