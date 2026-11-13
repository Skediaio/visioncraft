//! Shared UI pieces: the per-frame context handed to every mode, toasts, small widgets, the
//! floating bars (modes, file, view) and the layers popover.

use crate::strings as t;
use crate::studio::Studio;
use eframe::egui::{self, Align2, Color32, CornerRadius, Margin, Pos2, RichText, Vec2};
use serde_json::Value;
use std::ops::RangeInclusive;
use visioncraft_render::Quality;

pub const ACCENT: Color32 = Color32::from_rgb(255, 196, 64);
const PANEL: Color32 = Color32::from_rgba_premultiplied(14, 18, 26, 222);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Draw,
    Materials,
    Objects,
    Sun,
    Views,
    /// An extension's mode, by its id.
    Extension(&'static str),
}

pub const MODES: [Mode; 5] = [
    Mode::Draw,
    Mode::Materials,
    Mode::Objects,
    Mode::Sun,
    Mode::Views,
];

/// Pointer and key state of the viewport for one frame. Key flags are false while a text field
/// has the keyboard.
#[derive(Clone, Debug)]
pub struct Input {
    pub rect: egui::Rect,
    pub hover: Option<Pos2>,
    pub clicked: bool,
    pub double_clicked: bool,
    pub drag_started: bool,
    /// Where the primary button went down.
    pub press_origin: Option<Pos2>,
    pub dragging: bool,
    pub drag_stopped: bool,
    pub alt: bool,
    pub enter: bool,
    pub escape: bool,
    pub backspace: bool,
    pub delete: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            rect: egui::Rect::ZERO,
            hover: None,
            clicked: false,
            double_clicked: false,
            drag_started: false,
            press_origin: None,
            dragging: false,
            drag_stopped: false,
            alt: false,
            enter: false,
            escape: false,
            backspace: false,
            delete: false,
        }
    }
}

pub struct Cx<'a> {
    pub s: &'a mut Studio,
    pub toasts: &'a mut Toasts,
    pub input: &'a Input,
    pub ctx: &'a egui::Context,
}

impl Cx<'_> {
    pub fn size(&self) -> glam::Vec2 {
        glam::Vec2::new(self.input.rect.width(), self.input.rect.height())
    }

    /// Runs a command; a failure becomes a toast.
    pub fn run(&mut self, name: &str, params: Value) -> Option<Value> {
        match self.s.run(name, params) {
            Ok(v) => Some(v),
            Err(e) => {
                self.toasts.error(e.to_string());
                None
            }
        }
    }

    /// Where a world position appears, in points.
    pub fn project(&self, world: glam::Vec3) -> Option<Pos2> {
        let p = self.s.renderer.project(world)? / self.ctx.pixels_per_point();
        Some(Pos2::new(p.x, p.y))
    }

    /// A small readable label on the viewport.
    pub fn label(&self, at: Pos2, text: &str) {
        let painter = self.ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("labels"),
        ));
        let galley = painter.layout_no_wrap(
            text.to_string(),
            egui::FontId::proportional(12.5),
            Color32::WHITE,
        );
        let rect = egui::Align2::CENTER_CENTER
            .anchor_size(at, galley.size())
            .expand2(Vec2::new(5.0, 2.0));
        painter.rect_filled(rect, CornerRadius::same(5), Color32::from_black_alpha(165));
        painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
}

// ---- toasts ----

pub struct Toast {
    pub text: String,
    pub error: bool,
    /// Seconds after it first shows; `None` stays until dismissed.
    pub lifetime: Option<f64>,
    born: Option<f64>,
}

#[derive(Default)]
pub struct Toasts {
    pub list: Vec<Toast>,
}

