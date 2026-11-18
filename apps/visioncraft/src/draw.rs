//! Draw mode: outlines drawn in a top view with snapping, and edited by dragging their corners.

use crate::scene::line;
use crate::strings as t;
use crate::ui::{Cx, Edit, drag_field};
use eframe::egui::{self, Color32, Pos2, Vec2 as EVec2};
use glam::Vec3;
use serde_json::json;
use visioncraft_core::{Shape, measure};
use visioncraft_render::LineVertex;

pub const GRID: f32 = 0.1;
type P2 = [f32; 2];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    Free,
    Grid,
    Vertex,
    RightAngle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub pos: P2,
    pub kind: SnapKind,
}

/// The width of the stairs the tool draws (metres).
const STAIRS_WIDTH_M: f32 = 1.2;

/// `pos` held on the line through `b` at a right angle to the segment `a` to `b`, at least
/// `min` metres from `b`.
fn right_angle_point(a: P2, b: P2, pos: P2, min: f32) -> P2 {
    let len = dist(a, b);
    if len < 1e-4 {
        return pos;
    }
    let n = [-(b[1] - a[1]) / len, (b[0] - a[0]) / len];
    let t = (pos[0] - b[0]) * n[0] + (pos[1] - b[1]) * n[1];
    let t = if t < 0.0 { t.min(-min) } else { t.max(min) };
    [b[0] + n[0] * t, b[1] + n[1] * t]
}

fn round(v: f32, step: f32) -> f32 {
    (v / step).round() * step
}

fn dist(a: P2, b: P2) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Snaps `cursor` (metres). Existing vertices win within `radius`, then a right angle to the
/// previous segment (or the world axes for the first segment) within five degrees, then the grid.
/// `grid: false` is the Alt key: no grid, no right angles.
pub fn snap_point(cursor: P2, pending: &[P2], existing: &[P2], radius: f32, grid: bool) -> Snap {
    let near = pending
        .iter()
        .chain(existing)
        .copied()
        .filter(|v| dist(*v, cursor) <= radius);
    if let Some(v) = near.min_by(|a, b| dist(*a, cursor).total_cmp(&dist(*b, cursor))) {
        return Snap {
            pos: v,
            kind: SnapKind::Vertex,
        };
    }
    if !grid {
        return Snap {
            pos: cursor,
            kind: SnapKind::Free,
        };
    }
    if let Some(&last) = pending.last() {
        let reference = match pending.len() {
            0 | 1 => [1.0, 0.0],
            n => {
                let prev = pending[n - 2];
                let d = [last[0] - prev[0], last[1] - prev[1]];
                let l = d[0].hypot(d[1]).max(1e-6);
                [d[0] / l, d[1] / l]
            }
        };
        let v = [cursor[0] - last[0], cursor[1] - last[1]];
        let len = v[0].hypot(v[1]);
        if len > radius {
            let mut dir = reference;
            for _ in 0..4 {
                let along = v[0] * dir[0] + v[1] * dir[1];
                if along > 0.0 && along / len > 5f32.to_radians().cos() {
                    let l = round(along, GRID);
                    return Snap {
                        pos: [last[0] + dir[0] * l, last[1] + dir[1] * l],
                        kind: SnapKind::RightAngle,
                    };
                }
                dir = [-dir[1], dir[0]];
            }
        }
    }
    Snap {
        pos: [round(cursor[0], GRID), round(cursor[1], GRID)],
        kind: SnapKind::Grid,
    }
}

pub fn vertex_hit(outline: &[P2], p: P2, radius: f32) -> Option<usize> {
    (0..outline.len())
        .filter(|&i| dist(outline[i], p) <= radius)
        .min_by(|&a, &b| dist(outline[a], p).total_cmp(&dist(outline[b], p)))
}

/// The edge `p` is within `radius` of, and the closest point on it.
pub fn edge_hit(outline: &[P2], closed: bool, p: P2, radius: f32) -> Option<(usize, P2)> {
    let n = outline.len();
    let edges = if closed { n } else { n.saturating_sub(1) };
    (0..edges)
        .filter_map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            let ab = [b[0] - a[0], b[1] - a[1]];
            let l2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-9);
            let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0);
            let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
            (dist(q, p) <= radius).then_some((i, q, dist(q, p)))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(i, q, _)| (i, q))
}

/// A circle of `radius` round `c`, as the polygon `shape.add_circle` makes of it.
fn circle_polygon(c: P2, radius: f32) -> Vec<P2> {
    let r = radius.max(0.0);
    let points = [
        [c[0] + r, c[1]],
        [c[0], c[1] + r],
        [c[0] - r, c[1]],
        [c[0], c[1] - r],
    ];
    let arc = visioncraft_core::curve::Curve::Arc {
        sagitta_m: r * (1.0 - std::f32::consts::FRAC_1_SQRT_2),
    };
    let curves = (0..4).map(|i| (i, arc)).collect();
    visioncraft_core::curve::tessellate(&points, true, &curves, false).pts
}

/// Whether a click at `p` closes a pending outline of `points`.
/// The mean of a shape's corners (a centre to push labels away from).
fn centroid(points: &[P2]) -> P2 {
    let n = points.len().max(1) as f32;
    let s = points
        .iter()
        .fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
    [s[0] / n, s[1] / n]
}

pub fn closes(points: &[P2], p: P2, radius: f32) -> bool {
    points.len() >= 3 && dist(points[0], p) <= radius
}

fn y_point(p: P2, y: f32) -> Vec3 {
    Vec3::new(p[0], y, p[1])
}

/// Lines through `points`, closed or not, at height `y`.
pub fn outline_lines(
    points: &[P2],
    closed: bool,
    y: f32,
    color: [u8; 4],
    width: f32,
) -> Vec<LineVertex> {
    let n = points.len();
    let mut out = Vec::new();
    for i in 0..n.saturating_sub(1) {
        out.extend(line(
            y_point(points[i], y),
            y_point(points[i + 1], y),
            color,
            width,
        ));
    }
    if closed && n > 2 {
        out.extend(line(
            y_point(points[n - 1], y),
            y_point(points[0], y),
            color,
            width,
        ));
    }
    out
}

pub fn dot(p: P2, y: f32, color: [u8; 4], width: f32) -> [LineVertex; 2] {
    line(y_point(p, y), y_point(p, y), color, width)
}

/// Total length of an outline in metres.
pub fn outline_length(outline: &[P2], closed: bool) -> f32 {
    let mut pts: Vec<[f32; 3]> = outline.iter().map(|p| [p[0], 0.0, p[1]]).collect();
    if closed && !pts.is_empty() {
        pts.push(pts[0]);
    }
    measure::polyline_length(&pts)
}

/// Grid spacing in metres for a view `half_height` metres tall.
pub fn grid_step(half_height: f32) -> f32 {
    if half_height < 30.0 {
        1.0
    } else if half_height < 120.0 {
        5.0
    } else {
        20.0
    }
}

/// A scale bar length (1, 2, 5, 10 ...) that is at most `max_m`.
pub fn nice_length(max_m: f32) -> f32 {
    let mag = 10f32.powf(max_m.log10().floor());
    [5.0, 2.0, 1.0]
        .into_iter()
        .map(|k| k * mag)
        .find(|l| *l <= max_m)
        .unwrap_or(mag)
}

pub fn grid_lines(centre: P2, half_w: f32, half_h: f32, step: f32) -> Vec<LineVertex> {
    let mut out = Vec::new();
    let (x0, x1) = (
        round(centre[0] - half_w, step) - step,
        round(centre[0] + half_w, step) + step,
    );
    let (z0, z1) = (
        round(centre[1] - half_h, step) - step,
        round(centre[1] + half_h, step) + step,
    );
    let y = 0.03;
    let color = |i: f32| {
        if (i / step).round() as i64 % 5 == 0 {
            [255, 255, 255, 70]
        } else {
            [255, 255, 255, 30]
        }
    };
    let mut x = x0;
    while x <= x1 {
        out.extend(line(
            Vec3::new(x, y, z0),
            Vec3::new(x, y, z1),
            color(x),
            1.0,
        ));
        x += step;
    }
    let mut z = z0;
    while z <= z1 {
        out.extend(line(
            Vec3::new(x0, y, z),
            Vec3::new(x1, y, z),
            color(z),
            1.0,
        ));
        z += step;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Area,
    Block,
    Wall,
    /// An open line drawn as a fence (`fence.add`).
    Fence,
    /// Two clicks, the bottom and then the top of a flight of steps (`stairs.add`); a flight that
    /// turns takes the corners in between and ends with Enter (or a fourth click).
    Stairs,
    /// A closed outline drawn as a raised bed with soil (`bed.add`).
    Bed,
    /// A closed outline cut into the terrain as a level with a retaining wall (`terrace.add`).
    Terrace,
    /// A click on the plan places a piece of furniture or a set (`furniture.add`,
    /// `furniture.add_set`).
    Furniture,
    /// Two clicks, the centre and a point on the edge of a round area (`shape.add_circle`).
    Circle,
    /// Puts a window, or a door, in the wall of a block that is clicked.
    Window,
    Door,
    /// The plan image under the drawing: its scale and place (`underlay.*`).
    Plan,
}

/// The size and frame new windows and doors get.
pub struct OpeningForm {
    pub window: (f32, f32, f32),
    pub door: (f32, f32),
    pub frame: &'static str,
}

impl Default for OpeningForm {
    fn default() -> Self {
        Self {
            // Width, height, sill; a door's width and height.
            window: (1.2, 1.4, 0.9),
            door: (1.0, 2.2),
            frame: "slim",
        }
    }
}

/// What the Terrace tool cuts: the floor (the mean of the ground under the outline, or a height),
/// the wall's style (an index into [`WALL_STYLES`]) and its parapet.
pub struct TerraceForm {
    pub mean: bool,
    pub floor: f32,
    pub style: usize,
    pub parapet: f32,
}

impl Default for TerraceForm {
    fn default() -> Self {
        Self {
            mean: true,
            floor: 0.0,
            style: 0,
            parapet: 0.0,
        }
    }
}

/// The styles of a terrace's wall: key and label.
const WALL_STYLES: [(&str, &str); 5] = [
    ("plain", t::BED_PLAIN),
    ("masonry", t::BED_MASONRY),
    ("sleepers", t::BED_SLEEPERS),
    ("corten", t::BED_CORTEN),
    ("gabion", t::BED_GABION),
];

/// What the Furniture tool places: a piece of one of the kinds, or a set (the entries after the
/// kinds); sizes at 0 are the piece's own.
pub struct FurnitureForm {
    pub choice: usize,
    pub length: f32,
    pub depth: f32,
    pub height: f32,
    pub rotation_deg: f32,
    pub seats: usize,
}

impl Default for FurnitureForm {
    fn default() -> Self {
        Self {
            choice: 0,
            length: 0.0,
            depth: 0.0,
            height: 0.0,
            rotation_deg: 0.0,
            seats: visioncraft_core::furniture::DEFAULT_SEATS,
        }
    }
}

/// The kinds of furniture, then the sets, as the Furniture tool lists them.
fn furniture_choices() -> Vec<(String, Option<visioncraft_core::furniture::FurnitureKind>)> {
    use visioncraft_core::furniture::{FurnitureKind, SETS};
    let label = |name: &str| {
        let mut s = name.replace('_', " ");
        s[..1].make_ascii_uppercase();
        s
    };
    FurnitureKind::ALL
        .iter()
        .map(|k| (label(k.name()), Some(*k)))
        .chain(SETS.iter().map(|s| (format!("{} set", label(s)), None)))
        .collect()
}

/// What an area is: decides its name and its material, so a terrace drawn on a lawn shows at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AreaKind {
    #[default]
    Lawn,
    Terrace,
    Path,
    Border,
    Water,
}

