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

/// Whether a click at `p` closes a pending outline of `points`.
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
}

#[derive(Default)]
pub struct DrawState {
    pub tool: Tool,
    pub points: Vec<P2>,
    pub selected: Option<String>,
    pub vertex: Option<usize>,
    dragging: Option<usize>,
}

const ACCENT: [u8; 4] = [255, 200, 70, 255];

/// Replaces corner `i` of a shape's outline.
fn with_vertex(shape: &Shape, i: usize, p: P2) -> Shape {
    let mut s = shape.clone();
    match &mut s {
        Shape::Area(a) => a.outline[i] = p,
        Shape::Extrusion(e) => e.outline[i] = p,
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
        .flat_map(|s| s.outline().iter().copied())
        .collect()
}

impl DrawState {
    pub fn enter(&mut self, cx: &mut Cx) {
        cx.s.camera.view = crate::camera::View::Top;
        self.points.clear();
    }

    pub fn leave(&mut self, cx: &mut Cx) {
        cx.s.camera.view = crate::camera::View::Orbit;
        cx.s.set_draft(None);
        cx.s.extra = Default::default();
        self.points.clear();
        self.tool = Tool::Select;
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
        if self.tool != Tool::Select {
            self.drawing(cx, cursor, radius, &mut top);
        } else {
            self.editing(cx, cursor, radius, &mut top);
        }
        if cx.input.escape && self.tool != Tool::Select && self.points.is_empty() {
            self.tool = Tool::Select;
        }
        cx.s.extra = (depth, top);
        self.scale_bar(cx);
    }

    fn drawing(&mut self, cx: &mut Cx, cursor: Option<P2>, radius: f32, top: &mut Vec<LineVertex>) {
        let closed_shape = self.tool != Tool::Wall;
        let existing = all_vertices(cx, None);
        let snap = cursor.map(|c| snap_point(c, &self.points, &existing, radius, !cx.input.alt));
        if cx.input.escape && !self.points.is_empty() {
            self.points.clear();
        }
        if cx.input.backspace {
            self.points.pop();
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
        if cx.input.enter {
            let needed = if closed_shape { 3 } else { 2 };
            if self.points.len() >= needed {
                self.finish(cx);
            }
        }
        // Preview.
        let mut pts = self.points.clone();
        if let Some(s) = snap
            && pts.last() != Some(&s.pos)
        {
            pts.push(s.pos);
        }
        let y = 0.05;
        top.extend(outline_lines(&pts, false, y, ACCENT, 3.0));
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
            self.length_label(cx, w[0], w[1]);
        }
    }

    fn length_label(&self, cx: &Cx, a: P2, b: P2) {
        let len = dist(a, b);
        if len < 0.05 {
            return;
        }
        let mid = y_point([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0], 0.05);
        if let Some(p) = cx.project(mid) {
            cx.label(p, &format!("{len:.2} m"));
        }
    }

    fn finish(&mut self, cx: &mut Cx) {
        let points = std::mem::take(&mut self.points);
        let n = cx.s.project().shapes.len() + 1;
        let (cmd, material, params) = match self.tool {
            Tool::Area => (
                "shape.add_area",
                "lawn",
                json!({"name": format!("Area {n}"), "outline": points}),
            ),
            Tool::Block => (
                "shape.add_extrusion",
                "plaster",
                json!({"name": format!("Block {n}"), "outline": points, "closed": true, "height": 3.0}),
            ),
            Tool::Wall => (
                "shape.add_extrusion",
                "timber",
                json!({"name": format!("Wall {n}"), "outline": points, "closed": false, "height": 1.8, "thickness": 0.1}),
            ),
            Tool::Select => return,
        };
        if let Some(r) = cx.run(cmd, params) {
            let id = r["id"].as_str().unwrap_or_default().to_string();
            cx.run(
                "material.assign",
                json!({"target": {"type": "owner", "owner": id}, "material": material}),
            );
            self.selected = Some(id);
            self.vertex = None;
            self.tool = Tool::Select;
        }
    }

    fn editing(&mut self, cx: &mut Cx, cursor: Option<P2>, radius: f32, top: &mut Vec<LineVertex>) {
        let shape = self.shape(cx);
        if let (Some(shape), Some(c)) = (&shape, cursor) {
            let outline = shape.outline();
            if cx.input.drag_started {
                // egui reports the start once the pointer has already moved: test where it was pressed.
                let pressed = cx
                    .input
                    .press_origin
                    .and_then(|p| Self::world_at(cx, p))
                    .unwrap_or(c);
                self.dragging = vertex_hit(outline, pressed, radius);
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
                        crate::studio::Draft::Shape(s) => s.outline().get(i).copied(),
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
                .and_then(|s| vertex_hit(s.outline(), c, radius));
            let hit_edge = shape
                .as_ref()
                .and_then(|s| edge_hit(s.outline(), s.is_closed(), c, radius * 0.7));
            if let Some(i) = hit_vertex {
                self.vertex = Some(i);
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
            top.extend(outline_lines(
                shape.outline(),
                shape.is_closed(),
                y,
                [90, 200, 255, 255],
                3.0,
            ));
            for (i, p) in shape.outline().iter().enumerate() {
                let sel = self.vertex == Some(i);
                top.extend(dot(
                    *p,
                    y,
                    if sel { ACCENT } else { [255, 255, 255, 255] },
                    if sel { 13.0 } else { 9.0 },
                ));
            }
            let o = shape.outline();
            let n = o.len();
            for i in 0..if shape.is_closed() {
                n
            } else {
                n.saturating_sub(1)
            } {
                self.length_label(cx, o[i], o[(i + 1) % n]);
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

    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        ui.horizontal(|ui| {
            for (tool, label) in [
                (Tool::Select, t::TOOL_SELECT),
                (Tool::Area, t::TOOL_AREA),
                (Tool::Block, t::TOOL_BLOCK),
                (Tool::Wall, t::TOOL_WALL),
            ] {
                if ui.selectable_label(self.tool == tool, label).clicked() {
                    self.tool = tool;
                    self.points.clear();
                }
            }
        });
        ui.small(match self.tool {
            Tool::Select => t::SELECT_HINT,
            Tool::Wall => t::WALL_HINT,
            _ => t::DRAW_HINT,
        });
        let Some(shape) = self.shape(cx) else { return };
        ui.separator();
        let mut name = shape.name().to_string();
        ui.horizontal(|ui| {
            ui.label(t::NAME);
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
            if r.lost_focus() && name != shape.name() && !name.is_empty() {
                cx.run("shape.rename", json!({"id": shape.id(), "name": name}));
            }
        });
        let mut edited = shape.clone();
        let mut edit = Edit::None;
        let mut params = serde_json::Map::new();
        match &mut edited {
            Shape::Area(a) => {
                let e = drag_field(ui, t::ELEVATION, &mut a.elevation, 0.05, -50.0..=50.0, " m");
                if e != Edit::None {
                    params.insert("elevation".into(), json!(a.elevation));
                }
                edit = e;
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
                if !x.closed {
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
        let (closed, outline) = (shape.is_closed(), shape.outline());
        if closed {
            ui.label(format!(
                "{}: {:.2} m²",
                t::AREA,
                measure::polygon_area(outline)
            ));
        }
        ui.label(format!(
            "{}: {:.2} m",
            if closed { t::PERIMETER } else { "Length" },
            outline_length(outline, closed)
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
    }
}

#[cfg(test)]
mod tests {
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
