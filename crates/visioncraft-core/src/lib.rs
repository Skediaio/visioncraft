//! Headless core of VisionCraft: designs read into triangle meshes, the project document,
//! commands with undo, the sun, and ray queries. No GPU, no windowing, no filesystem: bytes in,
//! bytes out, so the same code runs on `wasm32-unknown-unknown`.
//!
//! World convention: metres, right-handed, **Y up**, +X east, -Z north (the glTF convention).

pub mod bed;
pub mod border;
pub mod canopy;
pub mod catalog;
mod commands;
pub mod curve;
pub mod export;
pub mod fence;
pub mod film;
pub mod furniture;
pub mod growth;
pub mod house;
mod import;
pub mod light;
pub mod material;
pub mod measure;
mod model;
pub mod object;
pub mod plant_ground;
pub mod plant_light;
pub mod planting;
pub mod presets;
pub mod project;
pub mod quantities;
pub mod raycast;
pub mod season;
pub mod session;
pub mod setin;
pub mod shape;
pub mod sky_volume;
pub mod spec;
pub mod stairs;
pub mod sun;
pub mod sun_hours;
pub mod sun_map;
pub mod terrace;
pub mod terrain;
pub mod tiers;
pub mod underlay;
pub mod viewpoint;
pub mod wall_photo;
pub mod water;
pub mod wind;

pub use catalog::{Asset, Catalog, CatalogError, attribution_markdown, validate_open_licences};
pub use commands::examples::{EXAMPLES, example, example_file, is_example_source};
pub use export::export_presets;
pub use film::{Film, Shot};
pub use import::{import_3dcraft, import_obj};
pub use light::{
    Light, LightKind, LightSpec, kelvin_to_linear_rgb, lights_for_placement, lights_for_placements,
};
pub use material::{
    Assignment, Material, McMat, McMatError, SurfaceKind, Target, Weathering, builtin_palette,
    dangling_assignments, read_mcmat, resolve_materials,
};
pub use model::{Body, Face, ImportError, ImportedModel, ModelCodecError};
pub use object::UserObject;
pub use plant_ground::{
    evaluate_in_scene, over_ids, placed_shape, placed_shape_mesh, plant_size, water_areas,
};
pub use planting::{
    Evaluation, Generator, LawnWindow, Placement, PlantLayer, Tuft, blocked_by_buildings, evaluate,
    evaluate_avoiding, evaluate_surface_fill, lawn_area, lawn_scatter, lawn_scatter_blended,
    lawn_scatter_within,
};
pub use presets::{PlantingPreset, planting_presets};
pub use project::{
    CloudKind, Clouds, CustomSpecies, Grade, GroundPlane, Layer, Look, ModelKind, ModelRef,
    Project, ProjectError, RenderStyle, Site, SkyChoice, SunTime, Surroundings, Transform,
    ViewState, WHITE_BALANCE_KELVIN, WhiteBalance, YEARS_MAX,
};
pub use raycast::{Hit, ground_height, raycast, raycast_any};
pub use season::Season;
pub use session::{Command, CommandError, Session};
pub use shape::{
    AREA_LIFT_M, GROUND_FACE_M, STACK_STEP_M, Shape, ShapeError, area_lifts, areas_over,
    blends_over, footing, ground_faces, ground_model_lift, inside_outline, offset_outline,
    shape_mesh, shape_mesh_on, shape_mesh_on_terrain, shape_mesh_on_terrain_with,
};
pub use sky_volume::{Crown, Lamp, SkyVolume, sky_volume, sky_volume_with_canopy};
pub use spec::{CommandSpec, ParamSpec, ParamType, example_project};
pub use sun_map::SunMap;
pub use terrain::{
    BrushStroke, DirtyRect, Terrain, TerrainChunk, Tool, apply_stroke, ground_height_over,
    terrain_chunk_mesh, terrain_chunk_mesh_holes, terrain_chunk_mesh_on, terrain_chunks,
};
pub use tiers::Tiers;
pub use underlay::{Placement as UnderlayPlacement, Underlay};
pub use viewpoint::{Viewpoint, interpolate};
pub use wall_photo::WallPhoto;
pub use water::{Sea, WaterKind, keep_dry};
