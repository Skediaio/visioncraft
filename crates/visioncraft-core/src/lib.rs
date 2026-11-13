//! Headless core of VisionCraft: designs read into triangle meshes, the project document,
//! commands with undo, the sun, and ray queries. No GPU, no windowing, no filesystem: bytes in,
//! bytes out, so the same code runs on `wasm32-unknown-unknown`.
//!
//! World convention: metres, right-handed, **Y up**, +X east, -Z north (the glTF convention).

mod import;
mod model;
pub mod project;
pub mod raycast;
pub mod session;
pub mod sun;

pub use import::{import_3dcraft, import_obj};
pub use model::{Body, Face, ImportError, ImportedModel};
pub use project::{ModelKind, ModelRef, Project, ProjectError, Site, SunTime, Transform};
pub use raycast::{Hit, ground_height, raycast};
pub use session::{Command, CommandError, Session};
