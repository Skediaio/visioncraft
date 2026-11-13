//! The "Objects" tab of the Objects panel: the catalogue by category with search, lazily loaded
//! thumbnails, hover details and a recent row. Also the thumbnail cache the other library panels
//! (materials, skies) use.

use crate::assets::{Key, Loaded, State};
use crate::lamps::LightsTool;
use crate::strings as t;
use crate::ui::{ACCENT, Cx};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::collections::HashMap;
use visioncraft_core::catalog::{Asset, Category};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Plants,
    Objects,
}

enum Thumb {
    Ready(egui::TextureHandle),
    Failed,
}

/// Library images for the UI, loaded when first drawn.
#[derive(Default)]
pub struct Thumbs {
    map: HashMap<String, Thumb>,
}

/// Draws a library image at `rect` (a grey tile while it loads); asks for it only if `visible`.
pub fn thumb(
    ui: &egui::Ui,
    cx: &mut Cx,
    path: &str,
    rect: egui::Rect,
    visible: bool,
    priority: f32,
) {
    let mut cache = std::mem::take(&mut cx.s.thumbs);
    let round = egui::CornerRadius::same(6);
    if !path.is_empty() && !cache.map.contains_key(path) {
        let key = Key::Thumb(path.to_string());
        let store = &mut cx.s.scene.library.store;
        if let Some(Loaded::Image(img)) = store.take(&key) {
            let image =
                egui::ColorImage::from_rgba_unmultiplied([img.w as usize, img.h as usize], &img.px);
            let tex = cx
                .ctx
                .load_texture(path, image, egui::TextureOptions::LINEAR);
            cache.map.insert(path.to_string(), Thumb::Ready(tex));
        } else if store.state(&key) == State::Failed {
            cache.map.insert(path.to_string(), Thumb::Failed);
        } else if visible {
            store.want(key, priority);
        }
    }
    match cache.map.get(path) {
        Some(Thumb::Ready(tex)) => {
            egui::Image::new(tex)
                .corner_radius(round)
                .paint_at(ui, rect);
        }
        _ => {
            ui.painter()
                .rect_filled(rect, round, Color32::from_white_alpha(20));
        }
    }
    cx.s.thumbs = cache;
}

pub struct LibraryUi {
    pub tab: Tab,
    pub lights: LightsTool,
    search: String,
    category: Option<Category>,
    recent: Vec<String>,
}

impl Default for LibraryUi {
    fn default() -> Self {
        Self {
            tab: Tab::Plants,
            lights: LightsTool::default(),
            search: String::new(),
            category: None,
            recent: Vec::new(),
        }
    }
}

fn matches(a: &Asset, needle: &str) -> bool {
    needle.is_empty()
        || a.name.to_lowercase().contains(needle)
        || a.tags.iter().any(|t| t.to_lowercase().contains(needle))
}

fn category_name(c: Category) -> String {
    format!("{c:?}")
}

fn details(a: &Asset) -> String {
    format!(
        "{}\n{:.2} × {:.2} × {:.2} m (w × h × d)\n{} · {}",
        a.name, a.size_m[0], a.size_m[1], a.size_m[2], a.license, a.author
    )
}

impl LibraryUi {
    /// The tab switch (Plants | Objects); true while the plants browser should show.
    pub fn tabs(&mut self, ui: &mut egui::Ui) -> bool {
        ui.horizontal(|ui| {
            for (tab, label) in [(Tab::Plants, t::TAB_PLANTS), (Tab::Objects, t::TAB_OBJECTS)] {
                if ui.selectable_label(self.tab == tab, label).clicked() {
                    self.tab = tab;
                }
            }
        });
        self.tab == Tab::Plants
    }

    /// The catalogue grid; returns the asset clicked.
    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) -> Option<String> {
        let Some(catalog) = cx.s.scene.library.catalog.clone() else {
            ui.small(t::LIBRARY_LOADING);
            return None;
        };
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(t::SEARCH)
                    .desired_width(110.0),
            );
            egui::ComboBox::from_id_salt("libcat")
                .selected_text(self.category.map_or("All".to_string(), category_name))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.category, None, "All");
                    let mut seen: Vec<Category> = Vec::new();
                    for a in &catalog.assets {
                        if !seen.contains(&a.category) {
                            seen.push(a.category);
                        }
                    }
                    for c in seen {
                        ui.selectable_value(&mut self.category, Some(c), category_name(c));
                    }
                });
        });
        let needle = self.search.to_lowercase();
        let tile = 54.0;
        let mut picked: Option<String> = None;
        if !self.recent.is_empty() {
            ui.small(t::RECENT);
            ui.horizontal(|ui| {
                for id in self.recent.clone() {
                    let Some(a) = catalog.asset(&id) else {
                        continue;
                    };
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
                    let vis = ui.is_rect_visible(rect);
                    thumb(ui, cx, &a.thumbnail, rect, vis, 0.0);
                    if resp.on_hover_text(&a.name).clicked() {
                        picked = Some(id.clone());
                    }
                }
            });
        }
        egui::ScrollArea::vertical()
            .max_height(190.0)
            .id_salt("objects-library")
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(5.0);
                    for (i, a) in catalog
                        .assets
                        .iter()
                        .filter(|a| {
                            self.category.is_none_or(|c| c == a.category) && matches(a, &needle)
                        })
                        .enumerate()
                    {
                        let (resp, painter) =
                            ui.allocate_painter(Vec2::splat(tile), Sense::click());
                        let visible = ui.is_rect_visible(resp.rect);
                        thumb(ui, cx, &a.thumbnail, resp.rect, visible, i as f32);
                        if self.recent.first() == Some(&a.id) {
                            painter.rect_stroke(
                                resp.rect,
                                egui::CornerRadius::same(6),
                                egui::Stroke::new(3.0, ACCENT),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if resp.on_hover_text(details(a)).clicked() {
                            picked = Some(a.id.clone());
                        }
                    }
                });
            });
        if let Some(id) = &picked {
            self.recent.retain(|r| r != id);
            self.recent.insert(0, id.clone());
            self.recent.truncate(8);
        }
        picked
    }
}
