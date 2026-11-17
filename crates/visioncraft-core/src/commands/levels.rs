//! Levels: steps and stairs (`crate::stairs`) and raised beds (`crate::bed`), shapes that change
//! height.

use super::shapes::{add, add_area};
use super::{fail, house::assign_defaults};
use crate::bed::{BedStyle, RaisedBed};
use crate::project::Project;
use crate::session::{CommandError, Session, parse};
use crate::shape::{Extrusion, Shape, shape_mesh};
use crate::spec::{ParamType as T, param, spec};
use crate::stairs::Stairs;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

type P2 = [f32; 2];

/// Default height of a raised bed's walls (metres).
const DEFAULT_BED_HEIGHT_M: f32 = 0.4;

/// Present and `null` (clear it) apart from absent (leave it).
fn double_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<f32>>, D::Error> {
    Option::<f32>::deserialize(d).map(Some)
}

pub(super) fn register(s: &mut Session) {
    let id = |what: &'static str| param("id", T::String, what).required();
    let sizes = || {
        vec![
            param("riser_m", T::Number, "Height of a riser to aim for, 0.05..0.30; the step count is round(height / riser_m) and the risers share the height evenly. Default 0.17.").unit("m"),
            param("tread_m", T::Number, "Depth of a tread, 0.15..0.60. A footprint longer than the steps need ends in a landing; a shorter one is warned about and the treads shrink to fit. Without it the treads share the footprint evenly."),
        ]
    };
    let mut add_params = vec![
        param("name", T::String, "Display name.").default(json!("Stairs")),
        param("from", T::Point2, "The bottom centre of the flight, [x, z] in metres.").required(),
        param("to", T::Point2, "The top centre of the flight, where you arrive, [x, z] in metres.").required(),
        param("width_m", T::Number, "Width of the flight, 0.2 and more.").unit("m").required(),
        param("base", T::Number, "Height of the bottom (on terrain, above the lowest ground under the footprint).").unit("m").default(json!(0)),
        param("height", T::Number, "Rise from the bottom to the top landing, 0.05..6. Left out it is the height of the ground at `to` (terrain and areas, such as a raised terrace) above the lowest ground under the stairs; refused when that is not clear.").unit("m"),
        param("layer", T::String, "Layer id.").default(json!("shapes")),
    ];
    add_params.extend(sizes());
    s.register_with_spec(
        spec(
            "stairs.add",
            "Add Stairs",
            "Adds a flight of solid steps (a closed extrusion on a rectangular footprint built from the bottom and top centres and a width). Every step is a block from the ground up, so none floats; the faces are `<id>:treads`, `<id>:risers` and `<id>:sides`. Checks Blondel's rule (2 x riser + tread between 0.60 and 0.65 m) and a footprint too short for the treads: these come back as `warnings` and are in the shape's mesh warnings. It starts with concrete where the project has none assigned.",
            add_params,
            "{id, steps, riser_m, tread_m, height_m, warnings}",
            json!({"name": "Garden steps", "from": [12, 5], "to": [12, 3.2], "width_m": 1.2, "height": 0.5}),
        ),
        add_stairs,
    );
    let mut set_params = vec![
        id("The stairs, or a closed 4-point rectangular block to turn into stairs (an extrusion)."),
        param("name", T::String, "Display name."),
        param("from", T::Point2, "New bottom centre."),
        param("to", T::Point2, "New top centre."),
        param("width_m", T::Number, "New width.").unit("m"),
        param(
            "up_edge",
            T::Integer,
            "Which outline edge (point n to n + 1) is the top. Moving from, to or width_m rebuilds the footprint with edge 2 on top.",
        ),
        param("base", T::Number, "New bottom height.").unit("m"),
        param("height", T::Number, "New rise.").unit("m"),
    ];
    set_params.extend(sizes());
    s.register_with_spec(
        spec(
            "stairs.set",
            "Set Stairs",
            "Changes any fields of a flight; the others stay. `riser_m` or `tread_m` set to null go back to their default. On a closed 4-point rectangular block that is not stairs yet it turns the block into stairs (`up_edge` says which edge is the top, default 0).",
            set_params,
            "{steps, riser_m, tread_m, height_m, warnings}",
            json!({"id": "S4", "height": 0.7}),
        ),
        set_stairs,
    );
    s.register_with_spec(
        spec(
            "stairs.clear",
            "Clear Stairs",
            "Turns stairs back into a plain block of the same footprint and height.",
            vec![id("The stairs.")],
            "null",
            json!({"id": "S4"}),
        ),
        clear_stairs,
    );

    let bed_fields = || {
        vec![
            param(
                "height",
                T::Number,
                "Height of the walls, 0.1..2. Default 0.4.",
            )
            .unit("m"),
            param("wall_thickness_m", T::Number, "Thickness of the walls: plain and masonry 0.05..0.3 (default 0.2), sleepers 0.1..0.2 (default 0.15, the beam's thickness), corten 0.005..0.02 (default 0.01; drawn at least 1 cm so it reads).").unit("m"),
            param("style", T::String, "The wall: plain (a solid timber ring, the default), masonry (brick with a coping slab), sleepers (stacked timber beams 0.2 m high with joints and overlapping corners) or corten (a thin weathering-steel plate with a folded top edge). `bed.set` with a new style gives the walls that style's material and default thickness.").allowed(&["plain", "masonry", "sleepers", "corten"]),
        ]
    };
    let mut bed_add = vec![
        param("name", T::String, "Display name.").default(json!("Raised bed")),
        param("outline", T::Outline, "The bed's outer outline as [x, z] pairs in metres, at least 3 points, not self-intersecting.").required(),
        param("layer", T::String, "Layer id.").default(json!("shapes")),
        param("material", T::String, "Material id for the walls (else the style's: timber, brick or corten).").default(json!(null)),
    ];
    bed_add.extend(bed_fields());
    s.register_with_spec(
        spec(
            "bed.add",
            "Add Raised Bed",
            "Adds a raised bed as one undoable step: a closed extrusion drawn as a hollow ring of walls with a rim (`<id>:wall`, `<id>:rim`; timber by default; see `style`) and an area named `<name> soil` inside it, 5 cm below the rim, with the soil material. The soil's id is kept on the bed (`soil`); plants and plantings on the soil stand on it, at the soil's height. The soil is flat at the height of the lowest ground under the bed.",
            bed_add,
            "{id, soil} (the walls' id and the soil area's id)",
            json!({"name": "Herb bed", "outline": [[12, 6], [14.4, 6], [14.4, 7.2], [12, 7.2]], "height": 0.4}),
        ),
        add_bed,
    );
    let mut bed_set = vec![
        id("The raised bed (the wall ring)."),
        param(
            "name",
            T::String,
            "Display name; the soil is renamed `<name> soil`.",
        ),
        param("outline", T::Outline, "New outer outline."),
    ];
    bed_set.extend(bed_fields());
    s.register_with_spec(
        spec(
            "bed.set",
            "Set Raised Bed",
            "Changes the height, wall thickness, outline or name of a raised bed and keeps its soil area in step (outline moved in by the wall, height 5 cm below the rim). A soil area that was removed is made again.",
            bed_set,
            "{soil}",
            json!({"id": "S4", "height": 0.6, "wall_thickness_m": 0.15}),
        ),
        set_bed,
    );
}