impl AreaKind {
    pub const ALL: [AreaKind; 5] = [
        AreaKind::Lawn,
        AreaKind::Terrace,
        AreaKind::Path,
        AreaKind::Border,
        AreaKind::Water,
    ];

    /// (label and name, project material).
    pub fn spec(self) -> (&'static str, &'static str) {
        match self {
            AreaKind::Lawn => (t::KIND_LAWN, "lawn"),
            AreaKind::Terrace => (t::KIND_TERRACE, "paving"),
            AreaKind::Path => (t::KIND_PATH, "gravel"),
            AreaKind::Border => (t::KIND_BORDER, "soil"),
            AreaKind::Water => (t::KIND_WATER, "water"),
        }
    }
}

/// `"{base} {n}"` with the first `n` from 1 that no shape uses yet: names count per kind.
pub fn next_name(project: &visioncraft_core::Project, base: &str) -> String {
    (1..)
        .map(|n| format!("{base} {n}"))
        .find(|name| !project.shapes.iter().any(|s| s.name() == name))
        .expect("an unused number")
}

#[derive(Default)]
pub struct DrawState {
    /// What the Area tool draws.
    pub area_kind: AreaKind,
    /// Whether the Stairs tool draws a flight that turns: the bottom, one or two corners and the
    /// top, each segment at a right angle to the one before (`stairs.add` with `via`).
    pub stairs_turn: bool,
    pub tool: Tool,
    pub points: Vec<P2>,
    pub selected: Option<String>,
    pub vertex: Option<usize>,
    /// The edge the curve fields act on (Shift-click on an edge, or the number field).
    pub edge: Option<usize>,
    /// The radius typed for the next arc (0: the edge's length).
    curve_radius: f32,
    dragging: Option<usize>,
    /// (shape id, text) of the name field while it has the keyboard.
    name_edit: Option<(String, String)>,
    /// Digits typed while drawing (the next segment's length), and a rectangle's width typed
    /// before Tab or x.
    typed: String,
    typed_width: Option<f32>,
    /// The camera before Draw switched to the plan.
    camera_before: Option<crate::camera::Camera>,
    /// The project had nothing in it when Draw opened.
    started_empty: bool,
    pub opening: OpeningForm,
    /// What the Furniture tool places.
    pub furniture: FurnitureForm,
    /// What the Terrace tool cuts.
    pub terrace: TerraceForm,
    /// The plan underlay's fields and clicks (the Plan tool).
    pub plan: crate::underlay_ui::PlanState,
}

const ACCENT: [u8; 4] = [255, 200, 70, 255];

/// Replaces corner `i` of a shape's outline.
fn with_vertex(shape: &Shape, i: usize, p: P2) -> Shape {
    let mut s = shape.clone();
    match &mut s {
        Shape::Area(a) => {
            a.points[i] = p;
            visioncraft_core::curve::fit(&a.points, true, &mut a.curves);
        }
        Shape::Extrusion(e) => {
            e.points[i] = p;
            visioncraft_core::curve::fit(&e.points, e.closed, &mut e.curves);
        }
    }
    s
}

fn snap_radius(cx: &Cx) -> f32 {
    // 10 points on screen, in metres
    10.0 * 2.0 * cx.s.camera.top_half_height / cx.input.rect.height().max(1.0)
}

fn all_vertices(cx: &Cx, except: Option<&str>) -> Vec<P2> {
    cx.s.project()
        .shapes
        .iter()
        .filter(|s| Some(s.id()) != except)
        .flat_map(|s| s.points().iter().copied())
        .collect()
}

impl DrawState {
    pub fn enter(&mut self, cx: &mut Cx) {
        self.camera_before = Some(cx.s.camera.clone());
        let p = cx.s.project();
        self.started_empty = p.shapes.is_empty() && p.models.is_empty();
        cx.s.camera.view = crate::camera::View::Top;
        self.points.clear();
    }

    /// Back to the camera there was before Draw (the plan's own pan and zoom stay for next time).
    pub fn leave(&mut self, cx: &mut Cx) {
        let (center, half) = (cx.s.camera.top_center, cx.s.camera.top_half_height);
        match self.camera_before.take() {
            Some(c) => cx.s.camera = c,
            None => cx.s.camera.view = crate::camera::View::Orbit,
        }
        (cx.s.camera.top_center, cx.s.camera.top_half_height) = (center, half);
        // The first drawing of a new project: the 3D view shows all of it, not the empty start.
        if std::mem::take(&mut self.started_empty) && !cx.s.project().shapes.is_empty() {
            cx.s.frame_all();
        }
        cx.s.set_draft(None);
        cx.s.extra = Default::default();
        self.plan.cancel(cx);
        self.points.clear();
        self.tool = Tool::Select;
    }

    /// Before leaving for Walk: a shape being drawn with enough corners is kept (as Enter would),
    /// a shorter one is dropped.
    pub fn finish_for_walk(&mut self, cx: &mut Cx) {
        let needed = if self.tool == Tool::Stairs && self.stairs_turn {
            3
        } else if matches!(
            self.tool,
            Tool::Wall | Tool::Fence | Tool::Stairs | Tool::Circle
        ) {
            2
        } else {
            3
        };
        if matches!(
            self.tool,
            Tool::Area
                | Tool::Block
                | Tool::Wall
                | Tool::Fence
                | Tool::Stairs
                | Tool::Bed
                | Tool::Terrace
                | Tool::Circle
        ) && self.points.len() >= needed
        {
            self.finish(cx);
        }
        self.points.clear();
    }

    fn shape(&self, cx: &Cx) -> Option<Shape> {
        let id = self.selected.as_ref()?;
        if let Some(crate::studio::Draft::Shape(s)) = &cx.s.draft
            && s.id() == id
        {
            return Some(s.clone());
        }
        cx.s.project().shapes.iter().find(|s| s.id() == id).cloned()
    }

    fn world_at(cx: &Cx, p: Pos2) -> Option<P2> {
        let (o, d) = cx.s.pointer_ray(
            glam::Vec2::new(p.x, p.y),
            glam::Vec2::new(cx.input.rect.width(), cx.input.rect.height()),
        );
        crate::camera::ray_plane(o, d, 0.0).map(|h| [h.x, h.z])
    }

    fn cursor_world(&self, cx: &Cx) -> Option<P2> {
        Self::world_at(cx, cx.input.hover?)
    }

    /// Viewport input, overlays and labels for one frame.
    pub fn update(&mut self, cx: &mut Cx) {
        let hs = cx.s.camera.top_half_height;
        let aspect = cx.input.rect.width() / cx.input.rect.height().max(1.0);
        let c = cx.s.camera.top_center;
        let depth = grid_lines([c.x, c.y], hs * aspect * 1.1, hs * 1.1, grid_step(hs));
        let mut top: Vec<LineVertex> = Vec::new();
        let radius = snap_radius(cx);
        let cursor = self.cursor_world(cx);

        if self
            .selected
            .as_ref()
            .is_some_and(|id| !cx.s.project().shapes.iter().any(|s| s.id() == id))
        {
            self.selected = None;
        }
        match self.tool {
            Tool::Select => self.editing(cx, cursor, radius, &mut top),
            Tool::Window | Tool::Door => self.placing(cx, cursor, radius, &mut top),
            Tool::Furniture => self.furnishing(cx, cursor, radius, &mut top),
            Tool::Plan => self.plan.update(cx, cursor, radius, &mut top),
            _ => self.drawing(cx, cursor, radius, &mut top),
        }
        if cx.input.escape && self.tool != Tool::Select && self.points.is_empty() {
            self.tool = Tool::Select;
            self.selected = None;
            self.vertex = None;
        }
        cx.s.extra = (depth, top);
        self.scale_bar(cx);
    }

