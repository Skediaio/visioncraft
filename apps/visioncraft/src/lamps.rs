//! Lamps: the lights of catalogue fixtures and the Lights tool for bare point and spot lights.
//!
//! Every placement of a lamp asset, made by any planting tool, gets its `Light` (the core's
//! `lights_for_placements`, `source` = the generator's id) from [`reconcile_commands`], which also
//! moves the lights with moved, turned or scaled fixtures and removes them with removed ones.
//! [`crate::studio::Studio::run`] runs it in the same `Session::transaction` as the command that
//! changed the fixture, so a lamp and its light are one undo step.

use crate::strings as t;
use crate::ui::{Cx, Edit, slider_field};
use eframe::egui;
use glam::Vec3;
use serde_json::{Value, json};
use visioncraft_core::project::LAYER_LIGHTING;
use visioncraft_core::{
    Generator, Light, LightKind, LightSpec, Placement, Project, lights_for_placements,
};

/// At most this many lights come from fixtures: every light costs shading time and only a few
/// cast shadows, so a brush of 500 lamps lights its first ones and says so.
pub const MAX_FIXTURE_LIGHTS: usize = 64;

/// What [`reconcile_commands`] wants run, and how many lamps got no light over the cap.
#[derive(Debug, Default)]
pub struct Reconcile {
    pub commands: Vec<(&'static str, Value)>,
    pub skipped: usize,
}

/// The commands that bring the lights in line with the fixtures (pure; tested). `lamps` are the
/// catalogue assets that carry a light; `placements_of` evaluates a generator (only generators
/// that use a lamp are evaluated).
pub fn reconcile_commands(
    project: &Project,
    lamps: &[(String, LightSpec)],
    placements_of: &dyn Fn(&Generator) -> Vec<Placement>,
) -> Reconcile {
    let mut out = Reconcile::default();
    let mut budget = MAX_FIXTURE_LIGHTS;
    for g in &project.generators {
        let used: Vec<&(String, LightSpec)> =
            lamps.iter().filter(|(id, _)| g.uses_asset(id)).collect();
        let mut want: Vec<Light> = Vec::new();
        if !used.is_empty() {
            let placements = placements_of(g);
            for (asset, spec) in used {
                let mine: Vec<Placement> = placements
                    .iter()
                    .filter(|p| &p.asset == asset)
                    .cloned()
                    .collect();
                let (lights, skipped) =
                    lights_for_placements(spec, &mine, "", g.id(), LAYER_LIGHTING, budget);
                budget -= lights.len();
                out.skipped += skipped;
                want.extend(lights);
            }
        }
        let have: Vec<&Light> = project
            .lights
            .iter()
            .filter(|l| l.source.as_deref() == Some(g.id()))
            .collect();
        let n = want.len();
        for (i, w) in want.into_iter().enumerate() {
            match have.get(i) {
                None => {
                    let name = if n == 1 {
                        format!("{} light", g.name())
                    } else {
                        format!("{} light {}", g.name(), i + 1)
                    };
                    out.commands.push((
                        "light.add",
                        json!({"name": name, "position": w.position, "kind": w.kind,
                               "color_temperature_k": w.color_temperature_k, "lumens": w.lumens,
                               "range_m": w.range_m, "source": g.id()}),
                    ));
                }
                Some(l) if l.position != w.position || l.kind != w.kind => out.commands.push((
                    "light.update",
                    json!({"id": l.id, "position": w.position, "kind": w.kind}),
                )),
                Some(_) => {}
            }
        }
        for l in have.iter().skip(n) {
            out.commands.push(("light.remove", json!({"id": l.id})));
        }
    }
    for l in &project.lights {
        if let Some(src) = &l.source
            && !project.generators.iter().any(|g| g.id() == src)
        {
            out.commands.push(("light.remove", json!({"id": l.id})));
        }
    }
    out
}

/// Shows what the lamp reconcile found (see `Studio::reconcile_lamps`).
pub fn reconcile(cx: &mut Cx) {
    for e in cx.s.reconcile_lamps() {
        cx.toasts.error(e);
    }
}

// ---- the Lights tool ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Point,
    Spot,
}

pub struct LightsTool {
    kind: Kind,
    kelvin: f32,
    lumens: f32,
    range: f32,
    height: f32,
    inner: f32,
    outer: f32,
    selected: Option<String>,
    /// While a spot's target handle is dragged: the ground point it is at.
    aiming: Option<Vec3>,
}

impl Default for LightsTool {
    fn default() -> Self {
        Self {
            kind: Kind::Point,
            kelvin: 2700.0,
            lumens: 800.0,
            range: 8.0,
            height: 2.5,
            inner: 20.0,
            outer: 35.0,
            selected: None,
            aiming: None,
        }
    }
}

fn line(a: Vec3, b: Vec3, color: [u8; 4], w: f32) -> [visioncraft_render::LineVertex; 2] {
    crate::scene::line(a, b, color, w)
}

