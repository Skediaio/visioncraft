//! An example VisionCraft extension: draw a pipe on the ground, read its length over the terrain
//! as a label in the scene. The pipes are the project's, under `extensions["example.pipe-length"]`;
//! each `pipe.add` is one undo step, so undo takes the last pipe away again.
//!
//! In the window: open the "Pipe" mode, click points on the ground, double click (or Enter) to
//! finish; Backspace takes the last point back, Escape drops the pipe being drawn.

use serde_json::{Value, json};
use visioncraft::{Extension, ExtensionCx, Overlay, egui};
use visioncraft_core::measure::polyline_length_on_ground;
use visioncraft_core::spec::{param, spec};
use visioncraft_core::{CommandError, ParamType, Project, Session};

const ID: &str = "example.pipe-length";
const COLOR: [u8; 4] = [40, 170, 255, 255];
const DRAFT: [u8; 4] = [255, 196, 64, 255];
/// The drawn line floats this far above the ground so the lawn does not hide it.
const LIFT: f32 = 0.03;

fn main() {
    visioncraft::Shell::new().with(PipeLength::default()).run();
}

#[derive(Default)]
pub struct PipeLength {
    /// The pipe being drawn, `[x, z]` points.
    drawing: Vec<[f32; 2]>,
}

/// The pipes of `project`, each an outline of `[x, z]` points.
fn pipes(project: &Project) -> Vec<Vec<[f32; 2]>> {
    project
        .extensions
        .get(ID)
        .and_then(|v| v.get("pipes"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn set_pipes(project: &mut Project, pipes: &[Vec<[f32; 2]>]) {
    if pipes.is_empty() {
        project.extensions.remove(ID);
    } else {
        project
            .extensions
            .insert(ID.into(), json!({ "pipes": pipes }));
    }
}

/// The terrain's height, 0 where there is none.
fn ground(project: &Project) -> impl Fn(f32, f32) -> Option<f32> + '_ {
    move |x, z| project.terrain.as_ref().and_then(|t| t.height_at(x, z))
}

fn length(project: &Project, points: &[[f32; 2]]) -> f32 {
    polyline_length_on_ground(points, &ground(project))
}

fn metres(m: f32) -> String {
    format!("{m:.2} m")
}

/// Draws `points` as a line that follows the ground, labelled with its length.
fn draw(project: &Project, points: &[[f32; 2]], color: [u8; 4], out: &mut Overlay) {
    const STEP: f32 = 0.25;
    let g = ground(project);
    let at = |x: f32, z: f32| [x, g(x, z).unwrap_or(0.0) + LIFT, z];
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / STEP).ceil().max(1.0) as usize;
        let mut prev = at(a[0], a[1]);
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let p = at(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
            out.line(prev, p, color);
            prev = p;
        }
    }
    for p in points {
        out.point(at(p[0], p[1]), color);
    }
    if let (Some(first), Some(last)) = (points.first(), points.last())
        && points.len() >= 2
    {
        let mid = at((first[0] + last[0]) / 2.0, (first[1] + last[1]) / 2.0);
        out.label(
            [mid[0], mid[1] + 0.4, mid[2]],
            metres(length(project, points)),
        );
    }
}

fn failed(why: &str) -> CommandError {
    CommandError::Failed(why.into())
}

/// `pipe.add`, `pipe.remove` and `pipe.lengths` on `session`.
pub fn register(session: &mut Session) {
    session.register_with_spec(
        spec(
            "pipe.add",
            "Add pipe",
            "Adds a pipe along the ground through the points; its length follows the terrain.",
            vec![param("points", ParamType::Outline, "At least two [x, z] points.").required()],
            "{index: integer, length_m: number}",
            json!({"points": [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]]}),
        ),
        |p, v| {
            let points: Vec<[f32; 2]> =
                serde_json::from_value(v["points"].clone()).map_err(|e| failed(&e.to_string()))?;
            if points.len() < 2 {
                return Err(failed("a pipe needs at least two points"));
            }
            let mut all = pipes(p);
            all.push(points);
            set_pipes(p, &all);
            let length_m = length(p, &all[all.len() - 1]);
            Ok(json!({"index": all.len() - 1, "length_m": length_m}))
        },
    );
    session.register_with_spec(
        spec(
            "pipe.remove",
            "Remove pipe",
            "Removes a pipe by its index (from pipe.lengths).",
            vec![param("index", ParamType::Integer, "Which pipe, from 0.").required()],
            "null",
            json!({"index": 0}),
        ),
        |p, v| {
            let mut all = pipes(p);
            let i = v["index"].as_u64().unwrap_or(u64::MAX) as usize;
            if i >= all.len() {
                return Err(failed("there is no pipe with that index"));
            }
            all.remove(i);
            set_pipes(p, &all);
            Ok(Value::Null)
        },
    );
    session.register_with_spec(
        spec(
            "pipe.lengths",
            "Pipe lengths",
            "The length of every pipe over the terrain, and their total, in metres.",
            vec![],
            "{pipes: [number], total_m: number}",
            json!({}),
        )
        .read_only(),
        |p, _| {
            let lengths: Vec<f32> = pipes(p).iter().map(|pts| length(p, pts)).collect();
            Ok(json!({"total_m": lengths.iter().sum::<f32>(), "pipes": lengths}))
        },
    );
}

