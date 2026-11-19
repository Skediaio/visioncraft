//! The film panel of Views mode: the films and their shots, a scrubber that moves the
//! viewport's camera along the film (`visioncraft_core::film`), and play in real time. Every
//! change is a `film.*` command; the frames themselves are made by `film.render` or
//! `visioncraft-cli film`.

use crate::strings as t;
use crate::ui::Cx;
use eframe::egui::{self, Align2, Vec2};
use serde_json::{Value, json};
use visioncraft_core::film::{Env, Frame, Timeline, sun_time};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Walk,
    Orbit,
    Flyover,
}

struct Form {
    kind: Kind,
    path: String,
    speed: f32,
    eye_height: f32,
    height: f32,
    center: [f32; 3],
    radius: f32,
    from_deg: f32,
    to_deg: f32,
    duration: f32,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            kind: Kind::Walk,
            path: String::new(),
            speed: 1.2,
            eye_height: 1.6,
            height: 4.0,
            center: [0.0; 3],
            radius: 8.0,
            from_deg: 0.0,
            to_deg: 180.0,
            duration: 6.0,
        }
    }
}

impl Form {
    /// The `shot` of `film.add_shot`, or `None` when the path is not [[x, z], ...].
    fn shot(&self) -> Option<Value> {
        let path = || -> Option<Vec<[f32; 2]>> {
            let n: Vec<f32> = self
                .path
                .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().ok())
                .collect::<Option<_>>()?;
            (n.len() >= 4 && n.len().is_multiple_of(2))
                .then(|| n.chunks(2).map(|c| [c[0], c[1]]).collect())
        };
        Some(match self.kind {
            Kind::Walk => {
                json!({"kind": "walk", "path": path()?, "eye_height_m": self.eye_height, "speed_mps": self.speed})
            }
            Kind::Flyover => {
                json!({"kind": "flyover", "path": path()?, "height_m": self.height, "speed_mps": self.speed})
            }
            Kind::Orbit => {
                json!({"kind": "orbit", "center": self.center, "radius_m": self.radius, "height_m": self.height, "from_deg": self.from_deg, "to_deg": self.to_deg, "duration_s": self.duration})
            }
        })
    }
}

#[derive(Default)]
pub struct FilmUi {
    selected: Option<String>,
    t: f32,
    /// While playing: the clock when it started and the film time then.
    play: Option<(f64, f32)>,
    /// The film time playing stops at: the end of the film, or of the shot played alone.
    play_end: f32,
    /// The selected shot (clicked on the timeline or in the list, or the one playing).
    shot: Option<usize>,
    /// The timeline was dragged this frame: the camera follows `t`.
    scrubbed: bool,
    /// The timeline of the selected film at a session revision.
    cache: Option<(u64, String, Timeline)>,
    form: Form,
}

impl FilmUi {
    /// Called when Views mode is left: stops playing and shows the project's own time again.
    pub fn leave(&mut self, cx: &mut Cx) {
        self.play = None;
        cx.s.time_draft = None;
        cx.s.set_years_draft(None);
    }

    pub fn show(&mut self, cx: &mut Cx) {
        let ctx = cx.ctx.clone();
        let w = ctx.content_rect().width();
        crate::ui::floating(
            &ctx,
            "filmpanel",
            Align2::RIGHT_BOTTOM,
            Vec2::new(-10.0, -176.0),
            (w - 20.0).min(340.0),
            |ui| {
                egui::CollapsingHeader::new(t::FILM)
                    .id_salt("film-header")
                    .default_open(false)
                    .show(ui, |ui| self.panel(ui, cx));
            },
        );
    }

    fn ensure_timeline(&mut self, cx: &Cx, id: &str) {
        let revision = cx.s.session.revision();
        if self
            .cache
            .as_ref()
            .is_some_and(|(r, i, _)| *r == revision && i == id)
        {
            return;
        }
        self.cache = None;
        let project = cx.s.project();
        let Some(film) = project.films.iter().find(|f| f.id == id) else {
            return;
        };
        let g = cx.s.scene.ground_fn();
        let ground = |x: f32, z: f32| g(x, z).unwrap_or(0.0);
        if let Ok(tl) = Timeline::new(film, &Env::new(project, &ground)) {
            self.cache = Some((revision, id.to_string(), tl));
        }
    }

