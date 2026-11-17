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

/// Draws the flight `g` from `lo` up on the faces of [`STAIR_FACES`].
pub(crate) fn mesh(s: &mut dyn Sink, g: &Geometry, lo: f32) {
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
    quad(
        s,
        SIDES,
        [
            p(0.0, -hw, lo),
            p(g.length, -hw, lo),
            p(g.length, hw, lo),
            p(0.0, hw, lo),
        ],
        neg(up),
    );
    quad(
        s,
        SIDES,
        [
            p(g.length, -hw, lo),
            p(g.length, hw, lo),
            p(g.length, hw, hi),
            p(g.length, -hw, hi),
        ],
        d3,
    );
    for k in 1..=g.steps {
        let u0 = (k - 1) as f32 * g.tread;
        let u1 = if k == g.steps {
            g.length
        } else {
            k as f32 * g.tread
        };
        let (y0, y1) = (lo + (k - 1) as f32 * g.riser, lo + k as f32 * g.riser);
        quad(
            s,
            TREADS,
            [p(u0, -hw, y1), p(u1, -hw, y1), p(u1, hw, y1), p(u0, hw, y1)],
            up,
        );
        quad(
            s,
            RISERS,
            [p(u0, -hw, y0), p(u0, hw, y0), p(u0, hw, y1), p(u0, -hw, y1)],
            neg(d3),
        );
        for (w, out) in [(-hw, neg(n3)), (hw, n3)] {
            quad(
                s,
                SIDES,
                [p(u0, w, lo), p(u1, w, lo), p(u1, w, y1), p(u0, w, y1)],
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
            outline: vec![[0.0, 0.0], [width, 0.0], [width, len], [0.0, len]],
            closed: true,
            base: 0.0,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: Some(stairs),
            raised_bed: None,
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
}
