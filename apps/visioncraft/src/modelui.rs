//! The scale prompt after an import of an odd-sized model, and the model panel.

use crate::strings as t;
use crate::ui::{Action, Cx, Edit, drag_field, floating};
use eframe::egui::{self, Align2, Vec2};
use serde_json::json;

/// A model whose largest side is outside this range (metres) gets the prompt.
pub const SENSIBLE: (f32, f32) = (0.5, 200.0);

pub fn needs_prompt(largest_m: f32) -> bool {
    !(SENSIBLE.0..=SENSIBLE.1).contains(&largest_m)
}

/// "12 cm" below a metre, "3.4 m" otherwise.
pub fn length_text(m: f32) -> String {
    if m < 1.0 {
        format!("{:.0} cm", m * 100.0)
    } else {
        format!("{m:.1} m")
    }
}

#[derive(Default)]
pub struct ModelUi {
    pub selected: Option<String>,
    prompt: Option<String>,
}

impl ModelUi {
    /// Called after a model was added: asks about scale when it is tiny or huge, and stands a figure by it.
    pub fn after_import(&mut self, cx: &mut Cx, id: &str) {
        cx.s.person_near(id);
        if let Some((lo, hi)) = cx.s.model_bounds(id) {
            let s =
                cx.s.project()
                    .models
                    .iter()
                    .find(|m| m.id == id)
                    .map_or(1.0, |m| m.transform.scale);
            let largest = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f32::max) * s;
            if needs_prompt(largest) {
                self.prompt = Some(id.to_string());
            }
        }
        self.selected = Some(id.to_string());
    }

    fn scale_by(cx: &mut Cx, id: &str, factor: f32) {
        let Some(m) = cx.s.project().models.iter().find(|m| m.id == id) else {
            return;
        };
        let scale = m.transform.scale * factor;
        if cx
            .run("model.set_transform", json!({"id": id, "scale": scale}))
            .is_some()
        {
            cx.s.sync();
            cx.s.frame_all();
            cx.s.person_near(id);
        }
    }

    pub fn show(&mut self, cx: &mut Cx, actions: &mut Vec<Action>) {
        let ctx = cx.ctx.clone();
        let narrow = crate::ui::narrow(&ctx);
        if let Some(id) = self.prompt.clone() {
            let Some((lo, hi)) = cx.s.model_bounds(&id) else {
                self.prompt = None;
                return;
            };
            let s =
                cx.s.project()
                    .models
                    .iter()
                    .find(|m| m.id == id)
                    .map_or(1.0, |m| m.transform.scale);
            let (height, largest) = (
                (hi[1] - lo[1]) * s,
                (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f32::max) * s,
            );
            let huge = largest > SENSIBLE.1;
            floating(
                &ctx,
                "scaleprompt",
                Align2::CENTER_TOP,
                Vec2::new(0.0, if narrow { 150.0 } else { 56.0 }),
                ctx.content_rect().width() - 20.0,
                |ui| {
                    ui.label(format!(
                        "This model is {} tall. Scale it?",
                        length_text(height)
                    ));
                    ui.horizontal(|ui| {
                        for k in [10.0_f32, 100.0, 1000.0] {
                            let (label, f) = if huge {
                                (format!("÷{k}"), 1.0 / k)
                            } else {
                                (format!("×{k}"), k)
                            };
                            if ui.button(label).clicked() {
                                Self::scale_by(cx, &id, f);
                                self.prompt = None;
                            }
                        }
                        if ui.button(t::KEEP).clicked() {
                            self.prompt = None;
                        }
                    });
                },
            );
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(model) = cx.s.project().models.iter().find(|m| m.id == id).cloned() else {
            self.selected = None;
            return;
        };
        let size = cx.s.model_bounds(&id).map(|(lo, hi)| {
            (0..3)
                .map(|k| (hi[k] - lo[k]) * model.transform.scale)
                .collect::<Vec<_>>()
        });
        let (anchor, offset) = if narrow {
            (Align2::LEFT_TOP, Vec2::new(10.0, 104.0))
        } else {
            (Align2::RIGHT_TOP, Vec2::new(-10.0, 56.0))
        };
        let mut close = false;
        floating(&ctx, "modelpanel", anchor, offset, 300.0, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&model.name).strong());
                if ui.small_button("x").clicked() {
                    close = true;
                }
            });
            if let Some(s) = &size {
                ui.label(format!(
                    "{:.2} × {:.2} × {:.2} m (w × d × h)",
                    s[0], s[2], s[1]
                ));
            }
            let tr = model.transform;
            let mut update = serde_json::Map::new();
            let mut scale = tr.scale;
            ui.horizontal(|ui| {
                if drag_field(
                    ui,
                    t::SCALE,
                    &mut scale,
                    f64::from(tr.scale) * 0.01 + 1e-5,
                    1e-4..=1e5,
                    "",
                ) == Edit::Done
                    && scale != tr.scale
                {
                    update.insert("scale".into(), json!(scale));
                }
                for (label, f) in [("÷10", 0.1_f32), ("×10", 10.0)] {
                    if ui.small_button(label).clicked() {
                        update.insert("scale".into(), json!(tr.scale * f));
                    }
                }
            });
            let mut rot = tr.rotation_y_deg;
            if drag_field(ui, t::ROTATION, &mut rot, 0.5, -360.0..=360.0, "°") == Edit::Done
                && rot != tr.rotation_y_deg
            {
                update.insert("rotation_y_deg".into(), json!(rot));
            }
            let mut pos = tr.translation;
            let mut moved = false;
            ui.horizontal(|ui| {
                for (label, v) in ["x", "y", "z"].iter().zip(pos.iter_mut()) {
                    let before = *v;
                    moved |= drag_field(ui, label, v, 0.05, -1e5..=1e5, "") == Edit::Done
                        && *v != before;
                }
            });
            if moved {
                update.insert("translation".into(), json!(pos));
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button(t::DROP_TO_GROUND).clicked()
                    && let Some((lo, _)) = cx.s.model_bounds(&id)
                {
                    let ground = cx.s.project().view.ground.map_or(0.0, |g| g.height);
                    update.insert(
                        "translation".into(),
                        json!([pos[0], ground - lo[1] * tr.scale, pos[2]]),
                    );
                }
                let loaded = cx.s.loaded.contains_key(&id);
                if ui
                    .button(if loaded { t::RELOAD_MODEL } else { t::RELINK })
                    .clicked()
                {
                    actions.push(if loaded {
                        Action::ReloadModel(id.clone())
                    } else {
                        Action::Relink(id.clone())
                    });
                }
                if ui.button(t::REMOVE).clicked() {
                    cx.run("model.remove", json!({"id": id}));
                    cx.s.loaded.remove(&id);
                    close = true;
                }
            });
            if !update.is_empty() {
                update.insert("id".into(), json!(id));
                cx.run("model.set_transform", serde_json::Value::Object(update));
            }
        });
        if close {
            self.selected = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_only_for_odd_sizes() {
        assert!(needs_prompt(0.1));
        assert!(needs_prompt(500.0));
        assert!(!needs_prompt(12.0));
        assert_eq!(length_text(0.12), "12 cm");
        assert_eq!(length_text(3.44), "3.4 m");
    }
}
