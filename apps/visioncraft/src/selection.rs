//! What the person has selected, as one model across the modes, for AI agents (`ui.get` ->
//! `selection`, `ui.select`): a person clicks something and says "make this a birch", and the
//! agent picks up what is selected.
//!
//! The window's modes keep their own selections (Draw's shape, edge and vertex, Objects' planting,
//! plant and lamp, a model, a plan image). Each frame the window collects them into [`Key`]s and
//! hands them to [`SelectionState::observe`], which bumps the `revision` when they changed. An
//! agent's `ui.select` goes the other way: [`SelectionState::set`] records the keys at once (so
//! the next `ui.get` shows them) and queues them in `apply`, which the window then puts into the
//! modes' own states, with the handles and mode a click would give.
//!
//! A [`Key`] is only identity (kind, id, sub-item). Names and one-line summaries are made when an
//! agent asks ([`describe`]), from the project, so they are always current.

use serde_json::{Value, json};
use visioncraft_core::planting::{Generator, Placement};
use visioncraft_core::shape::Shape;
use visioncraft_core::{Asset, Project};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A drawn shape: area, block, wall, terrace, stairs, fence, bed, furniture.
    Shape,
    /// One plant of a planting (`sub.index`).
    Placement,
    /// A whole planting (generator).
    Planting,
    Model,
    Lamp,
    /// A plan image under the drawing.
    Underlay,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Shape => "shape",
            Self::Placement => "placement",
            Self::Planting => "planting",
            Self::Model => "model",
            Self::Lamp => "lamp",
            Self::Underlay => "underlay",
        }
    }

    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "shape" => Self::Shape,
            "placement" => Self::Placement,
            "planting" => Self::Planting,
            "model" => Self::Model,
            "lamp" => Self::Lamp,
            "underlay" => Self::Underlay,
            "viewpoint" | "wall_photo" | "film" | "material" => {
                return Err(format!(
                    "`{s}` cannot be selected in the window (kinds: shape, placement, planting, model, lamp, underlay)"
                ));
            }
            _ => {
                return Err(format!(
                    "unknown kind `{s}` (kinds: shape, placement, planting, model, lamp, underlay)"
                ));
            }
        })
    }
}

/// One selected thing: what it is, its id, and the part of it that is picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub kind: Kind,
    /// The shape, planting, model, lamp or underlay id; for a placement its planting's id.
    pub id: String,
    pub edge: Option<usize>,
    pub vertex: Option<usize>,
    /// A placement's number in its planting.
    pub index: Option<usize>,
}

impl Key {
    pub fn new(kind: Kind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
            edge: None,
            vertex: None,
            index: None,
        }
    }

    fn sub(&self) -> Option<Value> {
        let mut o = serde_json::Map::new();
        for (k, v) in [
            ("edge", self.edge),
            ("vertex", self.vertex),
            ("index", self.index),
        ] {
            if let Some(v) = v {
                o.insert(k.into(), v.into());
            }
        }
        (!o.is_empty()).then_some(Value::Object(o))
    }

    /// The window's mode where a click would have made this selection (`None`: no mode, the plain
    /// 3D view where models are picked).
    pub fn mode(&self) -> Option<crate::ui::Mode> {
        use crate::ui::Mode;
        match self.kind {
            Kind::Shape | Kind::Underlay => Some(Mode::Draw),
            Kind::Placement | Kind::Planting | Kind::Lamp => Some(Mode::Objects),
            Kind::Model => None,
        }
    }
}

/// The selection, its revision and the last point clicked.
#[derive(Default)]
pub struct SelectionState {
    pub items: Vec<Key>,
    /// The last ground or surface point clicked, in world metres.
    pub point: Option<[f32; 3]>,
    /// Bumps whenever the items or the point change.
    pub revision: u64,
    /// Keys an agent selected, for the window to put into the modes (see the module doc).
    pub apply: Option<Vec<Key>>,
}

impl SelectionState {
    /// Takes what the modes show now; true when it differs from before (the revision bumped).
    pub fn observe(&mut self, now: Vec<Key>) -> bool {
        if now == self.items {
            return false;
        }
        self.items = now;
        self.revision += 1;
        true
    }

