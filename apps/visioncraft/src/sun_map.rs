//! The sun-hours map in the window and in pictures: the grid of hours is worked out off the UI
//! thread, turned into a small texture (one texel per cell) and drawn as a see-through quad
//! just over the area, with a legend and the panel in Sun & sky (`view.set_sun_map`).
//!
//! Computing: `visioncraft_automation::SunGridJob`, the work of `query.sun_hours`. Natively a
//! thread runs it (cancelled when the project changes again); on the web, with no threads, the
//! window does [`CELLS_PER_FRAME`] cells a frame so the page keeps moving (the garden lawn at
//! half a metre is about 1,100 cells: 0.2 s of work natively, a second or so spread over some
//! twenty frames in the browser). It runs again when the project's revision, the loaded models
//! or the setting (other than the opacity) change, never every frame; the old map stays on
//! screen until the new one is ready.
//!
//! The map is a flat quad at the target area's height plus [`visioncraft_core::sun_map::LIFT_M`],
//! also over terrain.

use crate::strings as t;
use crate::ui::Cx;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::json;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use visioncraft_automation::{SunGrid, SunGridJob};
use visioncraft_core::sun_hours::{FULL_SUN_HOURS, PART_SHADE_HOURS};
use visioncraft_core::sun_map::{GridSpec, RAMP_HOURS, SunMapStats, ramp};
use visioncraft_core::{Shape, SunMap};
use visioncraft_render::TextureId;

/// Cells a frame the web window works out (no threads there).
#[cfg(target_arch = "wasm32")]
pub const CELLS_PER_FRAME: usize = 48;

/// What a finished map was made for: the project's revision, what the loaded models were, and
/// the setting without its opacity (the opacity only changes how it is drawn).
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    revision: u64,
    models: u64,
    map: SunMap,
}

impl Key {
    pub fn new(revision: u64, models: u64, map: &SunMap) -> Self {
        Self {
            revision,
            models,
            map: SunMap {
                opacity: 0.0,
                ..map.clone()
            },
        }
    }
}

/// The map on the GPU.
pub struct Done {
    pub spec: GridSpec,
    pub texture: TextureId,
    pub stats: Option<SunMapStats>,
    pub years: f32,
}

enum Work {
    #[cfg(not(target_arch = "wasm32"))]
    Thread(Arc<Mutex<Option<SunGrid>>>),
    #[cfg(target_arch = "wasm32")]
    Sliced(Option<SunGridJob>),
}

struct Task {
    key: Key,
    cancel: Arc<AtomicBool>,
    work: Work,
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The map being made and the one shown.
#[derive(Default)]
pub struct SunMapState {
    task: Option<Task>,
    /// What the shown map (or the error) is for.
    key: Option<Key>,
    pub done: Option<Done>,
    /// Why there is no map (a shape gone, too many cells).
    pub error: Option<String>,
    /// The area the panel last chose, for when the map is switched on: a shape id, `None` the
    /// whole plot.
    pub target: Option<String>,
}

impl SunMapState {
    pub fn pending(&self) -> bool {
        self.task.is_some()
    }

    /// Whether `key` is what is shown or being made.
    pub fn is_current(&self, key: &Key) -> bool {
        self.key.as_ref() == Some(key)
    }

    /// Starts `job` for `key`, dropping (cancelling) the one under way.
    pub fn start(&mut self, key: Key, job: SunGridJob) {
        self.error = None;
        let cancel = Arc::new(AtomicBool::new(false));
        #[cfg(not(target_arch = "wasm32"))]
        let work = {
            let slot: Arc<Mutex<Option<SunGrid>>> = Arc::default();
            let (out, stop) = (slot.clone(), cancel.clone());
            std::thread::spawn(move || {
                let mut job = job;
                while !job.step(64) {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                }
                if let Ok(mut o) = out.lock() {
                    *o = Some(job.finish());
                }
            });
            Work::Thread(slot)
        };
        #[cfg(target_arch = "wasm32")]
        let work = Work::Sliced(Some(job));
        self.key = Some(key.clone());
        self.task = Some(Task { key, cancel, work });
    }