impl Toasts {
    fn push(&mut self, text: String, error: bool, lifetime: Option<f64>) {
        if !self.list.iter().any(|t| t.text == text) {
            self.list.push(Toast {
                text,
                error,
                lifetime,
                born: None,
            });
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(text.into(), false, Some(4.0));
    }

    /// Stays until dismissed.
    pub fn warn(&mut self, text: impl Into<String>) {
        self.push(text.into(), false, None);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(text.into(), true, None);
    }

    /// Draws the list; returns true while something expires (so the app keeps repainting).
    pub fn show(&mut self, ctx: &egui::Context, top: f32) -> bool {
        let now = ctx.input(|i| i.time);
        for t in &mut self.list {
            t.born.get_or_insert(now);
        }
        self.list
            .retain(|t| t.lifetime.is_none_or(|l| now - t.born.unwrap_or(now) < l));
        let mut dismiss = None;
        if !self.list.is_empty() {
            egui::Area::new(egui::Id::new("toasts"))
                .anchor(Align2::CENTER_TOP, Vec2::new(0.0, top))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    ui.set_max_width((ctx.content_rect().width() - 24.0).min(460.0));
                    for (i, toast) in self.list.iter().enumerate() {
                        let fill = if toast.error {
                            Color32::from_rgba_premultiplied(90, 24, 24, 235)
                        } else {
                            PANEL
                        };
                        egui::Frame::new()
                            .fill(fill)
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(10, 6))
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(&toast.text).color(Color32::WHITE));
                                    if ui.small_button("x").clicked() {
                                        dismiss = Some(i);
                                    }
                                });
                            });
                        ui.add_space(4.0);
                    }
                });
        }
        if let Some(i) = dismiss {
            self.list.remove(i);
        }
        self.list.iter().any(|t| t.lifetime.is_some())
    }
}

// ---- widgets ----

pub fn apply_theme(ctx: &egui::Context) {
    // The panels are dark whatever the system theme is.
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut v = egui::Visuals::dark();
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.selection.bg_fill = Color32::from_rgb(40, 110, 140);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(8);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 20);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 5.0);
        s.spacing.button_padding = Vec2::new(9.0, 4.0);
    });
}

pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(CornerRadius::same(14))
        .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(34)))
        .inner_margin(Margin::same(10))
        .shadow(egui::epaint::Shadow {
            offset: [0, 3],
            blur: 14,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
}

/// Whether the viewport says that clay view hides the materials.
pub fn clay_notice(p: &visioncraft_core::Project) -> bool {
    p.style.clay
}

/// A floating panel for content that scrolls by itself (the planting panel sizes its own
/// scroll area from the window height).
pub fn floating_own_scroll(
    ctx: &egui::Context,
    id: &str,
    anchor: Align2,
    offset: Vec2,
    max_width: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Area::new(egui::Id::new(id))
        .anchor(anchor, offset)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_max_width(max_width);
                if !folded(ui, id) {
                    add(ui);
                }
            });
        });
}

/// The mode panel folds to its title (`MODE_PANEL_TITLE` in the context's data, set by the
/// window each frame), so it can be moved out of a drawing's way; true while folded.
pub const MODE_PANEL: &str = "modepanel";
pub const MODE_PANEL_TITLE: &str = "modepanel-title";

fn folded(ui: &mut egui::Ui, id: &str) -> bool {
    if id != MODE_PANEL {
        return false;
    }
    let ctx = ui.ctx().clone();
    let key = egui::Id::new("modepanel-folded");
    let mut folded: bool = ctx.data(|d| d.get_temp(key)).unwrap_or(false);
    let title: String = ctx
        .data(|d| d.get_temp(egui::Id::new(MODE_PANEL_TITLE)))
        .unwrap_or_default();
    // Plain signs: the panel font has no triangle glyphs.
    let label = format!("{} {title}", if folded { "+" } else { "–" });
    if ui
        .add(egui::Button::new(RichText::new(label).small().strong()).frame(false))
        .on_hover_text(if folded {
            t::PANEL_UNFOLD
        } else {
            t::PANEL_FOLD
        })
        .clicked()
    {
        folded = !folded;
        ctx.data_mut(|d| d.insert_temp(key, folded));
    }
    folded
}

