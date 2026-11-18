//! Plants and the light a spot gets, in Sun & sky under the sun-hours map: "Plants for this
//! light" ranks the species for the area the map shows (its mean hours; the ranking of
//! `query.plants_for_light`), and "Check plant light" runs `query.light_check` and marks the
//! plants that are in the wrong light with a small ring on the plan in the Draw view: amber for
//! too sunny, blue for too shady.
//!
//! The light ranges are estimated per species and not yet checked against open data.

use crate::strings as t;
use crate::ui::Cx;
use eframe::egui::{self, Color32};
use visioncraft_core::growth::Grower;
use visioncraft_core::plant_light::{self, GROWERS};
use visioncraft_render::LineVertex;

/// How wide (m) a ring on the plan is.
const RING_RADIUS_M: f32 = 0.3;
const SUNNY: [u8; 4] = [255, 170, 40, 255];
const SHADY: [u8; 4] = [90, 150, 255, 255];

/// A plant in the wrong light.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub at: [f32; 2],
    pub too_sunny: bool,
}

/// One species of the check: how many of it are misplaced and what could replace it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub too_sunny: usize,
    pub too_shady: usize,
    pub mean_hours: f64,
    pub instead: Vec<String>,
}

/// What the last check found, for the revision of the project it was made on.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub revision: u64,
    pub day: (u32, u32),
    pub checked: usize,
    pub ok: usize,
    pub too_sunny: usize,
    pub too_shady: usize,
    pub marks: Vec<Mark>,
    pub rows: Vec<Row>,
}

#[derive(Default)]
pub struct LightState {
    pub check: Option<Check>,
    pub error: Option<String>,
    /// The grower filter of the list, an index into [`FILTERS`]; 0 is all.
    pub filter: usize,
    pub marks: bool,
}

/// The grower filter's choices: all, then these kinds.
const FILTERS: &[&str] = &[
    "tree",
    "shrub",
    "perennial",
    "grass",
    "ground_cover",
    "climber",
];

impl LightState {
    pub fn new() -> Self {
        Self {
            marks: true,
            ..Self::default()
        }
    }
}

