use super::{check_layer, fail, next_id};
use crate::material::{Assignment, Target};
use crate::project::{LAYER_SHAPES, Project};
use crate::session::{CommandError, Session, bad, parse};
use crate::shape::{Area, Extrusion, Shape, shape_mesh};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    let layer = || param("layer", T::String, "Layer id.").default(json!("shapes"));
    let material = || {
        param(
            "material",
            T::String,
            "Material id: also assigned to the whole new shape, in the same undo step.",
        )
    };
    let outline = |doc: &str| param("outline", T::Outline, doc).required();
    let curves = || {
        param(
            "curves",
            T::Object,
            "Arcs on single edges, by edge index: {\"0\": {\"type\": \"arc\", \"sagitta_m\": 0.8}}. The sagitta is the signed distance from the middle of the chord to the arc, positive to the left of the edge's direction (seen from above, +X east, -Z north), at most half the chord. See `shape.set_curve` to give a radius instead.",
        )
    };
    let smooth = || {
        param("smooth", T::Boolean, "A smooth curve through all the points (centripetal Catmull-Rom), closed or open as the shape is; edges with an arc keep it.").default(json!(false))
    };
    let edge_blend = || {
        param("edge_blend_m", T::Number, "Soft edge: the area gives way to what lies under it across a band this wide along its outline, dithered instead of a hard line (0: hard).").unit("m").default(json!(0))
    };
    s.register_with_spec(
        spec(
            "shape.add_area",
            "Add Area",
            "Draws a flat polygon (lawn, terrace, path, pond, bed) at a height; fails if the outline is self-intersecting or has fewer than 3 points.",
            vec![
                param("name", T::String, "Display name.").required(),
                outline("Control points as [x, z] pairs in metres, either winding, not self-intersecting."),
                curves(),
                smooth(),
                param("elevation", T::Number, "Height of the surface (above the ground when `drape`).").unit("m").default(json!(0)),
                param("drape", T::Boolean, "Lie on the ground: the surface follows the terrain, `elevation` is the height above it. False: flat at `elevation` (a raised deck, a pond's water). Default true, or false when `edge` is given (a draped area has no edge).").default(json!(true)),
                edge_blend(),
                param("edge", T::Object, "A retaining edge for a raised terrace: {style: plain|masonry|sleepers|corten, thickness_m?}. Same as `area.set_edge`."),
                layer(),
                material(),
            ],
            "{id} (S1, S2...; face names are `<id>:top`)",
            json!({"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "elevation": 0.0, "material": "lawn"}),
        ),
        add_area,
    );
    s.register_with_spec(
        spec(
            "shape.add_extrusion",
            "Add Extrusion",
            "Extrudes a closed polygon upwards (house volume, block), or an open polyline given a thickness (wall, fence, hedge block).",
            vec![
                param("name", T::String, "Display name.").required(),
                outline("Polygon (closed) or polyline (open) as [x, z] pairs in metres; at least 3 points closed, 2 open; not self-intersecting."),
                curves(),
                smooth(),
                param("closed", T::Boolean, "True: the last point joins the first. False: a polyline widened by `thickness`.").default(json!(true)),
                param("base", T::Number, "Height of the bottom.").unit("m").default(json!(0)),
                param("height", T::Number, "Height of the solid above `base`, above zero.").unit("m").required(),
                param("thickness", T::Number, "Width of an open extrusion; ignored when closed.").unit("m").default(json!(0.2)),
                layer(),
                material(),
                param("roof", T::Object, "A closed block's roof, making it a house: {kind: flat|gable|hip|lean_to, pitch_deg, overhang_m, parapet_m}; windows and doors then go in with shape.add_opening."),
            ],
            "{id}; faces `<id>:top`, `:bottom`, `:side(n)` (n = outline edge), and `:start`, `:end` when open",
            json!({"name": "House", "outline": [[2, 2], [8, 2], [8, 6], [2, 6]], "height": 3.0, "base": 0.0}),
        ),
        add_extrusion,
    );
    s.register_with_spec(
        spec(
            "shape.set_outline",
            "Set Outline",
            "Replaces the whole outline (the control points) of a shape; side materials are not renumbered (use insert/remove vertex for that). Arcs stay on their edge numbers where those edges still exist (eased to a semicircle where the new chord is too short for them).",
            vec![
                param("id", T::String, "Shape id.").required(),
                outline("New outline as [x, z] pairs in metres."),
            ],
            "null",
            json!({"id": "S2", "outline": [[2, 2], [9, 2], [9, 6], [2, 6]]}),
        ),
        set_outline,
    );
    s.register_with_spec(
        spec(
            "shape.move_vertex",
            "Move Vertex",
            "Moves one outline point (a control point, when the shape has curves); every face name stays. An arc on an edge of the point that would then bulge more than half its chord is eased to a semicircle.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param(
                    "index",
                    T::Integer,
                    "Index of the point in the outline, from 0.",
                )
                .required(),
                param("position", T::Point2, "New [x, z].")
                    .unit("m")
                    .required(),
            ],
            "null",
            json!({"id": "S2", "index": 2, "position": [9, 7]}),
        ),
        move_vertex,
    );
    s.register_with_spec(
        spec(
            "shape.insert_vertex",
            "Insert Vertex",
            "Splits an edge with a new point; the new point gets index edge+1, later sides renumber and their materials follow, both halves of the split edge keep its material. An arc on the split edge is split in two arcs on the same circle: the new point is projected onto the arc (kept off the ends) and each half gets the sagitta that keeps it on that circle; later arcs renumber with their edges. The answer has the point as placed.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("edge", T::Integer, "Index of the edge to split (edge n runs from point n to point n+1).").required(),
                param("position", T::Point2, "The new point [x, z].").unit("m").required(),
            ],
            "{position}",
            json!({"id": "S2", "edge": 0, "position": [5, 1.5]}),
        ),
        insert_vertex,
    );
    s.register_with_spec(
        spec(
            "shape.remove_vertex",
            "Remove Vertex",
            "Removes an outline point; its two edges merge into the earlier one (which keeps its material) and lose their arcs; later arcs renumber; refused if the shape would become invalid.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("index", T::Integer, "Index of the point, from 0.").required(),
            ],
            "null",
            json!({"id": "S2", "index": 3}),
        ),
        remove_vertex,
    );
    s.register_with_spec(
        spec(
            "shape.set_curve",
            "Curve an Edge",
            "Makes one edge of a shape an arc, typed as a radius (or a sagitta): edge `n` runs from point `n` to point `n+1`. The radius becomes the sagitta for the edge's length; a radius under half the edge is refused. Give the size as a radius, a sagitta or a point the arc passes through (`through`). Houses take no curves, and stairs only a curved front or back (`stairs.add` with arcs on the bottom or top edge). A fence follows the curve with posts spread by arc length.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("edge", T::Integer, "Edge index, from 0 (edge n runs from point n to point n+1, wrapping when closed).").required(),
                param("radius_m", T::Number, "Radius of the arc, at least half the edge's length.").unit("m"),
                param("sagitta_m", T::Number, "Instead of a radius: the signed distance from the middle of the chord to the arc, positive to the left of the edge's direction (seen from above, +X east, -Z north), at most half the chord. 0 makes the edge straight.").unit("m"),
                param("through", T::Point2, "Instead of either: a point [x, z] the arc passes through. It must lie between the ends of the edge (its foot on the chord inside the chord) and off the chord line, and the arc through it may not run past a semicircle; the side follows from where the point is.").unit("m"),
                param("side", T::String, "Which way the arc bulges, seen along the edge from its first point: `left` (the default for a radius) or `right`. With `sagitta_m` it overrides the sign; with `through` it is not allowed.").allowed(&["left", "right"]),
            ],
            "{sagitta_m, radius_m}",
            json!({"id": "S1", "edge": 0, "radius_m": 6.0, "side": "right"}),
        ),
        set_curve,
    );
    s.register_with_spec(
        spec(
            "shape.clear_curve",
            "Straighten an Edge",
            "Makes one curved edge straight again (an edge that is straight stays so).",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("edge", T::Integer, "Edge index, from 0.").required(),
            ],
            "null",
            json!({"id": "S1", "edge": 0}),
        ),
        clear_curve,
    );
    s.register_with_spec(
        spec(
            "shape.set_smooth",
            "Smooth Outline",
            "Turns the smooth outline of a shape on or off: a curve through all its points (centripetal Catmull-Rom, closed or open as the shape is). Edges with an arc keep their arc.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("smooth", T::Boolean, "True: smooth; false: straight edges between the points.").required(),
            ],
            "null",
            json!({"id": "S1", "smooth": true}),
        ),
        set_smooth,
    );
    s.register_with_spec(
        spec(
            "shape.add_circle",
            "Add Circle",
            "A round area or extrusion as 4 control points with 4 quarter arcs (an exact circle): a round lawn, pond or bed. Takes the other parameters of `shape.add_area` (kind `area`) or `shape.add_extrusion` (kind `extrusion`, with `height`).",
            vec![
                param("name", T::String, "Display name.").default(json!("Circle")),
                param("center", T::Point2, "Centre [x, z].").unit("m").required(),
                param("radius_m", T::Number, "Radius, above zero.").unit("m").required(),
                param("kind", T::String, "`area` (flat) or `extrusion` (solid).").allowed(&["area", "extrusion"]).default(json!("area")),
                param("elevation", T::Number, "Area: height of the surface.").unit("m"),
                param("drape", T::Boolean, "Area: lie on the ground."),
                edge_blend(),
                param("edge", T::Object, "Area: a retaining edge, as in `shape.add_area`."),
                param("base", T::Number, "Extrusion: height of the bottom.").unit("m"),
                param("height", T::Number, "Extrusion: height above `base`.").unit("m"),
                layer(),
                material(),
            ],
            "{id}",
            json!({"name": "Round lawn", "center": [11, 20], "radius_m": 6.0, "kind": "area", "material": "lawn"}),
        ),
        add_circle,
    );
    s.register_with_spec(
        spec(
            "shape.add_ellipse",
            "Add Ellipse",
            "An elliptical area or extrusion: an elliptical pond, lawn or bed. Drawn as control points on the ellipse with an arc between each pair (through the ellipse's point halfway between them), with as few as measure within 0.1% of the true area and perimeter (8 to 96 arcs). Takes the other parameters of `shape.add_area` (kind `area`) or `shape.add_extrusion` (kind `extrusion`, with `height`). The answer has the number of arcs, and the area and perimeter of the drawn outline.",
            vec![
                param("name", T::String, "Display name.").default(json!("Ellipse")),
                param("center", T::Point2, "Centre [x, z].").unit("m").required(),
                param("radius_x", T::Number, "Semi-axis along the x axis (before `rotation_deg`), above zero.").unit("m").required(),
                param("radius_z", T::Number, "Semi-axis along the z axis (before `rotation_deg`), above zero; at most 20 times (or at least a twentieth of) `radius_x`.").unit("m").required(),
                param("rotation_deg", T::Number, "Turns the x axis towards +Z, clockwise seen from above.").unit("deg").default(json!(0)),
                param("kind", T::String, "`area` (flat) or `extrusion` (solid).").allowed(&["area", "extrusion"]).default(json!("area")),
                param("elevation", T::Number, "Area: height of the surface.").unit("m"),
                param("drape", T::Boolean, "Area: lie on the ground."),
                edge_blend(),
                param("edge", T::Object, "Area: a retaining edge, as in `shape.add_area`."),
                param("base", T::Number, "Extrusion: height of the bottom.").unit("m"),
                param("height", T::Number, "Extrusion: height above `base`.").unit("m"),
                layer(),
                material(),
            ],
            "{id, arcs, area_m2, perimeter_m}",
            json!({"name": "Pond", "center": [11, 20], "radius_x": 4.0, "radius_z": 2.5, "rotation_deg": 20, "kind": "area", "material": "water"}),
        ),
        add_ellipse,
    );
    s.register_with_spec(
        spec(
            "shape.set_height",
            "Set Height",
            "Changes the heights of a shape: elevation for an area; base, height and thickness for an extrusion.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("base", T::Number, "Extrusion: height of the bottom.").unit("m"),
                param("height", T::Number, "Extrusion: height above `base`, above zero.").unit("m"),
                param("thickness", T::Number, "Open extrusion: width.").unit("m"),
                param("elevation", T::Number, "Area: height of the surface.").unit("m"),
                param("drape", T::Boolean, "Area: lie on the ground (follow the terrain, `elevation` above it) or be flat."),
            ],
            "null",
            json!({"id": "S2", "height": 4.5}),
        ),
        set_height,
    );
    s.register_with_spec(
        spec(
            "shape.drape_areas",
            "Lay Areas on the Ground",
            "Areas at height 0 that are flat start to lie on the ground (follow the terrain); what terrain.create does, for projects whose areas predate their terrain.",
            vec![],
            "{draped: integer}",
            json!({}),
        ),
        |p, _| Ok(json!({"draped": drape_areas(p)})),
    );
    s.register_with_spec(
        spec(
            "shape.remove",
            "Remove Shape",
            "Removes a shape and the material assignments made to it.",
            vec![param("id", T::String, "Shape id.").required()],
            "null",
            json!({"id": "S3"}),
        ),
        remove,
    );
    s.register_with_spec(
        spec(
            "shape.set_edge_blend",
            "Set Soft Edge",
            "Sets the width of an area's soft edge: the band along its outline where it dithers into what lies under it (a forest floor into the lawn). 0 gives it a hard edge again.",
            vec![
                param("id", T::String, "Area id.").required(),
                param("edge_blend_m", T::Number, "Band width, 0..10.").unit("m").required(),
            ],
            "null",
            json!({"id": "S1", "edge_blend_m": 1.5}),
        ),
        set_edge_blend,
    );
    s.register_with_spec(
        spec(
            "shape.rename",
            "Rename Shape",
            "Renames a shape.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("name", T::String, "New display name.").required(),
            ],
            "null",
            json!({"id": "S1", "name": "Front lawn"}),
        ),
        rename,
    );
}

