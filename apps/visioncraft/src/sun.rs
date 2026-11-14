//! Landscape & Sun mode: compass, month dial, date and place fields. Dragging applies live as a
//! draft on the studio; one command is committed on release.

use crate::strings as t;
use crate::ui::{Cx, Edit};
use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke, Vec2};
use serde_json::json;
use visioncraft_core::sun::{sun_position, sunrise_sunset};
use visioncraft_core::{Site, SunTime};

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// 0 = Sunday (Sakamoto).
pub fn day_of_week(y: i32, m: u32, d: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[m as usize - 1] + d as i32).rem_euclid(7)) as u32
}

/// The last Sunday of a month.
pub fn last_sunday(y: i32, m: u32) -> u32 {
    let last = days_in_month(y, m);
    last - day_of_week(y, m, last)
}

/// Whether EU summer time is in force on a local date and hour (it starts at 02:00 CET on the last
/// Sunday of March and ends at 03:00 CEST on the last Sunday of October).
pub fn eu_summer_time(y: i32, m: u32, d: u32, hour: f32) -> bool {
    let (start, end) = (last_sunday(y, 3), last_sunday(y, 10));
    match m {
        4..=9 => true,
        3 => d > start || (d == start && hour >= 2.0),
        10 => d < end || (d == end && hour < 3.0),
        _ => false,
    }
}

/// The UTC offset of a place whose winter offset is `base`, on a date.
pub fn offset_for(base: f64, t: &SunTime) -> f64 {
    base + f64::from(u8::from(eu_summer_time(t.year, t.month, t.day, t.hour)))
}

fn angle_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The local hour (0.05 h steps) at which the sun is closest to compass azimuth `az`, preferring
/// hours when it is above the horizon.
pub fn hour_for_azimuth(site: &Site, time: &SunTime, az: f64) -> f32 {
    let mut best = (f64::MAX, time.hour);
    for i in 0..480 {
        let hour = i as f32 * 0.05;
        let p = sun_position(site, &SunTime { hour, ..*time });
        let cost =
            angle_diff(p.azimuth_deg, az) + if p.elevation_deg < -1.0 { 1000.0 } else { 0.0 };
        if cost < best.0 {
            best = (cost, hour);
        }
    }
    best.1
}

fn hhmm(h: f64) -> String {
    let m = (h * 60.0).round() as i64;
    format!("{:02}:{:02}", (m / 60).rem_euclid(24), m % 60)
}

fn on_circle(c: Pos2, r: f32, az_deg: f64) -> Pos2 {
    let a = az_deg.to_radians();
    c + Vec2::new(a.sin() as f32, -a.cos() as f32) * r
}

/// Landscape & Sun: the sun and sky, or the terrain tools.
pub fn panel(ui: &mut egui::Ui, cx: &mut Cx, terrain: &mut crate::terrain_tool::TerrainTool) {
    // The floating panel around it scrolls. The tabs come last, next to the mode bar: the panel
    // grows upwards, so they stay where they are when the tab changes.
    if terrain.open {
        terrain.panel(ui, cx);
    } else {
        panel_body(ui, cx);
        egui::CollapsingHeader::new(egui::RichText::new(t::SKY).small())
            .id_salt("sun-sky")
            .default_open(false)
            .show(ui, |ui| crate::sky_panel::panel(ui, cx));
    }
    ui.separator();
    ui.horizontal(|ui| {
        if ui.selectable_label(!terrain.open, t::TAB_SUN).clicked() && terrain.open {
            // The plan is the mode's, not the tab's.
            let view = cx.s.camera.view;
            terrain.leave(cx);
            cx.s.camera.view = view;
            terrain.open = false;
        }
        if ui.selectable_label(terrain.open, t::TAB_TERRAIN).clicked() {
            terrain.open = true;
        }
        // The plan, as in Draw and Objects: a pond or a stroke lands where it is drawn.
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
    });
}

