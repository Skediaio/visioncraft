//! The planting tools of Objects mode: Single, Line, Cluster, Area, Surface fill and Brush, plus
//! selecting and editing existing plantings. This file is the logic (settings, preview, input);
//! `planting_panel` draws the panel. Everything that decides what a command contains is a pure
//! function (`Form::params`, `commit`, `commit_stroke`) that the headless smoke mode calls too.

use crate::draw::{closes, dot, snap_point};
use crate::handles::{self, Handle};
use crate::objects::display_name;
use crate::plant_scene::now_ms;
use crate::scene::{Scene, line};
use crate::strings as t;
use crate::studio::Draft;
use crate::ui::{Cx, Edit};
use eframe::egui::{Key, Pos2};
use glam::{Vec2, Vec3};
use serde_json::{Value, json};
use std::collections::HashMap;
use visioncraft_core::planting::{Spacing, Stroke, Weighted};
use visioncraft_core::{CommandError, Generator, PlantLayer, Session, planting_presets};
use visioncraft_render::LineVertex;

/// At most this many preview dots are drawn; more are thinned out evenly.
pub const MAX_DOTS: usize = 15_000;
pub const BRUSH_MIN: f32 = 0.25;
pub const BRUSH_MAX: f32 = 40.0;
const HANDLE_PX: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Tool {
    #[default]
    Select,
    Single,
    Line,
    Cluster,
    Area,
    Surface,
    Brush,
}

impl Tool {
    pub const ALL: [Tool; 7] = [
        Tool::Select,
        Tool::Single,
        Tool::Line,
        Tool::Cluster,
        Tool::Area,
        Tool::Surface,
        Tool::Brush,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => t::PT_SELECT,
            Tool::Single => t::PT_SINGLE,
            Tool::Line => t::PT_LINE,
            Tool::Cluster => t::PT_CLUSTER,
            Tool::Area => t::PT_AREA,
            Tool::Surface => t::PT_SURFACE,
            Tool::Brush => t::PT_BRUSH,
        }
    }

    /// Whether the tool's plantings can have layers (a preset, or the three storeys by hand).
    pub fn layered(self) -> bool {
        matches!(self, Tool::Area | Tool::Surface | Tool::Brush)
    }
}

/// The kind of tool that made (or edits) a generator.
pub fn tool_of(g: &Generator) -> Tool {
    match g {
        Generator::Single(_) => Tool::Single,
        Generator::SplineMass(_) => Tool::Line,
        Generator::Cluster(_) => Tool::Cluster,
        Generator::AreaScatter(_) => Tool::Area,
        Generator::SurfaceFill(_) => Tool::Surface,
        Generator::MixedBrush(_) => Tool::Brush,
    }
}

/// Everything the panel edits, for a new planting and for an existing one alike.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    /// The palette of a simple (single implicit layer) planting.
    pub palette: Vec<Weighted>,
    /// Layers, when a preset or the layered mode is on; they replace the palette, density,
    /// minimum distance and scale below.
    pub layers: Vec<PlantLayer>,
    /// Name of the preset the layers came from.
    pub preset: Option<String>,
    pub density: f32,
    pub min_distance: f32,
    pub scale_min: f32,
    /// A single plant's scale (its own, not a range).
    pub single_scale: f32,
    pub scale_max: f32,
    pub spin: bool,
    pub seed: u64,
    /// The project layer (planting by default).
    pub layer: String,
    /// Line: how far an item may slide along the line (0..1 of the spacing).
    pub jitter: f32,
    /// Area and surface: how far a point may stray from its grid cell (0..1).
    pub pos_jitter: f32,
    pub by_count: bool,
    /// Line: items on it; cluster: items in it.
    pub count: u32,
    pub every: f32,
    pub offset: f32,
    pub closed: bool,
    pub max_slope: f32,
    /// A single plant's rotation, degrees.
    pub rotation: f32,
    /// A cluster's radius, metres.
    pub radius: f32,
    /// Line: straight from point to point (a hedge along a plot) instead of a smooth curve.
    pub straight: bool,
    /// Brush: plants stay inside the area the stroke starts on.
    pub keep_inside: bool,
    /// Brush: the outline they stay inside (the area under the stroke's start), empty for none.
    pub clip: Vec<[f32; 2]>,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            palette: vec![Weighted {
                asset: "flora:prunus-serrulata".into(),
                weight: 1.0,
            }],
            layers: Vec::new(),
            preset: None,
            density: 0.5,
            min_distance: 0.0,
            scale_min: 0.85,
            single_scale: 1.0,
            scale_max: 1.15,
            spin: true,
            seed: 1,
            layer: visioncraft_core::project::LAYER_PLANTING.into(),
            jitter: 0.15,
            pos_jitter: 1.0,
            by_count: false,
            count: 12,
            every: 1.0,
            offset: 0.0,
            closed: false,
            max_slope: 45.0,
            rotation: 0.0,
            radius: 2.0,
            straight: false,
            keep_inside: true,
            clip: Vec::new(),
        }
    }
}

impl Form {
    /// What `tool` starts with: a closed hornbeam hedge for Line (1 m segments every metre, no
    /// jitter, no size spread, no turning), a perennial mix for Brush, a tree for the rest.
    pub fn for_tool(tool: Tool) -> Form {
        let w = |asset: &str| Weighted {
            asset: asset.into(),
            weight: 1.0,
        };
        match tool {
            Tool::Line => Form {
                palette: vec![w("flora:carpinus-hedge")],
                straight: true,
                jitter: 0.0,
                scale_min: 1.0,
                scale_max: 1.0,
                spin: false,
                every: 1.0,
                ..Default::default()
            },
            Tool::Brush => Form {
                palette: [
                    "flora:salvia-nemorosa",
                    "flora:nepeta-faassenii",
                    "flora:geranium-rozanne",
                    "flora:echinacea-purpurea",
                ]
                .into_iter()
                .map(w)
                .collect(),
                ..Default::default()
            },
            _ => Form::default(),
        }
    }
}

/// What a tool's gesture produced, in world metres.
#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    Single {
        position: [f32; 3],
        rotation_deg: f32,
    },
    Line(Vec<[f32; 2]>),
    Cluster {
        center: [f32; 2],
        radius: f32,
    },
    Area(Vec<[f32; 2]>),
    Surface {
        owner: String,
        faces: Vec<String>,
    },
    Brush(Vec<Stroke>),
}

pub fn next_seed(s: u64) -> u64 {
    s.wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}

/// A repeatable number in 0..1 from a counter.
fn hash01(n: u64) -> f32 {
    (next_seed(next_seed(n)) >> 40) as f32 / 16_777_216.0
}

/// The asset at fraction `r` (0..1) of the total weight.
pub fn pick_weighted(palette: &[Weighted], r: f32) -> Option<&str> {
    let total: f32 = palette.iter().map(|w| w.weight.max(0.0)).sum();
    if total <= 0.0 {
        return None;
    }
    let mut at = r.clamp(0.0, 0.999_999) * total;
    for w in palette {
        let w_ = w.weight.max(0.0);
        if at < w_ {
            return Some(&w.asset);
        }
        at -= w_;
    }
    palette.last().map(|w| w.asset.as_str())
}

/// The preset whose layers these are, if they are still untouched.
fn preset_of(layers: &[PlantLayer]) -> Option<String> {
    planting_presets()
        .into_iter()
        .find(|p| p.layers == layers)
        .map(|p| p.name)
}