/// Distance from a spot to the point it aims at, shown as a handle.
const HANDLE_M: f32 = 3.0;
/// How close (points) the pointer must be to grab the handle.
const HANDLE_PX: f32 = 18.0;

impl LightsTool {
    fn light<'a>(&self, cx: &'a Cx) -> Option<&'a Light> {
        let id = self.selected.as_ref()?;
        cx.s.project().lights.iter().find(|l| &l.id == id)
    }

    fn ground(cx: &Cx) -> Option<Vec3> {
        let p = cx.input.hover?;
        let (o, d) = cx.s.pointer_ray(glam::Vec2::new(p.x, p.y), cx.size());
        match cx.s.scene.raycast(o, d, false) {
            Some(h) => Some(h.point),
            None => crate::camera::ray_plane(o, d, 0.0),
        }
    }

    pub fn update(&mut self, cx: &mut Cx) {
        let mut top = Vec::new();
        cx.s.light_markers = true;
        let ground = Self::ground(cx);
        // The handle of the selected spot.
        let spot = self.light(cx).and_then(|l| match l.kind {
            LightKind::Spot { direction, .. } => {
                let d = Vec3::from(direction).normalize_or_zero();
                Some((l.id.clone(), Vec3::from(l.position), d))
            }
            LightKind::Point => None,
        });
        let handle = spot
            .as_ref()
            .map(|(_, p, d)| self.aiming.unwrap_or(*p + *d * HANDLE_M));
        let handle_at = handle.and_then(|h| cx.project(h));
        let near = |p: Option<eframe::egui::Pos2>| {
            handle_at
                .zip(p)
                .is_some_and(|(h, p)| h.distance(p) < HANDLE_PX)
        };
        let near_handle = near(cx.input.hover);
        // The press, not the pointer now: a drag has moved away by the time it is recognised.
        let press_on_handle = near(cx.input.press_origin);
        if near_handle || self.aiming.is_some() {
            cx.s.pointer_captured = true;
        }
        if let (Some((id, pos, _)), Some(g)) = (&spot, ground) {
            if (cx.input.drag_started && press_on_handle)
                || (self.aiming.is_some() && cx.input.dragging)
            {
                self.aiming = Some(g);
            }
            if self.aiming.is_some() && cx.input.drag_stopped {
                let dir = (self.aiming.take().unwrap_or(g) - *pos).normalize_or_zero();
                if dir != Vec3::ZERO {
                    let (inner, outer) = (self.inner, self.outer);
                    cx.run(
                        "light.update",
                        json!({"id": id, "kind": {"type": "spot", "direction": dir.to_array(), "inner_cone_deg": inner, "outer_cone_deg": outer}}),
                    );
                }
            }
        }
        if let (Some((_, pos, _)), Some(h)) = (&spot, handle) {
            top.extend(line(*pos, h, [255, 214, 120, 255], 2.0));
            top.extend(crate::draw::dot(
                [h.x, h.z],
                h.y,
                [255, 255, 255, 255],
                14.0,
            ));
        }
        if self.aiming.is_none()
            && !near_handle
            && let Some(g) = ground
        {
            let at = g + Vec3::Y * self.height;
            top.extend(line(g, at, [255, 255, 255, 120], 1.5));
            top.extend(line(at, at, [255, 214, 120, 255], 9.0));
            if cx.input.clicked {
                self.click(cx, at);
            }
        }
        if cx.input.delete
            && let Some(id) = self.selected.take()
        {
            cx.run("light.remove", json!({"id": id}));
        }
        if let Some(l) = self.light(cx) {
            let p = Vec3::from(l.position);
            top.extend(line(p, p, [255, 255, 255, 255], 16.0));
        }
        cx.s.extra = (Vec::new(), top);
    }

    /// A click: selects a light under the pointer, else places a new one at `at`.
    fn click(&mut self, cx: &mut Cx, at: Vec3) {
        let hit = cx.input.hover.and_then(|p| {
            cx.s.project()
                .lights
                .iter()
                .filter(|l| l.enabled)
                .filter_map(|l| {
                    let s = cx.project(Vec3::from(l.position))?;
                    (s.distance(p) < 16.0).then(|| (s.distance(p), l.id.clone()))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
        });
        if let Some((_, id)) = hit {
            self.selected = Some(id);
            return;
        }
        let kind = match self.kind {
            Kind::Point => json!({"type": "point"}),
            Kind::Spot => json!({"type": "spot", "direction": [0.0, -1.0, 0.0],
                                 "inner_cone_deg": self.inner, "outer_cone_deg": self.outer}),
        };
        let name = if self.kind == Kind::Point {
            "Point light"
        } else {
            "Spot light"
        };
        let params = json!({"name": name, "position": at.to_array(), "kind": kind,
                            "color_temperature_k": self.kelvin, "lumens": self.lumens, "range_m": self.range});
        if let Some(r) = cx.run("light.add", params) {
            self.selected = r["id"].as_str().map(str::to_string);
        }
    }

    pub fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Cx) {
        ui.small(t::LIGHTS_HINT);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.kind, Kind::Point, t::LIGHT_POINT);
            ui.selectable_value(&mut self.kind, Kind::Spot, t::LIGHT_SPOT);
        });
        // The values of the selected light (edits are committed on release), else the defaults
        // for the next one.
        let selected = self.light(cx).cloned();
        let (mut k, mut lm, mut rg) = selected
            .as_ref()
            .map_or((self.kelvin, self.lumens, self.range), |l| {
                (l.color_temperature_k, l.lumens, l.range_m)
            });
        let edits = [
            slider_field(ui, t::KELVIN, &mut k, 1800.0..=6500.0),
            slider_field(ui, t::LUMENS, &mut lm, 50.0..=4000.0),
            slider_field(ui, t::RANGE, &mut rg, 1.0..=30.0),
        ];
        match &selected {
            Some(l) if edits.contains(&Edit::Done) => {
                cx.run(
                    "light.update",
                    json!({"id": l.id, "color_temperature_k": k, "lumens": lm, "range_m": rg}),
                );
            }
            Some(_) => {}
            None => (self.kelvin, self.lumens, self.range) = (k, lm, rg),
        }
        slider_field(ui, t::LIGHT_HEIGHT, &mut self.height, 0.2..=8.0);
        if self.kind == Kind::Spot {
            slider_field(ui, t::CONE_INNER, &mut self.inner, 1.0..=60.0);
            self.outer = self.outer.max(self.inner);
            slider_field(ui, t::CONE_OUTER, &mut self.outer, 5.0..=85.0);
            self.inner = self.inner.min(self.outer);
        }
        if let Some(l) = selected {
            ui.horizontal(|ui| {
                ui.label(&l.name);
                if ui.small_button(t::REMOVE).clicked() {
                    cx.run("light.remove", json!({"id": l.id}));
                    self.selected = None;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_core::Session;

    fn lamps() -> Vec<(String, LightSpec)> {
        vec![(
            "lamp".into(),
            LightSpec {
                kind: LightKind::Point,
                color_temperature_k: 2700.0,
                lumens: 300.0,
                range_m: 5.0,
                offset_m: [0.0, 1.0, 0.0],
            },
        )]
    }

    fn flat(g: &Generator) -> Vec<Placement> {
        visioncraft_core::evaluate(g, &|_, _| Some(0.0)).placements
    }

    fn run_all(s: &mut Session) -> usize {
        let r = reconcile_commands(s.project(), &lamps(), &flat);
        let n = r.commands.len();
        for (c, p) in r.commands {
            s.run(c, p).unwrap();
        }
        n
    }

    #[test]
    fn a_lamp_gets_a_light_that_follows_it_and_goes_with_it() {
        let mut s = Session::new(Project::default());
        let r = s
            .run(
                "planting.add_single",
                json!({"name": "Lamp", "asset": "lamp", "position": [1.0, 0.0, 2.0]}),
            )
            .unwrap();
        s.run(
            "planting.add_single",
            json!({"name": "Tree", "asset": "tree", "position": [5.0, 0.0, 5.0]}),
        )
        .unwrap();
        assert_eq!(run_all(&mut s), 1);
        let l = s.project().lights.last().unwrap().clone();
        assert_eq!(l.source.as_deref(), r["id"].as_str());
        assert_eq!(l.position, [1.0, 1.0, 2.0]);
        assert_eq!(run_all(&mut s), 0, "settled");
        s.run(
            "planting.update",
            json!({"id": r["id"], "position": [4.0, 0.0, 2.0]}),
        )
        .unwrap();
        assert_eq!(run_all(&mut s), 1);
        assert_eq!(s.project().lights.last().unwrap().position, [4.0, 1.0, 2.0]);
        s.run("planting.remove", json!({"id": r["id"]})).unwrap();
        assert_eq!(run_all(&mut s), 1);
        assert!(s.project().lights.is_empty());
    }

    #[test]
    fn lamps_on_a_line_get_lights_up_to_the_cap() {
        let mut s = Session::new(Project::default());
        s.run(
            "planting.add_spline",
            json!({"name": "Path lamps", "asset_palette": [{"asset": "lamp", "weight": 1.0}],
                   "points": [[0.0, 0.0], [200.0, 0.0]], "spacing": {"every": 1.0}}),
        )
        .unwrap();
        let r = reconcile_commands(s.project(), &lamps(), &flat);
        let adds = r.commands.iter().filter(|c| c.0 == "light.add").count();
        assert_eq!(adds, MAX_FIXTURE_LIGHTS);
        assert!(r.skipped > 100, "{}", r.skipped);
    }
}
