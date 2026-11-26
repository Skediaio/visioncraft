//! Several water surfaces in one frame, each reflecting its surroundings as well as the
//! quality's budget allows (`Quality::water_planes`, lib.rs step 5). The ranking is plain
//! arithmetic on plain data: no device, so it is tested without a GPU.
//!
//! Each frame the visible water surfaces (the parts with a water material, the sea) are
//! gathered as [`Surface`]s and ranked:
//!
//! - **Planes**: sorted by height, a surface joins the group being built while it is within
//!   [`GROUP_M`] of the group's lowest surface; so two areas at one level share one plane. A
//!   group lies at the height of its largest surface when it first shows, and keeps last
//!   frame's height while that still lies among its surfaces and the water within [`MATCH_M`]
//!   of it covers 1 / [`KEEP_RATIO`] of what lies within [`MATCH_M`] of the largest surface:
//!   the mirror and the hysteresis key do not jump when two areas trade size, and a merged
//!   plane moves to the pond that comes to dominate it. The shaders match a surface to the
//!   nearest plane within [`MATCH_M`] (scene.wgsl `water_plane_at`), so neighbouring groups
//!   merge into one plane until their heights are [`SEPARATE_M`] apart and no surface of one
//!   lies within [`MATCH_M`] of the other's height: no water samples a layer that is not its
//!   own, outside that layer's scissor and black. A surface of a merged plane further than
//!   [`MATCH_M`] from its height finds no layer and reports its plane's fallback tier.
//! - **Score**: the share of the render target the plane covers (its rectangle's area, of the
//!   part of its box in front of the eye; for the sea the share of the rays that met it) times
//!   1 + nearness (1 at the eye, 0.5 at 10 m, towards 0 far off). A near small pond outranks a
//!   far lake of its size on screen; a lake several times larger still wins. The sea is
//!   preferred: a pond takes its pass only when it scores [`KEEP_RATIO`] times the sea's, so a
//!   garden pond does not routinely take a coast's only pass at Medium.
//! - **Tiers**: the best `budget` planes get a mirrored pass each ([`WaterTier::Planar`]), the
//!   rest reflect the opaque scene of this frame ([`WaterTier::ScreenSpace`]) when screen-space
//!   reflections are on, else the environment cube ([`WaterTier::Environment`]). The sea has
//!   no screen-space path (sea.wgsl): without a pass it takes the sky.
//! - **Hysteresis**: a plane that had a pass keeps it over a plane scoring less than
//!   [`KEEP_RATIO`] times its own, and gives it up only after [`KEEP_FRAMES`] consecutive
//!   frames of such a challenger: two ponds close in score do not swap while walking.

use glam::{Mat4, Vec3};

/// The planar reflection's layers in the globals (`reflection_planes`); `Quality::water_planes`
/// is clamped to it. Far under WebGPU's default `max_texture_array_layers` (256), so the array
/// target needs no limit of its own.
pub(crate) const MAX_PLANES: u32 = 4;
/// A surface joins the group being built within this distance of its lowest surface (m).
pub(crate) const GROUP_M: f32 = 0.04;
/// The shaders take a plane's layer for water within this distance of its height (m, scene.wgsl
/// `water_plane_at`).
pub(crate) const MATCH_M: f32 = 0.05;
/// Two planes' heights lie at least this far apart (m): twice [`MATCH_M`].
pub(crate) const SEPARATE_M: f32 = 2.0 * MATCH_M;
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
    /// The sea, not a pond: preferred in the ranking, and the sky when it gets no pass.
    pub sea: bool,
}

impl Surface {
    /// Whether it can be ranked: a height, a coverage and a distance that are numbers. A NaN
    /// height would be a plane of its own whose key (`key`) is that of 0 m, and NaN sorts
    /// above every score under `total_cmp`.
    pub fn usable(&self) -> bool {
        self.height.is_finite() && self.coverage.is_finite() && !self.distance.is_nan()
    }
}