    /// Moves the work along (a slice on the web) and takes a finished grid with its key.
    pub fn poll(&mut self) -> Option<(Key, SunGrid)> {
        let task = self.task.as_mut()?;
        #[cfg(not(target_arch = "wasm32"))]
        let grid = match &task.work {
            Work::Thread(slot) => slot.lock().ok()?.take(),
        };
        #[cfg(target_arch = "wasm32")]
        let grid = match &mut task.work {
            Work::Sliced(slot) => {
                if slot.as_mut()?.step(CELLS_PER_FRAME) {
                    slot.take().map(SunGridJob::finish)
                } else {
                    None
                }
            }
        };
        let grid = grid?;
        let task = self.task.take()?;
        Some((task.key.clone(), grid))
    }

    /// Records the key an error belongs to, so it is not tried again every frame.
    pub fn fail(&mut self, key: Key, why: String) {
        self.key = Some(key);
        self.error = Some(why);
        self.task = None;
    }

    /// Forgets the map; the texture to free, if any.
    pub fn clear(&mut self) -> Option<TextureId> {
        self.task = None;
        self.key = None;
        self.error = None;
        self.done.take().map(|d| d.texture)
    }
}

/// The box round the visible shapes, as a polygon: "the whole plot".
pub fn plot_outline(project: &visioncraft_core::Project) -> Option<Vec<[f32; 2]>> {
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for s in project
        .shapes
        .iter()
        .filter(|s| project.layer_visible(s.layer()))
    {
        for p in s.outline().iter() {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    (lo[0].is_finite() && hi[0] > lo[0] && hi[1] > lo[1]).then(|| {
        vec![
            [lo[0], lo[1]],
            [hi[0], lo[1]],
            [hi[0], hi[1]],
            [lo[0], hi[1]],
        ]
    })
}

/// A cell size for `outline` that keeps the grid under the cell limit, in quarter metres, 0.5 m
/// when the area is small enough for it.
pub fn cell_for(outline: &[[f32; 2]]) -> f32 {
    let spec = |c: f32| GridSpec::new(outline, c).inside_count(outline);
    let mut cell = 0.5;
    while spec(cell) > visioncraft_core::sun_map::MAX_CELLS * 4 / 5 && cell < 10.0 {
        cell += 0.25;
    }
    cell
}

/// The date label of a map: "21 Dec".
fn day_label(month: u32, day: u32) -> String {
    format!(
        "{day} {}",
        t::MONTHS.get(month as usize - 1).copied().unwrap_or("?")
    )
}

/// The panel in Sun & sky: area, day, years and opacity of the map, and off.
pub fn panel(ui: &mut egui::Ui, cx: &mut Cx) {
    ui.small(t::SUN_MAP_TIP);
    let p = cx.s.project();
    let map = p.view.sun_map.clone();
    let plot = plot_outline(p);
    let areas: Vec<(String, String)> = p
        .shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Area(a) => Some((a.id.clone(), format!("{} ({})", a.name, a.id))),
            Shape::Extrusion(_) => None,
        })
        .collect();
    if plot.is_none() && areas.is_empty() {
        ui.weak(t::SUN_MAP_NO_SHAPES);
        return;
    }
    let now = cx.s.time();
    // The area: the map's own, else the one last chosen.
    let current: Option<String> = match &map {
        Some(m) => m.shape.clone(),
        None => cx.s.sun_map.target.clone(),
    };
    let mut on = map.is_some();
    let mut run: Option<serde_json::Value> = None;
    let mut target = current.clone();
    if ui.checkbox(&mut on, t::SUN_MAP_SHOW).changed() {
        if on {
            run = Some(target_params(&target, plot.as_deref(), &areas, p));
        } else {
            cx.run("view.clear_sun_map", json!({}));
        }
    }
    ui.horizontal(|ui| {
        ui.label(t::SUN_MAP_AREA);
        let label = match &target {
            Some(id) => areas
                .iter()
                .find(|(i, _)| i == id)
                .map_or(id.clone(), |(_, n)| n.clone()),
            None => t::SUN_MAP_PLOT.to_string(),
        };
        egui::ComboBox::from_id_salt("sun-map-area")
            .selected_text(label)
            .show_ui(ui, |ui| {
                if plot.is_some()
                    && ui
                        .selectable_label(target.is_none(), t::SUN_MAP_PLOT)
                        .clicked()
                {
                    target = None;
                }
                for (id, name) in &areas {
                    if ui
                        .selectable_label(target.as_deref() == Some(id), name)
                        .clicked()
                    {
                        target = Some(id.clone());
                    }
                }
            });
    });
    if target != current {
        cx.s.sun_map.target = target.clone();
        if on {
            run = Some(target_params(
                &target,
                plot.as_deref(),
                &areas,
                cx.s.project(),
            ));
        }
    }
    if let Some(m) = &map {
        let by_season = m.season;
        let by_date = m.date.is_some();
        ui.horizontal_wrapped(|ui| {
            ui.label(t::SUN_MAP_DAY);
            for (s, label) in visioncraft_core::Season::ALL.into_iter().zip(t::SEASONS) {
                if ui.selectable_label(by_season == Some(s), label).clicked() {
                    run = Some(json!({"season": s.name()}));
                }
            }
            let project_date = by_season.is_none() && !by_date;
            if ui
                .selectable_label(project_date, t::SUN_MAP_PROJECT_DATE)
                .on_hover_text(day_label(now.month, now.day))
                .clicked()
            {
                run = Some(json!({"season": null, "date": null}));
            }
        });
        years_slider(ui, cx, m);
        let mut opacity = m.opacity;
        let r = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0).text(t::SUN_MAP_OPACITY));
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            run = Some(json!({"opacity": (opacity * 100.0).round() / 100.0}));
        }
        if cx.s.sun_map.pending() {
            ui.small(t::SUN_MAP_WORKING);
        }
        if let Some(e) = &cx.s.sun_map.error {
            ui.colored_label(Color32::from_rgb(255, 140, 120), e);
        }
    }
    if let Some(params) = run {
        cx.run("view.set_sun_map", params);
    }
    crate::light_ui::panel(ui, cx);
}

