//! Control handles of a planting generator (spline points, area vertices, cluster centre and
//! radius, a single plant's position, rotation and scale): where they are, which one a pointer
//! is on, and what moving one does. Pure functions over `Generator`, tested without a window.

use glam::Vec2;
use serde_json::{Value, json};
use visioncraft_core::Generator;

/// Distance of a single plant's rotation and scale handles from it, in metres (scale 1).
pub const ARM: f32 = 1.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// A control point of a line or a vertex of an area outline.
    Point(usize),
    Center,
    Radius,
    Position,
    Rotate,
    Scale,
}

/// Ground position (x, z) of every handle of a generator.
pub fn handles(g: &Generator) -> Vec<(Handle, [f32; 2])> {
    match g {
        Generator::SplineMass(s) => s
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| (Handle::Point(i), *p))
            .collect(),
        Generator::AreaScatter(a) => a
            .outline
            .iter()
            .enumerate()
            .map(|(i, p)| (Handle::Point(i), *p))
            .collect(),
        Generator::Cluster(c) => vec![
            (Handle::Center, c.center),
            (Handle::Radius, [c.center[0] + c.radius_m, c.center[1]]),
        ],
        Generator::Single(s) => {
            let (x, z) = (s.position[0], s.position[2]);
            let a = s.rotation_y_deg.to_radians();
            // rotation_y turns +X towards -Z; the scale arm is a quarter turn further.
            let rot = [x + ARM * a.cos(), z - ARM * a.sin()];
            let b = a + std::f32::consts::FRAC_PI_2;
            let sc = [x + ARM * s.scale * b.cos(), z - ARM * s.scale * b.sin()];
            vec![
                (Handle::Position, [x, z]),
                (Handle::Rotate, rot),
                (Handle::Scale, sc),
            ]
        }
        Generator::MixedBrush(_) | Generator::SurfaceFill(_) => Vec::new(),
    }
}

/// The control polyline of a line or an area (and whether it closes).
pub fn polyline(g: &Generator) -> Option<(&[[f32; 2]], bool)> {
    match g {
        Generator::SplineMass(s) => Some((&s.points, s.closed)),
        Generator::AreaScatter(a) => Some((&a.outline, true)),
        _ => None,
    }
}

/// The generator with a handle dragged to `to` (x, z); `ground_y` is the ground height there
/// (a single plant stands on it).
pub fn move_handle(g: &Generator, h: Handle, to: [f32; 2], ground_y: f32) -> Option<Generator> {
    let mut g = g.clone();
    match (&mut g, h) {
        (Generator::SplineMass(s), Handle::Point(i)) => *s.points.get_mut(i)? = to,
        (Generator::AreaScatter(a), Handle::Point(i)) => *a.outline.get_mut(i)? = to,
        (Generator::Cluster(c), Handle::Center) => c.center = to,
        (Generator::Cluster(c), Handle::Radius) => {
            c.radius_m = (to[0] - c.center[0]).hypot(to[1] - c.center[1]).max(0.2);
        }
        (Generator::Single(s), Handle::Position) => s.position = [to[0], ground_y, to[1]],
        (Generator::Single(s), Handle::Rotate) => {
            let (dx, dz) = (to[0] - s.position[0], to[1] - s.position[2]);
            if dx.hypot(dz) > 1e-3 {
                s.rotation_y_deg = (-dz).atan2(dx).to_degrees();
            }
        }
        (Generator::Single(s), Handle::Scale) => {
            let d = (to[0] - s.position[0]).hypot(to[1] - s.position[2]);
            s.scale = (d / ARM).clamp(0.05, 20.0);
        }
        _ => return None,
    }
    Some(g)
}

/// A new control point on edge `edge` (between point `edge` and the next) of a line or an area.
pub fn insert_point(g: &Generator, edge: usize, at: [f32; 2]) -> Option<Generator> {
    let mut g = g.clone();
    let pts = match &mut g {
        Generator::SplineMass(s) => &mut s.points,
        Generator::AreaScatter(a) => &mut a.outline,
        _ => return None,
    };
    (edge < pts.len()).then(|| pts.insert(edge + 1, at))?;
    Some(g)
}

