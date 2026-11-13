//! Lamps: the lights of catalogue fixtures and the Lights tool for bare point and spot lights.
//!
//! A fixture placed with `planting.add_single` gets its `Light` (through the core's
//! `lights_for_placement`, `source` = the placement's id) from [`reconcile`], which also moves the
//! light with a moved, turned or scaled fixture and removes it with a removed one. The core has no
//! way to group commands into one undo step, so a lamp and its light are two commands; [`Pairing`]
//! makes Undo and Redo step over both.

use crate::strings as t;
use crate::ui::{Cx, Edit, slider_field};
use eframe::egui;
use glam::Vec3;
use serde_json::{Value, json};
use std::collections::HashMap;
use visioncraft_core::project::LAYER_LIGHTING;
use visioncraft_core::{
    Generator, Light, LightKind, Placement, Project, Session, lights_for_placement,
};

/// Which undo steps are follow-ups of the step before them.
#[derive(Default)]
pub struct Pairing {
    /// Revision after the last follow-up -> number of follow-ups it ended.
    follow: HashMap<u64, u32>,
    /// After a paired undo: the revision it reached and how many steps to redo.
    redo: Option<(u64, u32)>,
    reconciled: u64,
}

impl Pairing {
    pub fn undo(&mut self, s: &mut Session) {
        let n = self.follow.get(&s.revision()).copied().unwrap_or(0);
        for _ in 0..=n {
            s.undo();
        }
        if n > 0 {
            self.redo = Some((s.revision(), n));
        }
    }

    pub fn redo(&mut self, s: &mut Session) {
        let n = match self.redo.take() {
            Some((rev, n)) if rev == s.revision() => n,
            _ => 0,
        };
        for _ in 0..=n {
            s.redo();
        }
        if n > 0 {
            self.follow.insert(s.revision(), n);
        }
    }
}

/// The commands that bring the lights in line with the fixtures (pure; tested).
pub fn reconcile_commands(
    project: &Project,
    spec_of: &dyn Fn(&str) -> Option<visioncraft_core::LightSpec>,
) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    for g in &project.generators {
        let Generator::Single(s) = g else { continue };
        let Some(spec) = spec_of(&s.asset) else {
            continue;
        };
        let placement = Placement {
            asset: s.asset.clone(),
            position: s.position,
            rotation_y_deg: s.rotation_y_deg,
            scale: s.scale,
            variant: 0,
        };
        let want = lights_for_placement(
            &spec,
            &placement,
            (String::new(), s.name.clone(), LAYER_LIGHTING.into()),
            &s.id,
        );
        match project
            .lights
            .iter()
            .find(|l| l.source.as_deref() == Some(s.id.as_str()))
        {
            None => out.push((
                "light.add",
                json!({"name": format!("{} light", s.name), "position": want.position, "kind": want.kind,
                       "color_temperature_k": want.color_temperature_k, "lumens": want.lumens,
                       "range_m": want.range_m, "source": s.id}),
            )),
            Some(l) if l.position != want.position || l.kind != want.kind => out.push((
                "light.update",
                json!({"id": l.id, "position": want.position, "kind": want.kind}),
            )),
            Some(_) => {}
        }
    }
    for l in &project.lights {
        if let Some(src) = &l.source
            && !project.generators.iter().any(|g| g.id() == src)
        {
            out.push(("light.remove", json!({"id": l.id})));
        }
    }
    out
}

/// Runs once per project change: creates, moves and removes the lights of placed fixtures.
/// Returns what went wrong, in words.
pub fn reconcile_studio(s: &mut crate::studio::Studio) -> Vec<String> {
    let rev = s.session.revision();
    let mut errors = Vec::new();
    if s.pairing.reconciled == rev {
        return errors;
    }
    let Some(catalog) = s.scene.library.catalog.clone() else {
        return errors; // fixtures are unknown until the catalogue is there; try again then
    };
    let spec = |id: &str| catalog.asset(id).and_then(|a| a.light);
    let commands = reconcile_commands(s.project(), &spec);
    let n = commands.len() as u32;
    for (name, params) in commands {
        if let Err(e) = s.session.run(name, params) {
            errors.push(e.to_string());
        }
    }
    if n > 0 {
        let after = s.session.revision();
        s.pairing.follow.insert(after, n);
    }
    s.pairing.reconciled = s.session.revision();
    errors
}

pub fn reconcile(cx: &mut Cx) {
    for e in reconcile_studio(cx.s) {
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
        let near_handle = handle
            .and_then(|h| cx.project(h))
            .zip(cx.input.hover)
            .is_some_and(|(h, p)| h.distance(p) < 16.0);
        if near_handle || self.aiming.is_some() {
            cx.s.drag_capture = true;
        }
        if let (Some((id, pos, _)), Some(g)) = (&spot, ground) {
            if cx.input.drag_started && near_handle {
                self.aiming = Some(g);
            } else if self.aiming.is_some() && cx.input.dragging {
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
        if self.aiming.is_none() && !near_handle {
            if let Some(g) = ground {
                let at = g + Vec3::Y * self.height;
                top.extend(line(g, at, [255, 255, 255, 120], 1.5));
                top.extend(line(at, at, [255, 214, 120, 255], 9.0));
                if cx.input.clicked {
                    self.click(cx, at);
                }
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
    use visioncraft_core::{LightSpec, Session};

    fn lamp_spec() -> LightSpec {
        LightSpec {
            kind: LightKind::Point,
            color_temperature_k: 2700.0,
            lumens: 300.0,
            range_m: 5.0,
            offset_m: [0.0, 1.0, 0.0],
        }
    }

    fn spec(id: &str) -> Option<LightSpec> {
        (id == "lamp").then(lamp_spec)
    }

    fn run_all(s: &mut Session) -> u32 {
        let cmds = reconcile_commands(s.project(), &spec);
        let n = cmds.len() as u32;
        for (c, p) in cmds {
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
        // moving the fixture moves the light
        s.run(
            "planting.update",
            json!({"id": r["id"], "position": [4.0, 0.0, 2.0]}),
        )
        .unwrap();
        assert_eq!(run_all(&mut s), 1);
        assert_eq!(s.project().lights.last().unwrap().position, [4.0, 1.0, 2.0]);
        // removing the fixture removes the light
        s.run("planting.remove", json!({"id": r["id"]})).unwrap();
        assert_eq!(run_all(&mut s), 1);
        assert!(s.project().lights.is_empty());
    }

    #[test]
    fn undo_and_redo_step_over_a_lamp_and_its_light_together() {
        let mut s = Session::new(Project::default());
        let mut p = Pairing::default();
        s.run(
            "planting.add_single",
            json!({"name": "Lamp", "asset": "lamp", "position": [0.0, 0.0, 0.0]}),
        )
        .unwrap();
        let n = run_all(&mut s);
        p.follow.insert(s.revision(), n);
        assert_eq!(
            (s.project().generators.len(), s.project().lights.len()),
            (1, 1)
        );
        p.undo(&mut s);
        assert_eq!(
            (s.project().generators.len(), s.project().lights.len()),
            (0, 0)
        );
        p.redo(&mut s);
        assert_eq!(
            (s.project().generators.len(), s.project().lights.len()),
            (1, 1)
        );
        p.undo(&mut s);
        assert!(s.project().generators.is_empty() && s.project().lights.is_empty());
        // an ordinary step is still a single step
        s.run(
            "light.add",
            json!({"name": "x", "position": [0.0, 1.0, 0.0]}),
        )
        .unwrap();
        p.undo(&mut s);
        assert!(s.project().lights.is_empty());
    }
}
