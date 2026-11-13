//! Pure helpers behind the plantings in the scene: which generators a ground change touches, how
//! placements are grouped per (asset, variant), and picking a plant by its bounding cylinder.

use glam::{Mat4, Vec3};
use std::collections::{HashMap, VecDeque};
use visioncraft_core::{Asset, Generator, Placement, Project};
use visioncraft_flora as flora;

/// `[min, max]` in world x and z.
pub type Rect = [[f32; 2]; 2];

pub const EVERYTHING: Rect = [[f32::NEG_INFINITY; 2], [f32::INFINITY; 2]];

/// Milliseconds from an arbitrary start, for timings (`Instant` panics on wasm32).
pub fn now_ms() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
    }
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
}

pub fn rect_overlap(a: Rect, b: Rect) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

pub fn rect_union(a: Rect, b: Rect) -> Rect {
    [
        [a[0][0].min(b[0][0]), a[0][1].min(b[0][1])],
        [a[1][0].max(b[1][0]), a[1][1].max(b[1][1])],
    ]
}

fn rect_of(points: impl Iterator<Item = [f32; 2]>, margin: f32) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for p in points {
        let q = [
            [p[0] - margin, p[1] - margin],
            [p[0] + margin, p[1] + margin],
        ];
        r = Some(r.map_or(q, |r| rect_union(r, q)));
    }
    r
}

/// Where a generator can put plants, from its control geometry. `None` for a surface fill (it
/// follows its owner's mesh, which the scene knows) and for an empty brush.
pub fn generator_bounds(g: &Generator) -> Option<Rect> {
    match g {
        Generator::Single(s) => rect_of([[s.position[0], s.position[2]]].into_iter(), 0.5),
        Generator::Cluster(c) => rect_of([c.center].into_iter(), c.radius_m + 0.5),
        Generator::AreaScatter(a) => rect_of(a.outline.iter().copied(), 0.5),
        Generator::SplineMass(s) => {
            let b = rect_of(s.points.iter().copied(), 0.0)?;
            let diag = (b[1][0] - b[0][0]).hypot(b[1][1] - b[0][1]);
            // The curve may bulge a little beyond its control points.
            let m = s.offset_m.abs() + 1.0 + 0.1 * diag;
            Some([[b[0][0] - m, b[0][1] - m], [b[1][0] + m, b[1][1] + m]])
        }
        Generator::MixedBrush(b) => b
            .strokes
            .iter()
            .filter(|s| !s.erase)
            .filter_map(|s| rect_of(s.points.iter().copied(), s.radius_m + 0.5))
            .reduce(rect_union),
        Generator::SurfaceFill(_) => None,
    }
}

/// A log of ground changes, so a generator only re-evaluates when one touched its area.
#[derive(Default)]
pub struct DirtyLog {
    entries: VecDeque<(u64, Rect)>,
    stamp: u64,
    /// Changes with a stamp up to this one were forgotten.
    floor: u64,
}

const LOG_LIMIT: usize = 128;

impl DirtyLog {
    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    pub fn mark(&mut self, r: Rect) {
        self.stamp += 1;
        self.entries.push_back((self.stamp, r));
        while self.entries.len() > LOG_LIMIT {
            if let Some((s, _)) = self.entries.pop_front() {
                self.floor = s;
            }
        }
    }

    /// Whether a change after `since` overlaps `bounds` (`None`: anywhere).
    pub fn touches(&self, since: u64, bounds: Option<Rect>) -> bool {
        if since >= self.stamp {
            return false;
        }
        if since < self.floor {
            return true;
        }
        self.entries
            .iter()
            .filter(|(s, _)| *s > since)
            .any(|(_, r)| bounds.is_none_or(|b| rect_overlap(*r, b)))
    }
}

