//! Several water surfaces in one frame, each reflecting its surroundings as well as the
//! quality's budget allows (`Quality::water_planes`, lib.rs step 5). The ranking is plain
//! arithmetic on plain data: no device, so it is tested without a GPU.
//!
//! Each frame the visible water surfaces (the parts with a water material, the sea) are
//! gathered as [`Surface`]s and ranked:
//!
//! - **Planes**: sorted by height, a surface joins the plane being built while it is within
//!   [`GROUP_M`] of the plane's height; so two areas at one level share one plane. The shaders
//!   match a surface to a plane within 5 cm (scene.wgsl `water`, sea.wgsl), a centimetre more,
//!   so every surface of a plane is well inside its window.
//! - **Score**: the share of the render target the plane covers (its rectangle's area; for the
//!   sea the share of the rays that met it) times 1 + nearness (1 at the eye, 0.5 at 10 m,
//!   towards 0 far off). A near small pond outranks a far lake of its size on screen; a lake
//!   several times larger still wins.
//! - **Tiers**: the best `budget` planes get a mirrored pass each ([`WaterTier::Planar`]), the
//!   rest reflect the opaque scene of this frame ([`WaterTier::ScreenSpace`]) when screen-space
//!   reflections are on, else the environment cube ([`WaterTier::Environment`]).
//! - **Hysteresis**: a plane that had a pass keeps it over a plane scoring less than
//!   [`KEEP_RATIO`] times its own, and gives it up only after [`KEEP_FRAMES`] consecutive
//!   frames of such a challenger: two ponds close in score do not swap while walking.

use glam::{Mat4, Vec3};

/// The planar reflection's layers in the globals (`reflection_planes`); `Quality::water_planes`
/// is clamped to it. Far under WebGPU's default `max_texture_array_layers` (256), so the array
/// target needs no limit of its own.
pub(crate) const MAX_PLANES: u32 = 4;
/// A surface joins the plane being built within this distance of the plane's height (m).
pub(crate) const GROUP_M: f32 = 0.04;
/// Nearness halves at this distance (m).
const NEAR_M: f32 = 10.0;
/// A challenger must score this times an incumbent's score to take its planar pass.
pub(crate) const KEEP_RATIO: f32 = 1.3;
/// ... for this many consecutive frames.
pub(crate) const KEEP_FRAMES: u32 = 10;
/// The ripple distortion's lookup offset in a pond (`0.6 / depth` in scene.wgsl `water`) and
/// in the sea (`1.2 / depth`).
pub(crate) const POND_DISTORTION: f32 = 0.6;
pub(crate) const SEA_DISTORTION: f32 = 1.2;
/// The most the ripples tilt a pond's or the sea's normal (`n.xz` after normalisation): the
/// wind waves stay under 0.41 (measured over 2.25 million points of scene.wgsl `wave_slope`
/// at the strongest calm), the rings of rain go further, so there 1.
pub(crate) const WIND_TILT: f32 = 0.5;
pub(crate) const RAIN_TILT: f32 = 1.0;

/// How a water surface is reflected this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterTier {
    /// A mirrored pass of the scene about the surface's plane.
    Planar,
    /// A march through the opaque scene of this frame, the environment cube where it misses.
    ScreenSpace,
    /// The environment cube, darkened towards the horizon.
    Environment,
}

impl WaterTier {
    pub fn name(self) -> &'static str {
        match self {
            WaterTier::Planar => "planar",
            WaterTier::ScreenSpace => "screen_space",
            WaterTier::Environment => "environment",
        }
    }
}

/// A rectangle of the render target, in pixels from its top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// One visible water surface as the ranking sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Surface {
    /// Height of the surface (m).
    pub height: f32,
    /// Where it shows on the render target, padded for the ripple distortion.
    pub rect: Rect,
    /// The share of the render target it covers, 0..1.
    pub coverage: f32,
    /// From the eye to the nearest of its visible bounds (m).
    pub distance: f32,
}

/// The visible water surfaces at one height: one plane for the planar reflection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterPlane {
    /// The height its largest surface lies at; every surface of the plane is within
    /// [`GROUP_M`] of it.
    pub height: f32,
    /// The union of its surfaces' rectangles.
    pub rect: Rect,
    /// The share of the render target its surfaces cover together (at most 1).
    pub coverage: f32,
    /// From the eye to the nearest of its surfaces (m).
    pub distance: f32,
    /// `coverage` times 1 + [`nearness`] of `distance`: what a planar pass is worth.
    pub score: f32,
}

