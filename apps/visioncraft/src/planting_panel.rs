//! The planting panel of Objects mode: tools, presets, palettes and layers, settings, the
//! readout, the list of plantings and the `?` overlay. Nothing here decides what a command
//! contains; that is `planting_tools`.

use crate::objects::{display_name, percentages};
use crate::planting_tools::{
    AddTarget, BRUSH_MAX, BRUSH_MIN, Form, LAYER_COLORS, PlantingTools, Tool, merge, next_seed,
    tool_of,
};
use crate::strings as t;
use crate::ui::{Cx, Edit, drag_field, panel_frame};
use eframe::egui::{self, Align2, Color32, RichText, Vec2};
use visioncraft_core::planting::Weighted;
use visioncraft_core::planting_presets;

/// 1240 as "1,240".
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn slider_edit(r: &egui::Response) -> Edit {
    if r.drag_stopped() || r.lost_focus() {
        Edit::Done
    } else if r.changed() {
        Edit::Live
    } else {
        Edit::None
    }
}

fn density_field(ui: &mut egui::Ui, label: &str, v: &mut f32) -> Edit {
    let mut edit = Edit::None;
    ui.horizontal(|ui| {
        ui.label(label);
        let speed = (f64::from(*v) * 0.02).max(0.002);
        let r = ui.add(
            egui::DragValue::new(v)
                .speed(speed)
                .range(0.001..=500.0)
                .max_decimals(3),
        );
        edit = slider_edit(&r);
        if r.changed() && !r.dragged() && !r.has_focus() {
            edit = Edit::Done;
        }
    });
    edit
}

fn checkbox(ui: &mut egui::Ui, v: &mut bool, label: &str) -> Edit {
    if ui.checkbox(v, label).changed() {
        Edit::Done
    } else {
        Edit::None
    }
}

/// What the palette's plants grow to, as a spacing to plant at: in a line their full width
/// (a hedge closes up when grown), in a bed a little closer (one plant per square that width).
fn spacing_hint(ui: &mut egui::Ui, palette: &[Weighted], line: bool) {
    let Some(w) = palette
        .iter()
        .map(|p| visioncraft_core::growth::growth_of(&p.asset, None))
        .filter(|g| g.from_table)
        .map(|g| g.mature_width_m)
        .reduce(f32::max)
    else {
        return;
    };
    let text = if line {
        format!("{} {:.2} m", t::SPACING_HINT_LINE, w * 0.6)
    } else {
        format!(
            "{} {:.1} / m²",
            t::SPACING_HINT_BED,
            1.0 / (w * 0.8).powi(2)
        )
    };
    ui.small(text).on_hover_text(t::SPACING_HINT_TIP);
}

fn palette_ui(
    ui: &mut egui::Ui,
    palette: &mut Vec<Weighted>,
    target: AddTarget,
    add_to: &mut Option<AddTarget>,
    names: &dyn Fn(&str) -> String,
) -> Edit {
    let pct = percentages(&palette.iter().map(|w| w.weight).collect::<Vec<_>>());
    let mut edit = Edit::None;
    let mut remove = None;
    for (i, w) in palette.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.add_sized([34.0, 18.0], egui::Label::new(format!("{}%", pct[i])));
            let r = ui.add(egui::Slider::new(&mut w.weight, 0.0..=10.0).show_value(false));
            edit = merge(edit, slider_edit(&r));
            ui.label(names(&w.asset));
            if ui.small_button("×").on_hover_text(t::REMOVE).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove
        && palette.len() > 1
    {
        palette.remove(i);
        edit = Edit::Done;
    }
    let open = *add_to == Some(target);
    if ui.selectable_label(open, t::PT_ADD_PLANTS).clicked() {
        *add_to = if open { None } else { Some(target) };
    }
    edit
}

fn swatch(ui: &mut egui::Ui, color: [u8; 4]) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(11.0), egui::Sense::hover());
    ui.painter().circle_filled(
        rect.center(),
        5.0,
        Color32::from_rgb(color[0], color[1], color[2]),
    );
}