/// Instance matrices per (asset, variant), in first-seen order.
pub fn group_placements(placements: &[Placement], variants: u32) -> Vec<(String, u32, Vec<Mat4>)> {
    let mut out: Vec<(String, u32, Vec<Mat4>)> = Vec::new();
    let mut index: HashMap<(&str, u32), usize> = HashMap::new();
    for p in placements {
        let m = Mat4::from_translation(Vec3::from(p.position))
            * Mat4::from_rotation_y(p.rotation_y_deg.to_radians())
            * Mat4::from_scale(Vec3::splat(p.scale));
        let variant = p.variant % variants.max(1);
        let i = *index.entry((p.asset.as_str(), variant)).or_insert_with(|| {
            out.push((p.asset.clone(), variant, Vec::new()));
            out.len() - 1
        });
        out[i].2.push(m);
    }
    out
}

/// A plant's bounding cylinder, for picking.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inst {
    pub pos: [f32; 3],
    pub radius: f32,
    pub height: f32,
}

/// Mature (height, crown radius) in metres of a palette asset.
pub fn plant_size(project: &Project, assets: &[Asset], id: &str) -> (f32, f32) {
    if let Some(s) = id.strip_prefix("flora:").and_then(flora::Species::by_id) {
        return (s.mature_height_m, s.mature_width_m / 2.0);
    }
    if let Some(c) = project.species.iter().find(|c| c.id == id) {
        if let Some(s) = c.base.as_deref().and_then(flora::Species::by_id) {
            return (s.mature_height_m, s.mature_width_m / 2.0);
        }
        return (2.0, 0.6);
    }
    assets.iter().find(|a| a.id == id).map_or((1.0, 0.4), |a| {
        (a.size_m[1], a.size_m[0].max(a.size_m[2]) / 2.0)
    })
}

pub fn instances(project: &Project, assets: &[Asset], placements: &[Placement]) -> Vec<Inst> {
    let mut sizes: HashMap<&str, (f32, f32)> = HashMap::new();
    placements
        .iter()
        .map(|p| {
            let (h, r) = *sizes
                .entry(p.asset.as_str())
                .or_insert_with(|| plant_size(project, assets, &p.asset));
            Inst {
                pos: p.position,
                radius: (r * 0.8).max(0.25) * p.scale,
                height: h.max(0.2) * p.scale,
            }
        })
        .collect()
}

/// Distance along the ray to a vertical cylinder (sides and caps), if it hits.
pub fn ray_cylinder(o: Vec3, d: Vec3, c: &Inst) -> Option<f32> {
    let (cx, cz) = (c.pos[0], c.pos[2]);
    let a = d.x * d.x + d.z * d.z;
    let (ox, oz) = (o.x - cx, o.z - cz);
    // Interval inside the infinite cylinder.
    let (mut t0, mut t1) = if a < 1e-9 {
        if ox * ox + oz * oz > c.radius * c.radius {
            return None;
        }
        (f32::NEG_INFINITY, f32::INFINITY)
    } else {
        let b = ox * d.x + oz * d.z;
        let disc = b * b - a * (ox * ox + oz * oz - c.radius * c.radius);
        if disc < 0.0 {
            return None;
        }
        let s = disc.sqrt();
        ((-b - s) / a, (-b + s) / a)
    };
    // Intersect with the slab between the caps.
    if d.y.abs() < 1e-9 {
        if o.y < c.pos[1] || o.y > c.pos[1] + c.height {
            return None;
        }
    } else {
        let (s0, s1) = ((c.pos[1] - o.y) / d.y, (c.pos[1] + c.height - o.y) / d.y);
        t0 = t0.max(s0.min(s1));
        t1 = t1.min(s0.max(s1));
    }
    (t0 <= t1 && t1 > 0.0).then(|| t0.max(0.0))
}

