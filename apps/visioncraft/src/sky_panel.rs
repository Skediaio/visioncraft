//! Skies: the picker in the Landscape & Sun panel (physical sky and the library's HDRIs), the
//! HDRI's calibration, "use this sky's sun", and the ground colour.
//!
//! Calibration: an HDRI's pixel values are scaled so that the sun in the image gives the
//! illuminance a clear sky has with the sun at that height (`Sun::from_angles`), which is what
//! the automatic exposure assumes; an image without a sun (overcast) gets a sky of about
//! 5000 cd/m2 (about 20 klx on the ground) and no direct sun. The renderer's own normalisation
//! helper has not landed yet; this is the app-side stand-in.

use crate::assets::{HdrData, Key, Loaded};
use crate::strings as t;
use crate::studio::Studio;
use crate::ui::Cx;
use eframe::egui::{self, Sense, Vec2};
use glam::{Quat, Vec3};
use serde_json::json;
use visioncraft_core::SkyChoice;
use visioncraft_render::{HdriImage, Sky, Sun, estimate_sun};

struct Hdr {
    id: String,
    data: HdrData,
    /// The sun at rotation 0: direction, and whether the image shows one.
    sun: Option<Vec3>,
    /// The `intensity` that calibrates this image.
    calibrated: f32,
}

#[derive(Default)]
pub struct SkyState {
    hdr: Option<Hdr>,
    /// Sky id asked for by a click, waiting for its image to choose the intensity.
    pub pick: Option<String>,
    applied: Option<SkyChoice>,
    pub use_sun: bool,
    /// The ground colour while its picker is being dragged.
    pub ground_draft: Option<[f32; 3]>,
}

const OVERCAST_CD: f32 = 5000.0;

fn mean_luminance(d: &HdrData) -> f32 {
    let (mut sum, mut weight) = (0.0f64, 0.0f64);
    for y in 0..d.h / 2 {
        let w = f64::from(((y as f32 + 0.5) / d.h as f32 * std::f32::consts::PI).sin());
        for x in 0..d.w {
            let i = ((y * d.w + x) * 3) as usize;
            let l = 0.2126 * d.rgb[i] + 0.7152 * d.rgb[i + 1] + 0.0722 * d.rgb[i + 2];
            sum += f64::from(l) * w;
            weight += w;
        }
    }
    (sum / weight.max(1e-9)) as f32
}

fn az_el(dir: Vec3) -> (f32, f32) {
    (
        dir.x.atan2(-dir.z).to_degrees().rem_euclid(360.0),
        dir.y.clamp(-1.0, 1.0).asin().to_degrees(),
    )
}

impl Hdr {
    fn new(id: String, data: HdrData) -> Self {
        let mean = mean_luminance(&data).max(1e-6);
        let image = HdriImage {
            width: data.w,
            height: data.h,
            rgb: &data.rgb,
        };
        let found = estimate_sun(&image, 0.0);
        let by_mean = OVERCAST_CD / mean;
        let calibrated = match found {
            Some((dir, illuminance, _)) => {
                let lux = Sun::from_angles(az_el(dir).0, az_el(dir).1.max(1.0)).illuminance;
                // A clipped sun would make the sky far too bright: keep within a factor of the
                // mean-based value for a typical sky (6000 cd/m2).
                let typical = 6000.0 / mean;
                (lux / illuminance.max(1e-6)).clamp(typical * 0.25, typical * 4.0)
            }
            None => by_mean,
        };
        Self {
            id,
            data,
            sun: found.map(|f| f.0),
            calibrated,
        }
    }
}

impl SkyState {
    /// The calibrated intensity of the loaded image of sky `id`, if it is the one loaded.
    pub fn calibrated(&self, id: &str) -> Option<f32> {
        self.hdr.as_ref().filter(|h| h.id == id).map(|h| h.calibrated)
    }

    /// The sun taken from the sky image, with its elevation, when "use this sky's sun" is on.
    pub fn sun_override(&self) -> Option<(Sun, f32)> {
        self.sky_sun_with(&self.applied_choice())
    }

    fn applied_choice(&self) -> Option<SkyChoice> {
        self.applied.clone()
    }

    fn sky_sun_with(&self, choice: &Option<SkyChoice>) -> Option<(Sun, f32)> {
        if !self.use_sun {
            return None;
        }
        let (Some(SkyChoice::Hdri { id, rotation_deg, .. }), Some(h)) = (choice, &self.hdr) else {
            return None;
        };
        if h.id != *id {
            return None;
        }
        Some(match h.sun {
            Some(dir) => {
                let d = Quat::from_rotation_y(-rotation_deg.to_radians()) * dir;
                let (az, el) = az_el(d);
                (Sun::from_angles(az, el), el)
            }
            // Overcast: light from the sky only.
            None => (
                Sun {
                    direction: Vec3::Y,
                    illuminance: 0.0,
                    color: Vec3::ONE,
                },
                45.0,
            ),
        })
    }

