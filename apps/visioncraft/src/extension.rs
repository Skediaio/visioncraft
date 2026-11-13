//! Extensions: third-party crates that add to the app, compiled in (no runtime loading).
//!
//! ```ignore
//! fn main() {
//!     visioncraft::Shell::new().with(MyExtension::default()).run();
//! }
//! ```
//!
//! An extension
//! - registers `Session` commands ([`Extension::register`]); every successful run is one undo
//!   step, and agents reach them over MCP like the built-in ones;
//! - keeps its data under its own namespace ([`Extension::id`]) in the `extensions` maps of the
//!   project (and of the models, materials and species that have one); other builds keep it as is;
//! - may have a mode pill with a panel ([`Extension::mode`], [`Extension::panel`]) and a pointer
//!   tool while that mode is open ([`Extension::update`]);
//! - draws lines, points and labels over the scene ([`Extension::overlay`]), in the same overlay
//!   pass as the Draw tool;
//! - makes its own kinds of objects from the existing primitives: a command that writes core
//!   shapes (ids under its namespace) and its own data next to them.

use crate::planting_tools::ground_at;
use crate::scene::line;
use crate::ui::{Cx, Input};
use glam::Vec3;
use serde_json::Value;
use visioncraft_core::{Project, Session};

pub trait Extension: 'static {
    /// Its namespace, as in `extensions` maps and command names: `"example.pipe-length"`.
    fn id(&self) -> &'static str;

    /// Adds its commands to the window's session, once at start.
    fn register(&self, _session: &mut Session) {}

    /// The label of its mode pill; `None` for an extension without a mode.
    fn mode(&self) -> Option<&'static str> {
        None
    }

    /// Every frame while its mode is open: the pointer tool. Lines added to `cx.overlay` show
    /// for this frame only (a drawing in progress).
    fn update(&mut self, _cx: &mut ExtensionCx) {}

    /// The panel of its mode.
    fn panel(&mut self, _ui: &mut egui::Ui, _cx: &mut ExtensionCx) {}

    /// Every frame, mode open or not: what it draws over the scene for `project`.
    fn overlay(&self, _project: &Project, _out: &mut Overlay) {}
}

/// Lines, points and labels in world space (metres, y up), for one frame.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    /// Hidden behind what is in front of them.
    pub lines: Vec<([f32; 3], [f32; 3], [u8; 4])>,
    /// Always visible.
    pub points: Vec<([f32; 3], [u8; 4])>,
    pub labels: Vec<([f32; 3], String)>,
}

impl Overlay {
    pub fn line(&mut self, a: [f32; 3], b: [f32; 3], color: [u8; 4]) {
        self.lines.push((a, b, color));
    }

    pub fn point(&mut self, at: [f32; 3], color: [u8; 4]) {
        self.points.push((at, color));
    }

    pub fn label(&mut self, at: [f32; 3], text: impl Into<String>) {
        self.labels.push((at, text.into()));
    }

    /// Into the overlay pass and the label layer.
    pub(crate) fn show(self, cx: &mut Cx) {
        const WIDTH: f32 = 3.0;
        const TICK: f32 = 0.06;
        for (a, b, c) in self.lines {
            cx.s.extra
                .0
                .extend(line(Vec3::from(a), Vec3::from(b), c, WIDTH));
        }
        for (p, c) in self.points {
            let p = Vec3::from(p);
            for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                cx.s.extra
                    .1
                    .extend(line(p - axis * TICK, p + axis * TICK, c, WIDTH));
            }
        }
        for (p, text) in self.labels {
            if let Some(at) = cx.project(Vec3::from(p)) {
                cx.label(at, &text);
            }
        }
    }
}

/// What an extension sees of the window for one frame.
pub struct ExtensionCx<'a, 'b> {
    pub(crate) cx: &'a mut Cx<'b>,
    pub overlay: &'a mut Overlay,
}

impl ExtensionCx<'_, '_> {
    pub fn project(&self) -> &Project {
        self.cx.s.project()
    }

    /// The project to read while drawing into this frame's overlay.
    pub fn project_and_overlay(&mut self) -> (&Project, &mut Overlay) {
        (self.cx.s.project(), self.overlay)
    }

    /// Runs a command on the window's session (one undo step); a failure becomes a toast.
    pub fn run(&mut self, name: &str, params: Value) -> Option<Value> {
        self.cx.run(name, params)
    }

    /// The viewport's pointer and keys this frame (keys are false while a text field types).
    pub fn input(&self) -> &Input {
        self.cx.input
    }

    /// The ground (terrain, floors, the zero plane) under the pointer.
    pub fn ground_under_pointer(&self) -> Option<[f32; 3]> {
        let pos = self.cx.input.hover?;
        ground_at(self.cx, pos).map(|p| p.to_array())
    }

    /// The primary drag is the tool's this frame: the camera does not orbit.
    pub fn capture_pointer(&mut self) {
        self.cx.s.pointer_captured = true;
    }

    /// A double click is the tool's this frame: the camera does not re-aim.
    pub fn capture_double_click(&mut self) {
        self.cx.s.double_click_captured = true;
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.cx.toasts.info(text.into());
    }
}