pub(super) fn add(
    project: &mut Project,
    (layer, material): (Option<String>, Option<String>),
    make: impl FnOnce(String, String) -> Shape,
) -> Result<Value, CommandError> {
    let layer = layer.unwrap_or_else(|| LAYER_SHAPES.into());
    check_layer(project, &layer)?;
    if let Some(m) = &material
        && project.materials.iter().all(|x| &x.id != m)
    {
        return Err(fail(format!("no material `{m}`")));
    }
    let id = next_id("S", |id| project.shapes.iter().any(|s| s.id() == id));
    let shape = make(id.clone(), layer);
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes.push(shape);
    if let Some(material) = material {
        project.assignments.push(Assignment {
            target: Target::Owner { owner: id.clone() },
            material,
        });
    }
    Ok(json!({ "id": id }))
}

pub(super) fn add_area(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        outline: Vec<[f32; 2]>,
        #[serde(default)]
        curves: crate::curve::Curves,
        #[serde(default)]
        smooth: bool,
        #[serde(default)]
        elevation: f32,
        drape: Option<bool>,
        #[serde(default)]
        edge_blend_m: f32,
        edge: Option<crate::bed::AreaEdge>,
        layer: Option<String>,
        material: Option<String>,
    }
    let p: P = parse(params)?;
    check_edge_blend(p.edge_blend_m)?;
    // A terrace with an edge stands at its own height; without one it lies on the ground.
    let drape = p.drape.unwrap_or(p.edge.is_none());
    let owned = p.material.is_some();
    let r = add(project, (p.layer, p.material), |id, layer| {
        Shape::Area(Area {
            id,
            name: p.name,
            layer,
            points: p.outline,
            curves: p.curves,
            smooth: p.smooth,
            elevation: p.elevation,
            drape,
            edge_blend_m: p.edge_blend_m,
            edge: p.edge,
            terrace: None,
            extensions: Default::default(),
        })
    })?;
    if let (Some(edge), false) = (p.edge, owned) {
        let id = r["id"].as_str().expect("add returns an id");
        super::levels::edge_materials(project, id, edge.style, false);
    }
    Ok(r)
}

