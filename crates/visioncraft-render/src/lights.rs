//! Point and spot lights, and the coarse world-space grid of light lists the
//! forward pass reads (built on the CPU whenever the lights change).
//!
//! Units: `intensity` is the luminous flux in lumens, as on a lamp's box.
//! Point lights emit it over the full sphere, I = flux / (4 pi) candela;
//! spot lights use I = flux / pi regardless of the cone, so narrowing a spot
//! does not change its brightness (Filament's documentation, "Physically
//! based rendering in Filament", section 5.2.2, punctual lights).
//!
//! Grid: square cells over world X/Z (no subdivision in height), at most
//! `MAX_GRID` cells per side; the cell size grows with the area the lights
//! cover (at least `MIN_CELL` metres). A light goes into every cell its range
//! circle touches, so the windowed falloff reaches zero before any cell
//! border: no visible seams. Limits: `MAX_LIGHTS` lights in total (later ones
//! are ignored) and `MAX_PER_CELL` per cell; a crowded cell keeps the lights
//! brightest at its centre, and the dropped ones end at that cell's edge.

use glam::{Vec2, Vec3};

pub const MAX_LIGHTS: usize = 1024;
pub const MAX_PER_CELL: usize = 32;
pub const MAX_GRID: u32 = 64;
pub const MIN_CELL: f32 = 2.0;
/// Bytes per light in the storage buffer (four vec4).
pub const LIGHT_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    Point,
    /// `direction` is where the spot points; `inner` and `outer` are cone
    /// half-angles in radians (full intensity inside `inner`, zero outside
    /// `outer`).
    Spot {
        direction: Vec3,
        inner: f32,
        outer: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub position: Vec3,
    pub kind: LightKind,
    /// Linear Rec.709 colour (luminance about 1 for a white lamp).
    pub color: Vec3,
    /// Luminous flux in lumens (a 60 W-equivalent bulb is ~800 lm).
    pub intensity: f32,
    /// Distance where the light has faded to zero (metres).
    pub range_m: f32,
}

impl Light {
    /// Luminous intensity in candela.
    pub fn candela(&self) -> f32 {
        match self.kind {
            LightKind::Point => self.intensity / (4.0 * std::f32::consts::PI),
            LightKind::Spot { .. } => self.intensity / std::f32::consts::PI,
        }
    }
}

/// The CPU side of the light grid, ready to upload.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grid {
    pub origin: Vec2,
    pub cell: f32,
    pub dims: (u32, u32),
    /// Per cell (row-major, x fastest) first index and count into this same
    /// array, then the light indices.
    pub data: Vec<u32>,
    /// Lights left out of at least one cell because it was full.
    pub dropped: usize,
}

/// Which grid cells a circle touches: the cell rectangle of its bounding
/// square, minus the cells whose nearest point is outside the circle.
fn touches(center: Vec2, radius: f32, lo: Vec2, hi: Vec2) -> bool {
    let nearest = center.clamp(lo, hi);
    nearest.distance_squared(center) <= radius * radius
}

pub fn build_grid(lights: &[Light]) -> Grid {
    let lights = &lights[..lights.len().min(MAX_LIGHTS)];
    let Some((lo, hi)) = lights
        .iter()
        .map(|l| {
            let c = Vec2::new(l.position.x, l.position.z);
            (c - l.range_m, c + l.range_m)
        })
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    else {
        return Grid {
            cell: 1.0,
            dims: (1, 1),
            data: vec![2, 0],
            ..Default::default()
        };
    };
    let extent = (hi - lo).max_element().max(MIN_CELL);
    let cell = (extent / MAX_GRID as f32).max(MIN_CELL);
    let dims = (
        (((hi.x - lo.x) / cell).ceil() as u32).clamp(1, MAX_GRID),
        (((hi.y - lo.y) / cell).ceil() as u32).clamp(1, MAX_GRID),
    );
    let mut cells: Vec<Vec<u32>> = vec![Vec::new(); (dims.0 * dims.1) as usize];
    for (i, l) in lights.iter().enumerate() {
        let c = Vec2::new(l.position.x, l.position.z);
        let first = ((c - l.range_m - lo) / cell).floor();
        let last = ((c + l.range_m - lo) / cell).floor();
        for z in (first.y.max(0.0) as u32)..=(last.y as u32).min(dims.1 - 1) {
            for x in (first.x.max(0.0) as u32)..=(last.x as u32).min(dims.0 - 1) {
                let cell_lo = lo + Vec2::new(x as f32, z as f32) * cell;
                if touches(c, l.range_m, cell_lo, cell_lo + cell) {
                    cells[(z * dims.0 + x) as usize].push(i as u32);
                }
            }
        }
    }
    let mut dropped = vec![false; lights.len()];
    let header = 2 * cells.len();
    let mut data = vec![0u32; header];
    for (k, list) in cells.iter_mut().enumerate() {
        if list.len() > MAX_PER_CELL {
            let centre = lo
                + (Vec2::new((k as u32 % dims.0) as f32, (k as u32 / dims.0) as f32) + 0.5) * cell;
            let strength = |i: &u32| {
                let l = &lights[*i as usize];
                let d = Vec2::new(l.position.x, l.position.z).distance_squared(centre);
                l.candela() * l.color.max_element() / (d + 1.0)
            };
            list.sort_by(|a, b| strength(b).total_cmp(&strength(a)));
            for i in &list[MAX_PER_CELL..] {
                dropped[*i as usize] = true;
            }
            list.truncate(MAX_PER_CELL);
        }
        data[2 * k] = data.len() as u32;
        data[2 * k + 1] = list.len() as u32;
        data.extend_from_slice(list);
    }
    Grid {
        origin: lo,
        cell,
        dims,
        data,
        dropped: dropped.iter().filter(|d| **d).count(),
    }
}