/// Spring, Summer, Autumn, Winter and a snowy winter (`view.set {season, snow}`), and the snow.
fn season_row(ui: &mut egui::Ui, cx: &mut Cx) {
    use visioncraft_core::Season;
    let p = cx.s.project();
    let (now, snow) = (Season::of(cx.s.time().month, p.site.latitude), p.view.snow);
    let mut run = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t::SEASON).strong());
        for (s, label) in Season::ALL.into_iter().zip(t::SEASONS) {
            let on = now == s && (s != Season::Winter || snow == 0.0);
            if ui.selectable_label(on, label).clicked() {
                // Snow belongs to winter: another season clears it.
                run = Some(if snow > 0.0 && s != Season::Winter {
                    json!({"season": s.name(), "snow": 0})
                } else {
                    json!({"season": s.name()})
                });
            }
        }
        let snowy = now == Season::Winter && snow > 0.0;
        if ui
            .selectable_label(snowy, t::SNOWY)
            .on_hover_text(t::SNOWY_TIP)
            .clicked()
            && !snowy
        {
            // The sky stays the person's: a sunny snowy day has the most form.
            run = Some(json!({"season": "winter", "snow": SNOWY}));
        }
    });
    if now == Season::Winter || snow > 0.0 {
        let mut v = snow;
        let r = ui
            .add(egui::Slider::new(&mut v, 0.0..=1.0).text(t::SNOW))
            .on_hover_text(t::SNOW_TIP);
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            run = Some(json!({"snow": (v * 100.0).round() / 100.0}));
        }
    }
    if let Some(params) = run {
        cx.run("view.set", params);
    }
}

/// Snow of the Snowy winter.
const SNOWY: f32 = 0.7;

