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
//!   step, and agents driving the window (`visioncraft --control`, `visioncraft-cli mcp --bridge`)
//!   reach them over MCP like the built-in ones, and open its mode with `ui.set {"mode": id}`.
//!   `visioncraft-cli` on its own and `visioncraft --agents-headless` build their own session
//!   from the core commands: they cannot know a third-party build's commands, by design (no
//!   runtime loading). Automate an extension through the window of the build that has it;
//! - keeps its data under its own namespace ([`Extension::id`]) in the `extensions` maps of the
//!   project (and of the models, materials and species that have one); other builds keep it as is;
//! - may have a mode pill with a panel ([`Extension::mode`], [`Extension::panel`]) and a pointer
//!   tool while that mode is open ([`Extension::update`], [`Extension::leave`]);
//! - may choose which built-in UI is shown and the mode the window starts in
//!   ([`Extension::host_ui`], or the shell's `.host_ui(..)`, which wins), and show a mesh it
//!   generated with [`ExtensionCx::import_object`];
//! - may read the undo state and whether there are unsaved changes ([`ExtensionCx::edit_state`],
//!   the same as the `edit.state` command) and open a project without the window's *Replace the
//!   current project?* question ([`ExtensionCx::open_project`]) when it asked already; the
//!   wording of that question is the host's ([`HostUi::replace_prompt`](crate::HostUi));
//! - draws lines, points and labels over the scene ([`Extension::overlay`]), in the same overlay
//!   pass as the Draw tool;
//! - makes its own kinds of objects from the existing primitives: a command that writes core
//!   shapes (ids under its namespace) and its own data next to them.

use crate::planting_tools::ground_at;
use crate::scene::line;
use crate::ui::{Cx, Input};
use glam::Vec3;
use serde_json::Value;
use visioncraft_automation::commands::EditState;
use visioncraft_core::{Project, Session};

/// What an extension asks the window to do after its frame (the window does it with the
/// person's own open and new, which an extension has no hold of).
pub(crate) enum Request {
    /// Open a project file's bytes; `ask`: ask first when there is unsaved work.
    Open {
        name: String,
        bytes: Vec<u8>,
        ask: bool,
    },
    /// Replace the project with a bundled example, as `project.load_example`.
    Example { name: String, title: String },
}

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

    /// Its mode closes (another mode opens, or none): drop what the tool was in the middle of.
    fn leave(&mut self, _cx: &mut ExtensionCx) {}

    /// Which built-in UI the build shows and which mode it starts in; the first extension that
    /// returns `Some` is used unless the shell has an explicit `.host_ui(..)`.
    fn host_ui(&self) -> Option<crate::host_ui::HostUi> {
        None
    }

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
    /// The project as last opened, created or saved (`edit.state`'s `dirty` compares to it).
    pub(crate) saved: Option<&'a Project>,
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

    /// Shows a mesh the extension generated (a binary glTF, `.glb`) without any server: the
    /// bytes are checked and held by the window, and the object is added to the project with
    /// `object.add` (one undo step). Returns its asset id, `user:<id>`, which is also what
    /// `placement.add` and the other commands take as the object's `asset`; it fails when the
    /// bytes are not a usable glb.
    pub fn import_object(&mut self, name: &str, glb_bytes: Vec<u8>) -> Result<String, String> {
        self.cx.s.import_object(name, glb_bytes)
    }

    /// Whether there is something to undo or redo, whether the project has unsaved changes
    /// (it differs from the project as last opened, created or saved) and the revision: what
    /// the `edit.state` command reports.
    pub fn edit_state(&self) -> EditState {
        EditState::of(&self.cx.s.session, self.saved)
    }

    /// Opens a `.visioncraft` project (JSON or the bundled `.zip`; `name` says which) now,
    /// without the *Replace the current project?* question: the caller has asked already, or
    /// there is nothing to lose. Unsaved changes are lost. A failure becomes a toast.
    pub fn open_project(&mut self, name: &str, bytes: Vec<u8>) {
        self.cx.s.requests.push(Request::Open {
            name: name.into(),
            bytes,
            ask: false,
        });
    }

    /// As the person's Open: asks first when there are unsaved changes, with the host's wording
    /// ([`HostUi::replace_prompt`](crate::HostUi)).
    pub fn open_project_asking(&mut self, name: &str, bytes: Vec<u8>) {
        self.cx.s.requests.push(Request::Open {
            name: name.into(),
            bytes,
            ask: true,
        });
    }

    /// Replaces the project with a bundled example (`project.load_example`: `garden`,
    /// `estate`, `beach-house` or `coast`) named `title`, without asking.
    pub fn open_example(&mut self, name: &str, title: &str) {
        self.cx.s.requests.push(Request::Example {
            name: name.into(),
            title: title.into(),
        });
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.cx.toasts.info(text.into());
    }
}

#[cfg(test)]
mod tests {
    use super::{ExtensionCx, Overlay, Request};
    use crate::studio::Studio;
    use crate::ui::{Cx, Input, Toasts};
    use serde_json::json;
    use visioncraft_core::Project;

    /// A window's pieces over a headless device, or nothing where there is no adapter.
    fn with_cx(saved: Option<Project>, f: impl FnOnce(&mut ExtensionCx)) {
        let Ok((device, queue)) = pollster::block_on(visioncraft_render::headless_device()) else {
            return;
        };
        let renderer =
            visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut studio = Studio::new(renderer, Project::default());
        let (mut toasts, input, ctx) = (
            Toasts::default(),
            Input::default(),
            egui::Context::default(),
        );
        let mut cx = Cx {
            s: &mut studio,
            toasts: &mut toasts,
            input: &input,
            ctx: &ctx,
        };
        let mut overlay = Overlay::default();
        f(&mut ExtensionCx {
            cx: &mut cx,
            overlay: &mut overlay,
            saved: saved.as_ref(),
        });
    }

    #[test]
    fn edit_state_before_and_after_an_edit_an_undo_and_a_save() {
        let saved = Project::default();
        with_cx(Some(saved.clone()), |x| {
            let s = x.edit_state();
            assert!(!s.can_undo && !s.can_redo && !s.dirty);
            let rev = s.revision;
            x.run("time.set", json!({"hour": 20.5})).unwrap();
            let s = x.edit_state();
            assert!(s.can_undo && s.dirty && s.revision > rev);
            assert!(x.cx.s.session.undo());
            let s = x.edit_state();
            assert!(!s.can_undo && s.can_redo && !s.dirty);
        });
        // Without a saved copy (nothing recorded yet) the project counts as unsaved.
        with_cx(None, |x| assert!(x.edit_state().dirty));
    }

    #[test]
    fn opening_without_asking_is_a_request_that_does_not_ask() {
        with_cx(None, |x| {
            x.open_project("a.visioncraft", b"{}".to_vec());
            x.open_project_asking("b.visioncraft", b"{}".to_vec());
            x.open_example("garden", "Garden");
            let asks: Vec<_> =
                x.cx.s
                    .requests
                    .iter()
                    .map(|r| match r {
                        Request::Open { name, ask, .. } => (name.clone(), Some(*ask)),
                        Request::Example { name, .. } => (name.clone(), None),
                    })
                    .collect();
            assert_eq!(
                asks,
                [
                    ("a.visioncraft".into(), Some(false)),
                    ("b.visioncraft".into(), Some(true)),
                    ("garden".into(), None)
                ]
            );
        });
    }
}