// --- Stairs ---------------------------------------------------------------------------------

/// The footprint of a flight from `from` (bottom centre) to `to` (top centre), `width` wide;
/// edge 2 is the top.
fn footprint(from: P2, to: P2, width: f32) -> Result<Vec<P2>, CommandError> {
    let len = (to[0] - from[0]).hypot(to[1] - from[1]);
    if ![from[0], from[1], to[0], to[1], width]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err(fail("`from`, `to` and `width_m` must be numbers"));
    }
    if len < 0.15 {
        return Err(fail(
            "`from` and `to` are too close: the flight needs 0.15 m or more",
        ));
    }
    if !(0.2..=20.0).contains(&width) {
        return Err(fail("`width_m` must be within 0.2..=20 m"));
    }
    let n = [
        -(to[1] - from[1]) / len * width / 2.0,
        (to[0] - from[0]) / len * width / 2.0,
    ];
    let at = |p: P2, k: f32| [p[0] + n[0] * k, p[1] + n[1] * k];
    Ok(vec![
        at(from, -1.0),
        at(from, 1.0),
        at(to, 1.0),
        at(to, -1.0),
    ])
}

/// The height of the surface at `(x, z)`: the terrain, or a flat or draped area lying over it
/// (with its lift), whichever is higher; 0 with neither.
fn surface_at(project: &Project, lifts: &std::collections::HashMap<String, f32>, p: P2) -> f32 {
    let terrain = project
        .terrain
        .as_deref()
        .and_then(|t| t.height_at(p[0], p[1]));
    project
        .shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Area(a) if crate::shape::inside_outline(p, &a.outline) => {
                let ground = if a.drape { terrain.unwrap_or(0.0) } else { 0.0 };
                Some(ground + a.elevation + lifts.get(&a.id).copied().unwrap_or(0.0))
            }
            _ => None,
        })
        .chain(terrain)
        .reduce(f32::max)
        .unwrap_or(0.0)
}