    fn drawing(&mut self, cx: &mut Cx, cursor: Option<P2>, radius: f32, top: &mut Vec<LineVertex>) {
        let closed_shape = !matches!(
            self.tool,
            Tool::Wall | Tool::Fence | Tool::Stairs | Tool::Circle
        );
        let existing = all_vertices(cx, None);
        let mut snap =
            cursor.map(|c| snap_point(c, &self.points, &existing, radius, !cx.input.alt));
        // A flight that turns: from the second segment on, the pointer is held at a right angle
        // to the segment before; the point after the first corner may be a U's second corner, so
        // it keeps one width from it (`stairs.add` refuses flights that would overlap).
        if self.tool == Tool::Stairs
            && self.stairs_turn
            && let (Some(s), [.., a, b]) = (snap.as_mut(), &self.points[..])
        {
            let min = if self.points.len() == 2 {
                STAIRS_WIDTH_M
            } else {
                0.0
            };
            s.pos = right_angle_point(*a, *b, s.pos, min);
            s.kind = SnapKind::RightAngle;
        }
        if cx.input.escape && !self.points.is_empty() {
            self.points.clear();
            self.typed.clear();
            self.typed_width = None;
        }
        // Typed sizes: digits set the length of the segment being drawn (towards the pointer);
        // with the first corner down, `20` Tab (or x) `30` Enter makes a 20 by 30 m rectangle.
        // Tab moves egui's focus to a button, which would swallow the digits and Enter after it:
        // while drawing, only a text field keeps the keyboard.
        if !self.points.is_empty()
            && !cx.ctx.text_edit_focused()
            && let Some(id) = cx.ctx.memory(|m| m.focused())
        {
            cx.ctx.memory_mut(|m| m.surrender_focus(id));
        }
        let typing_field = cx.ctx.text_edit_focused();
        let (text, tab) = cx.ctx.input(|i| {
            let mut text = String::new();
            let mut tab = false;
            for e in &i.events {
                match e {
                    egui::Event::Text(t) => text.push_str(t),
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        pressed: true,
                        ..
                    } => tab = true,
                    _ => {}
                }
            }
            (text, tab)
        });
        if !typing_field && !self.points.is_empty() {
            for c in text.chars() {
                match c {
                    '0'..='9' | '.' => self.typed.push(c),
                    ',' => self.typed.push('.'),
                    'x' | 'X' | '*' if closed_shape && self.points.len() == 1 => {
                        self.typed_width = self.typed.parse().ok();
                        self.typed.clear();
                    }
                    _ => {}
                }
            }
            if tab && closed_shape && self.points.len() == 1 {
                self.typed_width = self.typed.parse().ok();
                self.typed.clear();
            }
        }
        if cx.input.backspace && self.typed.pop().is_none() {
            self.points.pop();
        }
        let typed_len: Option<f32> = self.typed.parse().ok().filter(|v: &f32| *v > 0.0);
        if cx.input.enter
            && let (Some(len), Some(&last), Some(s)) = (typed_len, self.points.last(), snap)
        {
            match self.typed_width {
                Some(w) if closed_shape && self.points.len() == 1 => {
                    let corners = rectangle(last, s.pos, w, len);
                    self.points = corners.to_vec();
                    self.typed.clear();
                    self.typed_width = None;
                    self.finish(cx);
                    return;
                }
                _ => {
                    self.points.push(typed_point(last, s.pos, len));
                    self.typed.clear();
                }
            }
        }
        if cx.input.clicked
            && let Some(s) = snap
        {
            if closed_shape && closes(&self.points, s.pos, radius) {
                self.finish(cx);
            } else if self.points.last() != Some(&s.pos) {
                self.points.push(s.pos);
            }
        }
        // Steps are two clicks: the bottom, then the top; a circle too: the centre, then the edge.
        // A flight that turns takes up to four, and Enter ends it at three.
        let turning = self.tool == Tool::Stairs && self.stairs_turn;
        let clicks = if turning { 4 } else { 2 };
        if matches!(self.tool, Tool::Stairs | Tool::Circle) && self.points.len() >= clicks {
            self.finish(cx);
            return;
        }
        if cx.input.enter && typed_len.is_none() {
            let needed = if turning || closed_shape { 3 } else { 2 };
            if self.points.len() >= needed {
                self.finish(cx);
            }
        }
        // Preview: the typed size where one is typed, else the pointer.
        let mut pts = self.points.clone();
        let mut rect = false;
        let typed_len: Option<f32> = self.typed.parse().ok().filter(|v: &f32| *v > 0.0);
        if let (Some(&last), Some(s)) = (self.points.last(), snap) {
            match (self.typed_width, typed_len) {
                (Some(w), depth) if closed_shape && self.points.len() == 1 => {
                    // Width typed: the rectangle, its depth typed or up to the pointer.
                    let d = depth.unwrap_or((s.pos[1] - last[1]).abs().max(0.1));
                    pts = rectangle(last, s.pos, w, d).to_vec();
                    rect = true;
                }
                (_, Some(len)) => pts.push(typed_point(last, s.pos, len)),
                _ if last != s.pos => pts.push(s.pos),
                _ => {}
            }
        } else if let Some(s) = snap {
            pts.push(s.pos);
        }
        let y = 0.05;
        top.extend(outline_lines(&pts, rect, y, ACCENT, 3.0));
        if self.tool == Tool::Circle
            && let [c, e, ..] = pts[..]
        {
            top.extend(outline_lines(
                &circle_polygon(c, dist(c, e)),
                true,
                y,
                ACCENT,
                3.0,
            ));
        }
        if closed_shape
            && self.points.len() >= 2
            && let Some(&last) = pts.last()
        {
            top.extend(line(
                y_point(last, y),
                y_point(self.points[0], y),
                [255, 200, 70, 90],
                1.5,
            ));
        }
        for (i, p) in self.points.iter().enumerate() {
            let green = closed_shape && i == 0 && self.points.len() >= 3;
            top.extend(dot(
                *p,
                y,
                if green { [120, 230, 140, 255] } else { ACCENT },
                if green { 12.0 } else { 8.0 },
            ));
        }
        if let Some(s) = snap {
            let col = match s.kind {
                SnapKind::Vertex => [120, 230, 140, 255],
                SnapKind::RightAngle => [120, 190, 255, 255],
                _ => [255, 255, 255, 220],
            };
            top.extend(dot(s.pos, y, col, 6.0));
        }
        for w in pts.windows(2) {
            self.length_label(cx, w[0], w[1], centroid(&pts));
        }
        if (!self.typed.is_empty() || self.typed_width.is_some())
            && let Some(at) = cx.input.hover
        {
            let text = match self.typed_width {
                Some(w) => format!("{w} × {} m", self.typed),
                None => format!("{} m", self.typed),
            };
            cx.label(at + egui::vec2(24.0, -18.0), &text);
        }
    }

    /// The edge's length at its middle, pushed outwards from the shape's centre on screen so the
    /// labels of a narrow shape's two long sides do not sit on each other.
    fn length_label(&self, cx: &Cx, a: P2, b: P2, centre: P2) {
        let len = dist(a, b);
        if len < 0.05 {
            return;
        }
        self.label_at(
            cx,
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
            &format!("{len:.2} m"),
            centre,
        );
    }

    /// `text` at `at`, pushed outwards from `centre` like [`Self::length_label`].
    fn label_at(&self, cx: &Cx, at: P2, text: &str, centre: P2) {
        let mid = y_point(at, 0.05);
        let (Some(p), c) = (cx.project(mid), cx.project(y_point(centre, 0.05))) else {
            return;
        };
        let out = c.map_or(egui::Vec2::ZERO, |c| (p - c).normalized());
        let out = if out.is_finite() {
            out
        } else {
            egui::Vec2::ZERO
        };
        cx.label(p + out * 22.0, text);
    }

    fn finish(&mut self, cx: &mut Cx) {
        let points = std::mem::take(&mut self.points);
        let name = |base: &str| next_name(cx.s.project(), base);
        let (cmd, material, params) = match self.tool {
            Tool::Area => {
                let (label, material) = self.area_kind.spec();
                (
                    "shape.add_area",
                    material,
                    json!({"name": name(label), "outline": points}),
                )
            }
            // A new block is a house: two storeys, plaster walls and a gable roof.
            Tool::Block => (
                "shape.add_extrusion",
                "plaster",
                json!({"name": name(t::TOOL_BLOCK), "outline": points, "closed": true, "height": 6.0, "roof": {"kind": "gable"}}),
            ),
            Tool::Wall => (
                "shape.add_extrusion",
                "timber",
                json!({"name": name(t::TOOL_WALL), "outline": points, "closed": false, "height": 1.8, "thickness": 0.1}),
            ),
            // A fence starts as slats with timber posts and boards (the command assigns them).
            Tool::Fence => (
                "fence.add",
                "",
                json!({"name": name(t::TOOL_FENCE), "line": points, "height": 1.8, "kind": "slats"}),
            ),
            // Steps 1.2 m wide and half a metre high to start; the panel changes both (the rise
            // can be left out of the command, to read it off the ground).
            Tool::Stairs if self.stairs_turn && points.len() >= 3 => (
                "stairs.add",
                "",
                json!({"name": name(t::TOOL_STAIRS), "from": points[0], "via": &points[1..points.len() - 1], "to": points[points.len() - 1], "width_m": STAIRS_WIDTH_M, "height": 1.5}),
            ),
            Tool::Stairs => (
                "stairs.add",
                "",
                json!({"name": name(t::TOOL_STAIRS), "from": points[0], "to": points[1], "width_m": STAIRS_WIDTH_M, "height": 0.5}),
            ),
            // A round area: the centre and the radius (an exact circle of four arcs).
            Tool::Circle => {
                let (label, material) = self.area_kind.spec();
                let radius = dist(points[0], points[1]);
                if radius < 0.05 {
                    return;
                }
                (
                    "shape.add_circle",
                    material,
                    json!({"name": name(label), "center": points[0], "radius_m": radius, "kind": "area"}),
                )
            }
            // A raised bed starts 40 cm high with timber walls and soil (the command does both).
            Tool::Bed => (
                "bed.add",
                "",
                json!({"name": name(t::TOOL_BED), "outline": points}),
            ),
            // A level cut into the slope: the floor at the mean of the ground under it, or at a
            // height, with a wall in the chosen style.
            Tool::Terrace => {
                let f = &self.terrace;
                let mut p = json!({
                    "name": name(t::TOOL_TERRACE), "outline": points,
                    "style": WALL_STYLES[f.style.min(WALL_STYLES.len() - 1)].0,
                    "parapet_m": f.parapet,
                });
                if f.mean {
                    p["floor_from"] = json!("mean");
                } else {
                    p["floor"] = json!(f.floor);
                }
                ("terrace.add", "paving", p)
            }
            Tool::Select | Tool::Window | Tool::Door | Tool::Plan | Tool::Furniture => return,
        };
        let mut params = params;
        if !material.is_empty() {
            params["material"] = json!(material); // one command: one undo step
        }
        let block = self.tool == Tool::Block;
        if let Some(r) = cx.run(cmd, params) {
            let id = r["id"].as_str().unwrap_or_default().to_string();
            for w in r["warnings"].as_array().into_iter().flatten() {
                if let Some(w) = w.as_str() {
                    cx.toasts.warn(w);
                }
            }
            if block {
                roof_tiles(cx, &id);
            }
            // The tool stays: Area after Area until Escape (the new shape is selected, so its
            // panel shows its sizes).
            self.selected = Some(id);
            self.vertex = None;
        }
    }

    /// Window and Door: the wall of a block nearest the pointer shows where the opening goes; a
    /// click puts it there (`shape.add_opening`).
    fn placing(&mut self, cx: &mut Cx, cursor: Option<P2>, radius: f32, top: &mut Vec<LineVertex>) {
        let Some(c) = cursor else {
            return;
        };
        let door = self.tool == Tool::Door;
        let (w, h, sill) = if door {
            (self.opening.door.0, self.opening.door.1, 0.0)
        } else {
            self.opening.window
        };
        let Some((id, side, offset, (p, d))) = nearest_wall(cx, c, radius * 2.0, w) else {
            return;
        };
        let at = |u: f32| [p[0] + d[0] * u, p[1] + d[1] * u];
        top.extend(line(
            y_point(at(offset - w / 2.0), 0.08),
            y_point(at(offset + w / 2.0), 0.08),
            if door { [120, 190, 255, 255] } else { ACCENT },
            6.0,
        ));
        if cx.input.clicked {
            cx.run(
                "shape.add_opening",
                json!({"id": id, "side": side, "offset_m": offset, "kind": if door { "door" } else { "window" },
                       "width_m": w, "height_m": h, "sill_m": sill, "frame": self.opening.frame}),
            );
            self.selected = Some(id);
        }
    }

    /// The Furniture tool: the footprint of what a click would place follows the pointer; the
    /// click runs `furniture.add` or `furniture.add_set`.
    fn furnishing(
        &mut self,
        cx: &mut Cx,
        cursor: Option<P2>,
        radius: f32,
        top: &mut Vec<LineVertex>,
    ) {
        use visioncraft_core::furniture::{Furniture, SETS, rect_outline, set_pieces};
        let Some(c) = cursor else {
            return;
        };
        let existing = all_vertices(cx, None);
        let at = snap_point(c, &[], &existing, radius, !cx.input.alt).pos;
        let choices = furniture_choices();
        let form = &self.furniture;
        let (_, kind) = &choices[form.choice.min(choices.len() - 1)];
        let turn = form.rotation_deg;
        let sized = |default: [f32; 3]| {
            let pick = |v: f32, d: f32| if v > 0.0 { v } else { d };
            [pick(form.length, default[0]), pick(form.depth, default[1])]
        };
        let mut footprints: Vec<(Vec<P2>, P2)> = Vec::new();
        let set = kind
            .is_none()
            .then(|| SETS[form.choice - (choices.len() - SETS.len())]);
        match (kind, set) {
            (Some(k), _) => {
                let [l, d] = sized(Furniture::new(*k).default_size(false));
                footprints.push((rect_outline(at, turn, l, d), at));
            }
            (None, Some(set)) => {
                let t = turn.to_radians();
                let (ex, ez) = ([t.cos(), -t.sin()], [t.sin(), t.cos()]);
                for p in set_pieces(set, form.seats).unwrap_or_default() {
                    let pos = [
                        at[0] + ex[0] * p.position[0] + ez[0] * p.position[1],
                        at[1] + ex[1] * p.position[0] + ez[1] * p.position[1],
                    ];
                    footprints.push((
                        rect_outline(pos, turn + p.rotation_deg, p.size[0], p.size[1]),
                        pos,
                    ));
                }
            }
            (None, None) => {}
        }
        for (outline, centre) in &footprints {
            top.extend(outline_lines(outline, true, 0.05, ACCENT, 2.5));
            // A tick from the middle to the front edge shows which way the piece faces.
            let front = [
                (outline[2][0] + outline[3][0]) / 2.0,
                (outline[2][1] + outline[3][1]) / 2.0,
            ];
            top.extend(line(
                y_point(*centre, 0.05),
                y_point(front, 0.05),
                ACCENT,
                1.5,
            ));
        }
        top.extend(dot(at, 0.05, [255, 255, 255, 220], 6.0));
        if !cx.input.clicked {
            return;
        }
        let name = |base: &str| next_name(cx.s.project(), base);
        let result = match (kind, set) {
            (Some(k), _) => {
                let mut params = json!({
                    "kind": k.name(), "position": at, "rotation_deg": turn,
                    "name": name(&choices[form.choice].0),
                });
                for (key, v) in [
                    ("length", form.length),
                    ("depth", form.depth),
                    ("height", form.height),
                ] {
                    if v > 0.0 {
                        params[key] = json!(v);
                    }
                }
                cx.run("furniture.add", params)
            }
            (None, Some(set)) => cx.run(
                "furniture.add_set",
                json!({"set": set, "position": at, "rotation_deg": turn, "seats": form.seats}),
            ),
            (None, None) => None,
        };
        if let Some(r) = result {
            let id = r["id"]
                .as_str()
                .or_else(|| r["ids"][0].as_str())
                .unwrap_or_default()
                .to_string();
            self.selected = Some(id);
            self.vertex = None;
        }
    }

    /// The piece or set the Furniture tool places, its sizes and its turn.
    fn terrace_form_panel(&mut self, ui: &mut egui::Ui) {
        let f = &mut self.terrace;
        f.style = f.style.min(WALL_STYLES.len() - 1);
        ui.checkbox(&mut f.mean, t::TERRACE_FLOOR_MEAN);
        ui.add_enabled_ui(!f.mean, |ui| {
            drag_field(
                ui,
                t::TERRACE_FLOOR,
                &mut f.floor,
                0.05,
                -100.0..=1000.0,
                " m",
            );
        });
        ui.horizontal(|ui| {
            ui.label(t::TERRACE_WALL);
            egui::ComboBox::from_id_salt("terrace-style")
                .selected_text(WALL_STYLES[f.style].1)
                .show_ui(ui, |ui| {
                    for (i, (_, label)) in WALL_STYLES.iter().enumerate() {
                        ui.selectable_value(&mut f.style, i, *label);
                    }
                });
        });
        drag_field(
            ui,
            t::TERRACE_PARAPET,
            &mut f.parapet,
            0.01,
            0.0..=1.0,
            " m",
        );
    }

    fn furniture_form_panel(&mut self, ui: &mut egui::Ui) {
        let choices = furniture_choices();
        let f = &mut self.furniture;
        f.choice = f.choice.min(choices.len() - 1);
        ui.horizontal(|ui| {
            ui.label(t::FURNITURE_PIECE);
            egui::ComboBox::from_id_salt("furniture-choice")
                .selected_text(&choices[f.choice].0)
                .show_ui(ui, |ui| {
                    for (i, (label, _)) in choices.iter().enumerate() {
                        ui.selectable_value(&mut f.choice, i, label);
                    }
                });
        });
        if choices[f.choice].1.is_some() {
            drag_field(
                ui,
                t::FURNITURE_LENGTH,
                &mut f.length,
                0.01,
                0.0..=8.0,
                " m",
            );
            drag_field(ui, t::FURNITURE_DEPTH, &mut f.depth, 0.01, 0.0..=8.0, " m");
            drag_field(ui, t::HEIGHT, &mut f.height, 0.01, 0.0..=4.0, " m");
        } else if f.choice == choices.len() - 3 {
            let mut seats = f.seats as f32;
            drag_field(ui, t::FURNITURE_SEATS, &mut seats, 0.1, 2.0..=12.0, "");
            f.seats = seats.round() as usize;
        }
        drag_field(
            ui,
            t::FURNITURE_TURN,
            &mut f.rotation_deg,
            1.0,
            -360.0..=360.0,
            "°",
        );
    }

    fn editing(&mut self, cx: &mut Cx, cursor: Option<P2>, radius: f32, top: &mut Vec<LineVertex>) {
        let shape = self.shape(cx);
        if let (Some(shape), Some(c)) = (&shape, cursor) {
            let points = shape.points();
            if cx.input.drag_started {
                // egui reports the start once the pointer has already moved: test where it was pressed.
                let pressed = cx
                    .input
                    .press_origin
                    .and_then(|p| Self::world_at(cx, p))
                    .unwrap_or(c);
                self.dragging = vertex_hit(points, pressed, radius);
                if self.dragging.is_some() {
                    self.vertex = self.dragging;
                }
            }
            if let Some(i) = self.dragging {
                if cx.input.dragging {
                    let others = all_vertices(cx, Some(shape.id()));
                    let s = snap_point(c, &[], &others, radius, !cx.input.alt);
                    let base =
                        cx.s.project()
                            .shapes
                            .iter()
                            .find(|x| x.id() == shape.id())
                            .cloned();
                    if let Some(base) = base {
                        cx.s.set_draft(Some(crate::studio::Draft::Shape(with_vertex(
                            &base, i, s.pos,
                        ))));
                    }
                }
                if cx.input.drag_stopped {
                    self.dragging = None;
                    let to = cx.s.draft.as_ref().and_then(|d| match d {
                        crate::studio::Draft::Shape(s) => s.points().get(i).copied(),
                        _ => None,
                    });
                    cx.s.set_draft(None);
                    if let Some(p) = to {
                        cx.run(
                            "shape.move_vertex",
                            json!({"id": shape.id(), "index": i, "position": p}),
                        );
                    }
                }
            }
        }
        if cx.input.clicked
            && let Some(c) = cursor
        {
            let hit_vertex = shape
                .as_ref()
                .and_then(|s| vertex_hit(s.points(), c, radius));
            // The edge under the pointer, along the curve: its control edge and the point on it.
            let hit_edge = shape.as_ref().and_then(|s| {
                let tess = s.tessellate();
                edge_hit(&tess.pts, s.is_closed(), c, radius * 0.7)
                    .map(|(j, q)| (tess.edge_of[j], q))
            });
            if let Some(i) = hit_vertex {
                self.vertex = Some(i);
            } else if let (Some((edge, _)), true) = (hit_edge, cx.input.shift) {
                // Shift-click picks the edge for the curve fields instead of adding a corner.
                self.edge = Some(edge);
            } else if let (Some((edge, q)), Some(s)) = (hit_edge, &shape) {
                let q = if cx.input.alt {
                    q
                } else {
                    [round(q[0], GRID), round(q[1], GRID)]
                };
                cx.run(
                    "shape.insert_vertex",
                    json!({"id": s.id(), "edge": edge, "position": q}),
                );
                self.vertex = Some(edge + 1);
            } else {
                let pos = cx.input.hover.expect("clicked implies hover");
                let pick = cx.s.pick(
                    glam::Vec2::new(pos.x, pos.y),
                    glam::Vec2::new(cx.input.rect.width(), cx.input.rect.height()),
                    true,
                );
                self.selected = pick.map(|p| p.owner);
                self.vertex = None;
                self.edge = None;
            }
        }
        if (cx.input.delete || cx.input.backspace)
            && self.dragging.is_none()
            && let (Some(id), Some(i)) = (self.selected.clone(), self.vertex.take())
        {
            cx.run("shape.remove_vertex", json!({"id": id, "index": i}));
        }
        if cx.input.escape {
            self.selected = None;
            self.vertex = None;
        }
        // Outline of the selection.
        if let Some(shape) = self.shape(cx) {
            let y = match &shape {
                Shape::Area(a) => a.elevation + 0.06,
                Shape::Extrusion(e) => e.base + e.height + 0.06,
            };
            // The curved outline; the handles stay on the control points.
            let tess = shape.tessellate();
            let runs = tess.runs();
            top.extend(outline_lines(
                &tess.pts,
                shape.is_closed(),
                y,
                [90, 200, 255, 255],
                3.0,
            ));
            if let Some(run) = self.edge.and_then(|e| runs.get(e)) {
                top.extend(outline_lines(run, false, y + 0.01, ACCENT, 5.0));
            }
            for (i, p) in shape.points().iter().enumerate() {
                let sel = self.vertex == Some(i);
                top.extend(dot(
                    *p,
                    y,
                    if sel { ACCENT } else { [255, 255, 255, 255] },
                    if sel { 13.0 } else { 9.0 },
                ));
            }
            let centre = centroid(&tess.pts);
            for (i, run) in runs.iter().enumerate() {
                let (Some(&a), Some(&b)) = (run.first(), run.last()) else {
                    continue;
                };
                if run.len() <= 2 {
                    self.length_label(cx, a, b, centre);
                } else {
                    // A curve: its length along the curve, and the radius of an arc.
                    let len = outline_length(run, false);
                    let mut text = format!("{len:.2} m");
                    if let Some(c) = shape.curves().get(&i) {
                        text.push_str(&format!(" R {:.2}", c.radius_m(dist(a, b))));
                    }
                    self.label_at(cx, run[run.len() / 2], &text, centre);
                }
            }
        }
    }

    fn scale_bar(&self, cx: &Cx) {
        let h = cx.input.rect.height();
        let per_m = h / (2.0 * cx.s.camera.top_half_height);
        let len = nice_length((cx.input.rect.width() * 0.25 / per_m).max(0.5));
        let w = len * per_m;
        let p = cx.input.rect.right_bottom() - EVec2::new(24.0 + w, 28.0);
        let painter = cx.ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("scalebar"),
        ));
        let stroke = egui::Stroke::new(2.0, Color32::WHITE);
        let shadow = egui::Stroke::new(4.0, Color32::from_black_alpha(140));
        for s in [shadow, stroke] {
            painter.line_segment([p, p + EVec2::new(w, 0.0)], s);
            painter.line_segment([p + EVec2::new(0.0, -5.0), p + EVec2::new(0.0, 5.0)], s);
            painter.line_segment([p + EVec2::new(w, -5.0), p + EVec2::new(w, 5.0)], s);
        }
        cx.label(Pos2::new(p.x + w / 2.0, p.y - 14.0), &format!("{len} m"));
    }

    /// The selected shape's fields above the tools: the panel grows upwards from the bottom, so
    /// the tool row stays where it is when a shape is selected.
    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        if self.shape_panel(ui, cx) {
            ui.separator();
        }
        // Above the tools, so they stay put.
        if matches!(self.tool, Tool::Area | Tool::Circle) {
            ui.horizontal_wrapped(|ui| {
                for kind in AreaKind::ALL {
                    if ui
                        .selectable_label(self.area_kind == kind, kind.spec().0)
                        .clicked()
                    {
                        self.area_kind = kind;
                    }
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            for (tool, label) in [
                (Tool::Select, t::TOOL_SELECT),
                (Tool::Area, t::TOOL_AREA),
                (Tool::Block, t::TOOL_BLOCK),
                (Tool::Wall, t::TOOL_WALL),
                (Tool::Fence, t::TOOL_FENCE),
                (Tool::Stairs, t::TOOL_STAIRS),
                (Tool::Bed, t::TOOL_BED),
                (Tool::Terrace, t::TOOL_TERRACE),
                (Tool::Furniture, t::TOOL_FURNITURE),
                (Tool::Circle, t::TOOL_CIRCLE),
                (Tool::Window, t::TOOL_WINDOW),
                (Tool::Door, t::TOOL_DOOR),
                (Tool::Plan, t::TOOL_PLAN),
            ] {
                if ui.selectable_label(self.tool == tool, label).clicked() {
                    if self.tool == Tool::Plan && tool != Tool::Plan {
                        self.plan.cancel(cx);
                    }
                    self.tool = tool;
                    self.points.clear();
                }
            }
        });
        if self.tool == Tool::Plan {
            self.plan.panel(ui, cx);
        }
        if matches!(self.tool, Tool::Window | Tool::Door) {
            self.opening_panel(ui);
        }
        if self.tool == Tool::Furniture {
            self.furniture_form_panel(ui);
        }
        if self.tool == Tool::Terrace {
            self.terrace_form_panel(ui);
        }
        if self.tool == Tool::Stairs && ui.checkbox(&mut self.stairs_turn, t::STAIRS_TURN).changed()
        {
            self.points.clear();
        }
        ui.small(match self.tool {
            Tool::Stairs if self.stairs_turn => t::STAIRS_TURN_HINT,
            Tool::Select => t::SELECT_HINT,
            Tool::Wall | Tool::Fence => t::WALL_HINT,
            Tool::Stairs => t::STAIRS_HINT,
            Tool::Circle => t::CIRCLE_HINT,
            Tool::Furniture => t::FURNITURE_HINT,
            Tool::Terrace => t::TERRACE_HINT,
            Tool::Window | Tool::Door => t::OPENING_HINT,
            Tool::Plan => "",
            _ => t::DRAW_HINT,
        });
    }

    /// Curves: pick an edge, type a radius, curve it to the left or the right of its direction,
    /// straighten it; and the smooth outline. Each change one command. Stairs and houses are
    /// straight.
    fn curve_panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx, shape: &Shape) {
        if matches!(shape, Shape::Extrusion(x) if x.stairs.is_some() || x.house.is_some() || x.furniture.is_some())
        {
            return;
        }
        let points = shape.points();
        let edges = visioncraft_core::curve::edge_count(points.len(), shape.is_closed());
        if edges == 0 {
            return;
        }
        let edge = self.edge.unwrap_or(0).min(edges - 1);
        let mut picked = edge;
        ui.horizontal(|ui| {
            ui.label(t::CURVE_EDGE);
            ui.add(egui::DragValue::new(&mut picked).range(0..=edges - 1));
            ui.weak(format!("/ {}", edges - 1));
        });
        self.edge = Some(picked);
        let edge = picked;
        let chord = dist(points[edge], points[(edge + 1) % points.len()]);
        let now = shape.curves().get(&edge).copied();
        ui.label(match now {
            Some(c) => format!(
                "{:.2} m, {} {:.2} m",
                chord,
                t::CURVE_RADIUS.to_lowercase(),
                c.radius_m(chord)
            ),
            None => format!("{chord:.2} m, {}", t::CURVE_STRAIGHT),
        });
        let mut radius = if self.curve_radius > 0.0 {
            self.curve_radius
        } else {
            now.map_or(chord, |c| c.radius_m(chord)).max(chord / 2.0)
        };
        if drag_field(
            ui,
            t::CURVE_RADIUS,
            &mut radius,
            0.05,
            0.01..=10_000.0,
            " m",
        ) != Edit::None
        {
            self.curve_radius = radius;
        }
        ui.horizontal_wrapped(|ui| {
            for (side, label) in [("left", t::CURVE_LEFT), ("right", t::CURVE_RIGHT)] {
                if ui.button(label).clicked() {
                    cx.run(
                        "shape.set_curve",
                        json!({"id": shape.id(), "edge": edge, "radius_m": radius, "side": side}),
                    );
                }
            }
            if now.is_some() && ui.button(t::CURVE_CLEAR).clicked() {
                cx.run("shape.clear_curve", json!({"id": shape.id(), "edge": edge}));
            }
        });
        let mut smooth = shape.is_smooth();
        if ui
            .checkbox(&mut smooth, t::SMOOTH)
            .on_hover_text(t::SMOOTH_TIP)
            .changed()
        {
            cx.run(
                "shape.set_smooth",
                json!({"id": shape.id(), "smooth": smooth}),
            );
        }
        ui.weak(t::CURVE_HINT);
    }

    /// Sizes and frame of the windows and doors about to be placed.
    fn opening_panel(&mut self, ui: &mut egui::Ui) {
        let o = &mut self.opening;
        if self.tool == Tool::Door {
            drag_field(ui, t::WIDTH, &mut o.door.0, 0.01, 0.5..=4.0, " m");
            drag_field(ui, t::HEIGHT, &mut o.door.1, 0.01, 1.5..=4.0, " m");
        } else {
            drag_field(ui, t::WIDTH, &mut o.window.0, 0.01, 0.3..=8.0, " m");
            drag_field(ui, t::HEIGHT, &mut o.window.1, 0.01, 0.3..=4.0, " m");
            drag_field(ui, t::SILL, &mut o.window.2, 0.01, 0.0..=20.0, " m");
        }
        ui.horizontal(|ui| {
            ui.label(t::FRAME);
            for (key, label) in [
                ("slim", t::FRAME_SLIM),
                ("wide", t::FRAME_WIDE),
                ("split", t::FRAME_SPLIT),
                ("none", t::FRAME_NONE),
            ] {
                if ui.selectable_label(o.frame == key, label).clicked() {
                    o.frame = key;
                }
            }
        });
    }

    /// The selected shape's name, sizes and measurements; false when nothing is selected.
    fn shape_panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) -> bool {
        let Some(shape) = self.shape(cx) else {
            return false;
        };
        // The name being typed lives here until the field lets go (the shape keeps the old one).
        let mut name = match &self.name_edit {
            Some((id, n)) if id == shape.id() => n.clone(),
            _ => shape.name().to_string(),
        };
        ui.horizontal(|ui| {
            ui.label(t::NAME);
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
            if r.lost_focus() {
                self.name_edit = None;
                if name != shape.name() && !name.trim().is_empty() {
                    cx.run("shape.rename", json!({"id": shape.id(), "name": name}));
                }
            } else if r.has_focus() {
                self.name_edit = Some((shape.id().to_string(), name));
            }
        });
        // A rectangle (at any turn) shows its width and depth to type in.
        if shape.is_closed()
            && !shape.is_curved()
            && let Some((w0, d0)) = rect_size(shape.points())
        {
            let (mut w, mut d) = (w0, d0);
            let ew = drag_field(ui, t::WIDTH, &mut w, 0.01, 0.1..=1000.0, " m");
            let ed = drag_field(ui, t::DEPTH, &mut d, 0.01, 0.1..=1000.0, " m");
            if (ew == Edit::Done || ed == Edit::Done)
                && let Some(outline) = resized_rect(shape.points(), w, d)
            {
                cx.run(
                    "shape.set_outline",
                    json!({"id": shape.id(), "outline": outline}),
                );
            }
        }
        let mut edited = shape.clone();
        let mut edit = Edit::None;
        let mut params = serde_json::Map::new();
        match &mut edited {
            Shape::Area(a) => {
                let mut drape = a.drape;
                if ui
                    .checkbox(&mut drape, t::FOLLOW_GROUND)
                    .on_hover_text(t::FOLLOW_GROUND_TIP)
                    .changed()
                {
                    cx.run("shape.set_height", json!({"id": a.id, "drape": drape}));
                }
                let e = drag_field(ui, t::ELEVATION, &mut a.elevation, 0.05, -50.0..=50.0, " m");
                if e != Edit::None {
                    params.insert("elevation".into(), json!(a.elevation));
                }
                edit = e;
                if a.terrace.is_some() {
                    terrace_panel(ui, cx, a);
                } else {
                    edge_panel(ui, cx, a);
                    if !a.drape && a.edge.is_none() && ui.button(t::TERRACE_MAKE).clicked() {
                        cx.run("terrace.set", json!({"id": a.id, "style": "plain"}));
                    }
                }
            }
            Shape::Extrusion(x) if x.closed && x.raised_bed.is_some() => bed_panel(ui, cx, x),
            Shape::Extrusion(x) if x.closed && x.furniture.is_some() => {
                furniture_panel(ui, cx, x, shape.is_curved());
            }
            Shape::Extrusion(x) => {
                for (label, v, key, range) in [
                    (t::HEIGHT, &mut x.height, "height", 0.05..=100.0),
                    (t::BASE, &mut x.base, "base", -50.0..=100.0),
                ] {
                    let e = drag_field(ui, label, v, 0.05, range, " m");
                    if e != Edit::None {
                        params.insert(key.into(), json!(*v));
                        edit = e;
                    }
                }
                if x.closed && x.stairs.is_some() {
                    stairs_panel(ui, cx, x);
                } else if x.closed {
                    // Storeys of 3 m: a house is two clicks, not a typed number.
                    ui.horizontal(|ui| {
                        ui.label(t::STOREYS);
                        for n in 1..=3u8 {
                            let h = 3.0 * f32::from(n);
                            if ui
                                .selectable_label((x.height - h).abs() < 0.01, n.to_string())
                                .clicked()
                            {
                                x.height = h;
                                params.insert("height".into(), json!(h));
                                edit = Edit::Done;
                            }
                        }
                    });
                }
                if x.closed && x.stairs.is_none() {
                    house_panel(ui, cx, x);
                    // A plain rectangular block (no roof yet) can be steps instead.
                    if x.house.is_none()
                        && x.points.len() == 4
                        && !shape.is_curved()
                        && ui.button(t::MAKE_STAIRS).clicked()
                    {
                        cx.run("stairs.set", json!({"id": x.id, "up_edge": 0}));
                    }
                }
                if !x.closed {
                    fence_panel(ui, cx, x);
                    let e = drag_field(ui, t::THICKNESS, &mut x.thickness, 0.01, 0.02..=5.0, " m");
                    if e != Edit::None {
                        params.insert("thickness".into(), json!(x.thickness));
                        edit = e;
                    }
                }
            }
        }
        match edit {
            Edit::Live => cx.s.set_draft(Some(crate::studio::Draft::Shape(edited))),
            Edit::Done => {
                cx.s.set_draft(None);
                params.insert("id".into(), json!(shape.id()));
                cx.run("shape.set_height", serde_json::Value::Object(params));
            }
            Edit::None => {}
        }
        self.curve_panel(ui, cx, &shape);
        let (closed, outline) = (shape.is_closed(), shape.outline());
        if closed {
            ui.label(format!(
                "{}: {:.2} m²",
                t::AREA,
                measure::polygon_area(&outline)
            ));
        }
        ui.label(format!(
            "{}: {:.2} m",
            if closed { t::PERIMETER } else { "Length" },
            outline_length(&outline, closed)
        ));
        ui.horizontal(|ui| {
            if self.vertex.is_some()
                && ui.button(t::DELETE_CORNER).clicked()
                && let Some(i) = self.vertex.take()
            {
                cx.run("shape.remove_vertex", json!({"id": shape.id(), "index": i}));
            }
            if ui.button(t::DELETE_SHAPE).clicked() {
                cx.run("shape.remove", json!({"id": shape.id()}));
                self.selected = None;
                self.vertex = None;
            }
        });
        true
    }
}

