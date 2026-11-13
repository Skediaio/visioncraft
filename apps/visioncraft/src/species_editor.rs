//! The species editor: start from a built-in or own species, change its parameters (the
//! generator's own schema, `param_schema`), see it grow at the centre of the view, and save it
//! as a custom species (`species.add` / `species.update`). A saved species is an asset id like
//! any other, so every planting tool can use it.

use crate::strings as t;
use crate::studio::Studio;
use crate::ui::Cx;
use eframe::egui;
use serde_json::json;
use visioncraft_core::planting::Single;
use visioncraft_core::{CustomSpecies, Generator, Project};
use visioncraft_flora as flora;

/// A plant being edited.
pub struct Editing {
    /// The custom species it updates on save (`None`: saving adds one).
    pub id: Option<String>,
    pub base: Option<String>,
    pub name: String,
    pub params: flora::PlantParams,
    /// Bumps on every change the preview should show.
    pub serial: u64,
    /// Bloom months are a bit mask in the schema; shown as twelve toggles.
    group: String,
}

#[derive(Default)]
pub struct SpeciesEditor {
    pub editing: Option<Editing>,
    /// The parameters the preview shows (`serial` of `editing`).
    shown: u64,
}

/// The preview's generator id: it lives in the project the scene draws, never in the session.
const PREVIEW_GENERATOR: &str = "species-preview";

impl SpeciesEditor {
    /// Starts editing built-in species `flora:<id>` or own species `sp<n>`.
    pub fn open(&mut self, project: &Project, asset: &str) {
        let (id, base, name, params) = if let Some(b) = asset.strip_prefix("flora:") {
            let Some(s) = flora::Species::by_id(b) else {
                return;
            };
            (
                None,
                Some(b.to_string()),
                format!("{} {}", t::SP_MY, s.common_name.to_lowercase()),
                s.params.clone(),
            )
        } else {
            let Some(c) = project.species.iter().find(|c| c.id == asset) else {
                return;
            };
            let Ok(p) = serde_json::from_value(c.params.clone()) else {
                return;
            };
            (Some(c.id.clone()), c.base.clone(), c.name.clone(), p)
        };
        self.editing = Some(Editing {
            id,
            base,
            name,
            params,
            serial: 1,
            group: String::new(),
        });
        self.shown = 0;
    }

    pub fn close(&mut self, s: &mut Studio) {
        self.editing = None;
        if let Some((sp, _)) = s.species_preview.take() {
            s.scene.flora.retire(&sp.id);
        }
        s.force_sync();
    }

    /// The preview: a draft species and a single plant of it at the centre of the view. A new
    /// id per change, so the plant is generated afresh.
    pub fn update(&mut self, s: &mut Studio) {
        let Some(e) = &self.editing else {
            return;
        };
        if e.serial == self.shown {
            return;
        }
        self.shown = e.serial;
        let id = format!("species-preview-{}", e.serial);
        // The plant of the last change is generated anew under the new id: free the old one.
        if let Some((old, _)) = &s.species_preview {
            s.scene.flora.retire(&old.id);
        }
        let p = s.camera.pivot;
        let y = s.scene.floor_below(p.x, p.y + 50.0, p.z).unwrap_or(0.0);
        s.species_preview = Some((
            CustomSpecies {
                id: id.clone(),
                name: e.name.clone(),
                base: e.base.clone(),
                params: serde_json::to_value(&e.params).unwrap_or_default(),
                extensions: Default::default(),
            },
            Generator::Single(Single {
                id: PREVIEW_GENERATOR.into(),
                name: e.name.clone(),
                layer: "planting".into(),
                asset: id,
                position: [p.x, y, p.z],
                rotation_y_deg: 0.0,
                scale: 1.0,
            }),
        ));
        s.force_sync();
    }

