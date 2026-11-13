//! Materials mode: hover highlights the face under the cursor, a click opens a swatch grid.

use crate::scene::Pick;
use crate::strings as t;
use crate::studio::Draft;
use crate::ui::{ACCENT, Cx, Edit, panel_frame, slider_field};
use eframe::egui::{self, Color32, Pos2, Sense, Vec2};
use serde_json::json;
use std::collections::HashMap;
use visioncraft_core::material::{ColorSpace, MapUsage, McMatMap};
use visioncraft_core::{SurfaceKind, read_mcmat, resolve_materials};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Scope {
    #[default]
    Face,
    Body,
    Owner,
}

#[derive(Default)]
pub struct MaterialsState {
    picked: Option<(Pick, Pos2)>,
    scope: Scope,
    selected: Option<String>,
    eyedropper: bool,
    thumbs: HashMap<String, egui::TextureHandle>,
    /// Map bytes of imported `.mcmat` materials, kept for when the renderer can show textures.
    pub maps: HashMap<String, Vec<McMatMap>>,
    /// The Project | Library tabs of the swatch panel.
    lib: crate::material_library::MatLibUi,
}

pub fn to_srgb8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn to_linear(b: u8) -> f32 {
    let c = f32::from(b) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Decodes a PNG to RGBA8.
fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => px
            .chunks(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

/// Average colour in linear RGB and a small RGBA thumbnail of a base-colour map.
fn average_and_thumb(rgba: &[u8], w: u32, h: u32, srgb: bool) -> ([f32; 3], Vec<u8>) {
    let n = (w * h).max(1) as f32;
    let mut sum = [0.0f32; 3];
    for p in rgba.chunks(4) {
        for k in 0..3 {
            sum[k] += if srgb {
                to_linear(p[k])
            } else {
                f32::from(p[k]) / 255.0
            };
        }
    }
    let avg = sum.map(|s| s / n);
    const T: u32 = 32;
    let mut thumb = Vec::with_capacity((T * T * 4) as usize);
    for y in 0..T {
        for x in 0..T {
            let (sx, sy) = (x * w / T, y * h / T);
            let i = ((sy.min(h - 1) * w + sx.min(w - 1)) * 4) as usize;
            thumb.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    (avg, thumb)
}

impl MaterialsState {
    pub fn leave(&mut self, cx: &mut Cx) {
        self.picked = None;
        self.eyedropper = false;
        cx.s.highlight = None;
        cx.s.set_draft(None);
    }

    /// Adds a `.mcmat`: the base colour map's average becomes `base_color`, its thumbnail the swatch.
    pub fn add_mcmat(&mut self, cx: &mut Cx, bytes: &[u8]) {
        let mat = match read_mcmat(bytes) {
            Ok(m) => m,
            Err(e) => return cx.toasts.error(e.to_string()),
        };
        let base = mat.maps.iter().find(|m| m.usage == MapUsage::BaseColor);
        let decoded =
            base.and_then(|m| decode_png(&m.png).map(|d| (d, m.color_space == ColorSpace::Srgb)));
        let (color, thumb) = match decoded {
            Some(((w, h, rgba), srgb)) => {
                let (avg, thumb) = average_and_thumb(&rgba, w, h, srgb);
                ([avg[0], avg[1], avg[2], 1.0], Some(thumb))
            }
            None => ([0.5, 0.5, 0.5, 1.0], None),
        };
        // Every map becomes a texture (the pictures decide, so the factors are neutral).
        let stamp = cx.s.project().materials.len();
        let mut maps = serde_json::Map::new();
        for m in &mat.maps {
            let key = match m.usage {
                MapUsage::BaseColor => "base_color",
                MapUsage::Normal => "normal",
                MapUsage::Roughness => "roughness",
                MapUsage::Metallic => "metallic",
                MapUsage::AmbientOcclusion => "ambient_occlusion",
                MapUsage::Height => "height",
            };
            if m.usage != MapUsage::Height
                && let Some((w, h, px)) = decode_png(&m.png)
            {
                let name = format!("mcmat:{}#{stamp}/{key}", mat.name);
                cx.s.scene
                    .library
                    .add_memory_image(name.clone(), crate::assets::Rgba { w, h, px });
                maps.insert(key.into(), json!(name));
            }
        }
        let has = |k: &str| maps.contains_key(k);
        let color = if has("base_color") { [1.0; 4] } else { color };
        let (rough, metal) = (
            if has("roughness") { 1.0 } else { 0.8 },
            if has("metallic") { 1.0 } else { 0.0 },
        );
        let Some(r) = cx.run(
            "material.add",
            json!({"name": mat.name, "base_color": color, "roughness": rough, "metallic": metal,
                   "kind": {"type": "standard_triplanar_pbr", "map_scale_m": 1.0, "relief": 1.0, "gloss": 0.0, "weathering": 0.0},
                   "maps": maps}),
        ) else {
            return;
        };
        let id = r["id"].as_str().unwrap_or_default().to_string();
        if let Some(t) = thumb {
            let img = egui::ColorImage::from_rgba_unmultiplied([32, 32], &t);
            self.thumbs.insert(
                id.clone(),
                cx.ctx
                    .load_texture(format!("mat-{id}"), img, egui::TextureOptions::LINEAR),
            );
        }
        self.maps.insert(id.clone(), mat.maps);
        self.selected = Some(id);
        cx.toasts.info(t::MATERIAL_ADDED);
    }

    fn resolved(cx: &Cx, p: &Pick) -> Option<String> {
        let geom = cx.s.scene.geometry(&p.owner)?;
        let i = resolve_materials(cx.s.project(), &p.owner, geom)
            .get(p.face)
            .copied()
            .flatten()?;
        Some(cx.s.project().materials[i].id.clone())
    }

    pub fn update(&mut self, cx: &mut Cx) {
        let hover = cx
            .input
            .hover
            .and_then(|p| cx.s.pick(glam::Vec2::new(p.x, p.y), cx.size(), false));
        cx.s.highlight = hover
            .as_ref()
            .or(self.picked.as_ref().map(|p| &p.0))
            .map(|p| (p.owner.clone(), p.face));
        if let (Some(h), Some(pos), None) = (&hover, cx.input.hover, &self.picked) {
            cx.label(pos + Vec2::new(16.0, 18.0), &h.face_name);
        }
        if cx.input.escape {
            self.picked = None;
            self.eyedropper = false;
        }
        if cx.input.clicked {
            match (hover, cx.input.hover) {
                (Some(h), Some(pos)) if self.eyedropper => {
                    self.selected = Self::resolved(cx, &h);
                    self.eyedropper = false;
                    self.picked = Some((h, pos));
                }
                (Some(h), Some(pos)) => {
                    self.selected = Self::resolved(cx, &h);
                    self.picked = Some((h, pos));
                }
                _ => self.picked = None,
            }
        }
    }

    fn target(&self, p: &Pick) -> serde_json::Value {
        match self.scope {
            Scope::Face => json!({"type": "face", "owner": p.owner, "face": p.face_name}),
            Scope::Body => json!({"type": "body", "owner": p.owner, "body": p.body}),
            Scope::Owner => json!({"type": "owner", "owner": p.owner}),
        }
    }

    pub fn panel(&mut self, cx: &mut Cx) {
        let rect = cx.input.rect;
        let Some((pick, click)) = self.picked.clone() else {
            let _ = rect;
            return;
        };
        let pos = Pos2::new(
            (click.x + 14.0).clamp(8.0, (rect.width() - 300.0).max(8.0)),
            (click.y + 14.0).clamp(96.0, (rect.height() - 420.0).max(96.0)),
        );
        let ctx = cx.ctx.clone();
        egui::Area::new(egui::Id::new("matpanel"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                panel_frame().show(ui, |ui| {
                    ui.set_max_width((rect.width() - 24.0).min(290.0));
                    ui.small(&pick.face_name);
                    ui.horizontal(|ui| {
                        for (s, label) in [
                            (Scope::Face, t::THIS_FACE),
                            (Scope::Body, t::WHOLE_BODY),
                            (Scope::Owner, t::WHOLE_OBJECT),
                        ] {
                            ui.selectable_value(&mut self.scope, s, label);
                        }
                    });
                    let materials = cx.s.project().materials.clone();
                    let project_tab = self.lib.tabs(ui);
                    if !project_tab && let Some(lib_id) = self.lib.grid(ui, cx) {
                        match crate::material_library::ensure_material(cx.s, &lib_id) {
                            Ok(id) => {
                                self.selected = Some(id.clone());
                                let target = self.target(&pick);
                                cx.run(
                                    "material.assign",
                                    json!({"target": target, "material": id}),
                                );
                            }
                            Err(e) => cx.toasts.error(e),
                        }
                    }
                    if project_tab {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::splat(5.0);
                            for m in &materials {
                                let (r, painter) =
                                    ui.allocate_painter(Vec2::splat(34.0), Sense::click());
                                let rr = egui::CornerRadius::same(7);
                                match self.thumbs.get(&m.id) {
                                    Some(tex) => {
                                        egui::Image::new(tex)
                                            .corner_radius(rr)
                                            .paint_at(ui, r.rect);
                                    }
                                    None => {
                                        // A library material shows its library picture.
                                        let lib = m.maps.base_color.as_ref().and_then(|p| {
                                            cx.s.scene
                                                .library
                                                .materials
                                                .iter()
                                                .find(|e| e.maps.basecolor.as_ref() == Some(p))
                                                .map(|e| e.thumbnail.clone())
                                        });
                                        if let Some(path) = lib {
                                            let vis = ui.is_rect_visible(r.rect);
                                            crate::library_ui::thumb(
                                                ui, cx, &path, r.rect, vis, 0.0,
                                            );
                                        } else {
                                            let c = Color32::from_rgb(
                                                to_srgb8(m.base_color[0]),
                                                to_srgb8(m.base_color[1]),
                                                to_srgb8(m.base_color[2]),
                                            );
                                            painter.rect_filled(r.rect, rr, c);
                                        }
                                    }
                                }
                                let sel = self.selected.as_ref() == Some(&m.id);
                                painter.rect_stroke(
                                    r.rect,
                                    rr,
                                    egui::Stroke::new(
                                        if sel { 3.0 } else { 1.0 },
                                        if sel {
                                            ACCENT
                                        } else {
                                            Color32::from_white_alpha(70)
                                        },
                                    ),
                                    egui::StrokeKind::Inside,
                                );
                                if r.on_hover_text(&m.name).clicked() {
                                    self.selected = Some(m.id.clone());
                                    let target = self.target(&pick);
                                    cx.run(
                                        "material.assign",
                                        json!({"target": target, "material": m.id}),
                                    );
                                }
                            }
                        });
                    }
                    if ui
                        .selectable_label(self.eyedropper, t::EYEDROPPER)
                        .clicked()
                    {
                        self.eyedropper = !self.eyedropper;
                    }
                    self.sliders(ui, cx);
                });
            });
    }

    fn sliders(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let current = match &cx.s.draft {
            Some(Draft::Material(m)) if m.id == id => Some(m.clone()),
            _ => {
                cx.s.project()
                    .materials
                    .iter()
                    .find(|m| m.id == id)
                    .cloned()
            }
        };
        let Some(mut m) = current else { return };
        ui.separator();
        ui.label(egui::RichText::new(&m.name).strong());
        // Surface kind: standard, glass, water or lawn.
        let current_kind = crate::material_library::kind_index(&m.kind);
        ui.horizontal(|ui| {
            ui.label(t::SURFACE);
            for (i, name) in t::SURFACE_KINDS.iter().enumerate() {
                if ui.selectable_label(current_kind == i, *name).clicked() && current_kind != i {
                    let kind = crate::material_library::kind_payload(i, &m.kind);
                    cx.run("material.update", json!({"id": id, "kind": kind}));
                }
            }
        });
        let mut variation =
            cx.s.scene
                .library
                .variation
                .get(&id)
                .copied()
                .unwrap_or(0.0);
        if ui
            .add(
                egui::Slider::new(&mut variation, 0.0..=1.0)
                    .text(t::VARIATION)
                    .max_decimals(2),
            )
            .changed()
        {
            cx.s.scene.library.variation.insert(id.clone(), variation);
            cx.s.force_sync();
        }
        let SurfaceKind::StandardTriplanarPbr {
            map_scale_m,
            relief,
            gloss,
            weathering,
        } = &mut m.kind
        else {
            return;
        };
        let edits = [
            slider_field(ui, t::MAP_SCALE, map_scale_m, 0.1..=10.0),
            slider_field(ui, t::RELIEF, relief, 0.0..=1.0),
            slider_field(ui, t::GLOSS, gloss, 0.0..=1.0),
            slider_field(ui, t::WEATHERING, weathering, 0.0..=1.0),
        ];
        if edits.contains(&Edit::Done) {
            cx.s.set_draft(None);
            let original =
                cx.s.project()
                    .materials
                    .iter()
                    .find(|x| x.id == id)
                    .map(|x| x.kind.clone());
            if original.as_ref() != Some(&m.kind) {
                cx.run("material.update", json!({"id": id, "kind": m.kind}));
            }
        } else if edits.contains(&Edit::Live) {
            cx.s.set_draft(Some(Draft::Material(m)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trips() {
        for b in [0u8, 1, 50, 128, 200, 255] {
            assert_eq!(to_srgb8(to_linear(b)), b);
        }
    }

    #[test]
    fn mcmat_base_colour_decodes_to_an_average() {
        let mat = read_mcmat(include_bytes!(
            "../../../crates/visioncraft-core/tests/data/test.mcmat"
        ))
        .unwrap();
        let base = mat
            .maps
            .iter()
            .find(|m| m.usage == MapUsage::BaseColor)
            .expect("test.mcmat has a base colour");
        let (w, h, rgba) = decode_png(&base.png).expect("decodes");
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        let (avg, thumb) = average_and_thumb(&rgba, w, h, true);
        assert!(avg.iter().all(|c| (0.0..=1.0).contains(c)));
        assert_eq!(thumb.len(), 32 * 32 * 4);
    }
}