/// The visible water surfaces at one height: one plane for the planar reflection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterPlane {
    /// The height its largest surface lay at when it first showed, kept while it still lies
    /// among its surfaces and the water at it is not [`KEEP_RATIO`] times outcovered
    /// (`Span::settle`); [`SEPARATE_M`] from the next plane's at the least.
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
    /// Per surface of the frame: its plane's index in `planes`, its height, and whether it is
    /// the sea.
    owner: Vec<u16>,
    heights: Vec<f32>,
    seas: Vec<bool>,
    /// Per plane: the heights its surfaces span.
    spans: Vec<Span>,
    /// Last frame's planes' heights (`Span::settle`).
    last: Vec<f32>,
    /// Per group of `GROUP_M`: the plane it merged into.
    remap: Vec<u16>,
    /// What a plane without a pass falls back to this frame.
    fallback: Option<WaterTier>,
    /// Per plane: its tier.
    pub tiers: Vec<WaterTier>,
    /// Last frame's planar planes by key, and for how many consecutive frames a challenger
    /// scored [`KEEP_RATIO`] times their score.
    held: Vec<(i32, u32)>,
    order: Vec<usize>,
    chosen: Vec<bool>,
    streaks: Vec<(i32, u32)>,
}

/// The heights a plane's surfaces span, and its largest one.
#[derive(Clone, Copy, Debug)]
struct Span {
    lo: f32,
    hi: f32,
    largest: f32,
    at: f32,
    /// The sea is one of its surfaces.
    sea: bool,
}

impl Span {
    fn merge(&mut self, other: Span) {
        self.lo = self.lo.min(other.lo);
        self.hi = self.hi.max(other.hi);
        self.sea |= other.sea;
        if other.largest > self.largest {
            self.largest = other.largest;
            self.at = other.at;
        }
    }

    /// The plane's height: one of last frame's planes' within the span (the nearest to its
    /// largest surface) while the water the shaders give that height's layer covers at least
    /// 1 / [`KEEP_RATIO`] of what its largest surface's height would get, else its largest
    /// surface's. Within a group of [`GROUP_M`] every height gets all of it, so two areas
    /// trading size keep the plane where it is; a merged plane wider than [`MATCH_M`] moves to
    /// the pond that comes to dominate it.
    fn settle(&self, last: &[f32], surfaces: &[Surface]) -> f32 {
        let covered = |h: f32| -> f32 {
            surfaces
                .iter()
                .filter(|s| (self.lo..=self.hi).contains(&s.height))
                .filter(|s| (s.height - h).abs() < MATCH_M)
                .map(|s| s.coverage)
                .sum()
        };
        let best = covered(self.at);
        last.iter()
            .copied()
            .filter(|h| (self.lo..=self.hi).contains(h))
            .filter(|&h| covered(h) * KEEP_RATIO >= best)
            .min_by(|a, b| (a - self.at).abs().total_cmp(&(b - self.at).abs()))
            .unwrap_or(self.at)
    }
}

