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
/// The catalogue's light fixtures (assets that carry a light) as named tiles: the lamps for a
/// garden first, the indoor ones (chandeliers, desk lamps) folded under their own heading.
/// Returns the one clicked.
pub fn fixtures(ui: &mut egui::Ui, cx: &mut Cx) -> Option<String> {
    let catalog = cx.s.scene.library.catalog.clone()?;
    const OUTSIDE: [&str; 7] = [
        "street", "lamppost", "lantern", "exterior", "security", "wall", "garden",
    ];
    // Words that make a lamp an indoor one even when it is called a lantern.
    const INSIDE: [&str; 5] = ["chandelier", "candle", "desk", "ceiling", "diya"];
    let has = |a: &visioncraft_core::Asset, words: &[&str]| {
        words
            .iter()
            .any(|w| a.id.contains(w) || a.tags.iter().any(|t| t.contains(w)))
    };
    let outside = |a: &visioncraft_core::Asset| has(a, &OUTSIDE) && !has(a, &INSIDE);
    let (garden, indoor): (Vec<&visioncraft_core::Asset>, Vec<_>) = catalog
        .assets
        .iter()
        .filter(|a| a.light.is_some())
        .partition(|a| outside(a));
    let mut picked = fixture_tiles(ui, cx, &garden);
    egui::CollapsingHeader::new(t::INDOOR_LIGHTS)
        .default_open(false)
        .show(ui, |ui| {
            picked = picked.take().or(fixture_tiles(ui, cx, &indoor));
        });
    picked
}

fn fixture_tiles(
    ui: &mut egui::Ui,
    cx: &mut Cx,
    lamps: &[&visioncraft_core::Asset],
) -> Option<String> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for a in lamps {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(54.0, 66.0), Sense::click());
            let tile = egui::Rect::from_min_size(rect.min, Vec2::splat(54.0));
            let vis = ui.is_rect_visible(rect);
            thumb(ui, cx, &a.thumbnail, tile, vis, 0.0);
            if vis {
                let mut job = egui::text::LayoutJob::simple_singleline(
                    a.name.clone(),
                    egui::FontId::proportional(9.5),
                    ui.visuals().text_color(),
                );
                job.wrap = egui::text::TextWrapping::truncate_at_width(56.0);
                let galley = ui.painter().layout_job(job);
                let at = egui::pos2(tile.center().x - galley.size().x / 2.0, tile.max.y + 1.0);
                ui.painter().galley(at, galley, ui.visuals().text_color());
            }
            if resp.on_hover_text(&a.name).clicked() {
                picked = Some(a.id.clone());
            }
        }
    });
    picked
}

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
    /// An imported object being renamed: (object id, the new name so far).
    renaming: Option<(String, String)>,
}

impl Default for LibraryUi {
    fn default() -> Self {
        Self {
            tab: Tab::Plants,
            lights: LightsTool::default(),
            search: String::new(),
            category: None,
            recent: Vec::new(),
            renaming: None,
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
        if let Some((id, name)) = &mut self.renaming {
            let (mut ok, mut cancel) = (false, false);
            ui.horizontal(|ui| {
                let r = ui.text_edit_singleline(name);
                ok = ui.button(t::RENAME).clicked()
                    || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                cancel = ui.button(t::CANCEL).clicked();
            });
            if ok && !name.trim().is_empty() {
                cx.run(
                    "object.rename",
                    serde_json::json!({"id": id, "name": name.trim()}),
                );
                cancel = true;
            }
            if cancel {
                self.renaming = None;
            }
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
                        let user = a.id.strip_prefix("user:").map(str::to_string);
                        let tip = if user.is_some() {
                            format!("{}\n{}", details(a), t::OBJECT_MENU_HINT)
                        } else {
                            details(a)
                        };
                        let resp = resp.on_hover_text(tip);
                        if resp.clicked() {
                            picked = Some(a.id.clone());
                        }
                        // Imported objects: rename or remove (refused, with the reason, in use).
                        if let Some(id) = user {
                            resp.context_menu(|ui| {
                                if ui.button(t::RENAME).clicked() {
                                    self.renaming = Some((id.clone(), a.name.clone()));
                                    ui.close();
                                }
                                if ui.button(t::REMOVE).clicked() {
                                    cx.run("object.remove", serde_json::json!({ "id": id }));
                                    ui.close();
                                }
                            });
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
