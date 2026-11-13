//! Objects mode: single placement and area scatter against the demo catalogue. Brush, line,
//! cluster and surface-fill tools get their own `Tool` variant and panel section next round.

use crate::draw::{closes, dot, outline_lines, snap_point};
use crate::strings as t;
use crate::ui::Cx;
use eframe::egui;
use glam::Vec3;
use serde_json::json;
use visioncraft_flora as flora;
use visioncraft_render::LineVertex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Single,
    Scatter,
}

/// Weights as whole percentages that add up to 100 (largest remainder), for the palette readout.
pub fn percentages(weights: &[f32]) -> Vec<u32> {
    let total: f32 = weights.iter().sum();
    if total <= 0.0 || weights.is_empty() {
        return vec![0; weights.len()];
    }
    let exact: Vec<f32> = weights.iter().map(|w| w / total * 100.0).collect();
    let mut out: Vec<u32> = exact.iter().map(|e| *e as u32).collect();
    let mut order: Vec<usize> = (0..exact.len()).collect();
    order.sort_by(|&a, &b| (exact[b] - exact[b].floor()).total_cmp(&(exact[a] - exact[a].floor())));
    for i in order
        .into_iter()
        .take((100 - out.iter().sum::<u32>()) as usize)
    {
        out[i] += 1;
    }
    out
}

pub struct ObjectsState {
    pub tool: Tool,
    /// The chosen assets (catalogue id, `flora:<id>` or custom species id) and their weights.
    pub palette: Vec<(String, f32)>,
    pub density: f32,
    search: String,
    category: Option<flora::Category>,
    evergreen: bool,
    in_bloom: bool,
    points: Vec<[f32; 2]>,
    placed: u64,
    /// The Objects and Lights tabs beside the plants browser.
    pub library: crate::library_ui::LibraryUi,
}

impl Default for ObjectsState {
    fn default() -> Self {
        Self {
            tool: Tool::Single,
            palette: vec![("flora:prunus-serrulata".into(), 1.0)],
            density: 0.6,
            search: String::new(),
            category: None,
            evergreen: false,
            in_bloom: false,
            points: Vec::new(),
            placed: 0,
            library: Default::default(),
        }
    }
}

