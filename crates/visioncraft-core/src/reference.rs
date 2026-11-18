//! Things by reference: a point a command takes may be `[x, y, z]` or the name of something in
//! the project, which resolves to a centre, a height and a radius from its bounding box.
//!
//! A reference is a shape id (`S6`), a planting (`g4`: all its plants) or one plant of it
//! (`g4#12`, counted from 0), a model, a lamp or a viewpoint id, `plot` (everything drawn), or
//! a name matched without regard to case (`Pond`). A plant is as big as the growth model makes it
//! `years` after planting (`None`: full grown), so a tree's crown height and radius are known.
//! An unknown reference is refused with the close matches.
//!
//! Compass words use the drawing's axes: -Z is north, +X is east.

use crate::growth::growth_of;
use crate::planting::{Generator, Placement, evaluate};
use crate::project::Project;
use crate::shape::Shape;
use serde_json::{Value, json};

/// What a reference stands for: a box, with a name and what it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Extent {
    /// What was asked for (`S6`, `g4#12`, `Pond`), as given.
    pub reference: String,
    /// `shape`, `planting`, `plant`, `model`, `light`, `viewpoint`, `plot` or `point`.
    pub kind: &'static str,
    /// The item's name, or its id when it has none.
    pub name: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Another item the reference could have meant (a pond and a viewpoint of that name).
    pub note: Option<String>,
}

impl Extent {
    pub fn point(p: [f32; 3]) -> Extent {
        Extent {
            reference: String::new(),
            kind: "point",
            name: String::new(),
            min: p,
            max: p,
            note: None,
        }
    }

    pub fn center(&self) -> [f32; 3] {
        std::array::from_fn(|k| (self.min[k] + self.max[k]) / 2.0)
    }

    /// The point to look at: the centre of the box, except for the plot, whose box is as high as
    /// its tallest tree: a camera looks at the ground there, a little above it.
    pub fn look_point(&self) -> [f32; 3] {
        let mut c = self.center();
        if self.kind == "plot" {
            c[1] = self.min[1] + 0.15 * self.height();
        }
        c
    }

    /// Height of the box.
    pub fn height(&self) -> f32 {
        self.max[1] - self.min[1]
    }

    /// Width (x) and depth (z) of the box.
    pub fn footprint(&self) -> [f32; 2] {
        [self.max[0] - self.min[0], self.max[2] - self.min[2]]
    }

    /// Half the diagonal of the footprint: the radius of the circle that holds it.
    pub fn radius(&self) -> f32 {
        let f = self.footprint();
        f[0].hypot(f[1]) / 2.0
    }

    /// The box as JSON, as `film.*` commands take it in place of a point.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "ref": self.reference, "kind": self.kind, "name": self.name,
            "center": self.center(), "look_at": self.look_point(), "height": self.height(),
            "radius": self.radius(),
            "min": self.min, "max": self.max,
        });
        if let Some(n) = &self.note {
            v["note"] = n.clone().into();
        }
        v
    }

    /// The box from [`Extent::to_json`] (needs `min` and `max`).
    pub fn from_json(v: &Value) -> Option<Extent> {
        let p = |k: &str| serde_json::from_value::<[f32; 3]>(v.get(k)?.clone()).ok();
        Some(Extent {
            reference: v.get("ref")?.as_str()?.to_string(),
            kind: if v.get("kind").and_then(Value::as_str) == Some("plot") {
                "plot"
            } else {
                "reference"
            },
            name: v.get("name").and_then(Value::as_str)?.to_string(),
            min: p("min")?,
            max: p("max")?,
            note: None,
        })
    }

    fn union(&mut self, o: &Extent) {
        for k in 0..3 {
            self.min[k] = self.min[k].min(o.min[k]);
            self.max[k] = self.max[k].max(o.max[k]);
        }
    }
}

/// A model's world box by its id.
pub type ModelBounds<'a> = dyn Fn(&str) -> Option<[[f32; 3]; 2]> + 'a;
/// The plants a planting makes.
pub type PlantsOf<'a> = dyn Fn(&Generator) -> Vec<Placement> + 'a;

