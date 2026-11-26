//! Photos of facades on wall faces: add, change, remove.

use super::{fail, next_id};
use crate::material::Target;
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use crate::wall_photo::{Region, WallPhoto, image_corners};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

/// Present and `null` (clear it) apart from absent (leave it).
fn double_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<Region>>, D::Error> {
    Option::<Region>::deserialize(d).map(Some)
}

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "wall_photo.add",
            "Add Wall Photo",
            "Puts a photo of a facade (PNG or JPEG) on one wall face as its texture, straightened and scaled. `corners_px` are the facade's four corners as they stand in the photo (bottom-left, bottom-right, top-right, top-left as seen from outside): the photo is warped so that this four-sided shape fills `region`, a rectangle on the face in metres (u along the face to the right seen from outside, v up, from the face's lowest left point); the whole face when no region is given. Without `corners_px` the photo is taken as already straight (its own corners). The photo replaces the face's material inside the region, is lit like any surface and casts shadows like the wall. The host reads the image's size (`size_px`, as the photo is displayed: a JPEG's EXIF orientation is applied). A face that lies flat (a floor) or is curved takes no photo.",
            vec![
                param("file", T::String, "Image path relative to the project file, or `photos/<name>` inside a bundle.").required(),
                param("target", T::Object, "The face: {\"type\": \"face\", \"owner\": \"S1\", \"face\": \"S1:side(2)\"} (the faces are in `document.inspect`).").required(),
                param("corners_px", T::Array, "The facade's four corners in the photo, [[x, y], ...] in pixels as the photo is displayed (top-left of the image is 0, 0; y runs down): bottom-left, bottom-right, top-right, top-left as seen from outside. Default: the photo's own corners.").unit("px"),
                param("region", T::Object, "{u0, v0, u1, v1}: where on the face the facade lies, metres. Default: the whole face.").unit("m"),
                param("name", T::String, "Display name.").default(json!("the file's name")),
                param("size_px", T::Array, "[width, height] of the image in pixels as it is displayed (a JPEG's EXIF orientation is applied); filled in by the host when it can read the file, required otherwise.").unit("px"),
            ],
            "{id} (p1, p2...)",
            json!({"file": "facade.jpg", "target": {"type": "face", "owner": "S2", "face": "S2:side(2)"}, "corners_px": [[100, 700], [900, 650], [820, 150], [160, 100]], "size_px": [1000, 800]}),
        ),
        add,
    );
    s.register_with_spec(
        spec(
            "wall_photo.set",
            "Change Wall Photo",
            "Changes a wall photo; only the fields given change. `region: null` makes it the whole face again.",
            vec![
                param("id", T::String, "Wall photo id.").required(),
                param("name", T::String, "Display name."),
                param("file", T::String, "Another image file (give `size_px` too unless the host can read it)."),
                param("target", T::Object, "Another face."),
                param("corners_px", T::Array, "The facade's four corners in the photo, in pixels.").unit("px"),
                param("region", T::Object, "{u0, v0, u1, v1} in metres, or null for the whole face.").unit("m"),
                param("size_px", T::Array, "[width, height] of the image in pixels.").unit("px"),
            ],
            "null",
            json!({"id": "p1", "region": {"u0": 0.5, "v0": 0, "u1": 4.5, "v1": 2.8}}),
        ),
        set,
    );
    s.register_with_spec(
        spec(
            "wall_photo.remove",
            "Remove Wall Photo",
            "Removes a wall photo; the face shows its material again (the image file stays where it is).",
            vec![param("id", T::String, "Wall photo id.").required()],
            "null",
            json!({"id": "p1"}),
        ),
        remove,
    );
}

fn find<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut WallPhoto, CommandError> {
    project
        .wall_photos
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| fail(format!("no wall photo `{id}`")))
}

/// The file's name without folders and extension, for a default display name.
fn stem(file: &str) -> String {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_string()
}

fn check_owner(project: &Project, target: &Target) -> Result<(), CommandError> {
    let owner = target.owner();
    if project.models.iter().any(|m| m.id == owner)
        || project.shapes.iter().any(|s| s.id() == owner)
    {
        Ok(())
    } else {
        Err(fail(format!("no model or shape `{owner}`")))
    }
}

fn add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        file: String,
        target: Target,
        corners_px: Option<[[f32; 2]; 4]>,
        region: Option<Region>,
        name: Option<String>,
        size_px: Option<[u32; 2]>,
    }
    let p: P = parse(params)?;
    let size_px = p
        .size_px
        .ok_or_else(|| bad("size_px is needed: the image's width and height in pixels"))?;
    check_owner(project, &p.target)?;
    let id = next_id("p", |id| project.wall_photos.iter().any(|p| p.id == id));
    let photo = WallPhoto {
        id: id.clone(),
        name: p.name.unwrap_or_else(|| stem(&p.file)),
        file: p.file,
        size_px,
        target: p.target,
        corners_px: p.corners_px.unwrap_or_else(|| image_corners(size_px)),
        region: p.region,
        extensions: Default::default(),
    };
    photo.validate().map_err(|e| bad(&e))?;
    project.wall_photos.retain(|o| o.target != photo.target);
    project.wall_photos.push(photo);
    Ok(json!({ "id": id }))
}

fn set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: Option<String>,
        file: Option<String>,
        target: Option<Target>,
        corners_px: Option<[[f32; 2]; 4]>,
        #[serde(default, deserialize_with = "double_option")]
        region: Option<Option<Region>>,
        size_px: Option<[u32; 2]>,
    }
    let p: P = parse(params)?;
    if let Some(t) = &p.target {
        check_owner(project, t)?;
    }
    let mut w = find(project, &p.id)?.clone();
    if let Some(v) = p.name {
        w.name = v;
    }
    if let Some(v) = p.file {
        w.file = v;
    }
    if let Some(v) = p.target {
        w.target = v;
    }
    if let Some(v) = p.corners_px {
        w.corners_px = v;
    }
    if let Some(v) = p.region {
        w.region = v;
    }
    if let Some(v) = p.size_px {
        w.size_px = v;
    }
    w.validate().map_err(|e| bad(&e))?;
    // one photo a face: another on the same face is replaced
    project
        .wall_photos
        .retain(|o| o.id == w.id || o.target != w.target);
    *find(project, &p.id)? = w;
    Ok(Value::Null)
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    find(project, id)?;
    project.wall_photos.retain(|p| p.id != id);
    Ok(Value::Null)
}
