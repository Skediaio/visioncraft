//! The Terrain tab of Landscape & Sun: raise, lower, smooth and flatten brushes, painting with
//! library materials, and ponds (carved and filled with water). Every change is a `Session`
//! command (`terrain.*`, `shape.add_area`); while a brush is dragged the stroke so far is shown
//! as a draft, and on release it is one `terrain.stroke`. Single plants and objects that stood on
//! the ground move with it in the same undo step.

use crate::planting_tools::ground_at;
use crate::scene::line;
use crate::strings as t;
use crate::studio::{Draft, Studio};
use crate::ui::{Cx, slider_field};
use eframe::egui;
use glam::Vec3;
use serde_json::{Value, json};
use std::sync::Arc;
use visioncraft_core::{BrushStroke, CommandError, Generator, Session, Terrain, apply_stroke};
use visioncraft_render::LineVertex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    Raise,
    Lower,
    Smooth,
    Flatten,
    Paint,
    Pond,
}

impl Brush {
    const ALL: [Brush; 6] = [
        Brush::Raise,
        Brush::Lower,
        Brush::Smooth,
        Brush::Flatten,
        Brush::Paint,
        Brush::Pond,
    ];

    fn label(self) -> &'static str {
        match self {
            Brush::Raise => t::TR_RAISE,
            Brush::Lower => t::TR_LOWER,
            Brush::Smooth => t::TR_SMOOTH,
            Brush::Flatten => t::TR_FLATTEN,
            Brush::Paint => t::TR_PAINT,
            Brush::Pond => t::TR_POND,
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Brush::Pond => t::TR_POND_HINT,
            Brush::Flatten => t::TR_FLATTEN_HINT,
            _ => t::TR_BRUSH_HINT,
        }
    }

    /// The core's tool name.
    fn tool(self) -> &'static str {
        match self {
            Brush::Raise => "raise",
            Brush::Lower => "lower",
            Brush::Smooth => "smooth",
            Brush::Flatten => "flatten",
            Brush::Paint | Brush::Pond => "paint",
        }
    }
}

pub struct TerrainTool {
    /// The Terrain tab is open (else Sun & sky).
    pub open: bool,
    pub brush: Brush,
    pub radius: f32,
    /// 0..1; raise and lower move up to 15 cm per dab.
    pub strength: f32,
    pub layer: usize,
    pub pond_depth: f32,
    path: Vec<[f32; 2]>,
    flatten_to: Option<f32>,
    pond: Vec<[f32; 2]>,
    lib_pick: Option<String>,
}

impl Default for TerrainTool {
    fn default() -> Self {
        Self {
            open: false,
            brush: Brush::Raise,
            radius: 3.0,
            strength: 0.3,
            layer: 0,
            pond_depth: 0.8,
            path: Vec::new(),
            flatten_to: None,
            pond: Vec::new(),
            lib_pick: None,
        }
    }
}

/// Metres per dab at full strength for raise and lower.
const MAX_STEP_M: f32 = 0.15;

impl TerrainTool {
    /// The `terrain.stroke` parameters of `path` with the current settings.
    pub fn stroke_params(&self, path: &[[f32; 2]]) -> Value {
        let lift = matches!(self.brush, Brush::Raise | Brush::Lower);
        let mut p = json!({
            "tool": self.brush.tool(),
            "points": path,
            "radius_m": self.radius,
            "strength": if lift { self.strength * MAX_STEP_M } else { self.strength },
            "falloff": 0.5,
        });
        if self.brush == Brush::Flatten
            && let Some(h) = self.flatten_to
        {
            p["target_height"] = json!(h);
        }
        if self.brush == Brush::Paint {
            p["layer"] = json!(self.layer);
        }
        p
    }

    pub fn leave(&mut self, cx: &mut Cx) {
        if cx.s.camera.view == crate::camera::View::Top {
            cx.s.camera.view = crate::camera::View::Orbit;
        }
        self.path.clear();
        self.pond.clear();
        if matches!(cx.s.draft, Some(Draft::Terrain(_))) {
            cx.s.set_draft(None);
        }
    }