    /// A click on the ground or a surface.
    pub fn set_point(&mut self, p: Option<[f32; 3]>) {
        let p = p.map(|p| p.map(|v| (v * 100.0).round() / 100.0));
        if p != self.point {
            self.point = p;
            self.revision += 1;
        }
    }

    /// An agent's selection: shown at once, and queued for the modes.
    pub fn set(&mut self, items: Vec<Key>) {
        if items != self.items {
            self.items.clone_from(&items);
            self.revision += 1;
        }
        self.apply = Some(items);
    }

    /// `ui.get`'s `selection`.
    pub fn to_json(&self, project: &Project, assets: &[Asset]) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|k| {
                let (name, summary) = describe(project, assets, k);
                let mut o =
                    json!({"kind": k.kind.name(), "id": k.id, "name": name, "summary": summary});
                if let Some(sub) = k.sub() {
                    o["sub"] = sub;
                }
                o
            })
            .collect();
        let mut o = json!({"items": items, "revision": self.revision});
        if let Some(p) = self.point {
            o["point"] = json!(p);
        }
        o
    }

    /// The old `ui.get` field: `selected.planting` and `selected.model`.
    pub fn legacy(&self) -> Value {
        let of = |kinds: &[Kind]| {
            self.items
                .iter()
                .find(|k| kinds.contains(&k.kind))
                .map(|k| k.id.clone())
        };
        json!({"planting": of(&[Kind::Planting, Kind::Placement]), "model": of(&[Kind::Model])})
    }
}

/// The keys of `ui.select`'s parameters, checked against the project; `None` for "clear". Takes
/// `{items: [{kind, id, sub?: {edge?, vertex?, index?}}]}` (`null` or `[]` clears) and, as
/// before, `{planting, model}`.
pub fn parse(project: &Project, params: &Value) -> Result<Vec<Key>, String> {
    let mut keys = Vec::new();
    match params.get("items") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for v in a {
                keys.push(parse_item(v)?);
            }
        }
        Some(other) => {
            return Err(format!(
                "`items` is a list of {{kind, id, sub?}}, not {other}"
            ));
        }
    }
    for (name, kind) in [("planting", Kind::Planting), ("model", Kind::Model)] {
        if let Some(id) = params.get(name).and_then(Value::as_str) {
            keys.push(Key::new(kind, id));
        }
    }
    if keys.len() > 1 {
        return Err(
            "the window selects one thing at a time: give one item (a plant of a planting is the \
             `placement` kind)"
                .into(),
        );
    }
    for k in &keys {
        check(project, k)?;
    }
    Ok(keys)
}

fn parse_item(v: &Value) -> Result<Key, String> {
    let kind = Kind::parse(
        v.get("kind")
            .and_then(Value::as_str)
            .ok_or("an item is {kind, id, sub?}: `kind` is missing")?,
    )?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("an item is {kind, id, sub?}: `id` is missing")?;
    let mut k = Key::new(kind, id);
    if let Some(sub) = v.get("sub").filter(|s| !s.is_null()) {
        let n = |key: &str| -> Result<Option<usize>, String> {
            match sub.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(x) => x
                    .as_u64()
                    .map(|n| Some(n as usize))
                    .ok_or_else(|| format!("sub.{key} is a whole number, not {x}")),
            }
        };
        (k.edge, k.vertex, k.index) = (n("edge")?, n("vertex")?, n("index")?);
    }
    Ok(k)
}