pub(super) fn add_extrusion(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        outline: Vec<[f32; 2]>,
        #[serde(default)]
        curves: crate::curve::Curves,
        #[serde(default)]
        smooth: bool,
        #[serde(default = "yes")]
        closed: bool,
        #[serde(default)]
        base: f32,
        height: f32,
        #[serde(default = "thin")]
        thickness: f32,
        layer: Option<String>,
        material: Option<String>,
        /// A closed block's roof (`crate::house::Roof`): it becomes a house.
        roof: Option<crate::house::Roof>,
    }
    fn yes() -> bool {
        true
    }
    fn thin() -> f32 {
        0.2
    }
    let p: P = parse(params)?;
    let house = match (p.roof, p.closed) {
        (Some(roof), true) => Some(crate::house::House {
            roof,
            ..Default::default()
        }),
        (Some(_), false) => return Err(fail("only a closed block takes a roof")),
        (None, _) => None,
    };
    let roofed = house.is_some();
    let r = add(project, (p.layer, p.material), |id, layer| {
        Shape::Extrusion(Extrusion {
            id,
            name: p.name,
            layer,
            points: p.outline,
            curves: p.curves,
            smooth: p.smooth,
            closed: p.closed,
            base: p.base,
            height: p.height,
            thickness: p.thickness,
            extensions: Default::default(),
            house,
            fence: None,
            stairs: None,
            raised_bed: None,
            furniture: None,
        })
    })?;
    if roofed && let Some(id) = r["id"].as_str() {
        super::house::default_materials(project, id);
    }
    Ok(r)
}

