//! Viewpoints mode: a strip of up to eight saved cameras with thumbnails.

use crate::strings as t;
use crate::studio::Readback;
use crate::ui::{ACCENT, Cx};
use eframe::egui::{self, Color32, Rect, Sense, Vec2};
use serde_json::json;
use std::collections::HashMap;

const MAX: usize = 8;
const THUMB: [u32; 2] = [192, 108];
const SLOT: Vec2 = Vec2::new(128.0, 72.0);

#[derive(Default)]
pub struct ViewsState {
    thumbs: HashMap<String, egui::TextureHandle>,
    jobs: Vec<String>,
    pending: Vec<(String, Readback)>,
    menu: Option<String>,
    rename: String,
    dragging: Option<usize>,
    rects: Vec<Rect>,
    /// (models loaded, plants and library settled) when the thumbnails were last made: a model or
    /// the plants that arrive later make them anew.
    settled: Option<(usize, bool)>,
}

impl ViewsState {
    pub fn queue(&mut self, id: &str) {
        self.jobs.retain(|j| j != id);
        self.jobs.push(id.to_string());
    }

    pub fn leave(&mut self) {
        self.menu = None;
        self.dragging = None;
    }

    /// Renders one queued thumbnail offscreen and collects finished ones. Call before the main
    /// frame is rendered: it borrows the renderer at thumbnail size and the caller resizes it back.
    pub fn pump(&mut self, cx: &mut Cx, device: &wgpu::Device, queue: &wgpu::Queue) {
        let have: Vec<String> =
            cx.s.project()
                .viewpoints
                .iter()
                .map(|v| v.id.clone())
                .collect();
        self.thumbs.retain(|k, _| have.contains(k));
        let now = (
            cx.s.loaded.len(),
            cx.s.plants_pending() == 0 && cx.s.scene.library.idle(),
        );
        if now.1 && self.settled.is_some_and(|s| s != now) {
            // The old pictures stay until the new ones are in.
            for id in &have {
                self.queue(id);
            }
        }
        if now.1 || self.settled.is_none() {
            self.settled = Some(now);
        }
        for id in &have {
            if !self.thumbs.contains_key(id)
                && !self.jobs.contains(id)
                && !self.pending.iter().any(|(p, _)| p == id)
            {
                self.jobs.push(id.clone());
            }
        }
        let ready: Vec<usize> = (0..self.pending.len())
            .filter(|&i| self.pending[i].1.take(device).is_some())
            .collect();
        for i in ready.into_iter().rev() {
            let (id, rb) = self.pending.remove(i);
            if let Some(px) = rb.take(device) {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [rb.size[0] as usize, rb.size[1] as usize],
                    &px,
                );
                self.thumbs.insert(
                    id.clone(),
                    cx.ctx
                        .load_texture(format!("vp-{id}"), img, egui::TextureOptions::LINEAR),
                );
            }
        }
        if self.jobs.is_empty() {
            return;
        }
        let id = self.jobs.remove(0);
        let Some(vp) =
            cx.s.project()
                .viewpoints
                .iter()
                .find(|v| v.id == id)
                .cloned()
        else {
            return;
        };
        let saved = (
            cx.s.camera.clone(),
            cx.s.time_draft,
            cx.s.size,
            std::mem::take(&mut cx.s.extra),
            cx.s.highlight.take(),
        );
        cx.s.camera.look_from(vp.position.into(), vp.target.into());
        cx.s.camera.fov_y = vp.fov_y_deg.to_radians();
        cx.s.time_draft = vp.time;
        cx.s.resize(THUMB[0], THUMB[1]);
        cx.s.prepare();
        self.pending
            .push((id, Readback::start(cx.s, device, queue)));
        (cx.s.camera, cx.s.time_draft, _, cx.s.extra, cx.s.highlight) = saved;
    }

    pub fn busy(&self) -> bool {
        !self.jobs.is_empty() || !self.pending.is_empty()
    }

    /// The camera. A view keeps no time of its own unless asked (`VIEW_KEEP_TIME`): it shows
    /// the project's season and time, so choosing a season changes every view.
    fn pose(cx: &Cx) -> serde_json::Value {
        let (p, tg) = cx.s.camera.pose();
        json!({"position": p.to_array(), "target": tg.to_array(), "fov_y_deg": cx.s.camera.fov_y.to_degrees()})
    }

    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        let now = cx.ctx.input(|i| i.time);
        let views = cx.s.project().viewpoints.clone();
        let mut fly = None;
        let mut reorder = None;
        let mut new_rects = Vec::new();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, v) in views.iter().enumerate() {
                    ui.vertical(|ui| {
                        let (r, painter) = ui.allocate_painter(SLOT, Sense::click_and_drag());
                        let rr = egui::CornerRadius::same(8);
                        match self.thumbs.get(&v.id) {
                            Some(tex) => {
                                egui::Image::new(tex)
                                    .corner_radius(rr)
                                    .fit_to_exact_size(SLOT)
                                    .paint_at(ui, r.rect);
                            }
                            None => {
                                painter.rect_filled(r.rect, rr, Color32::from_white_alpha(20));
                            }
                        }
                        let target = self.dragging.is_some()
                            && ui
                                .input(|i| i.pointer.hover_pos())
                                .is_some_and(|p| r.rect.x_range().contains(p.x));
                        let stroke = if target {
                            egui::Stroke::new(3.0, ACCENT)
                        } else {
                            egui::Stroke::new(1.0, Color32::from_white_alpha(70))
                        };
                        painter.rect_stroke(r.rect, rr, stroke, egui::StrokeKind::Inside);
                        let dots = Rect::from_min_size(
                            r.rect.right_top() + Vec2::new(-26.0, 2.0),
                            Vec2::splat(24.0),
                        );
                        let dots_r = ui.interact(dots, ui.id().with(("dots", i)), Sense::click());
                        painter.rect_filled(
                            dots,
                            egui::CornerRadius::same(6),
                            Color32::from_black_alpha(120),
                        );
                        painter.text(
                            dots.center(),
                            egui::Align2::CENTER_CENTER,
                            "…",
                            egui::FontId::proportional(14.0),
                            Color32::WHITE,
                        );
                        if dots_r.clicked() {
                            self.menu = if self.menu.as_ref() == Some(&v.id) {
                                None
                            } else {
                                Some(v.id.clone())
                            };
                            self.rename = v.name.clone();
                        } else if r.clicked() {
                            fly = Some(v.clone());
                        }
                        if r.drag_started() {
                            self.dragging = Some(i);
                        }
                        if r.drag_stopped()
                            && let (Some(from), Some(p)) =
                                (self.dragging.take(), ui.input(|i| i.pointer.interact_pos()))
                        {
                            let to = self
                                .rects
                                .iter()
                                .position(|rc| rc.x_range().contains(p.x))
                                .unwrap_or(from);
                            if to != from {
                                reorder = Some((v.id.clone(), to));
                            }
                        }
                        new_rects.push(r.rect);
                        ui.add_sized(
                            [SLOT.x, 14.0],
                            egui::Label::new(egui::RichText::new(&v.name).small()).truncate(),
                        );
                    });
                }
                if views.len() < MAX {
                    ui.vertical(|ui| {
                        let (r, painter) = ui.allocate_painter(SLOT, Sense::click());
                        painter.rect_stroke(
                            r.rect,
                            egui::CornerRadius::same(8),
                            egui::Stroke::new(1.5, Color32::from_white_alpha(90)),
                            egui::StrokeKind::Inside,
                        );
                        painter.text(
                            r.rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "+",
                            egui::FontId::proportional(28.0),
                            Color32::WHITE,
                        );
                        if r.on_hover_text(t::SAVE_VIEW).clicked() {
                            let mut p = Self::pose(cx);
                            p["name"] = json!(format!("View {}", views.len() + 1));
                            if let Some(id) = cx
                                .run("viewpoint.add", p)
                                .and_then(|r| r["id"].as_str().map(str::to_string))
                            {
                                self.queue(&id);
                            }
                        }
                        ui.add_sized(
                            [SLOT.x, 14.0],
                            egui::Label::new(egui::RichText::new(t::SAVE_VIEW).small()).truncate(),
                        );
                    });
                }
            });
        });
        self.rects = new_rects;
        if let Some(v) = fly {
            cx.s.camera.start_fly(&v, now);
            if let Some(time) = v.time
                && time != cx.s.project().time
            {
                cx.run("time.set", json!({"year": time.year, "month": time.month, "day": time.day, "hour": time.hour}));
            }
            if let Some(years) = v.years
                && Some(years) != cx.s.project().view.years
            {
                cx.run("view.set", json!({"years": years}));
            }
        }
        if let Some((id, index)) = reorder {
            cx.run("viewpoint.reorder", json!({"id": id, "index": index}));
        }
    }

    /// The small menu of the slot whose "…" was clicked.
    pub fn menu(&mut self, cx: &mut Cx) {
        let Some(id) = self.menu.clone() else { return };
        if !cx.s.project().viewpoints.iter().any(|v| v.id == id) {
            self.menu = None;
            return;
        }
        let ctx = cx.ctx.clone();
        let w = ctx.content_rect().width();
        crate::ui::floating(
            &ctx,
            "vpmenu",
            egui::Align2::LEFT_BOTTOM,
            Vec2::new(10.0, -176.0),
            (w - 20.0).min(260.0),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(t::RENAME);
                    let r =
                        ui.add(egui::TextEdit::singleline(&mut self.rename).desired_width(150.0));
                    if r.lost_focus() && !self.rename.trim().is_empty() {
                        cx.run(
                            "viewpoint.update",
                            json!({"id": id, "name": self.rename.trim()}),
                        );
                    }
                });
                // The years after planting this view shows: its own, or the project's.
                let years =
                    cx.s.project()
                        .viewpoints
                        .iter()
                        .find(|v| v.id == id)
                        .and_then(|v| v.years);
                ui.horizontal(|ui| {
                    let mut own = years.is_some();
                    if ui.checkbox(&mut own, t::VIEW_YEARS).changed() {
                        let y = own.then(|| cx.s.project().view.years.unwrap_or(0.0));
                        cx.run("viewpoint.update", json!({"id": id, "years": y}));
                    }
                    if let Some(mut y) = years
                        && ui
                            .add(
                                egui::DragValue::new(&mut y)
                                    .range(0.0..=60.0)
                                    .speed(0.2)
                                    .suffix(" y"),
                            )
                            .drag_stopped()
                    {
                        cx.run("viewpoint.update", json!({"id": id, "years": y}));
                    }
                });
                // Its own date and time (a dusk view, a sun study), or the project's.
                let kept =
                    cx.s.project()
                        .viewpoints
                        .iter()
                        .find(|v| v.id == id)
                        .is_some_and(|v| v.time.is_some());
                let mut keep = kept;
                if ui
                    .checkbox(&mut keep, t::VIEW_KEEP_TIME)
                    .on_hover_text(t::VIEW_KEEP_TIME_TIP)
                    .changed()
                {
                    let time = keep.then(|| cx.s.project().time);
                    cx.run("viewpoint.update", json!({"id": id, "time": time}));
                }
                if ui.button(t::UPDATE_VIEW).clicked() {
                    let mut p = Self::pose(cx);
                    p["id"] = json!(id);
                    if kept {
                        p["time"] = json!(cx.s.project().time);
                    }
                    if cx.run("viewpoint.update", p).is_some() {
                        self.queue(&id);
                    }
                    self.menu = None;
                }
                if ui.button(t::DELETE).clicked() {
                    cx.run("viewpoint.remove", json!({"id": id}));
                    self.menu = None;
                }
            },
        );
    }
}