fn lowest_ground(project: &Project, outline: &[P2]) -> f32 {
    project.terrain.as_deref().map_or(0.0, |t| {
        crate::shape::footing(outline, &|x, z| t.height_at(x, z))
    })
}

/// The rise of a flight from `from` to `to` when none is given: the surface just past the top
/// over the lowest ground under the stairs and `base`.
fn derived_height(
    project: &Project,
    outline: &[P2],
    from: P2,
    to: P2,
    base: f32,
) -> Result<f32, CommandError> {
    let len = (to[0] - from[0]).hypot(to[1] - from[1]).max(1e-6);
    let beyond = [
        to[0] + (to[0] - from[0]) / len * 0.15,
        to[1] + (to[1] - from[1]) / len * 0.15,
    ];
    let lifts = crate::shape::area_lifts(&project.shapes)
        .into_iter()
        .collect();
    let h = surface_at(project, &lifts, beyond) - lowest_ground(project, outline) - base;
    let h = (h * 1000.0).round() / 1000.0;
    if h < 0.05 {
        return Err(fail(format!(
            "`height` is not given and the ground at `to` is not higher than the stairs' bottom ({h:.2} m); give `height`"
        )));
    }
    Ok(h)
}

fn stairs_materials(project: &mut Project, id: &str) {
    assign_defaults(
        project,
        id,
        &[
            ("treads", "concrete"),
            ("risers", "concrete"),
            ("sides", "concrete"),
        ],
    );
}

/// What `stairs.add` and `stairs.set` report on flight `id`.
fn stairs_report(project: &Project, id: &str) -> Value {
    let Some(Shape::Extrusion(e)) = project.shapes.iter().find(|s| s.id() == id) else {
        return Value::Null;
    };
    let (Some(st), Ok(mesh)) = (&e.stairs, shape_mesh(&Shape::Extrusion(e.clone()))) else {
        return Value::Null;
    };
    match st.geometry(&e.outline, e.height) {
        Ok(g) => json!({
            "steps": g.steps, "riser_m": g.riser, "tread_m": g.tread,
            "height_m": e.height, "warnings": mesh.warnings,
        }),
        Err(_) => Value::Null,
    }
}

fn add_stairs(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: Option<String>,
        from: P2,
        to: P2,
        width_m: f32,
        base: Option<f32>,
        height: Option<f32>,
        layer: Option<String>,
        riser_m: Option<f32>,
        tread_m: Option<f32>,
    }
    let p: P = parse(params)?;
    let outline = footprint(p.from, p.to, p.width_m)?;
    let base = p.base.unwrap_or(0.0);
    let height = match p.height {
        Some(h) => h,
        None => derived_height(project, &outline, p.from, p.to, base)?,
    };
    let r = add(project, (p.layer, None), |id, layer| {
        Shape::Extrusion(Extrusion {
            id,
            name: p.name.unwrap_or_else(|| "Stairs".into()),
            layer,
            outline,
            closed: true,
            base,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: Some(Stairs {
                up_edge: 2,
                riser_m: p.riser_m,
                tread_m: p.tread_m,
            }),
            raised_bed: None,
        })
    })?;
    let id = r["id"].as_str().expect("add returns an id").to_string();
    stairs_materials(project, &id);
    let mut out = stairs_report(project, &id);
    out["id"] = json!(id);
    Ok(out)
}

fn set_stairs(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: Option<String>,
        from: Option<P2>,
        to: Option<P2>,
        width_m: Option<f32>,
        up_edge: Option<usize>,
        base: Option<f32>,
        height: Option<f32>,
        #[serde(default, deserialize_with = "double_option")]
        riser_m: Option<Option<f32>>,
        #[serde(default, deserialize_with = "double_option")]
        tread_m: Option<Option<f32>>,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, true, |e| {
        if e.house.is_some() || e.raised_bed.is_some() {
            return Err(fail(format!(
                "`{}` is a house or a raised bed; make a plain block first",
                e.id
            )));
        }
        let had = e.stairs.is_some();
        let mut st = e.stairs.take().unwrap_or_else(|| Stairs::new(0));
        if let Some(u) = p.up_edge {
            st.up_edge = u;
        }
        if let Some(r) = p.riser_m {
            st.riser_m = r;
        }
        if let Some(t) = p.tread_m {
            st.tread_m = t;
        }
        if p.from.is_some() || p.to.is_some() || p.width_m.is_some() {
            // What is not given stays as the flight is now.
            let now = had
                .then(|| st.geometry(&e.outline, e.height).ok())
                .flatten();
            let from = p.from.or(now.as_ref().map(|g| g.origin));
            let to = p.to.or(now.as_ref().map(|g| {
                [
                    g.origin[0] + g.dir[0] * g.length,
                    g.origin[1] + g.dir[1] * g.length,
                ]
            }));
            let width = p.width_m.or(now.as_ref().map(|g| g.width));
            let (Some(from), Some(to), Some(width)) = (from, to, width) else {
                return Err(fail(
                    "`from`, `to` and `width_m` are all needed for a block that is not stairs yet",
                ));
            };
            e.outline = footprint(from, to, width)?;
            st.up_edge = 2;
        }
        e.stairs = Some(st);
        if let Some(n) = p.name {
            e.name = n;
        }
        if let Some(b) = p.base {
            e.base = b;
        }
        if let Some(h) = p.height {
            e.height = h;
        }
        Ok(())
    })?;
    stairs_materials(project, &p.id);
    Ok(stairs_report(project, &p.id))
}