/// `view.set_sun_map` parameters that put the map on `target`: an area, or the plot's box.
fn target_params(
    target: &Option<String>,
    plot: Option<&[[f32; 2]]>,
    areas: &[(String, String)],
    project: &visioncraft_core::Project,
) -> serde_json::Value {
    match target {
        Some(id) if areas.iter().any(|(i, _)| i == id) => {
            let cell = project
                .shapes
                .iter()
                .find_map(|s| match s {
                    Shape::Area(a) if a.id == *id => Some(cell_for(&a.outline())),
                    _ => None,
                })
                .unwrap_or(0.5);
            json!({"shape": id, "cell_m": cell})
        }
        _ => {
            let outline = plot.map(<[[f32; 2]]>::to_vec).unwrap_or_default();
            json!({"outline": outline, "cell_m": cell_for(&outline)})
        }
    }
}

/// The years after planting, tied to the view's (the sun panel's): the map follows them unless
/// it was given its own by a command.
fn years_slider(ui: &mut egui::Ui, cx: &mut Cx, map: &SunMap) {
    let committed = map.years.or(cx.s.project().view.years).unwrap_or(0.0);
    let mut y = cx.s.years_draft().unwrap_or(committed);
    let r = ui
        .add(
            egui::Slider::new(&mut y, 0.0..=30.0)
                .step_by(0.5)
                .suffix(" y")
                .text(t::SUN_MAP_YEARS),
        )
        .on_hover_text(t::SUN_MAP_YEARS_TIP);
    if r.dragged() {
        cx.s.set_years_draft(Some(y));
    } else if r.drag_stopped() || (r.changed() && y != committed) {
        cx.s.set_years_draft(None);
        cx.run("view.set", json!({"years": y}));
        if map.years.is_some() {
            cx.run("view.set_sun_map", json!({"years": null}));
        }
    }
}