    pub fn update(&mut self, cx: &mut Cx) {
        if !self.open {
            return;
        }
        let Some(terrain) = cx.s.project().terrain.clone() else {
            return;
        };
        let ctrl = cx.ctx.input(|i| i.modifiers.command);
        let ground = cx.input.hover.and_then(|h| ground_at(cx, h));
        let mut top: Vec<LineVertex> = Vec::new();
        if self.brush == Brush::Pond {
            self.pond_input(cx, ground, &terrain, &mut top);
            cx.s.extra = (Vec::new(), top);
            return;
        }
        // Ctrl orbits; otherwise the left button sculpts.
        cx.s.pointer_captured = !ctrl && ground.is_some() || !self.path.is_empty();
        if let Some(g) = ground {
            ring(&terrain, [g.x, g.z], self.radius, &mut top);
            let p = [g.x, g.z];
            if !ctrl && cx.input.drag_started {
                self.path = vec![p];
                self.flatten_to = terrain.height_at(g.x, g.z);
            } else if !self.path.is_empty() && cx.input.dragging {
                let last = *self.path.last().expect("not empty");
                let d = ((p[0] - last[0]).powi(2) + (p[1] - last[1]).powi(2)).sqrt();
                if d > self.radius * 0.25 {
                    self.path.push(p);
                    self.preview(cx, &terrain);
                }
            } else if !ctrl && cx.input.clicked {
                self.flatten_to = terrain.height_at(g.x, g.z);
                self.commit(cx, vec![p]);
            }
        }
        if !self.path.is_empty() && cx.input.drag_stopped {
            let path = std::mem::take(&mut self.path);
            self.commit(cx, path);
        }
        cx.s.extra = (Vec::new(), top);
    }

    /// The stroke so far, applied to a copy of the terrain and shown as a draft.
    fn preview(&self, cx: &mut Cx, terrain: &Arc<Terrain>) {
        let Ok(s) = serde_json::from_value::<BrushStroke>(self.stroke_params(&self.path)) else {
            return;
        };
        let mut t = (**terrain).clone();
        if s.validate(&t).is_ok() {
            apply_stroke(&mut t, &s);
            cx.s.set_draft(Some(Draft::Terrain(Arc::new(t))));
        }
    }

    fn commit(&mut self, cx: &mut Cx, path: Vec<[f32; 2]>) {
        let p = self.stroke_params(&path);
        if matches!(cx.s.draft, Some(Draft::Terrain(_))) {
            cx.s.set_draft(None);
        }
        let r = cx.s.with_lamps(|s| {
            let before = s.project().terrain.clone();
            s.run("terrain.stroke", p)?;
            follow_ground(s, before.as_deref())
        });
        if let Err(e) = r {
            cx.toasts.error(e.to_string());
        }
    }

    fn pond_input(
        &mut self,
        cx: &mut Cx,
        ground: Option<Vec3>,
        terrain: &Arc<Terrain>,
        top: &mut Vec<LineVertex>,
    ) {
        if cx.input.escape {
            self.pond.clear();
        } else if cx.input.backspace {
            self.pond.pop();
        }
        // Twelve points on screen around the first corner, in metres (the old guess from the
        // eye's distance to the origin took 4 m in a plan and closed ponds a corner early).
        let h = cx.input.rect.height().max(1.0);
        let close = if cx.s.camera.view == crate::camera::View::Top {
            12.0 * 2.0 * cx.s.camera.top_half_height / h
        } else {
            let d = self.pond.first().map_or(10.0, |q| {
                cx.s.camera.eye().distance(Vec3::new(q[0], 0.0, q[1]))
            });
            12.0 * 2.0 * (cx.s.camera.fov_y / 2.0).tan() * d / h
        };
        if let Some(g) = ground {
            let p = [g.x, g.z];
            if cx.input.clicked {
                if crate::draw::closes(&self.pond, p, close) {
                    self.finish_pond(cx, terrain);
                } else {
                    self.pond.push(p);
                }
            }
            let mut pts = self.pond.clone();
            pts.push(p);
            let y = |q: [f32; 2]| terrain.height_at(q[0], q[1]).unwrap_or(0.0) + 0.05;
            for w in pts.windows(2) {
                top.extend(line(
                    Vec3::new(w[0][0], y(w[0]), w[0][1]),
                    Vec3::new(w[1][0], y(w[1]), w[1][1]),
                    [120, 200, 255, 255],
                    3.0,
                ));
            }
        }
        if (cx.input.enter || cx.input.double_clicked) && self.pond.len() >= 3 {
            self.finish_pond(cx, terrain);
        }
    }