impl Form {
    pub fn from_generator(g: &Generator) -> Form {
        let mut f = Form {
            layer: g.layer().into(),
            ..Default::default()
        };
        let weighted = |p: &[Weighted]| {
            if p.is_empty() { None } else { Some(p.to_vec()) }
        };
        match g {
            Generator::Single(s) => {
                f.palette = vec![Weighted {
                    asset: s.asset.clone(),
                    weight: 1.0,
                }];
                f.rotation = s.rotation_y_deg;
                f.single_scale = s.scale;
            }
            Generator::SplineMass(s) => {
                f.palette = s.asset_palette.clone();
                (f.by_count, f.count, f.every) = match s.spacing {
                    Spacing::Count(n) => (true, n, f.every),
                    Spacing::Every(d) => (false, f.count, d),
                };
                (f.jitter, f.offset, f.closed) = (s.spacing_jitter, s.offset_m, s.closed);
                f.straight = s.straight;
                (f.scale_min, f.scale_max, f.spin, f.seed) =
                    (s.scale_min, s.scale_max, s.random_rotation, s.seed);
            }
            Generator::Cluster(c) => {
                f.palette = c.asset_palette.clone();
                (f.count, f.radius) = (c.count, c.radius_m);
                (f.scale_min, f.scale_max, f.spin, f.seed) =
                    (c.scale_min, c.scale_max, c.random_rotation, c.seed);
            }
            Generator::AreaScatter(a) => {
                f.palette = weighted(&a.asset_palette).unwrap_or(f.palette);
                (f.layers, f.density, f.min_distance) =
                    (a.layers.clone(), a.density_per_m2, a.min_distance_m);
                f.preset = preset_of(&f.layers);
                (f.pos_jitter, f.spin, f.seed) = (a.position_jitter, a.random_rotation, a.seed);
                (f.scale_min, f.scale_max) = (a.scale_min, a.scale_max);
            }
            Generator::SurfaceFill(s) => {
                f.palette = weighted(&s.asset_palette).unwrap_or(f.palette);
                (f.layers, f.density, f.min_distance) =
                    (s.layers.clone(), s.density_per_m2, s.min_distance_m);
                f.preset = preset_of(&f.layers);
                (f.pos_jitter, f.spin, f.seed) = (s.position_jitter, s.random_rotation, s.seed);
                (f.scale_min, f.scale_max, f.max_slope) =
                    (s.scale_min, s.scale_max, s.max_slope_deg);
            }
            Generator::MixedBrush(b) => {
                f.palette = weighted(&b.asset_palette).unwrap_or(f.palette);
                (f.layers, f.density, f.min_distance) =
                    (b.layers.clone(), b.density_per_m2, b.min_distance_m);
                f.preset = preset_of(&f.layers);
                (f.scale_min, f.scale_max, f.seed) = (b.scale_min, b.scale_max, b.seed);
                f.clip.clone_from(&b.clip_outline);
            }
        }
        f
    }

    /// `g` with the form's fields written into it; the geometry stays.
    pub fn apply(&self, g: &Generator) -> Generator {
        let mut g = g.clone();
        let pal = |f: &Form| {
            if f.layers.is_empty() {
                f.palette.clone()
            } else {
                Vec::new()
            }
        };
        match &mut g {
            Generator::Single(s) => {
                if let Some(w) = self.palette.first() {
                    s.asset.clone_from(&w.asset);
                }
                s.rotation_y_deg = self.rotation;
                s.scale = self.single_scale;
                s.layer.clone_from(&self.layer);
            }
            Generator::SplineMass(s) => {
                s.asset_palette.clone_from(&self.palette);
                s.spacing = if self.by_count {
                    Spacing::Count(self.count.max(1))
                } else {
                    Spacing::Every(self.every)
                };
                (s.spacing_jitter, s.offset_m, s.closed) = (self.jitter, self.offset, self.closed);
                s.straight = self.straight;
                (s.scale_min, s.scale_max, s.random_rotation, s.seed) =
                    (self.scale_min, self.scale_max, self.spin, self.seed);
                s.layer.clone_from(&self.layer);
            }
            Generator::Cluster(c) => {
                c.asset_palette.clone_from(&self.palette);
                (c.count, c.radius_m) = (self.count, self.radius);
                (c.scale_min, c.scale_max, c.random_rotation, c.seed) =
                    (self.scale_min, self.scale_max, self.spin, self.seed);
                c.layer.clone_from(&self.layer);
            }
            Generator::AreaScatter(a) => {
                a.asset_palette = pal(self);
                (a.layers, a.density_per_m2, a.min_distance_m) =
                    (self.layers.clone(), self.density, self.min_distance);
                (a.position_jitter, a.random_rotation, a.seed) =
                    (self.pos_jitter, self.spin, self.seed);
                (a.scale_min, a.scale_max) = (self.scale_min, self.scale_max);
                a.layer.clone_from(&self.layer);
            }
            Generator::SurfaceFill(s) => {
                s.asset_palette = pal(self);
                (s.layers, s.density_per_m2, s.min_distance_m) =
                    (self.layers.clone(), self.density, self.min_distance);
                (s.position_jitter, s.random_rotation, s.seed) =
                    (self.pos_jitter, self.spin, self.seed);
                (s.scale_min, s.scale_max, s.max_slope_deg) =
                    (self.scale_min, self.scale_max, self.max_slope);
                s.layer.clone_from(&self.layer);
            }
            Generator::MixedBrush(b) => {
                b.asset_palette = pal(self);
                (b.layers, b.density_per_m2, b.min_distance_m) =
                    (self.layers.clone(), self.density, self.min_distance);
                (b.scale_min, b.scale_max, b.seed) = (self.scale_min, self.scale_max, self.seed);
                b.layer.clone_from(&self.layer);
            }
        }
        g
    }

    /// The command that adds a planting of `geom` with these settings, and its parameters.
    pub fn params(&self, geom: &Geom, name: &str) -> (&'static str, Value) {
        let layered = !self.layers.is_empty();
        let palette = if layered {
            json!([])
        } else {
            serde_json::to_value(&self.palette).expect("weights serialise")
        };
        // Lines and clusters have no layers: they always use the palette.
        let plain = serde_json::to_value(&self.palette).expect("weights serialise");
        let layers = serde_json::to_value(&self.layers).expect("layers serialise");
        let common = |mut v: Value| {
            v["name"] = json!(name);
            if !matches!(geom, Geom::Single { .. }) {
                v["seed"] = json!(self.seed); // a single plant has no randomness
            }
            v["layer"] = json!(self.layer);
            v
        };
        match geom {
            Geom::Single {
                position,
                rotation_deg,
            } => (
                "planting.add_single",
                common(json!({
                    "asset": self.palette.first().map(|w| w.asset.clone()).unwrap_or_default(),
                    "position": position,
                    "rotation_y_deg": rotation_deg,
                    "scale": self.single_scale,
                })),
            ),
            Geom::Line(points) => (
                "planting.add_spline",
                common(json!({
                    "asset_palette": plain,
                    "points": points,
                    "spacing": if self.by_count { json!({"count": self.count.max(1)}) } else { json!({"every": self.every}) },
                    "closed": self.closed,
                    "straight": self.straight,
                    "spacing_jitter": self.jitter,
                    "offset_m": self.offset,
                    "scale_min": self.scale_min,
                    "scale_max": self.scale_max,
                    "random_rotation": self.spin,
                })),
            ),
            Geom::Cluster { center, radius } => (
                "planting.add_cluster",
                common(json!({
                    "asset_palette": plain,
                    "center": center,
                    "radius_m": radius,
                    "count": self.count,
                    "scale_min": self.scale_min,
                    "scale_max": self.scale_max,
                    "random_rotation": self.spin,
                })),
            ),
            Geom::Area(outline) => (
                "planting.add_area",
                common(json!({
                    "asset_palette": palette,
                    "outline": outline,
                    "density_per_m2": self.density,
                    "layers": layers,
                    "position_jitter": self.pos_jitter,
                    "scale_min": self.scale_min,
                    "scale_max": self.scale_max,
                    "random_rotation": self.spin,
                    "min_distance_m": self.min_distance,
                })),
            ),
            Geom::Surface { owner, faces } => (
                "planting.add_surface_fill",
                common(json!({
                    "asset_palette": palette,
                    "owner": owner,
                    "faces": faces,
                    "density_per_m2": self.density,
                    "layers": layers,
                    "position_jitter": self.pos_jitter,
                    "scale_min": self.scale_min,
                    "scale_max": self.scale_max,
                    "random_rotation": self.spin,
                    "min_distance_m": self.min_distance,
                    "max_slope_deg": self.max_slope,
                })),
            ),
            Geom::Brush(strokes) => (
                "planting.add_brush",
                common(json!({
                    "asset_palette": palette,
                    "density_per_m2": self.density,
                    "min_distance_m": self.min_distance,
                    "layers": layers,
                    "strokes": strokes,
                    "scale_min": self.scale_min,
                    "scale_max": self.scale_max,
                    "clip_outline": self.clip,
                })),
            ),
        }
    }

    /// What decides whether a brush stroke extends the same generator: everything but the seed
    /// and the fields a brush does not have (a cluster's radius, a single plant's rotation).
    pub fn signature(&self) -> String {
        let mut f = self.clone();
        (f.seed, f.radius, f.rotation, f.single_scale) = (0, 0.0, 0.0, 0.0);
        format!("{f:?}")
    }

    /// A readable name for a new planting.
    pub fn name(&self, label: &str, species: &str) -> String {
        match (&self.preset, self.layers.is_empty()) {
            (Some(p), false) => format!("{p} {}", label.to_lowercase()),
            (None, false) => format!("{label} (layered)"),
            _ => format!("{label} · {species}"),
        }
    }
}