/// Applies `f` to a copy of the shape and keeps it only if it still meshes.
fn edit(
    project: &mut Project,
    id: &str,
    f: impl FnOnce(&mut Shape) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == id)
        .ok_or_else(|| fail(format!("no shape `{id}`")))?;
    let mut shape = project.shapes[at].clone();
    f(&mut shape)?;
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    Ok(())
}

/// A flight of stairs that turns has an outline derived from its `path`: changing points by hand
/// would part the two.
fn refuse_turned(s: &Shape) -> Result<(), CommandError> {
    match s {
        Shape::Extrusion(e) if e.stairs.as_ref().is_some_and(|st| st.path.is_some()) => Err(bad(
            "the outline of a flight that turns comes from its path; change it with stairs.set and `path` or `via`",
        )),
        _ => Ok(()),
    }
}

fn set_outline(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        outline: Vec<[f32; 2]>,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        refuse_turned(s)?;
        *s.points_mut() = p.outline;
        fit_curves(s);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn move_vertex(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        index: usize,
        position: [f32; 2],
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        refuse_turned(s)?;
        *s.points_mut()
            .get_mut(p.index)
            .ok_or_else(|| bad("no such vertex"))? = p.position;
        fit_curves(s);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Renumbers the `side(n)` assignments of shape `id`: `f(n)` gives the new indices of side `n`
/// (none drops the assignment, several copy it).
fn remap_sides(project: &mut Project, id: &str, f: impl Fn(usize) -> Vec<usize>) {
    let prefix = format!("{id}:side(");
    let old = std::mem::take(&mut project.assignments);
    for a in old {
        let n = match &a.target {
            Target::Face { owner, face } if owner == id => face
                .strip_prefix(&prefix)
                .and_then(|r| r.strip_suffix(')'))
                .and_then(|n| n.parse::<usize>().ok()),
            _ => None,
        };
        let Some(n) = n else {
            project.assignments.push(a);
            continue;
        };
        for m in f(n) {
            let mut moved = a.clone();
            moved.target = Target::Face {
                owner: id.into(),
                face: format!("{prefix}{m})"),
            };
            project.assignments.push(moved);
        }
    }
    // a wall photo follows its side the same way (one per face: the first copy stays)
    let old = std::mem::take(&mut project.wall_photos);
    for p in old {
        let n = match &p.target {
            Target::Face { owner, face } if owner == id => face
                .strip_prefix(&prefix)
                .and_then(|r| r.strip_suffix(')'))
                .and_then(|n| n.parse::<usize>().ok()),
            _ => None,
        };
        let Some(n) = n else {
            project.wall_photos.push(p);
            continue;
        };
        for m in f(n).into_iter().take(1) {
            let mut moved = p.clone();
            moved.target = Target::Face {
                owner: id.into(),
                face: format!("{prefix}{m})"),
            };
            project.wall_photos.push(moved);
        }
    }
}

fn insert_vertex(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        edge: usize,
        position: [f32; 2],
    }
    let p: P = parse(params)?;
    let mut placed = p.position;
    edit(project, &p.id, |s| {
        refuse_turned(s)?;
        let edges = crate::curve::edge_count(s.points().len(), s.is_closed());
        if p.edge >= edges {
            return Err(bad("no such edge"));
        }
        let points = s.points().to_vec();
        let (a, b) = (points[p.edge], points[(p.edge + 1) % points.len()]);
        // An arc is split in two on its own circle, the new point projected onto it.
        let split = match s.curves().get(&p.edge) {
            Some(c) => {
                Some(crate::curve::split_arc(a, b, c.sagitta_m(), p.position).map_err(fail)?)
            }
            None => None,
        };
        let position = split.map_or(p.position, |(q, _, _)| q);
        s.points_mut().insert(p.edge + 1, position);
        let curves = crate::curve::after_insert(s.curves(), p.edge, split.map(|(_, x, y)| (x, y)));
        *s.curves_mut() = curves;
        placed = position;
        Ok(())
    })?;
    // The split edge keeps its material on both halves.
    remap_sides(project, &p.id, |n| match n.cmp(&p.edge) {
        std::cmp::Ordering::Less => vec![n],
        std::cmp::Ordering::Equal => vec![n, n + 1],
        std::cmp::Ordering::Greater => vec![n + 1],
    });
    Ok(json!({"position": placed}))
}

fn remove_vertex(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        index: usize,
    }
    let p: P = parse(params)?;
    let mut last_of_open = false;
    edit(project, &p.id, |s| {
        refuse_turned(s)?;
        let len = s.points().len();
        if p.index >= len {
            return Err(bad("no such vertex"));
        }
        last_of_open = !s.is_closed() && p.index == len - 1;
        let curves = crate::curve::after_remove(s.curves(), p.index, len, s.is_closed());
        s.points_mut().remove(p.index);
        *s.curves_mut() = curves;
        Ok(())
    })?;
    // Edges `index-1` and `index` merge into one that keeps the number `index-1` and its material;
    // the material of edge `index` goes. Removing the end of an open line drops its one edge.
    let dropped = if last_of_open { p.index - 1 } else { p.index };
    remap_sides(project, &p.id, |n| match n.cmp(&dropped) {
        std::cmp::Ordering::Less => vec![n],
        std::cmp::Ordering::Equal => vec![],
        std::cmp::Ordering::Greater => vec![n - 1],
    });
    Ok(Value::Null)
}

