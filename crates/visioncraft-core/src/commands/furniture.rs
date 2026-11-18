//! Furniture: `furniture.add`, `furniture.set`, `furniture.clear` and `furniture.add_set` turn a
//! closed extrusion into a table, chair, sofa and so on (`crate::furniture`), the way the stairs
//! commands do for steps.

use super::shapes::add;
use super::{fail, house::assign_defaults};
use crate::furniture::{self, Furniture, FurnitureKind, SETS};
use crate::project::Project;
use crate::session::{CommandError, Session, parse};
use crate::shape::{Extrusion, Shape, shape_mesh};
use crate::spec::{ParamType as T, param, spec};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

type P2 = [f32; 2];

/// Present and `null` (back to the default) apart from absent (leave it).
fn double_option<'de, D: Deserializer<'de>, V: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<V>>, D::Error> {
    Option::<V>::deserialize(d).map(Some)
}

/// The face names any kind of furniture has.
const ALL_FACES: [&str; 4] = ["frame", "top", "cushion", "canopy"];

fn kind_names() -> Vec<&'static str> {
    FurnitureKind::ALL.iter().map(|k| k.name()).collect()
}

/// The option parameters of `furniture.add` and `furniture.set`.
fn options() -> Vec<crate::spec::ParamSpec> {
    vec![
        param("back", T::Boolean, "Bench: with a back (default true) or a plain seat (then the height is the seat's, 0.45 m to begin with)."),
        param("back_deg", T::Number, "Lounger: the back rest's angle above flat, 0..80 (default 40). `null` in `furniture.set` gives the default back.").unit("deg"),
        param("open", T::Boolean, "Parasol: canopy open (default true) or folded round the pole."),
        param("cushions", T::Integer, "Sofa and armchair: the number of seat cushions, 1..8 (and as many back cushions). Without it one for each 0.6 to 0.8 m between the arms. `null` in `furniture.set` goes back to that."),
        param("doors", T::Integer, "Cabinet: doors side by side, 1..8; without it one for each 0.45 m. `null` in `furniture.set` goes back to that."),
        param("drawers", T::Integer, "Cabinet: rows of drawer fronts at the top, 0..4 (default 0)."),
        param("pedestal", T::Boolean, "Table and side table: one central pedestal instead of four legs. A round table has one unless this is false. `null` in `furniture.set` goes back to that."),
    ]
}

