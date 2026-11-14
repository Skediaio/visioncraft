//! Sea & water in Landscape & Sun: the sea level on or off, its height, kind, sea state, wave
//! direction, colour, clarity, foam and a river's flow (`water.set_sea`, `water.remove_sea`),
//! and a beach preset (`terrain.shore`) along a line across the view.

use crate::strings as t;
use crate::ui::Cx;
use eframe::egui;
use serde_json::{Value, json};
use visioncraft_core::WaterKind;

const KINDS: [(WaterKind, &str, &str); 3] = [
    (WaterKind::Sea, "sea", t::WA_SEA),
    (WaterKind::Lake, "lake", t::WA_LAKE),
    (WaterKind::River, "river", t::WA_RIVER),
];

pub fn panel(ui: &mut egui::Ui, cx: &mut Cx) {
    let sea = cx.s.project().sea.clone();
    let mut on = sea.is_some();
    if ui.checkbox(&mut on, t::WA_ON).changed() {
        if on {
            // Half a metre under the endless ground: nothing drowns until a shore is shaped.
            let level = cx.s.project().view.ground.map_or(0.0, |g| g.height) - 0.5;
            cx.run("water.set_sea", json!({"level": level}));
        } else {
            cx.run("water.remove_sea", json!({}));
        }
    }
    let Some(sea) = sea else {
        ui.small(t::WA_HINT);
        return;
    };
    let set = |cx: &mut Cx, p: Value| {
        cx.run("water.set_sea", p);
    };
    ui.horizontal(|ui| {
        for (kind, key, label) in KINDS {
            if ui.selectable_label(sea.kind == kind, label).clicked() && sea.kind != kind {
                set(cx, json!({"kind": key}));
            }
        }
    });
    let slider = |ui: &mut egui::Ui,
                  cx: &mut Cx,
                  label: &str,
                  key: &str,
                  mut value: f32,
                  range: std::ops::RangeInclusive<f32>,
                  suffix: &str| {
        let r = ui.add(
            egui::Slider::new(&mut value, range)
                .text(label)
                .suffix(suffix)
                .max_decimals(2),
        );
        if r.drag_stopped() || r.lost_focus() {
            set(cx, json!({ key: value }));
        }
    };
    slider(ui, cx, t::WA_LEVEL, "level", sea.level, -20.0..=20.0, " m");
    slider(
        ui,
        cx,
        t::WA_STATE,
        "sea_state",
        sea.sea_state,
        0.0..=6.0,
        "",
    );
    ui.small(format!("{} {:.2} m", t::WA_HEIGHT, sea.wave_height_m()));
    let mut follow = sea.direction_deg.is_none();
    if ui.checkbox(&mut follow, t::WA_FOLLOW_WIND).changed() {
        let deg = cx.s.project().view.wind_direction_deg;
        set(
            cx,
            json!({"direction_deg": if follow { None } else { Some(deg) }}),
        );
    }
    if let Some(d) = sea.direction_deg {
        slider(
            ui,
            cx,
            t::WA_DIRECTION,
            "direction_deg",
            d,
            0.0..=360.0,
            "°",
        );
    }
    if sea.kind == WaterKind::River {
        slider(
            ui,
            cx,
            t::WA_FLOW_DIR,
            "flow_deg",
            sea.flow_deg,
            0.0..=360.0,
            "°",
        );
        slider(
            ui,
            cx,
            t::WA_FLOW,
            "flow_speed",
            sea.flow_speed,
            0.0..=4.0,
            " m/s",
        );
    }
    slider(ui, cx, t::WA_CLARITY, "clarity", sea.clarity, 0.0..=1.0, "");
    slider(ui, cx, t::WA_FOAM, "foam", sea.foam, 0.0..=1.0, "");
    ui.horizontal(|ui| {
        ui.label(t::WA_COLOR);
        let mut c = sea.color;
        let r = egui::color_picker::color_edit_button_rgb(ui, &mut c);
        if r.changed() {
            set(cx, json!({ "color": c }));
        }
    });
    ui.separator();
    ui.small(t::WA_BEACH_HINT);
    if ui.button(t::WA_BEACH).clicked() {
        beach_across_view(cx);
    }
}

/// A beach along a 200 m line through the point the view looks at, across the view, the sea
/// on the far side: a terrain is made round it if there is none.
fn beach_across_view(cx: &mut Cx) {
    let (eye, target) = cx.s.camera.pose();
    let mut f = glam::Vec2::new(target.x - eye.x, target.z - eye.z);
    if f.length() < 1e-3 {
        f = glam::Vec2::new(0.0, -1.0);
    }
    let f = f.normalize();
    let c = glam::Vec2::new(target.x, target.z);
    // Travelling along `left` keeps the sea (the right) in the view's direction.
    let along = glam::Vec2::new(f.y, -f.x);
    let (a, b) = (c - along * 100.0, c + along * 100.0);
    if cx.s.project().terrain.is_none() {
        let o = c - glam::Vec2::splat(80.0);
        cx.run(
            "terrain.create",
            json!({"origin": [o.x, o.y], "width_m": 160, "depth_m": 160, "cell_m": 1.0, "base_material": "lawn"}),
        );
    }
    if cx.s.project().sea.is_none() {
        cx.run("water.set_sea", json!({"level": 0}));
    }
    cx.run(
        "terrain.shore",
        json!({"points": [[a.x, a.y], [b.x, b.y]], "top_m": 1.5, "beach_m": 15, "material": "sand"}),
    );
    // The endless ground under the water, so the sea runs on to the horizon past the terrain.
    let level = cx.s.project().sea.as_ref().map_or(0.0, |s| s.level);
    if let Some(g) = cx.s.project().view.ground
        && g.height > level - 4.0
    {
        cx.run(
            "view.set",
            json!({"ground": {"height": level - 4.0, "color": g.color}}),
        );
    }
}