/// Keeps the arcs of `s` valid after its points changed.
fn fit_curves(s: &mut Shape) {
    let (points, closed) = (s.points().to_vec(), s.is_closed());
    crate::curve::fit(&points, closed, s.curves_mut());
}

fn set_curve(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        edge: usize,
        radius_m: Option<f32>,
        sagitta_m: Option<f32>,
        through: Option<[f32; 2]>,
        side: Option<String>,
    }
    let p: P = parse(params)?;
    let left = match p.side.as_deref() {
        None | Some("left") => true,
        Some("right") => false,
        Some(_) => return Err(bad("`side` is `left` or `right`")),
    };
    if usize::from(p.radius_m.is_some())
        + usize::from(p.sagitta_m.is_some())
        + usize::from(p.through.is_some())
        != 1
    {
        return Err(bad("give one of `radius_m`, `sagitta_m` or `through`"));
    }
    if p.through.is_some() && p.side.is_some() {
        return Err(bad(
            "`side` goes with `radius_m` or `sagitta_m`; the side of `through` follows from the point",
        ));
    }
    let mut report = Value::Null;
    edit(project, &p.id, |s| {
        let (points, closed) = (s.points().to_vec(), s.is_closed());
        let edges = crate::curve::edge_count(points.len(), closed);
        if p.edge >= edges {
            return Err(fail(format!(
                "no edge {}: the shape has {edges} edges (0..{})",
                p.edge,
                edges.saturating_sub(1)
            )));
        }
        let (a, b) = (points[p.edge], points[(p.edge + 1) % points.len()]);
        let chord = (b[0] - a[0]).hypot(b[1] - a[1]);
        let curve = match (p.radius_m, p.sagitta_m) {
            _ if p.through.is_some() => crate::curve::Curve::Arc {
                sagitta_m: crate::curve::sagitta_through(a, b, p.through.expect("checked"))
                    .map_err(fail)?,
            },
            (Some(r), _) => crate::curve::Curve::from_radius(chord, r, left).map_err(fail)?,
            (_, Some(v)) => {
                let v = if p.side.is_some() {
                    v.abs() * if left { 1.0 } else { -1.0 }
                } else {
                    v
                };
                crate::curve::Curve::Arc { sagitta_m: v }
            }
            _ => unreachable!("one of the three is given"),
        };
        if !curve.sagitta_m().is_finite() {
            return Err(bad("the sagitta is not a number"));
        }
        if curve.sagitta_m().abs() > chord / 2.0 * (1.0 + 1e-4) {
            return Err(fail(format!(
                "a sagitta of {} m is more than half the edge ({:.3} m): an arc reaches at most a semicircle",
                curve.sagitta_m().abs(),
                chord / 2.0
            )));
        }
        if curve.sagitta_m().abs() < 1e-4 {
            s.curves_mut().remove(&p.edge);
        } else {
            s.curves_mut().insert(p.edge, curve);
            report = json!({"sagitta_m": curve.sagitta_m(), "radius_m": curve.radius_m(chord)});
        }
        Ok(())
    })?;
    Ok(report)
}