/// The planes and tiers of the frames so far: the state of the hysteresis, and the buffers
/// every frame reuses (nothing is allocated once they have grown).
#[derive(Default)]
pub(crate) struct Ranking {
    /// This frame's planes, by height.
    pub planes: Vec<WaterPlane>,
    /// Per surface of the frame: its plane's index in `planes`.
    owner: Vec<u16>,
    /// Per plane: its tier.
    pub tiers: Vec<WaterTier>,
    /// Last frame's planar planes by key, and for how many consecutive frames a challenger
    /// scored [`KEEP_RATIO`] times their score.
    held: Vec<(i32, u32)>,
    order: Vec<usize>,
    chosen: Vec<bool>,
    streaks: Vec<(i32, u32)>,
}

impl Rect {
    pub fn whole((w, h): (u32, u32)) -> Rect {
        Rect { x: 0, y: 0, w, h }
    }

    fn empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn union(self, other: Rect) -> Rect {
        if self.empty() {
            return other;
        }
        if other.empty() {
            return self;
        }
        let (x0, y0) = (self.x.min(other.x), self.y.min(other.y));
        let (x1, y1) = (
            (self.x + self.w).max(other.x + other.w),
            (self.y + self.h).max(other.y + other.h),
        );
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    pub fn area(self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    /// Grown by `(px, py)` on every side, and cut to the target `(w, h)`.
    pub fn padded(self, (px, py): (u32, u32), (w, h): (u32, u32)) -> Rect {
        if self.empty() {
            return Rect::default();
        }
        let x0 = self.x.saturating_sub(px);
        let y0 = self.y.saturating_sub(py);
        let x1 = (self.x + self.w).saturating_add(px).min(w);
        let y1 = (self.y + self.h).saturating_add(py).min(h);
        if x1 <= x0 || y1 <= y0 {
            return Rect::default();
        }
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

/// 1 at the eye, 0.5 at 10 m, towards 0 far off.
pub(crate) fn nearness(distance: f32) -> f32 {
    1.0 / (1.0 + distance.max(0.0) / NEAR_M)
}

impl Ranking {
    /// Ranks this frame's `surfaces`: `planes` by height, `tiers` per plane, and `owner` per
    /// surface. Nothing is allocated once the buffers have grown.
    pub fn rank(&mut self, surfaces: &[Surface], budget: u32, screen_space: bool) {
        self.planes.clear();
        self.owner.clear();
        self.owner.resize(surfaces.len(), 0);
        self.order.clear();
        self.order.extend(0..surfaces.len());
        self.order.sort_by(|&a, &b| {
            surfaces[a]
                .height
                .total_cmp(&surfaces[b].height)
                .then(a.cmp(&b))
        });
        for &s in &self.order {
            let surface = surfaces[s];
            let joins = self
                .planes
                .last()
                .is_some_and(|p| surface.height.is_finite() && surface.height - p.height < GROUP_M);
            if !joins {
                self.planes.push(WaterPlane {
                    height: surface.height,
                    rect: Rect::default(),
                    coverage: 0.0,
                    distance: f32::INFINITY,
                    score: 0.0,
                });
            }
            let i = self.planes.len() - 1;
            let plane = &mut self.planes[i];
            plane.rect = plane.rect.union(surface.rect);
            plane.coverage = (plane.coverage + surface.coverage).min(1.0);
            plane.distance = plane.distance.min(surface.distance);
            self.owner[s] = i as u16;
        }
        // The plane lies where its largest surface does (the height it shows from frame to
        // frame); its members lie within GROUP_M of the lowest, so of it too.
        for p in &mut self.planes {
            p.score = f32::NEG_INFINITY;
        }
        for &s in &self.order {
            let i = self.owner[s] as usize;
            if surfaces[s].coverage > self.planes[i].score {
                self.planes[i].score = surfaces[s].coverage;
                self.planes[i].height = surfaces[s].height;
            }
        }
        for p in &mut self.planes {
            p.score = p.coverage * (1.0 + nearness(p.distance));
        }
        self.assign(self.planes.len(), budget, screen_space);
    }

    /// The plane of the last [`Ranking::rank`] the surface `s` belongs to.
    pub fn plane_of(&self, s: usize) -> usize {
        self.owner[s] as usize
    }

    /// How the surface `s` is reflected after the last [`Ranking::rank`].
    pub fn tier_of(&self, s: usize) -> WaterTier {
        self.tiers[self.owner[s] as usize]
    }

    /// The layer of the planar reflection the plane `i` draws into (`None`: no pass).
    pub fn layer_of(&self, i: usize) -> Option<u32> {
        (self.tiers[i] == WaterTier::Planar).then(|| {
            self.tiers[..i]
                .iter()
                .filter(|t| **t == WaterTier::Planar)
                .count() as u32
        })
    }

    /// The best `budget` planes get a pass. An incumbent outside that selection keeps its pass
    /// over the weakest newcomer until that one scores [`KEEP_RATIO`] times its own for
    /// [`KEEP_FRAMES`] consecutive frames.
    fn assign(&mut self, n: usize, budget: u32, screen_space: bool) {
        let fallback = if screen_space {
            WaterTier::ScreenSpace
        } else {
            WaterTier::Environment
        };
        let Ranking {
            planes,
            tiers,
            held,
            order,
            chosen,
            streaks,
            ..
        } = self;
        tiers.clear();
        tiers.resize(n, fallback);
        chosen.clear();
        chosen.resize(n, false);
        let take = (budget as usize).min(n);
        // Best first: by score, then by plane (the order of `planes`: by height).
        order.clear();
        order.extend(0..n);
        order.sort_by(|&a, &b| planes[b].score.total_cmp(&planes[a].score).then(a.cmp(&b)));
        for &i in &order[..take] {
            chosen[i] = true;
        }
        let incumbent = |i: usize| held.iter().find(|(k, _)| *k == key(planes[i].height));
        streaks.clear();
        if take > 0 {
            for &i in &order[take..] {
                let Some(streak) = incumbent(i).map(|(_, s)| *s) else {
                    continue;
                };
                // The weakest chosen plane that kept no pass last frame: the one `i` displaces.
                let weak = (0..n)
                    .filter(|&j| chosen[j] && incumbent(j).is_none())
                    .min_by(|&a, &b| planes[a].score.total_cmp(&planes[b].score));
                let Some(w) = weak else {
                    continue;
                };
                let clearly_better = planes[w].score >= KEEP_RATIO * planes[i].score;
                let streak = if clearly_better { streak + 1 } else { 0 };
                streaks.push((key(planes[i].height), streak));
                if streak >= KEEP_FRAMES {
                    continue;
                }
                chosen[i] = true;
                chosen[w] = false;
            }
        }
        for (t, c) in tiers.iter_mut().zip(chosen.iter()) {
            if *c {
                *t = WaterTier::Planar;
            }
        }
        // Next frame's incumbents: the planes chosen now; a streak only lasts while its
        // challenger is clearly better frame after frame.
        held.retain(|(k, _)| {
            chosen
                .iter()
                .enumerate()
                .any(|(i, c)| *c && key(planes[i].height) == *k)
        });
        for (_, streak) in held.iter_mut() {
            *streak = 0;
        }
        for (k, streak) in streaks.drain(..) {
            if let Some(h) = held.iter_mut().find(|(hk, _)| *hk == k) {
                h.1 = streak;
            }
        }
        for i in 0..n {
            let k = key(planes[i].height);
            if chosen[i] && held.iter().all(|(hk, _)| *hk != k) {
                held.push((k, 0));
            }
        }
    }
}

/// The planes of two frames are the same when their heights round to the same centimetre.
fn key(height: f32) -> i32 {
    (height * 100.0).round() as i32
}

/// Where the box shows on a render target of `size` (pixels from its top left): the whole
/// target when a corner is behind or beside the eye.
pub(crate) fn screen_rect(view_proj: Mat4, (lo, hi): (Vec3, Vec3), size: (u32, u32)) -> Rect {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let mut rect = Rect::default();
    for i in 0..8 {
        let c = Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        let p = view_proj * c.extend(1.0);
        if p.w.is_nan() || p.w <= 1e-4 {
            return Rect::whole(size);
        }
        let x = (p.x / p.w * 0.5 + 0.5) * w;
        let y = (0.5 - p.y / p.w * 0.5) * h;
        if !(x.is_finite() && y.is_finite()) {
            return Rect::whole(size);
        }
        let x0 = x.floor().clamp(0.0, w) as u32;
        let y0 = y.floor().clamp(0.0, h) as u32;
        let x1 = (x.ceil().clamp(0.0, w) as u32).max(x0 + 1).min(size.0);
        let y1 = (y.ceil().clamp(0.0, h) as u32).max(y0 + 1).min(size.1);
        rect = rect.union(Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        });
    }
    rect
}

/// How far the ripples can shift a surface's lookup, in pixels, on a target of `size`: the
/// distortion times the normal's most tilt, over the depth (a metre at the least, as the
/// shaders take it), rounded to a pixel, and two pixels more for the filter's reach.
pub(crate) fn ripple_pad(
    distance: f32,
    distortion: f32,
    tilt: f32,
    size: (u32, u32),
) -> (u32, u32) {
    let share = tilt * distortion / distance.max(1.0);
    let px = (share * size.0 as f32 + 0.5).floor() as u32 + 2;
    let py = (share * size.1 as f32 + 0.5).floor() as u32 + 2;
    (px, py)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (480, 270);

    fn surface(height: f32, coverage: f32, distance: f32) -> Surface {
        // A rectangle of the given share of the target, placed by its coverage and distance
        // (so no two of the test's surfaces share a rectangle).
        let w = (coverage.sqrt() * SIZE.0 as f32).round() as u32;
        let h = w * SIZE.1 / SIZE.0;
        Surface {
            height,
            rect: Rect {
                x: (10.0 + coverage * 700.0) as u32,
                y: (20.0 + distance) as u32,
                w,
                h,
            },
            coverage,
            distance,
        }
    }

    fn rank(r: &mut Ranking, surfaces: &[Surface], budget: u32, ssr: bool) -> Vec<WaterTier> {
        r.rank(surfaces, budget, ssr);
        r.tiers.clone()
    }

    #[test]
    fn two_parts_2_cm_apart_share_a_plane_and_10_cm_do_not() {
        let mut r = Ranking::default();
        r.rank(&[surface(0.0, 0.1, 3.0), surface(0.02, 0.1, 3.0)], 4, true);
        assert_eq!(r.planes.len(), 1, "{:?}", r.planes);
        assert_eq!(r.planes[0].height, 0.0);
        assert_eq!(r.planes[0].coverage, 0.2);
        assert_eq!((r.plane_of(0), r.plane_of(1)), (0, 0));
        // The higher one first in the list: the order of the list does not matter.
        r.rank(&[surface(0.1, 0.1, 3.0), surface(0.0, 0.1, 3.0)], 4, true);
        assert_eq!(r.planes.len(), 2, "{:?}", r.planes);
        assert_eq!((r.plane_of(0), r.plane_of(1)), (1, 0));
        assert_eq!((r.planes[0].height, r.planes[1].height), (0.0, 0.1));
        // The higher surface sets a new plane when it is more than GROUP_M above.
        r.rank(
            &[
                surface(0.0, 0.1, 3.0),
                surface(0.039, 0.1, 3.0),
                surface(0.078, 0.1, 3.0),
            ],
            4,
            true,
        );
        assert_eq!(r.planes.len(), 2, "{:?}", r.planes);
        assert_eq!((r.plane_of(0), r.plane_of(1), r.plane_of(2)), (0, 0, 1));
    }

    #[test]
    fn twelve_surfaces_at_stepped_levels_with_a_pair_at_one_level() {
        let mut r = Ranking::default();
        let mut surfaces: Vec<Surface> = (0..11)
            .map(|i| surface(i as f32 * 0.4, 0.05, 5.0))
            .collect();
        // A second area at 2.0 m, further off.
        surfaces.push(surface(2.0, 0.03, 40.0));
        r.rank(&surfaces, 3, true);
        assert_eq!(r.planes.len(), 11, "{:?}", r.planes.len());
        let pair = surfaces
            .iter()
            .position(|s| s.height == 2.0)
            .expect("the first at 2.0");
        let other = surfaces
            .iter()
            .rposition(|s| s.height == 2.0)
            .expect("the second at 2.0");
        assert_eq!(r.plane_of(pair), r.plane_of(other));
        let plane = r.planes[r.plane_of(pair)];
        assert_eq!(plane.height, 2.0);
        assert!((plane.coverage - 0.08).abs() < 1e-6, "{}", plane.coverage);
        assert!((plane.distance - 5.0).abs() < 1e-6, "{}", plane.distance);
        // The union of both rectangles: the second lies elsewhere, so the union is not either.
        assert!(plane.rect.area() > surfaces[pair].rect.area());
        // Each stepped level is its own plane; the pair covers most and ranks first.
        let best = r
            .planes
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.score.total_cmp(&b.1.score))
            .expect("planes")
            .0;
        assert_eq!(r.planes[best].height, 2.0);
        assert!(
            r.planes
                .iter()
                .all(|p| p.score.is_finite() && p.coverage > 0.0)
        );
    }

    #[test]
    fn a_near_small_pond_outranks_a_far_lake_of_its_size_on_screen() {
        let mut r = Ranking::default();
        let pond = surface(0.0, 0.12, 2.0);
        let lake = surface(1.0, 0.12, 300.0);
        r.rank(&[pond, lake], 1, true);
        assert_eq!(r.planes.len(), 2);
        assert!(r.planes[0].score > r.planes[1].score, "{:?}", r.planes);
        assert_eq!(r.tiers, [WaterTier::Planar, WaterTier::ScreenSpace]);
        // A lake five times larger on screen still wins, far off as it is.
        let big = surface(1.0, 0.6, 300.0);
        r.rank(&[pond, big], 1, true);
        assert!(r.planes[1].score > r.planes[0].score, "{:?}", r.planes);
        assert!((nearness(0.0) - 1.0).abs() < 1e-6);
        assert!((nearness(10.0) - 0.5).abs() < 1e-6);
        assert!(nearness(1000.0) < 0.01);
    }

    #[test]
    fn the_budget_gives_the_best_planes_a_pass_and_the_rest_fall_back() {
        let mut r = Ranking::default();
        // Five planes; the higher the score, the nearer.
        let surfaces: Vec<Surface> = (0..5)
            .map(|i| surface(i as f32 * 0.5, 0.05, 40.0 - i as f32 * 5.0))
            .collect();
        assert_eq!(
            rank(&mut r, &surfaces, 2, true),
            [
                WaterTier::ScreenSpace,
                WaterTier::ScreenSpace,
                WaterTier::ScreenSpace,
                WaterTier::Planar,
                WaterTier::Planar
            ]
        );
        // Layers follow the planes' order (by height), not their scores.
        assert_eq!(r.layer_of(3), Some(0));
        assert_eq!(r.layer_of(4), Some(1));
        assert_eq!(r.layer_of(0), None);
        // Without screen-space reflections the rest take the environment cube.
        assert_eq!(
            rank(&mut r, &surfaces, 2, false)[..3],
            [WaterTier::Environment; 3]
        );
        // A budget over the number of planes, and none.
        assert!(
            rank(&mut r, &surfaces, 9, true)
                .iter()
                .all(|t| *t == WaterTier::Planar)
        );
        assert_eq!(r.layer_of(0), Some(0));
        assert!(
            rank(&mut r, &surfaces, 0, true)
                .iter()
                .all(|t| *t == WaterTier::ScreenSpace)
        );
        assert!(rank(&mut r, &[], 2, true).is_empty());
        assert!(r.planes.is_empty());
    }

    #[test]
    fn a_plane_keeps_its_pass_until_a_challenger_scores_1_3_times_its_own_for_10_frames() {
        let mut r = Ranking::default();
        let (a, b) = (surface(0.0, 0.2, 5.0), surface(1.5, 0.1, 5.0));
        assert_eq!(
            rank(&mut r, &[a, b], 1, true),
            [WaterTier::Planar, WaterTier::ScreenSpace]
        );
        // A challenger scoring 1.2x: the incumbent keeps its pass, frame after frame.
        let slightly = surface(1.5, 0.24, 5.0);
        for _ in 0..200 {
            assert_eq!(
                rank(&mut r, &[a, slightly], 1, true),
                [WaterTier::Planar, WaterTier::ScreenSpace]
            );
        }
        // 1.5x: nine frames keep, the tenth consecutive frame swaps.
        let clearly = surface(1.5, 0.3, 5.0);
        for frame in 1..10 {
            assert_eq!(
                rank(&mut r, &[a, clearly], 1, true),
                [WaterTier::Planar, WaterTier::ScreenSpace],
                "frame {frame}"
            );
        }
        assert_eq!(
            rank(&mut r, &[a, clearly], 1, true),
            [WaterTier::ScreenSpace, WaterTier::Planar]
        );
        // The new incumbent keeps its pass over a challenger under the ratio at once...
        assert_eq!(
            rank(&mut r, &[a, slightly], 1, true),
            [WaterTier::ScreenSpace, WaterTier::Planar]
        );
        // ...and the old one has to score 1.3x its own for ten frames to come back.
        for frame in 1..10 {
            assert_eq!(
                rank(&mut r, &[surface(0.0, 0.39, 5.0), slightly], 1, true),
                [WaterTier::ScreenSpace, WaterTier::Planar],
                "frame {frame}"
            );
        }
        assert_eq!(
            rank(&mut r, &[surface(0.0, 0.39, 5.0), slightly], 1, true),
            [WaterTier::Planar, WaterTier::ScreenSpace]
        );
        // One frame under the ratio resets the count.
        r = Ranking::default();
        rank(&mut r, &[a, b], 1, true);
        for _ in 0..9 {
            rank(&mut r, &[a, clearly], 1, true);
        }
        rank(&mut r, &[a, surface(1.5, 0.25, 5.0)], 1, true);
        for frame in 1..10 {
            assert_eq!(
                rank(&mut r, &[a, clearly], 1, true)[0],
                WaterTier::Planar,
                "frame {frame} after the reset"
            );
        }
        assert_eq!(
            rank(&mut r, &[a, clearly], 1, true)[0],
            WaterTier::ScreenSpace
        );
        // A plane that left the view is forgotten: back at its old score it starts over.
        r = Ranking::default();
        for _ in 0..5 {
            rank(&mut r, &[a, clearly], 1, true);
        }
        rank(&mut r, &[a], 1, true);
        for _ in 0..9 {
            assert_eq!(rank(&mut r, &[a, clearly], 1, true)[0], WaterTier::Planar);
        }
        assert_eq!(
            rank(&mut r, &[a, clearly], 1, true)[0],
            WaterTier::ScreenSpace
        );
    }

    #[test]
    fn the_sea_counts_as_a_plane() {
        let mut r = Ranking::default();
        // The sea at 0 m covering a third of the view (its rays), a pond at 1.5 m.
        let (sea, pond) = (surface(0.0, 0.3, 8.0), surface(1.5, 0.1, 3.0));
        assert_eq!(
            rank(&mut r, &[sea, pond], 1, true),
            [WaterTier::Planar, WaterTier::ScreenSpace]
        );
        assert_eq!(r.layer_of(0), Some(0));
        assert_eq!(
            rank(&mut r, &[sea, pond], 2, true),
            [WaterTier::Planar, WaterTier::Planar]
        );
        // Two ponds at one level and the sea: three surfaces, two planes, and both ponds
        // report their shared plane's tier.
        let twin = surface(1.5, 0.05, 20.0);
        rank(&mut r, &[twin, sea, pond], 2, false);
        assert_eq!(r.planes.len(), 2);
        assert_eq!((r.plane_of(0), r.plane_of(1), r.plane_of(2)), (1, 0, 1));
        assert_eq!(r.tiers, [WaterTier::Planar, WaterTier::Planar]);
        assert_eq!(
            (r.tier_of(0), r.tier_of(2)),
            (WaterTier::Planar, WaterTier::Planar)
        );
        rank(&mut r, &[twin, sea, pond], 1, false);
        assert_eq!(r.tiers, [WaterTier::Planar, WaterTier::Environment]);
        assert_eq!(
            (r.tier_of(0), r.tier_of(1), r.tier_of(2)),
            (
                WaterTier::Environment,
                WaterTier::Planar,
                WaterTier::Environment
            )
        );
    }

    #[test]
    fn the_screen_rect_holds_the_box_and_the_whole_target_behind_the_eye() {
        let cam = Mat4::look_at_rh(Vec3::new(0.0, 1.6, 7.5), Vec3::ZERO, Vec3::Y);
        let proj = Mat4::perspective_rh(50f32.to_radians(), 16.0 / 9.0, 0.1, 1000.0);
        let view_proj = proj * cam;
        // A 1 m cube on the ground straight ahead.
        let rect = screen_rect(
            view_proj,
            (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5)),
            SIZE,
        );
        assert!(rect.x > 180 && rect.x + rect.w < 300, "{rect:?}");
        assert!(rect.y > 60 && rect.y + rect.h < 170, "{rect:?}");
        // Every corner of the box lands inside, projected one by one.
        for i in 0..8 {
            let c = Vec3::new(
                if i & 1 == 0 { -0.5 } else { 0.5 },
                if i & 2 == 0 { 0.0 } else { 1.0 },
                if i & 4 == 0 { -0.5 } else { 0.5 },
            );
            let h = view_proj * c.extend(1.0);
            let (px, py) = (
                (h.x / h.w * 0.5 + 0.5) * SIZE.0 as f32,
                (0.5 - h.y / h.w * 0.5) * SIZE.1 as f32,
            );
            assert!(
                px >= rect.x as f32 - 1.0 && px <= (rect.x + rect.w) as f32 + 1.0,
                "corner {i}: {px} vs {rect:?}"
            );
            assert!(
                py >= rect.y as f32 - 1.0 && py <= (rect.y + rect.h) as f32 + 1.0,
                "corner {i}: {py} vs {rect:?}"
            );
        }
        // A box reaching behind the eye covers the whole target.
        let behind = screen_rect(
            view_proj,
            (Vec3::new(-1.0, 0.0, 6.0), Vec3::new(1.0, 1.0, 9.0)),
            SIZE,
        );
        assert_eq!(behind, Rect::whole(SIZE));
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            (c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5)
        };
        let (x0, y0) = project(Vec3::new(-1.0, 0.0, 6.0));
        assert!(
            x0 < 0.0 || y0 < 0.0 || x0 > 1.0 || y0 > 1.0,
            "out of frame: {x0} {y0}"
        );
    }