impl PipeLength {
    fn finish(&mut self, cx: &mut ExtensionCx) {
        if self.drawing.len() >= 2 {
            cx.run("pipe.add", json!({ "points": self.drawing }));
        }
        self.drawing.clear();
    }
}

impl Extension for PipeLength {
    fn id(&self) -> &'static str {
        ID
    }

    fn register(&self, session: &mut Session) {
        register(session);
    }

    fn mode(&self) -> Option<&'static str> {
        Some("Pipe")
    }

    fn update(&mut self, cx: &mut ExtensionCx) {
        cx.capture_double_click();
        let input = cx.input().clone();
        let under = cx.ground_under_pointer().map(|g| [g[0], g[2]]);
        if input.clicked
            && let Some(g) = under
            && self
                .drawing
                .last()
                .is_none_or(|l| (l[0] - g[0]).hypot(l[1] - g[1]) > 0.01)
        {
            self.drawing.push(g);
        }
        if input.double_clicked || input.enter {
            self.finish(cx);
        }
        if input.backspace {
            self.drawing.pop();
        }
        if input.escape {
            self.drawing.clear();
        }
        if !self.drawing.is_empty() {
            let mut preview = self.drawing.clone();
            preview.extend(under);
            let (project, overlay) = cx.project_and_overlay();
            draw(project, &preview, DRAFT, overlay);
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut ExtensionCx) {
        ui.label("Click points on the ground; double click or Enter finishes the pipe.");
        let all = pipes(cx.project());
        let lengths: Vec<f32> = all.iter().map(|p| length(cx.project(), p)).collect();
        let mut remove = None;
        for (i, m) in lengths.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("Pipe {}: {}", i + 1, metres(*m)));
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if !lengths.is_empty() {
            ui.strong(format!("Total {}", metres(lengths.iter().sum())));
        }
        if self.drawing.len() >= 2 && ui.button("Finish pipe").clicked() {
            self.finish(cx);
        }
        if let Some(i) = remove {
            cx.run("pipe.remove", json!({ "index": i }));
        }
    }

    fn overlay(&self, project: &Project, out: &mut Overlay) {
        for p in pipes(project) {
            draw(project, &p, COLOR, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_is_one_undo_step() {
        let mut s = Session::new(Project::default());
        register(&mut s);
        s.run("pipe.add", json!({"points": [[0, 0], [3, 0], [3, 4]]}))
            .unwrap();
        let flat = s.run("pipe.lengths", json!({})).unwrap()["total_m"]
            .as_f64()
            .unwrap();
        assert!((flat - 7.0).abs() < 1e-3, "{flat}");
        s.undo();
        assert!(pipes(s.project()).is_empty());
        assert!(!s.project().extensions.contains_key(ID));
        assert!(s.run("pipe.add", json!({"points": [[0, 0]]})).is_err());
        assert!(s.run("pipe.remove", json!({"index": 0})).is_err());
    }

    #[test]
    fn the_length_follows_the_terrain() {
        // A slope rising 3 m over every 4 m east: 4 m east along the ground is 5 m of pipe.
        let mut t =
            visioncraft_core::Terrain::flat([-1.0, -1.0], 10.0, 10.0, 1.0, "grass").unwrap();
        for iz in 0..=t.depth as usize {
            for ix in 0..=t.width as usize {
                t.heights[iz * (t.width as usize + 1) + ix] = (ix as f32 - 1.0) * 0.75;
            }
        }
        let project = Project {
            terrain: Some(std::sync::Arc::new(t)),
            ..Default::default()
        };
        let m = length(&project, &[[0.0, 2.0], [4.0, 2.0]]);
        assert!((m - 5.0).abs() < 1e-3, "{m}");
    }

    #[test]
    fn the_overlay_draws_the_pipe_and_labels_its_length() {
        let mut s = Session::new(Project::default());
        register(&mut s);
        s.run("pipe.add", json!({"points": [[0, 0], [2, 0]]}))
            .unwrap();
        let mut out = Overlay::default();
        PipeLength::default().overlay(s.project(), &mut out);
        assert_eq!(out.lines.len(), 8);
        assert_eq!(out.points.len(), 2);
        assert_eq!(out.labels[0].1, "2.00 m");
    }
}
