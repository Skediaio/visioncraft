//! The plan underlay in the window: choosing an image, its opacity and visibility, and the two
//! ways of fitting it to the drawing (a typed distance between two image points, or two image
//! points and the two drawing points they must land on). Every change is an `underlay.*` or
//! `layer.*` command; this file only collects the clicks and numbers.

use crate::draw::snap_point;
use crate::scene::line;
use crate::strings as t;
use crate::ui::{Cx, Edit, slider_field};
use eframe::egui;
use glam::Vec3;
use serde_json::json;
use visioncraft_core::Underlay;
use visioncraft_render::LineVertex;

type P2 = [f32; 2];

/// The file name an imported image gets inside a project or bundle: `underlays/<name>.png|jpg`,
/// not used by another underlay.
pub fn file_for(name: &str, bytes: &[u8], taken: &[Underlay]) -> String {
    let stem = crate::files::file_stem(name, "plan");
    let ext = if bytes.starts_with(&[0xFF, 0xD8]) {
        "jpg"
    } else {
        "png"
    };
    let mut n = 1;
    loop {
        let f = if n == 1 {
            format!("underlays/{stem}.{ext}")
        } else {
            format!("underlays/{stem}-{n}.{ext}")
        };
        if !taken.iter().any(|u| u.file == f) {
            return f;
        }
        n += 1;
    }
}

/// The file name an imported wall photo gets: `photos/<name>.png|jpg`, not in `taken` (the
/// project's image files).
pub fn photo_file_for(name: &str, bytes: &[u8], taken: &[&str]) -> String {
    let stem = crate::files::file_stem(name, "photo");
    let ext = if bytes.starts_with(&[0xFF, 0xD8]) {
        "jpg"
    } else {
        "png"
    };
    let mut n = 1;
    loop {
        let f = if n == 1 {
            format!("photos/{stem}.{ext}")
        } else {
            format!("photos/{stem}-{n}.{ext}")
        };
        if !taken.contains(&f.as_str()) {
            return f;
        }
        n += 1;
    }
}

/// Where the person is in fitting the image.
#[derive(Clone, Debug, Default, PartialEq)]
enum Step {
    #[default]
    Idle,
    /// Image points picked so far (pixels), then the typed distance.
    Scale(Vec<P2>),
    /// Image points (pixels), then the drawing points they must land on.
    Calibrate(Vec<P2>, Vec<P2>),
}

#[derive(Default)]
pub struct PlanState {
    selected: Option<String>,
    step: Step,
    distance: String,
}

const MARK: [u8; 4] = [255, 90, 160, 255];
const TARGET: [u8; 4] = [120, 230, 140, 255];
const Y: f32 = 0.06;

fn world(p: P2) -> Vec3 {
    Vec3::new(p[0], Y, p[1])
}

impl PlanState {
    /// Called when the plan tool is left or Draw closes.
    pub fn cancel(&mut self, cx: &mut Cx) {
        self.step = Step::Idle;
        self.distance.clear();
        cx.s.underlay_live = None;
    }