/// The nearest instance the ray hits: its index and distance.
pub fn pick_instance(insts: &[Inst], o: Vec3, d: Vec3) -> Option<(usize, f32)> {
    insts
        .iter()
        .enumerate()
        .filter_map(|(i, c)| ray_cylinder(o, d, c).map(|t| (i, t)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f32, z: f32, r: f32) -> Rect {
        [[x - r, z - r], [x + r, z + r]]
    }

    #[test]
    fn dirty_log_only_touches_overlapping_generators() {
        let mut log = DirtyLog::default();
        let (a, b) = (square(0.0, 0.0, 5.0), square(100.0, 0.0, 5.0));
        assert!(!log.touches(0, Some(a)), "nothing changed yet");
        log.mark(square(2.0, 2.0, 1.0));
        assert!(log.touches(0, Some(a)));
        assert!(!log.touches(0, Some(b)));
        assert!(log.touches(0, None), "no bounds: anywhere");
        assert!(!log.touches(log.stamp(), Some(a)), "already seen");
    }

    #[test]
    fn dirty_log_forgetting_forces_a_reevaluation() {
        let mut log = DirtyLog::default();
        for i in 0..(LOG_LIMIT + 10) {
            log.mark(square(1000.0 + i as f32, 0.0, 0.1));
        }
        assert!(log.touches(0, Some(square(0.0, 0.0, 1.0))), "history lost");
        assert!(!log.touches(log.stamp() - 1, Some(square(0.0, 0.0, 1.0))));
    }

    #[test]
    fn bounds_cover_the_control_geometry() {
        use visioncraft_core::planting::{Cluster, Weighted};
        let c = Generator::Cluster(Cluster {
            id: "g1".into(),
            name: "c".into(),
            layer: "planting".into(),
            asset_palette: vec![Weighted {
                asset: "a".into(),
                weight: 1.0,
            }],
            center: [10.0, -4.0],
            radius_m: 3.0,
            count: 5,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: true,
            seed: 1,
        });
        let b = generator_bounds(&c).unwrap();
        assert!(b[0][0] <= 7.0 && b[1][0] >= 13.0 && b[0][1] <= -7.0 && b[1][1] >= -1.0);
    }

    #[test]
    fn grouping_keeps_first_seen_order_and_wraps_variants() {
        let p = |asset: &str, variant: u32, x: f32| Placement {
            asset: asset.into(),
            position: [x, 0.0, 0.0],
            rotation_y_deg: 0.0,
            scale: 1.0,
            variant,
        };
        let g = group_placements(
            &[
                p("a", 0, 0.0),
                p("b", 1, 1.0),
                p("a", 3, 2.0),
                p("a", 1, 3.0),
            ],
            3,
        );
        let keys: Vec<_> = g
            .iter()
            .map(|(a, v, m)| (a.as_str(), *v, m.len()))
            .collect();
        assert_eq!(keys, vec![("a", 0, 2), ("b", 1, 1), ("a", 1, 1)]);
    }

    #[test]
    fn cylinder_picking_hits_sides_caps_and_prefers_the_nearest() {
        let tree = Inst {
            pos: [0.0, 0.0, 0.0],
            radius: 1.0,
            height: 5.0,
        };
        let o = Vec3::new(-10.0, 2.0, 0.0);
        assert!((ray_cylinder(o, Vec3::X, &tree).unwrap() - 9.0).abs() < 1e-4);
        assert!(ray_cylinder(Vec3::new(-10.0, 6.0, 0.0), Vec3::X, &tree).is_none());
        // Straight down onto the top cap.
        let t = ray_cylinder(Vec3::new(0.3, 10.0, 0.2), Vec3::NEG_Y, &tree).unwrap();
        assert!((t - 5.0).abs() < 1e-4);
        assert!(ray_cylinder(Vec3::new(2.0, 10.0, 0.0), Vec3::NEG_Y, &tree).is_none());
        let far = Inst {
            pos: [5.0, 0.0, 0.0],
            ..tree
        };
        assert_eq!(
            pick_instance(&[far, tree], o, Vec3::X).map(|p| p.0),
            Some(1)
        );
        assert!(pick_instance(&[far, tree], o, Vec3::NEG_X).is_none());
    }
}