/// The legend in the viewport's corner: the colour ramp with hour ticks, the day, and the shares
/// of the area in full sun, part shade and shade.
pub fn legend(ctx: &egui::Context, cx: &mut Cx) {
    let Some(d) = &cx.s.sun_map.done else {
        return;
    };
    if cx.s.project().view.sun_map.is_none() {
        return;
    }
    let Some(stats) = d.stats.clone() else {
        return;
    };
    let years = d.years;
    let pending = cx.s.sun_map.pending();
    egui::Area::new(egui::Id::new("sun-map-legend"))
        .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-10.0, -10.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            crate::ui::panel_frame().show(ui, |ui| {
                ui.set_max_width(220.0);
                ui.label(egui::RichText::new(t::SUN_MAP_LEGEND).strong());
                ui.small(format!(
                    "{}, {:.1} h daylight{}",
                    day_label(stats.month, stats.day),
                    stats.daylight_hours,
                    if years > 0.0 {
                        format!(", {years:.0} y")
                    } else {
                        String::new()
                    }
                ));
                ramp_bar(ui);
                for (label, share) in [
                    (
                        format!("{} ({}+ h)", t::SUN_MAP_FULL, FULL_SUN_HOURS),
                        stats.full_sun_share,
                    ),
                    (
                        format!(
                            "{} ({}-{} h)",
                            t::SUN_MAP_PART,
                            PART_SHADE_HOURS,
                            FULL_SUN_HOURS
                        ),
                        stats.part_shade_share,
                    ),
                    (
                        format!("{} (<{} h)", t::SUN_MAP_SHADE, PART_SHADE_HOURS),
                        stats.shade_share,
                    ),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(format!("{:>3.0} %", share * 100.0));
                        ui.small(label);
                    });
                }
                if pending {
                    ui.small(t::SUN_MAP_WORKING);
                }
            });
        });
}

/// The ramp as a bar with a tick every two hours.
fn ramp_bar(ui: &mut egui::Ui) {
    let width = ui.available_width().clamp(120.0, 200.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
    let bar = Rect::from_min_size(rect.min, Vec2::new(width, 12.0));
    let painter = ui.painter();
    let steps = 48;
    for i in 0..steps {
        let (a, b) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
        let c = ramp((a + b) / 2.0 * RAMP_HOURS);
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(bar.min.x + a * width, bar.min.y),
                Pos2::new(bar.min.x + b * width + 0.5, bar.max.y),
            ),
            0.0,
            Color32::from_rgb(c[0], c[1], c[2]),
        );
    }
    painter.rect_stroke(
        bar,
        0.0,
        Stroke::new(1.0, Color32::from_white_alpha(90)),
        egui::StrokeKind::Outside,
    );
    let mut h = 0.0;
    while h <= RAMP_HOURS + 0.01 {
        let x = bar.min.x + h / RAMP_HOURS * width;
        painter.line_segment(
            [Pos2::new(x, bar.max.y), Pos2::new(x, bar.max.y + 3.0)],
            Stroke::new(1.0, Color32::from_white_alpha(160)),
        );
        let label = if h >= RAMP_HOURS {
            format!("{h:.0}+")
        } else {
            format!("{h:.0}")
        };
        painter.text(
            Pos2::new(x, bar.max.y + 4.0),
            Align2::CENTER_TOP,
            label,
            egui::FontId::proportional(10.0),
            Color32::from_white_alpha(200),
        );
        h += 2.0;
    }
}
