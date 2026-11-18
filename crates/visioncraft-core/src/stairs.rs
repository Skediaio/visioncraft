//! Steps and stairs: what turns a closed extrusion with a rectangular footprint into a flight of
//! solid steps, drawn by `shape_mesh` with stable face names so materials attach:
//! `<id>:treads`, `<id>:risers`, `<id>:sides` (both flanks, the back and the underside).
//!
//! `base` is the bottom of the flight and `base + height` the level you arrive on. `up_edge`
//! names the outline edge (point `i` to point `i + 1`) at the top; the bottom is the opposite
//! edge. The flight runs between the midpoints of those two edges.
//!
//! Every step is a block from the bottom up (no floating slabs); the blocks sit side by side, so
//! the mesh is closed and its volume is the stairs' own. The step count is
//! `round(height / riser_m)` and the risers are all `height / count` high. With a `tread_m`
//! the treads are that deep and a footprint longer than the flight ends in a landing (the top
//! tread runs on to the back); without one the treads share the footprint evenly.

use crate::house::Sink;
use serde::{Deserialize, Serialize};

type P2 = [f32; 2];

/// Faces a flight has, in this order.
pub const STAIR_FACES: [&str; 3] = ["treads", "risers", "sides"];
const TREADS: usize = 0;
const RISERS: usize = 1;
const SIDES: usize = 2;

/// The riser height a flight starts with (metres).
pub const DEFAULT_RISER_M: f32 = 0.17;
/// The tread depth the Blondel rule pairs with the default riser (metres).
pub const DEFAULT_TREAD_M: f32 = 0.30;
/// Blondel's rule for a comfortable step: `2 * riser + tread` lies in this range (metres).
pub const BLONDEL_M: (f32, f32) = (0.60, 0.65);
const MAX_STEPS: usize = 80;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stairs {
    /// The outline edge (point `up_edge` to the next one) at the top, where you arrive.
    pub up_edge: usize,
    /// The height of a riser to aim for, 0.05..=0.30; the step count is `round(height / riser_m)`.
    /// Default [`DEFAULT_RISER_M`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub riser_m: Option<f32>,
    /// The depth of a tread, 0.15..=0.60. Without it the treads share the footprint evenly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tread_m: Option<f32>,
}

/// Where a flight lies and how its steps are cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// The middle of the bottom edge.
    pub origin: P2,
    /// Unit direction up the flight.
    pub dir: P2,
    /// From the bottom edge to the top edge.
    pub length: f32,
    pub width: f32,
    pub steps: usize,
    /// Height of one riser.
    pub riser: f32,
    /// Depth of one tread (the top one runs on to the back when there is a landing).
    pub tread: f32,
    /// Remarks that do not stop the flight being drawn.
    pub warnings: Vec<String>,
}

impl Stairs {
    pub fn new(up_edge: usize) -> Self {
        Self {
            up_edge,
            riser_m: None,
            tread_m: None,
        }
    }

