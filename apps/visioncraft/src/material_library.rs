//! The library's materials in Materials mode: the "Library" tab (kind chips, search, thumbnails),
//! turning a library entry into a project material, the surface-kind choices, and the library
//! looks the drawn shapes' plain palette materials get while the library is available.

use crate::library::{LibMaterial, Library, TexSpec, TexSrc};
use crate::library_ui::thumb;
use crate::strings as t;
use crate::studio::Studio;
use crate::ui::{ACCENT, Cx};
use eframe::egui::{self, Sense, Vec2};
use serde_json::{Value, json};
use visioncraft_core::material::SurfaceKind;
use visioncraft_core::{Material, builtin_palette};

pub const KINDS: [&str; 13] = [
    "brick", "plaster", "concrete", "wood", "paving", "gravel", "roof", "stone", "tile", "metal",
    "ground", "fabric", "other",
];

/// Which library material each plain palette material shows while the library is there
/// (lawn: a grass ground, paving: paving stones, plaster, timber and so on).
const DEFAULTS: [(&str, &str, f32); 9] = [
    ("lawn", "grass001", 0.6),
    ("paving", "pavingstones005", 0.4),
    ("plaster", "plaster001", 0.25),
    ("timber", "planks021", 0.2),
    ("brick", "bricks028", 0.25),
    ("roof", "roofingtiles006", 0.25),
    ("concrete", "concrete046", 0.3),
    ("gravel", "gravel016", 0.3),
    ("soil", "ground043", 0.4),
];

/// A library look for an unchanged palette material.
pub struct Overlay {
    pub spec: TexSpec,
    pub scale_m: f32,
    pub variation: f32,
}

fn spec_of_entry(m: &LibMaterial) -> TexSpec {
    let lib = |p: &Option<String>| p.clone().map(TexSrc::Lib);
    TexSpec {
        base: lib(&m.maps.basecolor),
        normal: lib(&m.maps.normal),
        orm: lib(&m.maps.orm),
        ..Default::default()
    }
}

/// Metres per texture repeat of a library material: its longer side, or 1 m when unknown.
pub fn scale_of(m: &LibMaterial) -> f32 {
    m.size_m.map_or(1.0, |s| s[0].max(s[1]).max(0.05))
}

/// The library look of palette material `m`, only while `m` is exactly as the app made it.
pub fn overlay(lib: &Library, m: &Material) -> Option<Overlay> {
    let (_, lib_id, variation) = DEFAULTS.iter().find(|d| d.0 == m.id)?;
    let entry = lib.materials.iter().find(|e| e.id == *lib_id)?;
    let builtin = builtin_palette().into_iter().find(|b| b.id == m.id)?;
    (builtin == *m).then(|| Overlay {
        spec: spec_of_entry(entry),
        scale_m: scale_of(entry),
        variation: *variation,
    })
}

/// The project material that shows library material `lib_id`: an existing one with the same
/// pictures, else a new one (`material.add`).
pub fn ensure_material(s: &mut Studio, lib_id: &str) -> Result<String, String> {
    let entry = s
        .scene
        .library
        .materials
        .iter()
        .find(|e| e.id == lib_id)
        .cloned()
        .ok_or_else(|| format!("no library material `{lib_id}`"))?;
    let base = entry.maps.basecolor.clone();
    if let Some(m) = s.project().materials.iter().find(|m| {
        m.maps.base_color.is_some()
            && m.maps.base_color == base
            && matches!(m.kind, SurfaceKind::StandardTriplanarPbr { .. })
    }) {
        return Ok(m.id.clone());
    }
    let orm = entry.maps.orm.clone();
    let r = s
        .session
        .run(
            "material.add",
            json!({
                "name": entry.name,
                "kind": {"type": "standard_triplanar_pbr", "map_scale_m": scale_of(&entry),
                         "relief": 1.0, "gloss": 0.0, "weathering": 0.0},
                // the maps scale the factors: 1 lets the pictures decide
                "base_color": [1.0, 1.0, 1.0, 1.0], "metallic": 1.0, "roughness": 1.0,
                "maps": {"base_color": base, "normal": entry.maps.normal,
                         // one packed occlusion-roughness-metallic picture: the same path in
                         // all three (the core has no field for it)
                         "roughness": orm, "metallic": orm, "ambient_occlusion": orm,
                         "height": entry.maps.height},
            }),
        )
        .map_err(|e| e.to_string())?;
    let id = r["id"].as_str().unwrap_or_default().to_string();
    s.scene
        .library
        .variation
        .insert(id.clone(), if entry.kind == "paving" { 0.4 } else { 0.25 });
    Ok(id)
}

/// The surface kinds of the selector, as `material.update` payloads (keeping a standard
/// surface's scale when going back to it).
pub fn kind_payload(index: usize, current: &SurfaceKind) -> Value {
    match index {
        0 => match current {
            SurfaceKind::StandardTriplanarPbr { .. } => serde_json::to_value(current).unwrap_or_default(),
            _ => json!({"type": "standard_triplanar_pbr", "map_scale_m": 1.0, "relief": 0.5, "gloss": 0.0, "weathering": 0.0}),
        },
        1 => json!({"type": "architectural_glass", "tint": [0.55, 0.75, 0.75], "roughness": 0.05}),
        2 => json!({"type": "animated_water", "flow_dir_deg": 0.0, "speed": 0.2, "tint": [0.02, 0.18, 0.20], "depth_m": 0.8}),
        _ => json!({"type": "lawn3d", "density_per_m2": 1500.0, "blade_height_m": 0.04, "seed": 1}),
    }
}