pub(super) fn register(s: &mut Session) {
    let kinds = kind_names();
    let sizes = || {
        vec![
            param("length", T::Number, "Length along the piece's width (a lounger and a bed: head to foot), 0.1..8. Per kind: table 1.8 (round 1.2, the diameter), chair 0.45, bench 1.5, sofa 2.2, armchair 0.9, lounger 1.95, parasol 3 (the canopy's diameter), side_table 0.45, cabinet 1.2, bed 2.1.").unit("m"),
            param("depth", T::Number, "Depth front to back (a lounger and a bed: across), 0.1..8; ignored for a round piece. Per kind: table 0.9, chair 0.5, bench 0.5 (0.45 without back), sofa 0.9, armchair 0.9, lounger 0.7, parasol 3, side_table 0.45, cabinet 0.45, bed 1.6.").unit("m"),
            param("height", T::Number, "Overall height, 0.1..4 (a bench without a back: the seat's). Per kind: table 0.75, chair 0.85, bench 0.85 (0.45 without back), sofa 0.8, armchair 0.8, lounger 0.85, parasol 2.4, side_table 0.5, cabinet 0.9, bed 0.9 (the headboard's).").unit("m"),
        ]
    };
    let mut add_params = vec![
        param("kind", T::String, "The kind of piece.").allowed(&kinds).required(),
        param("name", T::String, "Display name. Default: the kind, capitalised."),
        param("position", T::Point2, "Centre of the piece on the plan, [x, z] in metres.").required(),
        param("rotation_deg", T::Number, "Turn of the piece counter-clockwise seen from above (-Z is north, +X east). A chair's, sofa's or cabinet's front faces +Z (south) at 0; a lounger's and bed's head is at the -X (west) end at 0.").unit("deg").default(json!(0)),
        param("round", T::Boolean, "A circular footprint (table, side table or parasol): `length` is the diameter. Default false.").default(json!(false)),
        param("base", T::Number, "Height of the feet (on terrain, above the lowest ground under the footprint).").unit("m").default(json!(0)),
        param("layer", T::String, "Layer id.").default(json!("shapes")),
    ];
    add_params.extend(sizes());
    add_params.extend(options());
    s.register_with_spec(
        spec(
            "furniture.add",
            "Add Furniture",
            "Adds a parametric piece of furniture: a closed extrusion whose rectangular footprint (or circle, for a round table) is its length and depth and whose height is its overall height. Faces for materials, only those the kind has: `<id>:frame` (legs, frames, poles, bases, door fronts), `<id>:top` (table tops, seat shells, headboards, cabinet bodies), `<id>:cushion` (cushions, mattress, pillows) and `<id>:canopy`. Starts with teak, black powder coat, sand fabric and white canvas from the palette where the project has nothing assigned. Kinds: table, chair, bench, sofa, armchair, lounger, parasol, side_table, cabinet, bed.",
            add_params,
            "{id, kind, length_m, depth_m, height_m, faces}",
            json!({"kind": "table", "position": [5, 5], "length": 1.8, "depth": 0.9}),
        ),
        add_furniture,
    );
    let mut set_params = vec![
        param("id", T::String, "The piece, or a closed rectangular extrusion to turn into one (then `kind` is needed).").required(),
        param("kind", T::String, "A new kind; the options of the old one stay and the faces it lacks lose their materials.").allowed(&kinds),
        param("name", T::String, "Display name."),
        param("position", T::Point2, "New centre on the plan."),
        param("rotation_deg", T::Number, "New turn, counter-clockwise seen from above.").unit("deg"),
        param("round", T::Boolean, "Make the footprint a circle (table, side table, parasol; `length` is the diameter) or a rectangle again."),
        param("base", T::Number, "New height of the feet.").unit("m"),
    ];
    set_params.extend(sizes());
    set_params.extend(options());
    s.register_with_spec(
        spec(
            "furniture.set",
            "Set Furniture",
            "Changes the kind, options, size, position or turn of a piece; what is not given stays. A new `length` or `depth` keeps the centre and the turn. A closed rectangular block that is not furniture becomes the piece `kind` with its footprint and height.",
            set_params,
            "{id, kind, length_m, depth_m, height_m, faces}",
            json!({"id": "S4", "length": 2.4, "cushions": 4}),
        ),
        set_furniture,
    );
    s.register_with_spec(
        spec(
            "furniture.clear",
            "Clear Furniture",
            "Turns a piece back into a plain block of the same footprint and height, and takes off the materials of the faces only furniture has.",
            vec![param("id", T::String, "The piece.").required()],
            "null",
            json!({"id": "S4"}),
        ),
        clear_furniture,
    );
    s.register_with_spec(
        spec(
            "furniture.add_set",
            "Add Furniture Set",
            "Places a group as separate pieces (each one a furniture shape, in one undo step): `dining` is a table sized for `seats` with chairs pulled up round it, facing it; `lounge` a sofa, two armchairs and a coffee table; `sunbathing` two loungers with their heads north, a side table between and a parasol behind. The group turns round `position` by `rotation_deg`.",
            vec![
                param("set", T::String, "Which group.").allowed(&SETS).required(),
                param("position", T::Point2, "Centre of the group on the plan, [x, z] in metres.").required(),
                param("rotation_deg", T::Number, "Turn of the whole group counter-clockwise seen from above (-Z is north, +X east).").unit("deg").default(json!(0)),
                param("seats", T::Integer, "Dining set: the number of chairs, 2..12.").default(json!(furniture::DEFAULT_SEATS)),
                param("base", T::Number, "Height of the feet of every piece.").unit("m").default(json!(0)),
                param("layer", T::String, "Layer id.").default(json!("shapes")),
            ],
            "{set, ids: [..], pieces: [{id, kind, name}]}",
            json!({"set": "dining", "position": [6, 6], "seats": 6}),
        ),
        add_set,
    );
}

fn number(what: &str, v: f32) -> Result<f32, CommandError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(fail(format!("`{what}` must be a finite number")))
    }
}

fn positive(what: &str, v: f32) -> Result<f32, CommandError> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(fail(format!(
            "`{what}` must be a positive number of metres"
        )))
    }
}

fn point(what: &str, p: P2) -> Result<P2, CommandError> {
    number(what, p[0])?;
    number(what, p[1])?;
    Ok(p)
}

fn capitalised(name: &str) -> String {
    let mut s = name.replace('_', " ");
    if let Some(c) = s.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    s
}

fn kind_of(name: &str) -> Result<FurnitureKind, CommandError> {
    FurnitureKind::parse(name).ok_or_else(|| {
        fail(format!(
            "unknown furniture kind `{name}`; one of {}",
            kind_names().join(", ")
        ))
    })
}

fn materials(project: &mut Project, id: &str, kind: FurnitureKind) {
    assign_defaults(project, id, kind.default_materials());
}