/// The point `len` metres from `last` towards `toward`, along an axis when the pointer is within
/// 10 degrees of one (so typed sizes come out exact).
pub fn typed_point(last: P2, toward: P2, len: f32) -> P2 {
    let (dx, dz) = (toward[0] - last[0], toward[1] - last[1]);
    let (mut ux, mut uz) = if dx.hypot(dz) < 1e-6 {
        (1.0, 0.0)
    } else {
        (dx / dx.hypot(dz), dz / dx.hypot(dz))
    };
    let snap = 10f32.to_radians().sin();
    if uz.abs() < snap {
        (ux, uz) = (ux.signum(), 0.0);
    } else if ux.abs() < snap {
        (ux, uz) = (0.0, uz.signum());
    }
    [last[0] + ux * len, last[1] + uz * len]
}

/// A `width` (along x) by `depth` (along z) rectangle from corner `p0`, opening towards `toward`.
pub fn rectangle(p0: P2, toward: P2, width: f32, depth: f32) -> [P2; 4] {
    let sx = if toward[0] < p0[0] { -1.0 } else { 1.0 };
    let sz = if toward[1] < p0[1] { -1.0 } else { 1.0 };
    let (x1, z1) = (p0[0] + sx * width, p0[1] + sz * depth);
    [p0, [x1, p0[1]], [x1, z1], [p0[0], z1]]
}