pub fn kind_index(k: &SurfaceKind) -> usize {
    match k {
        SurfaceKind::StandardTriplanarPbr { .. } => 0,
        SurfaceKind::ArchitecturalGlass { .. } => 1,
        SurfaceKind::AnimatedWater { .. } => 2,
        SurfaceKind::Lawn3d(_) => 3,
    }
}

#[derive(Default)]
pub struct MatLibUi {
    pub library_tab: bool,
    search: String,
    kind: Option<&'static str>,
}

impl MatLibUi {
    /// The tab switch; true while the project's own materials show.
    pub fn tabs(&mut self, ui: &mut egui::Ui) -> bool {
        ui.horizontal(|ui| {
            if ui.selectable_label(!self.library_tab, t::TAB_PROJECT).clicked() {
                self.library_tab = false;
            }
            if ui.selectable_label(self.library_tab, t::TAB_LIBRARY).clicked() {
                self.library_tab = true;
            }
        });
        !self.library_tab
    }

    /// The library grid; returns the id of the entry clicked.
    pub fn grid(&mut self, ui: &mut egui::Ui, cx: &mut Cx) -> Option<String> {
        if cx.s.scene.library.materials.is_empty() {
            ui.small(t::LIBRARY_LOADING);
            return None;
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(t::SEARCH)
                .desired_width(150.0),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 2.0);
            if ui.selectable_label(self.kind.is_none(), t::KIND_ALL).clicked() {
                self.kind = None;
            }
            for k in KINDS {
                if ui.selectable_label(self.kind == Some(k), k).clicked() {
                    self.kind = Some(k);
                }
            }
        });
        let needle = self.search.to_lowercase();
        let items: Vec<LibMaterial> = cx
            .s
            .scene
            .library
            .materials
            .iter()
            .filter(|m| self.kind.is_none_or(|k| m.kind == k))
            .filter(|m| {
                needle.is_empty()
                    || m.name.to_lowercase().contains(&needle)
                    || m.tags.iter().any(|t| t.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect();
        let used: Vec<Option<String>> = cx
            .s
            .project()
            .materials
            .iter()
            .map(|m| m.maps.base_color.clone())
            .collect();
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .max_height(210.0)
            .id_salt("matlib")
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(5.0);
                    for (i, m) in items.iter().enumerate() {
                        let (resp, painter) = ui.allocate_painter(Vec2::splat(52.0), Sense::click());
                        let visible = ui.is_rect_visible(resp.rect);
                        thumb(ui, cx, &m.thumbnail, resp.rect, visible, i as f32);
                        if used.contains(&m.maps.basecolor) {
                            painter.rect_stroke(
                                resp.rect,
                                egui::CornerRadius::same(6),
                                egui::Stroke::new(2.0, ACCENT),
                                egui::StrokeKind::Inside,
                            );
                        }
                        let size = m
                            .size_m
                            .map_or("size unknown (1 m)".to_string(), |s| format!("{} × {} m", s[0], s[1]));
                        let tip = format!("{} ({})\n{size}\n{} · {}", m.name, m.kind, m.license, m.author);
                        if resp.on_hover_text(tip).clicked() {
                            clicked = Some(m.id.clone());
                        }
                    }
                });
            });
        clicked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibMaps;

    fn entry(id: &str, size: Option<[f32; 2]>) -> LibMaterial {
        LibMaterial {
            id: id.into(),
            name: id.into(),
            tags: vec![],
            kind: "ground".into(),
            license: "CC0-1.0".into(),
            author: "a".into(),
            source_url: String::new(),
            size_m: size,
            maps: LibMaps {
                basecolor: Some(format!("materials/{id}/basecolor.jpg")),
                normal: Some(format!("materials/{id}/normal.jpg")),
                orm: Some(format!("materials/{id}/orm.jpg")),
                height: None,
            },
            thumbnail: String::new(),
        }
    }

    #[test]
    fn scale_comes_from_the_size_or_is_a_metre() {
        assert_eq!(scale_of(&entry("a", Some([1.4, 1.4]))), 1.4);
        assert_eq!(scale_of(&entry("a", Some([0.95, 0.75]))), 0.95);
        assert_eq!(scale_of(&entry("a", None)), 1.0);
    }

    #[test]
    fn palette_materials_get_a_library_look_only_while_unchanged() {
        let mut lib = Library::new();
        let lawn = builtin_palette().into_iter().find(|m| m.id == "lawn").unwrap();
        assert!(overlay(&lib, &lawn).is_none(), "no library, no look");
        lib.materials = vec![entry("grass001", Some([1.4, 1.4]))];
        let o = overlay(&lib, &lawn).expect("a grass look");
        assert_eq!(o.scale_m, 1.4);
        assert!(o.spec.orm.is_some());
        let mut edited = lawn.clone();
        edited.base_color = [0.0, 0.5, 0.0, 1.0];
        assert!(overlay(&lib, &edited).is_none(), "the user's colour stays");
        let glass = builtin_palette().into_iter().find(|m| m.id == "glass").unwrap();
        assert!(overlay(&lib, &glass).is_none());
    }

    #[test]
    fn every_surface_kind_choice_is_a_valid_kind() {
        let std = SurfaceKind::default();
        for i in 0..4 {
            let v = kind_payload(i, &std);
            let k: SurfaceKind = serde_json::from_value(v).expect("parses");
            assert_eq!(kind_index(&k), i);
        }
    }
}
