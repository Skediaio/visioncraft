//! Level-of-detail selection per instance cell, with hysteresis.
//!
//! Level `k` covers distances up to `max[k]`; one virtual level past the
//! last (`max.len()`) means "culled" (only reachable when the last distance
//! is finite). A cell keeps its current level while the distance stays
//! within that level's range widened by `HYSTERESIS_M` on both sides, so a
//! camera hovering at a threshold does not make the cell flicker.

pub const HYSTERESIS_M: f32 = 1.0;

/// The level for distance `d`, given the previous one (`None` the first
/// time). Returns `max.len()` when culled.
pub fn select(d: f32, max: &[f32], current: Option<usize>, hysteresis: f32) -> usize {
    let raw = max.iter().position(|&m| d <= m).unwrap_or(max.len());
    let Some(c) = current.filter(|&c| c <= max.len()) else {
        return raw;
    };
    let lo = if c == 0 {
        f32::NEG_INFINITY
    } else {
        max[c - 1]
    };
    let hi = max.get(c).copied().unwrap_or(f32::INFINITY);
    if d >= lo - hysteresis && d <= hi + hysteresis {
        c
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_by_distance() {
        let max = [10.0, 30.0, 80.0];
        assert_eq!(select(5.0, &max, None, 1.0), 0);
        assert_eq!(select(10.0, &max, None, 1.0), 0);
        assert_eq!(select(10.5, &max, None, 1.0), 1);
        assert_eq!(select(50.0, &max, None, 1.0), 2);
        assert_eq!(select(81.0, &max, None, 1.0), 3, "culled past the last");
        assert_eq!(
            select(1e6, &[10.0, f32::INFINITY], None, 1.0),
            1,
            "never culled"
        );
    }

    #[test]
    fn hysteresis_holds_the_level_near_a_threshold() {
        let max = [10.0, 30.0];
        // Walking out: level 0 holds until 11 m, then 1.
        let mut lod = select(9.0, &max, None, 1.0);
        let mut trace = Vec::new();
        for d in [9.5, 10.2, 10.9, 11.2, 10.5, 9.2, 8.9] {
            lod = select(d, &max, Some(lod), 1.0);
            trace.push(lod);
        }
        assert_eq!(trace, [0, 0, 0, 1, 1, 1, 0]);
        // Jitter across the threshold does not flip it.
        let mut lod = select(10.1, &max, None, 1.0);
        for i in 0..100 {
            let d = 10.0 + if i % 2 == 0 { 0.4 } else { -0.4 };
            lod = select(d, &max, Some(lod), 1.0);
            assert_eq!(lod, 1);
        }
        // Culling has hysteresis too, and a big jump goes straight there.
        assert_eq!(select(30.5, &max, Some(2), 1.0), 2);
        assert_eq!(select(5.0, &max, Some(2), 1.0), 0);
    }
}