/// Takes the assignments of faces `id` has no more under `keep` (furniture faces only).
fn drop_faces(project: &mut Project, id: &str, keep: &[&str]) {
    project.assignments.retain(|a| {
        !matches!(&a.target, crate::Target::Face { owner, face }
            if owner == id
                && ALL_FACES
                    .iter()
                    .any(|f| !keep.contains(f) && face == &format!("{id}:{f}")))
    });
}

fn report(project: &Project, id: &str) -> Value {
    let Some(Shape::Extrusion(e)) = project.shapes.iter().find(|s| s.id() == id) else {
        return Value::Null;
    };
    let Some(f) = &e.furniture else {
        return Value::Null;
    };
    let Ok(fr) = f.frame(&e.points, Shape::Extrusion(e.clone()).is_curved(), e.height) else {
        return Value::Null;
    };
    json!({
        "id": id, "kind": f.kind, "length_m": fr.dims()[0], "depth_m": fr.dims()[1],
        "height_m": fr.dims()[2], "faces": f.faces(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddParams {
    kind: String,
    name: Option<String>,
    position: P2,
    rotation_deg: Option<f32>,
    round: Option<bool>,
    base: Option<f32>,
    layer: Option<String>,
    length: Option<f32>,
    depth: Option<f32>,
    height: Option<f32>,
    back: Option<bool>,
    back_deg: Option<f32>,
    open: Option<bool>,
    cushions: Option<u32>,
    doors: Option<u32>,
    drawers: Option<u32>,
    pedestal: Option<bool>,
}

/// Puts a piece of `f` on `project`: centred on `position`, turned `rotation_deg`, with the
/// sizes given or the kind's own; returns its id.
#[allow(clippy::too_many_arguments)]
fn place(
    project: &mut Project,
    f: Furniture,
    name: Option<String>,
    position: P2,
    rotation_deg: f32,
    round: bool,
    base: f32,
    layer: Option<String>,
    size: [Option<f32>; 3],
) -> Result<String, CommandError> {
    point("position", position)?;
    number("rotation_deg", rotation_deg)?;
    number("base", base)?;
    f.check().map_err(fail)?;
    if round && !f.kind.can_be_round() {
        return Err(fail(format!(
            "a {} cannot be round: only a table, a side table and a parasol can",
            f.kind.name()
        )));
    }
    let [dl, dd, dh] = f.default_size(round);
    let length = positive("length", size[0].unwrap_or(dl))?;
    // A parasol's canopy is round: its `length` alone sizes it.
    let square = f.kind == FurnitureKind::Parasol;
    let depth = positive("depth", size[1].unwrap_or(if square { length } else { dd }))?;
    let height = positive("height", size[2].unwrap_or(dh))?;
    let name = name.unwrap_or_else(|| capitalised(f.kind.name()));
    let (points, curves) = if round {
        furniture::round_outline(position, length)
    } else {
        (
            furniture::rect_outline(position, rotation_deg, length, depth),
            Default::default(),
        )
    };
    let kind = f.kind;
    let r = add(project, (layer, None), |id, layer| {
        Shape::Extrusion(Extrusion {
            id,
            name,
            layer,
            points,
            curves,
            smooth: false,
            closed: true,
            base,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: None,
            raised_bed: None,
            furniture: Some(f),
        })
    })?;
    let id = r["id"].as_str().expect("add returns an id").to_string();
    materials(project, &id, kind);
    Ok(id)
}

fn add_furniture(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let p: AddParams = parse(params)?;
    let mut f = Furniture::new(kind_of(&p.kind)?);
    f.back = p.back.unwrap_or(true);
    f.back_deg = p.back_deg;
    f.open = p.open.unwrap_or(true);
    f.cushions = p.cushions;
    f.doors = p.doors;
    f.drawers = p.drawers.unwrap_or(0);
    f.pedestal = p.pedestal;
    let id = place(
        project,
        f,
        p.name,
        p.position,
        p.rotation_deg.unwrap_or(0.0),
        p.round.unwrap_or(false),
        p.base.unwrap_or(0.0),
        p.layer,
        [p.length, p.depth, p.height],
    )?;
    Ok(report(project, &id))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetParams {
    id: String,
    kind: Option<String>,
    name: Option<String>,
    position: Option<P2>,
    rotation_deg: Option<f32>,
    round: Option<bool>,
    base: Option<f32>,
    length: Option<f32>,
    depth: Option<f32>,
    height: Option<f32>,
    back: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    back_deg: Option<Option<f32>>,
    open: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    cushions: Option<Option<u32>>,
    #[serde(default, deserialize_with = "double_option")]
    doors: Option<Option<u32>>,
    drawers: Option<u32>,
    #[serde(default, deserialize_with = "double_option")]
    pedestal: Option<Option<bool>>,
}

fn set_furniture(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let p: SetParams = parse(params)?;
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == p.id)
        .ok_or_else(|| fail(format!("no shape `{}`", p.id)))?;
    let mut shape = project.shapes[at].clone();
    let curved = shape.is_curved();
    let Shape::Extrusion(e) = &mut shape else {
        return Err(fail(format!("`{}` is an area, not furniture", p.id)));
    };
    if !e.closed {
        return Err(fail(format!(
            "`{}` is an open wall, not a block or furniture",
            p.id
        )));
    }
    let was = e.furniture.clone();
    if was.is_none()
        && (e.house.is_some() || e.fence.is_some() || e.stairs.is_some() || e.raised_bed.is_some())
    {
        return Err(fail(format!(
            "`{}` is a house, stairs or a raised bed; make a plain block first",
            p.id
        )));
    }
    let mut f = match &was {
        Some(f) => f.clone(),
        None => {
            let name = p.kind.as_deref().ok_or_else(|| {
                fail(format!(
                    "`{}` is not furniture yet: give a `kind` to make it one",
                    p.id
                ))
            })?;
            Furniture::new(kind_of(name)?)
        }
    };
    if let Some(k) = &p.kind {
        f.kind = kind_of(k)?;
    }
    if let Some(b) = p.back {
        f.back = b;
    }
    if let Some(a) = p.back_deg {
        f.back_deg = a;
    }
    if let Some(o) = p.open {
        f.open = o;
    }
    if let Some(c) = p.cushions {
        f.cushions = c;
    }
    if let Some(d) = p.doors {
        f.doors = d;
    }
    if let Some(d) = p.drawers {
        f.drawers = d;
    }
    if let Some(v) = p.pedestal {
        f.pedestal = v;
    }
    f.check().map_err(fail)?;
    // Where the piece is now: its own frame, or what the block's footprint gives.
    let now = Furniture::new(FurnitureKind::Table)
        .frame(&e.points, curved, e.height)
        .map_err(fail)?;
    let moves = p.position.is_some()
        || p.rotation_deg.is_some()
        || p.round.is_some()
        || p.length.is_some()
        || p.depth.is_some();
    let round = p.round.unwrap_or(now.round);
    if round && !f.kind.can_be_round() {
        return Err(fail(format!(
            "a {} cannot be round: only a table, a side table and a parasol can",
            f.kind.name()
        )));
    }
    if moves {
        let centre = point("position", p.position.unwrap_or(now.centre))?;
        let turn = number(
            "rotation_deg",
            p.rotation_deg.unwrap_or_else(|| now.rotation_deg()),
        )?;
        let length = positive("length", p.length.unwrap_or(now.length))?;
        // Going from round to a rectangle (or back) keeps the diameter as the length.
        let depth = positive(
            "depth",
            p.depth
                .unwrap_or(if now.round { now.length } else { now.depth }),
        )?;
        if round {
            let (points, curves) = furniture::round_outline(centre, length);
            e.points = points;
            e.curves = curves;
        } else {
            e.points = furniture::rect_outline(centre, turn, length, depth);
            e.curves = Default::default();
        }
        e.smooth = false;
    }
    if let Some(b) = p.base {
        e.base = number("base", b)?;
    }
    if let Some(h) = p.height {
        e.height = positive("height", h)?;
    }
    if let Some(n) = p.name {
        e.name = n;
    }
    let kind = f.kind;
    e.furniture = Some(f);
    let id = e.id.clone();
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    drop_faces(project, &id, kind.faces());
    materials(project, &id, kind);
    Ok(report(project, &id))
}

fn clear_furniture(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"]
        .as_str()
        .ok_or_else(|| fail("`id`: furniture"))?;
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == id)
        .ok_or_else(|| fail(format!("no shape `{id}`")))?;
    let Shape::Extrusion(e) = &mut project.shapes[at] else {
        return Err(fail(format!("`{id}` is an area, not furniture")));
    };
    let Some(f) = e.furniture.take() else {
        return Err(fail(format!("`{id}` is not furniture; use furniture.set")));
    };
    // The curves of a round piece stay: a plain block can be round too.
    let default_top = f
        .kind
        .default_materials()
        .iter()
        .find(|(face, _)| *face == "top")
        .map(|(_, m)| *m);
    drop_faces(project, id, &["top"]);
    // The block's own top keeps what the piece's top had only when it was the default.
    if let Some(m) = default_top {
        project.assignments.retain(|a| {
            !(matches!(&a.target, crate::Target::Face { owner, face }
                if owner == id && face == &format!("{id}:top"))
                && a.material == m)
        });
    }
    Ok(Value::Null)
}

fn add_set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        set: String,
        position: P2,
        rotation_deg: Option<f32>,
        seats: Option<usize>,
        base: Option<f32>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    let pieces =
        furniture::set_pieces(&p.set, p.seats.unwrap_or(furniture::DEFAULT_SEATS)).map_err(fail)?;
    let position = point("position", p.position)?;
    let turn = number("rotation_deg", p.rotation_deg.unwrap_or(0.0))?;
    let base = number("base", p.base.unwrap_or(0.0))?;
    let t = turn.to_radians();
    let (ex, ez) = ([t.cos(), -t.sin()], [t.sin(), t.cos()]);
    let mut rows = Vec::new();
    for piece in pieces {
        let [u, v] = piece.position;
        let at = [
            position[0] + ex[0] * u + ez[0] * v,
            position[1] + ex[1] * u + ez[1] * v,
        ];
        let id = place(
            project,
            Furniture::new(piece.kind),
            Some(piece.name.to_string()),
            at,
            turn + piece.rotation_deg,
            false,
            base,
            p.layer.clone(),
            piece.size.map(Some),
        )?;
        rows.push(json!({"id": id, "kind": piece.kind, "name": piece.name}));
    }
    let ids: Vec<Value> = rows.iter().map(|r| r["id"].clone()).collect();
    Ok(json!({"set": p.set, "ids": ids, "pieces": rows}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(s: &Session, id: &str) -> Extrusion {
        match s.project().shapes.iter().find(|x| x.id() == id).unwrap() {
            Shape::Extrusion(e) => e.clone(),
            Shape::Area(_) => panic!("an area"),
        }
    }

    fn table(s: &mut Session) -> String {
        s.run(
            "furniture.add",
            json!({"kind": "table", "position": [5, 5], "name": "Dining table"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn add_makes_one_undoable_piece_with_default_sizes_and_materials() {
        let mut s = Session::new(Default::default());
        let r = s
            .run(
                "furniture.add",
                json!({"kind": "table", "position": [5, 5]}),
            )
            .unwrap();
        let id = r["id"].as_str().unwrap();
        assert_eq!(r["kind"], "table");
        assert_eq!(
            (r["length_m"].as_f64(), r["depth_m"].as_f64()),
            (Some(1.8), Some(0.9))
        );
        assert!((r["height_m"].as_f64().unwrap() - 0.75).abs() < 1e-6);
        assert_eq!(r["faces"], json!(["top", "frame"]));
        let e = get(&s, id);
        assert_eq!(e.name, "Table");
        assert_eq!(e.furniture.as_ref().unwrap().kind, FurnitureKind::Table);
        // Centred on the position.
        let cx: f32 = e.points.iter().map(|p| p[0]).sum::<f32>() / 4.0;
        let cz: f32 = e.points.iter().map(|p| p[1]).sum::<f32>() / 4.0;
        assert!((cx - 5.0).abs() < 1e-5 && (cz - 5.0).abs() < 1e-5);
        let mats: Vec<_> = s
            .project()
            .assignments
            .iter()
            .map(|a| a.material.as_str())
            .collect();
        assert_eq!(mats, ["teak", "powder-coat-black"]);
        assert!(s.project().materials.iter().any(|m| m.id == "teak"));
        assert!(s.undo());
        assert!(s.project().shapes.is_empty() && s.project().assignments.is_empty());
    }

    #[test]
    fn add_takes_sizes_turn_round_and_options() {
        let mut s = Session::new(Default::default());
        let r = s
            .run(
                "furniture.add",
                json!({"kind": "sofa", "position": [0, 0], "length": 3.0, "depth": 1.0, "height": 0.9, "base": 0.3, "rotation_deg": 90, "cushions": 5}),
            )
            .unwrap();
        let e = get(&s, r["id"].as_str().unwrap());
        assert_eq!((e.base, e.height), (0.3, 0.9));
        assert_eq!(e.furniture.as_ref().unwrap().cushions, Some(5));
        // Turned 90 degrees the 3 m length runs north-south.
        let zs: Vec<f32> = e.points.iter().map(|p| p[1]).collect();
        let span = zs.iter().cloned().fold(f32::MIN, f32::max)
            - zs.iter().cloned().fold(f32::MAX, f32::min);
        assert!((span - 3.0).abs() < 1e-4, "{span}");
        let r = s
            .run(
                "furniture.add",
                json!({"kind": "table", "position": [8, 8], "round": true, "length": 1.4}),
            )
            .unwrap();
        let e = get(&s, r["id"].as_str().unwrap());
        assert_eq!(e.curves.len(), 4);
        assert_eq!(
            (r["length_m"].as_f64(), r["depth_m"].as_f64()),
            (Some(1.4), Some(1.4))
        );
        // The round default is a smaller table.
        let r = s
            .run(
                "furniture.add",
                json!({"kind": "table", "position": [1, 1], "round": true}),
            )
            .unwrap();
        assert!((r["length_m"].as_f64().unwrap() - 1.2).abs() < 1e-6);
    }

    #[test]
    fn add_refuses_what_cannot_stand() {
        let mut s = Session::new(Default::default());
        for (params, want) in [
            (json!({"kind": "throne", "position": [0, 0]}), "kind"),
            (
                json!({"kind": "chair", "position": [0, 0], "length": -1}),
                "length",
            ),
            (
                json!({"kind": "chair", "position": [0, 0], "depth": 0}),
                "depth",
            ),
            (
                json!({"kind": "chair", "position": [0, 0], "height": 9}),
                "height",
            ),
            (
                json!({"kind": "chair", "position": [0, 0], "round": true}),
                "round",
            ),
            (
                json!({"kind": "lounger", "position": [0, 0], "back_deg": 120}),
                "back_deg",
            ),
            (
                json!({"kind": "sofa", "position": [0, 0], "cushions": 0}),
                "cushions",
            ),
            (
                json!({"kind": "cabinet", "position": [0, 0], "drawers": 9}),
                "drawers",
            ),
            (
                json!({"kind": "chair", "position": [0, 0], "rotation_deg": null, "layer": "nope"}),
                "layer",
            ),
            (
                json!({"kind": "chair", "position": [0, 0], "colour": 1}),
                "colour",
            ),
        ] {
            let err = s
                .run("furniture.add", params.clone())
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{params}: {err}");
        }
        assert!(s.project().shapes.is_empty());
        assert!(
            s.run(
                "furniture.add",
                json!({"kind": "chair", "position": [0.0, f64::NAN]})
            )
            .is_err()
        );
    }

    #[test]
    fn set_changes_sizes_keeping_centre_and_turn_and_options_by_null() {
        let mut s = Session::new(Default::default());
        let id = s
            .run(
                "furniture.add",
                json!({"kind": "sofa", "position": [4, 4], "rotation_deg": 30}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let r = s
            .run(
                "furniture.set",
                json!({"id": id, "length": 2.8, "cushions": 4, "height": 0.85}),
            )
            .unwrap();
        assert_eq!(
            r["length_m"].as_f64().map(|v| (v * 100.0).round()),
            Some(280.0)
        );
        let e = get(&s, &id);
        assert_eq!(e.furniture.as_ref().unwrap().cushions, Some(4));
        let f = e
            .furniture
            .as_ref()
            .unwrap()
            .frame(&e.points, false, e.height)
            .unwrap();
        assert!((f.centre[0] - 4.0).abs() < 1e-4 && (f.centre[1] - 4.0).abs() < 1e-4);
        assert!(
            (f.rotation_deg() - 30.0).abs() < 1e-2,
            "{}",
            f.rotation_deg()
        );
        assert!((f.depth - 0.9).abs() < 1e-4);
        s.run("furniture.set", json!({"id": id, "cushions": null}))
            .unwrap();
        assert_eq!(get(&s, &id).furniture.unwrap().cushions, None);
        // Moving and turning.
        s.run(
            "furniture.set",
            json!({"id": id, "position": [10, 2], "rotation_deg": 0}),
        )
        .unwrap();
        let p0 = get(&s, &id).points[0];
        assert!((p0[0] - 8.6).abs() < 1e-4 && (p0[1] - 1.55).abs() < 1e-4);
        // Bad values keep the piece as it was.
        assert!(
            s.run("furniture.set", json!({"id": id, "length": 0}))
                .is_err()
        );
        assert!(
            s.run("furniture.set", json!({"id": id, "round": true}))
                .is_err()
        );
        assert!(
            s.run("furniture.set", json!({"id": "S9", "length": 1}))
                .is_err()
        );
        assert!(s.undo() && s.undo());
        assert_eq!(get(&s, &id).furniture.unwrap().cushions, Some(4));
    }

    #[test]
    fn set_changes_kind_round_and_makes_a_block_furniture() {
        let mut s = Session::new(Default::default());
        let id = table(&mut s);
        // Round, with the length as the diameter; back to a rectangle keeps it square.
        s.run(
            "furniture.set",
            json!({"id": id, "round": true, "length": 1.1}),
        )
        .unwrap();
        assert_eq!(get(&s, &id).curves.len(), 4);
        s.run("furniture.set", json!({"id": id, "round": false}))
            .unwrap();
        let e = get(&s, &id);
        assert!(e.curves.is_empty());
        let f = e
            .furniture
            .as_ref()
            .unwrap()
            .frame(&e.points, false, e.height)
            .unwrap();
        assert!((f.length - 1.1).abs() < 1e-4 && (f.depth - 1.1).abs() < 1e-4);
        // A new kind: the cushion faces are new, the materials of the old kind's other faces stay.
        s.run(
            "furniture.set",
            json!({"id": id, "kind": "sofa", "height": 0.8}),
        )
        .unwrap();
        let mats: Vec<_> = s
            .project()
            .assignments
            .iter()
            .map(|a| a.material.as_str())
            .collect();
        assert!(mats.contains(&"fabric-sand"), "{mats:?}");
        s.run("furniture.set", json!({"id": id, "kind": "parasol"}))
            .unwrap();
        let faces: Vec<_> = s
            .project()
            .assignments
            .iter()
            .filter_map(|a| match &a.target {
                crate::Target::Face { face, .. } => Some(face.clone()),
                _ => None,
            })
            .collect();
        assert!(
            !faces
                .iter()
                .any(|f| f.ends_with(":cushion") || f.ends_with(":top")),
            "{faces:?}"
        );
        assert!(faces.iter().any(|f| f.ends_with(":canopy")));
        // A plain block: needs a kind, then becomes the piece on its footprint.
        let b = s
            .run(
                "shape.add_extrusion",
                json!({"name": "Block", "outline": [[0, 0], [2, 0], [2, 1], [0, 1]], "height": 0.8}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let err = s
            .run("furniture.set", json!({"id": b, "length": 1}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("kind"), "{err}");
        let r = s
            .run("furniture.set", json!({"id": b, "kind": "sofa"}))
            .unwrap();
        assert_eq!(
            r["height_m"].as_f64().map(|v| (v * 10.0).round()),
            Some(8.0)
        );
        assert!((r["length_m"].as_f64().unwrap() - 2.0).abs() < 1e-5);
        // A house is refused.
        let h = s
            .run(
                "shape.add_extrusion",
                json!({"name": "House", "outline": [[20, 0], [24, 0], [24, 4], [20, 4]], "height": 3, "roof": {"kind": "flat"}}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            s.run("furniture.set", json!({"id": h, "kind": "table"}))
                .is_err()
        );
    }

    #[test]
    fn clear_makes_a_block_again() {
        let mut s = Session::new(Default::default());
        let id = table(&mut s);
        s.run("furniture.clear", json!({"id": id})).unwrap();
        let e = get(&s, &id);
        assert!(e.furniture.is_none() && e.points.len() == 4 && (e.height - 0.75).abs() < 1e-6);
        assert!(
            s.project().assignments.is_empty(),
            "{:?}",
            s.project().assignments
        );
        assert!(s.run("furniture.clear", json!({"id": id})).is_err());
        assert!(s.run("furniture.clear", json!({"id": "S9"})).is_err());
        assert!(s.undo());
        assert!(get(&s, &id).furniture.is_some());
    }

    fn pieces(s: &Session) -> Vec<(FurnitureKind, [f32; 2])> {
        s.project()
            .shapes
            .iter()
            .filter_map(|x| match x {
                Shape::Extrusion(e) => e.furniture.as_ref().map(|f| {
                    let c = [
                        e.points.iter().map(|p| p[0]).sum::<f32>() / e.points.len() as f32,
                        e.points.iter().map(|p| p[1]).sum::<f32>() / e.points.len() as f32,
                    ];
                    (f.kind, c)
                }),
                Shape::Area(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_dining_set_is_a_table_and_chairs_in_one_undo_step() {
        let mut s = Session::new(Default::default());
        let r = s
            .run(
                "furniture.add_set",
                json!({"set": "dining", "position": [10, 10], "seats": 6}),
            )
            .unwrap();
        assert_eq!(r["ids"].as_array().unwrap().len(), 7);
        let all = pieces(&s);
        assert_eq!(
            all.iter()
                .filter(|(k, _)| *k == FurnitureKind::Table)
                .count(),
            1
        );
        assert_eq!(
            all.iter()
                .filter(|(k, _)| *k == FurnitureKind::Chair)
                .count(),
            6
        );
        assert_eq!(all[0].1, [10.0, 10.0]);
        // Each chair's front looks at the table, also when the set is turned.
        for (rot, base) in [(0.0, 0.0), (65.0, 0.5)] {
            let mut s = Session::new(Default::default());
            s.run(
                "furniture.add_set",
                json!({"set": "dining", "position": [10, 10], "rotation_deg": rot, "base": base}),
            )
            .unwrap();
            for sh in &s.project().shapes {
                let Shape::Extrusion(e) = sh else { continue };
                let f = e.furniture.as_ref().unwrap();
                assert_eq!(e.base, base);
                if f.kind != FurnitureKind::Chair {
                    continue;
                }
                let fr = f.frame(&e.points, false, e.height).unwrap();
                let to = [10.0 - fr.centre[0], 10.0 - fr.centre[1]];
                assert!(
                    fr.ez[0] * to[0] + fr.ez[1] * to[1] > 0.1,
                    "turn {rot}: faces away"
                );
            }
        }
        assert!(s.undo());
        assert!(s.project().shapes.is_empty() && s.project().assignments.is_empty());
        assert!(
            s.run(
                "furniture.add_set",
                json!({"set": "dining", "position": [0, 0], "seats": 20})
            )
            .is_err()
        );
        assert!(
            s.run(
                "furniture.add_set",
                json!({"set": "picnic", "position": [0, 0]})
            )
            .is_err()
        );
        assert!(s.project().shapes.is_empty());
    }

    #[test]
    fn lounge_and_sunbathing_sets_place_their_pieces() {
        let mut s = Session::new(Default::default());
        s.run(
            "furniture.add_set",
            json!({"set": "lounge", "position": [0, 0]}),
        )
        .unwrap();
        s.run(
            "furniture.add_set",
            json!({"set": "sunbathing", "position": [20, 0], "rotation_deg": 90}),
        )
        .unwrap();
        let kinds: Vec<_> = pieces(&s).into_iter().map(|(k, _)| k.name()).collect();
        assert_eq!(
            kinds,
            [
                "sofa",
                "armchair",
                "armchair",
                "side_table",
                "lounger",
                "lounger",
                "side_table",
                "parasol"
            ]
        );
    }

    #[test]
    fn a_project_with_furniture_saves_loads_and_an_old_one_still_loads() {
        let mut s = Session::new(Default::default());
        s.run(
            "furniture.add_set",
            json!({"set": "dining", "position": [3, 3]}),
        )
        .unwrap();
        s.run(
            "furniture.add",
            json!({"kind": "table", "position": [9, 9], "round": true}),
        )
        .unwrap();
        let json = serde_json::to_string(s.project()).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, s.project());
        // A project from before: no `furniture` on any shape.
        let mut v: Value = serde_json::from_str(&json).unwrap();
        for shape in v["shapes"].as_array_mut().unwrap() {
            shape.as_object_mut().unwrap().remove("furniture");
        }
        let old: Project = serde_json::from_value(v).unwrap();
        assert!(
            old.shapes
                .iter()
                .all(|x| matches!(x, Shape::Extrusion(e) if e.furniture.is_none()))
        );
        // Plain extrusions do not write the field.
        let mut plain = Session::new(Default::default());
        plain
            .run(
                "shape.add_extrusion",
                json!({"name": "B", "outline": [[0, 0], [1, 0], [1, 1], [0, 1]], "height": 1}),
            )
            .unwrap();
        assert!(
            !serde_json::to_string(plain.project())
                .unwrap()
                .contains("furniture")
        );
    }

    #[test]
    fn quantities_list_the_pieces_per_kind_with_their_dimensions() {
        let mut s = Session::new(Default::default());
        s.run(
            "furniture.add_set",
            json!({"set": "dining", "position": [3, 3], "seats": 4}),
        )
        .unwrap();
        s.run(
            "furniture.add",
            json!({"kind": "sofa", "position": [9, 9], "length": 2.6}),
        )
        .unwrap();
        let q = s.run("query.quantities", json!({})).unwrap();
        let rows = q["furniture"].as_array().unwrap();
        let row = |k: &str| rows.iter().find(|r| r["kind"] == k).unwrap();
        assert_eq!(row("chair")["count"], 4);
        assert_eq!(row("table")["count"], 1);
        let sofa = &row("sofa")["pieces"][0];
        assert_eq!(
            sofa["length_m"].as_f64().map(|v| (v * 10.0).round()),
            Some(26.0)
        );
        assert!(sofa["depth_m"].as_f64().is_some() && sofa["height_m"].as_f64().is_some());
        let shape = q["shapes"].as_array().unwrap().last().unwrap();
        assert_eq!(shape["furniture"]["kind"], "sofa");
    }
}