/// Width and depth of a four-corner shape with right angles (a rectangle at any turn), else
/// `None`.
pub fn rect_size(outline: &[P2]) -> Option<(f32, f32)> {
    let [a, b, c, d] = <[P2; 4]>::try_from(outline).ok()?;
    let e = |p: P2, q: P2| [q[0] - p[0], q[1] - p[1]];
    let (u, v, w, x) = (e(a, b), e(b, c), e(c, d), e(d, a));
    let dot = |p: [f32; 2], q: [f32; 2]| p[0] * q[0] + p[1] * q[1];
    let len = |p: [f32; 2]| p[0].hypot(p[1]);
    let square = |p, q| dot(p, q).abs() < 1e-3 * len(p) * len(q) + 1e-6;
    (square(u, v) && square(v, w) && square(w, x)).then(|| (len(u), len(v)))
}

/// The same rectangle with sides `width` (first edge) and `depth` (second), first corner and
/// directions kept.
pub fn resized_rect(outline: &[P2], width: f32, depth: f32) -> Option<Vec<P2>> {
    let (w0, d0) = rect_size(outline)?;
    let [a, b, _, d] = <[P2; 4]>::try_from(outline).ok()?;
    let u = [(b[0] - a[0]) / w0, (b[1] - a[1]) / w0];
    let v = [(d[0] - a[0]) / d0, (d[1] - a[1]) / d0];
    let p = |s: f32, t: f32| [a[0] + u[0] * s + v[0] * t, a[1] + u[1] * s + v[1] * t];
    Some(vec![a, p(width, 0.0), p(width, depth), p(0.0, depth)])
}