    pub fn window(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        let Some(e) = &mut self.editing else {
            return;
        };
        let (mut save, mut close) = (false, false);
        let max_h = (ctx.content_rect().height() - 140.0).max(200.0);
        crate::ui::floating(
            ctx,
            "species-editor",
            egui::Align2::RIGHT_TOP,
            egui::Vec2::new(-10.0, 104.0),
            340.0,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(t::SP_TITLE).strong());
                    if ui.small_button("×").clicked() {
                        close = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(t::SP_NAME);
                    ui.text_edit_singleline(&mut e.name);
                });
                ui.small(t::SP_HINT);
                egui::ScrollArea::vertical()
                    .max_height(max_h - 120.0)
                    .id_salt("species-params")
                    .show(ui, |ui| {
                        if params_ui(ui, &mut e.params, &mut e.group) {
                            e.serial += 1;
                        }
                    });
                ui.horizontal(|ui| {
                    save = ui
                        .button(if e.id.is_some() {
                            t::SP_SAVE
                        } else {
                            t::SP_SAVE_NEW
                        })
                        .clicked();
                    if ui.button(t::CANCEL).clicked() {
                        close = true;
                    }
                });
            },
        );
        if save {
            let params = serde_json::to_value(&e.params).unwrap_or_default();
            let r = match &e.id {
                Some(id) => cx.run(
                    "species.update",
                    json!({"id": id, "name": e.name, "params": params}),
                ),
                None => {
                    let mut p = json!({"name": e.name, "params": params});
                    if let Some(b) = &e.base {
                        p["base"] = json!(b);
                    }
                    cx.run("species.add", p)
                }
            };
            if r.is_some() {
                close = true;
                cx.toasts.info(t::SP_SAVED);
            }
        }
        if close {
            self.close(cx.s);
        }
    }
}

/// The schema's fields, grouped by the first part of their path; true when one changed (on
/// release, for the sliders).
fn params_ui(ui: &mut egui::Ui, p: &mut flora::PlantParams, open: &mut String) -> bool {
    let mut changed = false;
    let schema = flora::param_schema(&p.form);
    let mut groups: Vec<&str> = Vec::new();
    for info in &schema {
        let g = info.path.split('.').next().unwrap_or("");
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    for g in groups {
        let fields: Vec<&flora::ParamInfo> = schema
            .iter()
            .filter(|i| i.path.split('.').next() == Some(g))
            .filter(|i| flora::get_param(p, i.path).is_some())
            .collect();
        if fields.is_empty() {
            continue;
        }
        let single = fields.len() == 1 && !fields[0].path.contains('.');
        let shown = single || *open == g;
        if !single {
            let label = format!("{} {}", if shown { "▾" } else { "▸" }, group_name(g));
            if ui.selectable_label(shown, label).clicked() {
                *open = if shown { String::new() } else { g.to_string() };
            }
            if !shown {
                continue;
            }
        }
        for info in fields {
            let Some(mut v) = flora::get_param(p, info.path) else {
                continue;
            };
            let before = v;
            let mut done = false;
            if info.path == "bloom_mask" {
                ui.label(info.label);
                let mut mask = v as u32;
                ui.horizontal_wrapped(|ui| {
                    for (m, name) in t::MONTHS.iter().enumerate() {
                        let mut on = mask & (1 << m) != 0;
                        if ui.toggle_value(&mut on, *name).changed() {
                            mask ^= 1 << m;
                            done = true;
                        }
                    }
                });
                v = mask as f32;
            } else if !info.choices.is_empty() {
                let mut i = v as usize;
                egui::ComboBox::from_label(info.label)
                    .selected_text(info.choices.get(i).copied().unwrap_or("?"))
                    .show_ui(ui, |ui| {
                        for (k, c) in info.choices.iter().enumerate() {
                            if ui.selectable_value(&mut i, k, *c).clicked() {
                                done = true;
                            }
                        }
                    });
                v = i as f32;
            } else {
                let r = ui
                    .add(
                        egui::Slider::new(&mut v, info.min..=info.max)
                            .step_by(f64::from(info.step))
                            .text(info.label),
                    )
                    .on_hover_text(info.help);
                done = r.drag_stopped() || (r.changed() && !r.dragged());
            }
            if v != before {
                flora::set_param(p, info.path, v);
            }
            changed |= done;
        }
    }
    changed
}

fn group_name(g: &str) -> String {
    let mut c = g.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
        .replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_in_species_opens_with_its_parameters_and_saves_through_the_spec() {
        let mut ed = SpeciesEditor::default();
        ed.open(&Project::default(), "flora:betula-pendula");
        let e = ed.editing.as_ref().expect("opened");
        assert_eq!(e.base.as_deref(), Some("betula-pendula"));
        let mut s = visioncraft_core::Session::new(Project::default());
        let params = serde_json::to_value(&e.params).unwrap();
        let r = s
            .run(
                "species.add",
                json!({"name": e.name, "base": e.base, "params": params}),
            )
            .unwrap();
        let id = r["id"].as_str().unwrap().to_string();
        // and it opens again as an own species
        ed.open(s.project(), &id);
        assert_eq!(ed.editing.unwrap().id.as_deref(), Some(id.as_str()));
    }
}