fn panel_body(ui: &mut egui::Ui, cx: &mut Cx) {
    let (site, mut time) = (cx.s.site(), cx.s.time());
    let committed = cx.s.project().time;
    // With the sky's own sun the compass shows it and cannot be dragged.
    let sky_sun = cx.s.sky.sun_angles();
    let mut pos = sun_position(&site, &time);
    if let Some((az, el)) = sky_sun {
        pos.azimuth_deg = az;
        pos.elevation_deg = el;
    }
    season_row(ui, cx);
    // The exact date (for sun studies) is folded away: the season is what most people choose.
    let exact_id = ui.id().with("exact-date");
    let mut exact = ui.data(|d| d.get_temp::<bool>(exact_id).unwrap_or(false));
    if ui
        .checkbox(&mut exact, t::EXACT_DATE)
        .on_hover_text(t::EXACT_DATE_TIP)
        .changed()
    {
        ui.data_mut(|d| d.insert_temp(exact_id, exact));
    }
    ui.small(t::SUN_HINT);
    let mut time_edit = Edit::None;
    ui.horizontal(|ui| {
        // Compass: azimuth around, sun nearer the centre when higher.
        let (r, painter) = ui.allocate_painter(Vec2::splat(168.0), Sense::click_and_drag());
        let (c, radius) = (r.rect.center(), 66.0);
        painter.circle_filled(c, radius + 6.0, Color32::from_white_alpha(14));
        painter.circle_stroke(c, radius, Stroke::new(1.5, Color32::from_white_alpha(110)));
        painter.circle_stroke(
            c,
            radius * 0.5,
            Stroke::new(1.0, Color32::from_white_alpha(30)),
        );
        for (label, az) in [("N", 0.0), ("E", 90.0), ("S", 180.0), ("W", 270.0)] {
            painter.text(
                on_circle(c, radius + 2.0, az) + (on_circle(Pos2::ZERO, 11.0, az) - Pos2::ZERO),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.0),
                if label == "N" {
                    Color32::from_rgb(255, 120, 100)
                } else {
                    Color32::WHITE
                },
            );
        }
        // The sun's path today, and sunrise and sunset ticks.
        let path: Vec<Pos2> = (0..=48)
            .filter_map(|i| {
                let p = sun_position(
                    &site,
                    &SunTime {
                        hour: i as f32 * 0.5 - 0.01 * (i == 48) as i32 as f32,
                        ..time
                    },
                );
                (p.elevation_deg > 0.0).then(|| {
                    on_circle(
                        c,
                        radius * (1.0 - p.elevation_deg as f32 / 90.0).clamp(0.08, 1.0),
                        p.azimuth_deg,
                    )
                })
            })
            .collect();
        for w in path.windows(2) {
            painter.line_segment(
                [w[0], w[1]],
                Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 200, 80, 90)),
            );
        }
        if let Some((rise, set)) = sunrise_sunset(&site, &time) {
            for h in [rise, set] {
                let az = sun_position(
                    &site,
                    &SunTime {
                        hour: h as f32,
                        ..time
                    },
                )
                .azimuth_deg;
                painter.line_segment(
                    [
                        on_circle(c, radius - 7.0, az),
                        on_circle(c, radius + 1.0, az),
                    ],
                    Stroke::new(3.0, Color32::from_rgb(255, 150, 60)),
                );
            }
        }
        let marker = on_circle(
            c,
            radius * (1.0 - pos.elevation_deg as f32 / 90.0).clamp(0.08, 1.0),
            pos.azimuth_deg,
        );
        let col = if pos.elevation_deg > 0.0 {
            Color32::from_rgb(255, 214, 80)
        } else {
            Color32::from_gray(120)
        };
        painter.line_segment([c, marker], Stroke::new(1.0, col.gamma_multiply(0.5)));
        painter.circle_filled(marker, 8.0, col);
        painter.circle_stroke(marker, 8.0, Stroke::new(1.5, Color32::BLACK));
        if let Some(p) = r
            .interact_pointer_pos()
            .filter(|_| (r.dragged() || r.clicked()) && sky_sun.is_none())
        {
            let d = p - c;
            let az = f64::from(d.x.atan2(-d.y).to_degrees()).rem_euclid(360.0);
            time.hour = hour_for_azimuth(&site, &time, az);
            time_edit = Edit::Live;
        }
        if (r.drag_stopped() || r.clicked()) && sky_sun.is_none() {
            time_edit = Edit::Done;
        }

        if !exact {
            return;
        }
        // Month dial.
        let (r, painter) = ui.allocate_painter(Vec2::splat(112.0), Sense::click_and_drag());
        let (c, radius) = (r.rect.center(), 44.0);
        painter.circle_filled(c, radius + 12.0, Color32::from_white_alpha(14));
        for (i, name) in t::MONTHS.iter().enumerate() {
            let p = on_circle(c, radius, i as f64 * 30.0);
            let active = i as u32 + 1 == time.month;
            if active {
                painter.circle_filled(p, 11.0, crate::ui::ACCENT);
            }
            painter.text(
                p,
                egui::Align2::CENTER_CENTER,
                &name[..if active { 3 } else { 1 }],
                egui::FontId::proportional(if active { 9.5 } else { 10.0 }),
                if active {
                    Color32::BLACK
                } else {
                    Color32::WHITE
                },
            );
        }
        if let Some(p) = r
            .interact_pointer_pos()
            .filter(|_| r.dragged() || r.clicked())
        {
            let d = p - c;
            let deg = f64::from(d.x.atan2(-d.y).to_degrees()).rem_euclid(360.0);
            time.month = ((deg + 15.0) / 30.0) as u32 % 12 + 1;
            time.day = time.day.min(days_in_month(time.year, time.month));
            time_edit = Edit::Live;
        }
        if r.drag_stopped() || r.clicked() {
            time_edit = Edit::Done;
        }
    });
    let rise_set = sunrise_sunset(&site, &time).map_or("no sunrise today".to_string(), |(a, b)| {
        format!("sunrise {}  sunset {}", hhmm(a), hhmm(b))
    });
    ui.label(format!(
        "azimuth {:.0}°  elevation {:.0}°  {rise_set}",
        pos.azimuth_deg, pos.elevation_deg
    ));

    // Date and time fields.
    let mut field_edit = |r: egui::Response| {
        if r.changed() {
            time_edit = Edit::Live;
        }
        if r.drag_stopped() || r.lost_focus() {
            time_edit = Edit::Done;
        }
    };
    ui.horizontal(|ui| {
        if exact {
            ui.label(t::DATE);
            field_edit(ui.add(egui::DragValue::new(&mut time.year).range(1900..=2100)));
            field_edit(ui.add(egui::DragValue::new(&mut time.month).range(1..=12)));
            let dim = days_in_month(time.year, time.month);
            time.day = time.day.min(dim);
            field_edit(ui.add(egui::DragValue::new(&mut time.day).range(1..=dim)));
        }
        ui.label(t::TIME);
        field_edit(
            ui.add(
                egui::DragValue::new(&mut time.hour)
                    .range(0.0..=23.99)
                    .speed(0.05)
                    .custom_formatter(|v, _| hhmm(v))
                    .custom_parser(|s| {
                        let (h, m) = s.split_once(':').unwrap_or((s, "0"));
                        Some(
                            h.trim().parse::<f64>().ok()?
                                + m.trim().parse::<f64>().unwrap_or(0.0) / 60.0,
                        )
                    }),
            ),
        );
    });
    years_fields(ui, cx);
    match time_edit {
        Edit::Live => cx.s.time_draft = Some(time),
        Edit::Done => {
            cx.s.time_draft = None;
            if time != committed {
                cx.run("time.set", json!({"year": time.year, "month": time.month, "day": time.day, "hour": time.hour.min(23.99)}));
                follow_summer_time(cx);
            }
        }
        Edit::None => {}
    }

    // Place.
    let mut s = site;
    let mut site_edit = Edit::None;
    ui.horizontal_wrapped(|ui| {
        let mut f =
            |ui: &mut egui::Ui, label: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>| {
                ui.label(label);
                let r = ui.add(
                    egui::DragValue::new(v)
                        .range(range)
                        .speed(0.01)
                        .max_decimals(3),
                );
                if r.changed() {
                    site_edit = Edit::Live;
                }
                if r.drag_stopped() || r.lost_focus() {
                    site_edit = Edit::Done;
                }
            };
        f(ui, t::LATITUDE, &mut s.latitude, -90.0..=90.0);
        f(ui, t::LONGITUDE, &mut s.longitude, -180.0..=180.0);
        let before = s.utc_offset_hours;
        f(ui, t::UTC_OFFSET, &mut s.utc_offset_hours, -14.0..=14.0);
        if s.utc_offset_hours != before {
            cx.s.auto_dst = false; // typed by hand: stays as typed
        }
    });
    ui.checkbox(&mut cx.s.auto_dst, t::SUMMER_TIME);
    match site_edit {
        Edit::Live => cx.s.site_draft = Some(s),
        Edit::Done => {
            cx.s.site_draft = None;
            if s != cx.s.project().site {
                cx.run("site.set", json!({"latitude": s.latitude, "longitude": s.longitude, "utc_offset_hours": s.utc_offset_hours}));
            }
        }
        Edit::None => {}
    }
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("city")
            .selected_text(t::CITY)
            .show_ui(ui, |ui| {
                for (name, lat, lon, off) in t::CITIES {
                    if ui.selectable_label(false, name).clicked() {
                        cx.s.base_offset = off;
                        let offset = if cx.s.auto_dst {
                            offset_for(off, &cx.s.project().time)
                        } else {
                            off
                        };
                        cx.run(
                            "site.set",
                            json!({"latitude": lat, "longitude": lon, "utc_offset_hours": offset}),
                        );
                    }
                }
            });
    });
    view_fields(ui, cx);
}

