//! Frame statistics for a performance overlay, and an automatic quality
//! choice from frame times.

use crate::Quality;

/// Counts of the last frame recorded by `Renderer::render`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderStats {
    /// Draw calls in all passes (shadow, depth, reflection, forward, post).
    pub draw_calls: u32,
    /// `draw_calls` per pass group: shadow (all cascades), depth,
    /// reflection, forward (with sky and transparent), post.
    pub pass_draws: Vec<(&'static str, u32)>,
    /// Triangles submitted per pass group, as `pass_draws`.
    pub pass_triangles: Vec<(&'static str, u64)>,
    /// Triangles submitted in all passes.
    pub triangles: u64,
    /// Instances drawn in the camera's view (after culling and LOD).
    pub instances: u32,
    /// Lights whose range reaches into the camera's view.
    pub visible_lights: u32,
    /// Estimate of the GPU memory held by textures and render targets.
    pub texture_bytes: u64,
    /// `texture_bytes` by kind: renderer textures (materials, plant atlases), shadow maps,
    /// other render targets, environment, terrain, impostor atlases, the sky image.
    pub texture_breakdown: Vec<(&'static str, u64)>,
    /// CPU time spent recording the frame (0 on the web, where there is no
    /// clock without a UI dependency).
    pub cpu_encode_ms: f32,
    /// GPU time per pass group of a frame a few frames back, when the device
    /// was created with `Features::TIMESTAMP_QUERY` (the renderer never asks
    /// for it); empty otherwise.
    pub gpu_passes: Vec<(&'static str, f32)>,
    /// Sum of `gpu_passes`.
    pub gpu_ms: Option<f32>,
    /// The scene's resolution after `render_scale`.
    pub render_size: (u32, u32),
}

/// The frame rate the app's automatic quality aims for unless told otherwise.
pub const DEFAULT_TARGET_FPS: f32 = 60.0;
/// The range of frame rates a person may ask the automatic quality to aim for.
pub const TARGET_FPS_RANGE: std::ops::RangeInclusive<f32> = 24.0..=240.0;

/// The frame-time budget (ms) of a target frame rate: what `suggest_quality` and
/// `starting_quality` take.
pub fn target_ms(fps: f32) -> f32 {
    1000.0 / fps
}

/// Frames needed before a decision.
pub const MIN_HISTORY: usize = 30;

/// The share of the target frame time ULTRA's predicted median may take for HIGH at full scale
/// to step up to it. The prediction is the measured median times `relative_cost(ULTRA) /
/// relative_cost(HIGH)` (6.2 / 3.7 = 1.68, the garden on an Intel UHD 620 measured 1.6), so HIGH
/// rises with a median under 0.65 / 1.68 = 39 % of the budget: 6.5 ms at 60 fps, 12.9 ms at 30.
/// 0.65 is the general rise threshold: ULTRA lands where a rise would still be allowed, a third
/// of the budget spare for heavier views, since leaving it again regenerates atlases and maps.
/// (It was a flat 30 % of the budget on HIGH's median, 5 ms at 60 fps: no GPU reached it once
/// the target moved from 30 to 60 fps.)
pub const ULTRA_HEADROOM: f32 = 0.65;

/// What automatic quality judges one frame by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameCost {
    /// The frame's own work (ms): the GPU time from timestamp queries (`RenderStats::gpu_ms`, a
    /// few frames late) or the CPU time of its update, whichever is longer. Vsync does not bound
    /// it: a GPU with room to spare shows it.
    Work(f32),
    /// The wall-clock interval since the previous frame (ms), without timestamp queries. Vsync
    /// rounds it up to whole refresh intervals, so it is only an upper bound of the frame's cost
    /// and at the refresh interval says nothing about room to spare (see `interval_budget`).
    Interval(f32),
}

impl FrameCost {
    pub fn ms(self) -> f32 {
        match self {
            FrameCost::Work(ms) | FrameCost::Interval(ms) => ms,
        }
    }

    pub fn is_work(self) -> bool {
        matches!(self, FrameCost::Work(_))
    }
}

/// A frame's cost from what was measured: `Work(max(gpu, cpu))` when the GPU time is known
/// (positive and finite), else the wall-clock `Interval`.
pub fn frame_cost(interval_ms: f32, gpu_ms: Option<f32>, cpu_ms: f32) -> FrameCost {
    match gpu_ms {
        Some(gpu) if gpu.is_finite() && gpu > 0.0 => FrameCost::Work(gpu.max(cpu_ms)),
        _ => FrameCost::Interval(interval_ms),
    }
}

/// Display refresh rates (Hz) whose interval `refresh_interval` recognises. None below 60: a
/// steady 30 or 40 fps is far likelier a GPU that misses vsync than a display that slow.
const REFRESH_HZ: [f32; 9] = [60.0, 72.0, 75.0, 90.0, 100.0, 120.0, 144.0, 165.0, 240.0];

/// The display's refresh interval (ms) when wall-clock intervals (oldest first) show one: the
/// 25th percentile of the last `MIN_HISTORY` (the steady minimum: frames that miss a refresh
/// only lengthen the upper ones, and rAF clocks jitter a few percent) within 8 % of a common
/// refresh rate's interval, which is returned. `None` on a short history or when the
/// fastest frames are no refresh interval (a GPU-bound scene).
pub fn refresh_interval(intervals: &[f32]) -> Option<f32> {
    if intervals.len() < MIN_HISTORY {
        return None;
    }
    let mut recent = intervals[intervals.len() - MIN_HISTORY..].to_vec();
    recent.sort_by(f32::total_cmp);
    let low = recent[MIN_HISTORY / 4];
    REFRESH_HZ
        .iter()
        .map(|hz| 1000.0 / hz)
        .find(|ms| (low / ms - 1.0).abs() < 0.08)
}

/// The frame time over which wall-clock intervals miss the target: `target_ms`, but never less
/// than 1.5 refresh intervals when the intervals show one (`refresh_interval`). Vsync rounds an
/// interval to whole refresh intervals, so up to 1.5 is a frame on time plus clock jitter and 2
/// is a missed refresh: a 60 Hz display's 16.7 ms frames meet a 60 fps target (and any faster
/// one, which stepping down could not reach either), 33 ms ones miss it.
pub fn interval_budget(intervals: &[f32], target_ms: f32) -> f32 {
    refresh_interval(intervals).map_or(target_ms, |r| target_ms.max(1.5 * r))
}

/// The quality ladder from cheapest to best: each preset at its own render
/// scale and at up to two coarser steps of 0.1 (not below 0.5); ULTRA only
/// at its own (a coarser ULTRA is worse than HIGH at full scale).
pub(crate) fn ladder() -> Vec<Quality> {
    let mut out = Vec::new();
    for (_, preset) in Quality::PRESETS {
        let steps: &[i32] = if preset == Quality::ULTRA {
            &[0]
        } else {
            &[2, 1, 0]
        };
        for &step in steps {
            let scale = preset.render_scale - 0.1 * step as f32;
            if scale >= 0.5 - 1e-4 {
                out.push(Quality {
                    render_scale: scale,
                    ..preset
                });
            }
        }
    }
    out
}

pub(crate) fn same_except_scale(a: &Quality, b: &Quality) -> bool {
    *a == Quality {
        render_scale: a.render_scale,
        ..*b
    }
}

/// Suggests the next quality from recent frame times (milliseconds, oldest
/// first): one step down the ladder when the median of the last
/// `MIN_HISTORY` frames is over 110 % of `target_ms` (the frame time of the
/// frame rate aimed for, see `target_ms`), one step up when
/// it is under 65 % (a step costs roughly 20-40 %, so the next one fits and
/// the choice does not oscillate), otherwise unchanged. The step from HIGH
/// up to ULTRA needs ULTRA's predicted median (the median scaled by `relative_cost`)
/// under `ULTRA_HEADROOM` of the budget. Medians ignore
/// single hitches. A quality that is no preset only has its `render_scale`
/// moved, by 0.1 within 0.5..=1.0. Pure: the caller decides when to ask
/// (after a change, clear the history so the new setting is measured).
pub fn suggest_quality(frame_ms_history: &[f32], current: Quality, target_ms: f32) -> Quality {
    if frame_ms_history.len() < MIN_HISTORY {
        return current;
    }
    let mut recent: Vec<f32> = frame_ms_history[frame_ms_history.len() - MIN_HISTORY..].to_vec();
    recent.sort_by(f32::total_cmp);
    let median = recent[MIN_HISTORY / 2];
    let direction: i32 = if median > target_ms * 1.1 {
        -1
    } else if median < target_ms * 0.65 {
        1
    } else {
        0
    };
    if direction == 0 {
        return current;
    }
    let ladder = ladder();
    let position = ladder.iter().position(|q| {
        same_except_scale(q, &current) && (q.render_scale - current.render_scale).abs() < 0.05
    });
    let Some(i) = position.or_else(|| {
        // A preset at another scale: the nearest rung of that preset.
        ladder
            .iter()
            .enumerate()
            .filter(|(_, q)| same_except_scale(q, &current))
            .min_by(|a, b| {
                let d = |q: &Quality| (q.render_scale - current.render_scale).abs();
                d(a.1).total_cmp(&d(b.1))
            })
            .map(|(i, _)| i)
    }) else {
        let scale = (current.render_scale + 0.1 * direction as f32).clamp(0.5, 1.0);
        return Quality {
            render_scale: scale,
            ..current
        };
    };
    let j = (i as i32 + direction).clamp(0, ladder.len() as i32 - 1) as usize;
    let cost = |q: &Quality| crate::hardware::relative_cost(q);
    if ladder[j] == Quality::ULTRA
        && ladder[i] != Quality::ULTRA
        && median * cost(&ladder[j]) / cost(&ladder[i]) > target_ms * ULTRA_HEADROOM
    {
        return current;
    }
    ladder[j]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 30 fps budget the older tests were written for.
    const T30: f32 = 1000.0 / 30.0;

    fn suggest_quality(history: &[f32], current: Quality) -> Quality {
        super::suggest_quality(history, current, T30)
    }

    #[test]
    fn the_target_decides_whether_25_ms_frames_are_too_slow() {
        let q25 = frames(25.0);
        // 25 ms is 40 fps: over 110 % of a 60 fps budget (16.7 ms), inside a 30 fps one.
        let down = super::suggest_quality(&q25, Quality::MEDIUM, target_ms(60.0));
        assert_eq!(down.render_scale, Quality::MEDIUM.render_scale - 0.1);
        let held = super::suggest_quality(&q25, Quality::MEDIUM, target_ms(30.0));
        assert_eq!(held, Quality::MEDIUM);

        assert_eq!(target_ms(60.0), 1000.0 / 60.0);
    }

    #[test]
    fn six_ms_of_gpu_work_on_high_rises_to_ultra_at_60_fps() {
        // ULTRA costs relative_cost(ULTRA) / relative_cost(HIGH) = 6.2 / 3.7 = 1.68 times HIGH:
        // about 10 ms, well inside 16.7 ms.
        assert_eq!(
            super::suggest_quality(&frames(6.0), Quality::HIGH, target_ms(60.0)),
            Quality::ULTRA
        );
    }

    #[test]
    fn the_frame_cost_is_the_gpu_time_when_known_else_the_interval() {
        // GPU timed: the work, not the 16.7 ms vsync interval; the CPU's when it is longer.
        assert_eq!(frame_cost(16.7, Some(6.0), 2.0), FrameCost::Work(6.0));
        assert_eq!(frame_cost(16.7, Some(6.0), 9.0), FrameCost::Work(9.0));
        // No timestamps (or no reading yet): the wall interval, judged vsync-aware.
        assert_eq!(frame_cost(16.7, None, 2.0), FrameCost::Interval(16.7));
        assert_eq!(frame_cost(16.7, Some(0.0), 2.0), FrameCost::Interval(16.7));
        assert!(frame_cost(16.7, Some(6.0), 2.0).is_work());
        assert_eq!(frame_cost(16.7, None, 2.0).ms(), 16.7);
    }

    #[test]
    fn intervals_at_the_refresh_rate_meet_a_target_at_or_above_it() {
        let jitter = |ms: f32| -> Vec<f32> {
            (0..40)
                .map(|i| ms * [0.97, 1.03, 1.0, 1.05, 0.98][i % 5])
                .collect()
        };
        let t60 = target_ms(60.0);
        // 60 Hz: the refresh is found and 16.7 ms frames do not miss 60 (or 120) fps.
        let r = refresh_interval(&jitter(1000.0 / 60.0)).expect("60 Hz");
        assert!((r - 1000.0 / 60.0).abs() < 1e-3);
        assert_eq!(interval_budget(&jitter(16.7), t60), 1.5 * r);
        assert_eq!(interval_budget(&jitter(16.7), target_ms(120.0)), 1.5 * r);
        // 144 Hz under a 60 fps target: the target decides.
        assert!(refresh_interval(&jitter(1000.0 / 144.0)).is_some());
        assert_eq!(interval_budget(&jitter(6.94), t60), t60);
        // Every other refresh missed (16.7 / 33.3 ms): still 60 Hz, and the slow ones miss.
        let halves: Vec<f32> = (0..40)
            .map(|i| if i % 2 == 0 { 16.7 } else { 33.3 })
            .collect();
        assert!(refresh_interval(&halves).is_some());
        assert!(33.3 > interval_budget(&halves, t60));
        // A steady GPU-bound 40 fps is no refresh rate; a short history shows none.
        assert_eq!(refresh_interval(&jitter(25.0)), None);
        assert_eq!(interval_budget(&jitter(25.0), t60), t60);
        assert_eq!(refresh_interval(&[16.7; 10]), None);
    }

    #[test]
    fn the_default_target_is_inside_the_allowed_range() {
        assert!(TARGET_FPS_RANGE.contains(&DEFAULT_TARGET_FPS));
    }

    fn frames(ms: f32) -> Vec<f32> {
        vec![ms; 40]
    }

    #[test]
    fn steps_down_when_slow_and_up_with_headroom() {
        let slow = suggest_quality(&frames(45.0), Quality::MEDIUM);
        assert_eq!(slow.render_scale, Quality::MEDIUM.render_scale - 0.1);
        assert!(same_except_scale(&slow, &Quality::MEDIUM));
        // Keep going down: through MEDIUM's coarser scales into LOW.
        let mut q = Quality::MEDIUM;
        for _ in 0..10 {
            q = suggest_quality(&frames(60.0), q);
        }
        assert!(same_except_scale(&q, &Quality::LOW), "{q:?}");
        assert!(q.render_scale >= 0.5 - 1e-4);
        // Fast frames climb back up to HIGH at full scale and stay there.
        for _ in 0..20 {
            q = suggest_quality(&frames(15.0), q);
        }
        assert_eq!(q, Quality::HIGH);
        // Far faster (over 100 fps) steps up to ULTRA, and ULTRA too slow
        // steps back to HIGH at full scale.
        assert_eq!(suggest_quality(&frames(6.0), Quality::HIGH), Quality::ULTRA);
        assert_eq!(
            suggest_quality(&frames(6.0), Quality::ULTRA),
            Quality::ULTRA
        );
        assert_eq!(
            suggest_quality(&frames(45.0), Quality::ULTRA),
            Quality::HIGH
        );
    }

    #[test]
    fn holds_inside_the_hysteresis_band_and_on_short_history() {
        for ms in [24.0, 30.0, 36.0] {
            assert_eq!(
                suggest_quality(&frames(ms), Quality::MEDIUM),
                Quality::MEDIUM
            );
        }
        assert_eq!(suggest_quality(&[90.0; 5], Quality::HIGH), Quality::HIGH);
        // One long hitch among fast frames does not step down.
        let mut h = frames(25.0);
        h[35] = 400.0;
        assert_eq!(suggest_quality(&h, Quality::MEDIUM), Quality::MEDIUM);
    }

    #[test]
    fn custom_quality_only_moves_the_scale() {
        let custom = Quality {
            shadow_resolution: 512,
            render_scale: 0.8,
            ..Quality::LOW
        };
        let down = suggest_quality(&frames(50.0), custom);
        assert!((down.render_scale - 0.7).abs() < 1e-5);
        assert_eq!(down.shadow_resolution, 512);
        let up = suggest_quality(
            &frames(5.0),
            Quality {
                render_scale: 0.95,
                ..custom
            },
        );
        assert_eq!(up.render_scale, 1.0);
    }
}