/// What references resolve against: the project, and where the host knows more than the core
/// (model geometry, plants placed with the building rule, the ground it stands on).
pub struct Scene<'a> {
    pub project: &'a Project,
    /// Years after planting the plants are measured at; `None`: full grown.
    pub years: Option<f32>,
    pub model_bounds: Option<&'a ModelBounds<'a>>,
    pub placements: Option<&'a PlantsOf<'a>>,
    pub ground: Option<&'a dyn Fn(f32, f32) -> f32>,
}

impl<'a> Scene<'a> {
    /// What the core alone knows: shapes by their outlines, plants evaluated over the terrain,
    /// a model as its origin.
    pub fn new(project: &'a Project, years: Option<f32>) -> Scene<'a> {
        Scene {
            project,
            years,
            model_bounds: None,
            placements: None,
            ground: None,
        }
    }

    pub fn ground_at(&self, x: f32, z: f32) -> f32 {
        match self.ground {
            Some(g) => g(x, z),
            None => crate::terrace::ground_fn(self.project)(x, z).unwrap_or(0.0),
        }
    }

    pub fn placements_of(&self, g: &Generator) -> Vec<Placement> {
        match self.placements {
            Some(f) => f(g),
            None => {
                let ground = crate::terrace::ground_fn(self.project);
                evaluate(g, &ground).placements
            }
        }
    }

    fn visible(&self, layer: &str) -> bool {
        self.project.layer_visible(layer)
    }
}

/// The box of one plant at `years` after planting: its crown, from the ground up.
pub fn plant_extent(g: &Generator, pl: &Placement, years: Option<f32>) -> ([f32; 3], [f32; 3]) {
    let gr = growth_of(&pl.asset, None);
    let age = match years {
        Some(y) => g.planted_age_years().unwrap_or(gr.planted_age_years) + y,
        None => gr.years_to_maturity * 4.0,
    };
    let size = gr.size(age);
    let h = gr.mature_height_m * size.height * pl.scale;
    let w = (gr.mature_width_m * size.width * pl.scale).max(0.05);
    let p = pl.position;
    (
        [p[0] - w / 2.0, p[1], p[2] - w / 2.0],
        [p[0] + w / 2.0, p[1] + h, p[2] + w / 2.0],
    )
}

fn named(kind: &'static str, reference: &str, name: &str, id: &str, b: [[f32; 3]; 2]) -> Extent {
    Extent {
        reference: reference.to_string(),
        kind,
        name: if name.is_empty() { id } else { name }.to_string(),
        min: b[0],
        max: b[1],
        note: None,
    }
}

fn shape_extent(scene: &Scene, s: &Shape, reference: &str) -> Option<Extent> {
    let pts = match s {
        Shape::Area(a) => a.outline().into_owned(),
        Shape::Extrusion(e) => e.outline().into_owned(),
    };
    if pts.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in &pts {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let (cx, cz) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let ground = scene.ground_at(cx, cz);
    let (y0, y1, grow) = match s {
        Shape::Area(a) => {
            let terrain = crate::terrace::ground_fn(scene.project)(cx, cz).unwrap_or(0.0);
            let y = if a.drape { terrain } else { 0.0 } + a.elevation;
            (y, y + 0.02, 0.0)
        }
        Shape::Extrusion(e) => (
            ground + e.base,
            ground + e.base + e.height,
            if e.closed { 0.0 } else { e.thickness / 2.0 },
        ),
    };
    Some(named(
        "shape",
        reference,
        s.name(),
        s.id(),
        [
            [lo[0] - grow, y0, lo[1] - grow],
            [hi[0] + grow, y1, hi[1] + grow],
        ],
    ))
}

fn generator_extent(
    scene: &Scene,
    g: &Generator,
    reference: &str,
    index: Option<usize>,
) -> Result<Extent, String> {
    let all = scene.placements_of(g);
    let mut out: Option<Extent> = None;
    for (i, pl) in all.iter().enumerate() {
        if index.is_some_and(|n| n != i) {
            continue;
        }
        let (min, max) = plant_extent(g, pl, scene.years);
        let e = named("plant", reference, g.name(), g.id(), [min, max]);
        match &mut out {
            Some(o) => o.union(&e),
            None => out = Some(e),
        }
    }
    let mut e = out.ok_or_else(|| match index {
        Some(n) => format!(
            "`{reference}`: planting {} ({}) has {} plants, counted from 0; there is no plant {n}",
            g.id(),
            g.name(),
            all.len()
        ),
        None => format!(
            "`{reference}`: planting {} ({}) has no plants",
            g.id(),
            g.name()
        ),
    })?;
    if index.is_none() {
        e.kind = "planting";
    }
    Ok(e)
}

fn model_extent(scene: &Scene, id: &str, reference: &str) -> Option<Extent> {
    let m = scene.project.models.iter().find(|m| m.id == id)?;
    let b = scene
        .model_bounds
        .and_then(|f| f(id))
        .unwrap_or([m.transform.translation; 2]);
    Some(named("model", reference, &m.name, &m.id, b))
}

/// The box of everything drawn: visible shapes and plantings outside the context layers.
fn plot_extent(scene: &Scene, reference: &str) -> Result<Extent, String> {
    let p = scene.project;
    let mut out: Option<Extent> = None;
    let mut add = |e: Extent| match &mut out {
        Some(o) => o.union(&e),
        None => out = Some(e),
    };
    for s in &p.shapes {
        if scene.visible(s.layer())
            && !p.layer_is_context(s.layer())
            && let Some(e) = shape_extent(scene, s, reference)
        {
            add(e);
        }
    }
    for g in &p.generators {
        if scene.visible(g.layer())
            && !p.layer_is_context(g.layer())
            && let Ok(e) = generator_extent(scene, g, reference, None)
        {
            add(e);
        }
    }
    for m in &p.models {
        if scene.visible(&m.layer)
            && !p.layer_is_context(&m.layer)
            && let Some(e) = model_extent(scene, &m.id, reference)
        {
            add(e);
        }
    }
    let mut e = out.ok_or("`plot`: the project has nothing drawn yet")?;
    e.kind = "plot";
    e.name = "plot".into();
    Ok(e)
}

/// Everything a reference can name: `(id, name, kind)`.
fn items(p: &Project) -> Vec<(String, String, &'static str)> {
    let mut v: Vec<(String, String, &'static str)> = Vec::new();
    v.extend(
        p.shapes
            .iter()
            .map(|s| (s.id().into(), s.name().into(), "shape")),
    );
    v.extend(
        p.generators
            .iter()
            .map(|g| (g.id().into(), g.name().into(), "planting")),
    );
    v.extend(
        p.models
            .iter()
            .map(|m| (m.id.clone(), m.name.clone(), "model")),
    );
    v.extend(
        p.lights
            .iter()
            .map(|l| (l.id.clone(), l.name.clone(), "light")),
    );
    v.extend(
        p.viewpoints
            .iter()
            .map(|x| (x.id.clone(), x.name.clone(), "viewpoint")),
    );
    v
}

fn by_id(scene: &Scene, id: &str, reference: &str) -> Result<Option<Extent>, String> {
    let p = scene.project;
    let eq = |a: &str| a.eq_ignore_ascii_case(id);
    if let Some(s) = p.shapes.iter().find(|s| eq(s.id())) {
        return Ok(shape_extent(scene, s, reference));
    }
    if let Some(g) = p.generators.iter().find(|g| eq(g.id())) {
        return generator_extent(scene, g, reference, None).map(Some);
    }
    if let Some(m) = p.models.iter().find(|m| eq(&m.id)) {
        return Ok(model_extent(scene, &m.id, reference));
    }
    if let Some(l) = p.lights.iter().find(|l| eq(&l.id)) {
        let r = 0.3;
        let q = l.position;
        return Ok(Some(named(
            "light",
            reference,
            &l.name,
            &l.id,
            [
                [q[0] - r, q[1] - r, q[2] - r],
                [q[0] + r, q[1] + r, q[2] + r],
            ],
        )));
    }
    if let Some(v) = p.viewpoints.iter().find(|v| eq(&v.id)) {
        return Ok(Some(named(
            "viewpoint",
            reference,
            &v.name,
            &v.id,
            [v.position; 2],
        )));
    }
    Ok(None)
}

fn by_item(
    scene: &Scene,
    (id, _, kind): &(String, String, &'static str),
    reference: &str,
) -> Result<Extent, String> {
    let found = by_id(scene, id, reference)?;
    found.ok_or_else(|| format!("`{reference}`: {kind} {id} has no extent"))
}

/// The box a reference stands for.
pub fn resolve(scene: &Scene, reference: &str) -> Result<Extent, String> {
    let r = reference.trim();
    if r.is_empty() {
        return Err(
            "a reference is empty: give [x, y, z], an id such as S6 or g4, or a name".into(),
        );
    }
    let lower = r.to_lowercase();
    if ["plot", "all", "everything", "scene"].contains(&lower.as_str()) {
        return plot_extent(scene, reference);
    }
    // One plant of a planting: `g4#12`.
    if let Some((id, n)) = r.split_once('#') {
        let g = scene
            .project
            .generators
            .iter()
            .find(|g| g.id().eq_ignore_ascii_case(id.trim()));
        if let (Some(g), Ok(n)) = (g, n.trim().parse::<usize>()) {
            return generator_extent(scene, g, reference, Some(n));
        }
        return Err(format!(
            "`{reference}`: `<planting>#<n>` names plant n (from 0) of a planting such as g4#12; the plantings are {}",
            scene
                .project
                .generators
                .iter()
                .map(|g| g.id())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(e) = by_id(scene, r, reference)? {
        return Ok(e);
    }
    let all = items(scene.project);
    let exact: Vec<&(String, String, &str)> = all
        .iter()
        .filter(|i| i.1.trim().to_lowercase() == lower)
        .collect();
    let hits: Vec<&(String, String, &str)> = if exact.is_empty() {
        all.iter()
            .filter(|i| i.1.to_lowercase().contains(&lower))
            .collect()
    } else {
        exact
    };
    // Shapes and plantings come first (what a film looks at), then models, lamps, viewpoints.
    if let Some(first) = hits.first() {
        let mut e = by_item(scene, first, reference)?;
        let others: Vec<String> = hits
            .iter()
            .skip(1)
            .map(|i| format!("{} {} ({})", i.2, i.0, i.1))
            .collect();
        if !others.is_empty() {
            e.note = Some(format!(
                "`{reference}` is {} {} ({}); it also names {}: use the id to pick",
                first.2,
                first.0,
                first.1,
                others.join(", ")
            ));
        }
        return Ok(e);
    }
    Err(unknown(reference, &all))
}

fn unknown(reference: &str, all: &[(String, String, &'static str)]) -> String {
    let labels: Vec<String> = all
        .iter()
        .flat_map(|i| [i.0.clone(), i.1.clone()])
        .filter(|s| !s.is_empty())
        .collect();
    let close = crate::spec::close_matches(reference, labels.iter().map(String::as_str), 5);
    let shown: Vec<String> = close
        .iter()
        .map(|c| {
            let i = all
                .iter()
                .find(|i| i.0 == *c || i.1 == *c)
                .expect("a label of an item");
            format!("{} ({} {})", i.0, i.2, i.1)
        })
        .collect();
    let mut seen = Vec::new();
    for s in shown {
        if !seen.contains(&s) {
            seen.push(s);
        }
    }
    if seen.is_empty() {
        let ids: Vec<String> = all
            .iter()
            .take(12)
            .map(|i| format!("{} ({})", i.0, i.1))
            .collect();
        format!(
            "unknown reference `{reference}`; use [x, y, z], `plot`, an id or a name. The project has: {}{}",
            if ids.is_empty() {
                "nothing yet".into()
            } else {
                ids.join(", ")
            },
            if all.len() > 12 { ", ..." } else { "" }
        )
    } else {
        format!(
            "unknown reference `{reference}`; close matches: {}",
            seen.join(", ")
        )
    }
}

// ----------------------------------------------------------------------------- values

/// `[x, y, z]`, a reference or the JSON of an [`Extent`] as the box it stands for.
pub fn place(scene: &Scene, v: &Value, field: &str) -> Result<Extent, String> {
    match v {
        Value::String(s) => resolve(scene, s).map_err(|e| format!("`{field}`: {e}")),
        Value::Array(a) if a.len() == 3 && a.iter().all(Value::is_number) => {
            let p: [f32; 3] = std::array::from_fn(|k| a[k].as_f64().unwrap_or(0.0) as f32);
            Ok(Extent::point(p))
        }
        Value::Object(_) => Extent::from_json(v)
            .ok_or_else(|| format!("`{field}`: an object needs ref, name, min and max")),
        _ => Err(format!(
            "`{field}` must be [x, y, z] or a reference (an id such as S6 or g4, a name, `plot`)"
        )),
    }
}

/// A point on the ground plane: `[x, z]`, `[x, y, z]` or a reference (its centre).
pub fn place_xz(scene: &Scene, v: &Value, field: &str) -> Result<[f32; 2], String> {
    match v {
        Value::Array(a) if a.len() == 2 && a.iter().all(Value::is_number) => Ok([
            a[0].as_f64().unwrap_or(0.0) as f32,
            a[1].as_f64().unwrap_or(0.0) as f32,
        ]),
        other => {
            let e = place(scene, other, field)?;
            let c = e.center();
            Ok([c[0], c[2]])
        }
    }
}

// ----------------------------------------------------------------------------- compass

/// A compass bearing in degrees, clockwise from north, from a word (`south`, `north-east`,
/// `ssw`) or a number of degrees.
pub fn bearing_deg(v: &Value) -> Result<f32, String> {
    if let Some(n) = v.as_f64() {
        return Ok(n as f32);
    }
    let word = v.as_str().ok_or(
        "a direction is a compass word (south, north-east, ...) or degrees (0 north, 90 east)",
    )?;
    let key: String = word
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphabetic())
        .collect();
    let eighth = |i: f32| Ok(i * 22.5);
    match key.as_str() {
        "n" | "north" => eighth(0.0),
        "nne" | "northnortheast" => eighth(1.0),
        "ne" | "northeast" => eighth(2.0),
        "ene" | "eastnortheast" => eighth(3.0),
        "e" | "east" => eighth(4.0),
        "ese" | "eastsoutheast" => eighth(5.0),
        "se" | "southeast" => eighth(6.0),
        "sse" | "southsoutheast" => eighth(7.0),
        "s" | "south" => eighth(8.0),
        "ssw" | "southsouthwest" => eighth(9.0),
        "sw" | "southwest" => eighth(10.0),
        "wsw" | "westsouthwest" => eighth(11.0),
        "w" | "west" => eighth(12.0),
        "wnw" | "westnorthwest" => eighth(13.0),
        "nw" | "northwest" => eighth(14.0),
        "nnw" | "northnorthwest" => eighth(15.0),
        _ => Err(format!(
            "`{word}` is not a direction: use north, north-east, east, south-east, south, south-west, west, north-west (or n, ne, ..., nnw) or degrees (0 north, 90 east)"
        )),
    }
}

/// The unit vector `[x, z]` towards a bearing: north is -Z, east is +X.
pub fn bearing_dir(bearing_deg: f32) -> [f32; 2] {
    let b = bearing_deg.to_radians();
    [b.sin(), -b.cos()]
}

/// An orbit's angle (from +X, counter-clockwise seen from above) of a compass bearing.
pub fn orbit_angle(bearing_deg: f32) -> f32 {
    90.0 - bearing_deg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn garden() -> Project {
        crate::example("garden").unwrap()
    }

    #[test]
    fn shapes_resolve_by_id_and_by_name_without_case() {
        let p = garden();
        let scene = Scene::new(&p, None);
        let pond = resolve(&scene, "S6").unwrap();
        assert_eq!(pond.kind, "shape");
        assert_eq!(pond.min[0], 8.0);
        assert_eq!(pond.max[2], 25.0);
        let c = pond.center();
        assert!((c[0] - 10.5).abs() < 1e-4 && (c[2] - 22.5).abs() < 1e-4);
        assert!((pond.radius() - 5.0f32.hypot(5.0) / 2.0).abs() < 1e-4);
        let named = resolve(&scene, "pOnD").unwrap();
        assert_eq!(named.min, pond.min);
        // The pond is also a viewpoint's name: noted, and the id picks.
        assert!(named.note.as_deref().unwrap().contains("v4"), "{named:?}");
        let house = resolve(&scene, "house").unwrap();
        assert!(house.height() > 5.0);
    }

    #[test]
    fn plants_have_the_crown_the_growth_model_gives_at_the_years() {
        let p = garden();
        let young = resolve(&Scene::new(&p, Some(0.0)), "g4").unwrap();
        let old = resolve(&Scene::new(&p, Some(30.0)), "g4").unwrap();
        let full = resolve(&Scene::new(&p, None), "Birch").unwrap();
        assert_eq!(young.kind, "planting");
        assert!(young.height() < old.height(), "{young:?} {old:?}");
        assert!(old.height() <= full.height() + 1e-3);
        assert!(full.height() > 8.0 && full.radius() > 1.0, "{full:?}");
        // One plant: g4 is a single, so g4#0 is the tree and g4#1 does not exist.
        let one = resolve(&Scene::new(&p, None), "g4#0").unwrap();
        assert_eq!(one.kind, "plant");
        assert!((one.height() - full.height()).abs() < 1e-3);
        let err = resolve(&Scene::new(&p, None), "g4#1").unwrap_err();
        assert!(err.contains("no plant 1"), "{err}");
        // A hedge of many plants spans its length.
        let hedge = resolve(&Scene::new(&p, None), "g1").unwrap();
        assert!(hedge.radius() > 8.0, "{hedge:?}");
    }

    #[test]
    fn plot_is_everything_and_unknown_names_list_close_ones() {
        let p = garden();
        let scene = Scene::new(&p, None);
        let plot = resolve(&scene, "plot").unwrap();
        assert!(plot.min[0] <= 0.8 && plot.max[0] >= 19.0 && plot.max[2] >= 29.0);
        let err = resolve(&scene, "Pnd").unwrap_err();
        assert!(err.contains("close matches") && err.contains("S6"), "{err}");
        let err = resolve(&scene, "xyzzy").unwrap_err();
        assert!(
            err.contains("unknown reference") && err.contains("S6 (Pond)"),
            "{err}"
        );
        // A name that is part of another is that item: "lights" is the path lights planting.
        assert_eq!(resolve(&scene, "lights").unwrap().kind, "planting");
        let err = resolve(&scene, "").unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn places_take_points_references_and_extents() {
        let p = garden();
        let scene = Scene::new(&p, None);
        let pt = place(&scene, &json!([1, 2, 3]), "x").unwrap();
        assert_eq!((pt.center(), pt.radius()), ([1.0, 2.0, 3.0], 0.0));
        let e = place(&scene, &json!("S6"), "x").unwrap();
        assert_eq!(Extent::from_json(&e.to_json()).unwrap().min, e.min);
        assert_eq!(place_xz(&scene, &json!([4, 5]), "x").unwrap(), [4.0, 5.0]);
        assert!(
            place(&scene, &json!(5), "target")
                .unwrap_err()
                .contains("`target`")
        );
        assert!(
            place(&scene, &json!("zzz"), "target")
                .unwrap_err()
                .contains("`target`")
        );
    }

    #[test]
    fn compass_words_use_minus_z_as_north_and_plus_x_as_east() {
        let near =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
        assert!(near(
            bearing_dir(bearing_deg(&json!("north")).unwrap()),
            [0.0, -1.0]
        ));
        assert!(near(
            bearing_dir(bearing_deg(&json!("East")).unwrap()),
            [1.0, 0.0]
        ));
        assert!(near(
            bearing_dir(bearing_deg(&json!("south")).unwrap()),
            [0.0, 1.0]
        ));
        assert!(near(
            bearing_dir(bearing_deg(&json!("west")).unwrap()),
            [-1.0, 0.0]
        ));
        assert_eq!(bearing_deg(&json!("north-east")).unwrap(), 45.0);
        assert_eq!(bearing_deg(&json!("south_west")).unwrap(), 225.0);
        assert_eq!(bearing_deg(&json!(135)).unwrap(), 135.0);
        assert!(
            bearing_deg(&json!("up"))
                .unwrap_err()
                .contains("not a direction")
        );
        // An orbit angle: east 0, north 90 (counter-clockwise seen from above), south -90.
        assert_eq!(orbit_angle(90.0), 0.0);
        assert_eq!(orbit_angle(0.0), 90.0);
        assert_eq!(orbit_angle(180.0), -90.0);
    }
}