fn clear_curve(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        edge: usize,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        s.curves_mut().remove(&p.edge);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn set_smooth(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        smooth: bool,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        s.set_smooth(p.smooth);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn add_circle(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        center: [f32; 2],
        radius_m: f32,
        kind: Option<String>,
    }
    let p: P = parse(&json!({
        "center": params["center"], "radius_m": params["radius_m"], "kind": params["kind"],
    }))?;
    if !(p.radius_m.is_finite() && p.radius_m > 0.0 && p.center.iter().all(|c| c.is_finite())) {
        return Err(bad("`center` and a positive `radius_m` are needed"));
    }
    let [cx, cz] = p.center;
    let r = p.radius_m;
    // East, south, west, north: clockwise seen from above, so the left of every edge is outward
    // and a positive sagitta bulges out.
    let outline = [[cx + r, cz], [cx, cz + r], [cx - r, cz], [cx, cz - r]];
    let sagitta = r * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
    let arc = json!({"type": "arc", "sagitta_m": sagitta});
    let mut q = params
        .as_object()
        .cloned()
        .ok_or_else(|| bad("expected an object"))?;
    for k in ["center", "radius_m", "kind"] {
        q.remove(k);
    }
    q.insert("outline".into(), json!(outline));
    q.insert(
        "curves".into(),
        json!({"0": arc, "1": arc, "2": arc, "3": arc}),
    );
    if q.get("name").is_none_or(Value::is_null) {
        q.insert("name".into(), "Circle".into());
    }
    match p.kind.as_deref() {
        None | Some("area") => add_area(project, &Value::Object(q)),
        Some("extrusion") => add_extrusion(project, &Value::Object(q)),
        Some(_) => Err(bad("`kind` is `area` or `extrusion`")),
    }
}

fn add_ellipse(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        center: [f32; 2],
        radius_x: f32,
        radius_z: f32,
        rotation_deg: Option<f32>,
        kind: Option<String>,
    }
    let p: P = parse(&json!({
        "center": params["center"], "radius_x": params["radius_x"],
        "radius_z": params["radius_z"], "rotation_deg": params["rotation_deg"],
        "kind": params["kind"],
    }))?;
    let rotation = p.rotation_deg.unwrap_or(0.0);
    let ratio = crate::curve::MAX_ELLIPSE_RATIO;
    if !(p.radius_x.is_finite()
        && p.radius_z.is_finite()
        && p.radius_x > 0.0
        && p.radius_z > 0.0
        && rotation.is_finite()
        && p.center.iter().all(|c| c.is_finite()))
    {
        return Err(bad(
            "`center`, a positive `radius_x` and a positive `radius_z` are needed",
        ));
    }
    if p.radius_x / p.radius_z > ratio || p.radius_z / p.radius_x > ratio {
        return Err(bad(&format!(
            "`radius_x` and `radius_z` may differ by at most a factor of {ratio}"
        )));
    }
    let (outline, curves) = crate::curve::ellipse(p.center, p.radius_x, p.radius_z, rotation);
    let tess = crate::curve::tessellate(&outline, true, &curves, false);
    let mut q = params
        .as_object()
        .cloned()
        .ok_or_else(|| bad("expected an object"))?;
    for k in ["center", "radius_x", "radius_z", "rotation_deg", "kind"] {
        q.remove(k);
    }
    q.insert("outline".into(), json!(outline));
    q.insert(
        "curves".into(),
        Value::Object(
            curves
                .iter()
                .map(|(k, c)| (k.to_string(), json!(c)))
                .collect(),
        ),
    );
    if q.get("name").is_none_or(Value::is_null) {
        q.insert("name".into(), "Ellipse".into());
    }
    let mut answer = match p.kind.as_deref() {
        None | Some("area") => add_area(project, &Value::Object(q))?,
        Some("extrusion") => add_extrusion(project, &Value::Object(q))?,
        Some(_) => return Err(bad("`kind` is `area` or `extrusion`")),
    };
    answer["arcs"] = curves.len().into();
    answer["area_m2"] = crate::measure::polygon_area(&tess.pts).into();
    let n = tess.pts.len();
    let perimeter: f32 = (0..n)
        .map(|i| {
            let (a, b) = (tess.pts[i], tess.pts[(i + 1) % n]);
            (b[0] - a[0]).hypot(b[1] - a[1])
        })
        .sum();
    answer["perimeter_m"] = perimeter.into();
    Ok(answer)
}

