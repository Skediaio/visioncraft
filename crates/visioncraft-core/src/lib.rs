//! Headless core of VisionCraft: designs read into triangle meshes, the project document,
//! commands with undo, the sun, and ray queries. No GPU, no windowing, no filesystem: bytes in,
//! bytes out, so the same code runs on `wasm32-unknown-unknown`.
//!
//! World convention: metres, right-handed, **Y up**, +X east, -Z north (the glTF convention).

pub mod catalog;
mod commands;
pub mod export;
pub mod growth;
pub mod house;
mod import;
pub mod light;
pub mod material;
pub mod measure;
mod model;
pub mod object;
pub mod plant_ground;
pub mod planting;
pub mod presets;
pub mod project;
pub mod raycast;
pub mod season;
pub mod session;
pub mod shape;
pub mod spec;
pub mod sun;
pub mod terrain;
pub mod tiers;
pub mod viewpoint;
pub mod water;

pub use catalog::{Asset, Catalog, CatalogError, attribution_markdown, validate_open_licences};
pub use commands::examples::{EXAMPLES, example};
pub use export::export_presets;
pub use import::{import_3dcraft, import_obj};
pub use light::{
    Light, LightKind, LightSpec, kelvin_to_linear_rgb, lights_for_placement, lights_for_placements,
};
pub use material::{
    Assignment, Material, McMat, McMatError, SurfaceKind, Target, builtin_palette,
    dangling_assignments, read_mcmat, resolve_materials,
};
pub use model::{Body, Face, ImportError, ImportedModel, ModelCodecError};
pub use object::UserObject;
pub use plant_ground::{
    evaluate_in_scene, over_ids, placed_shape, placed_shape_mesh, plant_size, water_areas,
};
pub use planting::{
    Evaluation, Generator, LawnWindow, Placement, PlantLayer, Tuft, blocked_by_buildings, evaluate,
    evaluate_avoiding, evaluate_surface_fill, lawn_area, lawn_scatter, lawn_scatter_within,
};
pub use presets::{PlantingPreset, planting_presets};
pub use project::{
    CloudKind, Clouds, CustomSpecies, GroundPlane, Layer, Look, ModelKind, ModelRef, Project,
    ProjectError, RenderStyle, Site, SkyChoice, SunTime, Transform, ViewState, YEARS_MAX,
};
pub use raycast::{Hit, ground_height, raycast};
pub use season::Season;
pub use session::{Command, CommandError, Session};
pub use shape::{
    AREA_LIFT_M, STACK_STEP_M, Shape, ShapeError, area_lifts, areas_over, footing, inside_outline,
    shape_mesh, shape_mesh_on, shape_mesh_on_terrain,
};
pub use spec::{CommandSpec, ParamSpec, ParamType, example_project};
pub use terrain::{
    BrushStroke, DirtyRect, Terrain, TerrainChunk, Tool, apply_stroke, ground_height_over,
    terrain_chunk_mesh, terrain_chunk_mesh_on, terrain_chunks,
};
pub use tiers::Tiers;
pub use viewpoint::{Viewpoint, interpolate};
pub use water::{Sea, WaterKind, keep_dry};