fn clear_stairs(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| fail("`id`: stairs"))?;
    edit(project, id, false, |e| {
        e.stairs = None;
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Applies `f` to a copy of closed extrusion `id` and keeps it only if it still meshes. Stairs
/// are required unless `make`.
fn edit(
    project: &mut Project,
    id: &str,
    make: bool,
    f: impl FnOnce(&mut Extrusion) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == id)
        .ok_or_else(|| fail(format!("no shape `{id}`")))?;
    let mut shape = project.shapes[at].clone();
    let Shape::Extrusion(e) = &mut shape else {
        return Err(fail(format!("`{id}` is an area, not stairs")));
    };
    if !e.closed {
        return Err(fail(format!(
            "`{id}` is an open wall, not a block or stairs"
        )));
    }
    if e.stairs.is_none() && !make {
        return Err(fail(format!("`{id}` is not stairs; use stairs.set")));
    }
    f(e)?;
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    Ok(())
}

// --- Raised beds ----------------------------------------------------------------------------

fn check_bed(height: f32, bed: &RaisedBed) -> Result<(), CommandError> {
    if !(height.is_finite() && (0.1..=2.0).contains(&height)) {
        return Err(fail("`height` must be within 0.1..=2 m"));
    }
    bed.check().map_err(fail)
}

/// Gives the bed's walls the default material of its style, replacing the face assignments
/// there are when `replace`.
fn bed_materials(project: &mut Project, id: &str, style: BedStyle, replace: bool) {
    if replace {
        project.assignments.retain(|a| {
            !matches!(&a.target, crate::Target::Face { owner, face }
                if owner == id && (face == &format!("{id}:wall") || face == &format!("{id}:rim")))
        });
    }
    let m = style.default_material();
    assign_defaults(project, id, &[("wall", m), ("rim", m)]);
}

/// Puts the soil of raised bed `id` where it belongs: the outline moved in by the wall, flat at
/// the lowest ground under the bed plus the soil's height; made (and its id kept on the bed)
/// when the bed has none or it was removed. `rename` also renames it after the bed. Returns the
/// soil's id.
fn sync_soil(project: &mut Project, id: &str, rename: bool) -> Result<String, CommandError> {
    let Some(Shape::Extrusion(e)) = project.shapes.iter().find(|s| s.id() == id) else {
        return Err(fail(format!("no shape `{id}`")));
    };
    let Some(bed) = &e.raised_bed else {
        return Err(fail(format!("`{id}` is not a raised bed")));
    };
    let inner = bed.inner(&e.outline).map_err(fail)?;
    let elevation = lowest_ground(project, &e.outline) + e.base + RaisedBed::soil_top(e.height);
    let (name, layer) = (format!("{} soil", e.name), e.layer.clone());
    let existing = bed.soil.clone().filter(|s| {
        project
            .shapes
            .iter()
            .any(|x| x.id() == s && matches!(x, Shape::Area(_)))
    });
    let soil = match existing {
        Some(soil) => {
            if let Some(Shape::Area(a)) = project.shapes.iter_mut().find(|s| s.id() == soil) {
                a.outline = inner;
                a.elevation = elevation;
                a.drape = false;
                if rename {
                    a.name = name;
                }
            }
            soil
        }
        None => {
            let r = add_area(
                project,
                &json!({"name": name, "outline": inner, "elevation": elevation, "drape": false, "layer": layer}),
            )?;
            let soil = r["id"].as_str().expect("add returns an id").to_string();
            assign_defaults(project, &soil, &[("top", "soil")]);
            for s in &mut project.shapes {
                if let Shape::Extrusion(x) = s
                    && x.id == id
                    && let Some(b) = &mut x.raised_bed
                {
                    b.soil = Some(soil.clone());
                }
            }
            soil
        }
    };
    Ok(soil)
}

fn add_bed(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: Option<String>,
        outline: Vec<P2>,
        layer: Option<String>,
        material: Option<String>,
        height: Option<f32>,
        wall_thickness_m: Option<f32>,
        style: Option<BedStyle>,
    }
    let p: P = parse(params)?;
    let height = p.height.unwrap_or(DEFAULT_BED_HEIGHT_M);
    let style = p.style.unwrap_or_default();
    let bed = RaisedBed {
        wall_thickness_m: p.wall_thickness_m.unwrap_or(style.default_wall_m()),
        style,
        soil: None,
    };
    check_bed(height, &bed)?;
    let owned = p.material.is_some();
    let r = add(project, (p.layer, p.material), |id, layer| {
        Shape::Extrusion(Extrusion {
            id,
            name: p.name.unwrap_or_else(|| "Raised bed".into()),
            layer,
            outline: p.outline,
            closed: true,
            base: 0.0,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: None,
            raised_bed: Some(bed),
        })
    })?;
    let id = r["id"].as_str().expect("add returns an id").to_string();
    if !owned {
        bed_materials(project, &id, style, false);
    }
    let soil = sync_soil(project, &id, true)?;
    Ok(json!({"id": id, "soil": soil}))
}

