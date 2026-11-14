//! Where plants stand and what they keep out of, computed the same in the window and for agents
//! (`document.inspect`): areas lifted by their stacking and draped on the terrain with the water
//! on them cut out, blocks standing on the terrain, and the building rule applied with each
//! asset's size.

use std::collections::HashMap;

use crate::catalog::Asset;
use crate::material::{SurfaceKind, resolve_materials};
use crate::model::ImportedModel;
use crate::planting::{Evaluation, Generator, Placement, blocked_by_buildings, evaluate_avoiding};
use crate::project::Project;
use crate::shape::{Shape, ShapeError, areas_over, footing, shape_mesh, shape_mesh_on_terrain};

type P2 = [f32; 2];

/// Ids of the areas whose surface is water.
pub fn water_areas(project: &Project) -> Vec<String> {
    project
        .shapes
        .iter()
        .filter(|s| {
            matches!(s, Shape::Area(_))
                && shape_mesh(s).is_ok_and(|m| {
                    resolve_materials(project, s.id(), &m).into_iter().any(|i| {
                        i.and_then(|i| project.materials.get(i))
                            .is_some_and(|m| matches!(m.kind, SurfaceKind::AnimatedWater { .. }))
                    })
                })
        })
        .map(|s| s.id().to_string())
        .collect()
}

/// A shape as the scene places it: an area lifted by `lifts` (`area_lifts`), a block's height
/// above the origin (it stands on the terrain's lowest point under it), and for a draped area
/// the outlines of the water lying on it, cut out of its mesh.
pub fn placed_shape(
    project: &Project,
    shape: &Shape,
    lifts: &HashMap<String, f32>,
    water: &[String],
) -> (Shape, f32, Vec<Vec<P2>>) {
    let terrain = project.terrain.as_deref();
    let mut placed = shape.clone();
    let mut lift = 0.0;
    let mut draped = false;
    match &mut placed {
        Shape::Area(a) => {
            a.elevation += lifts.get(&a.id).copied().unwrap_or(0.0);
            draped = a.drape && terrain.is_some();
        }
        Shape::Extrusion(e) => {
            if let Some(t) = terrain {
                lift = footing(&e.outline, &|x, z| t.height_at(x, z));
            }
        }
    }
    let holes = if draped {
        let id = shape.id();
        areas_over(&project.shapes, id)
            .into_iter()
            .zip(over_ids(&project.shapes, id))
            .filter(|(_, o)| water.contains(o))
            .map(|(h, _)| h.to_vec())
            .collect()
    } else {
        Vec::new()
    };
    (placed, lift, holes)
}

/// The mesh of a shape placed by `placed_shape` (cut along the terrain's own triangles when
/// there is one), and the block's height above the origin.
pub fn placed_shape_mesh(
    project: &Project,
    shape: &Shape,
    lifts: &HashMap<String, f32>,
    water: &[String],
) -> Result<(ImportedModel, f32), ShapeError> {
    let (placed, lift, holes) = placed_shape(project, shape, lifts, water);
    let holes: Vec<&[P2]> = holes.iter().map(Vec::as_slice).collect();
    let mesh = match project.terrain.as_deref() {
        Some(t) => shape_mesh_on_terrain(&placed, t, &holes)?,
        None => shape_mesh(&placed)?,
    };
    Ok((mesh, lift))
}

/// Ids of the areas `areas_over` returns for `id`, in the same order.
pub fn over_ids(shapes: &[Shape], id: &str) -> Vec<String> {
    let Some(i) = shapes.iter().position(|s| s.id() == id) else {
        return Vec::new();
    };
    let Shape::Area(a) = &shapes[i] else {
        return Vec::new();
    };
    shapes[i + 1..]
        .iter()
        .filter_map(|s| match s {
            Shape::Area(b)
                if areas_over(&[Shape::Area(a.clone()), s.clone()], &a.id).len() == 1 =>
            {
                Some(b.id.clone())
            }
            _ => None,
        })
        .collect()
}

/// Height and crown radius of a plant or object at scale 1: a built-in species by its mature
/// size (`species`, from the flora crate the core does not depend on), a custom species by its
/// base, a catalogue asset by its size.
pub fn plant_size(
    project: &Project,
    assets: &[Asset],
    id: &str,
    species: &dyn Fn(&str) -> Option<(f32, f32)>,
) -> (f32, f32) {
    if let Some(s) = id.strip_prefix("flora:").and_then(species) {
        return s;
    }
    if let Some(c) = project.species.iter().find(|c| c.id == id) {
        return c.base.as_deref().and_then(species).unwrap_or((2.0, 0.6));
    }
    assets.iter().find(|a| a.id == id).map_or((1.0, 0.4), |a| {
        (a.size_m[1], a.size_m[0].max(a.size_m[2]) / 2.0)
    })
}

/// A generator's placements with the building rule: `ground` is what plants stand on (terrain,
/// areas, models), `open_ground` the terrain and areas alone, `up` the distance straight up to a
/// model or block, `size` an asset's height and crown radius at scale 1.
pub fn evaluate_in_scene(
    g: &Generator,
    ground: &dyn Fn(f32, f32) -> Option<f32>,
    open_ground: &dyn Fn(f32, f32) -> Option<f32>,
    up: &dyn Fn([f32; 3]) -> Option<f32>,
    size: &dyn Fn(&str) -> (f32, f32),
) -> Evaluation {
    evaluate_avoiding(g, ground, &|p: &Placement| {
        let (h, r) = size(&p.asset);
        let open = open_ground(p.position[0], p.position[2]).unwrap_or(0.0);
        blocked_by_buildings(p.position, h * p.scale, r * p.scale, open, up)
    })
}
