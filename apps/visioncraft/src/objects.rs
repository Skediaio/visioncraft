//! Objects mode: single placement and area scatter against the demo catalogue. Brush, line,
//! cluster and surface-fill tools get their own `Tool` variant and panel section next round.

use crate::draw::{closes, dot, outline_lines, snap_point};
use crate::strings as t;
use crate::ui::Cx;
use eframe::egui;
use glam::Vec3;
use serde_json::json;
use visioncraft_render::LineVertex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Single,
    Scatter,
}

pub struct ObjectsState {
    pub tool: Tool,
    pub asset: usize,
    pub density: f32,
    points: Vec<[f32; 2]>,
    placed: u64,
}

impl Default for ObjectsState {
    fn default() -> Self {
        Self {
            tool: Tool::Single,
            asset: 0,
            density: 0.6,
            points: Vec::new(),
            placed: 0,
        }
    }
}

impl ObjectsState {
    pub fn leave(&mut self, cx: &mut Cx) {
        self.points.clear();
        cx.s.extra = Default::default();
    }

    /// The ground point under the pointer: the first surface hit, else the plane y = 0.
    fn ground(cx: &Cx) -> Option<Vec3> {
        let p = cx.input.hover?;
        let (o, d) = cx.s.pointer_ray(glam::Vec2::new(p.x, p.y), cx.size());
        match cx.s.scene.raycast(o, d, false) {
            Some(h) => Some(h.point),
            None => crate::camera::ray_plane(o, d, 0.0),
        }
    }

    pub fn update(&mut self, cx: &mut Cx) {
        let mut top: Vec<LineVertex> = Vec::new();
        let ground = Self::ground(cx);
        let Some(asset) = cx.s.assets.get(self.asset).cloned() else {
            return;
        };
        match self.tool {
            Tool::Single => {
                if let Some(g) = ground {
                    top.extend(dot([g.x, g.z], g.y + 0.05, [255, 255, 255, 230], 10.0));
                    if cx.input.clicked {
                        self.placed += 1;
                        cx.run(
                            "planting.add_single",
                            json!({"name": asset.name, "asset": asset.id, "position": [g.x, g.y, g.z], "rotation_y_deg": (self.placed * 137 % 360) as f32}),
                        );
                    }
                }
            }
            Tool::Scatter => {
                if cx.input.escape || cx.input.backspace {
                    if cx.input.escape {
                        self.points.clear()
                    } else {
                        self.points.pop();
                    }
                }
                let radius = 0.4 * (cx.s.camera.eye().length().max(5.0) / 20.0);
                let snap = ground
                    .map(|g| snap_point([g.x, g.z], &self.points, &[], radius, !cx.input.alt));
                if cx.input.clicked
                    && let Some(s) = snap
                {
                    if closes(&self.points, s.pos, radius) {
                        self.finish(cx, &asset);
                    } else {
                        self.points.push(s.pos);
                    }
                }
                if cx.input.enter && self.points.len() >= 3 {
                    self.finish(cx, &asset);
                }
                let mut pts = self.points.clone();
                pts.extend(snap.map(|s| s.pos));
                top.extend(outline_lines(&pts, false, 0.08, [120, 230, 140, 255], 3.0));
                for p in &self.points {
                    top.extend(dot(*p, 0.08, [120, 230, 140, 255], 9.0));
                }
            }
        }
        cx.s.extra = (Vec::new(), top);
    }

    fn finish(&mut self, cx: &mut Cx, asset: &visioncraft_core::Asset) {
        let outline = std::mem::take(&mut self.points);
        self.placed += 1;
        cx.run(
            "planting.add_area",
            json!({
                "name": format!("{} scatter", asset.name),
                "asset_palette": [{"asset": asset.id, "weight": 1.0}],
                "outline": outline,
                "density_per_m2": self.density,
                "scale_min": 0.8, "scale_max": 1.2,
                "seed": self.placed,
            }),
        );
    }

    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        ui.horizontal_wrapped(|ui| {
            for (i, a) in cx.s.assets.iter().enumerate() {
                ui.selectable_value(&mut self.asset, i, &a.name);
            }
        });
        ui.horizontal(|ui| {
            for (tool, label) in [
                (Tool::Single, t::TOOL_SINGLE),
                (Tool::Scatter, t::TOOL_SCATTER),
            ] {
                if ui.selectable_label(self.tool == tool, label).clicked() {
                    self.tool = tool;
                    self.points.clear();
                }
            }
        });
        ui.small(if self.tool == Tool::Single {
            t::SINGLE_HINT
        } else {
            t::SCATTER_HINT
        });
        if self.tool == Tool::Scatter {
            ui.horizontal(|ui| {
                ui.label(t::DENSITY);
                ui.add(
                    egui::DragValue::new(&mut self.density)
                        .range(0.02..=20.0)
                        .speed(0.02),
                );
            });
        }
        let list: Vec<(String, String)> =
            cx.s.project()
                .generators
                .iter()
                .map(|g| (g.id().to_string(), g.name().to_string()))
                .collect();
        if !list.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(t::PLANTINGS).strong());
            egui::ScrollArea::vertical()
                .max_height(110.0)
                .show(ui, |ui| {
                    for (id, name) in list {
                        ui.horizontal(|ui| {
                            ui.label(name);
                            if ui.small_button(t::REMOVE).clicked() {
                                cx.run("planting.remove", json!({"id": id}));
                            }
                        });
                    }
                });
        }
    }
}