/// Whether the thing exists (and its sub-item), with an error that says what is missing.
pub fn check(project: &Project, k: &Key) -> Result<(), String> {
    let id = &k.id;
    match k.kind {
        Kind::Shape => {
            let s = project
                .shapes
                .iter()
                .find(|s| s.id() == id)
                .ok_or_else(|| format!("no shape `{id}`"))?;
            let n = s.points().len();
            if k.vertex.is_some_and(|v| v >= n) {
                return Err(format!(
                    "shape `{id}` has {n} vertices: sub.vertex is out of range"
                ));
            }
            let edges = if s.is_closed() {
                n
            } else {
                n.saturating_sub(1)
            };
            if k.edge.is_some_and(|e| e >= edges) {
                return Err(format!(
                    "shape `{id}` has {edges} edges: sub.edge is out of range"
                ));
            }
        }
        Kind::Planting => {
            let g = generator(project, id)?;
            if let Some(v) = k.vertex
                && v >= crate::handles::polyline(g).map_or(0, |(p, _)| p.len())
            {
                return Err(format!("planting `{id}` has no vertex {v}"));
            }
        }
        Kind::Placement => {
            let g = generator(project, id)?;
            let i = k
                .index
                .ok_or("a placement needs `sub.index` (its number in the planting, from 0)")?;
            let list = plants(project, g);
            if !list.iter().any(|(j, _)| *j == i) {
                return Err(format!(
                    "planting `{id}` has {} plant(s) and none is number {i} (out of range, or taken out)",
                    list.len()
                ));
            }
        }
        Kind::Model => {
            if !project.models.iter().any(|m| &m.id == id) {
                return Err(format!("no model `{id}`"));
            }
        }
        Kind::Lamp => {
            if !project.lights.iter().any(|l| &l.id == id) {
                return Err(format!("no lamp `{id}` (a light's id)"));
            }
        }
        Kind::Underlay => {
            if !project.underlays.iter().any(|u| &u.id == id) {
                return Err(format!("no underlay `{id}`"));
            }
        }
    }
    Ok(())
}

fn generator<'a>(project: &'a Project, id: &str) -> Result<&'a Generator, String> {
    project
        .generators
        .iter()
        .find(|g| g.id() == id)
        .ok_or_else(|| format!("no planting `{id}`"))
}

/// The plants of a planting with their numbers, the ones `planting.replace_placement` and
/// `planting.remove_placement` count by (a plant's `sub.index`), with the planting's edits
/// applied (a plant taken out is missing, the others keep their numbers): what the generator
/// evaluates to before the window leaves out what stands in a building or in the water.
pub fn plants(project: &Project, g: &Generator) -> Vec<(usize, Placement)> {
    if matches!(g, Generator::SurfaceFill(_)) {
        return Vec::new();
    }
    let floor = visioncraft_core::terrace::ground_fn(project);
    let ground = |x: f32, z: f32| floor(x, z).or(Some(0.0));
    visioncraft_core::planting::evaluate_numbered(g, &ground).0
}

/// Plant number `i` of `g`, if it is there.
pub fn plant(project: &Project, g: &Generator, i: usize) -> Option<Placement> {
    plants(project, g)
        .into_iter()
        .find(|(j, _)| *j == i)
        .map(|(_, p)| p)
}

/// The plant of `g` nearest to (x, z) within `within` metres: its number and place.
pub fn plant_near(
    project: &Project,
    g: &Generator,
    x: f32,
    z: f32,
    within: f32,
) -> Option<(usize, [f32; 3])> {
    plants(project, g)
        .into_iter()
        .map(|(i, p)| (i, p.position, (p.position[0] - x).hypot(p.position[2] - z)))
        .filter(|c| c.2 <= within)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|c| (c.0, c.1))
}