    #[test]
    fn the_ripple_pad_grows_near_and_is_cut_to_the_target() {
        // A pond 3 m off: the wind tilt times 0.6 over the depth, and two pixels more.
        let (px, py) = ripple_pad(3.0, POND_DISTORTION, WIND_TILT, SIZE);
        assert_eq!((px, py), (50, 29));
        // The sea's distortion is twice a pond's.
        assert_eq!(ripple_pad(3.0, SEA_DISTORTION, WIND_TILT, SIZE), (98, 56));
        // Rain's rings tilt further still.
        assert_eq!(ripple_pad(3.0, POND_DISTORTION, RAIN_TILT, SIZE), (98, 56));
        // The depth never counts below a metre (the shaders' `max(depth, 1)`).
        assert_eq!(ripple_pad(0.0, POND_DISTORTION, WIND_TILT, SIZE), (146, 83));
        assert_eq!(ripple_pad(1.0, POND_DISTORTION, WIND_TILT, SIZE), (146, 83));
        assert_eq!(
            ripple_pad(0.0, POND_DISTORTION, RAIN_TILT, SIZE),
            (290, 164)
        );
        let r = Rect {
            x: 100,
            y: 50,
            w: 20,
            h: 10,
        };
        assert_eq!(
            r.padded((5, 5), SIZE),
            Rect {
                x: 95,
                y: 45,
                w: 30,
                h: 20
            }
        );
        assert_eq!(
            r.padded((200, 200), SIZE),
            Rect {
                x: 0,
                y: 0,
                w: 320,
                h: 260
            }
        );
        assert_eq!(r.padded((500, 500), SIZE), Rect::whole(SIZE));
        assert_eq!(Rect::default().padded((5, 5), SIZE), Rect::default());
        assert_eq!(Rect::default().union(r), r);
        assert_eq!(r.union(Rect::default()), r);
        assert_eq!(
            r.union(Rect {
                x: 90,
                y: 60,
                w: 10,
                h: 10
            }),
            Rect {
                x: 90,
                y: 50,
                w: 30,
                h: 20
            }
        );
    }
}
