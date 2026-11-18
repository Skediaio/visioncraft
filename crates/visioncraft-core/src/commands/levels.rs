//! Levels: steps and stairs (`crate::stairs`) and raised beds (`crate::bed`), shapes that change
//! height.

use super::shapes::{add, add_area};
use super::{fail, house::assign_defaults};
use crate::bed::{BedStyle, RaisedBed};
use crate::project::Project;
use crate::session::{CommandError, Session, parse};
use crate::setin::SetIn;
use crate::shape::{Extrusion, Shape, shape_mesh};
use crate::spec::{ParamType as T, param, spec};
use crate::stairs::Stairs;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

type P2 = [f32; 2];

/// Default height of a raised bed's walls (metres).
const DEFAULT_BED_HEIGHT_M: f32 = 0.4;

const SET_IN_HELP: &str = "Sets the flight into the slope it climbs: {cheek_style, thickness_m?, coping_m?}; `null` (stairs.set) takes it out. The step count is still round(height / riser_m), but each riser stands where the original ground (terrain and terraces, along the centreline) reaches the level of the step before, so a flat stretch becomes a long tread and a steep one a short one; each going is held within 0.25..2 m, and a going outside 0.28..0.45 m is warned about when Blondel's rule fails, or when it was held. A flight that turns does this per flight (the steps shared in proportion to the ground each climbs; landings as before). The flight's footprint, widened at the sides by the cheeks' thickness, is cut out of the terrain like a terrace's, and the ground inside it is the treads' level (nothing grows on the steps). Cheek walls stand along both sides from below the steps to the ground beside them plus `coping_m` (0..1, default 0.1) where they hold earth, and just under the treads where the ground is lower; faces `<id>:cheeks` and `<id>:cheeks_rim`. `cheek_style`: plain (default), masonry, sleepers, corten, gabion or none (no cheeks: the terrain is cut and a bank beside the steps is not held, which is warned about). `thickness_m`: the style's own when left out (plain and masonry 0.05..0.3, sleepers 0.1..0.2, corten 0.005..0.02, gabion 0.2..0.6). With `set_in` and no `height`, the rise is the ground at the top minus the lowest ground under the flight.";

/// A list that is present, `null` (clear it) or absent (leave it).
fn double_list<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<Vec<P2>>>, D::Error> {
    Option::<Vec<P2>>::deserialize(d).map(Some)
}

/// Present and `null` (clear it) apart from absent (leave it).
fn double_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<f32>>, D::Error> {
    Option::<f32>::deserialize(d).map(Some)
}

/// A set-in object that is present, `null` (clear it) or absent (leave it).
fn double_set_in<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<SetIn>>, D::Error> {
    Option::<SetIn>::deserialize(d).map(Some)
}