/// A floating panel anchored to a corner; it scrolls when taller than the window.
pub fn floating(
    ctx: &egui::Context,
    id: &str,
    anchor: Align2,
    offset: Vec2,
    max_width: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Area::new(egui::Id::new(id))
        .anchor(anchor, offset)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_max_width(max_width);
                // A panel taller than the window scrolls instead of running off it.
                let room = (ctx.content_rect().height() - offset.y.abs() - 40.0).max(120.0);
                // An area hanging from the bottom only has the room of its last size: ask for
                // the content's height of the last frame, and paint again until it fits.
                let key = egui::Id::new(("floating-height", id));
                let last: f32 = ctx.data(|d| d.get_temp(key)).unwrap_or(0.0);
                if folded(ui, id) {
                    return;
                }
                let out = egui::ScrollArea::vertical()
                    .id_salt(("floating", id))
                    .max_height(room)
                    .min_scrolled_height(last.min(room))
                    .auto_shrink([true, true])
                    .show(ui, add);
                ctx.data_mut(|d| d.insert_temp(key, out.content_size.y));
                let want = out.content_size.y.min(room);
                if (out.inner_rect.height() - want).abs() > 1.0 {
                    ctx.request_repaint();
                }
            });
        });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    None,
    /// Value is changing: show it live.
    Live,
    /// The edit ended: commit.
    Done,
}

pub fn drag_field(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut f32,
    speed: f64,
    range: RangeInclusive<f32>,
    suffix: &str,
) -> Edit {
    let mut edit = Edit::None;
    ui.horizontal(|ui| {
        ui.label(label);
        let r = ui.add(
            egui::DragValue::new(v)
                .speed(speed)
                .range(range)
                .suffix(suffix)
                .max_decimals(3),
        );
        if r.changed() {
            edit = Edit::Live;
        }
        if r.drag_stopped() || r.lost_focus() {
            edit = Edit::Done;
        }
    });
    edit
}

/// A slider that reports live changes and the end of the edit.
pub fn slider_field(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut f32,
    range: RangeInclusive<f32>,
) -> Edit {
    let mut edit = Edit::None;
    ui.horizontal(|ui| {
        ui.label(label);
        let r = ui.add(egui::Slider::new(v, range).max_decimals(2));
        if r.changed() {
            edit = Edit::Live;
        }
        if r.drag_stopped() || r.lost_focus() {
            edit = Edit::Done;
        }
    });
    edit
}

// ---- bars ----

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open,
    Save,
    Undo,
    Redo,
    ToggleWalk,
    Mode(Option<Mode>),
    ReloadModel(String),
    Relink(String),
    SelectModel(String),
    New,
    Export(usize),
    /// Every viewpoint at export preset `usize`.
    ExportAll(usize),
}

pub struct Hud {
    pub mode: Option<Mode>,
    /// Extensions' modes after the built-in ones: (id, label).
    pub extension_modes: Vec<(&'static str, &'static str)>,
    pub show_fps: bool,
    pub layers_open: bool,
    pub export_open: bool,
    /// Where the Export button was last frame: its panel opens under it.
    pub export_at: egui::Rect,
    /// Export every viewpoint instead of the current view.
    pub export_all: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            mode: None,
            extension_modes: Vec::new(),
            show_fps: true,
            layers_open: false,
            export_open: false,
            export_at: egui::Rect::ZERO,
            export_all: false,
        }
    }
}

pub fn narrow(ctx: &egui::Context) -> bool {
    ctx.content_rect().width() < 720.0
}

fn pill(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let text = if active {
        RichText::new(label).color(Color32::BLACK).strong()
    } else {
        RichText::new(label)
    };
    ui.add(
        egui::Button::new(text)
            .corner_radius(CornerRadius::same(16))
            .fill(if active {
                ACCENT
            } else {
                Color32::from_white_alpha(18)
            }),
    )
}