/// The generator `geom` would make, without touching the project (for the preview).
pub fn preview_generator(form: &Form, geom: &Geom) -> Option<Generator> {
    let (cmd, mut p) = form.params(geom, "preview");
    let ty = match cmd {
        "planting.add_single" => "single",
        "planting.add_spline" => "spline_mass",
        "planting.add_cluster" => "cluster",
        "planting.add_area" => "area_scatter",
        "planting.add_surface_fill" => "surface_fill",
        _ => "mixed_brush",
    };
    let o = p.as_object_mut()?;
    o.insert("id".into(), json!("preview"));
    o.insert("type".into(), json!(ty));
    let g: Generator = serde_json::from_value(p).ok()?;
    g.validate().ok()?;
    Some(g)
}

/// Adds the planting; returns its id.
pub fn commit(
    session: &mut Session,
    form: &Form,
    geom: &Geom,
    name: &str,
) -> Result<String, String> {
    let (cmd, p) = form.params(geom, name);
    let r = session.run(cmd, p).map_err(|e| e.to_string())?;
    Ok(r["id"].as_str().unwrap_or_default().to_string())
}

/// The brush a drag extends: its generator id and the settings signature it was made with.
pub type BrushTarget = Option<(String, String)>;

/// One brush stroke: it extends the target generator when the settings are unchanged, else a
/// painting stroke starts a new brush (an erasing one has nothing to erase). Returns the id.
pub fn commit_stroke(
    session: &mut Session,
    target: &mut BrushTarget,
    form: &Form,
    stroke: Stroke,
    name: &str,
) -> Result<String, String> {
    let sig = form.signature();
    if let Some((id, s)) = target.as_ref()
        && *s == sig
        && session.project().generators.iter().any(|g| g.id() == id)
    {
        let id = id.clone();
        session
            .run(
                "planting.brush_stroke",
                json!({"id": id, "points": stroke.points, "radius_m": stroke.radius_m, "erase": stroke.erase}),
            )
            .map_err(|e| e.to_string())?;
        return Ok(id);
    }
    if stroke.erase {
        return Err(t::PT_NOTHING_TO_ERASE.into());
    }
    let id = commit(session, form, &Geom::Brush(vec![stroke]), name)?;
    *target = Some((id.clone(), sig));
    Ok(id)
}

/// Limits the rate of preview evaluations: at most one per 100 ms, and spaced so that evaluation
/// takes a third of the time at most when it is slow.
#[derive(Default)]
pub struct Throttle {
    last: f64,
    cost: f64,
}

impl Throttle {
    pub fn interval(&self) -> f64 {
        (self.cost * 2.0).max(0.1)
    }

    pub fn ready(&self, now: f64) -> bool {
        now - self.last >= self.interval()
    }