fn form_ui(
    ui: &mut egui::Ui,
    form: &mut Form,
    kind: Tool,
    add_to: &mut Option<AddTarget>,
    names: &dyn Fn(&str) -> String,
    project_layers: &[(String, String)],
) -> Edit {
    let mut edit = Edit::None;
    let layered = kind.layered() && !form.layers.is_empty();
    if kind.layered() {
        // One line: a preset menu instead of eleven buttons wrapping over three.
        let current = match (&form.preset, layered) {
            (Some(p), true) => p.clone(),
            _ => t::PT_SIMPLE.to_string(),
        };
        ui.horizontal(|ui| {
            ui.label(t::PT_PRESET);
            egui::ComboBox::from_id_salt(("planting-preset", kind.label()))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(!layered, t::PT_SIMPLE).clicked() && layered {
                        form.layers.clear();
                        form.preset = None;
                        edit = Edit::Done;
                    }
                    for p in planting_presets() {
                        let on = layered && form.preset.as_deref() == Some(p.name.as_str());
                        if ui.selectable_label(on, &p.name).clicked() {
                            form.layers = p.layers;
                            form.preset = Some(p.name);
                            edit = Edit::Done;
                        }
                    }
                });
        });
    }
    if layered {
        let n = form.layers.len();
        for (i, l) in form.layers.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                swatch(ui, LAYER_COLORS[i % 4]);
                egui::CollapsingHeader::new(RichText::new(&l.name).strong())
                    .id_salt(("planting-layer", i))
                    .default_open(i == 0 && n <= 3)
                    .show(ui, |ui| {
                        edit = merge(
                            edit,
                            palette_ui(
                                ui,
                                &mut l.asset_palette,
                                AddTarget::Layer(i),
                                add_to,
                                names,
                            ),
                        );
                        edit = merge(edit, density_field(ui, t::DENSITY, &mut l.density_per_m2));
                        edit = merge(
                            edit,
                            drag_field(
                                ui,
                                t::PT_MIN_DISTANCE,
                                &mut l.min_distance_m,
                                0.05,
                                0.0..=30.0,
                                " m",
                            ),
                        );
                        if i > 0 {
                            edit = merge(
                                edit,
                                drag_field(
                                    ui,
                                    t::PT_KEEP_CLEAR,
                                    &mut l.keep_clear_m,
                                    0.05,
                                    0.0..=30.0,
                                    " m",
                                ),
                            );
                        }
                        ui.horizontal(|ui| {
                            edit = merge(
                                edit,
                                drag_field(ui, t::PT_SCALE, &mut l.scale_min, 0.01, 0.1..=10.0, ""),
                            );
                            edit = merge(
                                edit,
                                drag_field(ui, "–", &mut l.scale_max, 0.01, 0.1..=10.0, ""),
                            );
                        });
                        l.scale_max = l.scale_max.max(l.scale_min);
                        edit = merge(
                            edit,
                            checkbox(ui, &mut l.random_rotation, t::PT_RANDOM_ROTATION),
                        );
                    });
            });
        }
    } else {
        edit = merge(
            edit,
            palette_ui(ui, &mut form.palette, AddTarget::Palette, add_to, names),
        );
    }

    ui.separator();
    match kind {
        Tool::Single => {
            edit = merge(
                edit,
                drag_field(
                    ui,
                    t::PT_ROTATION,
                    &mut form.rotation,
                    1.0,
                    -360.0..=360.0,
                    "°",
                ),
            );
            edit = merge(edit, checkbox(ui, &mut form.spin, t::PT_RANDOM_ROTATION));
            let mut s = form.single_scale;
            let e = drag_field(ui, t::PT_SCALE, &mut s, 0.01, 0.1..=6.0, "");
            form.single_scale = s;
            edit = merge(edit, e);
        }
        Tool::Line => {
            ui.horizontal(|ui| {
                if ui.selectable_label(form.by_count, t::PT_COUNT).clicked() {
                    form.by_count = true;
                    edit = Edit::Done;
                }
                if ui.selectable_label(!form.by_count, t::PT_SPACING).clicked() {
                    form.by_count = false;
                    edit = Edit::Done;
                }
                if form.by_count {
                    let mut c = form.count as f32;
                    let e = drag_field(ui, "", &mut c, 0.2, 1.0..=2000.0, "");
                    form.count = c.round() as u32;
                    edit = merge(edit, e);
                } else {
                    edit = merge(
                        edit,
                        drag_field(ui, "", &mut form.every, 0.02, 0.05..=100.0, " m"),
                    );
                }
            });
            edit = merge(edit, slider(ui, t::PT_JITTER, &mut form.jitter, 0.0..=1.0));
            edit = merge(
                edit,
                drag_field(ui, t::PT_OFFSET, &mut form.offset, 0.05, -50.0..=50.0, " m"),
            );
            edit = merge(edit, checkbox(ui, &mut form.closed, t::PT_CLOSED));
            edit = merge(edit, checkbox(ui, &mut form.straight, t::PT_STRAIGHT));
            spacing_hint(ui, &form.palette, true);
        }
        Tool::Cluster => {
            let mut c = form.count as f32;
            let e = drag_field(ui, t::PT_COUNT, &mut c, 0.2, 1.0..=2000.0, "");
            form.count = c.round() as u32;
            edit = merge(edit, e);
            edit = merge(
                edit,
                drag_field(ui, t::PT_RADIUS, &mut form.radius, 0.05, 0.3..=100.0, " m"),
            );
        }
        Tool::Area | Tool::Surface | Tool::Brush => {
            if !layered {
                edit = merge(edit, density_field(ui, t::DENSITY, &mut form.density));
                edit = merge(
                    edit,
                    drag_field(
                        ui,
                        t::PT_MIN_DISTANCE,
                        &mut form.min_distance,
                        0.05,
                        0.0..=30.0,
                        " m",
                    ),
                );
            }
            if kind == Tool::Area {
                edit = merge(
                    edit,
                    slider(ui, t::PT_JITTER, &mut form.pos_jitter, 0.0..=1.0),
                );
            }
            spacing_hint(ui, &form.palette, false);
            if kind == Tool::Surface {
                edit = merge(
                    edit,
                    slider(ui, t::PT_MAX_SLOPE, &mut form.max_slope, 0.0..=90.0),
                );
            }
        }
        Tool::Select => {}
    }
    // Size spread, turning, reshuffle and layer: folded, they are rarely changed.
    if matches!(kind, Tool::Line | Tool::Area | Tool::Brush) {
        edit = merge(
            edit,
            checkbox(ui, &mut form.avoid_buildings, t::AVOID_BUILDINGS),
        );
    }
    egui::CollapsingHeader::new(RichText::new(t::PT_MORE).small())
        .id_salt(("planting-more", kind.label()))
        .default_open(false)
        .show(ui, |ui| {
            if !layered && kind != Tool::Single {
                ui.horizontal(|ui| {
                    edit = merge(
                        edit,
                        drag_field(ui, t::PT_SCALE, &mut form.scale_min, 0.01, 0.1..=10.0, ""),
                    );
                    edit = merge(
                        edit,
                        drag_field(ui, "–", &mut form.scale_max, 0.01, 0.1..=10.0, ""),
                    );
                });
                form.scale_max = form.scale_max.max(form.scale_min);
                edit = merge(edit, checkbox(ui, &mut form.spin, t::PT_RANDOM_ROTATION));
            }
            ui.horizontal(|ui| {
                if kind != Tool::Single
                    && ui
                        .button(t::PT_RESHUFFLE)
                        .on_hover_text(t::PT_RESHUFFLE_HINT)
                        .clicked()
                {
                    form.seed = next_seed(form.seed);
                    edit = Edit::Done;
                }
                let name = project_layers
                    .iter()
                    .find(|l| l.0 == form.layer)
                    .map_or(form.layer.clone(), |l| l.1.clone());
                egui::ComboBox::from_id_salt("planting-layer-choice")
                    .selected_text(format!("{} {name}", t::PT_LAYER))
                    .show_ui(ui, |ui| {
                        for (id, n) in project_layers {
                            if ui.selectable_label(form.layer == *id, n).clicked()
                                && form.layer != *id
                            {
                                form.layer.clone_from(id);
                                edit = Edit::Done;
                            }
                        }
                    });
            });
        });
    edit
}

