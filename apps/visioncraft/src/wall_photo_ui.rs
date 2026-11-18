//! A photo on a wall in the window: the straightening step. The photo is shown with four corner
//! handles to drag onto the facade's corners, and the region of the wall it covers can be typed.
//! Applying runs `wall_photo.add` or `wall_photo.set`; this file only collects the drags and
//! numbers.

use crate::assets::{Key, Loaded};
use crate::strings as t;
use crate::ui::{Cx, panel_frame};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::{Value, json};
use visioncraft_core::WallPhoto;
use visioncraft_core::wall_photo::{Region, image_corners};

/// The handles' colours, in the order bottom-left, bottom-right, top-right, top-left.
const HANDLE: [Color32; 4] = [
    Color32::from_rgb(80, 150, 255),
    Color32::from_rgb(250, 220, 60),
    Color32::from_rgb(70, 210, 110),
    Color32::from_rgb(240, 80, 80),
];
const GRAB_PT: f32 = 16.0;
/// The longest side the window shows the photo at, points.
const SHOWN_PT: f32 = 640.0;

/// A photo being fitted to a face.
struct Fitting {
    target: Value,
    /// The photo being changed, `None` for a new one.
    existing: Option<String>,
    file: String,
    name: String,
    size_px: [u32; 2],
    /// The facade's corners in the photo, pixels: bottom-left, bottom-right, top-right, top-left.
    corners: [[f32; 2]; 4],
    whole: bool,
    /// u0, v0, u1, v1 as typed.
    region: [String; 4],
    texture: Option<egui::TextureHandle>,
    dragging: Option<usize>,
}

#[derive(Default)]
pub struct WallPhotoUi {
    fitting: Option<Fitting>,
}

fn metres(v: f32) -> String {
    format!("{v:.2}")
}

/// The corners of a photo as first offered: a little inside its edges.
fn first_corners(size: [u32; 2]) -> [[f32; 2]; 4] {
    let (w, h) = (size[0] as f32, size[1] as f32);
    image_corners(size).map(|c| {
        [
            c[0] + (w / 2.0 - c[0]) * 0.12,
            c[1] + (h / 2.0 - c[1]) * 0.12,
        ]
    })
}

impl WallPhotoUi {
    pub fn is_open(&self) -> bool {
        self.fitting.is_some()
    }

    /// A new photo (its file held by the asset store) for the face `target` names.
    pub fn begin_new(&mut self, target: Value, name: &str, file: String, size_px: [u32; 2]) {
        self.fitting = Some(Fitting {
            target,
            existing: None,
            file,
            name: name.to_string(),
            size_px,
            corners: first_corners(size_px),
            whole: true,
            region: Default::default(),
            texture: None,
            dragging: None,
        });
    }

    /// An existing photo's corners and region again.
    pub fn begin_edit(&mut self, cx: &mut Cx, w: &WallPhoto) {
        cx.s.scene
            .library
            .store
            .want(Key::Thumb(w.file.clone()), 1.0);
        let region = w
            .region
            .map_or_else(Default::default, |r| [r.u0, r.v0, r.u1, r.v1].map(metres));
        self.fitting = Some(Fitting {
            target: serde_json::to_value(&w.target).unwrap_or_default(),
            existing: Some(w.id.clone()),
            file: w.file.clone(),
            name: w.name.clone(),
            size_px: w.size_px,
            corners: w.corners_px,
            whole: w.region.is_none(),
            region,
            texture: None,
            dragging: None,
        });
    }