    /// Records an evaluation at `now` that took `cost` seconds.
    pub fn done(&mut self, now: f64, cost: f64) {
        self.last = now;
        self.cost = cost;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Dot {
    pub pos: [f32; 3],
    pub layer: u8,
}

#[derive(Default)]
pub struct Preview {
    key: String,
    pub dots: Vec<Dot>,
    pub count: usize,
    pub truncated: bool,
    pub layered: bool,
}

fn layers_of(g: &Generator) -> &[PlantLayer] {
    match g {
        Generator::AreaScatter(a) => &a.layers,
        Generator::MixedBrush(b) => &b.layers,
        Generator::SurfaceFill(f) => &f.layers,
        _ => &[],
    }
}

/// Which layer (storey) each asset of a generator belongs to: the first that uses it.
pub fn layer_indices(g: &Generator) -> HashMap<String, u8> {
    let mut m = HashMap::new();
    for (i, l) in layers_of(g).iter().enumerate() {
        for w in &l.asset_palette {
            m.entry(w.asset.clone()).or_insert(i as u8);
        }
    }
    m
}

pub const LAYER_COLORS: [[u8; 4]; 4] = [
    [110, 225, 120, 255],
    [245, 205, 90, 255],
    [120, 175, 255, 255],
    [235, 130, 205, 255],
];

/// Evaluates `g` against the scene's ground (a few thousand points are cheap, 20,000 take about
/// 30 ms in the core) and keeps the dots, thinned to `MAX_DOTS`.
pub fn compute_preview(scene: &Scene, g: &Generator) -> Preview {
    let e = scene.evaluate_generator(g);
    let idx = layer_indices(g);
    let stride = e.placements.len().div_ceil(MAX_DOTS).max(1);
    Preview {
        key: String::new(),
        dots: e
            .placements
            .iter()
            .step_by(stride)
            .map(|p| Dot {
                pos: [p.position[0], p.position[1] + 0.12, p.position[2]],
                layer: idx.get(&p.asset).copied().unwrap_or(0),
            })
            .collect(),
        count: e.placements.len(),
        truncated: e.truncated,
        layered: !layers_of(g).is_empty(),
    }
}

fn dot_lines(p: &Preview, out: &mut Vec<LineVertex>) {
    for d in &p.dots {
        let l = if p.layered { d.layer as usize % 4 } else { 0 };
        let w = [7.0, 5.5, 4.5, 4.0][l];
        out.extend(line(
            Vec3::from(d.pos),
            Vec3::from(d.pos),
            LAYER_COLORS[l],
            w,
        ));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddTarget {
    Palette,
    Layer(usize),
}

enum Drag {
    None,
    /// Single: where it stands; cluster: its centre.
    Anchor(Vec3),
    Handle(Handle),
    Stroke,
}

pub struct PlantingTools {
    pub tool: Tool,
    /// The Lights tool of the tool row is open instead of a planting tool.
    pub lights: bool,
    /// Settings for new plantings with the current tool.
    pub form: Form,
    /// The other tools' settings: each tool keeps its own palette.
    forms: HashMap<Tool, Form>,
    pub selected: Option<String>,
    /// The selected planting's settings, edited live.
    pub edit: Option<Form>,
    edit_rev: u64,
    pub panel_height: f32,
    pub grow: u8,
    pub add_to: Option<AddTarget>,
    pub show_help: bool,
    pub vertex: Option<usize>,
    pub brush_radius: f32,
    pub erase: bool,
    pub brush_target: BrushTarget,
    pub points: Vec<[f32; 2]>,
    drag: Drag,
    stroke: Vec<[f32; 2]>,
    stroke_erase: bool,
    pub preview: Preview,
    throttle: Throttle,
    last_fill: Option<String>,
    counter: u64,
}

impl Default for PlantingTools {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            lights: false,
            form: Form::default(),
            forms: HashMap::new(),
            selected: None,
            edit: None,
            edit_rev: u64::MAX,
            panel_height: 0.0,
            grow: 0,
            add_to: None,
            show_help: false,
            vertex: None,
            brush_radius: 4.0,
            erase: false,
            brush_target: None,
            points: Vec::new(),
            drag: Drag::None,
            stroke: Vec::new(),
            stroke_erase: false,
            preview: Preview::default(),
            throttle: Throttle::default(),
            last_fill: None,
            counter: 0,
        }
    }
}

pub fn merge(a: Edit, b: Edit) -> Edit {
    match (a, b) {
        (Edit::Done, _) | (_, Edit::Done) => Edit::Done,
        (Edit::Live, _) | (_, Edit::Live) => Edit::Live,
        _ => Edit::None,
    }
}

// ---- pointer helpers ----

/// The ground point under a pointer position: the nearest surface of models, shapes and terrain,
/// else the plane y = 0.
pub fn ground_at(cx: &Cx, pos: Pos2) -> Option<Vec3> {
    let (o, d) = cx.s.pointer_ray(Vec2::new(pos.x, pos.y), cx.size());
    let mut best = cx.s.scene.raycast(o, d, false).map(|h| h.distance);
    if let Some(t) =
        cx.s.project()
            .terrain
            .as_ref()
            .and_then(|t| t.raycast(o.to_array(), d.to_array()))
    {
        best = Some(best.map_or(t.0, |b| b.min(t.0)));
    }
    match best {
        Some(t) => Some(o + d * t),
        None => crate::camera::ray_plane(o, d, 0.0),
    }
}

/// Distance along the pointer ray to the nearest ground, if any.
fn ground_distance(cx: &Cx, o: Vec3, d: Vec3) -> Option<f32> {
    let a = cx.s.scene.raycast(o, d, false).map(|h| h.distance);
    let b =
        cx.s.project()
            .terrain
            .as_ref()
            .and_then(|t| t.raycast(o.to_array(), d.to_array()))
            .map(|t| t.0);
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn ground_y(cx: &Cx, x: f32, z: f32) -> f32 {
    cx.s.scene.ground_fn()(x, z).unwrap_or(0.0)
}

/// Metres per screen pixel at a world point.
fn px_metres(cx: &Cx, at: Vec3) -> f32 {
    let h = cx.input.rect.height().max(1.0);
    let cam = &cx.s.camera;
    match cam.view {
        crate::camera::View::Top => 2.0 * cam.top_half_height / h,
        _ => cam.eye().distance(at) * 2.0 * (cam.fov_y / 2.0).tan() / h,
    }
}

/// Lines along `pts` that hug the ground (sampled about every metre).
fn ground_line(
    cx: &Cx,
    pts: &[[f32; 2]],
    closed: bool,
    color: [u8; 4],
    width: f32,
) -> Vec<LineVertex> {
    let g = cx.s.scene.ground_fn();
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..if closed { n } else { n.saturating_sub(1) } {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let k = ((a[0] - b[0]).hypot(a[1] - b[1]).ceil() as usize).clamp(1, 300);
        let mut prev: Option<Vec3> = None;
        for s in 0..=k {
            let f = s as f32 / k as f32;
            let (x, z) = (a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f);
            let v = Vec3::new(x, g(x, z).unwrap_or(0.0) + 0.1, z);
            if let Some(p) = prev {
                out.extend(line(p, v, color, width));
            }
            prev = Some(v);
        }
    }
    out
}

fn ring(cx: &Cx, c: [f32; 2], r: f32, color: [u8; 4], width: f32) -> Vec<LineVertex> {
    let steps = 64;
    let pts: Vec<[f32; 2]> = (0..steps)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / steps as f32;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect();
    let g = cx.s.scene.ground_fn();
    let v: Vec<Vec3> = pts
        .iter()
        .map(|p| Vec3::new(p[0], g(p[0], p[1]).unwrap_or(0.0) + 0.15, p[1]))
        .collect();
    (0..steps)
        .flat_map(|i| line(v[i], v[(i + 1) % steps], color, width))
        .collect()
}

fn stored(cx: &Cx, id: &str) -> Option<Generator> {
    cx.s.project()
        .generators
        .iter()
        .find(|g| g.id() == id)
        .cloned()
}

/// The generator as shown now: with the live draft, if it is one.
pub fn current(cx: &Cx, id: &str) -> Option<Generator> {
    if let Some(Draft::Generator(g)) = &cx.s.draft
        && g.id() == id
    {
        return Some(g.clone());
    }
    stored(cx, id)
}

fn key_pressed(cx: &Cx, k: Key) -> bool {
    !cx.ctx.egui_wants_keyboard_input() && cx.ctx.input(|i| i.key_pressed(k))
}

impl PlantingTools {
    pub fn leave(&mut self, cx: &mut Cx) {
        // The plan was this mode's: other modes get the 3D view back.
        if cx.s.camera.view == crate::camera::View::Top {
            cx.s.camera.view = crate::camera::View::Orbit;
        }
        self.points.clear();
        self.stroke.clear();
        self.drag = Drag::None;
        self.selected = None;
        self.edit = None;
        self.vertex = None;
        self.brush_target = None;
        self.preview = Preview::default();
        cx.s.set_draft(None);
        cx.s.pointer_captured = false;
        cx.s.double_click_captured = false;
    }

    pub fn set_tool(&mut self, cx: &mut Cx, tool: Tool) {
        self.lights = false;
        if self.tool == tool {
            return;
        }
        self.switch_form(tool);
        self.points.clear();
        self.stroke.clear();
        self.drag = Drag::None;
        self.brush_target = None;
        self.preview = Preview::default();
        self.last_fill = None;
        self.vertex = None;
        if tool != Tool::Select {
            self.selected = None;
            self.edit = None;
        }
        cx.s.set_draft(None);
    }

    pub fn select_generator(&mut self, cx: &mut Cx, id: Option<String>) {
        self.vertex = None;
        self.edit = id
            .as_deref()
            .and_then(|id| stored(cx, id))
            .map(|g| Form::from_generator(&g));
        self.selected = self.edit.is_some().then_some(id).flatten();
        self.edit_rev = cx.s.session.revision();
    }

    /// Makes `tool` current with its own settings, keeping the old tool's.
    fn switch_form(&mut self, tool: Tool) {
        if tool == self.tool {
            return;
        }
        let next = self
            .forms
            .remove(&tool)
            .unwrap_or_else(|| Form::for_tool(tool));
        let old = std::mem::replace(&mut self.form, next);
        self.forms.insert(self.tool, old);
        self.tool = tool;
    }

    /// Selects `g` (or nothing) without a `Cx`, as automation does.
    pub fn select_stored(&mut self, g: Option<&Generator>, revision: u64) {
        self.lights = false;
        self.switch_form(Tool::Select);
        self.vertex = None;
        self.edit = g.map(Form::from_generator);
        self.selected = g.map(|g| g.id().to_string());
        self.edit_rev = revision;
    }

    /// The form the panel edits: the selected planting's, else the new-planting settings.
    pub fn active_form(&mut self) -> &mut Form {
        match (&self.selected, &mut self.edit) {
            (Some(_), Some(f)) => f,
            _ => &mut self.form,
        }
    }

    pub fn editing(&self) -> bool {
        self.selected.is_some() && self.edit.is_some()
    }

    /// Writes the live edit of the selected planting: a draft while it changes, a command when it ends.
    pub fn finish_edit(&mut self, cx: &mut Cx, edit: Edit) {
        let (Some(id), Some(form)) = (self.selected.clone(), self.edit.clone()) else {
            return;
        };
        if edit == Edit::None {
            return;
        }
        let Some(old) = stored(cx, &id) else {
            return;
        };
        let new = form.apply(&old);
        if edit == Edit::Live {
            cx.s.set_draft(Some(Draft::Generator(new)));
        } else {
            cx.s.set_draft(None);
            self.commit_update(cx, &old, &new);
        }
    }

    fn commit_update(&mut self, cx: &mut Cx, old: &Generator, new: &Generator) {
        if let Some(p) = handles::diff_params(old, new) {
            cx.run("planting.update", p);
            self.edit_rev = cx.s.session.revision();
        }
    }

    /// Adds an asset to the palette (or a layer's) that "+" opened the browser for.
    pub fn add_asset(&mut self, cx: &mut Cx, id: String) {
        let Some(target) = self.add_to else {
            return;
        };
        // A single plant has one asset: what was picked replaces it.
        let single = self
            .selected
            .as_deref()
            .and_then(|id| stored(cx, id))
            .map_or(self.tool, |g| tool_of(&g))
            == Tool::Single;
        let editing = self.editing();
        let tool = self.tool;
        let form = self.active_form();
        // The starting palette is only a placeholder: the first pick replaces it (a hedge drawn
        // after picking hornbeam is not half cherry trees).
        let placeholder = !editing && form.palette == Form::for_tool(tool).palette;
        if (single || placeholder) && matches!(target, AddTarget::Palette) {
            form.palette.clear();
        }
        let palette = match target {
            AddTarget::Palette => &mut form.palette,
            AddTarget::Layer(i) => match form.layers.get_mut(i) {
                Some(l) => &mut l.asset_palette,
                None => return,
            },
        };
        if !palette.iter().any(|w| w.asset == id) {
            palette.push(Weighted {
                asset: id,
                weight: 1.0,
            });
            if self.editing() {
                self.finish_edit(cx, Edit::Done);
            }
        }
    }

    pub fn duplicate(&mut self, cx: &mut Cx) {
        let Some(g) = self.selected.as_deref().and_then(|id| stored(cx, id)) else {
            return;
        };
        let p = json!({"id": g.id(), "offset": [2.0, 2.0], "new_seed": true});
        if let Some(r) = cx.run("planting.duplicate", p) {
            let id = r["id"].as_str().map(str::to_string);
            self.select_generator(cx, id);
        }
    }

    pub fn delete_selected(&mut self, cx: &mut Cx) {
        if let Some(id) = self.selected.take() {
            cx.s.set_draft(None);
            cx.run("planting.remove", json!({"id": id}));
            self.edit = None;
            self.vertex = None;
        }
    }

    /// Paints on an existing brush: switches to the Brush tool with its settings.
    pub fn paint_more(&mut self, cx: &mut Cx) {
        let Some(g) = self.selected.as_deref().and_then(|id| stored(cx, id)) else {
            return;
        };
        if !matches!(g, Generator::MixedBrush(_)) {
            return;
        }
        let form = Form::from_generator(&g);
        self.set_tool(cx, Tool::Brush);
        self.brush_target = Some((g.id().to_string(), form.signature()));
        self.form = form;
    }

    pub fn target_name(&self, cx: &Cx) -> Option<String> {
        let (id, _) = self.brush_target.as_ref()?;
        stored(cx, id).map(|g| g.name().to_string())
    }

    fn name_for(&self, cx: &Cx, form: &Form, tool: Tool) -> String {
        let first = form
            .palette
            .first()
            .map_or(String::new(), |w| display_name(cx, &w.asset));
        // Lines, clusters and singles use the palette even when the form holds a preset's layers.
        if tool.layered() {
            form.name(tool.label(), &first)
        } else {
            format!("{} · {first}", tool.label())
        }
    }

    fn place(&mut self, cx: &mut Cx, form: &Form, tool: Tool, geom: &Geom) -> Option<String> {
        let name = self.name_for(cx, form, tool);
        match cx
            .s
            .with_lamps(|s| commit(s, form, geom, &name).map_err(CommandError::Failed))
            .map_err(|e| e.to_string())
        {
            Ok(id) => {
                self.form.seed = next_seed(self.form.seed);
                self.counter += 1;
                Some(id)
            }
            Err(e) => {
                cx.toasts.error(e);
                None
            }
        }
    }

    // ---- per frame ----

    pub fn update(&mut self, cx: &mut Cx) {
        let now = cx.ctx.input(|i| i.time);
        self.housekeeping(cx);
        self.keys(cx);
        let (mut depth, mut top) = (Vec::new(), Vec::new());
        let mut captured = false;
        let ctrl = cx.ctx.input(|i| i.modifiers.command);
        match self.tool {
            Tool::Select => captured = self.select(cx, &mut top),
            Tool::Single => {
                self.single(cx, &mut top);
                captured = !ctrl;
            }
            Tool::Line => self.polyline_tool(cx, now, &mut top, false),
            Tool::Area => self.polyline_tool(cx, now, &mut top, true),
            Tool::Cluster => {
                self.cluster(cx, now, &mut top);
                captured = !ctrl;
            }
            Tool::Surface => self.surface(cx, now),
            Tool::Brush => {
                self.brush(cx, now, &mut top);
                captured = !ctrl;
            }
        }
        dot_lines(&self.preview, &mut depth);
        cx.s.extra = (depth, top);
        cx.s.pointer_captured = captured || !matches!(self.drag, Drag::None);
        cx.s.double_click_captured =
            matches!(self.tool, Tool::Line | Tool::Area) && !self.points.is_empty();
    }

    fn housekeeping(&mut self, cx: &mut Cx) {
        if let Some(id) = &self.selected {
            match stored(cx, id) {
                None => {
                    self.selected = None;
                    self.edit = None;
                    self.vertex = None;
                    self.drag = Drag::None;
                }
                // Undo, redo or another command changed the project: show its settings.
                Some(g) if self.edit_rev != cx.s.session.revision() && cx.s.draft.is_none() => {
                    self.edit = Some(Form::from_generator(&g));
                    self.edit_rev = cx.s.session.revision();
                }
                Some(_) => {}
            }
        }
        if let Some((id, _)) = &self.brush_target
            && stored(cx, id).is_none()
        {
            self.brush_target = None;
        }
    }

    fn keys(&mut self, cx: &mut Cx) {
        if cx.ctx.egui_wants_keyboard_input() {
            return;
        }
        let (ctrl, shift) = cx.ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        if key_pressed(cx, Key::Questionmark) || (shift && key_pressed(cx, Key::Slash)) {
            self.show_help = !self.show_help;
        }
        if cx.input.escape && self.show_help {
            self.show_help = false;
            return;
        }
        if !ctrl {
            for (k, tool) in [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
            ]
            .into_iter()
            .zip(Tool::ALL)
            {
                if key_pressed(cx, k) {
                    self.set_tool(cx, tool);
                }
            }
        }
        if self.tool == Tool::Brush {
            if key_pressed(cx, Key::OpenBracket) {
                self.brush_radius = (self.brush_radius / 1.2).clamp(BRUSH_MIN, BRUSH_MAX);
            }
            if key_pressed(cx, Key::CloseBracket) {
                self.brush_radius = (self.brush_radius * 1.2).clamp(BRUSH_MIN, BRUSH_MAX);
            }
        }
        if self.tool == Tool::Single {
            // The same as Shift+wheel, for a keyboard or a wheel the browser keeps for itself.
            let up = key_pressed(cx, Key::Plus) || key_pressed(cx, Key::Equals);
            if up || key_pressed(cx, Key::Minus) {
                let k = if up { 1.1 } else { 1.0 / 1.1 };
                self.form.single_scale = (self.form.single_scale * k).clamp(0.1, 6.0);
            }
        }
        if ctrl && key_pressed(cx, Key::D) {
            self.duplicate(cx);
        }
        // Esc: drop what is pending, then leave the tool, then deselect.
        if cx.input.escape {
            if !self.points.is_empty() || !self.stroke.is_empty() {
                self.points.clear();
                self.stroke.clear();
                self.drag = Drag::None;
                self.preview = Preview::default();
            } else if self.tool != Tool::Select {
                self.set_tool(cx, Tool::Select);
            } else if self.vertex.is_some() {
                self.vertex = None;
            } else if self.selected.is_some() {
                self.select_generator(cx, None);
            }
        }
    }

    /// Re-evaluates the preview when `key` changed and the throttle allows it.
    fn refresh_preview(&mut self, cx: &Cx, key: String, g: Option<Generator>, now: f64) {
        let Some(g) = g else {
            if !self.preview.key.is_empty() {
                self.preview = Preview::default();
            }
            return;
        };
        if self.preview.key == key {
            return;
        }
        if !self.throttle.ready(now) {
            cx.ctx
                .request_repaint_after(std::time::Duration::from_millis(60));
            return;
        }
        let t0 = now_ms();
        self.preview = compute_preview(&cx.s.scene, &g);
        self.preview.key = key;
        self.throttle.done(now, (now_ms() - t0) / 1000.0);
    }

    // ---- Select ----

    fn select(&mut self, cx: &mut Cx, top: &mut Vec<LineVertex>) -> bool {
        let input = cx.input;
        let sel = self.selected.as_deref().and_then(|id| current(cx, id));
        let base = self.selected.as_deref().and_then(|id| stored(cx, id));
        // Handles on screen.
        let mut screen: Vec<(Handle, Vec2)> = Vec::new();
        let mut world: Vec<(Handle, [f32; 2], Vec3)> = Vec::new();
        if let Some(g) = &sel {
            for (h, p) in handles::handles(g) {
                let at = Vec3::new(p[0], ground_y(cx, p[0], p[1]) + 0.15, p[1]);
                world.push((h, p, at));
                if let Some(s) = cx.project(at) {
                    screen.push((h, Vec2::new(s.x, s.y)));
                }
            }
        }
        let hover = input.hover.map(|p| Vec2::new(p.x, p.y));
        let hot = hover.and_then(|p| handles::hit_handle(&screen, p, HANDLE_PX));

        if input.drag_started
            && let Some(po) = input.press_origin
            && let Some(h) = handles::hit_handle(&screen, Vec2::new(po.x, po.y), HANDLE_PX)
        {
            self.drag = Drag::Handle(h);
            if let Handle::Point(i) = h {
                self.vertex = Some(i);
            }
        }
        if let Drag::Handle(h) = self.drag {
            if input.dragging
                && let (Some(base), Some(hp)) = (&base, input.hover)
                && let Some(gp) = ground_at(cx, hp)
                && let Some(ng) =
                    handles::move_handle(base, h, [gp.x, gp.z], ground_y(cx, gp.x, gp.z))
            {
                cx.s.set_draft(Some(Draft::Generator(ng)));
            }
            if input.drag_stopped {
                if let (Some(base), Some(Draft::Generator(ng))) = (&base, cx.s.draft.clone()) {
                    cx.s.set_draft(None);
                    self.commit_update(cx, base, &ng);
                }
                cx.s.set_draft(None);
                self.drag = Drag::None;
            }
        } else if input.clicked
            && let Some(hv) = hover
        {
            if let Some(Handle::Point(i)) = handles::hit_handle(&screen, hv, HANDLE_PX) {
                self.vertex = Some(i);
            } else if let Some(g) = &sel
                && let Some((pts, closed)) = handles::polyline(g)
                && pts.len() == screen.len()
                && let Some((edge, f)) = handles::edge_hit_screen(
                    &screen.iter().map(|s| s.1).collect::<Vec<_>>(),
                    closed,
                    hv,
                    HANDLE_PX * 0.7,
                )
            {
                let (a, b) = (pts[edge], pts[(edge + 1) % pts.len()]);
                let at = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
                if let (Some(base), Some(ng)) = (&base, handles::insert_point(g, edge, at)) {
                    self.commit_update(cx, base, &ng);
                    self.vertex = Some(edge + 1);
                }
            } else {
                self.pick(cx, hv);
            }
        }

        // Delete a vertex, or the whole planting.
        if input.delete || (input.backspace && self.vertex.is_some()) {
            match (self.vertex, &base) {
                (Some(i), Some(g)) => match handles::remove_point(g, i) {
                    Some(ng) => {
                        self.commit_update(cx, g, &ng);
                        self.vertex = None;
                    }
                    None => cx.toasts.info(t::PT_KEEP_POINTS),
                },
                (None, Some(_)) if input.delete => self.delete_selected(cx),
                _ => {}
            }
        }

        // Overlay of the selection.
        if let Some(g) = &sel {
            let blue = [90, 200, 255, 255];
            match g {
                Generator::Cluster(c) => top.extend(ring(cx, c.center, c.radius_m, blue, 2.5)),
                Generator::Single(s) => {
                    let (x, z) = (s.position[0], s.position[2]);
                    let pts: Vec<[f32; 2]> = world.iter().map(|w| w.1).collect();
                    top.extend(ground_line(cx, &[[x, z], pts[1]], false, blue, 2.0));
                    top.extend(ground_line(
                        cx,
                        &[[x, z], pts[2]],
                        false,
                        [255, 200, 70, 200],
                        2.0,
                    ));
                }
                _ => {
                    if let Some((pts, closed)) = handles::polyline(g) {
                        top.extend(ground_line(cx, pts, closed, blue, 3.0));
                    }
                }
            }
            for (h, p, at) in &world {
                let picked = matches!(h, Handle::Point(k) if self.vertex == Some(*k));
                let is_hot = hot == Some(*h) || matches!(self.drag, Drag::Handle(d) if d == *h);
                let (color, w) = if picked {
                    ([255, 200, 70, 255], 13.0)
                } else if is_hot {
                    ([255, 255, 255, 255], 13.0)
                } else if matches!(h, Handle::Rotate | Handle::Scale | Handle::Radius) {
                    ([255, 200, 70, 255], 9.0)
                } else {
                    ([255, 255, 255, 255], 9.0)
                };
                top.extend(dot(*p, at.y, color, w));
            }
        }
        hot.is_some()
    }

    fn pick(&mut self, cx: &mut Cx, hv: Vec2) {
        let (o, d) = cx.s.pointer_ray(hv, cx.size());
        let plant = cx.s.scene.pick_plant(o, d);
        let ground = ground_distance(cx, o, d);
        // A trunk stands on the ground that is hit around its foot: allow a little behind it.
        let id = plant
            .filter(|(_, t)| ground.is_none_or(|g| *t <= g + 2.0))
            .map(|p| p.0);
        self.select_generator(cx, id);
    }

    // ---- Single ----

    fn single(&mut self, cx: &mut Cx, top: &mut Vec<LineVertex>) {
        let input = cx.input;
        let (shift, ctrl) = cx.ctx.input(|i| (i.modifiers.shift, i.modifiers.command));
        let hover = input.hover.and_then(|p| ground_at(cx, p));
        if shift && input.hover.is_some() {
            let wheel = cx.ctx.input(|i| i.smooth_scroll_delta);
            let d = wheel.x + wheel.y;
            if d != 0.0 {
                let s = (self.form.single_scale * 1.1f32.powf(d.signum())).clamp(0.1, 6.0);
                self.form.single_scale = s;
            }
        }
        if input.drag_started
            && !ctrl
            && let Some(a) = input.press_origin.and_then(|p| ground_at(cx, p))
        {
            self.drag = Drag::Anchor(a);
        }
        let anchor = match self.drag {
            Drag::Anchor(a) => Some(a),
            _ => None,
        };
        let spin = hash01(self.counter * 7 + 1) * 360.0;
        let mut rot = if self.form.spin {
            spin
        } else {
            self.form.rotation
        };
        if let (Some(a), Some(h)) = (anchor, hover)
            && (h.x - a.x).hypot(h.z - a.z) > 0.25
        {
            rot = (-(h.z - a.z)).atan2(h.x - a.x).to_degrees();
        }
        let at = anchor.or(hover);
        let asset =
            pick_weighted(&self.form.palette, hash01(self.counter * 13 + 5)).map(str::to_string);
        if let (Some(at), Some(asset)) = (at, &asset) {
            let (h, r) = crate::plant_scene::plant_size(cx.s.project(), &cx.s.assets, asset);
            let s = self.form.single_scale;
            let base = Vec3::new(at.x, at.y + 0.1, at.z);
            top.extend(ring(
                cx,
                [at.x, at.z],
                (r * 0.8).max(0.25) * s,
                [255, 255, 255, 230],
                2.0,
            ));
            top.extend(line(
                base,
                base + Vec3::Y * h * s,
                [255, 255, 255, 160],
                2.0,
            ));
            let a = rot.to_radians();
            let tip = base + Vec3::new(a.cos(), 0.0, -a.sin()) * (r.max(0.5) * s + 0.8);
            top.extend(line(base, tip, [255, 200, 70, 255], 3.0));
            top.extend(dot([at.x, at.z], base.y, [255, 255, 255, 255], 8.0));
            cx.label(
                cx.project(base + Vec3::Y * (h * s + 0.3))
                    .unwrap_or_default(),
                &format!("×{s:.2}  {rot:.0}°"),
            );
        }
        let place_at = if input.drag_stopped {
            anchor
        } else if input.clicked {
            hover
        } else {
            None
        };
        if input.drag_stopped {
            self.drag = Drag::None;
        }
        if let (Some(p), Some(asset)) = (place_at, asset) {
            let mut f = self.form.clone();
            f.palette = vec![Weighted { asset, weight: 1.0 }];
            let y = cx.s.scene.ground_fn()(p.x, p.z).unwrap_or(p.y);
            self.place(
                cx,
                &f,
                Tool::Single,
                &Geom::Single {
                    position: [p.x, y, p.z],
                    rotation_deg: rot,
                },
            );
        }
    }

    // ---- Line and Area ----

    fn polyline_tool(&mut self, cx: &mut Cx, now: f64, top: &mut Vec<LineVertex>, area: bool) {
        let input = cx.input;
        let hover = input.hover.and_then(|p| ground_at(cx, p));
        let radius = hover.map_or(0.3, |h| 10.0 * px_metres(cx, h));
        let existing: Vec<[f32; 2]> =
            cx.s.project()
                .shapes
                .iter()
                .flat_map(|s| s.outline().iter().copied())
                .collect();
        let snap =
            hover.map(|g| snap_point([g.x, g.z], &self.points, &existing, radius, !input.alt));
        if input.backspace {
            self.points.pop();
        }
        let need = if area { 3 } else { 2 };
        let closing = area && snap.is_some_and(|s| closes(&self.points, s.pos, radius));
        if input.clicked
            && let Some(s) = snap
        {
            if closing || (self.points.last() == Some(&s.pos) && self.points.len() >= need) {
                // The first point closes an area; the last one clicked again (a double click) ends it.
                self.finish_polyline(cx, area);
            } else if !input.double_clicked && self.points.last() != Some(&s.pos) {
                self.points.push(s.pos);
            }
        }
        if (input.enter || input.double_clicked) && self.points.len() >= need {
            self.finish_polyline(cx, area);
        }
        let mut pts = self.points.clone();
        if let Some(s) = snap
            && pts.last() != Some(&s.pos)
        {
            pts.push(s.pos);
        }
        if pts.len() >= need {
            let geom = if area {
                Geom::Area(pts.clone())
            } else {
                Geom::Line(pts.clone())
            };
            let key = format!("{:?}|{:?}", self.form, pts);
            self.refresh_preview(cx, key, preview_generator(&self.form, &geom), now);
        } else {
            self.refresh_preview(cx, String::new(), None, now);
        }
        let accent = [120, 230, 140, 255];
        top.extend(ground_line(cx, &pts, area && pts.len() >= 3, accent, 3.0));
        let y = |p: [f32; 2]| ground_y(cx, p[0], p[1]) + 0.12;
        for (i, p) in self.points.iter().enumerate() {
            let first = area && i == 0 && self.points.len() >= 3;
            top.extend(dot(
                *p,
                y(*p),
                if first { [255, 255, 255, 255] } else { accent },
                if first { 12.0 } else { 9.0 },
            ));
        }
        if let Some(s) = snap {
            top.extend(dot(s.pos, y(s.pos), [255, 255, 255, 220], 6.0));
        }
    }

    fn finish_polyline(&mut self, cx: &mut Cx, area: bool) {
        let pts = std::mem::take(&mut self.points);
        self.preview = Preview::default();
        let (geom, tool) = if area {
            (Geom::Area(pts), Tool::Area)
        } else {
            (Geom::Line(pts), Tool::Line)
        };
        let form = self.form.clone();
        self.place(cx, &form, tool, &geom);
    }

    // ---- Cluster ----

    fn cluster(&mut self, cx: &mut Cx, now: f64, top: &mut Vec<LineVertex>) {
        let input = cx.input;
        let ctrl = cx.ctx.input(|i| i.modifiers.command);
        let hover = input.hover.and_then(|p| ground_at(cx, p));
        if input.drag_started
            && !ctrl
            && let Some(a) = input.press_origin.and_then(|p| ground_at(cx, p))
        {
            self.drag = Drag::Anchor(a);
        }
        let anchor = match self.drag {
            Drag::Anchor(a) => Some(a),
            _ => None,
        };
        let (center, radius) = match (anchor, hover) {
            (Some(a), Some(h)) => (Some(a), (h.x - a.x).hypot(h.z - a.z).max(0.3)),
            (None, Some(h)) => (Some(h), self.form.radius),
            _ => (None, self.form.radius),
        };
        if let Some(c) = center {
            let geom = Geom::Cluster {
                center: [c.x, c.z],
                radius,
            };
            let key = format!("{:?}|{:?}", self.form, geom);
            self.refresh_preview(cx, key, preview_generator(&self.form, &geom), now);
            top.extend(ring(cx, [c.x, c.z], radius, [120, 230, 140, 255], 2.5));
            cx.label(cx.project(c).unwrap_or_default(), &format!("{radius:.1} m"));
            let commit_now = if input.drag_stopped {
                anchor.is_some()
            } else {
                input.clicked
            };
            if input.drag_stopped {
                self.drag = Drag::None;
            }
            if commit_now {
                self.form.radius = radius.max(0.3);
                let form = self.form.clone();
                self.preview = Preview::default();
                self.place(cx, &form, Tool::Cluster, &geom);
            }
        } else {
            self.refresh_preview(cx, String::new(), None, now);
        }
    }

    // ---- Surface fill ----

    fn surface(&mut self, cx: &mut Cx, now: f64) {
        let input = cx.input;
        let size = cx.size();
        let pick = input
            .hover
            .and_then(|h| cx.s.pick(Vec2::new(h.x, h.y), size, false));
        cx.s.highlight = pick.as_ref().map(|p| (p.owner.clone(), p.face));
        match &pick {
            Some(p) => {
                let geom = Geom::Surface {
                    owner: p.owner.clone(),
                    faces: vec![p.face_name.clone()],
                };
                let key = format!("{:?}|{:?}", self.form, geom);
                self.refresh_preview(cx, key, preview_generator(&self.form, &geom), now);
            }
            None => self.refresh_preview(cx, String::new(), None, now),
        }
        let shift = cx.ctx.input(|i| i.modifiers.shift);
        if input.clicked
            && let Some(p) = pick
        {
            let extend = self
                .last_fill
                .as_deref()
                .and_then(|id| stored(cx, id))
                .filter(|g| shift && matches!(g, Generator::SurfaceFill(f) if f.owner == p.owner));
            if let Some(Generator::SurfaceFill(f)) = extend {
                if !f.faces.contains(&p.face_name) {
                    let mut faces = f.faces.clone();
                    faces.push(p.face_name.clone());
                    cx.run("planting.update", json!({"id": f.id, "faces": faces}));
                }
            } else {
                let form = self.form.clone();
                let geom = Geom::Surface {
                    owner: p.owner,
                    faces: vec![p.face_name],
                };
                self.last_fill = self.place(cx, &form, Tool::Surface, &geom);
            }
        }
    }

    // ---- Brush ----

    /// With "Keep inside area": the outline of the topmost area under `p` (a border's bed, a
    /// lawn), else nothing.
    fn clip_under(&self, cx: &Cx, p: [f32; 2]) -> Vec<[f32; 2]> {
        if !self.form.keep_inside {
            return Vec::new();
        }
        cx.s.project()
            .shapes
            .iter()
            .rev()
            .find_map(|s| match s {
                visioncraft_core::Shape::Area(a)
                    if visioncraft_core::inside_outline(p, &a.outline) =>
                {
                    Some(a.outline.clone())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    fn brush(&mut self, cx: &mut Cx, now: f64, top: &mut Vec<LineVertex>) {
        let input = cx.input;
        let ctrl = cx.ctx.input(|i| i.modifiers.command);
        let erase = self.erase || input.alt;
        let r = self.brush_radius;
        let hover = input.hover.and_then(|p| ground_at(cx, p));
        let spacing = (r * 0.25).max(0.05);
        let far_enough = |s: &[[f32; 2]], p: Vec3| {
            s.last()
                .is_none_or(|l| (l[0] - p.x).hypot(l[1] - p.z) >= spacing)
        };
        if input.drag_started
            && !ctrl
            && let Some(g) = input.press_origin.and_then(|p| ground_at(cx, p))
        {
            self.drag = Drag::Stroke;
            self.stroke = vec![[g.x, g.z]];
            self.stroke_erase = erase;
        }
        let mut finished: Option<Vec<[f32; 2]>> = None;
        if matches!(self.drag, Drag::Stroke) {
            if input.dragging
                && let Some(g) = hover
                && far_enough(&self.stroke, g)
                && self.stroke.len() < 6000
            {
                self.stroke.push([g.x, g.z]);
            }
            if input.drag_stopped {
                if let Some(g) = hover
                    && far_enough(&self.stroke, g)
                {
                    self.stroke.push([g.x, g.z]);
                }
                finished = Some(std::mem::take(&mut self.stroke));
                self.drag = Drag::None;
            }
        } else if input.clicked
            && let Some(g) = hover
        {
            self.stroke_erase = erase;
            finished = Some(vec![[g.x, g.z]]);
        }
        if let Some(points) = finished {
            if !self.stroke_erase {
                self.form.clip = self.clip_under(cx, points[0]);
            }
            let stroke = Stroke {
                points,
                radius_m: r,
                erase: self.stroke_erase,
            };
            let form = self.form.clone();
            let name = self.name_for(cx, &form, Tool::Brush);
            self.preview = Preview::default();
            let mut target = self.brush_target.take();
            let before = target.as_ref().map(|t| t.0.clone());
            match cx
                .s
                .with_lamps(|s| {
                    commit_stroke(s, &mut target, &form, stroke, &name)
                        .map_err(CommandError::Failed)
                })
                .map_err(|e| e.to_string())
            {
                Ok(id) => {
                    // A new brush gets a new seed for the next one; this one keeps its own.
                    if before.as_deref() != Some(id.as_str()) {
                        self.form.seed = next_seed(self.form.seed);
                    }
                }
                Err(e) => cx.toasts.error(e),
            }
            self.brush_target = target;
        }
        // Preview: the in-progress stroke on a scratch brush with the target's seed.
        if self.stroke.is_empty() || self.stroke_erase {
            self.refresh_preview(cx, String::new(), None, now);
        } else {
            let mut f = self.form.clone();
            f.clip = self.clip_under(cx, self.stroke[0]);
            if let Some((id, _)) = &self.brush_target
                && let Some(Generator::MixedBrush(b)) = stored(cx, id)
            {
                f.seed = b.seed;
            }
            let geom = Geom::Brush(vec![Stroke {
                points: self.stroke.clone(),
                radius_m: r,
                erase: false,
            }]);
            let key = format!("{:?}|{}|{r}|{:?}", f, self.stroke.len(), self.stroke.last());
            self.refresh_preview(cx, key, preview_generator(&f, &geom), now);
        }
        // Overlay: the trail and the circle that follows the ground.
        let color = if erase {
            [255, 110, 100, 255]
        } else {
            [255, 255, 255, 235]
        };
        if self.stroke.len() >= 2 {
            top.extend(ground_line(cx, &self.stroke, false, color, 3.0));
        }
        if let Some(g) = hover {
            top.extend(ring(cx, [g.x, g.z], r, color, 2.5));
            cx.label(
                cx.project(g).unwrap_or_default(),
                &format!("{r:.1} m{}", if erase { " · erase" } else { "" }),
            );
        }
    }

    pub fn readout(&self, cx: &Cx) -> Option<(usize, bool)> {
        match (&self.selected, self.tool) {
            (Some(id), Tool::Select) => cx.s.scene.gen_stats(id).map(|s| (s.count, s.truncated)),
            (None, tool) if tool != Tool::Select && !self.preview.key.is_empty() => {
                Some((self.preview.count, self.preview.truncated))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_core::Project;
    use visioncraft_core::planting_presets;

    fn w(a: &str, weight: f32) -> Weighted {
        Weighted {
            asset: a.into(),
            weight,
        }
    }

    fn oak() -> Form {
        Form {
            palette: vec![w("flora:quercus-robur", 1.0)],
            ..Form::default()
        }
    }

    fn square() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    fn stroke(x: f32, erase: bool) -> Stroke {
        Stroke {
            points: vec![[x, 0.0], [x + 5.0, 0.0]],
            radius_m: 2.0,
            erase,
        }
    }

    #[test]
    fn throttle_waits_a_tenth_of_a_second_and_longer_when_slow() {
        let mut t = Throttle::default();
        assert!(t.ready(1.0));
        t.done(1.0, 0.01);
        assert!(!t.ready(1.05));
        assert!(t.ready(1.11));
        t.done(2.0, 0.2);
        assert!(
            !t.ready(2.3),
            "a slow evaluation backs off: {}",
            t.interval()
        );
        assert!(t.ready(2.41));
    }

    #[test]
    fn weighted_pick_follows_the_weights() {
        let p = [w("a", 1.0), w("b", 3.0), w("c", 0.0)];
        assert_eq!(pick_weighted(&p, 0.1), Some("a"));
        assert_eq!(pick_weighted(&p, 0.3), Some("b"));
        assert_eq!(pick_weighted(&p, 0.999), Some("b"));
        assert_eq!(pick_weighted(&[w("a", 0.0)], 0.5), None);
    }

    #[test]
    fn every_tool_commits_one_valid_undoable_command() {
        let geoms = [
            Geom::Single {
                position: [1.0, 0.0, 2.0],
                rotation_deg: 30.0,
            },
            Geom::Line(vec![[0.0, 0.0], [10.0, 0.0], [20.0, 5.0]]),
            Geom::Cluster {
                center: [3.0, 3.0],
                radius: 2.0,
            },
            Geom::Area(square()),
            Geom::Brush(vec![stroke(0.0, false)]),
        ];
        for geom in geoms {
            for layered in [false, true] {
                let mut form = oak();
                if layered {
                    form.layers = planting_presets()[2].layers.clone();
                }
                let mut s = Session::new(Project::default());
                let rev = s.revision();
                commit(&mut s, &form, &geom, "x").unwrap_or_else(|e| panic!("{geom:?}: {e}"));
                assert_eq!(s.revision(), rev + 1);
                assert_eq!(s.project().generators.len(), 1);
                assert!(preview_generator(&form, &geom).is_some(), "{geom:?}");
                s.undo();
                assert!(s.project().generators.is_empty());
            }
        }
    }

    #[test]
    fn surface_fill_stores_face_names_of_a_shape() {
        let mut s = Session::new(Project::default());
        let r = s
            .run(
                "shape.add_area",
                json!({"name": "Lawn", "outline": square()}),
            )
            .unwrap();
        let owner = r["id"].as_str().unwrap().to_string();
        let geom = Geom::Surface {
            owner,
            faces: vec!["top".into()],
        };
        commit(&mut s, &oak(), &geom, "fill").unwrap();
        assert!(
            matches!(&s.project().generators[0], Generator::SurfaceFill(f) if f.faces == ["top"])
        );
    }

    #[test]
    fn strokes_extend_one_generator_until_the_settings_change() {
        let mut s = Session::new(Project::default());
        let mut target = None;
        let form = oak();
        commit_stroke(&mut s, &mut target, &form, stroke(0.0, false), "b").unwrap();
        commit_stroke(&mut s, &mut target, &form, stroke(20.0, false), "b").unwrap();
        commit_stroke(&mut s, &mut target, &form, stroke(5.0, true), "b").unwrap();
        assert_eq!(s.project().generators.len(), 1);
        let Generator::MixedBrush(b) = &s.project().generators[0] else {
            panic!()
        };
        assert_eq!(b.strokes.len(), 3);
        assert!(b.strokes[2].erase);
        // A reshuffled seed alone does not start another brush; a changed palette does.
        let reshuffled = Form {
            seed: 99,
            ..form.clone()
        };
        commit_stroke(&mut s, &mut target, &reshuffled, stroke(40.0, false), "b").unwrap();
        assert_eq!(s.project().generators.len(), 1);
        let other = Form {
            palette: vec![w("flora:fagus-sylvatica", 1.0)],
            ..form
        };
        commit_stroke(&mut s, &mut target, &other, stroke(60.0, false), "b").unwrap();
        assert_eq!(s.project().generators.len(), 2);
        // Erasing with nothing painted yet says so instead of making an empty brush.
        let mut none = None;
        let mut s2 = Session::new(Project::default());
        assert!(commit_stroke(&mut s2, &mut none, &oak(), stroke(0.0, true), "b").is_err());
        assert!(s2.project().generators.is_empty());
    }

    #[test]
    fn a_form_round_trips_through_its_generator() {
        let mut form = oak();
        form.layers = planting_presets()[0].layers.clone();
        form.density = 0.3;
        form.max_slope = 20.0;
        for geom in [Geom::Area(square()), Geom::Brush(vec![stroke(0.0, false)])] {
            let g = preview_generator(&form, &geom).unwrap();
            let back = Form::from_generator(&g);
            assert_eq!(back.layers, form.layers);
            assert_eq!(back.density, 0.3);
            // Editing the density only changes that field of the stored generator.
            let edited = Form {
                density: 0.4,
                ..back
            };
            let g2 = edited.apply(&g);
            let d = handles::diff_params(&g, &g2).unwrap();
            assert_eq!(d["density_per_m2"], json!(0.4f32));
            assert_eq!(d.as_object().unwrap().len(), 2, "{d}");
        }
    }

    #[test]
    fn layer_indices_name_the_storey_of_each_asset() {
        let mut form = oak();
        form.layers = planting_presets()[0].layers.clone();
        let g = preview_generator(&form, &Geom::Area(square())).unwrap();
        let m = layer_indices(&g);
        assert_eq!(m["flora:quercus-robur"], 0);
        assert_eq!(m["flora:ilex-aquifolium"], 1);
        assert_eq!(m["flora:dryopteris-filix-mas"], 2);
    }

    #[test]
    fn every_flora_asset_of_every_preset_resolves() {
        for p in planting_presets() {
            for l in &p.layers {
                for w in &l.asset_palette {
                    let id = w
                        .asset
                        .strip_prefix("flora:")
                        .unwrap_or_else(|| panic!("{}: {}", p.id, w.asset));
                    assert!(
                        visioncraft_flora::Species::by_id(id).is_some(),
                        "preset {}: unknown species `{id}`",
                        p.id
                    );
                }
            }
        }
    }

    #[test]
    fn names_say_what_was_made() {
        let mut f = oak();
        assert_eq!(f.name("Line", "Oak"), "Line · Oak");
        f.layers = planting_presets()[0].layers.clone();
        assert_eq!(f.name("Brush", "Oak"), "Brush (layered)");
        f.preset = Some("Mixed woodland".into());
        assert_eq!(f.name("Brush", "Oak"), "Mixed woodland brush");
    }

    #[test]
    fn tools_start_with_a_palette_that_suits_them() {
        let mut t = PlantingTools::default();
        t.switch_form(Tool::Line);
        assert_eq!(t.form.palette[0].asset, "flora:carpinus-hedge");
        assert_eq!(
            (t.form.jitter, t.form.scale_min, t.form.scale_max),
            (0.0, 1.0, 1.0)
        );
        assert!(!t.form.spin);
        t.switch_form(Tool::Brush);
        assert!(t.form.palette.len() >= 3);
        t.switch_form(Tool::Single);
        assert_eq!(t.form.palette, Form::default().palette);
    }

    #[test]
    fn each_tool_keeps_its_own_palette() {
        let mut t = PlantingTools::default();
        t.switch_form(Tool::Line);
        t.form.palette = vec![Weighted {
            asset: "flora:carpinus-hedge".into(),
            weight: 1.0,
        }];
        t.switch_form(Tool::Single);
        assert_ne!(t.form.palette[0].asset, "flora:carpinus-hedge");
        t.switch_form(Tool::Line);
        assert_eq!(t.form.palette[0].asset, "flora:carpinus-hedge");
    }

    /// The same strokes give the same plants: two sessions, two evaluations each, a duplicate
    /// (its own seed) and a layered preset.
    #[test]
    fn a_brush_is_deterministic() {
        let ground = |_: f32, _: f32| Some(0.0);
        let mut forms = vec![oak()];
        let mut layered = oak();
        layered.layers = planting_presets()[2].layers.clone();
        forms.push(layered);
        for form in forms {
            let run = || {
                let mut s = Session::new(Project::default());
                let mut target = None;
                for x in [0.0, 7.0, 30.0] {
                    commit_stroke(&mut s, &mut target, &form, stroke(x, false), "b").unwrap();
                }
                commit_stroke(&mut s, &mut target, &form, stroke(7.0, true), "b").unwrap();
                s.project().generators[0].clone()
            };
            let (a, b) = (run(), run());
            assert_eq!(a, b);
            let pa = visioncraft_core::evaluate(&a, &ground).placements;
            let pb = visioncraft_core::evaluate(&b, &ground).placements;
            assert!(!pa.is_empty());
            assert_eq!(pa, pb);
            assert_eq!(pa, visioncraft_core::evaluate(&a, &ground).placements);
        }
    }
}