/// Without control point `i`; a line keeps at least two, an area three.
pub fn remove_point(g: &Generator, i: usize) -> Option<Generator> {
    let mut g = g.clone();
    let (pts, min) = match &mut g {
        Generator::SplineMass(s) => (&mut s.points, 2),
        Generator::AreaScatter(a) => (&mut a.outline, 3),
        _ => return None,
    };
    (pts.len() > min && i < pts.len()).then(|| pts.remove(i))?;
    Some(g)
}

/// The handle whose screen position is nearest to `p` and within `radius` pixels.
pub fn hit_handle(screen: &[(Handle, Vec2)], p: Vec2, radius: f32) -> Option<Handle> {
    screen
        .iter()
        .map(|(h, q)| (*h, q.distance(p)))
        .filter(|(_, d)| *d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(h, _)| h)
}

/// The edge of a screen polyline within `radius` pixels of `p`, and how far along it (0..1) the
/// closest point is.
pub fn edge_hit_screen(pts: &[Vec2], closed: bool, p: Vec2, radius: f32) -> Option<(usize, f32)> {
    let n = pts.len();
    let edges = if closed { n } else { n.saturating_sub(1) };
    (0..edges)
        .filter_map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let ab = b - a;
            let t = if ab.length_squared() > 1e-9 {
                ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = (a + ab * t).distance(p);
            (d <= radius).then_some((i, t, d))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(i, t, _)| (i, t))
}