    pub fn sun_active(&self) -> bool {
        self.sun_override().is_some()
    }

    /// Compass azimuth and elevation (degrees) of the sky's sun, for the read-only compass.
    pub fn sun_angles(&self) -> Option<(f64, f64)> {
        self.sun_override().map(|(s, el)| {
            let (az, _) = az_el(s.direction);
            (f64::from(az), f64::from(el))
        })
    }

    /// Exposure for the sun at `elevation`; an image without a sun is lit like an overcast day.
    pub fn auto_ev(&self, elevation: f32, lamps: bool) -> f32 {
        let overcast = matches!(self.applied, Some(SkyChoice::Hdri { .. }))
            && self.hdr.as_ref().is_some_and(|h| h.sun.is_none());
        crate::studio::auto_ev(if overcast { elevation.min(10.0) } else { elevation }, lamps)
    }
}

/// Keeps the renderer's sky in line with the project's view state; loads the image when needed.
pub fn apply(s: &mut Studio) {
    let choice = s.project().view.sky.clone();
    // A click on a sky waits for its image, then chooses the calibrated intensity.
    if let Some(id) = s.sky.pick.clone() {
        let file = s.scene.library.skies.iter().find(|k| k.id == id).map(|k| k.file.clone());
        match file {
            Some(file) => {
                if s.sky.hdr.as_ref().is_none_or(|h| h.id != id) {
                    take_or_ask(s, &id, &file);
                }
                if let Some(c) = s.sky.calibrated(&id) {
                    s.sky.pick = None;
                    let _ = s.session.run(
                        "view.set",
                        json!({"sky": {"type": "hdri", "id": id, "rotation_deg": 0.0, "intensity": c}}),
                    );
                }
            }
            None => s.sky.pick = None,
        }
    }
    match &choice {
        SkyChoice::Physical => {
            if s.sky.applied.as_ref() != Some(&choice) {
                let _ = s.renderer.set_sky(&Sky::Physical);
                s.sky.applied = Some(choice);
            }
        }
        SkyChoice::Hdri {
            id,
            rotation_deg,
            intensity,
        } => {
            let file = s.scene.library.skies.iter().find(|k| k.id == *id).map(|k| k.file.clone());
            let Some(file) = file else { return };
            if s.sky.hdr.as_ref().is_none_or(|h| h.id != *id) {
                take_or_ask(s, id, &file);
            }
            let ready = s.sky.hdr.as_ref().filter(|h| h.id == *id);
            if let Some(h) = ready
                && s.sky.applied.as_ref() != Some(&choice)
            {
                let image = HdriImage {
                    width: h.data.w,
                    height: h.data.h,
                    rgb: &h.data.rgb,
                };
                let _ = s.renderer.set_sky(&Sky::Hdri {
                    image,
                    rotation_deg: *rotation_deg,
                    intensity: *intensity,
                });
                s.sky.applied = Some(choice.clone());
            }
        }
    }
    // The ground colour while the picker is dragged.
    if let (Some(c), Some(g)) = (s.sky.ground_draft, s.project().view.ground) {
        s.renderer.set_ground(Some(visioncraft_render::Ground {
            height: g.height,
            color: c,
            ..Default::default()
        }));
    }
}

fn take_or_ask(s: &mut Studio, id: &str, file: &str) {
    let key = Key::Hdr(file.to_string());
    let store = &mut s.scene.library.store;
    match store.take(&key) {
        Some(Loaded::Hdr(d)) => s.sky.hdr = Some(Hdr::new(id.to_string(), d)),
        _ => store.want(key, 0.0),
    }
}