fn display_name(cx: &Cx, id: &str) -> String {
    if let Some(s) = id.strip_prefix("flora:").and_then(flora::Species::by_id) {
        return s.common_name.to_string();
    }
    if let Some(c) = cx.s.project().species.iter().find(|c| c.id == id) {
        return c.name.clone();
    }
    cx.s.assets
        .iter()
        .find(|a| a.id == id)
        .map_or_else(|| id.to_string(), |a| a.name.clone())
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

    fn palette_json(&self) -> serde_json::Value {
        json!(
            self.palette
                .iter()
                .map(|(a, w)| json!({"asset": a, "weight": w}))
                .collect::<Vec<_>>()
        )
    }

    pub fn update(&mut self, cx: &mut Cx) {
        if self.library.tab == crate::library_ui::Tab::Lights {
            return self.library.lights.update(cx);
        }
        let mut top: Vec<LineVertex> = Vec::new();
        let ground = Self::ground(cx);
        let Some(first) = self.palette.first().map(|p| p.0.clone()) else {
            return;
        };
        match self.tool {
            Tool::Single => {
                if let Some(g) = ground {
                    top.extend(dot([g.x, g.z], g.y + 0.05, [255, 255, 255, 230], 10.0));
                    if cx.input.clicked {
                        self.placed += 1;
                        let name = display_name(cx, &first);
                        cx.run(
                            "planting.add_single",
                            json!({"name": name, "asset": first, "position": [g.x, g.y, g.z], "rotation_y_deg": (self.placed * 137 % 360) as f32}),
                        );
                    }
                }
            }
            Tool::Scatter => {
                if cx.input.escape {
                    self.points.clear();
                } else if cx.input.backspace {
                    self.points.pop();
                }
                let radius = 0.4 * (cx.s.camera.eye().length().max(5.0) / 20.0);
                let snap = ground
                    .map(|g| snap_point([g.x, g.z], &self.points, &[], radius, !cx.input.alt));
                if cx.input.clicked
                    && let Some(s) = snap
                {
                    if closes(&self.points, s.pos, radius) {
                        self.finish(cx);
                    } else {
                        self.points.push(s.pos);
                    }
                }
                if cx.input.enter && self.points.len() >= 3 {
                    self.finish(cx);
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

    fn finish(&mut self, cx: &mut Cx) {
        let outline = std::mem::take(&mut self.points);
        self.placed += 1;
        let name = format!("{} scatter", display_name(cx, &self.palette[0].0));
        cx.run(
            "planting.add_area",
            json!({
                "name": name,
                "asset_palette": self.palette_json(),
                "outline": outline,
                "density_per_m2": self.density,
                "scale_min": 0.8, "scale_max": 1.2,
                "seed": self.placed,
            }),
        );
    }

    /// The plant browser: a searchable, filterable list of the built-in species and the project's own.
    fn browser(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(t::SEARCH)
                    .desired_width(120.0),
            );
            egui::ComboBox::from_id_salt("plantcat")
                .selected_text(
                    self.category
                        .map_or("All".to_string(), |c| format!("{c:?}")),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.category, None, "All");
                    for c in [
                        flora::Category::Tree,
                        flora::Category::Conifer,
                        flora::Category::Palm,
                        flora::Category::Shrub,
                        flora::Category::Hedge,
                        flora::Category::Perennial,
                        flora::Category::Fern,
                        flora::Category::Grass,
                        flora::Category::GroundCover,
                        flora::Category::Climber,
                    ] {
                        ui.selectable_value(&mut self.category, Some(c), format!("{c:?}"));
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.evergreen, t::EVERGREEN);
            ui.checkbox(&mut self.in_bloom, t::IN_BLOOM);
        });
        let month = cx.s.project().time.month as u8;
        let needle = self.search.to_lowercase();
        let mut add: Option<String> = None;
        egui::ScrollArea::vertical()
            .max_height(170.0)
            .id_salt("plants")
            .show(ui, |ui| {
                for c in &cx.s.project().species {
                    if needle.is_empty() || c.name.to_lowercase().contains(&needle) {
                        let r = ui.button(format!("{} (own)", c.name));
                        if r.clicked() {
                            add = Some(c.id.clone());
                        }
                    }
                }
                for s in flora::species() {
                    if self.category.is_some_and(|c| c != s.category)
                        || (self.evergreen && !s.evergreen)
                        || (self.in_bloom && !s.bloom_months.contains(&month))
                        || !(needle.is_empty()
                            || s.common_name.to_lowercase().contains(&needle)
                            || s.botanical_name.to_lowercase().contains(&needle))
                    {
                        continue;
                    }
                    let r = ui
                        .button(format!("{}  ·  {:.1} m", s.common_name, s.mature_height_m))
                        .on_hover_text(format!(
                            "{}\n{:.1} × {:.1} m, {}{}",
                            s.botanical_name,
                            s.mature_height_m,
                            s.mature_width_m,
                            if s.evergreen {
                                "evergreen"
                            } else {
                                "deciduous"
                            },
                            if s.bloom_months.is_empty() {
                                String::new()
                            } else {
                                format!(", blooms {:?}", s.bloom_months)
                            },
                        ));
                    if r.clicked() {
                        add = Some(format!("flora:{}", s.id));
                    }
                }
            });
        if let Some(id) = add
            && !self.palette.iter().any(|p| p.0 == id)
        {
            self.palette.push((id, 1.0));
        }
    }

    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        if self.library.tabs(ui) {
            self.browser(ui, cx);
        } else {
            self.library
                .panel(ui, cx, &mut self.palette, &mut self.tool);
            if self.library.tab == crate::library_ui::Tab::Lights {
                return;
            }
        }
        ui.separator();
        ui.label(egui::RichText::new(t::PALETTE).strong());
        let pct = percentages(&self.palette.iter().map(|p| p.1).collect::<Vec<_>>());
        let mut remove = None;
        for (i, (id, w)) in self.palette.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}%", pct[i]));
                ui.add(egui::Slider::new(w, 0.0..=10.0).show_value(false));
                ui.label(display_name(cx, id));
                if ui.small_button("x").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove
            && self.palette.len() > 1
        {
            self.palette.remove(i);
        }
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
                .max_height(90.0)
                .id_salt("placed")
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
        if cx.s.plants_pending() > 0 {
            ui.small(format!("generating plants… {}", cx.s.plants_pending()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::percentages;

    #[test]
    fn percentages_add_up_to_100() {
        assert_eq!(percentages(&[1.0, 1.0, 1.0]).iter().sum::<u32>(), 100);
        assert_eq!(percentages(&[3.0, 1.0]), vec![75, 25]);
        assert_eq!(percentages(&[0.0, 0.0]), vec![0, 0]);
        assert_eq!(
            percentages(&[1.0, 2.0, 4.0, 8.0, 16.0, 32.0])
                .iter()
                .sum::<u32>(),
            100
        );
    }
}