    /// Carves the outline and fills it with water a little below the old ground, one step.
    fn finish_pond(&mut self, cx: &mut Cx, terrain: &Arc<Terrain>) {
        let outline = std::mem::take(&mut self.pond);
        if outline.len() < 3 {
            return;
        }
        let rim = outline
            .iter()
            .filter_map(|p| terrain.height_at(p[0], p[1]))
            .fold(f32::INFINITY, f32::min);
        let level = if rim.is_finite() {
            rim - POND_MARGIN_M
        } else {
            -POND_MARGIN_M
        };
        let depth = self.pond_depth;
        // The water reaches as far out as the carved bank is below it: no dry strip of bank shows
        // between the water's edge and the grass.
        let water = offset_outline(&outline, wet_bank(depth, POND_BANK_M, POND_MARGIN_M));
        let r = cx.s.with_lamps(|s| {
            let before = s.project().terrain.clone();
            s.run(
                "terrain.carve",
                json!({"outline": outline, "depth_m": depth, "bank_m": POND_BANK_M, "bed_material": "soil"}),
            )?;
            s.run(
                "shape.add_area",
                json!({"name": t::TR_POND_NAME, "outline": water, "elevation": level, "drape": false, "material": "water"}),
            )?;
            follow_ground(s, before.as_deref())
        });
        if let Err(e) = r {
            cx.toasts.error(e.to_string());
        }
    }

    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        let Some(terrain) = cx.s.project().terrain.clone() else {
            ui.small(t::TR_NONE);
            if ui.button(t::TR_CREATE).clicked() {
                let p = create_params(cx.s);
                cx.run("terrain.create", p);
            }
            return;
        };
        ui.small(self.brush.hint());
        if self.brush == Brush::Pond {
            slider_field(ui, t::TR_DEPTH, &mut self.pond_depth, 0.2..=3.0);
        } else {
            ui.horizontal(|ui| {
                ui.label(t::PT_RADIUS);
                ui.add(
                    egui::Slider::new(&mut self.radius, 0.5..=30.0)
                        .logarithmic(true)
                        .suffix(" m"),
                );
            });
            ui.horizontal(|ui| {
                ui.label(t::TR_STRENGTH);
                ui.add(egui::Slider::new(&mut self.strength, 0.02..=1.0));
            });
        }
        if self.brush == Brush::Paint {
            self.layers_ui(ui, cx, &terrain);
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.small(format!(
                "{:.0} × {:.0} m, {} m {}",
                terrain.width as f32 * terrain.cell_m,
                terrain.depth as f32 * terrain.cell_m,
                terrain.cell_m,
                t::TR_CELLS
            ));
            if ui.small_button(t::TR_REMOVE).clicked() {
                cx.run("terrain.remove", json!({}));
            }
        });
        // The brushes last, next to the tabs: they stay put whatever the brush shows above.
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            for b in Brush::ALL {
                if ui.selectable_label(self.brush == b, b.label()).clicked() {
                    self.brush = b;
                    self.pond.clear();
                }
            }
        });
    }

    fn layers_ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx, terrain: &Terrain) {
        ui.label(egui::RichText::new(t::TR_LAYERS).strong());
        let names: Vec<String> = terrain
            .layers
            .iter()
            .map(|l| {
                cx.s.project()
                    .materials
                    .iter()
                    .find(|m| m.id == l.material)
                    .map_or_else(|| l.material.clone(), |m| m.name.clone())
            })
            .collect();
        self.layer = self.layer.min(names.len().saturating_sub(1));
        ui.horizontal_wrapped(|ui| {
            for (i, n) in names.iter().enumerate() {
                if ui.selectable_label(self.layer == i, n).clicked() {
                    self.layer = i;
                }
            }
        });
        if terrain.layers.len() >= visioncraft_core::terrain::MAX_LAYERS {
            return;
        }
        let lib: Vec<(String, String)> =
            cx.s.scene
                .library
                .materials
                .iter()
                .map(|m| (m.id.clone(), m.name.clone()))
                .collect();
        let mut add = None;
        egui::ComboBox::from_id_salt("terrain-layer-add")
            .selected_text(t::TR_ADD_LAYER)
            .height(260.0)
            .show_ui(ui, |ui| {
                for (id, name) in &lib {
                    if ui
                        .selectable_label(self.lib_pick.as_ref() == Some(id), name)
                        .clicked()
                    {
                        add = Some(id.clone());
                    }
                }
            });
        if let Some(id) = add {
            self.lib_pick = Some(id.clone());
            let made = crate::material_library::material_for(cx.s, &id);
            let r = made.map_err(CommandError::Failed).and_then(|m| {
                cx.s.with_lamps(|s| {
                    let mat = match m {
                        Ok(existing) => existing,
                        Err(params) => s.run("material.add", params)?["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    };
                    s.run("terrain.add_layer", json!({ "material": mat }))
                })
            });
            match r {
                Ok(v) => self.layer = v["index"].as_u64().unwrap_or(0) as usize,
                Err(e) => cx.toasts.error(e.to_string()),
            }
        }
    }
}

/// A terrain over everything there is, with 20 m to spare (at least 60 × 60 m), at 0.5 m cells
/// while that stays within the core's 1024 cells per side.
pub fn create_params(s: &Studio) -> Value {
    let mut lo = [-30.0f32, -30.0];
    let mut hi = [30.0f32, 30.0];
    if let Some((a, b)) = s.scene.bounds() {
        lo = [lo[0].min(a.x - 20.0), lo[1].min(a.z - 20.0)];
        hi = [hi[0].max(b.x + 20.0), hi[1].max(b.z + 20.0)];
    }
    let (w, d) = (hi[0] - lo[0], hi[1] - lo[1]);
    let cell = (w.max(d) / 1024.0).max(0.5);
    let base = if s.project().materials.iter().any(|m| m.id == "lawn") {
        "lawn".to_string()
    } else {
        s.project()
            .materials
            .first()
            .map_or_else(String::new, |m| m.id.clone())
    };
    json!({"origin": lo, "width_m": w, "depth_m": d, "cell_m": cell, "base_material": base})
}

/// Single plants and objects that stood on the old terrain stand on the new one (a
/// `planting.update` each, in the caller's transaction).
pub fn follow_ground(s: &mut Session, before: Option<&Terrain>) -> Result<(), CommandError> {
    let (Some(before), Some(after)) = (before, s.project().terrain.clone()) else {
        return Ok(());
    };
    let moves: Vec<(String, [f32; 3])> = s
        .project()
        .generators
        .iter()
        .filter_map(|g| {
            let Generator::Single(g) = g else { return None };
            let [x, y, z] = g.position;
            let old = before.height_at(x, z)?;
            let new = after.height_at(x, z)?;
            ((y - old).abs() < 0.05 && (new - old).abs() > 1e-3)
                .then(|| (g.id.clone(), [x, new + (y - old), z]))
        })
        .collect();
    for (id, position) in moves {
        s.run("planting.update", json!({"id": id, "position": position}))?;
    }
    Ok(())
}

/// The brush outline on the ground.
fn ring(t: &Terrain, c: [f32; 2], r: f32, out: &mut Vec<LineVertex>) {
    let n = 48;
    let at = |i: usize| {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let (x, z) = (c[0] + r * a.cos(), c[1] + r * a.sin());
        Vec3::new(x, t.height_at(x, z).unwrap_or(0.0) + 0.05, z)
    };
    for i in 0..n {
        out.extend(line(at(i), at(i + 1), [255, 255, 255, 220], 2.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_core::Project;

    #[test]
    fn stroke_params_pass_the_spec_and_scale_lifting() {
        let s = Session::new(Project::default());
        let spec = s.command_spec("terrain.stroke").unwrap();
        let mut tool = TerrainTool::default();
        for b in Brush::ALL {
            tool.brush = b;
            tool.flatten_to = Some(1.0);
            let p = tool.stroke_params(&[[0.0, 0.0], [1.0, 0.0]]);
            spec.check(&p).unwrap_or_else(|e| panic!("{b:?}: {e}"));
        }
        tool.brush = Brush::Raise;
        tool.strength = 1.0;
        assert_eq!(
            tool.stroke_params(&[[0.0, 0.0]])["strength"],
            json!(MAX_STEP_M)
        );
    }

    #[test]
    fn a_single_on_the_ground_follows_a_raise() {
        let mut s = Session::new(Project::default());
        s.run(
            "terrain.create",
            json!({"origin": [0, 0], "width_m": 20, "depth_m": 20, "base_material": "lawn"}),
        )
        .unwrap();
        s.run(
            "planting.add_single",
            json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [10, 0, 10]}),
        )
        .unwrap();
        s.run(
            "planting.add_single",
            json!({"name": "Lamp on a roof", "asset": "x", "position": [5, 3, 5]}),
        )
        .unwrap();
        let rev = s.revision();
        s.transaction(|s| {
            let before = s.project().terrain.clone();
            s.run(
                "terrain.stroke",
                json!({"tool": "raise", "points": [[10, 10], [5, 5]], "radius_m": 4, "strength": 0.5}),
            )?;
            follow_ground(s, before.as_deref())
        })
        .unwrap();
        assert_eq!(s.revision(), rev + 1, "one undo step");
        let t = s.project().terrain.clone().unwrap();
        let pos = |i: usize| match &s.project().generators[i] {
            Generator::Single(g) => g.position,
            _ => unreachable!(),
        };
        assert!((pos(0)[1] - t.height_at(10.0, 10.0).unwrap()).abs() < 1e-4);
        assert!(pos(0)[1] > 0.1);
        assert_eq!(pos(1)[1], 3.0, "an object above the ground stays");
    }
}

/// A pond's bank slopes up over this distance outside its outline.
const POND_BANK_M: f32 = 1.5;
/// Its water stands this far below the lowest point of the old ground on the outline.
const POND_MARGIN_M: f32 = 0.15;

/// How far outside the outline the carved bank is still below the water: the carve lowers the
/// ground by `depth * (1 - smoothstep(d / bank))` at distance `d` outside (the core's
/// `Terrain::carve`), the water is `margin` below the old ground.
fn wet_bank(depth: f32, bank: f32, margin: f32) -> f32 {
    if depth <= margin {
        return 0.0;
    }
    let target = 1.0 - margin / depth;
    let smooth = |x: f32| x * x * (3.0 - 2.0 * x);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if smooth(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo * bank
}

/// `outline` moved `d` outward along its edges' normals (mitred corners, capped at 3 d).
fn offset_outline(outline: &[[f32; 2]], d: f32) -> Vec<[f32; 2]> {
    let n = outline.len();
    if d <= 0.0 || n < 3 {
        return outline.to_vec();
    }
    let area2: f32 = (0..n)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    // Outward for a counter-clockwise ring (positive area in x, z) is to the right of an edge.
    let side = if area2 > 0.0 { 1.0 } else { -1.0 };
    let normal = |a: [f32; 2], b: [f32; 2]| {
        let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dz).max(1e-6);
        [side * dz / l, -side * dx / l]
    };
    (0..n)
        .map(|i| {
            let p = outline[i];
            let n1 = normal(outline[(i + n - 1) % n], p);
            let n2 = normal(p, outline[(i + 1) % n]);
            let dot = (n1[0] * n2[0] + n1[1] * n2[1]).max(-0.9);
            let k = (d / (1.0 + dot)).min(3.0 * d);
            [p[0] + (n1[0] + n2[0]) * k, p[1] + (n1[1] + n2[1]) * k]
        })
        .collect()
}

#[cfg(test)]
mod pond_tests {
    use super::*;

    #[test]
    fn the_water_covers_the_bank_up_to_the_waterline() {
        let d = wet_bank(0.8, POND_BANK_M, POND_MARGIN_M);
        // At d the carved bank is exactly at the waterline.
        let s = |x: f32| x * x * (3.0 - 2.0 * x);
        let lowered = 0.8 * (1.0 - s(d / POND_BANK_M));
        assert!((lowered - POND_MARGIN_M).abs() < 1e-3 && d > 0.5 && d < POND_BANK_M);
        assert_eq!(wet_bank(0.1, POND_BANK_M, POND_MARGIN_M), 0.0);
        // A 4 by 2 square, either winding, grows by d on every side.
        for sq in [
            vec![[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0]],
            vec![[0.0, 0.0], [0.0, 2.0], [4.0, 2.0], [4.0, 0.0]],
        ] {
            let o = offset_outline(&sq, 1.0);
            let (lo, hi) = o
                .iter()
                .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| {
                    (
                        [lo[0].min(p[0]), lo[1].min(p[1])],
                        [hi[0].max(p[0]), hi[1].max(p[1])],
                    )
                });
            assert!(
                (lo[0] + 1.0).abs() < 1e-4 && (hi[0] - 5.0).abs() < 1e-4,
                "{o:?}"
            );
            assert!(
                (lo[1] + 1.0).abs() < 1e-4 && (hi[1] - 3.0).abs() < 1e-4,
                "{o:?}"
            );
        }
    }
}