/// The fields in which `new` differs from `old`, as `planting.update` parameters (`None` if equal).
pub fn diff_params(old: &Generator, new: &Generator) -> Option<Value> {
    let (a, b) = (
        serde_json::to_value(old).ok()?,
        serde_json::to_value(new).ok()?,
    );
    let (a, b) = (a.as_object()?, b.as_object()?);
    let mut out = serde_json::Map::new();
    for (k, v) in b {
        if k != "id" && k != "type" && a.get(k) != Some(v) {
            out.insert(k.clone(), v.clone());
        }
    }
    (!out.is_empty()).then(|| {
        out.insert("id".into(), json!(old.id()));
        Value::Object(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_core::planting::{AreaScatter, Cluster, Single, Spacing, SplineMass, Weighted};

    fn pal() -> Vec<Weighted> {
        vec![Weighted {
            asset: "flora:quercus-robur".into(),
            weight: 1.0,
        }]
    }

    fn area() -> Generator {
        Generator::AreaScatter(AreaScatter {
            id: "g1".into(),
            name: "Area".into(),
            planted_age_years: None,
            layer: "planting".into(),
            asset_palette: pal(),
            outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            density_per_m2: 0.5,
            position_jitter: 1.0,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: true,
            min_distance_m: 0.0,
            seed: 3,
            layers: vec![],
            avoid_buildings: false,
        })
    }

    fn single(rot: f32, scale: f32) -> Generator {
        Generator::Single(Single {
            id: "g2".into(),
            name: "S".into(),
            planted_age_years: None,
            layer: "planting".into(),
            asset: "flora:quercus-robur".into(),
            position: [5.0, 1.0, 5.0],
            rotation_y_deg: rot,
            scale,
        })
    }

    #[test]
    fn hit_testing_picks_the_nearest_handle_in_range() {
        let s = [
            (Handle::Point(0), Vec2::new(10.0, 10.0)),
            (Handle::Point(1), Vec2::new(18.0, 10.0)),
        ];
        assert_eq!(
            hit_handle(&s, Vec2::new(15.0, 10.0), 12.0),
            Some(Handle::Point(1))
        );
        assert_eq!(hit_handle(&s, Vec2::new(60.0, 10.0), 12.0), None);
    }

    #[test]
    fn edge_hits_report_the_edge_and_position() {
        let sq = [Vec2::ZERO, Vec2::new(100.0, 0.0), Vec2::new(100.0, 100.0)];
        let (i, t) = edge_hit_screen(&sq, false, Vec2::new(40.0, 5.0), 8.0).unwrap();
        assert_eq!(i, 0);
        assert!((t - 0.4).abs() < 1e-4);
        assert!(edge_hit_screen(&sq, false, Vec2::new(50.0, 50.0), 8.0).is_none());
        // Closed: the edge from the last point back to the first exists.
        assert_eq!(
            edge_hit_screen(&sq, true, Vec2::new(50.0, 52.0), 8.0).map(|e| e.0),
            Some(2)
        );
    }

    #[test]
    fn dragging_a_vertex_moves_only_that_vertex() {
        let g = move_handle(&area(), Handle::Point(2), [12.0, 13.0], 0.0).unwrap();
        let (pts, closed) = polyline(&g).unwrap();
        assert!(closed);
        assert_eq!(pts[2], [12.0, 13.0]);
        assert_eq!(pts[1], [10.0, 0.0]);
        assert!(move_handle(&area(), Handle::Point(9), [0.0, 0.0], 0.0).is_none());
    }

    #[test]
    fn edges_insert_and_vertices_delete_with_minimum_counts() {
        let g = insert_point(&area(), 3, [0.0, 5.0]).unwrap();
        assert_eq!(polyline(&g).unwrap().0.len(), 5);
        assert_eq!(polyline(&g).unwrap().0[4], [0.0, 5.0]);
        assert_eq!(
            polyline(&remove_point(&area(), 0).unwrap())
                .unwrap()
                .0
                .len(),
            3
        );
        let triangle = remove_point(&area(), 0).unwrap();
        assert!(
            remove_point(&triangle, 0).is_none(),
            "an area keeps three corners"
        );
        let line = Generator::SplineMass(SplineMass {
            id: "g3".into(),
            name: "L".into(),
            planted_age_years: None,
            layer: "planting".into(),
            asset_palette: pal(),
            points: vec![[0.0, 0.0], [5.0, 0.0]],
            closed: false,
            spacing: Spacing::Count(4),
            spacing_jitter: 0.0,
            offset_m: 0.0,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: false,
            seed: 1,
            straight: false,
            avoid_buildings: false,
        });
        assert!(remove_point(&line, 0).is_none(), "a line keeps two points");
    }

    #[test]
    fn single_rotation_and_scale_handles_round_trip() {
        let g = single(30.0, 1.0);
        let h = handles(&g);
        let rot = h.iter().find(|h| h.0 == Handle::Rotate).unwrap().1;
        let back = move_handle(&single(0.0, 1.0), Handle::Rotate, rot, 0.0).unwrap();
        let Generator::Single(s) = back else { panic!() };
        assert!(
            (s.rotation_y_deg - 30.0).abs() < 1e-3,
            "{}",
            s.rotation_y_deg
        );
        let sc = handles(&single(0.0, 2.0))
            .iter()
            .find(|h| h.0 == Handle::Scale)
            .unwrap()
            .1;
        let Generator::Single(s) = move_handle(&single(0.0, 1.0), Handle::Scale, sc, 0.0).unwrap()
        else {
            panic!()
        };
        assert!((s.scale - 2.0).abs() < 1e-4);
        let Generator::Single(m) = move_handle(&g, Handle::Position, [7.0, 8.0], 2.5).unwrap()
        else {
            panic!()
        };
        assert_eq!(m.position, [7.0, 2.5, 8.0]);
    }

    #[test]
    fn cluster_radius_follows_the_handle() {
        let c = Generator::Cluster(Cluster {
            id: "g4".into(),
            name: "C".into(),
            planted_age_years: None,
            layer: "planting".into(),
            asset_palette: pal(),
            center: [1.0, 1.0],
            radius_m: 2.0,
            count: 3,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: true,
            seed: 1,
        });
        let Generator::Cluster(m) = move_handle(&c, Handle::Radius, [1.0, 6.0], 0.0).unwrap()
        else {
            panic!()
        };
        assert!((m.radius_m - 5.0).abs() < 1e-5);
    }

    #[test]
    fn updates_send_only_changes() {
        let moved = move_handle(&area(), Handle::Point(0), [1.0, 1.0], 0.0).unwrap();
        let d = diff_params(&area(), &moved).unwrap();
        assert_eq!(d["id"], "g1");
        assert!(d.get("outline").is_some() && d.get("seed").is_none());
        assert!(diff_params(&area(), &area()).is_none());
    }
}