    fn apply(cx: &mut Cx, f: &Frame) {
        let o = &f.overrides;
        cx.s.camera.cancel_fly();
        cx.s.camera
            .look_from(glam::Vec3::from(f.eye), glam::Vec3::from(f.target));
        cx.s.camera.fov_y = f.fov_y_deg.to_radians();
        let own_time = o.time_of_day.is_some() || o.date.is_some() || o.season.is_some();
        cx.s.time_draft =
            own_time.then(|| sun_time(cx.s.project().time, o, cx.s.project().site.latitude));
        cx.s.set_years_draft(o.years);
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        let films: Vec<(String, String)> =
            cx.s.project()
                .films
                .iter()
                .map(|f| (f.id.clone(), f.name.clone()))
                .collect();
        if !films
            .iter()
            .any(|(id, _)| self.selected.as_ref() == Some(id))
        {
            self.selected = films.first().map(|f| f.0.clone());
            self.play = None;
        }
        ui.horizontal(|ui| {
            if !films.is_empty() {
                let shown = films
                    .iter()
                    .find(|f| Some(&f.0) == self.selected.as_ref())
                    .map_or("", |f| f.1.as_str());
                egui::ComboBox::from_id_salt("film-pick")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (id, name) in &films {
                            if ui
                                .selectable_value(&mut self.selected, Some(id.clone()), name)
                                .clicked()
                            {
                                self.play = None;
                                self.shot = None;
                                self.t = 0.0;
                            }
                        }
                    });
            }
            if ui.button(t::FILM_NEW).clicked()
                && let Some(r) = cx.run(
                    "film.add",
                    json!({"name": format!("Film {}", films.len() + 1)}),
                )
            {
                self.selected = r["id"].as_str().map(str::to_string);
                self.shot = None;
                self.t = 0.0;
            }
            if let Some(id) = self.selected.clone()
                && ui.button(t::FILM_DELETE).clicked()
            {
                self.play = None;
                cx.run("film.remove", json!({"id": id}));
            }
        });
        let Some(id) = self.selected.clone() else {
            ui.small(t::FILM_NONE);
            return;
        };
        self.ensure_timeline(cx, &id);
        let shots: Vec<String> =
            cx.s.project()
                .films
                .iter()
                .find(|f| f.id == id)
                .map(|f| f.shots.iter().map(|s| s.kind.name().to_string()).collect())
                .unwrap_or_default();
        let spans = self.cache.as_ref().map(|c| c.2.spans()).unwrap_or_default();
        let duration = self.cache.as_ref().map_or(0.0, |c| c.2.duration_s);
        if shots.is_empty() {
            ui.small(t::FILM_NO_SHOTS);
        }
        if self.shot.is_some_and(|i| i >= shots.len()) {
            self.shot = None;
        }
        // The shots on the timeline: click one to select it and jump to its start.
        let mut jump = None;
        if duration > 0.0 {
            jump = self.strip(ui, &shots, &spans, duration);
        }
        let mut remove = None;
        for (i, kind) in shots.iter().enumerate() {
            ui.horizontal(|ui| {
                let (start, d) = spans.get(i).copied().unwrap_or_default();
                let label = format!("{}. {kind}  {start:.1}-{:.1} s", i + 1, start + d);
                if ui.selectable_label(self.shot == Some(i), label).clicked() {
                    jump = Some(i);
                }
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = jump
            && let Some(&(start, _)) = spans.get(i)
        {
            self.shot = Some(i);
            self.t = start;
            self.play_end = duration;
            if self.play.is_some() {
                self.play = Some((cx.ctx.input(|i| i.time), start));
            }
            if let Some(f) = self.cache.as_ref().and_then(|c| c.2.camera_at(start).ok()) {
                Self::apply(cx, &f);
            }
        }
        if let Some(i) = remove {
            self.play = None;
            self.shot = None;
            cx.run("film.remove_shot", json!({"id": id, "index": i}));
        }
        // The scrubber and play.
        if duration > 0.0 {
            let now = cx.ctx.input(|i| i.time);
            let mut moved = std::mem::take(&mut self.scrubbed);
            if let Some((start, t0)) = self.play {
                let end = self.play_end.min(duration);
                self.t = (t0 + (now - start) as f32).min(end);
                moved = true;
                self.shot = shot_at(&spans, self.t).or(self.shot);
                if self.t >= end {
                    self.play = None;
                }
                cx.ctx.request_repaint();
            }
            ui.horizontal(|ui| {
                let label = if self.play.is_some() {
                    t::FILM_PAUSE
                } else {
                    t::FILM_PLAY
                };
                if ui.button(label).clicked() {
                    self.play = if self.play.is_some() {
                        None
                    } else {
                        if self.t >= duration {
                            self.t = 0.0;
                        }
                        self.play_end = duration;
                        Some((now, self.t))
                    };
                }
                // The selected shot alone, or from it to the end of the film.
                if let Some(&(start, d)) = self.shot.and_then(|i| spans.get(i)) {
                    if ui.button(t::FILM_PLAY_SHOT).clicked() {
                        self.t = start;
                        self.play_end = start + d;
                        self.play = Some((now, start));
                    }
                    if ui.button(t::FILM_PLAY_FROM_SHOT).clicked() {
                        self.t = start;
                        self.play_end = duration;
                        self.play = Some((now, start));
                    }
                }
                let r = ui.add(
                    egui::Slider::new(&mut self.t, 0.0..=duration)
                        .suffix(" s")
                        .max_decimals(1),
                );
                if r.changed() {
                    self.play = self.play.map(|_| (now, self.t));
                    self.shot = shot_at(&spans, self.t).or(self.shot);
                    moved = true;
                }
            });
            if moved && let Some(f) = self.cache.as_ref().and_then(|c| c.2.camera_at(self.t).ok()) {
                Self::apply(cx, &f);
            }
        }
        ui.separator();
        if ui.button(t::FILM_HOLD_HERE).clicked() {
            let (eye, target) = cx.s.camera.pose();
            cx.run(
                "film.add_shot",
                json!({"id": id, "shot": {"kind": "hold", "eye": eye.to_array(), "target": target.to_array(), "duration_s": 3.0, "fov_y_deg": cx.s.camera.fov_y.to_degrees()}}),
            );
        }
        self.form(ui, cx, &id);
        ui.small(t::FILM_RENDER_HINT);
    }

    /// The timeline: each shot a block as wide as it is long, the playhead on it. A click
    /// returns the shot it is on; a drag scrubs (`scrubbed`).
    fn strip(
        &mut self,
        ui: &mut egui::Ui,
        kinds: &[String],
        spans: &[(f32, f32)],
        duration: f32,
    ) -> Option<usize> {
        let size = Vec2::new(ui.available_width().max(60.0), 34.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let response = response.on_hover_text(t::FILM_STRIP_TIP);
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        for (i, &(start, d)) in spans.iter().enumerate() {
            let [x0, x1] = block_x(start, d, duration, rect.left(), rect.width());
            let block = egui::Rect::from_min_max(
                egui::pos2(x0, rect.top()),
                egui::pos2((x1 - 1.0).max(x0 + 1.0), rect.bottom()),
            );
            let selected = self.shot == Some(i);
            let fill = if selected {
                visuals.selection.bg_fill
            } else if i % 2 == 0 {
                visuals.widgets.inactive.bg_fill
            } else {
                visuals.widgets.noninteractive.bg_fill
            };
            painter.rect_filled(block, 2.0, fill);
            if block.width() > 16.0 {
                painter.with_clip_rect(block).text(
                    block.left_center() + Vec2::new(4.0, 0.0),
                    Align2::LEFT_CENTER,
                    format!("{} {}", i + 1, kinds.get(i).map_or("", String::as_str)),
                    egui::FontId::proportional(12.0),
                    visuals.text_color(),
                );
            }
        }
        let x = rect.left() + rect.width() * (self.t / duration).clamp(0.0, 1.0);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, visuals.strong_text_color()),
        );
        let time_at = |p: egui::Pos2| time_at_x(p.x, rect.left(), rect.width(), duration);
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            return shot_at(spans, time_at(p));
        }
        if response.dragged()
            && let Some(p) = response.interact_pointer_pos()
        {
            self.t = time_at(p);
            self.shot = shot_at(spans, self.t).or(self.shot);
            self.play = self.play.map(|_| (ui.input(|i| i.time), self.t));
            self.scrubbed = true;
        }
        None
    }

    fn form(&mut self, ui: &mut egui::Ui, cx: &mut Cx, id: &str) {
        let f = &mut self.form;
        ui.horizontal(|ui| {
            for (kind, label) in [
                (Kind::Walk, t::FILM_WALK),
                (Kind::Orbit, t::FILM_ORBIT),
                (Kind::Flyover, t::FILM_FLYOVER),
            ] {
                if ui.selectable_label(f.kind == kind, label).clicked() && f.kind != kind {
                    f.kind = kind;
                    f.speed = if kind == Kind::Flyover { 8.0 } else { 1.2 };
                    f.height = if kind == Kind::Flyover { 15.0 } else { 4.0 };
                }
            }
        });
        let drag = |ui: &mut egui::Ui, label: &str, v: &mut f32, suffix: &str| {
            ui.horizontal(|ui| {
                ui.label(label);
                ui.add(egui::DragValue::new(v).speed(0.1).suffix(suffix));
            });
        };
        match f.kind {
            Kind::Walk | Kind::Flyover => {
                ui.label(t::FILM_PATH);
                ui.add(egui::TextEdit::singleline(&mut f.path).desired_width(260.0));
                drag(ui, t::FILM_SPEED, &mut f.speed, " m/s");
                if f.kind == Kind::Walk {
                    drag(ui, t::FILM_EYE_HEIGHT, &mut f.eye_height, " m");
                } else {
                    drag(ui, t::FILM_HEIGHT, &mut f.height, " m");
                }
            }
            Kind::Orbit => {
                ui.horizontal(|ui| {
                    ui.label(t::FILM_CENTER);
                    for c in &mut f.center {
                        ui.add(egui::DragValue::new(c).speed(0.1));
                    }
                });
                if ui.small_button(t::FILM_CENTER_HERE).clicked() {
                    f.center = cx.s.camera.pose().1.to_array();
                }
                drag(ui, t::FILM_RADIUS, &mut f.radius, " m");
                drag(ui, t::FILM_HEIGHT, &mut f.height, " m");
                drag(ui, t::FILM_FROM_DEG, &mut f.from_deg, "°");
                drag(ui, t::FILM_TO_DEG, &mut f.to_deg, "°");
                ui.small(t::FILM_ORBIT_TIP);
                drag(ui, t::FILM_DURATION, &mut f.duration, " s");
            }
        }
        if ui.button(t::FILM_ADD_SHOT).clicked() {
            match f.shot() {
                Some(shot) => {
                    cx.run("film.add_shot", json!({"id": id, "shot": shot}));
                }
                None => cx.toasts.error(t::FILM_PATH_BAD.to_string()),
            }
        }
    }
}