    /// The flight on a 4-point rectangular `outline` rising `height`; the reason if it cannot be
    /// drawn.
    pub fn geometry(&self, outline: &[P2], height: f32) -> Result<Geometry, String> {
        if outline.len() != 4 {
            return Err("stairs need a 4-point rectangular outline".into());
        }
        if self.up_edge >= 4 {
            return Err("`up_edge` must be 0..=3".into());
        }
        if !(height.is_finite() && (0.05..=6.0).contains(&height)) {
            return Err("stairs need a `height` within 0.05..=6 m".into());
        }
        let riser_m = self.riser_m.unwrap_or(DEFAULT_RISER_M);
        if !(riser_m.is_finite() && (0.05..=0.30).contains(&riser_m)) {
            return Err("`riser_m` must be within 0.05..=0.30 m".into());
        }
        if let Some(t) = self.tread_m
            && !(t.is_finite() && (0.15..=0.60).contains(&t))
        {
            return Err("`tread_m` must be within 0.15..=0.60 m".into());
        }
        let i = self.up_edge;
        let (a, b) = (outline[i], outline[(i + 1) % 4]);
        let (c, d) = (outline[(i + 2) % 4], outline[(i + 3) % 4]);
        let dist = |p: P2, q: P2| (q[0] - p[0]).hypot(q[1] - p[1]);
        let (width, side) = (dist(a, b), dist(b, c));
        let tol = 0.02;
        let dot = (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]);
        if width < 0.2
            || side < 0.15
            || (dist(c, d) - width).abs() > tol
            || (dist(d, a) - side).abs() > tol
            || (dot / (width * side)).abs() > 0.02
        {
            return Err(
                "the stairs' outline must be a rectangle at least 0.2 m wide and 0.15 m long"
                    .into(),
            );
        }
        let top = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let origin = [(c[0] + d[0]) / 2.0, (c[1] + d[1]) / 2.0];
        let length = dist(origin, top);
        let dir = [(top[0] - origin[0]) / length, (top[1] - origin[1]) / length];
        let steps = ((height / riser_m).round() as usize).max(1);
        if steps > MAX_STEPS {
            return Err(format!("{steps} steps is too many (at most {MAX_STEPS})"));
        }
        let riser = height / steps as f32;
        if riser > 0.4 {
            return Err(format!(
                "a single riser of {riser:.2} m is too tall; give a lower `height`"
            ));
        }
        let mut warnings = Vec::new();
        let tread = match self.tread_m {
            Some(t) if t * steps as f32 <= length + 1e-4 => t,
            Some(t) => {
                warnings.push(format!(
                    "the footprint is too short for {steps} treads of {t:.2} m ({length:.2} m long); \
                     the treads are {:.2} m deep",
                    length / steps as f32
                ));
                length / steps as f32
            }
            None => length / steps as f32,
        };
        if tread < 0.1 {
            return Err(format!(
                "the footprint is too short for {steps} steps: {tread:.2} m a tread"
            ));
        }
        let blondel = 2.0 * riser + tread;
        if !(BLONDEL_M.0 - 1e-3..=BLONDEL_M.1 + 1e-3).contains(&blondel) {
            warnings.push(format!(
                "uncomfortable steps: 2 x riser {riser:.3} + tread {tread:.3} = {blondel:.2} m, \
                 Blondel's rule wants {:.2}..{:.2} m",
                BLONDEL_M.0, BLONDEL_M.1
            ));
        }
        Ok(Geometry {
            origin,
            dir,
            length,
            width,
            steps,
            riser,
            tread,
            warnings,
        })
    }
}

fn quad(s: &mut dyn Sink, face: usize, v: [[f32; 3]; 4], outward: [f32; 3]) {
    s.tri(face, [v[0], v[1], v[2]], outward);
    s.tri(face, [v[0], v[2], v[3]], outward);
}

/// How low each step's block goes: the lowest ground under the step's footprint (its corners,
/// edge midpoints and a few points between, as [`crate::shape::footing`] samples a footprint)
/// less `lift`, the height the whole flight stands at, but never above `lo`, the bottom of the
/// flight. Without ground every block starts at `lo`.
pub fn step_bottoms(
    g: &Geometry,
    lo: f32,
    lift: f32,
    ground: &dyn Fn(f32, f32) -> Option<f32>,
) -> Vec<f32> {
    let d = g.dir;
    let n = [-d[1], d[0]];
    let hw = g.width / 2.0;
    (1..=g.steps)
        .map(|k| {
            let u0 = (k - 1) as f32 * g.tread;
            let u1 = if k == g.steps {
                g.length
            } else {
                k as f32 * g.tread
            };
            let mut low = f32::INFINITY;
            for i in 0..=4 {
                for j in 0..=4 {
                    let u = u0 + (u1 - u0) * i as f32 / 4.0;
                    let w = -hw + g.width * j as f32 / 4.0;
                    let (x, z) = (
                        g.origin[0] + d[0] * u + n[0] * w,
                        g.origin[1] + d[1] * u + n[1] * w,
                    );
                    low = low.min(ground(x, z).unwrap_or(0.0));
                }
            }
            (low - lift).min(lo)
        })
        .collect()
}