/// The wall of a block nearest `c` within `reach`: (block id, edge, offset of an opening of
/// width `w` centred at the pointer, kept inside the wall, (edge start, unit direction)).
/// Where an opening goes: (block id, wall index, offset along it, (wall start, direction)).
type WallSpot = (String, usize, f32, (P2, P2));

fn nearest_wall(cx: &Cx, c: P2, reach: f32, w: f32) -> Option<WallSpot> {
    let mut best: Option<(f32, WallSpot)> = None;
    for s in &cx.s.project().shapes {
        let Shape::Extrusion(e) = s else { continue };
        if !e.closed {
            continue;
        }
        let n = e.points.len();
        for i in 0..n {
            let (p, q) = (e.points[i], e.points[(i + 1) % n]);
            let len = dist(p, q);
            if len < w + 0.2 {
                continue;
            }
            let d = [(q[0] - p[0]) / len, (q[1] - p[1]) / len];
            let u = ((c[0] - p[0]) * d[0] + (c[1] - p[1]) * d[1]).clamp(0.0, len);
            let foot = [p[0] + d[0] * u, p[1] + d[1] * u];
            let gap = dist(foot, c);
            if gap <= reach && best.as_ref().is_none_or(|b| gap < b.0) {
                let offset = u.clamp(w / 2.0 + 0.1, len - w / 2.0 - 0.1);
                best = Some((gap, (e.id.clone(), i, offset, (p, d))));
            }
        }
    }
    best.map(|b| b.1)
}

/// A new block's roof in the library's roof tiles, when the library has them (else the
/// palette's dark roof stays).
fn roof_tiles(cx: &mut Cx, id: &str) {
    const TILES: &str = "roofingtiles006";
    if let Ok(material) = crate::material_library::ensure_material(cx.s, TILES) {
        cx.run(
            "material.assign",
            json!({"target": {"type": "face", "owner": id, "face": format!("{id}:roof")}, "material": material}),
        );
    }
}

