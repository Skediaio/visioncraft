//! Measurements on points and outlines, for the app and for third-party tools. Metres.

fn dist(a: [f32; 3], b: [f32; 3]) -> f64 {
    (0..3)
        .map(|k| f64::from(b[k] - a[k]).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Length of the 3D polyline through `points`.
pub fn polyline_length(points: &[[f32; 3]]) -> f32 {
    points.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>() as f32
}

/// Length of the polyline through `points_xz` as it runs over the ground: sampled every 0.25 m
/// and measured in 3D between samples. Points with no ground count as height 0.
pub fn polyline_length_on_ground(
    points_xz: &[[f32; 2]],
    ground: &dyn Fn(f32, f32) -> Option<f32>,
) -> f32 {
    const STEP: f32 = 0.25;
    let at = |x: f32, z: f32| [x, ground(x, z).unwrap_or(0.0), z];
    let mut total = 0.0;
    for w in points_xz.windows(2) {
        let (a, b) = (w[0], w[1]);
        let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / STEP).ceil().max(1.0) as usize;
        let mut prev = at(a[0], a[1]);
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let p = at(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
            total += dist(prev, p);
            prev = p;
        }
    }
    total as f32
}

/// Area of a simple polygon in m2, whichever way it winds.
pub fn polygon_area(outline: &[[f32; 2]]) -> f32 {
    let twice: f64 = (0..outline.len())
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
            f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1])
        })
        .sum();
    (twice.abs() / 2.0) as f32
}

/// How much higher `b` is than `a` (negative when lower).
pub fn height_difference(a: [f32; 3], b: [f32; 3]) -> f32 {
    b[1] - a[1]
}

/// Rise over run from `a` to `b` in percent (negative downhill); `None` when the two are
/// straight above each other.
pub fn slope_percent(a: [f32; 3], b: [f32; 3]) -> Option<f32> {
    let run = (b[0] - a[0]).hypot(b[2] - a[2]);
    (run > 0.0).then(|| 100.0 * (b[1] - a[1]) / run)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_shapes() {
        assert!(
            (polyline_length(&[[0.0; 3], [3.0, 4.0, 0.0], [3.0, 4.0, 12.0]]) - 17.0).abs() < 1e-5
        );
        assert_eq!(polyline_length(&[[1.0; 3]]), 0.0);
        let rect = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.5], [0.0, 2.5]];
        assert_eq!(polygon_area(&rect), 10.0);
        let rev: Vec<_> = rect.iter().rev().copied().collect();
        assert_eq!(polygon_area(&rev), 10.0);
        assert_eq!(height_difference([0.0, 1.0, 0.0], [5.0, 0.5, 5.0]), -0.5);
        assert_eq!(slope_percent([0.0; 3], [10.0, 1.0, 0.0]), Some(10.0));
        assert_eq!(slope_percent([0.0; 3], [0.0, 1.0, 0.0]), None);
    }

    #[test]
    fn ground_length_follows_a_slope() {
        // A 10 m walk up a 3:4 slope (1.0 rise per 1.0... y = 0.75 x): 12.5 m on the ground.
        let slope = |x: f32, _: f32| Some(0.75 * x);
        let l = polyline_length_on_ground(&[[0.0, 0.0], [10.0, 0.0]], &slope);
        assert!((l - 12.5).abs() < 1e-3, "{l}");
        let flat = polyline_length_on_ground(&[[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]], &|_, _| None);
        assert!((flat - 15.0).abs() < 1e-3);
    }
}