/// Draws the flight `g` from `lo` up on the faces of [`STAIR_FACES`]. `bottoms` holds the bottom
/// of each step's block (see [`step_bottoms`]), all at or under `lo`, so that on uneven ground
/// every step stays solid down to the ground under it.
pub(crate) fn mesh(s: &mut dyn Sink, g: &Geometry, lo: f32, bottoms: &[f32]) {
    let d = g.dir;
    let n = [-d[1], d[0]];
    let hw = g.width / 2.0;
    let p = |u: f32, w: f32, y: f32| {
        [
            g.origin[0] + d[0] * u + n[0] * w,
            y,
            g.origin[1] + d[1] * u + n[1] * w,
        ]
    };
    let (d3, n3, up) = ([d[0], 0.0, d[1]], [n[0], 0.0, n[1]], [0.0, 1.0, 0.0]);
    let neg = |v: [f32; 3]| v.map(|c| -c);
    let hi = lo + g.riser * g.steps as f32;
    let bottom = |k: usize| bottoms.get(k - 1).copied().unwrap_or(lo).min(lo);
    // A wall across the flight at `u` from `ya` to `yb`, facing `outward`.
    let wall = |s: &mut dyn Sink, face: usize, u: f32, ya: f32, yb: f32, outward: [f32; 3]| {
        quad(
            s,
            face,
            [p(u, -hw, ya), p(u, hw, ya), p(u, hw, yb), p(u, -hw, yb)],
            outward,
        );
    };
    wall(s, SIDES, g.length, bottom(g.steps), hi, d3);
    // What the block before has under the riser: the bottom of the flight for the first step.
    let mut before = lo;
    for k in 1..=g.steps {
        let u0 = (k - 1) as f32 * g.tread;
        let u1 = if k == g.steps {
            g.length
        } else {
            k as f32 * g.tread
        };
        let (y0, y1) = (lo + (k - 1) as f32 * g.riser, lo + k as f32 * g.riser);
        let yb = bottom(k);
        quad(
            s,
            SIDES,
            [p(u0, -hw, yb), p(u1, -hw, yb), p(u1, hw, yb), p(u0, hw, yb)],
            neg(up),
        );
        quad(
            s,
            TREADS,
            [p(u0, -hw, y1), p(u1, -hw, y1), p(u1, hw, y1), p(u0, hw, y1)],
            up,
        );
        wall(s, RISERS, u0, y0, y1, neg(d3));
        // Where this block goes lower than the one before, its front shows under it; where the
        // one before goes lower, its back shows.
        if yb < before {
            wall(s, SIDES, u0, yb, before, neg(d3));
        } else if yb > before && k > 1 {
            wall(s, SIDES, u0, before, yb, d3);
        }
        before = yb;
        for (w, out) in [(-hw, neg(n3)), (hw, n3)] {
            quad(
                s,
                SIDES,
                [p(u0, w, yb), p(u1, w, yb), p(u1, w, y1), p(u0, w, y1)],
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{Extrusion, Shape, shape_mesh};

    /// A flight of `len` m by `width` m from z = 0 (bottom) up to z = `len`, rising `height`.
    fn flight(len: f32, width: f32, height: f32, stairs: Stairs) -> Shape {
        Shape::Extrusion(Extrusion {
            id: "S1".into(),
            name: "Stairs".into(),
            layer: "shapes".into(),
            // Edge 2 (point 2 to 3) is the top, at z = len.
            points: vec![[0.0, 0.0], [width, 0.0], [width, len], [0.0, len]],
            curves: Default::default(),
            smooth: false,
            closed: true,
            base: 0.0,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: Some(stairs),
            raised_bed: None,
            furniture: None,
        })
    }

    fn top_edge() -> Stairs {
        Stairs::new(2)
    }

    #[test]
    fn the_step_count_and_positions_follow_the_rise() {
        // 0.51 m at 0.17 m a riser: 3 steps, each 0.17 high, 0.30 deep with a 0.30 tread.
        let mut st = top_edge();
        st.tread_m = Some(0.30);
        let outline = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.2], [0.0, 1.2]];
        let g = st.geometry(&outline, 0.51).unwrap();
        assert_eq!(g.steps, 3);
        assert!((g.riser - 0.17).abs() < 1e-5 && (g.tread - 0.30).abs() < 1e-5);
        assert!((g.length - 1.2).abs() < 1e-5 && (g.width - 1.0).abs() < 1e-5);
        assert_eq!(g.origin, [0.5, 0.0]);
        assert!(g.dir[1] > 0.99);
        assert!(g.warnings.is_empty(), "{:?}", g.warnings);
        // Rounds: 0.5 m is still 3 steps, 0.6 m is 4 (0.15 each).
        assert_eq!(st.geometry(&outline, 0.5).unwrap().steps, 3);
        let g4 = st.geometry(&outline, 0.6).unwrap();
        assert_eq!(g4.steps, 4);
        assert!((g4.riser - 0.15).abs() < 1e-5);
    }

    #[test]
    fn treads_sit_at_every_riser_height_and_the_footprint_is_filled() {
        let shape = flight(
            1.2,
            1.0,
            0.51,
            Stairs {
                tread_m: Some(0.30),
                ..top_edge()
            },
        );
        let m = shape_mesh(&shape).unwrap();
        let treads = m.faces.iter().position(|f| f.name == "S1:treads").unwrap() as u32;
        let mut heights: Vec<f32> = Vec::new();
        for (t, f) in m.triangles.iter().zip(&m.triangle_faces) {
            if *f == treads {
                let y = m.positions[t[0] as usize][1];
                if !heights.iter().any(|h| (h - y).abs() < 1e-4) {
                    heights.push(y);
                }
            }
        }
        heights.sort_by(f32::total_cmp);
        let want = [0.17, 0.34, 0.51];
        assert_eq!(heights.len(), 3, "{heights:?}");
        for (h, w) in heights.iter().zip(want) {
            assert!((h - w).abs() < 1e-4, "{heights:?}");
        }
        // The treads add up to the footprint's area (1.0 x 1.2).
        let area: f32 = m
            .triangles
            .iter()
            .zip(&m.triangle_faces)
            .filter(|(_, f)| **f == treads)
            .map(|(t, _)| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        assert!((area - 1.2).abs() < 1e-3, "{area}");
    }

    #[test]
    fn the_mesh_is_finite_named_bounded_and_closed() {
        for up_edge in 0..4 {
            let mut shape = flight(1.5, 0.9, 0.6, Stairs::new(up_edge));
            if let Shape::Extrusion(e) = &mut shape {
                e.base = 0.2;
            }
            let m = shape_mesh(&shape).unwrap();
            let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, ["S1:treads", "S1:risers", "S1:sides"]);
            for i in 0..3u32 {
                assert!(m.triangle_faces.contains(&i), "face {i} is empty");
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &m.positions {
                for k in 0..3 {
                    assert!(p[k].is_finite());
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            assert!(
                (lo[1] - 0.2).abs() < 1e-5 && (hi[1] - 0.8).abs() < 1e-5,
                "{lo:?} {hi:?}"
            );
            assert!(
                lo[0].abs() < 1e-5 && (hi[0] - 0.9).abs() < 1e-5,
                "{lo:?} {hi:?}"
            );
            assert!(
                lo[2].abs() < 1e-5 && (hi[2] - 1.5).abs() < 1e-5,
                "{lo:?} {hi:?}"
            );
        }
    }

    #[test]
    fn the_volume_is_a_flight_of_blocks() {
        let shape = flight(
            1.2,
            1.0,
            0.51,
            Stairs {
                tread_m: Some(0.30),
                ..top_edge()
            },
        );
        let m = shape_mesh(&shape).unwrap();
        let v: f32 = m
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                (a[0] * (b[1] * c[2] - b[2] * c[1])
                    + a[1] * (b[2] * c[0] - b[0] * c[2])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum::<f32>()
            .abs();
        // 3 steps 0.3 deep (the top one runs to the back: 0.6 deep), 1 m wide.
        let want = 1.0 * (0.17 * 0.3 + 0.34 * 0.3 + 0.51 * 0.6);
        assert!((v - want).abs() < 1e-3, "{v} vs {want}");
    }

    #[test]
    fn blondel_and_short_footprints_warn() {
        let outline = [[0.0, 0.0], [1.0, 0.0], [1.0, 3.0], [0.0, 3.0]];
        // Steeper than comfortable: 2 x 0.25 + 0.30 = 0.80.
        let steep = Stairs {
            up_edge: 2,
            riser_m: Some(0.25),
            tread_m: Some(0.30),
        };
        let g = steep.geometry(&outline, 0.75).unwrap();
        assert!(
            g.warnings.iter().any(|w| w.contains("Blondel")),
            "{:?}",
            g.warnings
        );
        // Too short for the treads: 6 steps of 0.30 m do not fit 1 m.
        let short = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let g = Stairs {
            tread_m: Some(0.30),
            ..top_edge()
        }
        .geometry(&short, 1.0)
        .unwrap();
        assert!(
            g.warnings.iter().any(|w| w.contains("too short")),
            "{:?}",
            g.warnings
        );
        assert!((g.tread - 1.0 / 6.0).abs() < 1e-5);
        // The warnings reach the mesh.
        let m = shape_mesh(&flight(
            1.0,
            1.0,
            1.0,
            Stairs {
                tread_m: Some(0.30),
                ..top_edge()
            },
        ))
        .unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("too short")));
        // A comfortable flight has none.
        let g = Stairs::new(2).geometry(&[[0.0, 0.0], [1.0, 0.0], [1.0, 0.9], [0.0, 0.9]], 0.51);
        assert!(g.unwrap().warnings.is_empty());
    }

    #[test]
    fn bad_stairs_are_refused_with_a_reason() {
        let rect = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let st = Stairs::new(2);
        assert!(st.geometry(&rect[..3], 0.5).is_err());
        assert!(Stairs::new(4).geometry(&rect, 0.5).is_err());
        assert!(st.geometry(&rect, f32::NAN).is_err());
        assert!(st.geometry(&rect, 0.0).is_err());
        let skew = [[0.0, 0.0], [1.0, 0.0], [1.4, 1.0], [0.0, 1.0]];
        assert!(st.geometry(&skew, 0.5).unwrap_err().contains("rectangle"));
        let bad = Stairs {
            riser_m: Some(0.9),
            ..Stairs::new(2)
        };
        assert!(bad.geometry(&rect, 0.5).unwrap_err().contains("riser_m"));
        // 18 steps in 1 m: no tread left.
        assert!(st.geometry(&rect, 3.0).unwrap_err().contains("too short"));
    }

    fn volume(m: &crate::ImportedModel) -> f32 {
        m.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                (a[0] * (b[1] * c[2] - b[2] * c[1])
                    + a[1] * (b[2] * c[0] - b[0] * c[2])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum::<f32>()
            .abs()
    }

    /// The volume the closed mesh of `g` (from 0) encloses, each triangle turned to face its
    /// stated outward side: a mesh that is not closed or has a wrong side gives another number.
    fn mesh_volume(g: &Geometry, bottoms: &[f32]) -> f32 {
        struct Vol(f32);
        impl Sink for Vol {
            fn tri(&mut self, _: usize, [a, b, c]: [[f32; 3]; 3], outward: [f32; 3]) {
                let det = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
                    a[0] * (b[1] * c[2] - b[2] * c[1])
                        + a[1] * (b[2] * c[0] - b[0] * c[2])
                        + a[2] * (b[0] * c[1] - b[1] * c[0])
                };
                let n = [
                    (b[1] - a[1]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[1] - a[1]),
                    (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]),
                    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]),
                ];
                let flip = n.iter().zip(outward).map(|(x, y)| x * y).sum::<f32>() < 0.0;
                let v = if flip { det(a, c, b) } else { det(a, b, c) };
                self.0 += v / 6.0;
            }
        }
        let mut v = Vol(0.0);
        mesh(&mut v, g, 0.0, bottoms);
        v.0
    }

    #[test]
    fn on_sloping_ground_every_block_goes_down_to_the_ground_under_it() {
        let st = Stairs {
            tread_m: Some(0.30),
            ..top_edge()
        };
        let outline = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.2], [0.0, 1.2]];
        let g = st.geometry(&outline, 0.51).unwrap();
        // The ground falls away 0.2 m per m along the flight, the way a bank runs down past
        // the flight's own footing: the flight stands at 0.
        let fall = |_: f32, z: f32| Some(-0.2 * z);
        let bottoms = step_bottoms(&g, 0.0, 0.0, &fall);
        assert_eq!(bottoms.len(), 3);
        assert!(bottoms.windows(2).all(|w| w[1] < w[0]), "{bottoms:?}");
        // Step 1 covers z 0..0.3 (lowest ground -0.06), step 3 z 0.6..1.2 (-0.24).
        assert!((bottoms[0] + 0.06).abs() < 1e-5 && (bottoms[2] + 0.24).abs() < 1e-5);
        // Level ground, or ground above the flight, changes nothing.
        let level = step_bottoms(&g, 0.0, 0.0, &|_, _| Some(0.0));
        assert_eq!(level, [0.0; 3]);
        assert_eq!(step_bottoms(&g, 0.0, 0.0, &|_, z| Some(z)), [0.0; 3]);
        // The mesh stays closed: its volume is the flat flight's plus the columns under it.
        let v0 = mesh_volume(&g, &[]);
        let extra: f32 = (0..3)
            .map(|k| {
                let u1 = if k == 2 {
                    g.length
                } else {
                    (k + 1) as f32 * g.tread
                };
                1.0 * (u1 - k as f32 * g.tread) * -bottoms[k]
            })
            .sum();
        assert!((mesh_volume(&g, &bottoms) - v0 - extra).abs() < 1e-3);
    }

    #[test]
    fn a_flight_on_a_terrain_with_a_dip_reaches_down_into_it() {
        let mut t = crate::Terrain::flat([-1.0, -1.0], 4.0, 4.0, 0.5, "lawn").unwrap();
        let w = t.width as usize + 1;
        // A trench across the flight at z = 0.5, and the flight's corners and edge midpoints
        // (where its footing is taken) on the flat.
        for (i, h) in t.heights.iter_mut().enumerate() {
            let z = -1.0 + (i / w) as f32 * 0.5;
            if (z - 0.5).abs() < 1e-3 {
                *h = -0.3;
            }
        }
        let shape = flight(
            1.2,
            1.0,
            0.51,
            Stairs {
                tread_m: Some(0.30),
                ..top_edge()
            },
        );
        let flat = shape_mesh(&shape).unwrap();
        let on = crate::shape::shape_mesh_on_terrain(&shape, &t, &[]).unwrap();
        let low =
            |m: &crate::ImportedModel| m.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        assert!(low(&flat).abs() < 1e-6);
        assert!(low(&on) < -0.03, "the flight stops at {}", low(&on));
        assert!(volume(&on) > volume(&flat));
    }
}