/// The light storage buffer: per light four vec4 (see scene.wgsl `Light`).
pub fn pack(lights: &[Light]) -> Vec<f32> {
    let mut out = Vec::with_capacity(lights.len().clamp(1, MAX_LIGHTS) * 16);
    for l in &lights[..lights.len().min(MAX_LIGHTS)] {
        let range = l.range_m.max(0.01);
        let color = l.color * l.candela();
        let (dir, scale, offset) = match l.kind {
            LightKind::Point => (Vec3::NEG_Y, 0.0, 1.0),
            LightKind::Spot {
                direction,
                inner,
                outer,
            } => {
                let outer = outer.clamp(1e-3, std::f32::consts::FRAC_PI_2);
                let cos_outer = outer.cos();
                let cos_inner = inner.clamp(0.0, outer).cos();
                let scale = 1.0 / (cos_inner - cos_outer).max(1e-4);
                (
                    direction.normalize_or(Vec3::NEG_Y),
                    scale,
                    -cos_outer * scale,
                )
            }
        };
        out.extend_from_slice(&[l.position.x, l.position.y, l.position.z, range]);
        out.extend_from_slice(&[color.x, color.y, color.z, 0.0]);
        out.extend_from_slice(&[dir.x, dir.y, dir.z, scale]);
        out.extend_from_slice(&[offset, 0.0, 0.0, 0.0]);
    }
    if out.is_empty() {
        out.resize(16, 0.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, z: f32, range: f32) -> Light {
        Light {
            position: Vec3::new(x, 1.0, z),
            kind: LightKind::Point,
            color: Vec3::ONE,
            intensity: 800.0,
            range_m: range,
        }
    }

    fn cell_lights(g: &Grid, x: f32, z: f32) -> &[u32] {
        let c = ((Vec2::new(x, z) - g.origin) / g.cell).floor();
        assert!(c.x >= 0.0 && c.y >= 0.0, "outside grid");
        let k = (c.y as u32 * g.dims.0 + c.x as u32) as usize;
        let (first, count) = (g.data[2 * k] as usize, g.data[2 * k + 1] as usize);
        &g.data[first..first + count]
    }

    #[test]
    fn every_lit_point_finds_its_lights() {
        let lights = [
            point(0.0, 0.0, 6.0),
            point(9.0, 3.0, 4.0),
            point(-20.0, 15.0, 10.0),
        ];
        let g = build_grid(&lights);
        assert!(g.dims.0 <= MAX_GRID && g.dims.1 <= MAX_GRID);
        // Sample points inside each range circle: the light is in that cell.
        for (i, l) in lights.iter().enumerate() {
            for k in 0..64 {
                let a = k as f32 * 0.7;
                let r = l.range_m * (k % 8) as f32 / 8.0;
                let (x, z) = (l.position.x + r * a.cos(), l.position.z + r * a.sin());
                assert!(
                    cell_lights(&g, x, z).contains(&(i as u32)),
                    "light {i} at {x},{z}"
                );
            }
        }
        // A cell far from light 1 does not list it.
        assert!(!cell_lights(&g, -20.0, 15.0).contains(&1));
        assert_eq!(g.dropped, 0);
    }

    #[test]
    fn crowded_cells_keep_the_brightest() {
        let mut lights: Vec<Light> = (0..40).map(|i| point(i as f32 * 0.01, 0.0, 3.0)).collect();
        lights[39].intensity = 1e5;
        let g = build_grid(&lights);
        let list = cell_lights(&g, 0.0, 0.0);
        assert_eq!(list.len(), MAX_PER_CELL);
        assert!(list.contains(&39));
        assert!(g.dropped >= 8, "{}", g.dropped);
        assert!(build_grid(&[]).data == vec![2, 0]);
    }

    #[test]
    fn spot_cone_terms() {
        let l = Light {
            kind: LightKind::Spot {
                direction: Vec3::NEG_Y,
                inner: 0.3,
                outer: 0.5,
            },
            ..point(0.0, 0.0, 5.0)
        };
        let p = pack(&[l]);
        let (scale, offset) = (p[11], p[12]);
        let cone = |angle: f32| (angle.cos() * scale + offset).clamp(0.0, 1.0);
        assert!((cone(0.0) - 1.0).abs() < 1e-6 && (cone(0.3) - 1.0).abs() < 1e-3);
        assert!(cone(0.5).abs() < 1e-3 && cone(0.4) > 0.2 && cone(0.4) < 0.8);
        // Candela: spots use flux / pi, points flux / 4 pi.
        assert!((l.candela() * std::f32::consts::PI - 800.0).abs() < 1e-3);
    }
}