fn set_bed(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: Option<String>,
        outline: Option<Vec<P2>>,
        height: Option<f32>,
        wall_thickness_m: Option<f32>,
        style: Option<BedStyle>,
    }
    let p: P = parse(params)?;
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == p.id)
        .ok_or_else(|| fail(format!("no shape `{}`", p.id)))?;
    let mut shape = project.shapes[at].clone();
    let Shape::Extrusion(e) = &mut shape else {
        return Err(fail(format!("`{}` is an area, not a raised bed", p.id)));
    };
    let Some(bed) = &mut e.raised_bed else {
        return Err(fail(format!("`{}` is not a raised bed; use bed.add", p.id)));
    };
    let restyled = p.style.filter(|s| *s != bed.style);
    if let Some(style) = restyled {
        bed.style = style;
        bed.wall_thickness_m = style.default_wall_m();
    }
    if let Some(t) = p.wall_thickness_m {
        bed.wall_thickness_m = t;
    }
    if let Some(h) = p.height {
        e.height = h;
    }
    if let Some(o) = p.outline {
        e.outline = o;
    }
    let renamed = p.name.is_some();
    if let Some(n) = p.name {
        e.name = n;
    }
    check_bed(e.height, bed)?;
    let style = bed.style;
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    if restyled.is_some() {
        bed_materials(project, &p.id, style, true);
    }
    let soil = sync_soil(project, &p.id, renamed)?;
    Ok(json!({"soil": soil}))
}

#[cfg(test)]
mod tests {
    use crate::{Project, Session, Shape};
    use serde_json::json;

    fn get(s: &Session, id: &str) -> crate::shape::Extrusion {
        match s.project().shapes.iter().find(|x| x.id() == id).unwrap() {
            Shape::Extrusion(e) => e.clone(),
            _ => panic!("not an extrusion"),
        }
    }

    fn area(s: &Session, id: &str) -> crate::shape::Area {
        match s.project().shapes.iter().find(|x| x.id() == id).unwrap() {
            Shape::Area(a) => a.clone(),
            _ => panic!("not an area"),
        }
    }