    /// The selected underlay (the first when none is), if any.
    fn current<'a>(&mut self, underlays: &'a [Underlay]) -> Option<&'a Underlay> {
        if let Some(id) = &self.selected
            && let Some(u) = underlays.iter().find(|u| &u.id == id)
        {
            return Some(u);
        }
        let first = underlays.first()?;
        self.selected = Some(first.id.clone());
        Some(first)
    }

    /// Selects an underlay (after an import).
    pub fn select(&mut self, id: String) {
        self.selected = Some(id);
        self.step = Step::Idle;
    }

    /// Viewport clicks and overlays for one frame of the plan tool.
    pub fn update(
        &mut self,
        cx: &mut Cx,
        cursor: Option<P2>,
        radius: f32,
        top: &mut Vec<LineVertex>,
    ) {
        let underlays = cx.s.project().underlays.clone();
        let Some(u) = self.current(&underlays).cloned() else {
            self.step = Step::Idle;
            return;
        };
        // The image's outline, so it can be told from the drawing.
        let c = u.corners();
        for i in 0..4 {
            top.extend(line(
                world(c[i]),
                world(c[(i + 1) % 4]),
                [255, 90, 160, 110],
                1.5,
            ));
        }
        if cx.input.escape && self.step != Step::Idle {
            self.step = Step::Idle;
            self.distance.clear();
            return;
        }
        let placement = u.placement;
        let snapped = |cx: &Cx, p: P2| {
            let existing: Vec<P2> =
                cx.s.project()
                    .shapes
                    .iter()
                    .flat_map(|s| s.points().iter().copied())
                    .collect();
            snap_point(p, &[], &existing, radius, !cx.input.alt).pos
        };
        if cx.input.clicked
            && let Some(at) = cursor
        {
            let mut done = None;
            match &mut self.step {
                Step::Idle => {}
                Step::Scale(pixels) if pixels.len() < 2 => {
                    pixels.push(placement.world_to_pixel(at));
                }
                Step::Scale(_) => {}
                Step::Calibrate(pixels, _) if pixels.len() < 2 => {
                    pixels.push(placement.world_to_pixel(at));
                }
                Step::Calibrate(pixels, worlds) if worlds.len() < 2 => {
                    worlds.push(snapped(cx, at));
                    if worlds.len() == 2 {
                        done = Some((pixels.clone(), worlds.clone()));
                    }
                }
                Step::Calibrate(..) => {}
            }
            if let Some((p, w)) = done {
                self.step = Step::Idle;
                cx.run(
                    "underlay.calibrate",
                    json!({"id": u.id, "pixel_a": p[0], "pixel_b": p[1],
                           "world_a": w[0], "world_b": w[1]}),
                );
            }
        }
        // Marks: the picked image points where they lie now, the drawing points they go to.
        let (pixels, worlds): (&[P2], &[P2]) = match &self.step {
            Step::Idle => (&[], &[]),
            Step::Scale(p) => (p, &[]),
            Step::Calibrate(p, w) => (p, w),
        };
        let marks: Vec<P2> = pixels
            .iter()
            .map(|p| placement.pixel_to_world(*p))
            .collect();
        if let [a, b] = marks[..] {
            top.extend(line(world(a), world(b), MARK, 2.0));
        }
        for m in &marks {
            top.extend(line(world(*m), world(*m), MARK, 10.0));
        }
        for w in worlds {
            top.extend(line(world(*w), world(*w), TARGET, 10.0));
        }
        if let [a, b] = marks[..]
            && let (Some(pa), Some(pb)) = (cx.project(world(a)), cx.project(world(b)))
        {
            let px = ((pixels[0][0] - pixels[1][0]).powi(2)
                + (pixels[0][1] - pixels[1][1]).powi(2))
            .sqrt();
            let m = placement.metres_per_pixel * px;
            cx.label((pa + pb.to_vec2()) / 2.0, &format!("{px:.0} px = {m:.2} m"));
        }
        // The pointer, where the next click would land.
        if let Some(at) = cursor {
            let next = match &self.step {
                Step::Calibrate(p, w) if p.len() == 2 && w.len() < 2 => {
                    Some((snapped(cx, at), TARGET))
                }
                Step::Scale(p) if p.len() < 2 => Some((at, MARK)),
                Step::Calibrate(p, _) if p.len() < 2 => Some((at, MARK)),
                _ => None,
            };
            if let Some((p, color)) = next {
                top.extend(line(world(p), world(p), color, 6.0));
            }
        }
    }

    /// The plan panel above the tool row.
    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        let underlays = cx.s.project().underlays.clone();
        if underlays.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for u in &underlays {
                    let on = self.selected.as_deref() == Some(&u.id);
                    if ui.selectable_label(on, &u.name).clicked() {
                        self.select(u.id.clone());
                    }
                }
            });
        }
        match self.current(&underlays).cloned() {
            None => {
                ui.weak(t::PLAN_NONE);
            }
            Some(u) => self.underlay_panel(ui, cx, &u),
        }
        if ui.button(t::PLAN_ADD).clicked() {
            cx.s.pick_underlay = true;
        }
    }

    fn underlay_panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx, u: &Underlay) {
        let layer_visible =
            cx.s.project()
                .layers
                .iter()
                .find(|l| l.id == u.layer)
                .is_none_or(|l| l.visible);
        ui.horizontal(|ui| {
            ui.strong(&u.name);
            let mut show = layer_visible;
            if ui.checkbox(&mut show, t::PLAN_SHOW).changed() {
                cx.run("layer.set_visible", json!({"id": u.layer, "visible": show}));
            }
            if ui.button(t::PLAN_REMOVE).clicked() {
                self.step = Step::Idle;
                cx.run("underlay.remove", json!({"id": u.id}));
            }
        });
        if cx.s.scene.library.store.provided(&u.file).is_none() {
            ui.colored_label(egui::Color32::from_rgb(255, 170, 60), t::PLAN_FILE_MISSING);
        }
        let mut opacity =
            cx.s.underlay_live
                .as_ref()
                .filter(|(id, _)| *id == u.id)
                .map_or(u.opacity, |(_, v)| *v);
        match slider_field(ui, t::PLAN_OPACITY, &mut opacity, 0.0..=1.0) {
            Edit::Live => cx.s.underlay_live = Some((u.id.clone(), opacity)),
            Edit::Done => {
                cx.s.underlay_live = None;
                if (opacity - u.opacity).abs() > 1e-4 {
                    cx.run("underlay.set", json!({"id": u.id, "opacity": opacity}));
                }
            }
            Edit::None => {}
        }
        ui.small(format!(
            "{:.4} {}, {:.1}°",
            u.placement.metres_per_pixel,
            t::PLAN_SCALE_NOW,
            u.placement.rotation_deg
        ));
        ui.horizontal(|ui| {
            let scaling = matches!(self.step, Step::Scale(_));
            let calibrating = matches!(self.step, Step::Calibrate(..));
            if ui
                .selectable_label(scaling, t::PLAN_SET_SCALE)
                .on_hover_text(t::PLAN_SET_SCALE_TIP)
                .clicked()
            {
                self.step = if scaling {
                    Step::Idle
                } else {
                    Step::Scale(Vec::new())
                };
                self.distance.clear();
            }
            if ui
                .selectable_label(calibrating, t::PLAN_CALIBRATE)
                .on_hover_text(t::PLAN_CALIBRATE_TIP)
                .clicked()
            {
                self.step = if calibrating {
                    Step::Idle
                } else {
                    Step::Calibrate(Vec::new(), Vec::new())
                };
            }
        });
        let hint = match &self.step {
            Step::Idle => t::PLAN_HINT_IDLE,
            Step::Scale(p) if p.is_empty() => t::PLAN_HINT_A,
            Step::Scale(p) if p.len() == 1 => t::PLAN_HINT_B,
            Step::Scale(_) => t::PLAN_HINT_DISTANCE,
            Step::Calibrate(p, _) if p.is_empty() => t::PLAN_HINT_A,
            Step::Calibrate(p, _) if p.len() == 1 => t::PLAN_HINT_B,
            Step::Calibrate(_, w) if w.is_empty() => t::PLAN_HINT_WORLD_A,
            Step::Calibrate(..) => t::PLAN_HINT_WORLD_B,
        };
        if let Step::Scale(p) = &self.step
            && p.len() == 2
        {
            let p = p.clone();
            let mut go = false;
            ui.horizontal(|ui| {
                ui.label(t::PLAN_DISTANCE);
                let r = ui.add(egui::TextEdit::singleline(&mut self.distance).desired_width(70.0));
                ui.label("m");
                go = ui.button(t::PLAN_APPLY).clicked()
                    || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            });
            if go {
                match self.distance.trim().replace(',', ".").parse::<f32>() {
                    Ok(d) if d > 0.0 => {
                        cx.run(
                            "underlay.scale_by_distance",
                            json!({"id": u.id, "pixel_a": p[0], "pixel_b": p[1], "distance_m": d}),
                        );
                        self.step = Step::Idle;
                        self.distance.clear();
                    }
                    _ => cx.toasts.error(t::PLAN_HINT_DISTANCE.to_string()),
                }
            }
        }
        if self.step != Step::Idle && ui.button(t::PLAN_CANCEL).clicked() {
            self.step = Step::Idle;
            self.distance.clear();
        }
        ui.small(hint);
    }
}