    /// The straightening window, once a frame while a photo is being fitted.
    pub fn window(&mut self, cx: &mut Cx) {
        if let Some(id) = cx.s.wall_photo_edit.take()
            && let Some(w) =
                cx.s.project()
                    .wall_photos
                    .iter()
                    .find(|w| w.id == id)
                    .cloned()
        {
            self.begin_edit(cx, &w);
        }
        let Some(f) = &mut self.fitting else { return };
        if f.texture.is_none()
            && let Some(Loaded::Image(mut img)) =
                cx.s.scene.library.store.take(&Key::Thumb(f.file.clone()))
        {
            crate::assets::shrink(&mut img, 2048);
            let image =
                egui::ColorImage::from_rgba_unmultiplied([img.w as usize, img.h as usize], &img.px);
            f.texture = Some(cx.ctx.load_texture(
                format!("wallphoto-{}", f.file),
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
        // The wall's size, to type a region against.
        let face = match &f.target {
            Value::Object(o) => cx.s.scene.face_frame_of(
                o.get("owner").and_then(Value::as_str).unwrap_or_default(),
                o.get("face").and_then(Value::as_str).unwrap_or_default(),
            ),
            _ => Err("no face".to_string()),
        };
        let mut apply = false;
        let mut cancel = false;
        let ctx = cx.ctx.clone();
        egui::Window::new(t::WALL_PHOTO_TITLE)
            .collapsible(false)
            .resizable(false)
            .default_pos(Pos2::new(40.0, 90.0))
            .frame(panel_frame())
            .show(&ctx, |ui| {
                ui.set_max_width(SHOWN_PT + 24.0);
                ui.small(t::WALL_PHOTO_HINT);
                fit_corners(ui, f);
                ui.separator();
                ui.checkbox(&mut f.whole, t::WALL_PHOTO_WHOLE);
                if let Ok(frame) = &face {
                    ui.small(format!(
                        "{} {:.2} × {:.2} m",
                        t::WALL_PHOTO_WALL_IS,
                        frame.size[0],
                        frame.size[1]
                    ));
                    if !f.whole && f.region.iter().all(String::is_empty) {
                        f.region = [0.0, 0.0, frame.size[0], frame.size[1]].map(metres);
                    }
                }
                if !f.whole {
                    ui.small(t::WALL_PHOTO_REGION);
                    ui.horizontal(|ui| {
                        for (i, label) in [
                            t::WALL_PHOTO_LEFT,
                            t::WALL_PHOTO_RIGHT,
                            t::WALL_PHOTO_BOTTOM,
                            t::WALL_PHOTO_TOP,
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            // typed in the order u0, u1, v0, v1; kept as u0, v0, u1, v1
                            let k = [0, 2, 1, 3][i];
                            ui.label(label);
                            ui.add(
                                egui::TextEdit::singleline(&mut f.region[k]).desired_width(52.0),
                            );
                        }
                    });
                }
                if let Err(why) = &face {
                    ui.colored_label(Color32::from_rgb(255, 170, 60), why);
                }
                ui.horizontal(|ui| {
                    apply = ui
                        .add_enabled(face.is_ok(), egui::Button::new(t::WALL_PHOTO_APPLY))
                        .clicked();
                    cancel = ui.button(t::WALL_PHOTO_CANCEL).clicked();
                });
            });
        if cx.input.escape {
            cancel = true;
        }
        if apply {
            let region = if f.whole {
                Ok(None)
            } else {
                let n: Vec<Option<f32>> = f
                    .region
                    .iter()
                    .map(|s| s.trim().replace(',', ".").parse().ok())
                    .collect();
                match n[..] {
                    [Some(u0), Some(v0), Some(u1), Some(v1)] => {
                        let r = Region { u0, v0, u1, v1 };
                        r.validate().map(|()| Some(r))
                    }
                    _ => Err(t::WALL_PHOTO_BAD_REGION.to_string()),
                }
            };
            match region {
                Err(why) => cx.toasts.error(why),
                Ok(region) => {
                    let corners = f.corners.map(|c| [c[0].round(), c[1].round()]);
                    let done = match &f.existing {
                        Some(id) => cx.run(
                            "wall_photo.set",
                            json!({"id": id, "corners_px": corners, "region": region}),
                        ),
                        None => {
                            let mut p = json!({"file": f.file, "target": f.target,
                                "corners_px": corners, "name": f.name, "size_px": f.size_px});
                            if let Some(r) = region {
                                p["region"] = json!(r);
                            }
                            cx.run("wall_photo.add", p)
                        }
                    };
                    if done.is_some() {
                        self.fitting = None;
                    }
                }
            }
        } else if cancel {
            self.fitting = None;
        }
    }
}

/// The photo with its four handles; drags move them.
fn fit_corners(ui: &mut egui::Ui, f: &mut Fitting) {
    let (w, h) = (f.size_px[0] as f32, f.size_px[1] as f32);
    let k = (SHOWN_PT / w.max(h)).clamp(0.05, 1.0);
    let size = Vec2::new(w * k, h * k);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let to_screen = |c: [f32; 2]| Pos2::new(rect.min.x + c[0] * k, rect.min.y + c[1] * k);
    let painter = ui.painter_at(rect.expand(20.0));
    match &f.texture {
        Some(tex) => {
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            painter.rect_filled(rect, 0.0, Color32::from_gray(30));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                t::WALL_PHOTO_LOADING,
                FontId::proportional(14.0),
                Color32::from_gray(190),
            );
        }
    }
    if response.drag_started()
        && let Some(p) = response.interact_pointer_pos()
    {
        f.dragging = (0..4)
            .map(|i| (i, to_screen(f.corners[i]).distance(p)))
            .filter(|(_, d)| *d <= GRAB_PT * 1.6)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
    }
    if let (Some(i), Some(p)) = (f.dragging, response.interact_pointer_pos())
        && response.dragged()
    {
        f.corners[i] = [
            ((p.x - rect.min.x) / k).clamp(0.0, w),
            ((p.y - rect.min.y) / k).clamp(0.0, h),
        ];
    }
    if response.drag_stopped() {
        f.dragging = None;
    }
    let pts: Vec<Pos2> = f.corners.iter().map(|c| to_screen(*c)).collect();
    painter.add(egui::Shape::closed_line(
        pts.clone(),
        Stroke::new(2.0, Color32::from_white_alpha(220)),
    ));
    for (i, p) in pts.iter().enumerate() {
        painter.circle_filled(*p, 8.0, HANDLE[i]);
        painter.circle_stroke(*p, 8.0, Stroke::new(1.5, Color32::BLACK));
        painter.text(
            *p + Vec2::new(11.0, -11.0),
            Align2::LEFT_BOTTOM,
            (i + 1).to_string(),
            FontId::proportional(14.0),
            HANDLE[i],
        );
    }
}