/// Reads a `query.light_check` answer.
pub fn read_check(r: &serde_json::Value, revision: u64) -> Check {
    let n = |v: &serde_json::Value| v.as_u64().unwrap_or(0) as usize;
    let s = &r["summary"];
    let marks = r["plants"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let at = [
                        p["position"][0].as_f64()? as f32,
                        p["position"][1].as_f64()? as f32,
                    ];
                    Some(Mark {
                        at,
                        too_sunny: p["verdict"] == "too_sunny",
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let rows = r["by_species"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|v| n(&v["too_sunny"]) + n(&v["too_shady"]) > 0)
                .map(|v| Row {
                    name: v["name"]
                        .as_str()
                        .or_else(|| v["species"].as_str())
                        .unwrap_or("?")
                        .to_string(),
                    too_sunny: n(&v["too_sunny"]),
                    too_shady: n(&v["too_shady"]),
                    mean_hours: v["mean_hours"].as_f64().unwrap_or(0.0),
                    // The first plant of the species that is listed has the suggestions.
                    instead: r["plants"]
                        .as_array()
                        .and_then(|p| p.iter().find(|p| p["species"] == v["species"]))
                        .and_then(|p| p["suggestions"].as_array())
                        .map(|s| {
                            s.iter()
                                .filter_map(|x| {
                                    x["name"].as_str().or_else(|| x["species"].as_str())
                                })
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Check {
        revision,
        day: (
            r["date"]["month"].as_u64().unwrap_or(6) as u32,
            r["date"]["day"].as_u64().unwrap_or(21) as u32,
        ),
        checked: n(&s["checked"]),
        ok: n(&s["ok"]),
        too_sunny: n(&s["too_sunny"]),
        too_shady: n(&s["too_shady"]),
        marks,
        rows,
    }
}

/// Runs the check for `day` (the map's, else the summer solstice).
fn run_check(cx: &mut Cx, day: Option<(u32, u32)>) {
    let s = &*cx.s;
    let loaded = |id: &str| s.loaded.get(id).map(|l| l.model.clone());
    let q = visioncraft_automation::LightCheckQuery {
        date: day,
        limit: 5000,
        ..Default::default()
    };
    let revision = s.session.revision();
    let result = visioncraft_automation::light_check(s.project(), &loaded, &s.assets, &q);
    match result {
        Ok(r) => {
            cx.s.light.check = Some(read_check(&r, revision));
            cx.s.light.error = None;
        }
        Err(e) => {
            cx.s.light.check = None;
            cx.s.light.error = Some(e);
        }
    }
}

/// The two lists under the sun map's panel.
pub fn panel(ui: &mut egui::Ui, cx: &mut Cx) {
    ui.separator();
    ui.label(egui::RichText::new(t::LIGHT_FOR).strong());
    let stats = cx.s.sun_map.done.as_ref().and_then(|d| d.stats.clone());
    match stats.filter(|_| cx.s.project().view.sun_map.is_some()) {
        None => {
            ui.weak(t::LIGHT_NEEDS_MAP);
        }
        Some(st) => {
            ui.small(format!(
                "{:.1} h of sun on average ({:.1}-{:.1} h)",
                st.mean, st.min, st.max
            ));
            ui.horizontal(|ui| {
                ui.label(t::LIGHT_KIND);
                let f = cx.s.light.filter;
                egui::ComboBox::from_id_salt("light-kind")
                    .selected_text(if f == 0 {
                        t::LIGHT_ALL.to_string()
                    } else {
                        FILTERS[f - 1].replace('_', " ")
                    })
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(f == 0, t::LIGHT_ALL).clicked() {
                            cx.s.light.filter = 0;
                        }
                        for (i, name) in FILTERS.iter().enumerate() {
                            if ui
                                .selectable_label(f == i + 1, name.replace('_', " "))
                                .clicked()
                            {
                                cx.s.light.filter = i + 1;
                            }
                        }
                    });
            });
            let grower: Option<Grower> = match cx.s.light.filter {
                0 => None,
                i => GROWERS.iter().find(|g| g.0 == FILTERS[i - 1]).map(|g| g.1),
            };
            let ranked = plant_light::rank(st.mean as f32, grower);
            if ranked.is_empty() {
                ui.weak(t::LIGHT_NONE);
            }
            egui::ScrollArea::vertical()
                .id_salt("light-list")
                .max_height(150.0)
                .show(ui, |ui| {
                    for r in ranked.iter().take(15) {
                        let name =
                            visioncraft_flora::Species::by_id(r.id).map_or(r.id, |s| s.common_name);
                        let tag = match r.fit {
                            plant_light::Fit::Full => "",
                            plant_light::Fit::Partial => " (partly)",
                        };
                        ui.label(format!("{name}{tag}")).on_hover_text(&r.reason);
                    }
                });
            ui.small(t::LIGHT_ESTIMATED);
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .button(t::LIGHT_CHECK)
            .on_hover_text(t::LIGHT_CHECK_TIP)
            .clicked()
        {
            let day =
                cx.s.sun_map
                    .done
                    .as_ref()
                    .and_then(|d| d.stats.as_ref())
                    .map(|s| (s.month, s.day));
            run_check(cx, day);
        }
        ui.checkbox(&mut cx.s.light.marks, t::LIGHT_MARKS);
    });
    if let Some(e) = &cx.s.light.error {
        ui.colored_label(Color32::from_rgb(255, 140, 120), e);
    }
    let revision = cx.s.session.revision();
    if let Some(c) = &cx.s.light.check {
        if c.revision != revision {
            ui.weak(t::LIGHT_STALE);
        }
        ui.label(format!(
            "{} plants: {} fine, {} too sunny, {} too shady",
            c.checked, c.ok, c.too_sunny, c.too_shady
        ));
        egui::ScrollArea::vertical()
            .id_salt("light-rows")
            .max_height(150.0)
            .show(ui, |ui| {
                for r in c.rows.iter().take(12) {
                    let what = match (r.too_sunny, r.too_shady) {
                        (s, 0) => format!("{s} too sunny"),
                        (0, h) => format!("{h} too shady"),
                        (s, h) => format!("{s} too sunny, {h} too shady"),
                    };
                    ui.label(format!("{}: {what} ({:.1} h)", r.name, r.mean_hours));
                    if !r.instead.is_empty() {
                        ui.small(format!("  instead: {}", r.instead.join(", ")));
                    }
                }
            });
    }
}

/// The rings of the last check for the plan (Draw view), while it is current.
pub fn rings(cx: &Cx, top: &mut Vec<LineVertex>) {
    let Some(c) = &cx.s.light.check else {
        return;
    };
    if !cx.s.light.marks || c.revision != cx.s.session.revision() {
        return;
    }
    for m in &c.marks {
        let ring: Vec<[f32; 2]> = (0..16)
            .map(|i| {
                let a = i as f32 / 16.0 * std::f32::consts::TAU;
                [
                    m.at[0] + RING_RADIUS_M * a.cos(),
                    m.at[1] + RING_RADIUS_M * a.sin(),
                ]
            })
            .collect();
        top.extend(crate::draw::outline_lines(
            &ring,
            true,
            0.07,
            if m.too_sunny { SUNNY } else { SHADY },
            3.0,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_check_answer_is_read_into_marks_and_rows() {
        let r = json!({
            "date": {"month": 6, "day": 21},
            "summary": {"checked": 5, "ok": 3, "too_sunny": 1, "too_shady": 1},
            "plants": [
                {"species": "flora:hosta", "position": [1.0, 2.0], "verdict": "too_sunny",
                 "suggestions": [{"name": "Sedum", "species": "flora:x"}]},
                {"species": "flora:rosa-shrub", "position": [3.0, 4.0], "verdict": "too_shady"},
            ],
            "by_species": [
                {"species": "flora:hosta", "name": "Hosta", "too_sunny": 1, "too_shady": 0, "mean_hours": 9.0},
                {"species": "flora:other", "too_sunny": 0, "too_shady": 0, "mean_hours": 5.0},
            ],
        });
        let c = read_check(&r, 7);
        assert_eq!(
            (c.checked, c.too_sunny, c.too_shady, c.revision),
            (5, 1, 1, 7)
        );
        assert_eq!(c.marks.len(), 2);
        assert!(c.marks[0].too_sunny && !c.marks[1].too_sunny);
        assert_eq!(c.rows.len(), 1);
        assert_eq!(c.rows[0].instead, vec!["Sedum".to_string()]);
    }
}
