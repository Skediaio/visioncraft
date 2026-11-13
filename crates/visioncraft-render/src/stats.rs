//! Frame statistics for a performance overlay, and an automatic quality
//! choice from frame times.

use crate::Quality;

/// Counts of the last frame recorded by `Renderer::render`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderStats {
    /// Draw calls in all passes (shadow, depth, reflection, forward, post).
    pub draw_calls: u32,
    /// Triangles submitted in all passes.
    pub triangles: u64,
    /// Instances drawn in the camera's view (after culling and LOD).
    pub instances: u32,
    /// Lights whose range reaches into the camera's view.
    pub visible_lights: u32,
    /// Estimate of the GPU memory held by textures and render targets.
    pub texture_bytes: u64,
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

/// Frame-time budget the automatic choice aims for (30 fps).
pub const TARGET_MS: f32 = 1000.0 / 30.0;
/// Frames needed before a decision.
pub const MIN_HISTORY: usize = 30;

/// The quality ladder from cheapest to best: each preset at its own render
/// scale and at up to two coarser steps of 0.1 (not below 0.5).
fn ladder() -> Vec<Quality> {
    let mut out = Vec::new();
    for (_, preset) in Quality::PRESETS {
        for step in [2, 1, 0] {
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

fn same_except_scale(a: &Quality, b: &Quality) -> bool {
    *a == Quality {
        render_scale: a.render_scale,
        ..*b
    }
}

/// Suggests the next quality from recent frame times (milliseconds, oldest
/// first): one step down the ladder when the median of the last
/// `MIN_HISTORY` frames is over 110 % of the 30 fps budget, one step up when
/// it is under 65 % (a step costs roughly 20-40 %, so the next one fits and
/// the choice does not oscillate), otherwise unchanged. Medians ignore
/// single hitches. A quality that is no preset only has its `render_scale`
/// moved, by 0.1 within 0.5..=1.0. Pure: the caller decides when to ask
/// (after a change, clear the history so the new setting is measured).
pub fn suggest_quality(frame_ms_history: &[f32], current: Quality) -> Quality {
    if frame_ms_history.len() < MIN_HISTORY {
        return current;
    }
    let mut recent: Vec<f32> = frame_ms_history[frame_ms_history.len() - MIN_HISTORY..].to_vec();
    recent.sort_by(f32::total_cmp);
    let median = recent[MIN_HISTORY / 2];
    let direction: i32 = if median > TARGET_MS * 1.1 {
        -1
    } else if median < TARGET_MS * 0.65 {
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
    ladder[j]
}

#[cfg(test)]
mod tests {
    use super::*;

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
            q = suggest_quality(&frames(10.0), q);
        }
        assert_eq!(q, Quality::HIGH);
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