pub fn panel(ui: &mut egui::Ui, cx: &mut Cx) {
    ui.separator();
    ui.label(egui::RichText::new(t::SKY).strong());
    let skies = cx.s.scene.library.skies.clone();
    let current = cx.s.project().view.sky.clone();
    let is_physical = matches!(current, SkyChoice::Physical);
    let mut chosen: Option<Option<String>> = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(5.0);
        if ui.selectable_label(is_physical, t::SKY_PHYSICAL).clicked() {
            chosen = Some(None);
        }
        for k in &skies {
            let (resp, painter) = ui.allocate_painter(Vec2::new(64.0, 36.0), Sense::click());
            let visible = ui.is_rect_visible(resp.rect);
            crate::library_ui::thumb(ui, cx, &k.thumbnail, resp.rect, visible, 0.0);
            if matches!(&current, SkyChoice::Hdri { id, .. } if *id == k.id) {
                painter.rect_stroke(
                    resp.rect,
                    egui::CornerRadius::same(6),
                    egui::Stroke::new(3.0, crate::ui::ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
            if resp
                .on_hover_text(format!("{}\n{} · {}", k.name, k.license, k.author))
                .clicked()
            {
                chosen = Some(Some(k.id.clone()));
            }
        }
    });
    match chosen {
        Some(None) => {
            cx.s.sky.use_sun = false;
            cx.run("view.set", json!({"sky": {"type": "physical"}}));
        }
        Some(Some(id)) => cx.s.sky.pick = Some(id),
        None => {}
    }
    if cx.s.sky.pick.is_some() {
        ui.small(t::LOADING);
    }
    if let SkyChoice::Hdri {
        id,
        rotation_deg,
        intensity,
    } = current
    {
        let calibrated = cx.s.sky.calibrated(&id).unwrap_or(intensity.max(1.0));
        let (mut rot, mut stops) = (rotation_deg, (intensity / calibrated).max(1e-3).log2());
        let a = ui.add(egui::Slider::new(&mut rot, 0.0..=360.0).text(t::SKY_ROTATION));
        let b = ui.add(egui::Slider::new(&mut stops, -3.0..=3.0).text(t::SKY_INTENSITY).max_decimals(1));
        if a.drag_stopped() || b.drag_stopped() || a.lost_focus() || b.lost_focus() {
            cx.run(
                "view.set",
                json!({"sky": {"type": "hdri", "id": id, "rotation_deg": rot, "intensity": calibrated * stops.exp2()}}),
            );
        }
        let has_sun = cx.s.sky.hdr.as_ref().is_some_and(|h| h.id == id && h.sun.is_some());
        if cx.s.sky.use_sun {
            ui.small(if has_sun { t::SKY_SUN_FROM_IMAGE } else { t::SKY_NO_SUN });
            if ui.button(t::SKY_BACK_TO_DATE).clicked() {
                cx.s.sky.use_sun = false;
            }
        } else if ui.button(t::SKY_USE_SUN).clicked() {
            cx.s.sky.use_sun = true;
        }
    }
    ground_color(ui, cx);
}

fn ground_color(ui: &mut egui::Ui, cx: &mut Cx) {
    let Some(g) = cx.s.project().view.ground else {
        return;
    };
    let srgb = |c: f32| crate::materials::to_srgb8(c);
    let mut rgb = [srgb(g.color[0]), srgb(g.color[1]), srgb(g.color[2])];
    ui.horizontal(|ui| {
        ui.label(t::GROUND_COLOR);
        let r = ui.color_edit_button_srgb(&mut rgb);
        let linear = rgb.map(|b| {
            let c = f32::from(b) / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        });
        if r.changed() {
            cx.s.sky.ground_draft = Some(linear);
        }
        if cx.s.sky.ground_draft.is_some() && !ui.input(|i| i.pointer.any_down()) && !r.has_focus() {
            cx.s.sky.ground_draft = None;
            if linear != g.color {
                cx.run("view.set", json!({"ground": {"height": g.height, "color": linear}}));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_luminance_of_a_flat_sky_is_its_value() {
        let d = HdrData {
            w: 16,
            h: 8,
            rgb: vec![2.0; 16 * 8 * 3],
        };
        assert!((mean_luminance(&d) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn an_overcast_image_is_calibrated_to_a_bright_overcast_sky_without_a_sun() {
        let d = HdrData {
            w: 64,
            h: 32,
            rgb: vec![0.5; 64 * 32 * 3],
        };
        let h = Hdr::new("flat".into(), d);
        assert!(h.sun.is_none());
        assert!((h.calibrated * 0.5 - OVERCAST_CD).abs() < 1.0);
    }

    #[test]
    fn a_sun_is_found_and_the_sky_scaled_to_clear_sky_lux() {
        let (w, h) = (256u32, 128u32);
        let mut rgb = vec![0.0f32; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                let sky = if y < h / 2 { 1.0 } else { 0.2 };
                rgb[i..i + 3].copy_from_slice(&[sky * 0.5, sky * 0.7, sky]);
            }
        }
        // a sun at azimuth 90 (u = 0.75), elevation about 40 (row 0.22 * h)
        for y in 26..30 {
            for x in 190..194 {
                let i = ((y * w + x) * 3) as usize;
                rgb[i..i + 3].copy_from_slice(&[4000.0, 3600.0, 3000.0]);
            }
        }
        let hdr = Hdr::new("sunny".into(), HdrData { w, h, rgb });
        let dir = hdr.sun.expect("a sun");
        let (az, el) = az_el(dir);
        assert!((az - 90.0).abs() < 8.0 && el > 25.0 && el < 55.0, "{az} {el}");
        assert!(hdr.calibrated > 0.0);
    }
}