impl WaterPlane {
    fn merge(&mut self, other: WaterPlane) {
        self.rect = self.rect.union(other.rect);
        self.coverage = (self.coverage + other.coverage).min(1.0);
        self.distance = self.distance.min(other.distance);
    }
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
        self.last.clear();
        self.last.extend(self.planes.iter().map(|p| p.height));
        self.planes.clear();
        self.spans.clear();
        self.owner.clear();
        self.owner.resize(surfaces.len(), 0);
        self.heights.clear();
        self.heights.extend(surfaces.iter().map(|s| s.height));
        self.seas.clear();
        self.seas.extend(surfaces.iter().map(|s| s.sea));
        self.order.clear();
        self.order.extend(0..surfaces.len());
        self.order.sort_by(|&a, &b| {
            surfaces[a]
                .height
                .total_cmp(&surfaces[b].height)
                .then(a.cmp(&b))
        });
        // Groups of GROUP_M, by height.
        for &s in &self.order {
            let surface = surfaces[s];
            let joins = self
                .spans
                .last()
                .is_some_and(|g| surface.height - g.lo < GROUP_M);
            if !joins {
                self.planes.push(WaterPlane {
                    height: surface.height,
                    rect: Rect::default(),
                    coverage: 0.0,
                    distance: f32::INFINITY,
                    score: 0.0,
                });
                self.spans.push(Span {
                    lo: surface.height,
                    hi: surface.height,
                    largest: f32::NEG_INFINITY,
                    at: surface.height,
                    sea: false,
                });
            }
            let i = self.planes.len() - 1;
            self.planes[i].merge(WaterPlane {
                height: surface.height,
                rect: surface.rect,
                coverage: surface.coverage,
                distance: surface.distance,
                score: 0.0,
            });
            self.spans[i].merge(Span {
                lo: surface.height,
                hi: surface.height,
                largest: surface.coverage,
                at: surface.height,
                sea: surface.sea,
            });
            self.owner[s] = i as u16;
        }
        // Each group settles on its height, and merges into the plane below it until the two
        // are apart (`apart`). Planes are only ever merged with their neighbours: once each
        // pair of neighbours is apart, so is every pair (the spans are ordered and each height
        // lies in its span).
        self.remap.clear();
        let mut kept = 0;
        for g in 0..self.planes.len() {
            self.planes[kept] = self.planes[g];
            self.spans[kept] = self.spans[g];
            self.planes[kept].height = self.spans[kept].settle(&self.last, surfaces);
            self.remap.push(kept as u16);
            kept += 1;
            while kept >= 2 && !self.apart(kept - 2, kept - 1) {
                let (a, b) = (kept - 2, kept - 1);
                let (plane, span) = (self.planes[b], self.spans[b]);
                self.planes[a].merge(plane);
                self.spans[a].merge(span);
                self.planes[a].height = self.spans[a].settle(&self.last, surfaces);
                for r in &mut self.remap {
                    if *r as usize == b {
                        *r = a as u16;
                    }
                }
                kept -= 1;
            }
        }
        self.planes.truncate(kept);
        self.spans.truncate(kept);
        for o in &mut self.owner {
            *o = self.remap[*o as usize];
        }
        for p in &mut self.planes {
            p.score = p.coverage * (1.0 + nearness(p.distance));
        }
        self.assign(self.planes.len(), budget, screen_space);
    }

    /// Whether the planes `a` and `b` (`b` the higher, both settled) can each keep a layer of
    /// their own: their heights [`SEPARATE_M`] apart, and no surface of either within
    /// [`MATCH_M`] of the other's height.
    fn apart(&self, a: usize, b: usize) -> bool {
        let (ha, hb) = (self.planes[a].height, self.planes[b].height);
        hb - ha >= SEPARATE_M
            && self.spans[b].lo - ha >= MATCH_M
            && hb - self.spans[a].hi >= MATCH_M
    }

    /// The plane of the last [`Ranking::rank`] the surface `s` belongs to.
    pub fn plane_of(&self, s: usize) -> usize {
        self.owner[s] as usize
    }

    /// How the surface `s` is reflected after the last [`Ranking::rank`]: its plane's tier, but
    /// a surface further than [`MATCH_M`] from its plane's height finds no layer in the shaders
    /// and takes the fallback, and the sea without a layer takes the sky (sea.wgsl has no
    /// screen-space path).
    pub fn tier_of(&self, s: usize) -> WaterTier {
        let i = self.owner[s] as usize;
        let tier = match self.tiers[i] {
            WaterTier::Planar if (self.heights[s] - self.planes[i].height).abs() >= MATCH_M => {
                self.fallback.unwrap_or(WaterTier::Environment)
            }
            t => t,
        };
        if self.seas[s] && tier == WaterTier::ScreenSpace {
            WaterTier::Environment
        } else {
            tier
        }
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

    /// The best `budget` planes get a pass, the sea's score counting [`KEEP_RATIO`] times. An
    /// incumbent outside that selection keeps its pass over the weakest newcomer until that one
    /// scores [`KEEP_RATIO`] times its own for [`KEEP_FRAMES`] consecutive frames.
    fn assign(&mut self, n: usize, budget: u32, screen_space: bool) {
        let fallback = if screen_space {
            WaterTier::ScreenSpace
        } else {
            WaterTier::Environment
        };
        self.fallback = Some(fallback);
        let Ranking {
            planes,
            spans,
            tiers,
            held,
            order,
            chosen,
            streaks,
            ..
        } = self;
        // What a pass is worth in the ranking: a pond has to score KEEP_RATIO times the sea's
        // to take it.
        let worth = |i: usize| planes[i].score * if spans[i].sea { KEEP_RATIO } else { 1.0 };
        tiers.clear();
        tiers.resize(n, fallback);
        chosen.clear();
        chosen.resize(n, false);
        let take = (budget as usize).min(n);
        // Best first: by score, then by plane (the order of `planes`: by height).
        order.clear();
        order.extend(0..n);
        order.sort_by(|&a, &b| worth(b).total_cmp(&worth(a)).then(a.cmp(&b)));
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
                    .min_by(|&a, &b| worth(a).total_cmp(&worth(b)));
                let Some(w) = weak else {
                    continue;
                };
                let clearly_better = worth(w) >= KEEP_RATIO * worth(i);
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

/// The corner `i` (0..8) of the box.
fn corner((lo, hi): (Vec3, Vec3), i: usize) -> Vec3 {
    Vec3::new(
        if i & 1 == 0 { lo.x } else { hi.x },
        if i & 2 == 0 { lo.y } else { hi.y },
        if i & 4 == 0 { lo.z } else { hi.z },
    )
}

/// Where the part of the box in front of the eye shows on a render target of `size` (pixels
/// from its top left): the box is clipped to just in front of the eye's plane (its corners
/// there, and where its 12 edges cross that plane) and the clipped points projected, so a pond
/// reaching behind the eye covers the part of the view it reaches into, not the whole of it.
/// The whole target when the projection breaks down (a non-finite matrix).
pub(crate) fn screen_rect(view_proj: Mat4, b: (Vec3, Vec3), size: (u32, u32)) -> Rect {
    /// Clip-space w the box is clipped at: points nearer project far off the target and are
    /// cut to its edge.
    const NEAR_W: f32 = 1e-3;
    let clip: [glam::Vec4; 8] = std::array::from_fn(|i| view_proj * corner(b, i).extend(1.0));
    if clip.iter().any(|p| p.w.is_nan()) {
        return Rect::whole(size);
    }
    let mut rect = Rect::default();
    for i in 0..8 {
        if clip[i].w > NEAR_W {
            match point_rect(clip[i], size) {
                Some(r) => rect = rect.union(r),
                None => return Rect::whole(size),
            }
        }
        // The edges from this corner along +x, +y and +z that cross the plane.
        for bit in [1, 2, 4] {
            let (a, c) = (clip[i], clip[i | bit]);
            if i & bit == 0 && (a.w > NEAR_W) != (c.w > NEAR_W) {
                let p = a + (c - a) * ((NEAR_W - a.w) / (c.w - a.w));
                match point_rect(p, size) {
                    Some(r) => rect = rect.union(r),
                    None => return Rect::whole(size),
                }
            }
        }
    }
    rect
}

/// The pixel a clip-space point in front of the eye lands on, cut to the target (`None`: not a
/// number).
fn point_rect(p: glam::Vec4, size: (u32, u32)) -> Option<Rect> {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let x = (p.x / p.w * 0.5 + 0.5) * w;
    let y = (0.5 - p.y / p.w * 0.5) * h;
    if !(x.is_finite() && y.is_finite()) {
        return None;
    }
    // A point past the right or bottom edge lands on the last pixel, not on none.
    let x0 = x.floor().clamp(0.0, (w - 1.0).max(0.0)) as u32;
    let y0 = y.floor().clamp(0.0, (h - 1.0).max(0.0)) as u32;
    let x1 = (x.ceil().clamp(0.0, w) as u32).max(x0 + 1).min(size.0);
    let y1 = (y.ceil().clamp(0.0, h) as u32).max(y0 + 1).min(size.1);
    Some(Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// The view-space depth of the box's nearest point, as the shaders divide the ripples' shift
/// by it (`max(depth, 1)`): its nearest corner's (the depth is linear over the box), a metre at
/// the least, and so a metre when the box reaches the eye's plane. The clip-space w of
/// `view_proj`, the view-space depth under a perspective projection (an orthographic one
/// gives 1, the most padding).
pub(crate) fn view_depth(view_proj: Mat4, b: (Vec3, Vec3)) -> f32 {
    (0..8)
        .map(|i| (view_proj * corner(b, i).extend(1.0)).w)
        .fold(f32::INFINITY, |d, w| if w > 1.0 { d.min(w) } else { 1.0 })
}

/// How far the ripples can shift a surface's lookup, in pixels, on a target of `size`: the
/// distortion times the normal's most tilt, over the view-space depth (`view_depth`; a metre
/// at the least, as the shaders take it), rounded to a pixel, and two pixels more for the
/// filter's reach.
pub(crate) fn ripple_pad(depth: f32, distortion: f32, tilt: f32, size: (u32, u32)) -> (u32, u32) {
    let share = tilt * distortion / depth.max(1.0);
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
            sea: false,
        }
    }

    fn sea(height: f32, coverage: f32, distance: f32) -> Surface {
        Surface {
            sea: true,
            ..surface(height, coverage, distance)
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
        // The higher surface sets a new group when it is more than GROUP_M above; that group
        // merges into the plane below while their heights are under SEPARATE_M apart.
        r = Ranking::default();
        r.rank(
            &[
                surface(0.0, 0.1, 3.0),
                surface(0.039, 0.1, 3.0),
                surface(0.078, 0.1, 3.0),
            ],
            4,
            true,
        );
        assert_eq!(r.planes.len(), 1, "{:?}", r.planes);
        assert_eq!(r.planes[0].height, 0.0);
        // 7.8 cm from the plane's height: in no layer's window, so no planar pass reported.
        assert_eq!(
            (r.tier_of(0), r.tier_of(1), r.tier_of(2)),
            (WaterTier::Planar, WaterTier::Planar, WaterTier::ScreenSpace)
        );
        r = Ranking::default();
        r.rank(
            &[
                surface(0.0, 0.1, 3.0),
                surface(0.039, 0.1, 3.0),
                surface(0.15, 0.1, 3.0),
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
        let (sea, pond) = (sea(0.0, 0.3, 8.0), surface(1.5, 0.1, 3.0));
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
    fn the_screen_rect_holds_the_box_and_its_front_part_behind_the_eye() {
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
        // A box reaching behind the eye, below it: its front part fills the bottom of the view
        // from edge to edge (where it reaches the eye's plane), not the whole target.
        let behind = screen_rect(
            view_proj,
            (Vec3::new(-1.0, 0.0, 6.0), Vec3::new(1.0, 1.0, 9.0)),
            SIZE,
        );
        assert_eq!((behind.x, behind.w), (0, SIZE.0), "{behind:?}");
        assert_eq!(behind.y + behind.h, SIZE.1, "{behind:?}");
        assert!(behind.y > 170 && behind.y < 200, "{behind:?}");
        // Nothing projects: the whole target.
        let broken = Mat4::from_cols_array(&[f32::NAN; 16]);
        assert_eq!(
            screen_rect(broken, (Vec3::ZERO, Vec3::ONE), SIZE),
            Rect::whole(SIZE)
        );
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

    #[test]
    fn a_non_finite_surface_is_not_gathered() {
        assert!(surface(0.0, 0.1, 3.0).usable());
        // The sea seen only far off is still a surface.
        assert!(
            Surface {
                distance: f32::INFINITY,
                ..surface(0.0, 0.1, 3.0)
            }
            .usable()
        );
        let nan = f32::NAN;
        for bad in [
            Surface {
                height: nan,
                ..surface(0.0, 0.1, 3.0)
            },
            Surface {
                height: f32::INFINITY,
                ..surface(0.0, 0.1, 3.0)
            },
            Surface {
                height: f32::NEG_INFINITY,
                ..surface(0.0, 0.1, 3.0)
            },
            surface(0.0, nan, 3.0),
            surface(0.0, 0.1, nan),
        ] {
            assert!(!bad.usable(), "{bad:?}");
        }
    }

    #[test]
    fn surfaces_at_0_0_039_and_0_041_share_one_plane() {
        // By GROUP_M alone 0 and 0.039 form a plane (at 0.039, the larger) and 0.041 one of its
        // own, 2 mm away: each would match the other's layer within the shaders' 5 cm.
        let mut r = Ranking::default();
        let surfaces = [
            surface(0.0, 0.02, 3.0),
            surface(0.039, 0.2, 3.0),
            surface(0.041, 0.05, 3.0),
        ];
        r.rank(&surfaces, 1, true);
        assert_eq!(r.planes.len(), 1, "{:?}", r.planes);
        assert_eq!(r.planes[0].height, 0.039);
        assert_eq!((r.plane_of(0), r.plane_of(1), r.plane_of(2)), (0, 0, 0));
        assert!((0..3).all(|s| r.tier_of(s) == WaterTier::Planar));
    }

    #[test]
    fn no_surface_lies_in_the_shaders_window_of_a_plane_but_its_own() {
        // Heights to the millimetre from a fixed sequence: clusters, staircases and near pairs.
        let mut seed = 12345u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut r = Ranking::default();
        for round in 0..500 {
            let n = 2 + round % 11;
            let surfaces: Vec<Surface> = (0..n)
                .map(|_| {
                    let h = (next() * 400.0).round() / 1000.0;
                    surface(h, 0.01 + next() * 0.08, 1.0 + next() * 30.0)
                })
                .collect();
            r.rank(&surfaces, 1 + round as u32 % 3, true);
            for w in r.planes.windows(2) {
                assert!(
                    w[1].height - w[0].height >= 0.10,
                    "round {round}: {:?}",
                    r.planes
                );
            }
            for (s, surface) in surfaces.iter().enumerate() {
                let own = r.plane_of(s);
                for (p, plane) in r.planes.iter().enumerate() {
                    if p != own {
                        assert!(
                            (surface.height - plane.height).abs() >= MATCH_M,
                            "round {round}: {} in {own} near {p}: {:?}",
                            surface.height,
                            r.planes
                        );
                    }
                }
                // A surface the shader finds no plane for does not report a planar pass.
                if r.tier_of(s) == WaterTier::Planar {
                    assert!((surface.height - r.planes[own].height).abs() < MATCH_M);
                }
            }
        }
    }

    #[test]
    fn a_swap_of_the_largest_member_keeps_the_height_and_the_pass() {
        let mut r = Ranking::default();
        // Plane A: two areas 3 cm apart; plane B a metre up, scoring just under A.
        let a = |low: f32, high: f32| [surface(0.0, low, 5.0), surface(0.03, high, 5.0)];
        let b = surface(1.0, 0.25, 5.0);
        let [x, y] = a(0.05, 0.25);
        assert_eq!(
            rank(&mut r, &[x, y, b], 1, true),
            [WaterTier::Planar, WaterTier::ScreenSpace]
        );
        assert_eq!(r.planes[0].height, 0.03);
        // The two areas trade sizes frame after frame (A's total stays) while B scores 1.1x A,
        // under the ratio: A keeps its height, its key and so its pass.
        let b = surface(1.0, 0.33, 5.0);
        for frame in 0..30 {
            let [x, y] = if frame % 2 == 0 {
                a(0.25, 0.05)
            } else {
                a(0.05, 0.25)
            };
            assert_eq!(
                rank(&mut r, &[x, y, b], 1, true),
                [WaterTier::Planar, WaterTier::ScreenSpace],
                "frame {frame}"
            );
            assert_eq!(r.planes[0].height, 0.03, "frame {frame}");
        }
        // Once the area at 0.03 m is gone the plane moves to the one left.
        rank(&mut r, &[surface(0.0, 0.3, 5.0), b], 1, true);
        assert_eq!(r.planes[0].height, 0.0);
    }

    #[test]
    fn a_merged_plane_moves_to_the_pond_that_comes_to_dominate_it() {
        // Cascade ponds 7 cm apart: one plane (under SEPARATE_M apart), at the larger one's
        // height when it first shows.
        let mut r = Ranking::default();
        let cascade = |low: f32, high: f32| [surface(0.0, low, 5.0), surface(0.07, high, 5.0)];
        r.rank(&cascade(0.3, 0.05), 1, true);
        assert_eq!(r.planes.len(), 1, "{:?}", r.planes);
        assert_eq!(r.planes[0].height, 0.0);
        // The upper pond grows to nearly the lower's size: under KEEP_RATIO times it, the
        // plane stays.
        r.rank(&cascade(0.3, 0.35), 1, true);
        assert_eq!(r.planes[0].height, 0.0);
        // Walking up to it, the upper pond fills the view: the plane follows it, and it is
        // the one mirrored.
        r.rank(&cascade(0.05, 0.6), 1, true);
        assert_eq!(r.planes.len(), 1, "{:?}", r.planes);
        assert_eq!(r.planes[0].height, 0.07);
        assert_eq!(
            (r.tier_of(0), r.tier_of(1)),
            (WaterTier::ScreenSpace, WaterTier::Planar)
        );
        // And back only once the lower one scores KEEP_RATIO times the upper.
        r.rank(&cascade(0.35, 0.3), 1, true);
        assert_eq!(r.planes[0].height, 0.07);
        r.rank(&cascade(0.6, 0.3), 1, true);
        assert_eq!(r.planes[0].height, 0.0);
    }

    #[test]
    fn an_unbudgeted_sea_takes_the_sky_and_ponds_screen_space() {
        let mut r = Ranking::default();
        let (sea, pond) = (sea(0.0, 0.1, 50.0), surface(1.5, 0.3, 3.0));
        rank(&mut r, &[sea, pond], 1, true);
        assert_eq!(
            (r.tier_of(0), r.tier_of(1)),
            (WaterTier::Environment, WaterTier::Planar)
        );
        // No pass at all: a pond at the sea's level shares its plane and marches the screen,
        // the sea (sea.wgsl has no screen-space path) takes the sky.
        let shore = surface(0.01, 0.05, 5.0);
        rank(&mut r, &[sea, pond, shore], 0, true);
        assert_eq!(r.plane_of(0), r.plane_of(2));
        assert_eq!(
            (r.tier_of(0), r.tier_of(1), r.tier_of(2)),
            (
                WaterTier::Environment,
                WaterTier::ScreenSpace,
                WaterTier::ScreenSpace
            )
        );
    }

    #[test]
    fn the_sea_keeps_its_pass_over_a_pond_unless_the_pond_scores_1_3_times_its_own() {
        // The sea over 0.3 of the view, 20 m off: 0.3 (1 + 1/3) = 0.4. A pond at the eye
        // scores twice its coverage.
        let s = sea(0.0, 0.3, 20.0);
        for (pond, winner) in [(0.24, 0), (0.25, 0), (0.27, 1)] {
            let mut r = Ranking::default();
            rank(&mut r, &[s, surface(1.0, pond, 0.0)], 1, true);
            let planar = r.tiers.iter().position(|t| *t == WaterTier::Planar);
            assert_eq!(planar, Some(winner), "pond {pond}: {:?}", r.planes);
        }
    }

    #[test]
    fn a_box_reaching_behind_the_eye_covers_what_shows_of_it() {
        // Standing at the edge of a pond 4 m wide, a metre of it behind the eye, looking ahead
        // and down: it shows over the bottom of the view only.
        let eye = Vec3::new(0.0, 1.6, 0.0);
        let cam = Mat4::look_at_rh(eye, Vec3::new(0.0, 0.0, -5.0), Vec3::Y);
        let proj = Mat4::perspective_rh(50f32.to_radians(), 16.0 / 9.0, 0.1, 1000.0);
        let rect = screen_rect(
            proj * cam,
            (Vec3::new(-2.0, 0.0, -3.0), Vec3::new(2.0, 0.0, 1.0)),
            SIZE,
        );
        assert_eq!((rect.x, rect.w), (0, SIZE.0), "{rect:?}");
        assert_eq!(rect.y + rect.h, SIZE.1, "{rect:?}");
        assert!(rect.y > 180 && rect.y < 195, "{rect:?}");
        let coverage = rect.area() as f32 / (SIZE.0 * SIZE.1) as f32;
        assert!(coverage > 0.25 && coverage < 0.35, "{coverage}");
    }

    #[test]
    fn the_ripple_pad_takes_the_view_depth_of_the_nearest_corner() {
        let proj = Mat4::perspective_rh(50f32.to_radians(), 16.0 / 9.0, 0.1, 1000.0);
        let view_proj = proj * Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        // A pond off to the side: 5.5 m deep in the view, 6.8 m from the eye. The shaders
        // divide the shift by the depth, so the pad does too.
        let pond = (Vec3::new(4.0, -0.5, -6.0), Vec3::new(4.5, 0.0, -5.5));
        let depth = view_depth(view_proj, pond);
        assert!((depth - 5.5).abs() < 1e-3, "{depth}");
        let distance = Vec3::ZERO.clamp(pond.0, pond.1).length();
        assert!(distance > 6.7, "{distance}");
        let (deep, far) = (
            ripple_pad(depth, POND_DISTORTION, WIND_TILT, SIZE),
            ripple_pad(distance, POND_DISTORTION, WIND_TILT, SIZE),
        );
        assert!(deep.0 > far.0 && deep.1 > far.1, "{deep:?} {far:?}");
        // A metre at the least, and a box reaching behind the eye's plane reaches its depth 0.
        let near = (Vec3::new(-0.1, -0.5, -0.6), Vec3::new(0.1, 0.0, -0.4));
        assert_eq!(view_depth(view_proj, near), 1.0);
        let around = (Vec3::new(-2.0, -0.5, -9.0), Vec3::new(2.0, 0.0, 3.0));
        assert_eq!(view_depth(view_proj, around), 1.0);
    }
}