/// A wall's fence: kind, gates, and the way back to a plain wall; each change one command.
fn fence_panel(ui: &mut egui::Ui, cx: &mut Cx, x: &visioncraft_core::shape::Extrusion) {
    use visioncraft_core::fence::FenceKind;
    let Some(f) = &x.fence else {
        if ui.button(t::MAKE_FENCE).clicked() {
            cx.run("fence.set", json!({"id": x.id, "kind": "slats"}));
        }
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(t::FENCE_KIND);
        for (kind, key, label) in [
            (FenceKind::Slats, "slats", t::FENCE_SLATS),
            (
                FenceKind::HorizontalSlats,
                "horizontal_slats",
                t::FENCE_HORIZONTAL,
            ),
            (FenceKind::Panels, "panels", t::FENCE_PANELS),
            (FenceKind::Picket, "picket", t::FENCE_PICKET),
            (FenceKind::Mesh, "mesh", t::FENCE_MESH),
            (FenceKind::Screen, "screen", t::FENCE_SCREEN),
        ] {
            if ui.selectable_label(f.kind == kind, label).clicked() && f.kind != kind {
                cx.run("fence.set", json!({"id": x.id, "kind": key}));
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        // The gate goes in the middle of the first stretch.
        if ui.button(t::ADD_GATE).clicked()
            && let [a, b, ..] = x.points[..]
        {
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            cx.run(
                "fence.add_gate",
                json!({"id": x.id, "offset_m": len / 2.0, "width_m": 1.0}),
            );
        }
        if !f.gates.is_empty() && ui.button(t::REMOVE_LAST_GATE).clicked() {
            cx.run(
                "fence.remove_gate",
                json!({"id": x.id, "index": f.gates.len() - 1}),
            );
        }
        if ui.button(t::BACK_TO_WALL).clicked() {
            cx.run("fence.clear", json!({"id": x.id}));
        }
    });
}

/// A flight's steps, riser and tread, and the way back to a plain block; each change one command.
fn stairs_panel(ui: &mut egui::Ui, cx: &mut Cx, x: &visioncraft_core::shape::Extrusion) {
    let Some(st) = &x.stairs else {
        return;
    };
    if let Ok(g) = st.geometry(&x.points, x.height) {
        ui.label(format!(
            "{} {} ({:.0} mm / {:.0} mm)",
            g.steps,
            t::STEPS,
            g.riser * 1000.0,
            g.tread * 1000.0
        ));
        // A flight that turns: its flights and landings.
        if let Some(turned) = &g.turned {
            for (i, f) in turned.flights.iter().enumerate() {
                ui.label(format!(
                    "{} {}: {} {} ({:.2} m)",
                    t::FLIGHT,
                    i + 1,
                    f.steps,
                    t::STEPS,
                    f.run_m
                ));
            }
            for l in &turned.landings {
                ui.label(format!(
                    "{}: {:.2} x {:.2} m, {:.2} m {}",
                    t::LANDING,
                    l.size_m[0],
                    l.size_m[1],
                    l.height_m,
                    t::UP
                ));
            }
        }
        let (mut riser, mut tread) = (st.riser_m.unwrap_or(g.riser), st.tread_m.unwrap_or(g.tread));
        if drag_field(ui, t::RISER, &mut riser, 0.005, 0.05..=0.3, " m") == Edit::Done {
            cx.run("stairs.set", json!({"id": x.id, "riser_m": riser}));
        }
        if drag_field(ui, t::TREAD, &mut tread, 0.005, 0.15..=0.6, " m") == Edit::Done {
            cx.run("stairs.set", json!({"id": x.id, "tread_m": tread}));
        }
    }
    if let Ok(m) = visioncraft_core::shape_mesh(&Shape::Extrusion(x.clone())) {
        for w in &m.warnings {
            ui.colored_label(egui::Color32::from_rgb(230, 170, 70), w);
        }
    }
    ui.horizontal_wrapped(|ui| {
        if st.tread_m.is_some() && ui.button(t::FIT_TREADS).clicked() {
            cx.run("stairs.set", json!({"id": x.id, "tread_m": null}));
        }
        if st.path.is_some() && ui.button(t::STRAIGHTEN_STAIRS).clicked() {
            cx.run("stairs.set", json!({"id": x.id, "via": null}));
        }
        if ui.button(t::BACK_TO_BLOCK).clicked() {
            cx.run("stairs.clear", json!({"id": x.id}));
        }
    });
}

/// A terrace's floor, wall style, coping and parapet, and what it moves and builds; each change
/// one command. The numbers are worked out again only when the terrace or the terrain changed.
fn terrace_panel(ui: &mut egui::Ui, cx: &mut Cx, a: &visioncraft_core::shape::Area) {
    use std::hash::{Hash, Hasher};
    let Some(terrace) = a.terrace else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(t::TERRACE_WALL);
        for (key, label) in WALL_STYLES {
            let now = serde_json::to_value(terrace.style).ok();
            let is = now.as_ref().and_then(|v| v.as_str()) == Some(key);
            if ui.selectable_label(is, label).clicked() && !is {
                cx.run("terrace.set", json!({"id": a.id, "style": key}));
            }
        }
    });
    let (mut floor, mut coping, mut parapet) = (a.elevation, terrace.coping_m, terrace.parapet_m);
    let ef = drag_field(
        ui,
        t::TERRACE_FLOOR,
        &mut floor,
        0.05,
        -100.0..=1000.0,
        " m",
    );
    let ec = drag_field(ui, t::TERRACE_COPING, &mut coping, 0.01, 0.0..=1.0, " m");
    let ep = drag_field(ui, t::TERRACE_PARAPET, &mut parapet, 0.01, 0.0..=1.0, " m");
    if ef == Edit::Done || ec == Edit::Done || ep == Edit::Done {
        cx.run(
            "terrace.set",
            json!({"id": a.id, "floor": floor, "coping_m": coping, "parapet_m": parapet}),
        );
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{a:?}").hash(&mut h);
    cx.s.project()
        .terrain
        .as_ref()
        .map(|t| std::sync::Arc::as_ptr(t) as usize)
        .hash(&mut h);
    let key = h.finish();
    let slot = egui::Id::new(("terrace-numbers", a.id.as_str()));
    let cached: Option<(u64, Vec<String>, Vec<String>)> = ui.data(|d| d.get_temp(slot));
    let (lines, warnings) = match cached {
        Some((k, lines, warnings)) if k == key => (lines, warnings),
        _ => {
            let p = cx.s.project();
            let (lines, warnings) = match visioncraft_core::terrace::report_for(p, a) {
                Some(r) => (
                    vec![
                        format!("{} {:.1} m3", t::TERRACE_CUT, r.cut_m3),
                        format!("{} {:.1} m3", t::TERRACE_FILL, r.fill_m3),
                        format!(
                            "{} {:.1} m, {:.1} m2",
                            t::TERRACE_WALL,
                            r.wall_length_m,
                            r.outer_face_m2
                        ),
                    ],
                    visioncraft_core::terrace::warnings(p, a, &r),
                ),
                None => (Vec::new(), Vec::new()),
            };
            ui.data_mut(|d| d.insert_temp(slot, (key, lines.clone(), warnings.clone())));
            (lines, warnings)
        }
    };
    for l in lines {
        ui.label(l);
    }
    for w in warnings {
        ui.colored_label(egui::Color32::from_rgb(230, 170, 70), w);
    }
    if ui.button(t::TERRACE_CLEAR).clicked() {
        cx.run("terrace.clear", json!({"id": a.id}));
    }
}

/// A raised area's retaining edge: none or a style; each change one command.
fn edge_panel(ui: &mut egui::Ui, cx: &mut Cx, a: &visioncraft_core::shape::Area) {
    use visioncraft_core::bed::BedStyle;
    let now = a.edge.map(|e| e.style);
    ui.horizontal_wrapped(|ui| {
        ui.label(t::EDGE);
        if ui.selectable_label(now.is_none(), t::EDGE_NONE).clicked() && now.is_some() {
            cx.run("area.clear_edge", json!({"id": a.id}));
        }
        ui.add_enabled_ui(!a.drape, |ui| {
            for (style, key, label) in [
                (BedStyle::Plain, "plain", t::BED_PLAIN),
                (BedStyle::Masonry, "masonry", t::BED_MASONRY),
                (BedStyle::Sleepers, "sleepers", t::BED_SLEEPERS),
                (BedStyle::Corten, "corten", t::BED_CORTEN),
            ] {
                if ui.selectable_label(now == Some(style), label).clicked() && now != Some(style) {
                    cx.run("area.set_edge", json!({"id": a.id, "style": key}));
                }
            }
        });
    });
    if a.drape {
        ui.weak(t::EDGE_NEEDS_FLAT);
    }
}

/// A piece of furniture's kind, size, turn and the options its kind has; each change one command.
fn furniture_panel(
    ui: &mut egui::Ui,
    cx: &mut Cx,
    x: &visioncraft_core::shape::Extrusion,
    curved: bool,
) {
    use visioncraft_core::furniture::FurnitureKind;
    let Some(f) = &x.furniture else {
        return;
    };
    let Ok(fr) = f.frame(&x.points, curved, x.height) else {
        return;
    };
    let kinds = furniture_choices();
    ui.horizontal(|ui| {
        ui.label(t::FURNITURE_KIND);
        egui::ComboBox::from_id_salt("furniture-kind")
            .selected_text(f.kind.name().replace('_', " "))
            .show_ui(ui, |ui| {
                for (label, k) in kinds.iter().filter_map(|(l, k)| k.map(|k| (l, k))) {
                    if ui.selectable_label(f.kind == k, label).clicked() && f.kind != k {
                        cx.run("furniture.set", json!({"id": x.id, "kind": k.name()}));
                    }
                }
            });
    });
    let [mut length, mut depth, mut height] = [fr.length, fr.depth, fr.height];
    let mut done = false;
    done |= drag_field(ui, t::FURNITURE_LENGTH, &mut length, 0.01, 0.1..=8.0, " m") == Edit::Done;
    if !fr.round {
        done |= drag_field(ui, t::FURNITURE_DEPTH, &mut depth, 0.01, 0.1..=8.0, " m") == Edit::Done;
    }
    done |= drag_field(ui, t::HEIGHT, &mut height, 0.01, 0.1..=4.0, " m") == Edit::Done;
    if done {
        let mut p = json!({"id": x.id, "length": length, "height": height});
        if !fr.round {
            p["depth"] = json!(depth);
        }
        cx.run("furniture.set", p);
    }
    let mut turn = fr.rotation_deg();
    if !fr.round
        && drag_field(ui, t::FURNITURE_TURN, &mut turn, 1.0, -360.0..=360.0, "°") == Edit::Done
    {
        cx.run("furniture.set", json!({"id": x.id, "rotation_deg": turn}));
    }
    let mut toggle = |ui: &mut egui::Ui, label: &str, now: bool, key: &str| {
        let mut v = now;
        if ui.checkbox(&mut v, label).changed() {
            cx.run("furniture.set", json!({"id": x.id, key: v}));
        }
    };
    if f.kind.can_be_round() {
        toggle(ui, t::FURNITURE_ROUND, fr.round, "round");
    }
    match f.kind {
        FurnitureKind::Table | FurnitureKind::SideTable => {
            toggle(
                ui,
                t::FURNITURE_PEDESTAL,
                f.pedestal.unwrap_or(fr.round),
                "pedestal",
            );
        }
        FurnitureKind::Bench => toggle(ui, t::FURNITURE_BACK, f.back, "back"),
        FurnitureKind::Parasol => toggle(ui, t::FURNITURE_OPEN, f.open, "open"),
        FurnitureKind::Lounger => {
            let mut deg = f.back_angle();
            if drag_field(ui, t::FURNITURE_BACK_DEG, &mut deg, 0.5, 0.0..=80.0, "°") == Edit::Done
            {
                cx.run("furniture.set", json!({"id": x.id, "back_deg": deg}));
            }
        }
        FurnitureKind::Sofa | FurnitureKind::Armchair => {
            let mut n = f.cushion_count(fr.length) as f32;
            if drag_field(ui, t::FURNITURE_CUSHIONS, &mut n, 0.1, 1.0..=8.0, "") == Edit::Done {
                cx.run(
                    "furniture.set",
                    json!({"id": x.id, "cushions": n.round() as u32}),
                );
            }
        }
        FurnitureKind::Cabinet => {
            let (mut doors, mut drawers) = (f.door_count(fr.length) as f32, f.drawers as f32);
            let ed = drag_field(ui, t::FURNITURE_DOORS, &mut doors, 0.1, 1.0..=8.0, "");
            let er = drag_field(ui, t::FURNITURE_DRAWERS, &mut drawers, 0.1, 0.0..=4.0, "");
            if ed == Edit::Done || er == Edit::Done {
                cx.run(
                    "furniture.set",
                    json!({"id": x.id, "doors": doors.round() as u32, "drawers": drawers.round() as u32}),
                );
            }
        }
        FurnitureKind::Chair | FurnitureKind::Bed => {}
    }
    if ui.button(t::BACK_TO_BLOCK).clicked() {
        cx.run("furniture.clear", json!({"id": x.id}));
    }
}