pub(super) fn register(s: &mut Session) {
    let id = |what: &'static str| param("id", T::String, what).required();
    let sizes = || {
        vec![
            param("riser_m", T::Number, "Height of a riser to aim for, 0.05..0.30; the step count is round(height / riser_m) and the risers share the height evenly. Default 0.17.").unit("m"),
            param("tread_m", T::Number, "Depth of a tread, 0.15..0.60. A footprint longer than the steps need ends in a landing; a shorter one is warned about and the treads shrink to fit. Without it the treads share the footprint evenly. Not used by a flight set into the slope (`set_in`): its goings follow the ground."),
            param("set_in", T::Object, SET_IN_HELP),
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
        param("via", T::Outline, "One or two corner points [[x, z]] between `from` and `to`: the flight turns 90 degrees at each on a square landing of width x width centred on the corner (an L with one corner, a U or S with two). Segments must meet at right angles (within 1 degree). With two corners at least one width apart (exactly one width: a half-landing, the flights side by side with no gap; more: a wider landing with a well between the flights; less is refused). The steps share the height in proportion to each flight's clear run (the segment less half a landing at each corner end); a landing lies at the height of the last step reaching it. `to` is then the top centre of the last flight."),
    ];
    add_params.push(param("front_bow_m", T::Number, "A curved front: the bottom edge becomes an arc that bulges this far (metres, from its chord) away from the flight, a round front or bullnose first step; negative makes it hollow, into the flight; 0 straightens it. Every riser is an arc concentric with it (the step is cut by the arcs and the flight's two sides), the goings counted from the arc's apex, so the first step is the widest. At most half the width (a semicircle); the arc of the top step must stay wider than the flight, which asks for a radius larger than the flight's run plus half its width. The shape's `curves` has it on the bottom edge (edge 0 for a flight made here; also `shape.set_curve`); a flight that turns or is set into the slope has no curved front.").unit("m"));
    add_params.extend(sizes());
    s.register_with_spec(
        spec(
            "stairs.add",
            "Add Stairs",
            "Adds a flight of solid steps (a closed extrusion on a rectangular footprint built from the bottom and top centres and a width). Every step is a block from the ground up, so none floats: on uneven terrain each block goes down to the lowest ground under its own footprint; the faces are `<id>:treads`, `<id>:risers` and `<id>:sides`. Checks Blondel's rule (2 x riser + tread between 0.60 and 0.65 m) and a footprint too short for the treads: these come back as `warnings` and are in the shape's mesh warnings. With `via` the flight turns (L, U or S; see `via`): its outline is derived from `from`, `via`, `to` and `width_m`, so the vertex commands refuse it (use stairs.set with `path` or `via`). It starts with concrete where the project has none assigned.",
            add_params,
            "{id, steps, riser_m, tread_m, height_m, flights: [{steps, run_m}], landings: [{size_m: [width, length], height_m}], warnings}",
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
        param(
            "via",
            T::Outline,
            "New corner points (one or two, see stairs.add) between the bottom and the top centre; `null` makes the flight straight again, from its first to its last point. Left out, a flight that turns keeps its corners.",
        ),
        param(
            "path",
            T::Outline,
            "The whole centreline of a flight that turns, 2 (straight), 3 or 4 points: bottom centre, corners, top centre. Not with `via`, `from` or `to`.",
        ),
    ];
    set_params.push(param("front_bow_m", T::Number, "A curved front: the bottom edge becomes an arc that bulges this far (metres, from its chord) away from the flight, a round front or bullnose first step; negative makes it hollow, into the flight; 0 straightens it. Every riser is an arc concentric with it (the step is cut by the arcs and the flight's two sides), the goings counted from the arc's apex, so the first step is the widest. At most half the width (a semicircle); the arc of the top step must stay wider than the flight, which asks for a radius larger than the flight's run plus half its width. The shape's `curves` has it on the bottom edge (edge 0 for a flight made here; also `shape.set_curve`); a flight that turns or is set into the slope has no curved front.").unit("m"));
    set_params.extend(sizes());
    s.register_with_spec(
        spec(
            "stairs.set",
            "Set Stairs",
            "Changes any fields of a flight; the others stay. `riser_m` or `tread_m` set to null go back to their default. On a closed 4-point rectangular block that is not stairs yet it turns the block into stairs (`up_edge` says which edge is the top, default 0). On a flight that turns, `from` and `to` move its first and last point and `width_m` its width; the outline is derived again.",
            set_params,
            "{steps, riser_m, tread_m, height_m, flights, landings, warnings}",
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
            param("curves", T::Object, "Arcs on single edges of the outline by edge index: {\"0\": {\"type\": \"arc\", \"sagitta_m\": 0.8}} (see `shape.set_curve`; a round bed is four quarter arcs, as `shape.add_circle` makes). A sleeper bed lays its beams as chords of the curve (within 2 cm), with mitred ends. `bed.set` with `outline` and no `curves` clears the arcs."),
            param("smooth", T::Boolean, "A smooth curve through all the outline's points."),
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
    s.register_with_spec(
        spec(
            "area.set_edge",
            "Set Area Edge",
            "Gives a raised area a retaining edge: a skirt in a bed style along its whole outline, from the area's top down to the lowest ground under the outline (and to the terrain's ground there). It sits just outside the outline, so the walkable top keeps its size. Faces `<id>:edge` and `<id>:edge_rim` (the coping or folded rim), with the style's material (timber, brick or corten) unless the faces have one. An area on the ground (`drape`) refuses it; set `drape` false first. Changing the style gives the edge that style's material.",
            vec![
                id("The area."),
                param("style", T::String, "plain (a solid timber side), masonry (brick with a coping slab), sleepers (stacked timber beams 0.2 m high) or corten (a steel plate with a folded rim).").allowed(&["plain", "masonry", "sleepers", "corten"]).required(),
                param("thickness_m", T::Number, "Thickness of the skirt: plain and masonry 0.05..0.3 (default 0.2), sleepers 0.1..0.2 (default 0.15), corten 0.005..0.02 (default 0.01).").unit("m"),
            ],
            "{style, thickness_m}",
            json!({"id": "S4", "style": "masonry"}),
        ),
        set_edge,
    );
    s.register_with_spec(
        spec(
            "area.clear_edge",
            "Clear Area Edge",
            "Takes the retaining edge off an area, and the materials of its edge faces.",
            vec![id("The area.")],
            "null",
            json!({"id": "S4"}),
        ),
        clear_edge,
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
    let terrain = crate::terrace::ground_fn(project)(p[0], p[1]);
    project
        .shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Area(a) if crate::shape::inside_outline(p, &a.outline()) => {
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
    let ground = crate::terrace::ground_fn(project);
    crate::shape::footing(outline, &|x, z| ground(x, z))
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

/// Gives the flight's faces their materials where they have none; `restyle` gives the cheeks the
/// material of their style over what they have.
fn stairs_materials(project: &mut Project, id: &str, restyle: bool) {
    assign_defaults(
        project,
        id,
        &[
            ("treads", "concrete"),
            ("risers", "concrete"),
            ("sides", "concrete"),
        ],
    );
    let style = project.shapes.iter().find_map(|s| match s {
        Shape::Extrusion(e) if e.id == id => e.stairs.as_ref()?.set_in?.cheek_style.bed(),
        _ => None,
    });
    if restyle || style.is_none() {
        drop_face_materials(project, id, crate::setin::FACES);
    }
    if let Some(style) = style {
        assign_defaults(
            project,
            id,
            &[
                ("cheeks", style.default_material()),
                ("cheeks_rim", style.rim_material()),
            ],
        );
    }
}

/// The mesh of closed extrusion `shape` as the scene draws it: on the project's terrain.
fn placed_mesh(project: &Project, shape: &Shape) -> Option<crate::ImportedModel> {
    let lifts = crate::shape::area_lifts(&project.shapes)
        .into_iter()
        .collect();
    crate::plant_ground::placed_shape_mesh(project, shape, &lifts, &[])
        .ok()
        .map(|(m, _)| m)
}

/// What `stairs.add` and `stairs.set` report on flight `id`.
fn stairs_report(project: &Project, id: &str) -> Value {
    let Some(shape @ Shape::Extrusion(e)) = project.shapes.iter().find(|s| s.id() == id) else {
        return Value::Null;
    };
    let (Some(st), Some(mesh)) = (&e.stairs, placed_mesh(project, shape)) else {
        return Value::Null;
    };
    let built = st.set_in.and_then(|_| crate::setin::build_in(project, e));
    let geometry = match &built {
        Some(b) => Ok(b.geometry.clone()),
        None => st.geometry_curved(&e.points, &e.curves, e.height),
    };
    match geometry {
        Ok(g) => {
            let flights: Vec<Value> = g
                .flights()
                .into_iter()
                .map(|(steps, run)| json!({"steps": steps, "run_m": run}))
                .collect();
            let landings: Vec<Value> = g.turned.as_ref().map_or_else(Vec::new, |t| {
                t.landings
                    .iter()
                    .map(|l| json!({"size_m": l.size_m, "height_m": l.height_m}))
                    .collect()
            });
            let mut out = json!({
                "steps": g.steps, "riser_m": g.riser, "tread_m": g.tread,
                "height_m": e.height, "flights": flights, "landings": landings,
                "warnings": mesh.warnings,
            });
            if let (Some(b), Some(si)) = (&built, &st.set_in) {
                out["goings_m"] = json!(g.goings);
                out["set_in"] = b.report().json(si);
            }
            out
        }
        Err(_) => Value::Null,
    }
}

/// Puts an arc on the bottom edge (the one opposite `up_edge`) of the 4-point flight `outline`
/// that bulges `bow` m away from the flight (into it when negative); none for 0.
fn front_curve(
    outline: &[P2],
    up_edge: usize,
    bow: f32,
    curves: &mut crate::curve::Curves,
) -> Result<(), CommandError> {
    if !bow.is_finite() {
        return Err(fail("`front_bow_m` must be a number"));
    }
    if outline.len() != 4 || up_edge >= 4 {
        return Err(fail("a curved front needs a 4-point rectangular footprint"));
    }
    if bow.abs() < crate::curve::MIN_SAGITTA_M {
        return Ok(());
    }
    let bottom = (up_edge + 2) % 4;
    let mid = |i: usize| {
        let (a, b) = (outline[i], outline[(i + 1) % 4]);
        [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
    };
    let (top, front) = (mid(up_edge), mid(bottom));
    let (p, q) = (outline[bottom], outline[(bottom + 1) % 4]);
    let width = (q[0] - p[0]).hypot(q[1] - p[1]);
    if bow.abs() > width / 2.0 * (1.0 + 1e-4) {
        return Err(fail(format!(
            "`front_bow_m` of {} m is more than half the width ({:.3} m): an arc reaches at most a semicircle",
            bow.abs(),
            width / 2.0
        )));
    }
    // Left of the bottom edge's direction, and the way down the flight.
    let left = [(q[1] - p[1]) / width, -(q[0] - p[0]) / width];
    let down = [front[0] - top[0], front[1] - top[1]];
    let sign = if left[0] * down[0] + left[1] * down[1] >= 0.0 {
        1.0
    } else {
        -1.0
    };
    curves.insert(
        bottom,
        crate::curve::Curve::Arc {
            sagitta_m: sign * bow,
        },
    );
    Ok(())
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
        via: Option<Vec<P2>>,
        set_in: Option<SetIn>,
        front_bow_m: Option<f32>,
    }
    let p: P = parse(params)?;
    if let Some(si) = &p.set_in {
        si.check().map_err(fail)?;
    }
    let via = p.via.clone().unwrap_or_default();
    if via.len() > 2 {
        return Err(fail("`via` takes one or two corner points"));
    }
    let (outline, path) = if via.is_empty() {
        (footprint(p.from, p.to, p.width_m)?, None)
    } else {
        let mut path = vec![p.from];
        path.extend(&via);
        path.push(p.to);
        let outline = crate::stairs::turned_outline(&path, p.width_m).map_err(fail)?;
        (outline, Some(path))
    };
    let base = p.base.unwrap_or(0.0);
    // The direction the last flight arrives in.
    let before_top = path.as_ref().map_or(p.from, |p| p[p.len() - 2]);
    let height = match p.height {
        Some(h) => h,
        None => derived_height(project, &outline, before_top, p.to, base)?,
    };
    let width_m = path.as_ref().map(|_| p.width_m);
    let mut curves = crate::curve::Curves::new();
    if let Some(bow) = p.front_bow_m {
        front_curve(&outline, 2, bow, &mut curves)?;
    }
    let r = add(project, (p.layer, None), |id, layer| {
        Shape::Extrusion(Extrusion {
            id,
            name: p.name.unwrap_or_else(|| "Stairs".into()),
            layer,
            points: outline,
            curves,
            smooth: false,
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
                path,
                width_m,
                set_in: p.set_in,
            }),
            raised_bed: None,
            furniture: None,
        })
    })?;
    let id = r["id"].as_str().expect("add returns an id").to_string();
    // A flight set into the slope can only be checked against the terrain.
    if let Some(shape) = project.shapes.iter().find(|s| s.id() == id) {
        let lifts = crate::shape::area_lifts(&project.shapes)
            .into_iter()
            .collect();
        crate::plant_ground::placed_shape_mesh(project, shape, &lifts, &[])
            .map_err(|e| fail(e.to_string()))?;
    }
    stairs_materials(project, &id, false);
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
        #[serde(default, deserialize_with = "double_list")]
        via: Option<Option<Vec<P2>>>,
        path: Option<Vec<P2>>,
        #[serde(default, deserialize_with = "double_set_in")]
        set_in: Option<Option<SetIn>>,
        front_bow_m: Option<f32>,
    }
    let p: P = parse(params)?;
    if let Some(Some(si)) = &p.set_in {
        si.check().map_err(fail)?;
    }
    if p.path.is_some() && (p.via.is_some() || p.from.is_some() || p.to.is_some()) {
        return Err(fail("give `path` alone, without `via`, `from` or `to`"));
    }
    edit(project, &p.id, true, |e| {
        if e.house.is_some() || e.raised_bed.is_some() {
            return Err(fail(format!(
                "`{}` is a house or a raised bed; make a plain block first",
                e.id
            )));
        }
        let had = e.stairs.is_some();
        let mut st = e.stairs.take().unwrap_or_else(|| Stairs::new(0));
        if let Some(si) = p.set_in {
            st.set_in = si;
        }
        if let Some(u) = p.up_edge {
            st.up_edge = u;
        }
        if let Some(r) = p.riser_m {
            st.riser_m = r;
        }
        if let Some(t) = p.tread_m {
            st.tread_m = t;
        }
        if p.from.is_some()
            || p.to.is_some()
            || p.width_m.is_some()
            || p.via.is_some()
            || p.path.is_some()
        {
            // What is not given stays as the flight is now.
            let now = had
                .then(|| st.geometry_curved(&e.points, &e.curves, e.height).ok())
                .flatten();
            let (cur_from, cur_to, cur_via) = match (&st.path, &now) {
                (Some(path), _) => (
                    Some(path[0]),
                    path.last().copied(),
                    path[1..path.len() - 1].to_vec(),
                ),
                (None, Some(g)) => (
                    Some(g.origin),
                    Some([
                        g.origin[0] + g.dir[0] * g.length,
                        g.origin[1] + g.dir[1] * g.length,
                    ]),
                    Vec::new(),
                ),
                (None, None) => (None, None, Vec::new()),
            };
            let width = p.width_m.or(st.width_m).or(now.as_ref().map(|g| g.width));
            let path = match &p.path {
                Some(path) if path.len() >= 2 => path.clone(),
                Some(_) => return Err(fail("`path` takes 2, 3 or 4 points")),
                None => {
                    let (Some(from), Some(to)) = (p.from.or(cur_from), p.to.or(cur_to)) else {
                        return Err(fail(
                            "`from`, `to` and `width_m` are all needed for a block that is not stairs yet",
                        ));
                    };
                    let via = match &p.via {
                        Some(Some(v)) => v.clone(),
                        Some(None) => Vec::new(),
                        None => cur_via,
                    };
                    if via.len() > 2 {
                        return Err(fail("`via` takes one or two corner points"));
                    }
                    let mut path = vec![from];
                    path.extend(via);
                    path.push(to);
                    path
                }
            };
            if path.len() > 4 {
                return Err(fail("`path` takes 2, 3 or 4 points"));
            }
            let Some(width) = width else {
                return Err(fail(
                    "`from`, `to` and `width_m` are all needed for a block that is not stairs yet",
                ));
            };
            if path.len() == 2 {
                e.points = footprint(path[0], path[1], width)?;
                st.up_edge = 2;
                st.path = None;
                st.width_m = None;
            } else {
                e.points = crate::stairs::turned_outline(&path, width).map_err(fail)?;
                st.path = Some(path);
                st.width_m = Some(width);
            }
        }
        if let Some(bow) = p.front_bow_m {
            e.curves.clear();
            front_curve(&e.points, st.up_edge, bow, &mut e.curves)?;
        } else {
            // A new footprint may be too short for the arc it had.
            crate::curve::fit(&e.points, true, &mut e.curves);
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
    stairs_materials(project, &p.id, p.set_in.is_some());
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
    let lifts = crate::shape::area_lifts(&project.shapes)
        .into_iter()
        .collect();
    crate::plant_ground::placed_shape_mesh(project, &shape, &lifts, &[])
        .map_err(|e| fail(e.to_string()))?;
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
    face_materials(project, id, ["wall", "rim"], style, replace);
}

/// The same for the two faces `faces` of shape `id`.
fn face_materials(
    project: &mut Project,
    id: &str,
    faces: [&str; 2],
    style: BedStyle,
    replace: bool,
) {
    if replace {
        drop_face_materials(project, id, faces);
    }
    let m = style.default_material();
    assign_defaults(project, id, &[(faces[0], m), (faces[1], m)]);
}

fn drop_face_materials(project: &mut Project, id: &str, faces: [&str; 2]) {
    project.assignments.retain(|a| {
        !matches!(&a.target, crate::Target::Face { owner, face }
            if owner == id && faces.iter().any(|f| face == &format!("{id}:{f}")))
    });
}

/// Gives an area's retaining edge the default material of its style (`replace`: over what the
/// faces have).
pub(super) fn edge_materials(project: &mut Project, id: &str, style: BedStyle, replace: bool) {
    face_materials(project, id, crate::bed::EDGE_FACES, style, replace);
}

// --- Terrace edges --------------------------------------------------------------------------

fn set_edge(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        style: BedStyle,
        thickness_m: Option<f32>,
    }
    let p: P = parse(params)?;
    let (at, mut shape) = find_area(project, &p.id)?;
    let Shape::Area(a) = &mut shape else {
        unreachable!("find_area returns an area")
    };
    let restyled = a.edge.is_none_or(|e| e.style != p.style);
    let edge = crate::bed::AreaEdge {
        style: p.style,
        thickness_m: p.thickness_m,
    };
    a.edge = Some(edge);
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    edge_materials(project, &p.id, p.style, restyled);
    Ok(json!({"style": edge.style, "thickness_m": edge.wall()}))
}

fn clear_edge(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
    }
    let p: P = parse(params)?;
    let (at, mut shape) = find_area(project, &p.id)?;
    let Shape::Area(a) = &mut shape else {
        unreachable!("find_area returns an area")
    };
    a.edge = None;
    project.shapes[at] = shape;
    drop_face_materials(project, &p.id, crate::bed::EDGE_FACES);
    Ok(Value::Null)
}

pub(super) fn find_area(project: &Project, id: &str) -> Result<(usize, Shape), CommandError> {
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == id)
        .ok_or_else(|| fail(format!("no shape `{id}`")))?;
    match &project.shapes[at] {
        s @ Shape::Area(_) => Ok((at, s.clone())),
        _ => Err(fail(format!("`{id}` is not an area"))),
    }
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
    let outline = e.outline();
    let inner = bed.inner(&outline).map_err(fail)?;
    let elevation = lowest_ground(project, &outline) + e.base + RaisedBed::soil_top(e.height);
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
                a.points = inner;
                a.curves.clear();
                a.smooth = false;
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
        #[serde(default)]
        curves: crate::curve::Curves,
        #[serde(default)]
        smooth: bool,
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
            points: p.outline,
            curves: p.curves,
            smooth: p.smooth,
            closed: true,
            base: 0.0,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: None,
            raised_bed: Some(bed),
            furniture: None,
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
        curves: Option<crate::curve::Curves>,
        smooth: Option<bool>,
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
        e.points = o;
        // New vertices: the old arcs no longer belong to the same edges.
        if p.curves.is_none() {
            e.curves.clear();
        }
    }
    if let Some(c) = p.curves {
        e.curves = c;
    }
    if let Some(s) = p.smooth {
        e.smooth = s;
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
        assert_eq!(e.points.len(), 4);
        assert!(e.points.iter().any(|p| (p[1] - 11.0).abs() < 1e-5));
        assert!(e.points.iter().any(|p| (p[1] - 14.0).abs() < 1e-5));
        assert!((e.points[1][0] - e.points[0][0]).abs() > 1.19);
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
        let xs: Vec<f32> = a.points.iter().map(|p| p[0]).collect();
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
        assert!(a.points.iter().any(|p| (p[0] - 0.1).abs() < 1e-5));
        s.run(
            "bed.set",
            json!({"id": id, "outline": [[0, 0], [4, 0], [4, 1.2], [0, 1.2]]}),
        )
        .unwrap();
        assert!(
            area(&s, &soil)
                .points
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
        assert_eq!(get(&s, &id).points[1], [3.0, 0.0]);
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
                .points
                .iter()
                .any(|p| (p[0] - 5.01).abs() < 1e-5)
        );
        assert!(s.undo());
        assert_eq!(
            get(&s, &sleepers).raised_bed.unwrap().style,
            crate::bed::BedStyle::Masonry
        );
    }

    fn terrace(s: &mut Session, extra: serde_json::Value) -> String {
        let mut p = json!({"name": "Terrace", "outline": [[0, 0], [4, 0], [4, 3], [0, 3]], "elevation": 0.5, "drape": false});
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        s.run("shape.add_area", p).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_raised_area_gets_a_skirt_down_to_the_ground_that_can_be_undone() {
        let mut s = Session::new(Project::default());
        let id = terrace(&mut s, json!({}));
        let plain = crate::shape_mesh(&s.project().shapes[0]).unwrap();
        assert_eq!(plain.faces.len(), 1);
        let r = s
            .run("area.set_edge", json!({"id": id, "style": "masonry"}))
            .unwrap();
        assert!((r["thickness_m"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        let m = crate::shape_mesh(&s.project().shapes[0]).unwrap();
        let names: Vec<_> = m.faces.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["S1:top", "S1:edge", "S1:edge_rim"]);
        let (lo, hi) = m
            .positions
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p[1]), h.max(p[1])));
        assert!(lo.abs() < 1e-5 && (hi - 0.5).abs() < 1e-5, "{lo} {hi}");
        // The skirt is outside: the top keeps its 4 x 3 m.
        let top = m.triangle_faces.iter().position(|f| *f == 0).unwrap();
        for i in m.triangles[top] {
            let p = m.positions[i as usize];
            assert!((0.0..=4.0).contains(&p[0]) && (0.0..=3.0).contains(&p[2]));
        }
        assert!(m.positions.iter().any(|p| p[0] < -0.19));
        let mats: Vec<_> = s
            .project()
            .assignments
            .iter()
            .map(|a| (a.target.owner().to_string(), a.material.as_str()))
            .collect();
        assert_eq!(mats.len(), 2);
        assert!(mats.iter().all(|(o, m)| o == "S1" && *m == "brick"));
        // Clearing takes the skirt and its materials away; the mesh is the plain one again.
        s.run("area.clear_edge", json!({"id": id})).unwrap();
        assert!(s.project().assignments.is_empty());
        let after = crate::shape_mesh(&s.project().shapes[0]).unwrap();
        assert_eq!(after.triangles.len(), plain.triangles.len());
        assert_eq!(after.positions, plain.positions);
        assert!(s.undo());
        assert!(area(&s, &id).edge.is_some());
        assert!(s.undo());
        assert!(area(&s, &id).edge.is_none() && s.project().assignments.is_empty());
        // No edge: not even a mention in the file.
        let json = serde_json::to_string(&s.project().shapes[0]).unwrap();
        assert!(!json.contains("edge\""), "{json}");
    }

    #[test]
    fn a_draped_area_refuses_an_edge_and_every_style_draws() {
        let mut s = Session::new(Project::default());
        let draped = s
            .run(
                "shape.add_area",
                json!({"name": "Lawn", "outline": [[0, 0], [4, 0], [4, 3], [0, 3]], "drape": true}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let err = s
            .run("area.set_edge", json!({"id": draped, "style": "plain"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("drape"), "{err}");
        assert!(area(&s, &draped).edge.is_none());
        assert!(s
            .run(
                "shape.add_area",
                json!({"name": "X", "outline": [[0, 0], [4, 0], [4, 3], [0, 3]], "drape": true, "edge": {"style": "plain"}}),
            )
            .is_err());
        // add_area with an edge stands flat by default.
        let id = terrace(&mut s, json!({"edge": {"style": "corten"}}));
        let _ = id;
        for style in ["plain", "masonry", "sleepers", "corten"] {
            s.run("area.set_edge", json!({"id": "S2", "style": style}))
                .unwrap();
            let m = crate::shape_mesh(&s.project().shapes[1]).unwrap();
            assert!(m.triangles.len() > 2, "{style}");
        }
        assert!(
            s.run(
                "area.set_edge",
                json!({"id": "S2", "style": "corten", "thickness_m": 0.2})
            )
            .is_err()
        );
        assert!(
            s.run("area.set_edge", json!({"id": "S9", "style": "plain"}))
                .is_err()
        );
    }

    #[test]
    fn an_edged_area_reports_its_skirt_in_the_quantities() {
        let mut s = Session::new(Project::default());
        let a = terrace(&mut s, json!({"edge": {"style": "sleepers"}}));
        let b = terrace(&mut s, json!({"edge": {"style": "corten"}}));
        let c = terrace(&mut s, json!({}));
        let q = s.run("query.quantities", json!({})).unwrap();
        let rows = q["shapes"].as_array().unwrap();
        let edge = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap()["edge"].clone();
        // 0.5 m of area lift sits on top: two courses of 0.2 m... 0.5 / 0.2 rounds to 3 beams high.
        let e = edge(&a);
        assert!((e["length_m"].as_f64().unwrap() - 14.0).abs() < 1e-4, "{e}");
        assert!(e["beams"].as_u64().unwrap() >= 12, "{e}");
        assert!(e["face_area_m2"].as_f64().unwrap() > 7.0, "{e}");
        let e = edge(&b);
        let plate = e["plate_area_m2"].as_f64().unwrap();
        assert!(
            (plate - 14.0 * e["height_m"].as_f64().unwrap()).abs() < 1e-3,
            "{e}"
        );
        assert!(e.get("beams").is_none());
        assert!(edge(&c).is_null());
    }

    #[test]
    fn the_skirt_goes_down_to_the_lowest_ground_of_the_terrain() {
        let mut t = crate::Terrain::flat([-2.0, -2.0], 10.0, 10.0, 1.0, "lawn").unwrap();
        for (i, h) in t.heights.iter_mut().enumerate() {
            *h = 0.05 * (i % 11) as f32;
        }
        let project = Project {
            terrain: Some(std::sync::Arc::new(t)),
            ..Default::default()
        };
        let mut s = Session::new(project);
        let id = terrace(&mut s, json!({"edge": {"style": "plain"}}));
        let p = s.project();
        let shape = p.shapes.iter().find(|x| x.id() == id).unwrap();
        let (m, _) =
            crate::plant_ground::placed_shape_mesh(p, shape, &Default::default(), &[]).unwrap();
        let lo = m.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        // The ground under the outline is lowest at x = 0: 2 cells in, 0.1 m.
        assert!((lo - 0.1).abs() < 1e-4, "{lo}");
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

    fn turned(s: &mut Session, via: serde_json::Value) -> String {
        s.run(
            "stairs.add",
            json!({"name": "Turn", "from": [10, 14], "to": [12.5, 11.5], "via": via,
                   "width_m": 1.0, "height": 1.5}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_turned_flight_is_added_with_flights_and_landings_and_set_again() {
        let mut s = Session::new(Project::default());
        let r = s
            .run(
                "stairs.add",
                json!({"from": [10, 14], "to": [12.5, 11.5], "via": [[10, 11.5]],
                       "width_m": 1.0, "height": 1.5}),
            )
            .unwrap();
        assert_eq!(r["steps"], 9);
        let flights = r["flights"].as_array().unwrap();
        assert_eq!(flights.len(), 2);
        assert_eq!(
            flights
                .iter()
                .map(|f| f["steps"].as_u64().unwrap())
                .sum::<u64>(),
            9
        );
        let landing = &r["landings"][0];
        assert_eq!(landing["size_m"], json!([1.0, 1.0]));
        assert!(landing["height_m"].as_f64().unwrap() > 0.5);
        let id = r["id"].as_str().unwrap().to_string();
        let e = get(&s, &id);
        assert_eq!(e.points.len(), 6);
        let st = e.stairs.clone().unwrap();
        assert_eq!(st.path.as_ref().unwrap().len(), 3);
        assert_eq!(st.width_m, Some(1.0));
        // Materials as for a straight flight.
        assert!(
            s.project()
                .assignments
                .iter()
                .any(|a| a.target.owner() == id)
        );
        // Changing the rise or the width keeps the turn; the outline follows.
        let r = s
            .run(
                "stairs.set",
                json!({"id": id, "height": 2.0, "width_m": 1.2}),
            )
            .unwrap();
        assert_eq!(r["steps"], 12);
        let size = &r["landings"][0]["size_m"];
        assert!((size[0].as_f64().unwrap() - 1.2).abs() < 1e-5 && size[0] == size[1]);
        assert_eq!(get(&s, &id).points.len(), 6);
        // A U with a half-landing, set from the L through `via`.
        let r = s
            .run(
                "stairs.set",
                json!({"id": id, "from": [10, 14], "to": [11, 14], "via": [[10, 11.5], [11, 11.5]], "width_m": 1.0}),
            )
            .unwrap();
        assert_eq!(r["flights"].as_array().unwrap().len(), 2);
        assert_eq!(r["landings"][0]["size_m"], json!([1.0, 2.0]));
        assert_eq!(get(&s, &id).stairs.unwrap().path.unwrap().len(), 4);
        // `via: null` makes it straight again, from the first to the last point.
        // (The U's first and last points are a metre apart: give a top that fits 12 steps.)
        let r = s
            .run("stairs.set", json!({"id": id, "via": null, "to": [10, 9]}))
            .unwrap();
        let e = get(&s, &id);
        assert!(e.stairs.as_ref().unwrap().path.is_none());
        assert!(e.stairs.as_ref().unwrap().width_m.is_none());
        assert_eq!(e.points.len(), 4);
        assert_eq!(r["flights"].as_array().unwrap().len(), 1);
        assert_eq!(r["landings"], json!([]));
        // `path` sets the whole centreline.
        s.run(
            "stairs.set",
            json!({"id": id, "path": [[10, 14], [10, 11.5], [12.5, 11.5]], "width_m": 1.0}),
        )
        .unwrap();
        assert_eq!(get(&s, &id).points.len(), 6);
        // Each of those was one undo step.
        assert!(s.undo());
        assert!(get(&s, &id).stairs.unwrap().path.is_none());
    }

    #[test]
    fn bad_turns_are_refused_and_leave_the_project_alone() {
        let mut s = Session::new(Project::default());
        let before = s.project().shapes.len();
        let add = |s: &mut Session, via: serde_json::Value, to: serde_json::Value| {
            s.run(
                "stairs.add",
                json!({"from": [0, 0], "to": to, "via": via, "width_m": 1.0, "height": 1.0}),
            )
            .unwrap_err()
            .to_string()
        };
        let e = add(&mut s, json!([[0, 2.5]]), json!([2.5, 3.0]));
        assert!(e.contains("right angle"), "{e}");
        let e = add(&mut s, json!([[0, 3], [0.6, 3]]), json!([0.6, 0]));
        assert!(e.contains("overlap"), "{e}");
        let e = add(&mut s, json!([[0, 1], [1, 1], [1, 2]]), json!([2, 2]));
        assert!(e.contains("one or two"), "{e}");
        assert_eq!(s.project().shapes.len(), before);
        // A bad set leaves the flight as it was.
        let id = turned(&mut s, json!([[10, 11.5]]));
        let was = get(&s, &id);
        assert!(
            s.run(
                "stairs.set",
                json!({"id": id, "path": [[10, 14], [10, 11.5], [12.5, 12]], "width_m": 1.0})
            )
            .is_err()
        );
        assert!(
            s.run(
                "stairs.set",
                json!({"id": id, "path": [[0, 0]], "width_m": 1.0})
            )
            .is_err()
        );
        assert!(
            s.run(
                "stairs.set",
                json!({"id": id, "path": [[10, 14], [10, 12], [12, 12]], "via": []})
            )
            .is_err()
        );
        assert_eq!(get(&s, &id), was);
    }

    #[test]
    fn the_vertex_commands_refuse_a_turned_flight() {
        let mut s = Session::new(Project::default());
        let id = turned(&mut s, json!([[10, 11.5]]));
        let was = get(&s, &id);
        for (command, params) in [
            (
                "shape.move_vertex",
                json!({"id": id, "index": 0, "position": [9, 14]}),
            ),
            (
                "shape.insert_vertex",
                json!({"id": id, "edge": 0, "position": [9.5, 14]}),
            ),
            ("shape.remove_vertex", json!({"id": id, "index": 1})),
            (
                "shape.set_outline",
                json!({"id": id, "outline": [[0, 0], [1, 0], [1, 1], [0, 1]]}),
            ),
        ] {
            let e = s.run(command, params).unwrap_err().to_string();
            assert!(
                e.contains("stairs.set") && e.contains("path"),
                "{command}: {e}"
            );
        }
        assert_eq!(get(&s, &id), was);
    }

    #[test]
    fn a_turned_flight_survives_a_save_and_load_and_old_projects_still_load() {
        let mut s = Session::new(Project::default());
        let id = turned(&mut s, json!([[10, 11.5]]));
        let straight = stairs(&mut s);
        let bytes = s.project().to_json_bytes();
        let loaded = Project::from_json_bytes(&bytes).unwrap();
        assert_eq!(&loaded, s.project());
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("\"path\""));
        // A straight flight writes neither field, and a project without them loads.
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let shapes = v["shapes"].as_array().unwrap();
        let find = |id: &str| shapes.iter().find(|x| x["id"] == id).unwrap();
        assert!(find(&straight)["stairs"].get("path").is_none());
        assert!(find(&straight)["stairs"].get("width_m").is_none());
        assert!(find(&id)["stairs"].get("path").is_some());
    }

    #[test]
    fn quantities_count_a_turned_flights_landings_and_going() {
        let mut s = Session::new(Project::default());
        let id = turned(&mut s, json!([[10, 11.5]]));
        let q = s.run("query.quantities", json!({})).unwrap();
        let rows = q["shapes"].as_array().unwrap();
        let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
        let st = &row["stairs"];
        assert_eq!(st["steps"], 9);
        assert_eq!(st["flights"], 2);
        assert_eq!(st["landings"], 1);
        assert!((st["landing_area_m2"].as_f64().unwrap() - 1.0).abs() < 1e-4);
        assert!((st["total_going_m"].as_f64().unwrap() - 5.0).abs() < 1e-3);
    }

    /// A 40 x 30 m terrain rising 1 in 3 towards +x: the ground is `x / 3`.
    fn hillside() -> Session {
        let mut t = crate::Terrain::flat([0.0, 0.0], 40.0, 30.0, 0.5, "lawn").unwrap();
        let w = t.width as usize + 1;
        for (i, h) in t.heights.iter_mut().enumerate() {
            *h = (i % w) as f32 * t.cell_m / 3.0;
        }
        Session::new(Project {
            terrain: Some(std::sync::Arc::new(t)),
            ..Project::default()
        })
    }

    fn set_in_steps(s: &mut Session, extra: serde_json::Value) -> serde_json::Value {
        let mut p = json!({"from": [6, 10], "to": [12, 10], "width_m": 1.2,
            "set_in": {"cheek_style": "sleepers"}});
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        s.run("stairs.add", p).unwrap()
    }

    fn mats(s: &Session, id: &str) -> Vec<(String, String)> {
        s.project()
            .assignments
            .iter()
            .filter(|a| a.target.owner() == id)
            .map(|a| (format!("{:?}", a.target), a.material.clone()))
            .collect()
    }

    #[test]
    fn set_in_stairs_follow_the_slope_and_report_goings_cheeks_and_earth() {
        let mut s = hillside();
        let r = set_in_steps(&mut s, json!({}));
        let id = r["id"].as_str().unwrap().to_string();
        // The rise is the ground at the top over the lowest ground under the flight.
        assert!((r["height_m"].as_f64().unwrap() - 2.05).abs() < 0.01, "{r}");
        assert_eq!(r["steps"], 12);
        let goings = r["goings_m"].as_array().unwrap();
        assert_eq!(goings.len(), 12);
        assert!((goings[0].as_f64().unwrap() - 0.5).abs() < 0.03);
        let si = &r["set_in"];
        assert_eq!(si["cheek_style"], "sleepers");
        // The treads stand on the slope: the earth under them is fill.
        assert!(si["fill_m3"].as_f64().unwrap() > 0.5, "{si}");
        assert!(si["cheek_length_m"].as_f64().unwrap() > 11.0, "{si}");
        assert!(si["cheek_outer_face_area_m2"].as_f64().unwrap() > 1.0);
        assert!(si["beams"].as_u64().unwrap() > 5);
        // 1 in 3 is gentler than a comfortable stair.
        assert!(r["warnings"].to_string().contains("uncomfortable"), "{r}");
        // The cheeks have their style's material.
        let m = mats(&s, &id);
        assert!(
            m.iter()
                .any(|(t, mat)| t.contains("cheeks_rim") && mat == "timber"),
            "{m:?}"
        );
        // The ground inside the flight is a tread, beside it the terrain's.
        let (at, beside) = {
            let ground = crate::terrace::ground_fn(s.project());
            (ground(9.0, 10.0).unwrap(), ground(9.0, 12.0).unwrap())
        };
        assert!((at - 3.0).abs() < 0.3 && (at - 3.0).abs() > 1e-4, "{at}");
        assert!((beside - 3.0).abs() < 1e-4);
        // The quantities say it too.
        let q = s.run("query.quantities", json!({})).unwrap();
        let row = q["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id.as_str())
            .unwrap();
        assert_eq!(
            row["stairs"]["set_in"]["goings_m"]
                .as_array()
                .unwrap()
                .len(),
            12
        );
        assert!(row["stairs"]["set_in"]["fill_m3"].as_f64().unwrap() > 0.5);
        // One undo takes it all away.
        assert!(s.undo());
        assert!(s.project().shapes.is_empty());
        assert!(crate::terrace::project_zones(s.project()).is_empty());
    }

    #[test]
    fn set_changes_the_cheeks_and_takes_the_flight_out_of_the_slope() {
        let mut s = hillside();
        let id = set_in_steps(&mut s, json!({}))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let r = s
            .run(
                "stairs.set",
                json!({"id": id, "set_in": {"cheek_style": "masonry", "coping_m": 0.2}}),
            )
            .unwrap();
        assert_eq!(r["set_in"]["cheek_style"], "masonry");
        let m = mats(&s, &id);
        assert!(
            m.iter()
                .any(|(t, mat)| t.contains("\"S1:cheeks\"") && mat == "brick"),
            "{m:?}"
        );
        assert_eq!(get(&s, &id).stairs.unwrap().set_in.unwrap().coping_m, 0.2);
        // No cheeks: the terrain is still cut, the faces and their materials are gone.
        let r = s
            .run(
                "stairs.set",
                json!({"id": id, "set_in": {"cheek_style": "none"}}),
            )
            .unwrap();
        assert_eq!(r["set_in"]["cheek_length_m"], 0.0);
        assert!(
            r["warnings"].to_string().contains("no cheeks")
                || r["warnings"].to_string().contains("uncomfortable")
        );
        assert_eq!(crate::terrace::project_zones(s.project()).len(), 1);
        assert!(mats(&s, &id).iter().all(|(t, _)| !t.contains("cheeks")));
        // Null takes it out: equal treads again, no zone, no set-in answer.
        let r = s
            .run("stairs.set", json!({"id": id, "set_in": null}))
            .unwrap();
        assert!(r.get("set_in").is_none());
        assert!(get(&s, &id).stairs.unwrap().set_in.is_none());
        assert!(crate::terrace::project_zones(s.project()).is_empty());
        // A bad one is refused and leaves the project alone.
        let before = s.project().clone();
        assert!(
            s.run("stairs.set", json!({"id": id, "set_in": {"coping_m": 3}}))
                .is_err()
        );
        assert!(
            s.run(
                "stairs.set",
                json!({"id": id, "set_in": {"cheek_style": "marble"}})
            )
            .is_err()
        );
        assert!(
            s.run(
                "stairs.set",
                json!({"id": id, "set_in": {"cheek_style": "sleepers", "thickness_m": 0.5}})
            )
            .is_err()
        );
        assert_eq!(&before, s.project());
    }

    #[test]
    fn set_in_survives_a_save_and_load_and_old_flights_have_no_field() {
        let mut s = hillside();
        let id = set_in_steps(&mut s, json!({"height": 2.0}))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let plain = stairs(&mut s);
        let bytes = s.project().to_json_bytes();
        let loaded = Project::from_json_bytes(&bytes).unwrap();
        assert_eq!(&loaded, s.project());
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let shapes = v["shapes"].as_array().unwrap();
        let find = |id: &str| shapes.iter().find(|x| x["id"] == id).unwrap();
        assert_eq!(find(&id)["stairs"]["set_in"]["cheek_style"], "sleepers");
        assert!(find(&plain)["stairs"].get("set_in").is_none());
        // A project saved before the field existed loads with none.
        let mut old = v.clone();
        for sh in old["shapes"].as_array_mut().unwrap() {
            if let Some(st) = sh.get_mut("stairs").and_then(|x| x.as_object_mut()) {
                st.remove("set_in");
            }
        }
        let loaded = Project::from_json_bytes(&serde_json::to_vec(&old).unwrap()).unwrap();
        for sh in &loaded.shapes {
            if let Shape::Extrusion(e) = sh {
                assert!(e.stairs.as_ref().unwrap().set_in.is_none());
            }
        }
    }

    #[test]
    fn a_turned_flight_can_be_set_in_and_refuses_a_flight_too_short_for_its_steps() {
        let mut s = hillside();
        let r = s
            .run(
                "stairs.add",
                json!({"from": [6, 6], "via": [[6, 12]], "to": [12, 12], "width_m": 1.0,
                       "height": 2.0, "set_in": {"cheek_style": "plain"}}),
            )
            .unwrap();
        assert_eq!(r["flights"].as_array().unwrap().len(), 2);
        assert_eq!(
            r["goings_m"].as_array().unwrap().len() as u64,
            r["steps"].as_u64().unwrap()
        );
        assert_eq!(r["landings"].as_array().unwrap().len(), 1);
        // 12 steps need 3 m at least: a 1.5 m flight cannot be set in.
        let before = s.project().clone();
        assert!(
            s.run(
                "stairs.add",
                json!({"from": [20, 10], "to": [21.5, 10], "width_m": 1.0, "height": 2.0,
                       "set_in": {}}),
            )
            .is_err()
        );
        assert_eq!(&before, s.project());
    }
}