    fn stairs(s: &mut Session) -> String {
        s.run(
            "stairs.add",
            json!({"from": [10, 14], "to": [10, 12], "width_m": 1.2, "height": 0.51, "tread_m": 0.3}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn stairs_are_added_with_a_footprint_steps_and_materials_and_can_be_undone() {
        let mut s = Session::new(Project::default());
        let r = s
            .run(
                "stairs.add",
                json!({"from": [10, 14], "to": [10, 12], "width_m": 1.2, "height": 0.51, "tread_m": 0.3}),
            )
            .unwrap();
        assert_eq!(r["steps"], 3);
        assert!((r["riser_m"].as_f64().unwrap() - 0.17).abs() < 1e-5);
        assert!(r["warnings"].as_array().unwrap().is_empty(), "{r}");
        let e = get(&s, "S1");
        assert!(e.closed && e.stairs.as_ref().unwrap().up_edge == 2);
        assert_eq!(e.name, "Stairs");
        // The footprint is the flight's line and width; the top edge is at z = 12.
        let m = crate::shape_mesh(&s.project().shapes[0]).unwrap();
        let top = m.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!((top - 0.51).abs() < 1e-5);
        let (mut zlo, mut zhi, mut xlo, mut xhi) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &m.positions {
            zlo = zlo.min(p[2]);
            zhi = zhi.max(p[2]);
            xlo = xlo.min(p[0]);
            xhi = xhi.max(p[0]);
        }
        assert!((zlo - 12.0).abs() < 1e-4 && (zhi - 14.0).abs() < 1e-4);
        assert!((xlo - 9.4).abs() < 1e-4 && (xhi - 10.6).abs() < 1e-4);
        let mats: Vec<_> = s
            .project()
            .assignments
            .iter()
            .map(|a| a.material.as_str())
            .collect();
        assert_eq!(mats, ["concrete"; 3]);
        assert!(s.undo());
        assert!(s.project().shapes.is_empty() && s.project().assignments.is_empty());
    }

    #[test]
    fn the_height_comes_from_the_ground_when_it_is_left_out() {
        let mut s = Session::new(Project::default());
        let area = |s: &mut Session, y: f32, z0: f32, z1: f32| {
            s.run(
                "shape.add_area",
                json!({"name": "A", "outline": [[0, z0], [20, z0], [20, z1], [0, z1]], "elevation": y, "drape": false}),
            )
            .unwrap();
        };
        area(&mut s, 0.0, 12.0, 30.0);
        // A terrace at 0.5 m, 0.03 m of area lift on top of it (it touches the lawn).
        area(&mut s, 0.5, 9.0, 12.0);
        let r = s
            .run(
                "stairs.add",
                json!({"from": [10, 14], "to": [10, 12], "width_m": 1.2}),
            )
            .unwrap();
        let h = r["height_m"].as_f64().unwrap();
        assert!((h - 0.53).abs() < 1e-3, "{h}");
        // No ground above the bottom: refused with a reason.
        let err = s
            .run(
                "stairs.add",
                json!({"from": [10, 20], "to": [10, 18], "width_m": 1.2}),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("height"), "{err}");
    }

    #[test]
    fn set_changes_fields_moves_the_flight_and_turns_a_block_into_stairs() {
        let mut s = Session::new(Project::default());
        let id = stairs(&mut s);
        let r = s
            .run(
                "stairs.set",
                json!({"id": id, "height": 0.68, "riser_m": 0.17}),
            )
            .unwrap();
        assert_eq!(r["steps"], 4);
        assert_eq!(get(&s, &id).height, 0.68);
        // Moving the top keeps the width and the bottom.
        s.run("stairs.set", json!({"id": id, "to": [10, 11]}))
            .unwrap();
        let e = get(&s, &id);
        assert_eq!(e.outline.len(), 4);
        assert!(e.outline.iter().any(|p| (p[1] - 11.0).abs() < 1e-5));
        assert!(e.outline.iter().any(|p| (p[1] - 14.0).abs() < 1e-5));
        assert!((e.outline[1][0] - e.outline[0][0]).abs() > 1.19);
        // null gives the default back.
        s.run("stairs.set", json!({"id": id, "tread_m": null}))
            .unwrap();
        assert!(get(&s, &id).stairs.unwrap().tread_m.is_none());
        let err = s
            .run("stairs.set", json!({"id": id, "riser_m": 0.9}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("riser_m"), "{err}");
        assert!(s.run("stairs.set", json!({"id": id, "colour": 1})).is_err());
        assert!(s.undo());
        assert!(get(&s, &id).stairs.unwrap().tread_m.is_some());
        // A plain rectangular block becomes stairs.
        let b = s
            .run(
                "shape.add_extrusion",
                json!({"name": "Block", "outline": [[0, 0], [1, 0], [1, 2], [0, 2]], "height": 0.5}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        s.run("stairs.set", json!({"id": b, "up_edge": 2})).unwrap();
        assert!(get(&s, &b).stairs.is_some());
    }

    #[test]
    fn clear_turns_stairs_into_a_block_and_the_rest_is_refused() {
        let mut s = Session::new(Project::default());
        let id = stairs(&mut s);
        s.run("stairs.clear", json!({"id": id})).unwrap();
        let e = get(&s, &id);
        assert!(e.stairs.is_none() && e.height == 0.51);
        assert!(s.run("stairs.clear", json!({"id": id})).is_err());
        assert!(s.run("stairs.clear", json!({"id": "S9"})).is_err());
        assert!(s.undo());
        assert!(get(&s, &id).stairs.is_some());
        assert!(
            s.run(
                "stairs.add",
                json!({"from": [0, 0], "to": [0, 0.01], "width_m": 1.0, "height": 0.5})
            )
            .is_err()
        );
    }

    fn bed(s: &mut Session) -> (String, String) {
        let r = s
            .run(
                "bed.add",
                json!({"name": "Herbs", "outline": [[0, 0], [3, 0], [3, 1.2], [0, 1.2]]}),
            )
            .unwrap();
        (
            r["id"].as_str().unwrap().to_string(),
            r["soil"].as_str().unwrap().to_string(),
        )
    }

    #[test]
    fn a_bed_is_a_ring_and_a_soil_area_added_in_one_undoable_step() {
        let mut s = Session::new(Project::default());
        let (id, soil) = bed(&mut s);
        let e = get(&s, &id);
        assert_eq!(
            (e.height, e.raised_bed.as_ref().unwrap().wall_thickness_m),
            (0.4, 0.2)
        );
        assert_eq!(
            e.raised_bed.as_ref().unwrap().soil.as_deref(),
            Some(soil.as_str())
        );
        let a = area(&s, &soil);
        assert_eq!(a.name, "Herbs soil");
        assert!((a.elevation - 0.35).abs() < 1e-6 && !a.drape);
        // The soil is the outline moved in by the wall: 2.6 x 0.8.
        let xs: Vec<f32> = a.outline.iter().map(|p| p[0]).collect();
        assert!(
            xs.iter().any(|x| (x - 0.2).abs() < 1e-5) && xs.iter().any(|x| (x - 2.8).abs() < 1e-5)
        );
        let mats: Vec<_> = s
            .project()
            .assignments
            .iter()
            .map(|a| a.material.as_str())
            .collect();
        assert!(
            mats.contains(&"timber") && mats.contains(&"soil"),
            "{mats:?}"
        );
        assert!(s.undo());
        assert!(s.project().shapes.is_empty() && s.project().assignments.is_empty());
    }

    #[test]
    fn set_keeps_the_soil_in_step_and_removing_the_bed_removes_it() {
        let mut s = Session::new(Project::default());
        let (id, soil) = bed(&mut s);
        s.run(
            "bed.set",
            json!({"id": id, "height": 0.6, "wall_thickness_m": 0.1, "name": "Salad"}),
        )
        .unwrap();
        let a = area(&s, &soil);
        assert!((a.elevation - 0.55).abs() < 1e-6);
        assert_eq!(a.name, "Salad soil");
        assert!(a.outline.iter().any(|p| (p[0] - 0.1).abs() < 1e-5));
        s.run(
            "bed.set",
            json!({"id": id, "outline": [[0, 0], [4, 0], [4, 1.2], [0, 1.2]]}),
        )
        .unwrap();
        assert!(
            area(&s, &soil)
                .outline
                .iter()
                .any(|p| (p[0] - 3.9).abs() < 1e-5)
        );
        for bad in [
            json!({"id": id, "wall_thickness_m": 0.5}),
            json!({"id": id, "height": 5.0}),
            json!({"id": id, "outline": [[0, 0], [0.2, 0], [0.2, 0.2], [0, 0.2]], "wall_thickness_m": 0.3}),
        ] {
            assert!(s.run("bed.set", bad).is_err());
        }
        assert!(s.undo());
        assert_eq!(get(&s, &id).outline[1], [3.0, 0.0]);
        // A removed soil is made again.
        s.run("shape.remove", json!({"id": soil})).unwrap();
        let r = s.run("bed.set", json!({"id": id, "height": 0.5})).unwrap();
        assert_eq!(s.project().shapes.len(), 2);
        assert!(area(&s, r["soil"].as_str().unwrap()).name == "Salad soil");
        assert!((area(&s, r["soil"].as_str().unwrap()).elevation - 0.45).abs() < 1e-6);
        s.run("shape.remove", json!({"id": id})).unwrap();
        assert!(s.project().shapes.is_empty());
        assert!(s.run("bed.set", json!({"id": "S9"})).is_err());
    }

    #[test]
    fn bad_beds_are_refused_whole() {
        let mut s = Session::new(Project::default());
        for p in [
            json!({"outline": [[0, 0], [3, 0], [3, 0.5], [0, 0.5]], "wall_thickness_m": 0.3}),
            json!({"outline": [[0, 0], [3, 0], [3, 1], [0, 1]], "height": 0.01}),
            json!({"outline": [[0, 0], [3, 0]]}),
        ] {
            assert!(s.run("bed.add", p).is_err());
        }
        assert!(s.project().shapes.is_empty());
    }

    #[test]
    fn quantities_report_steps_and_soil() {
        let mut s = Session::new(Project::default());
        let st = stairs(&mut s);
        let (bed_id, _) = bed(&mut s);
        let q = s.run("query.quantities", json!({})).unwrap();
        let rows = q["shapes"].as_array().unwrap();
        let row = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap();
        let r = &row(&st)["stairs"];
        assert_eq!(r["steps"], 3);
        assert!((r["rise_m"].as_f64().unwrap() - 0.51).abs() < 1e-6);
        assert!((r["going_m"].as_f64().unwrap() - 0.3).abs() < 1e-5);
        let v = row(&st)["volume_m3"].as_f64().unwrap();
        assert!(
            (v - 1.2 * (0.17 * 0.3 + 0.34 * 0.3 + 0.51 * 1.4)).abs() < 1e-3,
            "{v}"
        );
        let b = &row(&bed_id)["raised_bed"];
        // Soil: 2.6 x 0.8 x 0.35 m.
        assert!(
            (b["soil_volume_m3"].as_f64().unwrap() - 2.6 * 0.8 * 0.35).abs() < 1e-3,
            "{b}"
        );
        // The wall's centre line: 2 x (2.8 + 1.0).
        assert!(
            (b["wall_length_m"].as_f64().unwrap() - 7.6).abs() < 1e-3,
            "{b}"
        );
        let ring = row(&bed_id)["volume_m3"].as_f64().unwrap();
        assert!(
            (ring - (3.0 * 1.2 - 2.6 * 0.8) * 0.4).abs() < 1e-3,
            "{ring}"
        );
    }

    #[test]
    fn bed_styles_set_the_wall_materials_and_report_their_quantities() {
        let mut s = Session::new(Project::default());
        let sleepers = s
            .run(
                "bed.add",
                json!({"outline": [[0, 0], [3, 0], [3, 1.2], [0, 1.2]], "style": "sleepers"}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let e = get(&s, &sleepers);
        let bed = e.raised_bed.as_ref().unwrap();
        assert_eq!(
            (bed.style, bed.wall_thickness_m),
            (crate::bed::BedStyle::Sleepers, 0.15)
        );
        let corten = s
            .run(
                "bed.add",
                json!({"outline": [[5, 0], [8, 0], [8, 1.2], [5, 1.2]], "style": "corten"}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let mats = |s: &Session, id: &str| -> Vec<String> {
            s.project()
                .assignments
                .iter()
                .filter(|a| a.target.owner() == id)
                .map(|a| a.material.clone())
                .collect()
        };
        assert_eq!(mats(&s, &corten), ["corten", "corten"]);
        assert!(s.project().materials.iter().any(|m| m.id == "corten"));
        let q = s.run("query.quantities", json!({})).unwrap();
        let rows = q["shapes"].as_array().unwrap();
        let row = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap()["raised_bed"].clone();
        assert_eq!(row(&sleepers)["beams"], 12);
        assert!((row(&sleepers)["beam_length_m"].as_f64().unwrap() - 15.6).abs() < 0.05);
        // Centre line 2 x (2.99 + 1.19) = 8.36 m, 0.4 m high.
        let plate = row(&corten)["plate_area_m2"].as_f64().unwrap();
        assert!((plate - 8.36 * 0.4).abs() < 0.01, "{plate}");
        assert!(row(&corten).get("beams").is_none());
        // A new style gives its material and thickness; the wrong thickness is refused.
        s.run("bed.set", json!({"id": sleepers, "style": "masonry"}))
            .unwrap();
        assert_eq!(mats(&s, &sleepers), ["brick", "brick"]);
        assert_eq!(get(&s, &sleepers).raised_bed.unwrap().wall_thickness_m, 0.2);
        assert!(
            s.run("bed.set", json!({"id": corten, "wall_thickness_m": 0.2}))
                .is_err()
        );
        assert!(
            s.run("bed.add", json!({"outline": [[0, 0], [3, 0], [3, 1], [0, 1]], "style": "sleepers", "wall_thickness_m": 0.3}))
                .is_err()
        );
        assert!(
            s.run(
                "bed.add",
                json!({"outline": [[0, 0], [3, 0], [3, 1], [0, 1]], "style": "bamboo"})
            )
            .is_err()
        );
        // The soil follows the plate's drawn thickness (1 cm), not the 6 mm asked.
        s.run("bed.set", json!({"id": corten, "wall_thickness_m": 0.006}))
            .unwrap();
        let soil = get(&s, &corten).raised_bed.unwrap().soil.unwrap();
        assert!(
            area(&s, &soil)
                .outline
                .iter()
                .any(|p| (p[0] - 5.01).abs() < 1e-5)
        );
        assert!(s.undo());
        assert_eq!(
            get(&s, &sleepers).raised_bed.unwrap().style,
            crate::bed::BedStyle::Masonry
        );
    }

    #[test]
    fn a_fence_can_be_made_in_one_material() {
        let mut s = Session::new(Project::default());
        let id = s
            .run("fence.add", json!({"line": [[0, 0], [4, 0]], "height": 1.8, "kind": "screen", "material": "metal"}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let a: Vec<_> = s
            .project()
            .assignments
            .iter()
            .map(|a| (a.target.owner().to_string(), a.material.clone()))
            .collect();
        assert_eq!(a, [(id, "metal".to_string())]);
    }
}