fn set_height(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        base: Option<f32>,
        height: Option<f32>,
        thickness: Option<f32>,
        elevation: Option<f32>,
        drape: Option<bool>,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        match s {
            Shape::Area(a) => {
                if p.base.or(p.height).or(p.thickness).is_some() {
                    return Err(bad("an area has only an elevation"));
                }
                a.elevation = p.elevation.unwrap_or(a.elevation);
                a.drape = p.drape.unwrap_or(a.drape);
            }
            Shape::Extrusion(e) => {
                if p.elevation.is_some() || p.drape.is_some() {
                    return Err(bad("an extrusion has base, height and thickness"));
                }
                e.base = p.base.unwrap_or(e.base);
                e.height = p.height.unwrap_or(e.height);
                e.thickness = p.thickness.unwrap_or(e.thickness);
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn check_edge_blend(m: f32) -> Result<(), CommandError> {
    if (0.0..=crate::shape::MAX_EDGE_BLEND_M).contains(&m) {
        Ok(())
    } else {
        Err(bad("edge_blend_m: 0..10"))
    }
}

fn set_edge_blend(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        edge_blend_m: f32,
    }
    let p: P = parse(params)?;
    check_edge_blend(p.edge_blend_m)?;
    edit(project, &p.id, |s| match s {
        Shape::Area(a) => {
            a.edge_blend_m = p.edge_blend_m;
            Ok(())
        }
        Shape::Extrusion(_) => Err(bad("only an area has a soft edge")),
    })?;
    Ok(Value::Null)
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    let before = project.shapes.len();
    // A raised bed takes its soil with it.
    let soil = project.shapes.iter().find_map(|s| match s {
        Shape::Extrusion(e) if e.id == id => e.raised_bed.as_ref().and_then(|b| b.soil.clone()),
        _ => None,
    });
    project.shapes.retain(|s| s.id() != id);
    if project.shapes.len() == before {
        return Err(fail(format!("no shape `{id}`")));
    }
    project.assignments.retain(|a| a.target.owner() != id);
    project.wall_photos.retain(|p| p.target.owner() != id);
    if let Some(soil) = soil {
        project.shapes.retain(|s| s.id() != soil);
        project.assignments.retain(|a| a.target.owner() != soil);
    }
    Ok(Value::Null)
}

fn rename(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: String,
    }
    let p: P = parse(params)?;
    let s = project
        .shapes
        .iter_mut()
        .find(|s| s.id() == p.id)
        .ok_or_else(|| fail(format!("no shape `{}`", p.id)))?;
    *s.name_mut() = p.name;
    Ok(Value::Null)
}

pub(crate) fn yes() -> bool {
    true
}

/// Areas at height 0 that are flat start to lie on the ground; returns how many.
pub(crate) fn drape_areas(project: &mut Project) -> usize {
    let mut n = 0;
    for s in &mut project.shapes {
        if let Shape::Area(a) = s
            && !a.drape
            && a.elevation == 0.0
        {
            a.drape = true;
            n += 1;
        }
    }
    n
}