fn slider(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> Edit {
    crate::ui::slider_field(ui, label, v, range)
}

impl PlantingTools {
    /// The tools: the planting tools, then Lights.
    pub fn tool_row(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.spacing_mut().button_padding.x = 6.0;
            for tool in Tool::ALL {
                if ui
                    .selectable_label(!self.lights && self.tool == tool, tool.label())
                    .clicked()
                {
                    self.set_tool(cx, tool);
                }
            }
            if ui.selectable_label(self.lights, t::TAB_LIGHTS).clicked() && !self.lights {
                // The plan stays: it is the mode's, not the tool's.
                let view = cx.s.camera.view;
                self.leave(cx);
                cx.s.camera.view = view;
                self.set_tool(cx, Tool::Select);
                self.lights = true;
            }
            // The plan Draw uses: placing from above lands where it looks like it does.
            let plan = cx.s.camera.view == crate::camera::View::Top;
            if ui
                .selectable_label(plan, t::PLAN_VIEW)
                .on_hover_text(t::PLAN_VIEW_TIP)
                .clicked()
            {
                cx.s.camera.view = if plan {
                    crate::camera::View::Orbit
                } else {
                    cx.s.camera.leave_walk();
                    crate::camera::View::Top
                };
            }
            if !self.lights
                && ui
                    .selectable_label(self.show_help, "?")
                    .on_hover_text(t::PT_HELP_TIP)
                    .clicked()
            {
                self.show_help = !self.show_help;
            }
        });
    }

    /// The tool row comes last, next to the mode bar: the panel grows upwards from the bottom, so
    /// the row stays put whatever the tool shows.
    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        // Room for the file bar above and the mode bar below.
        let max_h = (cx.input.rect.height() - 200.0).clamp(140.0, 640.0);
        let out = egui::ScrollArea::vertical()
            .max_height(max_h)
            .min_scrolled_height(self.panel_height.min(max_h).max(64.0))
            .auto_shrink([true, true])
            .id_salt("planting-scroll")
            .show(ui, |ui| self.body(ui, cx));
        // The panel hangs from the bottom edge and a scroll area is sized from the last frame, so
        // it takes a few frames to grow to its content: paint on until it has.
        let wanted = out.content_size.y.min(max_h);
        let growing = (out.inner_rect.height() - wanted).abs() > 1.0
            || (out.content_size.y - self.panel_height).abs() > 0.5;
        self.panel_height = out.content_size.y;
        if growing && self.grow < 40 {
            self.grow += 1;
            cx.ctx.request_repaint();
        } else if !growing {
            self.grow = 0;
        }
        ui.separator();
        self.tool_row(ui, cx);
        self.help(cx);
    }

    fn body(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        let project_layers: Vec<(String, String)> =
            cx.s.project()
                .layers
                .iter()
                .map(|l| (l.id.clone(), l.name.clone()))
                .collect();
        let selected = if self.editing() {
            self.selected
                .as_deref()
                .and_then(|id| crate::planting_tools::current(cx, id))
        } else {
            None
        };
        let kind = selected.as_ref().map_or(self.tool, tool_of);
        let (mut dup, mut del, mut paint) = (false, false, false);
        let mut unstroke: Option<String> = None;
        if let Some(g) = &selected {
            ui.horizontal(|ui| {
                ui.label(RichText::new(g.name()).strong());
                dup = ui
                    .small_button(t::PT_DUPLICATE)
                    .on_hover_text("Ctrl+D")
                    .clicked();
                del = ui.small_button(t::REMOVE).on_hover_text("Delete").clicked();
                if let visioncraft_core::Generator::MixedBrush(b) = g {
                    paint = ui.small_button(t::PT_PAINT_MORE).clicked();
                    if b.strokes.len() > 1
                        && ui
                            .small_button(t::PT_UNDO_STROKE)
                            .on_hover_text(t::PT_UNDO_STROKE_TIP)
                            .clicked()
                    {
                        unstroke = Some(b.id.clone());
                    }
                }
            });
        } else {
            ui.small(match self.tool {
                Tool::Select => t::PT_SELECT_HINT,
                Tool::Single => t::PT_SINGLE_HINT,
                Tool::Line => t::PT_LINE_HINT,
                Tool::Cluster => t::PT_CLUSTER_HINT,
                Tool::Area => t::PT_AREA_HINT,
                Tool::Surface => t::PT_SURFACE_HINT,
                Tool::Brush => t::PT_BRUSH_HINT,
            });
        }
        if self.tool == Tool::Brush && selected.is_none() {
            ui.horizontal(|ui| {
                ui.label(t::PT_RADIUS);
                ui.add(
                    egui::Slider::new(&mut self.brush_radius, BRUSH_MIN..=BRUSH_MAX)
                        .logarithmic(true)
                        .suffix(" m"),
                );
            });
            ui.checkbox(&mut self.form.keep_inside, t::KEEP_INSIDE)
                .on_hover_text(t::KEEP_INSIDE_TIP);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.erase, t::PT_ERASE);
                match self.target_name(cx) {
                    Some(n) => {
                        ui.small(format!("{} {n}", t::PT_EXTENDING));
                        if ui.small_button(t::PT_NEW_BRUSH).clicked() {
                            self.brush_target = None;
                        }
                        if let Some((id, _)) = self.brush_target.clone()
                            && ui
                                .small_button(t::PT_UNDO_STROKE)
                                .on_hover_text(t::PT_UNDO_STROKE_TIP)
                                .clicked()
                        {
                            cx.run("planting.undo_stroke", serde_json::json!({ "id": id }));
                        }
                    }
                    None => {
                        ui.small(t::PT_NEW_BRUSH_NEXT);
                    }
                }
            });
        }
        let show_form = selected.is_some() || self.tool != Tool::Select;
        if show_form {
            let names = |id: &str| display_name(cx, id);
            let editing = selected.is_some();
            let PlantingTools {
                form, edit, add_to, ..
            } = self;
            let f = match (editing, edit.as_mut()) {
                (true, Some(f)) => f,
                _ => form,
            };
            let e = form_ui(ui, f, kind, add_to, &names, &project_layers);
            if editing {
                self.finish_edit(cx, e);
            }
            if let Some((n, truncated)) = self.readout(cx) {
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        thousands(n),
                        if n == 1 { t::PT_PLANT } else { t::PT_PLANTS }
                    ))
                    .strong(),
                );
                if truncated {
                    ui.colored_label(Color32::from_rgb(255, 170, 60), t::PT_CAPPED);
                }
            }
        }
        if dup {
            self.duplicate(cx);
        }
        if del {
            self.delete_selected(cx);
        }
        if paint {
            self.paint_more(cx);
        }
        if let Some(id) = unstroke {
            cx.run("planting.undo_stroke", serde_json::json!({ "id": id }));
        }
        self.list(ui, cx);
    }

    /// All plantings: select the ones that are hard to click, hide a layer. Single plants of one
    /// kind share a row that counts them (clicking it selects the last one placed).
    fn list(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        let rows: Vec<_> =
            cx.s.project()
                .generators
                .iter()
                .map(|g| {
                    (
                        g.id().to_string(),
                        g.name().to_string(),
                        tool_of(g).label(),
                        cx.s.scene.gen_stats(g.id()).map_or(0, |s| s.count),
                        g.layer().to_string(),
                        cx.s.project().layer_visible(g.layer()),
                    )
                })
                .collect();
        let rows = group_singles(rows);
        if rows.is_empty() {
            return;
        }
        ui.separator();
        let (mut pick, mut toggle) = (None, None);
        // Folded by default (and in the Select tool open): the list is what made the panel tall.
        egui::CollapsingHeader::new(
            RichText::new(format!("{} ({})", t::PLANTINGS, rows.len())).strong(),
        )
        .id_salt("planting-list-header")
        .default_open(false)
        .open(if self.tool == Tool::Select {
            Some(true)
        } else {
            None
        })
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(110.0)
                .id_salt("planting-list")
                .show(ui, |ui| {
                    for (id, name, kind, count, layer, visible) in &rows {
                        ui.horizontal(|ui| {
                            let on = self.selected.as_deref() == Some(id.as_str());
                            if ui
                                .selectable_label(on, name)
                                .on_hover_text(format!(
                                    "{kind}, {} {}",
                                    thousands(*count),
                                    t::PT_PLANTS
                                ))
                                .clicked()
                            {
                                pick = Some(id.clone());
                            }
                            ui.small(format!("{kind} · {}", thousands(*count)));
                            let mut v = *visible;
                            if ui
                                .checkbox(&mut v, "")
                                .on_hover_text(t::PT_VISIBLE)
                                .changed()
                            {
                                toggle = Some((layer.clone(), v));
                            }
                        });
                    }
                });
        });
        if let Some((layer, v)) = toggle {
            cx.run(
                "layer.set_visible",
                serde_json::json!({"id": layer, "visible": v}),
            );
        }
        if let Some(id) = pick {
            self.set_tool(cx, Tool::Select);
            self.select_generator(cx, Some(id));
        }
    }

    /// The `?` overlay: the shortcuts of the active tool.
    fn help(&mut self, cx: &mut Cx) {
        if !self.show_help {
            return;
        }
        let tool = self.selected.as_ref().map_or(self.tool, |_| Tool::Select);
        let mut close = false;
        egui::Area::new(egui::Id::new("planting-help"))
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Tooltip)
            .show(cx.ctx, |ui| {
                panel_frame().show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{}: {}", t::PT_SHORTCUTS, tool.label()))
                                .strong(),
                        );
                        if ui.small_button("×").clicked() {
                            close = true;
                        }
                    });
                    egui::Grid::new("planting-keys")
                        .num_columns(2)
                        .show(ui, |ui| {
                            for (k, what) in shortcuts(tool) {
                                ui.label(RichText::new(k).monospace());
                                ui.label(what);
                                ui.end_row();
                            }
                        });
                });
            });
        if close {
            self.show_help = false;
        }
    }
}

