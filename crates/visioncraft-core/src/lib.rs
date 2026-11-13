//! Headless core of VisionCraft: designs read into triangle meshes, the project document,
//! commands with undo, the sun, and ray queries. No GPU, no windowing, no filesystem: bytes in,
//! bytes out, so the same code runs on `wasm32-unknown-unknown`.
//!
//! World convention: metres, right-handed, **Y up**, +X east, -Z north (the glTF convention).

pub mod catalog;
mod commands;
pub mod export;
mod import;
pub mod light;
pub mod material;
pub mod measure;
mod model;
pub mod planting;
pub mod presets;
pub mod project;
pub mod raycast;
pub mod session;
pub mod shape;
pub mod sun;
pub mod terrain;
pub mod viewpoint;

pub use catalog::{Asset, Catalog, CatalogError, attribution_markdown, validate_open_licences};
pub use export::export_presets;
pub use import::{import_3dcraft, import_obj};
pub use light::{Light, LightKind, LightSpec, kelvin_to_linear_rgb, lights_for_placement};
pub use material::{
    Assignment, Material, McMat, McMatError, SurfaceKind, Target, builtin_palette,
    dangling_assignments, read_mcmat, resolve_materials,
};
pub use model::{Body, Face, ImportError, ImportedModel};
pub use planting::{
    Evaluation, Generator, Placement, PlantLayer, Tuft, evaluate, evaluate_surface_fill,
    lawn_scatter,
};
pub use presets::{PlantingPreset, planting_presets};
pub use project::{
    CustomSpecies, GroundPlane, Layer, ModelKind, ModelRef, Project, ProjectError, RenderStyle,
    Site, SkyChoice, SunTime, Transform, ViewState,
};
pub use raycast::{Hit, ground_height, raycast};
pub use session::{Command, CommandError, Session};
pub use shape::{Shape, ShapeError, shape_mesh, shape_mesh_on};
pub use terrain::{
    BrushStroke, DirtyRect, Terrain, TerrainChunk, Tool, apply_stroke, ground_height_over,
    terrain_chunk_mesh, terrain_chunks,
};
pub use viewpoint::{Viewpoint, interpolate};