/// A number for a summary: at most two decimals, without trailing zeros.
fn n(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn at(p: [f32; 3]) -> String {
    format!("[{}, {}, {}]", n(p[0]), n(p[1]), n(p[2]))
}

fn polygon_centre(points: &[[f32; 2]]) -> [f32; 2] {
    let k = points.len().max(1) as f32;
    let s = points
        .iter()
        .fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
    [s[0] / k, s[1] / k]
}

/// A species' or object's name for `asset` (the asset id when nothing names it).
pub fn species_name(project: &Project, assets: &[Asset], asset: &str) -> String {
    if let Some(s) = asset
        .strip_prefix("flora:")
        .and_then(visioncraft_flora::Species::by_id)
    {
        return s.common_name.trim_end_matches(" (in blossom)").to_string();
    }
    if let Some(c) = project.species.iter().find(|c| c.id == asset) {
        return c.name.clone();
    }
    if let Some(o) = asset
        .strip_prefix("user:")
        .and_then(|id| project.objects.iter().find(|o| o.id == id))
    {
        return o.name.clone();
    }
    assets
        .iter()
        .find(|a| a.id == asset)
        .map_or_else(|| asset.to_string(), |a| a.name.clone())
}

/// The name and the one-line summary of a selected thing.
pub fn describe(project: &Project, assets: &[Asset], k: &Key) -> (String, String) {
    let missing = || (k.id.clone(), "no longer in the project".to_string());
    match k.kind {
        Kind::Shape => match project.shapes.iter().find(|s| s.id() == k.id) {
            Some(s) => (s.name().to_string(), shape_summary(project, s, k)),
            None => missing(),
        },
        Kind::Planting => match project.generators.iter().find(|g| g.id() == k.id) {
            Some(g) => (g.name().to_string(), planting_summary(project, assets, g)),
            None => missing(),
        },
        Kind::Placement => {
            let Some(g) = project.generators.iter().find(|g| g.id() == k.id) else {
                return missing();
            };
            let list = plants(project, g);
            let i = k.index.unwrap_or(0);
            let Some(p) = &plant(project, g, i) else {
                return (g.name().to_string(), "that plant is gone".to_string());
            };
            let name = species_name(project, assets, &p.asset);
            let (h, _) = crate::plant_scene::plant_size(project, assets, &p.asset);
            (
                name.clone(),
                format!(
                    "{name} ({}), plant number {i} of {} in planting `{}` ({}), about {} m tall when grown (scale {}), turned {} deg, at {}",
                    p.asset,
                    list.len(),
                    g.id(),
                    g.name(),
                    n(h * p.scale),
                    n(p.scale),
                    n(p.rotation_y_deg),
                    at(p.position),
                ),
            )
        }
        Kind::Model => match project.models.iter().find(|m| m.id == k.id) {
            Some(m) => {
                let t = &m.transform;
                (
                    m.name.clone(),
                    format!(
                        "linked model from {}, at {}, turned {} deg, scale {}",
                        m.source,
                        at(t.translation),
                        n(t.rotation_y_deg),
                        n(t.scale)
                    ),
                )
            }
            None => missing(),
        },
        Kind::Lamp => match project.lights.iter().find(|l| l.id == k.id) {
            Some(l) => {
                let kind = match l.kind {
                    visioncraft_core::LightKind::Point => "point light".to_string(),
                    visioncraft_core::LightKind::Spot { outer_cone_deg, .. } => {
                        format!("spot light ({} deg cone)", n(outer_cone_deg * 2.0))
                    }
                };
                (
                    l.name.clone(),
                    format!(
                        "{kind}, {} lm, {} K, range {} m, {}, at {}",
                        n(l.lumens),
                        n(l.color_temperature_k),
                        n(l.range_m),
                        if l.enabled { "on" } else { "off" },
                        at(l.position)
                    ),
                )
            }
            None => missing(),
        },
        Kind::Underlay => match project.underlays.iter().find(|u| u.id == k.id) {
            Some(u) => (
                u.name.clone(),
                format!(
                    "plan image {} ({}x{} px), {} m per pixel, turned {} deg, origin [{}, {}]",
                    u.file,
                    u.size_px[0],
                    u.size_px[1],
                    n(u.placement.metres_per_pixel),
                    n(u.placement.rotation_deg),
                    n(u.placement.origin[0]),
                    n(u.placement.origin[1]),
                ),
            ),
            None => missing(),
        },
    }
}

fn material_names(project: &Project, owner: &str) -> String {
    let mut names: Vec<String> = Vec::new();
    for a in project
        .assignments
        .iter()
        .filter(|a| a.target.owner() == owner)
    {
        let name = project
            .materials
            .iter()
            .find(|m| m.id == a.material)
            .map_or(a.material.clone(), |m| m.name.clone());
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        String::new()
    } else {
        format!(", material {}", names.join(" / "))
    }
}

fn shape_summary(project: &Project, s: &Shape, k: &Key) -> String {
    use visioncraft_core::measure::{polygon_area, polyline_length};
    let outline = s.outline();
    let c = polygon_centre(s.points());
    let area = polygon_area(&outline);
    let length = || {
        let pts: Vec<[f32; 3]> = outline.iter().map(|q| [q[0], 0.0, q[1]]).collect();
        polyline_length(&pts)
    };
    let what = match s {
        Shape::Area(a) => {
            if let Some(t) = &a.terrace {
                format!(
                    "terrace, floor at {} m, {} m2{}",
                    n(a.elevation),
                    n(area),
                    if t.parapet_m > 0.0 {
                        format!(", seat wall {} m", n(t.parapet_m))
                    } else {
                        String::new()
                    }
                )
            } else if a.edge.is_some() {
                format!("raised area, top at {} m, {} m2", n(a.elevation), n(area))
            } else if a.drape {
                format!("area on the ground, {} m2", n(area))
            } else {
                format!("area at {} m, {} m2", n(a.elevation), n(area))
            }
        }
        Shape::Extrusion(e) => {
            if let Some(st) = &e.stairs {
                let steps = st
                    .geometry(&e.points, e.height)
                    .map(|g| g.flights().iter().map(|f| f.0).sum::<usize>());
                format!(
                    "stairs, {} steps, rising {} m from {} m",
                    steps.map_or("?".to_string(), |s| s.to_string()),
                    n(e.height),
                    n(e.base)
                )
            } else if let Some(f) = &e.fence {
                let kind = serde_json::to_value(f.kind).unwrap_or_default();
                format!(
                    "fence ({}), {} m long, {} m high",
                    kind.as_str().unwrap_or("fence").replace('_', " "),
                    n(length()),
                    n(e.height)
                )
            } else if let Some(f) = &e.furniture {
                let kind = serde_json::to_value(f.kind).unwrap_or_default();
                let (l, d) = crate::draw::rect_size(&e.points).unwrap_or((0.0, 0.0));
                format!(
                    "furniture: {}, {} x {} x {} m",
                    kind.as_str().unwrap_or("piece").replace('_', " "),
                    n(l),
                    n(d),
                    n(e.height)
                )
            } else if let Some(b) = &e.raised_bed {
                format!(
                    "raised bed, {} m2, walls {} m high",
                    n(area),
                    n(e.height.max(b.wall_thickness_m))
                )
            } else if e.closed && e.house.is_some() {
                format!("house block, {} m2, {} m high", n(area), n(e.height))
            } else if e.closed {
                format!("block, {} m2, {} m high", n(area), n(e.height))
            } else {
                format!(
                    "wall, {} m long, {} m high, {} m thick",
                    n(length()),
                    n(e.height),
                    n(e.thickness)
                )
            }
        }
    };
    let sub = match (k.vertex, k.edge) {
        (Some(v), _) => format!(", vertex {v} picked"),
        (None, Some(e)) => format!(", edge {e} picked"),
        _ => String::new(),
    };
    format!(
        "{what}, layer {}{}, centre [{}, {}]{sub}",
        s.layer(),
        material_names(project, s.id()),
        n(c[0]),
        n(c[1])
    )
}

fn planting_summary(project: &Project, assets: &[Asset], g: &Generator) -> String {
    let kind = match g {
        Generator::Single(_) => "single plant",
        Generator::SplineMass(_) => "planting along a line",
        Generator::AreaScatter(a) if a.border.is_some() => "border",
        Generator::AreaScatter(_) => "area planting",
        Generator::MixedBrush(_) => "brushed planting",
        Generator::SurfaceFill(_) => "planting on a surface",
        Generator::Cluster(_) => "cluster",
    };
    let list = plants(project, g);
    // What it plants now, with the edits: the plants' own species, in order of first sight.
    let mut kinds: Vec<&str> = Vec::new();
    for (_, p) in &list {
        if !kinds.contains(&p.asset.as_str()) {
            kinds.push(&p.asset);
        }
    }
    if kinds.is_empty() {
        kinds = g.assets();
    }
    let species: Vec<String> = kinds
        .iter()
        .take(4)
        .map(|a| species_name(project, assets, a))
        .collect();
    let more = kinds.len().saturating_sub(4);
    let centre = if list.is_empty() {
        String::new()
    } else {
        let k = list.len() as f32;
        let (x, z) = list.iter().fold((0.0, 0.0), |a, (_, p)| {
            (a.0 + p.position[0], a.1 + p.position[2])
        });
        format!(", around [{}, {}]", n(x / k), n(z / k))
    };
    format!(
        "{kind}, {} plant(s) of {}{}, layer {}{centre}",
        list.len(),
        species.join(", "),
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        },
        g.layer()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_core::Session;

    fn garden() -> Session {
        let mut s = Session::new(Project::default());
        for (c, p) in [
            (
                "shape.add_area",
                json!({"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 6], [0, 6]]}),
            ),
            (
                "planting.add_single",
                json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [4, 0, 3]}),
            ),
            (
                "planting.add_cluster",
                json!({"name": "Grove", "center": [8, 8], "radius_m": 2, "count": 4,
                    "asset_palette": [{"asset": "flora:betula-pendula", "weight": 1}]}),
            ),
        ] {
            s.run(c, p).unwrap();
        }
        s
    }

    #[test]
    fn parse_takes_one_item_in_either_form_and_clears_on_null() {
        let s = garden();
        let p = s.project();
        let one = parse(
            p,
            &json!({"items": [{"kind": "shape", "id": "S1", "sub": {"vertex": 2}}]}),
        )
        .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].vertex, Some(2));
        let old = parse(p, &json!({"planting": "g1"})).unwrap();
        assert_eq!(old, vec![Key::new(Kind::Planting, "g1")]);
        assert!(parse(p, &json!({"items": null})).unwrap().is_empty());
        assert!(parse(p, &json!({"items": []})).unwrap().is_empty());
        assert!(parse(p, &json!({})).unwrap().is_empty());
    }

    #[test]
    fn unknown_ids_and_bad_items_give_clear_errors() {
        let s = garden();
        let p = s.project();
        for (items, want) in [
            (json!([{"kind": "shape", "id": "S9"}]), "no shape `S9`"),
            (
                json!([{"kind": "planting", "id": "g9"}]),
                "no planting `g9`",
            ),
            (json!([{"kind": "model", "id": "m9"}]), "no model `m9`"),
            (json!([{"kind": "lamp", "id": "l9"}]), "no lamp `l9`"),
            (
                json!([{"kind": "underlay", "id": "u9"}]),
                "no underlay `u9`",
            ),
            (
                json!([{"kind": "placement", "id": "g2", "sub": {"index": 9}}]),
                "out of range",
            ),
            (json!([{"kind": "placement", "id": "g2"}]), "sub.index"),
            (
                json!([{"kind": "shape", "id": "S1", "sub": {"vertex": 9}}]),
                "out of range",
            ),
            (json!([{"kind": "tree", "id": "x"}]), "unknown kind"),
            (
                json!([{"kind": "viewpoint", "id": "v1"}]),
                "cannot be selected",
            ),
            (json!([{"id": "x"}]), "`kind` is missing"),
            (
                json!([{"kind": "shape", "id": "S1"}, {"kind": "planting", "id": "g1"}]),
                "one thing at a time",
            ),
        ] {
            let e = parse(p, &json!({ "items": items })).unwrap_err();
            assert!(e.contains(want), "{items}: {e}");
        }
    }

    #[test]
    fn summaries_carry_what_an_agent_needs() {
        let s = garden();
        let p = s.project();
        let say = |k: Key| describe(p, &[], &k).1;
        let shape = say(Key::new(Kind::Shape, "S1"));
        assert!(shape.contains("area") && shape.contains("60 m2"), "{shape}");
        let mut vertex = Key::new(Kind::Shape, "S1");
        vertex.vertex = Some(1);
        assert!(say(vertex).contains("vertex 1"));
        let oak = say(Key::new(Kind::Planting, "g1"));
        assert!(
            oak.contains("single plant") && oak.contains("Oak") || oak.contains("oak"),
            "{oak}"
        );
        let mut tree = Key::new(Kind::Placement, "g2");
        tree.index = Some(1);
        let (name, line) = describe(p, &[], &tree);
        assert!(name.to_lowercase().contains("birch"), "{name}");
        assert!(
            line.contains("flora:betula-pendula")
                && line.contains("plant number 1 of 4")
                && line.contains("m tall")
                && line.contains("at ["),
            "{line}"
        );
    }

    #[test]
    fn shape_kinds_say_their_specs() {
        let mut s = Session::new(Project::default());
        s.run(
            "terrace.add",
            json!({"name": "Patio", "outline": [[0, 0], [4, 0], [4, 3], [0, 3]], "floor": -0.3}),
        )
        .unwrap();
        s.run(
            "fence.add",
            json!({"name": "Fence", "line": [[0, 0], [5, 0]], "height": 1.8}),
        )
        .unwrap();
        s.run(
            "furniture.add",
            json!({"kind": "bench", "position": [2, 1]}),
        )
        .unwrap();
        let p = s.project();
        let lines: Vec<String> = p
            .shapes
            .iter()
            .map(|sh| describe(p, &[], &Key::new(Kind::Shape, sh.id())).1)
            .collect();
        assert!(
            lines[0].starts_with("terrace, floor at -0.3 m"),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("fence (") && lines[1].contains("5 m long"),
            "{lines:?}"
        );
        assert!(lines[2].starts_with("furniture: bench"), "{lines:?}");
    }

    #[test]
    fn the_summary_sees_an_edited_plant_and_a_removed_one_is_not_selectable() {
        let mut s = garden();
        s.run(
            "planting.replace_placement",
            json!({"planting": "g2", "index": 1, "asset": "flora:quercus-robur"}),
        )
        .unwrap();
        s.run(
            "planting.remove_placement",
            json!({"planting": "g2", "index": 3}),
        )
        .unwrap();
        let p = s.project();
        let mut tree = Key::new(Kind::Placement, "g2");
        tree.index = Some(1);
        let line = describe(p, &[], &tree).1;
        assert!(
            line.contains("flora:quercus-robur") && line.contains("plant number 1 of 3"),
            "{line}"
        );
        tree.index = Some(3);
        let e = check(p, &tree).unwrap_err();
        assert!(e.contains("none is number 3"), "{e}");
        let grove = describe(p, &[], &Key::new(Kind::Planting, "g2")).1;
        assert!(grove.contains("3 plant(s)"), "{grove}");
    }

    #[test]
    fn the_revision_bumps_on_a_change_only() {
        let mut st = SelectionState::default();
        assert_eq!(st.revision, 0);
        assert!(!st.observe(Vec::new()));
        assert!(st.observe(vec![Key::new(Kind::Model, "m1")]));
        assert_eq!(st.revision, 1);
        assert!(!st.observe(vec![Key::new(Kind::Model, "m1")]));
        assert_eq!(st.revision, 1);
        st.set(vec![Key::new(Kind::Model, "m1")]);
        assert_eq!(st.revision, 1, "the same selection is not a change");
        assert!(st.apply.is_some(), "but it is still applied");
        st.set(Vec::new());
        assert_eq!(st.revision, 2);
        st.set_point(Some([1.0, 0.0, 2.0]));
        assert_eq!(st.revision, 3);
        st.set_point(Some([1.0, 0.0, 2.0]));
        assert_eq!(st.revision, 3);
        let j = st.to_json(&Project::default(), &[]);
        assert_eq!(j["point"], json!([1.0, 0.0, 2.0]));
        assert_eq!(j["revision"], 3);
        assert_eq!(j["items"], json!([]));
    }

    #[test]
    fn the_nearest_plant_is_found_by_its_place() {
        let s = garden();
        let p = s.project();
        let g = &p.generators[0];
        assert_eq!(plant_near(p, g, 4.1, 3.0, 0.5).map(|c| c.0), Some(0));
        assert!(plant_near(p, g, 9.0, 9.0, 0.5).is_none());
    }
}