/// Keeps the UTC offset on the summer-time rule when the date changes.
fn follow_summer_time(cx: &mut Cx) {
    if !cx.s.auto_dst {
        return;
    }
    let want = offset_for(cx.s.base_offset, &cx.s.project().time);
    if (want - cx.s.project().site.utc_offset_hours).abs() > 1e-6 {
        cx.run("site.set", json!({"utc_offset_hours": want}));
    }
}

/// Years after planting: a slider (0..30) with the usual steps, live while dragged, and the
/// conflicts that growing brings.
fn years_fields(ui: &mut egui::Ui, cx: &mut Cx) {
    let committed = cx.s.project().view.years;
    ui.horizontal_wrapped(|ui| {
        ui.label(t::YEARS).on_hover_text(t::YEARS_TIP);
        let mut grow = committed.is_some();
        if ui.checkbox(&mut grow, t::YEARS_GROW).changed() {
            cx.s.set_years_draft(None);
            cx.run(
                "view.set",
                json!({"years": if grow { Some(0.0) } else { None }}),
            );
        }
        let Some(now) = cx.s.years_draft().or(committed) else {
            return;
        };
        let mut y = now;
        let r = ui.add(
            egui::Slider::new(&mut y, 0.0..=30.0)
                .step_by(0.5)
                .suffix(" y"),
        );
        if r.dragged() {
            cx.s.set_years_draft(Some(y));
        } else if r.drag_stopped() || (r.changed() && Some(y) != committed) {
            cx.s.set_years_draft(None);
            cx.run("view.set", json!({"years": y}));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if ui
            .button(t::COMPARE)
            .on_hover_text(t::COMPARE_TIP)
            .clicked()
        {
            // Toggles: the app closes an open wipe, else takes the picture.
            if cx.s.compare_open {
                cx.s.compare_close = true;
            } else {
                cx.s.compare_requested = true;
            }
        }
        for step in [0.0, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0] {
            if ui
                .selectable_label(committed == Some(step), format!("{step}"))
                .clicked()
            {
                cx.s.set_years_draft(None);
                cx.run("view.set", json!({"years": step}));
            }
        }
    });
    if let Some(years) = committed
        && let Ok(c) = cx.s.session.run(
            "planting.growth_conflicts",
            json!({"years": years.max(0.0)}),
        )
        && let Some(list) = c["conflicts"].as_array()
        && !list.is_empty()
    {
        egui::CollapsingHeader::new(egui::RichText::new(t::YEARS_CONFLICTS).small())
            .id_salt("sun-growth-conflicts")
            .show(ui, |ui| {
                for c in list.iter().take(12) {
                    let with = if c["kind"] == "wall" {
                        format!(
                            "the wall of {}",
                            c["with_name"].as_str().unwrap_or("a block")
                        )
                    } else {
                        c["with_name"]
                            .as_str()
                            .unwrap_or("another plant")
                            .to_string()
                    };
                    ui.small(format!(
                        "{} reaches {} in year {}",
                        c["name"].as_str().unwrap_or("A plant"),
                        with,
                        c["year"]
                    ));
                }
            });
    }
}

/// Exposure, haze, wind and ground: stored with the project through `view.set`.
fn view_fields(ui: &mut egui::Ui, cx: &mut Cx) {
    let v = cx.s.project().view.clone();
    ui.separator();
    ui.horizontal(|ui| {
        let mut auto = v.exposure_ev.is_none();
        if ui.checkbox(&mut auto, t::EXPOSURE_AUTO).changed() {
            cx.run(
                "view.set",
                json!({"exposure_ev": if auto { None } else { Some(13.0) }}),
            );
        }
        if let Some(mut ev) = v.exposure_ev
            && ui
                .add(
                    egui::Slider::new(&mut ev, 1.0..=18.0)
                        .text(t::EXPOSURE)
                        .max_decimals(1),
                )
                .drag_stopped()
        {
            cx.run("view.set", json!({"exposure_ev": ev}));
        }
    });
    ui.horizontal(|ui| {
        ui.label(t::LOOK);
        let now = serde_json::to_value(v.look).unwrap_or_default();
        for (key, label) in t::LOOKS {
            if ui.selectable_label(now == key, label).clicked() && now != key {
                cx.run("view.set", json!({"look": key}));
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(t::CLOUDS);
        let c = v.clouds;
        let now = serde_json::to_value(c.kind).unwrap_or_default();
        for (key, label) in t::CLOUD_KINDS {
            if ui.selectable_label(now == key, label).clicked() && now != key {
                let cover = if key == "overcast" {
                    1.0
                } else {
                    c.cover.min(0.6)
                };
                cx.run("view.set", json!({"clouds": {"kind": key, "cover": cover}}));
            }
        }
    });
    if v.clouds.kind != visioncraft_core::CloudKind::Clear {
        let mut cover = v.clouds.cover;
        if ui
            .add(
                egui::Slider::new(&mut cover, 0.0..=1.0)
                    .text(t::CLOUD_COVER)
                    .max_decimals(2),
            )
            .drag_stopped()
        {
            cx.run(
                "view.set",
                json!({"clouds": {"kind": v.clouds.kind, "cover": cover}}),
            );
        }
    }
    // Haze, wind and the ground plane: folded, they are rarely changed.
    egui::CollapsingHeader::new(egui::RichText::new(t::AIR_AND_GROUND).small())
        .id_salt("sun-air")
        .default_open(false)
        .show(ui, |ui| {
            let mut ground = v.ground.is_some();
            if ui.checkbox(&mut ground, t::GROUND).changed() {
                let g = if ground {
                    json!({"height": 0.0, "color": [0.065, 0.12, 0.035]})
                } else {
                    serde_json::Value::Null
                };
                cx.run("view.set", json!({"ground": g}));
            }
            let mut field = |ui: &mut egui::Ui,
                             label: &str,
                             key: &str,
                             mut value: f32,
                             range: std::ops::RangeInclusive<f32>| {
                if ui
                    .add(
                        egui::Slider::new(&mut value, range)
                            .text(label)
                            .max_decimals(1),
                    )
                    .drag_stopped()
                {
                    cx.run("view.set", json!({ key: value }));
                }
            };
            field(ui, t::HAZE, "haze", v.haze, 0.0..=20.0);
            field(ui, t::WIND, "wind_strength", v.wind_strength, 0.0..=2.0);
            field(
                ui,
                t::WIND_DIRECTION,
                "wind_direction_deg",
                v.wind_direction_deg,
                0.0..=360.0,
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn south_is_around_solar_noon() {
        // Brussels on summer time: the sun is south at about 13:40.
        let (site, time) = (Site::default(), SunTime::default());
        let h = hour_for_azimuth(&site, &time, 180.0);
        assert!((13.0..14.5).contains(&h), "{h}");
        let morning = hour_for_azimuth(&site, &time, 90.0);
        assert!(morning < h && morning > 6.0, "{morning}");
        let evening = hour_for_azimuth(&site, &time, 270.0);
        assert!(evening > h && evening < 22.5, "{evening}");
    }

    #[test]
    fn summer_time_follows_the_eu_rule() {
        assert_eq!(last_sunday(2026, 3), 29);
        assert_eq!(last_sunday(2026, 10), 25);
        assert_eq!(last_sunday(2024, 3), 31);
        assert!(!eu_summer_time(2026, 3, 29, 1.5));
        assert!(eu_summer_time(2026, 3, 29, 2.0));
        assert!(eu_summer_time(2026, 10, 25, 2.5));
        assert!(!eu_summer_time(2026, 10, 25, 3.0));
        assert!(!eu_summer_time(2026, 1, 15, 12.0));
        let t = SunTime {
            year: 2026,
            month: 6,
            day: 21,
            hour: 14.0,
        };
        assert_eq!(offset_for(1.0, &t), 2.0);
        assert_eq!(offset_for(0.0, &t), 1.0);
    }

    #[test]
    fn month_lengths() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 11), 30);
        assert_eq!(hhmm(13.5), "13:30");
        assert_eq!(hhmm(23.999), "00:00");
    }
}