impl Hud {
    /// The floating bars. `walking`/`fps` come from the app.
    pub fn show(&mut self, cx: &mut Cx, fps: f64, walking: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        let ctx = cx.ctx.clone();
        let w = ctx.content_rect().width();
        let compact = w < 520.0;

        // Bottom-left: the five modes.
        floating(
            &ctx,
            "modes",
            Align2::LEFT_BOTTOM,
            Vec2::new(10.0, -10.0),
            w - 20.0,
            |ui| {
                ui.horizontal(|ui| {
                    let built_in = crate::ui::MODES.iter().zip(t::MODE_LABELS);
                    let extensions = self
                        .extension_modes
                        .iter()
                        .map(|(id, label)| (Mode::Extension(id), (*label, *label)));
                    let modes: Vec<_> = built_in.map(|(m, l)| (*m, l)).chain(extensions).collect();
                    for (mode, (full, short)) in modes {
                        let active = self.mode == Some(mode);
                        if pill(ui, if compact { short } else { full }, active)
                            .on_hover_text(full)
                            .clicked()
                        {
                            actions.push(Action::Mode(if active { None } else { Some(mode) }));
                        }
                    }
                });
            },
        );

        // Top-left: file and history.
        floating(
            &ctx,
            "filebar",
            Align2::LEFT_TOP,
            Vec2::new(10.0, 10.0),
            w - 20.0,
            |ui| {
                ui.horizontal(|ui| {
                    if ui.button(t::OPEN).clicked() {
                        actions.push(Action::Open);
                    }
                    if ui.button(t::SAVE).clicked() {
                        actions.push(Action::Save);
                    }
                    ui.separator();
                    if ui
                        .add_enabled(cx.s.session.can_undo(), egui::Button::new(t::UNDO))
                        .clicked()
                    {
                        actions.push(Action::Undo);
                    }
                    if ui
                        .add_enabled(cx.s.session.can_redo(), egui::Button::new(t::REDO))
                        .clicked()
                    {
                        actions.push(Action::Redo);
                    }
                });
            },
        );

        // Top-right (below the file bar on a phone): the view group.
        let (anchor, offset) = if narrow(&ctx) {
            (Align2::LEFT_TOP, Vec2::new(10.0, 56.0))
        } else {
            (Align2::RIGHT_TOP, Vec2::new(-10.0, 10.0))
        };
        floating(&ctx, "viewbar", anchor, offset, w - 20.0, |ui| {
            ui.horizontal(|ui| {
                if ui.selectable_label(self.layers_open, t::LAYERS).clicked() {
                    self.layers_open = !self.layers_open;
                }
                if ui
                    .selectable_label(cx.s.lights_on, t::LIGHTS_BUTTON)
                    .clicked()
                {
                    cx.s.lights_on = !cx.s.lights_on;
                }
                if cx.s.dropped_lights > 0 {
                    ui.weak(format!("{} {}", cx.s.dropped_lights, t::LIGHTS_DROPPED));
                }
                let clay = cx.s.project().style.clay;
                if ui.selectable_label(clay, t::CLAY).clicked() {
                    cx.run("style.set", serde_json::json!({"clay": !clay}));
                }
                let name = if cx.s.quality_auto {
                    t::AUTO
                } else {
                    Quality::PRESETS[cx.s.quality].0
                };
                egui::ComboBox::from_id_salt("quality")
                    .selected_text(if compact {
                        name.to_string()
                    } else {
                        format!("{} {name}", t::QUALITY)
                    })
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(cx.s.quality_auto, t::AUTO).clicked() {
                            cx.s.quality_auto = true;
                        }
                        for (i, (name, _)) in Quality::PRESETS.iter().enumerate() {
                            if ui
                                .selectable_label(!cx.s.quality_auto && cx.s.quality == i, *name)
                                .clicked()
                            {
                                cx.s.quality = i;
                                cx.s.quality_auto = false;
                            }
                        }
                        ui.separator();
                        // Stored with the project (a command); Auto still lowers detail by fps.
                        let mut vd = cx.s.project().view.view_distance;
                        let r = ui
                            .add(
                                egui::Slider::new(&mut vd, 0.25..=4.0)
                                    .logarithmic(true)
                                    .max_decimals(2)
                                    .text(t::VIEW_DISTANCE),
                            )
                            .on_hover_text(t::VIEW_DISTANCE_TIP);
                        if r.drag_stopped() || (r.changed() && !r.dragged()) {
                            cx.run("view.set", serde_json::json!({"view_distance": vd}));
                        }
                    });
                let export = ui.selectable_label(self.export_open, t::EXPORT);
                self.export_at = export.rect;
                if export.clicked() {
                    self.export_open = !self.export_open;
                }
                if ui.selectable_label(walking, t::WALK).clicked() {
                    actions.push(Action::ToggleWalk);
                }
                if self.show_fps {
                    if ui
                        .small_button(format!("{fps:.0} {}", t::FPS))
                        .on_hover_text(format!(
                            "{} (render scale {:.0}%)",
                            t::HIDE_FPS,
                            cx.s.current_quality().render_scale * 100.0
                        ))
                        .clicked()
                    {
                        self.show_fps = false;
                    }
                } else if ui.small_button("…").on_hover_text(t::SHOW_FPS).clicked() {
                    self.show_fps = true;
                }
            });
        });

        if self.export_open {
            // Hanging under its button, right edges aligned.
            let (a, o) = if narrow(&ctx) {
                (Align2::LEFT_TOP, Vec2::new(10.0, 150.0))
            } else {
                let right = ctx.content_rect().right() - self.export_at.right();
                (
                    Align2::RIGHT_TOP,
                    Vec2::new(-(right - 8.0).max(10.0), self.export_at.bottom() + 14.0),
                )
            };
            floating(&ctx, "export", a, o, 260.0, |ui| {
                ui.label(RichText::new(t::EXPORT).strong());
                let views = cx.s.project().viewpoints.len();
                ui.add_enabled_ui(views > 0, |ui| {
                    ui.checkbox(
                        &mut self.export_all,
                        format!("{} ({views})", t::EXPORT_ALL_VIEWS),
                    )
                    .on_hover_text(t::EXPORT_ALL_TIP);
                });
                let all = self.export_all && views > 0;
                for (i, (name, w, h)) in visioncraft_core::export_presets().iter().enumerate() {
                    if ui.button(format!("{name}  {w} × {h}")).clicked() {
                        actions.push(if all {
                            Action::ExportAll(i)
                        } else {
                            Action::Export(i)
                        });
                        self.export_open = false;
                    }
                }
            });
        }
        if self.layers_open {
            let (a, o) = if narrow(&ctx) {
                (Align2::LEFT_TOP, Vec2::new(10.0, 104.0))
            } else {
                (Align2::RIGHT_TOP, Vec2::new(-10.0, 56.0))
            };
            floating(&ctx, "layers", a, o, 300.0, |ui| {
                self.layers(ui, cx, &mut actions)
            });
        }
        // Only over a bare view: a mode's panel would cover it.
        if walking && self.mode.is_none() {
            egui::Area::new(egui::Id::new("walkhint"))
                .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -64.0))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    ui.label(
                        RichText::new(t::WALK_HINT)
                            .color(Color32::WHITE)
                            .background_color(Color32::from_black_alpha(150)),
                    );
                });
        }
        actions
    }

    fn layers(&mut self, ui: &mut egui::Ui, cx: &mut Cx, actions: &mut Vec<Action>) {
        let layers = cx.s.project().layers.clone();
        for l in layers {
            let mut visible = l.visible;
            if ui.checkbox(&mut visible, &l.name).changed() {
                cx.run(
                    "layer.set_visible",
                    serde_json::json!({"id": l.id, "visible": visible}),
                );
            }
        }
        let models: Vec<(String, String, bool)> =
            cx.s.project()
                .models
                .iter()
                .map(|m| {
                    (
                        m.id.clone(),
                        m.name.clone(),
                        cx.s.loaded.contains_key(&m.id),
                    )
                })
                .collect();
        ui.separator();
        ui.checkbox(&mut cx.s.show_people, t::PERSON);
        if ui.button(t::PLACE_PERSON).clicked() {
            cx.s.show_people = true;
            cx.s.placing_person = true;
        }
        if !models.is_empty() {
            ui.separator();
            ui.label(RichText::new(t::MODELS).strong());
            for (id, name, loaded) in models {
                ui.horizontal(|ui| {
                    if ui.link(name).clicked() {
                        actions.push(Action::SelectModel(id.clone()));
                    }
                    let label = if loaded { t::RELOAD_MODEL } else { t::RELINK };
                    // Natively a changed file reloads by itself; the button is the way on the web.
                    if ui.small_button(label).clicked() {
                        actions.push(if loaded {
                            Action::ReloadModel(id)
                        } else {
                            Action::Relink(id)
                        });
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use visioncraft_core::{Project, Session};

    #[test]
    fn clay_view_is_off_by_default_and_announced_when_on() {
        let mut s = Session::new(Project::default());
        assert!(!super::clay_notice(s.project()));
        let restored = Project::from_json_bytes(&s.project().to_json_bytes()).unwrap();
        assert!(!super::clay_notice(&restored));
        s.run("style.set", serde_json::json!({"clay": true}))
            .unwrap();
        assert!(super::clay_notice(s.project()));
        s.undo();
        assert!(!super::clay_notice(s.project()));
    }
}
