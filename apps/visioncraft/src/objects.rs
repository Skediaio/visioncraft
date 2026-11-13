//! Objects mode: the planting tools (`planting_tools`, `planting_panel`), which place plants and
//! catalogue objects alike, the browser (Plants | Objects) that their "+ Add plants" buttons
//! open, and the Lights tool.

use crate::planting_tools::PlantingTools;
use crate::strings as t;
use crate::ui::Cx;
use eframe::egui;
use visioncraft_flora as flora;

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

#[derive(Default)]
pub struct ObjectsState {
    /// The planting tools: palettes, presets, six tools, selecting and editing plantings.
    pub tools: PlantingTools,
    search: String,
    category: Option<flora::Category>,
    evergreen: bool,
    in_bloom: bool,
    /// The Plants | Objects tabs of the browser, and the Lights tool.
    pub library: crate::library_ui::LibraryUi,
    /// Making a species of one's own from a plant of the browser.
    pub species: crate::species_editor::SpeciesEditor,
}

pub(crate) fn display_name(cx: &Cx, id: &str) -> String {
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
        self.species.close(cx.s);
        self.tools.leave(cx);
        cx.s.extra = Default::default();
    }

    pub fn update(&mut self, cx: &mut Cx) {
        if self.tools.lights {
            self.library.lights.update(cx);
        } else {
            self.tools.update(cx);
        }
        self.species.update(cx.s);
    }

    /// The plant browser: a searchable, filterable list of the built-in species and the project's own.
    fn browser(&mut self, ui: &mut egui::Ui, cx: &mut Cx) -> Option<String> {
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
        let mut edit: Option<String> = None;
        egui::ScrollArea::vertical()
            .max_height(170.0)
            .id_salt("plants")
            .show(ui, |ui| {
                for c in &cx.s.project().species {
                    if needle.is_empty() || c.name.to_lowercase().contains(&needle) {
                        ui.horizontal(|ui| {
                            if ui.button(format!("{} ({})", c.name, t::SP_OWN)).clicked() {
                                add = Some(c.id.clone());
                            }
                            if ui.small_button(t::SP_EDIT).clicked() {
                                edit = Some(c.id.clone());
                            }
                        });
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
                    let row = ui.horizontal(|ui| {
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
                        if ui
                            .small_button(t::SP_EDIT)
                            .on_hover_text(t::SP_EDIT_TIP)
                            .clicked()
                        {
                            edit = Some(format!("flora:{}", s.id));
                        }
                        r
                    });
                    if row.inner.clicked() {
                        add = Some(format!("flora:{}", s.id));
                    }
                }
            });
        if let Some(id) = edit {
            self.species.open(cx.s.project(), &id);
        }
        add
    }

    /// The planting panel, and next to it the plants browser while a "+" is open.
    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        self.species.window(cx.ctx, cx);
        if self.tools.lights {
            self.tools.tool_row(ui, cx);
            self.library.lights.panel(ui, cx);
            return;
        }
        self.tools.panel(ui, cx);
        if cx.s.plants_pending() > 0 {
            ui.small(format!("generating plants… {}", cx.s.plants_pending()));
        }
        if self.tools.add_to.is_some() {
            let narrow = crate::ui::narrow(cx.ctx);
            let (anchor, offset) = if narrow {
                (egui::Align2::LEFT_TOP, egui::Vec2::new(10.0, 150.0))
            } else {
                (
                    egui::Align2::LEFT_BOTTOM,
                    egui::Vec2::new(10.0 + 430.0 + 10.0, -58.0),
                )
            };
            let (mut added, mut close) = (None, false);
            crate::ui::floating(cx.ctx, "plantbrowser", anchor, offset, 330.0, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(t::PT_ADD_PLANTS).strong());
                    if ui.small_button("×").clicked() {
                        close = true;
                    }
                });
                // Catalogue objects are palette entries like plants: benches can be brushed too.
                added = if self.library.tabs(ui) {
                    self.browser(ui, cx)
                } else {
                    self.library.panel(ui, cx)
                };
            });
            if let Some(id) = added {
                self.tools.add_asset(cx, id);
            }
            if close {
                self.tools.add_to = None;
            }
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