/// A raised bed's wall height and thickness (the soil follows); each change one command.
fn bed_panel(ui: &mut egui::Ui, cx: &mut Cx, x: &mut visioncraft_core::shape::Extrusion) {
    let Some(bed) = &x.raised_bed else {
        return;
    };
    use visioncraft_core::bed::BedStyle;
    ui.horizontal_wrapped(|ui| {
        for (style, key, label) in [
            (BedStyle::Plain, "plain", t::BED_PLAIN),
            (BedStyle::Masonry, "masonry", t::BED_MASONRY),
            (BedStyle::Sleepers, "sleepers", t::BED_SLEEPERS),
            (BedStyle::Corten, "corten", t::BED_CORTEN),
        ] {
            if ui.selectable_label(bed.style == style, label).clicked() && bed.style != style {
                cx.run("bed.set", json!({"id": x.id, "style": key}));
            }
        }
    });
    let (mut height, mut wall) = (x.height, bed.wall_thickness_m);
    let eh = drag_field(ui, t::HEIGHT, &mut height, 0.01, 0.1..=2.0, " m");
    let ew = drag_field(ui, t::WALL, &mut wall, 0.01, 0.05..=0.3, " m");
    if eh == Edit::Done || ew == Edit::Done {
        cx.run(
            "bed.set",
            json!({"id": x.id, "height": height, "wall_thickness_m": wall}),
        );
    }
}

/// A block's roof, plinth and windows: each change one command.
fn house_panel(ui: &mut egui::Ui, cx: &mut Cx, x: &visioncraft_core::shape::Extrusion) {
    use visioncraft_core::house::RoofKind;
    let Some(h) = &x.house else {
        if ui.button(t::ADD_ROOF).clicked() {
            cx.run("shape.set_roof", json!({"id": x.id, "kind": "gable"}));
            roof_tiles(cx, &x.id);
        }
        return;
    };
    let roof = h.roof.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label(t::ROOF);
        for (kind, key, label) in [
            (RoofKind::Gable, "gable", t::ROOF_GABLE),
            (RoofKind::Hip, "hip", t::ROOF_HIP),
            (RoofKind::LeanTo, "lean_to", t::ROOF_LEAN_TO),
            (RoofKind::Flat, "flat", t::ROOF_FLAT),
        ] {
            if ui.selectable_label(roof.kind == kind, label).clicked() && roof.kind != kind {
                cx.run(
                    "shape.set_roof",
                    json!({"id": x.id, "kind": key, "pitch_deg": roof.pitch_deg, "overhang_m": roof.overhang_m, "parapet_m": roof.parapet_m}),
                );
            }
        }
    });
    let key = |k: RoofKind| match k {
        RoofKind::Flat => "flat",
        RoofKind::Gable => "gable",
        RoofKind::Hip => "hip",
        RoofKind::LeanTo => "lean_to",
    };
    let (mut pitch, mut over) = (roof.pitch_deg, roof.overhang_m);
    let pe = if roof.kind != RoofKind::Flat {
        drag_field(ui, t::PITCH, &mut pitch, 0.5, 5.0..=70.0, "°")
    } else {
        Edit::None
    };
    let oe = drag_field(ui, t::OVERHANG, &mut over, 0.01, 0.0..=2.0, " m");
    if pe == Edit::Done || oe == Edit::Done {
        cx.run(
            "shape.set_roof",
            json!({"id": x.id, "kind": key(roof.kind), "pitch_deg": pitch, "overhang_m": over, "parapet_m": roof.parapet_m}),
        );
    }
    let mut plinth = h.plinth_m;
    if drag_field(ui, t::PLINTH, &mut plinth, 0.01, 0.0..=3.0, " m") == Edit::Done {
        cx.run("shape.set_plinth", json!({"id": x.id, "height_m": plinth}));
    }
    if !h.openings.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} {}", h.openings.len(), t::OPENINGS));
            if ui.small_button(t::REMOVE_LAST_OPENING).clicked()
                && let Some(o) = h.openings.last()
            {
                cx.run("shape.remove_opening", json!({"id": x.id, "opening": o.id}));
            }
        });
    }
}

#[cfg(test)]
mod tests {

    /// Walk from Draw (`App::toggle_walk`): the area being drawn is kept as Enter would keep it,
    /// a two-point start is dropped, and leaving Draw leaves the plan for the 3D view.
    #[test]
    fn walking_from_draw_keeps_the_drawn_area_and_leaves_the_plan() {
        let Ok((device, queue)) = pollster::block_on(visioncraft_render::headless_device()) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let renderer =
            visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut s = crate::studio::Studio::new(renderer, visioncraft_core::Project::default());
        let (mut toasts, input, ctx) = (
            crate::ui::Toasts::default(),
            crate::ui::Input::default(),
            eframe::egui::Context::default(),
        );
        let mut cx = crate::ui::Cx {
            s: &mut s,
            toasts: &mut toasts,
            input: &input,
            ctx: &ctx,
        };
        let mut d = super::DrawState::default();
        d.enter(&mut cx);
        assert!(cx.s.camera.view == crate::camera::View::Top);
        d.tool = super::Tool::Area;
        d.points = vec![[0.0, 0.0], [4.0, 0.0]];
        d.finish_for_walk(&mut cx);
        assert!(cx.s.project().shapes.is_empty() && d.points.is_empty());
        d.points = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]];
        d.finish_for_walk(&mut cx);
        assert_eq!(cx.s.project().shapes.len(), 1);
        d.leave(&mut cx);
        assert!(cx.s.camera.view != crate::camera::View::Top);
    }

    #[test]
    fn typed_sizes_come_out_exact() {
        let p0 = [-10.2, 4.3];
        let r = super::rectangle(p0, [0.0, 20.0], 20.0, 30.0);
        let (w, d) = super::rect_size(&r).unwrap();
        assert!(
            (w - 20.0).abs() < 1e-4 && (d - 30.0).abs() < 1e-4,
            "{w} {d}"
        );
        // Towards a pointer a little off the x axis: still along it.
        let q = super::typed_point(p0, [0.0, 5.0], 12.5);
        assert!((q[0] - (p0[0] + 12.5)).abs() < 1e-4 && (q[1] - p0[1]).abs() < 1e-6);
        // A turned rectangle keeps its turn when resized.
        let turned = vec![[0.0, 0.0], [3.0, 4.0], [-1.0, 7.0], [-4.0, 3.0]];
        let resized = super::resized_rect(&turned, 10.0, 2.0).unwrap();
        let (w, d) = super::rect_size(&resized).unwrap();
        assert!((w - 10.0).abs() < 1e-4 && (d - 2.0).abs() < 1e-4);
        assert!(super::rect_size(&[[0.0, 0.0], [1.0, 0.0], [2.0, 1.0], [0.0, 1.0]]).is_none());
    }

    #[test]
    fn names_count_per_kind() {
        let mut s = visioncraft_core::Session::new(visioncraft_core::Project::default());
        let sq = serde_json::json!([[0, 0], [1, 0], [1, 1]]);
        assert_eq!(super::next_name(s.project(), "Terrace"), "Terrace 1");
        s.run(
            "shape.add_area",
            serde_json::json!({"name": "Terrace 1", "outline": sq}),
        )
        .unwrap();
        s.run(
            "shape.add_area",
            serde_json::json!({"name": "Lawn 1", "outline": sq}),
        )
        .unwrap();
        assert_eq!(super::next_name(s.project(), "Terrace"), "Terrace 2");
        assert_eq!(super::next_name(s.project(), "Block"), "Block 1");
        for k in super::AreaKind::ALL {
            let m = k.spec().1;
            assert!(s.project().materials.iter().any(|x| x.id == m), "{m}");
        }
    }

    use super::*;

    #[test]
    fn grid_snaps_to_a_tenth() {
        let s = snap_point([1.234, 4.567], &[], &[], 0.2, true);
        assert_eq!(s.kind, SnapKind::Grid);
        assert!((s.pos[0] - 1.2).abs() < 1e-5 && (s.pos[1] - 4.6).abs() < 1e-5);
        let free = snap_point([1.234, 4.567], &[], &[], 0.2, false);
        assert_eq!((free.kind, free.pos), (SnapKind::Free, [1.234, 4.567]));
    }

    #[test]
    fn vertices_beat_grid_and_pending_counts() {
        let s = snap_point([5.04, 5.03], &[], &[[5.0, 5.0]], 0.2, true);
        assert_eq!((s.kind, s.pos), (SnapKind::Vertex, [5.0, 5.0]));
        let s = snap_point(
            [0.05, 0.02],
            &[[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]],
            &[],
            0.2,
            true,
        );
        assert_eq!(s.kind, SnapKind::Vertex);
    }

    #[test]
    fn right_angle_to_the_previous_segment() {
        // Previous segment runs along +x; the cursor is almost straight "up" from the last point.
        let pending = [[0.0, 0.0], [4.0, 0.0]];
        let s = snap_point([4.1, 3.02], &pending, &[], 0.2, true);
        assert_eq!(s.kind, SnapKind::RightAngle);
        assert!(
            (s.pos[0] - 4.0).abs() < 1e-4 && (s.pos[1] - 3.0).abs() < 1e-4,
            "{:?}",
            s.pos
        );
        // Far from any right angle: plain grid.
        let s = snap_point([6.0, 2.0], &pending, &[], 0.2, true);
        assert_eq!(s.kind, SnapKind::Grid);
    }

    #[test]
    fn hits_and_closing() {
        let sq = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        assert_eq!(vertex_hit(&sq, [3.9, 0.1], 0.3), Some(1));
        assert_eq!(vertex_hit(&sq, [2.0, 2.0], 0.3), None);
        let (edge, q) = edge_hit(&sq, true, [2.0, 0.1], 0.3).unwrap();
        assert_eq!(edge, 0);
        assert!((q[0] - 2.0).abs() < 1e-5 && q[1].abs() < 1e-5);
        assert!(edge_hit(&sq, false, [0.1, 2.0], 0.3).is_none()); // open: no closing edge
        assert!(closes(&sq, [0.1, 0.1], 0.3));
        assert!(!closes(&sq[..2], [0.1, 0.1], 0.3));
    }

    #[test]
    fn scale_bar_and_grid_steps() {
        assert_eq!(nice_length(7.3), 5.0);
        assert_eq!(nice_length(19.0), 10.0);
        assert_eq!(nice_length(0.9), 0.5);
        assert_eq!(grid_step(10.0), 1.0);
        assert_eq!(grid_step(60.0), 5.0);
        assert!((outline_length(&[[0.0, 0.0], [3.0, 4.0]], false) - 5.0).abs() < 1e-5);
        assert!((outline_length(&[[0.0, 0.0], [3.0, 0.0], [3.0, 4.0]], true) - 12.0).abs() < 1e-5);
    }
}