/// The keys and gestures of a tool, then the ones every tool has.
pub fn shortcuts(tool: Tool) -> Vec<(&'static str, &'static str)> {
    let own: &[(&str, &str)] = match tool {
        Tool::Select => t::PT_KEYS_SELECT,
        Tool::Single => t::PT_KEYS_SINGLE,
        Tool::Line => t::PT_KEYS_LINE,
        Tool::Cluster => t::PT_KEYS_CLUSTER,
        Tool::Area => t::PT_KEYS_AREA,
        Tool::Surface => t::PT_KEYS_SURFACE,
        Tool::Brush => t::PT_KEYS_BRUSH,
    };
    own.iter().chain(t::PT_KEYS_COMMON).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_get_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1240), "1,240");
        assert_eq!(thousands(200_000), "200,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn every_tool_lists_its_keys_and_the_common_ones() {
        for tool in Tool::ALL {
            let k = shortcuts(tool);
            assert!(k.len() > t::PT_KEYS_COMMON.len(), "{tool:?}");
            assert!(k.iter().any(|(key, _)| *key == "?"));
        }
        assert!(shortcuts(Tool::Brush).iter().any(|(k, _)| k.contains('[')));
    }
}

type Row = (String, String, &'static str, usize, String, bool);

/// Rows of single plants with the same name merged: their counts add up, the last one's id stays.
fn group_singles(rows: Vec<Row>) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for row in rows {
        let single = row.2 == Tool::Single.label();
        match out
            .iter_mut()
            .find(|o| single && o.2 == row.2 && o.1 == row.1 && o.4 == row.4)
        {
            Some(o) => {
                o.3 += row.3;
                o.0 = row.0;
            }
            None => out.push(row),
        }
    }
    out
}

#[cfg(test)]
mod group_tests {
    use super::*;

    #[test]
    fn single_plants_of_one_kind_share_a_row() {
        let row = |id: &str, name: &str, kind: &'static str| {
            (
                id.to_string(),
                name.to_string(),
                kind,
                1,
                "planting".to_string(),
                true,
            )
        };
        let s = Tool::Single.label();
        let rows = group_singles(vec![
            row("g1", "Cherry", s),
            row("g2", "Hedge", Tool::Line.label()),
            row("g3", "Cherry", s),
            row("g4", "Oak", s),
            row("g5", "Cherry", s),
        ]);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].0.as_str(), rows[0].3), ("g5", 3));
    }
}