/// The shot playing at film time `t`: the one whose span holds it (a boundary belongs to the
/// next shot, the end of the film to the last).
fn shot_at(spans: &[(f32, f32)], t: f32) -> Option<usize> {
    let last = spans.len().checked_sub(1)?;
    Some(
        spans
            .partition_point(|&(start, d)| start + d <= t)
            .min(last),
    )
}

/// The film time under screen `x` of a strip from `left`, `width` wide, of a film `duration` long.
fn time_at_x(x: f32, left: f32, width: f32, duration: f32) -> f32 {
    ((x - left) / width.max(1.0)).clamp(0.0, 1.0) * duration
}

/// The screen `[x0, x1]` of a shot starting at `start` and lasting `d`, on the same strip.
fn block_x(start: f32, d: f32, duration: f32, left: f32, width: f32) -> [f32; 2] {
    let at = |t: f32| left + width * (t / duration.max(1e-3)).clamp(0.0, 1.0);
    [at(start), at(start + d)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPANS: [(f32, f32); 3] = [(0.0, 4.0), (4.0, 2.0), (6.0, 6.0)];

    #[test]
    fn a_time_belongs_to_the_shot_that_holds_it() {
        assert_eq!(shot_at(&SPANS, 0.0), Some(0));
        assert_eq!(shot_at(&SPANS, 3.9), Some(0));
        assert_eq!(shot_at(&SPANS, 4.0), Some(1));
        assert_eq!(shot_at(&SPANS, 11.0), Some(2));
        assert_eq!(shot_at(&SPANS, 99.0), Some(2), "past the end: the last");
        assert_eq!(shot_at(&[], 1.0), None);
    }

    #[test]
    fn the_strip_maps_times_to_blocks_and_back() {
        // A 12 s film on 240 px from x = 10: 20 px a second.
        let [x0, x1] = block_x(4.0, 2.0, 12.0, 10.0, 240.0);
        assert_eq!((x0, x1), (90.0, 130.0));
        assert_eq!(time_at_x(90.0, 10.0, 240.0, 12.0), 4.0);
        assert_eq!(
            time_at_x(-5.0, 10.0, 240.0, 12.0),
            0.0,
            "left of it: the start"
        );
        assert_eq!(
            time_at_x(999.0, 10.0, 240.0, 12.0),
            12.0,
            "right of it: the end"
        );
        // A click on a block's middle lands in that shot.
        let click = (x0 + x1) / 2.0;
        assert_eq!(
            shot_at(&SPANS, time_at_x(click, 10.0, 240.0, 12.0)),
            Some(1)
        );
        // The blocks tile the strip.
        let last = block_x(6.0, 6.0, 12.0, 10.0, 240.0);
        assert_eq!(last[1], 250.0);
    }
}
