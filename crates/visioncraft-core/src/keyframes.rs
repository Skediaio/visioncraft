//! Keyframe animation of rigid nodes, as glTF 2.0 stores it (specification 2.0, "Animations" and
//! appendix C "Interpolation"). A [`Clip`] holds channels; each animates one property of one
//! node with keys interpolated STEP, LINEAR or CUBICSPLINE. [`sample`] gives every node's local
//! transform at a time and [`model_space`] composes the parent chains into matrices. Pure maths:
//! no GPU and no files, so the same code runs on `wasm32`.

/// A node's local transform: a translation in metres, a rotation as a unit quaternion
/// (x, y, z, w) and a scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Trs {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Trs {
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    };

    pub fn is_finite(&self) -> bool {
        self.translation
            .iter()
            .chain(&self.rotation)
            .chain(&self.scale)
            .all(|c| c.is_finite())
    }
}

/// The property a channel animates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    Translation,
    Rotation,
    Scale,
}

impl Path {
    /// Floats per key.
    pub fn width(self) -> usize {
        match self {
            Self::Translation | Self::Scale => 3,
            Self::Rotation => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// The value of the key at or before the time.
    Step,
    /// Straight between the keys; a rotation turns along the short arc (slerp).
    Linear,
    /// Each key holds an in-tangent, a value and an out-tangent; the tangents are scaled by the
    /// key interval.
    CubicSpline,
}

impl Interpolation {
    /// Floats per key: the value's width, or three of them.
    pub fn stride(self, path: Path) -> usize {
        path.width()
            * match self {
                Self::CubicSpline => 3,
                Self::Step | Self::Linear => 1,
            }
    }
}

/// The largest key time in seconds, and the largest magnitude of a value or a tangent, a clip
/// may have: a million. Within these limits every sample is finite: a cubic segment multiplies a
/// tangent by its key interval, and a million times a million is far inside a float's range (a
/// 1e30 tangent over a 1e9 s interval overflowed to infinity).
pub const MAX_TIME: f32 = 1e6;
pub const MAX_VALUE: f32 = 1e6;

/// The keys of one property of one node.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    /// The node: an index into the `rest` given to [`sample`].
    pub node: usize,
    pub path: Path,
    pub interpolation: Interpolation,
    /// Seconds: finite and strictly ascending.
    pub times: Vec<f32>,
    /// [`Interpolation::stride`] floats per key, key after key; a cubic-spline key holds its
    /// in-tangent, its value and its out-tangent, in that order.
    pub values: Vec<f32>,
}

impl Channel {
    /// The channel's value at `t` in its first `width` floats, whatever its keys are like.
    fn at(&self, t: f32) -> [f32; 4] {
        let (n, width, stride) = (
            self.times.len(),
            self.path.width(),
            self.interpolation.stride(self.path),
        );
        let value = |key: usize, part: usize| -> [f32; 4] {
            // `part` 0, 1, 2: the in-tangent, the value, the out-tangent of a cubic key.
            let mut v = [0.0; 4];
            let at = key * stride + part * width;
            if let Some(src) = self.values.get(at..at + width) {
                v[..width].copy_from_slice(src);
            }
            v
        };
        if n == 0 {
            return [0.0; 4];
        }
        // The keys `t` lies between: `times[k] <= t < times[k + 1]`; before the first key and
        // at or after the last the end keys hold.
        let i = self.times.partition_point(|&k| k <= t);
        if i == 0 || i >= n {
            return value(
                i.saturating_sub(1).min(n - 1),
                usize::from(self.interpolation == Interpolation::CubicSpline),
            );
        }
        let k = i - 1;
        let interval = self.times[k + 1] - self.times[k];
        let u = if interval > 0.0 {
            (t - self.times[k]) / interval
        } else {
            0.0
        };
        match self.interpolation {
            Interpolation::Step => value(k, 0),
            Interpolation::Linear => {
                let (a, b) = (value(k, 0), value(k + 1, 0));
                if self.path == Path::Rotation {
                    slerp(a, b, u)
                } else {
                    lerp(a, b, u)
                }
            }
            Interpolation::CubicSpline => {
                let v = hermite(
                    value(k, 1),
                    value(k, 2),
                    value(k + 1, 1),
                    value(k + 1, 0),
                    u,
                    interval,
                );
                if self.path == Path::Rotation {
                    normalised(v)
                } else {
                    v
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub name: String,
    /// Seconds, from 0 to the last key: a looped play wraps at it.
    pub duration: f32,
    pub channels: Vec<Channel>,
}

impl Clip {
    /// What is wrong with the clip, if anything. [`sample`] gives finite values and never
    /// panics for a clip this accepts.
    pub fn validate(&self) -> Result<(), String> {
        if !self.duration.is_finite() || self.duration < 0.0 {
            return Err(format!(
                "the duration is {} s and not a finite number of seconds from 0",
                self.duration
            ));
        }
        for (i, ch) in self.channels.iter().enumerate() {
            let what = format!("channel {i} ({:?} of node {})", ch.path, ch.node);
            ch.check(&what)?;
            let last = ch.times[ch.times.len() - 1];
            if last > self.duration {
                return Err(format!(
                    "the duration {} s ends before the last key of {what} at {last} s",
                    self.duration
                ));
            }
        }
        Ok(())
    }

    /// The last key time over the channels (0 without channels): what `duration` usually is.
    pub fn end(&self) -> f32 {
        self.channels
            .iter()
            .filter_map(|c| c.times.last())
            .fold(0.0, |end, t| end.max(*t))
    }
}

impl Channel {
    /// What is wrong with the channel, if anything; `what` names it in the message. [`sample`]
    /// gives finite values for a channel this accepts, whatever the time.
    pub fn check(&self, what: &str) -> Result<(), String> {
        if self.times.is_empty() {
            return Err(format!("{what} has no key"));
        }
        if let Some(bad) = self.times.iter().find(|t| !t.is_finite()) {
            return Err(format!("a key time of {what} is not finite: {bad}"));
        }
        if self.times[0] < 0.0 {
            return Err(format!(
                "a key time of {what} is negative: {}",
                self.times[0]
            ));
        }
        if let Some(bad) = self.times.iter().find(|t| **t > MAX_TIME) {
            return Err(format!(
                "a key time of {what} is {bad} s: key times go up to {MAX_TIME:e} s"
            ));
        }
        if let Some(w) = self.times.windows(2).find(|w| w[0] >= w[1]) {
            return Err(format!(
                "the key times of {what} are not in ascending order: {} then {}",
                w[0], w[1]
            ));
        }
        let needed = self.times.len() * self.interpolation.stride(self.path);
        if self.values.len() != needed {
            return Err(format!(
                "{what} has {} values where its {} keys need {}",
                self.values.len(),
                self.times.len(),
                needed
            ));
        }
        if let Some(bad) = self.values.iter().find(|v| !v.is_finite()) {
            return Err(format!("a value of {what} is not finite: {bad}"));
        }
        if let Some(bad) = self.values.iter().find(|v| v.abs() > MAX_VALUE) {
            return Err(format!(
                "a value or tangent of {what} is {bad}: they go up to {MAX_VALUE:e} in magnitude"
            ));
        }
        if self.path == Path::Rotation && self.values.iter().all(|v| *v == 0.0) {
            return Err(format!("the rotations of {what} are all zero"));
        }
        Ok(())
    }
}

/// The local transform of every node of `rest` at time `t`: a node no channel moves keeps its
/// rest transform, and a channel to a node beyond `rest` is ignored. Before the first key and
/// after the last the end keys hold, unless `looped` takes `t` modulo the duration. A sampled
/// rotation is always a unit quaternion.
pub fn sample(clip: &Clip, t: f32, looped: bool, rest: &[Trs]) -> Vec<Trs> {
    let mut out = Vec::new();
    sample_into(clip, t, looped, rest, &mut out);
    out
}

/// [`sample`] into `out`, which is filled afresh: no new allocation once it has its length.
pub fn sample_into(clip: &Clip, t: f32, looped: bool, rest: &[Trs], out: &mut Vec<Trs>) {
    out.clear();
    out.extend_from_slice(rest);
    apply(clip, wrap(clip, t, looped), out);
}

/// `t` as the clip takes it: modulo its duration when looped.
fn wrap(clip: &Clip, t: f32, looped: bool) -> f32 {
    if looped && t.is_finite() && clip.duration > 0.0 {
        t.rem_euclid(clip.duration)
    } else {
        t
    }
}

/// [`wrap`] for seconds on the clock: taken in `f64`, and narrowed to the clip's `f32` time
/// only then, so a loop that ran for days keeps its phase (at 86 400 s an `f32` steps in 8 ms).
fn wrap_clock(clip: &Clip, t: f64, looped: bool) -> f32 {
    if looped && t.is_finite() && clip.duration > 0.0 {
        t.rem_euclid(f64::from(clip.duration)) as f32
    } else {
        t as f32
    }
}

/// The channels of `clip` over the transforms in `out`, each over what the channels before it
/// left (so of two clips the one that started later wins where they share a property).
fn apply(clip: &Clip, t: f32, out: &mut [Trs]) {
    for ch in &clip.channels {
        let Some(trs) = out.get_mut(ch.node) else {
            continue;
        };
        let v = ch.at(t);
        match ch.path {
            Path::Translation => trs.translation = [v[0], v[1], v[2]],
            Path::Rotation => trs.rotation = normalised(v),
            Path::Scale => trs.scale = [v[0], v[1], v[2]],
        }
    }
}

fn lerp(a: [f32; 4], b: [f32; 4], u: f32) -> [f32; 4] {
    std::array::from_fn(|i| (1.0 - u) * a[i] + u * b[i])
}

/// glTF appendix C: the dot product decides the arc (a negative one flips `b`), and nearly
/// parallel keys take the plain line; the result is a unit quaternion either way.
fn slerp(a: [f32; 4], b: [f32; 4], u: f32) -> [f32; 4] {
    let mut d = a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let mut b = b;
    if d < 0.0 {
        d = -d;
        b = b.map(|c| -c);
    }
    let d = d.clamp(0.0, 1.0);
    if 1.0 - d <= 1e-6 {
        return normalised(lerp(a, b, u));
    }
    let angle = d.acos();
    let (sin, sa, sb) = (angle.sin(), (angle * (1.0 - u)).sin(), (angle * u).sin());
    normalised(std::array::from_fn(|i| (sa * a[i] + sb * b[i]) / sin))
}

/// A cubic Hermite segment between two keys `interval` seconds apart: the out-tangent of the
/// first key and the in-tangent of the second, each scaled by the interval (glTF appendix C).
fn hermite(
    v0: [f32; 4],
    out0: [f32; 4],
    v1: [f32; 4],
    in1: [f32; 4],
    u: f32,
    interval: f32,
) -> [f32; 4] {
    let (u2, u3) = (u * u, u * u * u);
    let (h00, h10, h01, h11) = (
        2.0 * u3 - 3.0 * u2 + 1.0,
        u3 - 2.0 * u2 + u,
        -2.0 * u3 + 3.0 * u2,
        u3 - u2,
    );
    std::array::from_fn(|i| {
        h00 * v0[i] + h10 * out0[i] * interval + h01 * v1[i] + h11 * in1[i] * interval
    })
}

/// `q` at unit length; the identity when it has no length (a key that is all zero).
fn normalised(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > 1e-6 {
        q.map(|c| c / len)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// A column-major 4x4 matrix (the glam and WGSL layout).
pub type Matrix = [f32; 16];

const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// The matrix of a transform: it scales, then rotates, then translates.
pub fn trs_matrix(trs: &Trs) -> Matrix {
    let [x, y, z, w] = normalised(trs.rotation);
    let (xx, yy, zz, xy, xz, yz) = (x * x, y * y, z * z, x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    let [sx, sy, sz] = trs.scale;
    [
        (1.0 - 2.0 * (yy + zz)) * sx,
        (2.0 * (xy + wz)) * sx,
        (2.0 * (xz - wy)) * sx,
        0.0,
        (2.0 * (xy - wz)) * sy,
        (1.0 - 2.0 * (xx + zz)) * sy,
        (2.0 * (yz + wx)) * sy,
        0.0,
        (2.0 * (xz + wy)) * sz,
        (2.0 * (yz - wx)) * sz,
        (1.0 - 2.0 * (xx + yy)) * sz,
        0.0,
        trs.translation[0],
        trs.translation[1],
        trs.translation[2],
        1.0,
    ]
}

/// `a` after `b` (both column-major).
fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut m = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            m[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    m
}

/// The model-space matrix of each node: its own transform after its parent chain's. `parents`
/// names each node's parent (`None`: a root); a parent outside the slice counts as a root. A
/// parent may come after its child.
pub fn model_space(parents: &[Option<usize>], local: &[Trs]) -> Vec<Matrix> {
    let mut out = Vec::new();
    let mut done = Vec::new();
    let mut chain = Vec::new();
    model_space_into(parents, local, &mut out, &mut done, &mut chain);
    out
}

/// [`model_space`] into `out` (its length is `parents.len()`), with `done` and `chain` as scratch:
/// no new allocation once they have their lengths.
fn model_space_into(
    parents: &[Option<usize>],
    local: &[Trs],
    out: &mut Vec<Matrix>,
    done: &mut Vec<bool>,
    chain: &mut Vec<usize>,
) {
    let n = parents.len().min(local.len());
    let root = |i: usize| parents[i].filter(|p| *p < n);
    out.clear();
    out.resize(n, IDENTITY);
    done.clear();
    done.resize(n, false);
    for i in 0..n {
        if done[i] {
            continue;
        }
        // The chain up from `i` that is not computed yet, bottom first; one that loops back on
        // itself (not a hierarchy) stops at the node it meets.
        chain.clear();
        let mut at = Some(i);
        while let Some(k) = at
            && !done[k]
        {
            done[k] = true;
            chain.push(k);
            at = root(k);
        }
        for &k in chain.iter().rev() {
            out[k] = match root(k) {
                Some(p) => mul(&out[p], &trs_matrix(&local[k])),
                None => trs_matrix(&local[k]),
            };
        }
    }
}

/// A model's nodes as far as animation goes: each one's rest transform and parent, and the
/// clips that move them (the `parts` and `clips` of a glTF file). The node a clip's channel
/// names is an index into `rest`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rig {
    pub rest: Vec<Trs>,
    pub parents: Vec<Option<usize>>,
    pub clips: Vec<Clip>,
}

/// A clip of a [`Rig`] playing: which one, from which time on the clock, and whether it loops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Played {
    pub clip: usize,
    /// Seconds on the clock when the clip started, in `f64` as the clock is.
    pub start_s: f64,
    pub looped: bool,
}

/// Scratch space for posing a [`Rig`] frame after frame without allocating.
#[derive(Default)]
pub struct Sampler {
    local: Vec<Trs>,
    out: Vec<Matrix>,
    done: Vec<bool>,
    chain: Vec<usize>,
}

impl Sampler {
    /// The model-space matrix of every node of `rig` at clock time `t`, with `plays` playing
    /// in the order they started: where two of them move the same property, the later one wins.
    /// A node no clip moves is at rest, and so is one before its clips start: a play that has
    /// not started contributes nothing (it neither holds its first key nor loops early).
    pub fn pose(&mut self, rig: &Rig, plays: &[Played], t: f64) -> &[Matrix] {
        self.local.clear();
        self.local.extend_from_slice(&rig.rest);
        for p in plays {
            // In `f64`, as `t` and the starts are: it is narrowed once it is within the clip.
            let since = t - p.start_s;
            if since < 0.0 {
                continue;
            }
            if let Some(clip) = rig.clips.get(p.clip) {
                apply(clip, wrap_clock(clip, since, p.looped), &mut self.local);
            }
        }
        model_space_into(
            &rig.parents,
            &self.local,
            &mut self.out,
            &mut self.done,
            &mut self.chain,
        );
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Path = Path::Translation;
    const R: Path = Path::Rotation;
    const S: Path = Path::Scale;
    /// The sine and the cosine of 45 degrees.
    const Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| close(*x, *y))
    }

    fn near4(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| close(*x, *y))
    }

    /// Matrices agree within the rounding of the sine of 45 degrees.
    fn near16(a: Matrix, b: Matrix) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| close(*x, *y))
    }

    fn channel(path: Path, interpolation: Interpolation, times: &[f32], values: &[f32]) -> Channel {
        Channel {
            node: 0,
            path,
            interpolation,
            times: times.to_vec(),
            values: values.to_vec(),
        }
    }

    /// A clip of one channel on node 0, as long as its last key.
    fn clip(path: Path, interpolation: Interpolation, times: &[f32], values: &[f32]) -> Clip {
        let of = |c: Channel| Clip {
            name: "test".into(),
            duration: 0.0,
            channels: vec![c],
        };
        let mut c = of(channel(path, interpolation, times, values));
        c.duration = c.end();
        c
    }

    /// One node at rest unturned, and a clip that starts it turned a quarter about Z and turns
    /// it back: its first key is not the rest pose.
    fn turned_rig() -> Rig {
        Rig {
            rest: vec![Trs::IDENTITY],
            parents: vec![None],
            clips: vec![clip(
                R,
                Interpolation::Linear,
                &[0.0, 1.0],
                &[0.0, 0.0, Q, Q, 0.0, 0.0, 0.0, 1.0],
            )],
        }
    }

    #[test]
    fn before_its_start_a_clip_leaves_the_rest_pose_alone() {
        let r = turned_rig();
        let rest = model_space(&r.parents, &r.rest)[0];
        let mut s = Sampler::default();
        for looped in [false, true] {
            let plays = [Played {
                clip: 0,
                start_s: 10.0,
                looped,
            }];
            for t in [0.0, 5.0, 8.5, 9.999] {
                assert!(
                    near16(s.pose(&r, &plays, t)[0], rest),
                    "looped {looped}, t = {t}: not at rest"
                );
            }
            // From its start it plays: its first key, not the rest.
            assert!(
                !near16(s.pose(&r, &plays, 10.0)[0], rest),
                "looped {looped}"
            );
        }
    }

    /// A quarter turn about Z in half a second, looped by the plays below.
    fn half_turn() -> Rig {
        Rig {
            rest: vec![Trs::IDENTITY],
            parents: vec![None],
            clips: vec![clip(
                R,
                Interpolation::Linear,
                &[0.0, 0.5],
                &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, Q, Q],
            )],
        }
    }

    /// Node 0 as the sampler poses `r` at clock time `t`.
    fn posed(r: &Rig, plays: &[Played], t: f64) -> Matrix {
        let mut s = Sampler::default();
        s.pose(r, plays, t)[0]
    }

    /// Node 0 at clip time `ct`, without looping.
    fn at_clip_time(r: &Rig, ct: f32) -> Matrix {
        let local = sample(&r.clips[0], ct, false, &r.rest);
        model_space(&r.parents, &local)[0]
    }

    fn within_1e_5(a: Matrix, b: Matrix) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    /// A window open for a day has a clock around 86 400 s, where an `f32` steps in 7.8 ms: a
    /// clip started at such a time and narrowed to `f32` lost its phase. The start is the
    /// clock's own `f64`: 0.125 s in is the pose at clip time 0.125.
    #[test]
    fn a_clip_keeps_its_phase_started_at_a_long_clock_time() {
        let r = half_turn();
        // 100 000.251 is not a number `f32` holds (there it is 100 000.25). The literal has no
        // suffix: it is what the field's type makes of it.
        let plays = [Played {
            clip: 0,
            start_s: 100_000.251,
            looped: true,
        }];
        let pose = posed(&r, &plays, 100_000.251_f64 + 0.125);
        let want = at_clip_time(&r, 0.125);
        assert!(
            within_1e_5(pose, want),
            "{pose:?} is not the pose at clip time 0.125: {want:?}"
        );
    }

    /// A looped clip keeps its phase after running for days: `since` is wrapped in `f64` and
    /// narrowed to the clip's `f32` time only then. At 100 000 s an `f32` steps in 7.8 ms, 0.28
    /// degrees of a quarter turn in half a second.
    #[test]
    fn a_looped_clip_keeps_its_phase_after_many_loops() {
        let r = half_turn();
        let plays = [Played {
            clip: 0,
            start_s: 100_000.25,
            looped: true,
        }];
        // 100 000.1 s on: 200 000 loops and 0.1 s.
        let pose = posed(&r, &plays, 200_000.35);
        let want = at_clip_time(&r, 0.1);
        assert!(
            within_1e_5(pose, want),
            "{pose:?} is not the pose at clip time 0.1: {want:?}"
        );
    }

    #[test]
    fn step_holds_the_key_at_or_before_the_time() {
        let c = clip(
            T,
            Interpolation::Step,
            &[0.0, 1.0, 2.0],
            &[10.0, 0.0, 0.0, 20.0, 0.0, 0.0, 30.0, 0.0, 0.0],
        );
        for (t, x) in [
            (-3.0, 10.0),
            (0.0, 10.0),
            (0.5, 10.0),
            (0.999, 10.0),
            (1.0, 20.0),
            (1.5, 20.0),
            (2.0, 30.0),
            (7.0, 30.0),
        ] {
            let p = sample(&c, t, false, &[Trs::IDENTITY])[0].translation;
            assert!(near(p, [x, 0.0, 0.0]), "t = {t}: {p:?}");
        }
    }

    #[test]
    fn linear_translation_is_straight_between_the_keys() {
        let c = clip(
            T,
            Interpolation::Linear,
            &[0.0, 2.0],
            &[0.0, 0.0, 0.0, 4.0, 2.0, -2.0],
        );
        for (t, p) in [
            (0.25, [0.5, 0.25, -0.25]),
            (0.5, [1.0, 0.5, -0.5]),
            (1.0, [2.0, 1.0, -1.0]),
            (1.75, [3.5, 1.75, -1.75]),
            (2.0, [4.0, 2.0, -2.0]),
        ] {
            let got = sample(&c, t, false, &[Trs::IDENTITY])[0].translation;
            assert!(near(got, p), "t = {t}: {got:?}");
        }
    }

    #[test]
    fn cubic_spline_scales_its_tangents_by_the_key_interval() {
        // Keys at 0, 1 and 3 s: intervals of 1 s and of 2 s.
        //   key 0: in (unused) 0, value 0, out (2, 0, 0)
        //   key 1: in 0, value (1, 0, 0), out (0, 3, 0)
        //   key 2: in 0, value (1, 1, 0), out (unused) 0
        // At 0.5 s (half of the first interval, t_d = 1), x:
        //   (t^3 - 2t^2 + t) * 2 * 1 + (-2t^3 + 3t^2) * 1 = 0.125 * 2 + 0.5 = 0.75.
        // At 2 s (half of the second interval, t_d = 2), y:
        //   (t^3 - 2t^2 + t) * 3 * 2 + (-2t^3 + 3t^2) * 1 = 0.125 * 6 + 0.5 = 1.25;
        //   without the interval it would be 0.875.
        let c = clip(
            T,
            Interpolation::CubicSpline,
            &[0.0, 1.0, 3.0],
            &[
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 3.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, //
            ],
        );
        for (t, p) in [
            (-1.0, [0.0, 0.0, 0.0]),
            (0.0, [0.0, 0.0, 0.0]),
            (0.5, [0.75, 0.0, 0.0]),
            (1.0, [1.0, 0.0, 0.0]),
            (2.0, [1.0, 1.25, 0.0]),
            (3.0, [1.0, 1.0, 0.0]),
            (5.0, [1.0, 1.0, 0.0]),
        ] {
            let got = sample(&c, t, false, &[Trs::IDENTITY])[0].translation;
            assert!(near(got, p), "t = {t}: {got:?}");
        }
        // One key holds its value. Between two keys with zero tangents the spline eases in and
        // out: a quarter of the way it has covered -2u^3 + 3u^2 = 0.15625 of the distance.
        let one = clip(
            T,
            Interpolation::CubicSpline,
            &[0.5],
            &[9.0, 9.0, 9.0, 1.0, 2.0, 3.0, 9.0, 9.0, 9.0],
        );
        let p = sample(&one, 4.0, false, &[Trs::IDENTITY])[0].translation;
        assert!(near(p, [1.0, 2.0, 3.0]), "{p:?}");
        let straight = clip(
            T,
            Interpolation::CubicSpline,
            &[0.0, 1.0],
            &[
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0,
                0.0, 0.0,
            ],
        );
        let p = sample(&straight, 0.25, false, &[Trs::IDENTITY])[0].translation;
        assert!(near(p, [0.3125, 0.0, 0.0]), "{p:?}");
    }

    #[test]
    fn a_linear_rotation_takes_the_short_arc() {
        // 120 degrees about Z, written negated (its dot product with the identity is -0.5):
        // half way is 60 degrees about +Z, not 60 degrees about -Z.
        let z120 = [0.0, 0.0, -0.8660254, -0.5];
        let (q0, q1) = ([0.0, 0.0, 0.0, 1.0], z120);
        let c = clip(
            R,
            Interpolation::Linear,
            &[0.0, 1.0],
            &[q0[0], q0[1], q0[2], q0[3], q1[0], q1[1], q1[2], q1[3]],
        );
        let q = sample(&c, 0.5, false, &[Trs::IDENTITY])[0].rotation;
        assert!(near4(q, [0.0, 0.0, 0.5, 0.8660254]), "{q:?}");
        let q = sample(&c, 0.25, false, &[Trs::IDENTITY])[0].rotation;
        assert!(near4(q, [0.0, 0.0, 0.258819, 0.9659258]), "{q:?}");
        // The same turn written the other way round gives the same path.
        let c = clip(
            R,
            Interpolation::Linear,
            &[0.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.8660254, 0.5],
        );
        let q = sample(&c, 0.5, false, &[Trs::IDENTITY])[0].rotation;
        assert!(near4(q, [0.0, 0.0, 0.5, 0.8660254]), "{q:?}");
        // Nearly parallel keys: the plain line, renormalised.
        let c = clip(
            R,
            Interpolation::Linear,
            &[0.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0005, 0.9999999],
        );
        let q = sample(&c, 0.5, false, &[Trs::IDENTITY])[0].rotation;
        assert!(near4(q, [0.0, 0.0, 0.00025, 1.0]), "{q:?}");
        // The keys themselves hold at their times.
        let c = clip(
            R,
            Interpolation::Linear,
            &[0.0, 1.0],
            &[q0[0], q0[1], q0[2], q0[3], q1[0], q1[1], q1[2], q1[3]],
        );
        assert_eq!(sample(&c, 0.0, false, &[Trs::IDENTITY])[0].rotation, q0);
        assert_eq!(sample(&c, 1.0, false, &[Trs::IDENTITY])[0].rotation, q1);
    }

    #[test]
    fn a_sampled_rotation_is_a_unit_quaternion() {
        let keys: [[f32; 4]; 4] = [
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, -0.8660254, -0.5],
            [0.6, 0.0, 0.0, 0.8],
            [0.5, 0.5, 0.5, 0.5],
        ];
        for interpolation in [Interpolation::Linear, Interpolation::CubicSpline] {
            let mut values = Vec::new();
            for k in keys {
                // The tangents of a cubic key: small turns from the value.
                for tangent in [[-0.1, 0.05, 0.0, 0.02], [0.0; 4], [0.03, 0.0, -0.06, 0.01]] {
                    values.extend(k);
                    if interpolation == Interpolation::CubicSpline {
                        values.extend(tangent);
                    }
                }
            }
            let c = clip(R, interpolation, &[0.0, 1.0, 2.5, 4.0], &values);
            for i in 0..=400 {
                let q = sample(&c, i as f32 / 100.0, false, &[Trs::IDENTITY])[0].rotation;
                let len = q.iter().map(|x| x * x).sum::<f32>().sqrt();
                assert!(
                    close(len, 1.0),
                    "{interpolation:?} at {}: {q:?}",
                    i as f32 / 100.0
                );
            }
        }
    }

    #[test]
    fn before_and_after_the_clip_the_end_keys_hold() {
        let c = clip(
            S,
            Interpolation::Linear,
            &[1.0, 3.0],
            &[1.0, 1.0, 1.0, 3.0, 3.0, 3.0],
        );
        for t in [-100.0, -0.5, 0.0, 0.999] {
            let s = sample(&c, t, false, &[Trs::IDENTITY])[0].scale;
            assert!(near(s, [1.0; 3]), "t = {t}: {s:?}");
        }
        for t in [3.0, 3.001, 100.0] {
            let s = sample(&c, t, false, &[Trs::IDENTITY])[0].scale;
            assert!(near(s, [3.0; 3]), "t = {t}: {s:?}");
        }
    }

    #[test]
    fn a_looped_time_wraps_at_the_duration() {
        let c = clip(
            T,
            Interpolation::Linear,
            &[0.0, 2.0],
            &[0.0, 0.0, 0.0, 4.0, 2.0, -2.0],
        );
        let rest = [Trs::IDENTITY];
        let at = |t: f32| sample(&c, t, true, &rest)[0].translation;
        assert_eq!(at(2.25), at(0.25));
        assert!(near(at(2.25), [0.5, 0.25, -0.25]), "{:?}", at(2.25));
        assert!(near(at(4.5), [1.0, 0.5, -0.5]), "{:?}", at(4.5));
        // From before the start: the end of the loop.
        assert!(near(at(-0.25), [3.5, 1.75, -1.75]), "{:?}", at(-0.25));
        assert!(near(at(2.0), [0.0, 0.0, 0.0]), "{:?}", at(2.0));
        // Without a duration there is nothing to wrap at.
        let one = clip(T, Interpolation::Step, &[0.0], &[7.0, 8.0, 9.0]);
        let p = sample(&one, 5.0, true, &rest)[0].translation;
        assert!(near(p, [7.0, 8.0, 9.0]), "{p:?}");
    }

    #[test]
    fn a_node_without_a_channel_stays_at_rest() {
        let rest = [
            Trs {
                translation: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.6, 0.0, 0.8],
                scale: [2.0; 3],
            },
            Trs {
                translation: [5.0, 0.0, 0.0],
                ..Trs::IDENTITY
            },
            Trs {
                scale: [0.5, 1.0, 1.5],
                ..Trs::IDENTITY
            },
        ];
        let mut c = clip(
            T,
            Interpolation::Linear,
            &[0.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        );
        c.channels[0].node = 1;
        // A channel to a node the rest does not have is ignored.
        c.channels.push(Channel {
            node: 9,
            ..channel(S, Interpolation::Step, &[0.0], &[9.0; 3])
        });
        for t in [-1.0, 0.0, 0.5, 1.0, 9.0] {
            let got = sample(&c, t, false, &rest);
            assert_eq!(got.len(), 3);
            assert_eq!(got[0], rest[0]);
            assert_eq!(got[2], rest[2]);
            assert!(
                near(got[1].translation, [t.clamp(0.0, 1.0), 0.0, 0.0]),
                "{t}"
            );
        }
        // Nothing at all plays: the rest itself.
        let none = Clip {
            name: "empty".into(),
            duration: 0.0,
            channels: vec![],
        };
        assert_eq!(sample(&none, 3.0, true, &rest), rest.to_vec());
    }

    #[test]
    fn validate_rejects_each_malformed_clip() {
        fn of(path: Path, interpolation: Interpolation, times: &[f32], values: &[f32]) -> Clip {
            let mut c = Clip {
                name: "bad".into(),
                duration: 0.0,
                channels: vec![channel(path, interpolation, times, values)],
            };
            c.duration = c.end();
            c
        }
        // What passes.
        of(T, Interpolation::Linear, &[0.0, 1.0], &[0.0; 6])
            .validate()
            .unwrap();
        of(T, Interpolation::CubicSpline, &[0.5], &[0.0; 9])
            .validate()
            .unwrap();
        of(T, Interpolation::CubicSpline, &[0.0, 1.0], &[0.0; 18])
            .validate()
            .unwrap();
        of(R, Interpolation::Linear, &[0.0], &[0.0, 0.0, 0.0, 1.0])
            .validate()
            .unwrap();
        Clip {
            name: String::new(),
            duration: 5.0,
            channels: vec![],
        }
        .validate()
        .unwrap();

        let good = of(T, Interpolation::Linear, &[0.0, 1.0], &[0.0; 6]);
        let longer = Clip {
            duration: 2.5,
            ..good.clone()
        };
        longer.validate().unwrap();
        let cases: Vec<(Clip, &str)> = vec![
            (of(T, Interpolation::Linear, &[], &[]), "key"),
            (
                of(T, Interpolation::Linear, &[1.0, 1.0], &[0.0; 6]),
                "ascending",
            ),
            (
                of(T, Interpolation::Linear, &[2.0, 1.0], &[0.0; 6]),
                "ascending",
            ),
            (
                of(T, Interpolation::Linear, &[0.0, f32::NAN], &[0.0; 6]),
                "finite",
            ),
            (
                of(T, Interpolation::Linear, &[0.0, f32::INFINITY], &[0.0; 6]),
                "finite",
            ),
            (
                of(T, Interpolation::Linear, &[0.0, 1.0], &[0.0; 5]),
                "values",
            ),
            (
                of(T, Interpolation::Linear, &[0.0, 1.0], &[0.0; 9]),
                "values",
            ),
            (
                of(
                    T,
                    Interpolation::Linear,
                    &[0.0, 1.0],
                    &[0.0, 0.0, f32::NEG_INFINITY, 0.0, 0.0, 0.0],
                ),
                "finite",
            ),
            (
                of(T, Interpolation::CubicSpline, &[0.0, 1.0], &[0.0; 12]),
                "values",
            ),
            (
                of(T, Interpolation::CubicSpline, &[0.0, 1.0], &[0.0; 6]),
                "values",
            ),
            (of(R, Interpolation::Linear, &[0.0, 1.0], &[0.0; 8]), "zero"),
            (
                Clip {
                    duration: -1.0,
                    ..good.clone()
                },
                "duration",
            ),
            (
                Clip {
                    duration: f32::NAN,
                    ..good.clone()
                },
                "duration",
            ),
            (
                Clip {
                    duration: 0.5,
                    ..good.clone()
                },
                "duration",
            ),
        ];
        for (bad, word) in cases {
            let e = bad.validate().expect_err("a malformed clip is refused");
            assert!(e.contains(word), "{word:?} not in {e:?}");
        }
    }

    #[test]
    fn a_cubic_spline_rotation_matches_the_hand_computed_value() {
        // Keys at 0 s and 1 s: no turn, then a quarter about Z. The first key leaves along +Z
        // (its out-tangent), the second is entered from -Z (its in-tangent), per second.
        // At a quarter of the interval (u = 0.25, t_d = 1): h00 = 0.84375, h10 = 0.140625,
        // h01 = 0.15625, h11 = -0.046875, so before the normalising:
        //   z = 0.140625 * 0.2 + 0.15625 * sin 45deg - 0.046875 * (-0.1) = 0.1432979
        //   w = 0.84375 * 1 + 0.15625 * cos 45deg = 0.9542354
        //   z / |q| = 0.1485053, w / |q| = 0.9889116 (17.08 degrees).
        let c = clip(
            R,
            Interpolation::CubicSpline,
            &[0.0, 1.0],
            &[
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.2, 0.0, //
                0.0, 0.0, -0.1, 0.0, 0.0, 0.0, Q, Q, 0.0, 0.0, 0.0, 0.0,
            ],
        );
        c.validate().unwrap();
        for (t, q) in [
            (0.25, [0.0, 0.0, 0.1485053, 0.9889116]),
            (0.5, [0.0, 0.0, 0.4165151, 0.9091288]),
            (0.75, [0.0, 0.0, 0.6357361, 0.7719065]),
        ] {
            let got = sample(&c, t, false, &[Trs::IDENTITY])[0].rotation;
            assert!(near4(got, q), "t = {t}: {got:?}");
        }
        // The keys themselves, and beyond them the end keys.
        for (t, q) in [(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [0.0, 0.0, Q, Q])] {
            let got = sample(&c, t, false, &[Trs::IDENTITY])[0].rotation;
            assert!(near4(got, q), "t = {t}: {got:?}");
        }
        // With no tangents the spline takes the chord: half way both keys weigh the same, which
        // is the bisector, 45 degrees.
        let straight = clip(
            R,
            Interpolation::CubicSpline,
            &[0.0, 1.0],
            &[
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, //
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, Q, Q, 0.0, 0.0, 0.0, 0.0,
            ],
        );
        let got = sample(&straight, 0.5, false, &[Trs::IDENTITY])[0].rotation;
        assert!(near4(got, [0.0, 0.0, 0.3826834, 0.9238795]), "{got:?}");
    }

    #[test]
    fn a_looped_clip_longer_than_its_last_key_holds_then_wraps() {
        // Keys at 0 and 2 s, in a loop of 3 s.
        let mut c = clip(
            T,
            Interpolation::Linear,
            &[0.0, 2.0],
            &[0.0, 0.0, 0.0, 4.0, 2.0, -2.0],
        );
        c.duration = 3.0;
        c.validate().unwrap();
        let rest = [Trs::IDENTITY];
        let at = |t: f32, looped: bool| sample(&c, t, looped, &rest)[0].translation;
        // After the last key, up to the end of the loop, the last key holds.
        for t in [2.0, 2.5, 2.999] {
            let p = at(t, true);
            assert!(near(p, [4.0, 2.0, -2.0]), "t = {t}: {p:?}");
        }
        // At the end of the loop it is the start.
        let p = at(3.0, true);
        assert!(near(p, [0.0, 0.0, 0.0]), "t = 3: {p:?}");
        assert!(near(at(2.5, false), [4.0, 2.0, -2.0]), "not looped");
        // The wrap: 3.25 s is a quarter of a second into the clip, the next loop the same.
        for t in [0.25, 3.25, 6.25] {
            let p = at(t, true);
            assert!(near(p, [0.5, 0.25, -0.25]), "t = {t}: {p:?}");
        }
        assert!(near(at(5.0, true), [4.0, 2.0, -2.0]), "4 s is 1 s in");
        // From before the start: a quarter of a second before the end of the loop.
        let p = at(-0.75, true);
        assert!(near(p, [4.0, 2.0, -2.0]), "{:?}", p);
    }

    #[test]
    fn validate_rejects_a_negative_key_time_and_what_would_overflow() {
        fn of(path: Path, interpolation: Interpolation, times: &[f32], values: &[f32]) -> Clip {
            let mut c = Clip {
                name: "bad".into(),
                duration: 0.0,
                channels: vec![channel(path, interpolation, times, values)],
            };
            c.duration = c.end();
            c
        }
        let cubic = |a: f32, b: f32| {
            of(
                T,
                Interpolation::CubicSpline,
                &[0.0, 1.0],
                &[
                    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, a, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
                    0.0, b,
                ],
            )
        };
        let cases: Vec<(Clip, &str)> = vec![
            (
                of(T, Interpolation::Linear, &[-1.0, 1.0], &[0.0; 6]),
                "negative",
            ),
            (
                of(
                    T,
                    Interpolation::Linear,
                    &[0.0, 1.0],
                    &[2e6, 0.0, 0.0, 0.0, 0.0, 0.0],
                ),
                "1e6",
            ),
            (cubic(1e30, 0.0), "1e6"),
            (cubic(0.0, -3e6), "1e6"),
            (of(T, Interpolation::Linear, &[0.0, 2e6], &[0.0; 6]), "1e6"),
        ];
        for (i, (bad, word)) in cases.into_iter().enumerate() {
            let e = bad.validate().expect_err("refused");
            assert!(e.contains(word), "case {i}: {e}");
        }
        // At the limits everything passes: a key time of a million seconds, a scale of a
        // million, tangents of a million.
        of(S, Interpolation::Step, &[0.0], &[1e6; 3])
            .validate()
            .unwrap();
        of(T, Interpolation::Linear, &[999_999.0, MAX_TIME], &[0.0; 6])
            .validate()
            .unwrap();
        cubic(MAX_VALUE, -MAX_VALUE).validate().unwrap();
        // The tangent the reviewer showed, over its long interval: refused.
        let mut shown = clip(
            T,
            Interpolation::CubicSpline,
            &[0.0, 1e9],
            &[
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1e30, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                0.0, 0.0,
            ],
        );
        shown.channels[0].times[1] = MAX_TIME;
        shown.duration = shown.end();
        assert!(shown.validate().unwrap_err().contains("1e6"));
    }

    #[test]
    fn the_limits_keep_every_sample_finite() {
        // Everything at its largest: keys a million seconds apart, values and tangents a
        // million, a rest transform a million wide. Nothing overflows.
        let c = clip(
            T,
            Interpolation::CubicSpline,
            &[0.0, MAX_TIME],
            &[
                -MAX_VALUE, 0.0, 0.0, MAX_VALUE, -MAX_VALUE, 0.0, MAX_VALUE, 0.0, 0.0, //
                MAX_VALUE, 0.0, 0.0, -MAX_VALUE, MAX_VALUE, 0.0, -MAX_VALUE, 0.0, 0.0,
            ],
        );
        c.validate().unwrap();
        let r = clip(
            R,
            Interpolation::CubicSpline,
            &[0.0, MAX_TIME],
            &[
                -MAX_VALUE, MAX_VALUE, -MAX_VALUE, MAX_VALUE, 0.0, 0.0, Q, Q, MAX_VALUE,
                -MAX_VALUE, MAX_VALUE, -MAX_VALUE, //
                MAX_VALUE, -MAX_VALUE, MAX_VALUE, -MAX_VALUE, 0.0, Q, 0.0, Q, -MAX_VALUE,
                MAX_VALUE, -MAX_VALUE, MAX_VALUE,
            ],
        );
        r.validate().unwrap();
        let rest = [
            Trs {
                translation: [MAX_VALUE; 3],
                rotation: [MAX_VALUE; 4],
                scale: [MAX_VALUE; 3],
            },
            Trs::IDENTITY,
        ];
        let mut both = c.clone();
        both.channels[0].node = 1;
        both.channels.extend(r.channels.iter().cloned());
        for i in 0..=100 {
            let t = MAX_TIME * i as f32 / 100.0;
            let got = sample(&both, t, i % 2 == 0, &rest);
            assert_eq!(got.len(), 2);
            for trs in &got {
                assert!(trs.is_finite(), "t = {t}: {trs:?}");
            }
            let m = model_space(&[None, Some(0)], &got);
            assert!(
                m.iter().flat_map(|m| m.iter()).all(|x| x.is_finite()),
                "{}",
                m[1][0]
            );
        }
    }

    #[test]
    fn the_matrix_scales_rotates_then_translates() {
        let quarter = [0.0, Q, 0.0, Q];
        let m = trs_matrix(&Trs {
            translation: [1.0, 2.0, 3.0],
            rotation: quarter,
            scale: [2.0, 3.0, 4.0],
        });
        assert!(
            near16(
                m,
                [
                    0.0, 0.0, -2.0, 0.0, 0.0, 3.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0,
                    1.0,
                ]
            ),
            "{m:?}"
        );
        // A rotation that is not unit still only turns.
        let twice = trs_matrix(&Trs {
            rotation: [0.0, std::f32::consts::SQRT_2, 0.0, std::f32::consts::SQRT_2],
            ..Trs::IDENTITY
        });
        let unit = trs_matrix(&Trs {
            rotation: quarter,
            ..Trs::IDENTITY
        });
        assert!(near16(twice, unit), "{twice:?} vs {unit:?}");
        assert!(
            near16(
                unit,
                [
                    0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                    1.0,
                ]
            ),
            "{unit:?}"
        );
        assert_eq!(
            trs_matrix(&Trs::IDENTITY),
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0
            ]
        );
    }

    #[test]
    fn model_space_composes_a_three_deep_chain() {
        // 0 sits 1 m east; 1 under it turns a quarter about Y; 2 under that sits 2 m along -Z,
        // twice as large. The origin of 2: -Z turned a quarter is -X, so 1 m west of the centre.
        let local = [
            Trs {
                translation: [1.0, 0.0, 0.0],
                ..Trs::IDENTITY
            },
            Trs {
                rotation: [0.0, Q, 0.0, Q],
                ..Trs::IDENTITY
            },
            Trs {
                translation: [0.0, 0.0, -2.0],
                scale: [2.0; 3],
                ..Trs::IDENTITY
            },
        ];
        let m = model_space(&[None, Some(0), Some(1)], &local);
        assert_eq!(m.len(), 3);
        assert!(
            near16(
                m[2],
                [
                    0.0, 0.0, -2.0, 0.0, 0.0, 2.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0,
                    1.0,
                ]
            ),
            "{:?}",
            m[2]
        );
        assert!(
            near16(
                m[1],
                [
                    0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
                    1.0,
                ]
            ),
            "{:?}",
            m[1]
        );
        assert_eq!(m[0], trs_matrix(&local[0]));
        // A parent outside the slice is a root, and so is a shorter `local`.
        assert_eq!(model_space(&[None, Some(0)], &[local[0]]), vec![m[0]]);
        assert_eq!(model_space(&[Some(7)], &[local[0]]), vec![m[0]]);
        // Roots side by side do not see each other.
        let apart = model_space(&[None, None, None], &local);
        assert_eq!(apart.len(), 3);
        for (a, own) in apart.iter().zip(&local) {
            assert!(near16(*a, trs_matrix(own)), "{a:?}");
        }
    }

    /// splitmix64: small, deterministic and good enough for a test.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// -4..4.
        fn f32(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1 << 21) as f32 - 4.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn random_clip(rng: &mut Rng, nodes: usize) -> Clip {
        let mut channels = Vec::new();
        for _ in 0..rng.below(4) {
            let path = [T, R, S][rng.below(3)];
            let interpolation = [
                Interpolation::Step,
                Interpolation::Linear,
                Interpolation::CubicSpline,
            ][rng.below(3)];
            let keys = 1 + rng.below(6);
            let (mut times, mut t) = (Vec::new(), 0.0);
            for _ in 0..keys {
                times.push(t);
                t += 0.05 + rng.f32().abs();
            }
            let stride = interpolation.stride(path);
            let mut values: Vec<f32> = (0..keys * stride).map(|_| rng.f32()).collect();
            if path == R {
                // Most quaternions are turned a little; one in eight is zero, which the sampler
                // must survive.
                for k in values.as_chunks_mut::<4>().0 {
                    if rng.below(8) == 0 {
                        k.fill(0.0);
                    } else {
                        k[3] = k[3].abs() + 0.5;
                    }
                }
                if values.iter().all(|v| *v == 0.0) {
                    values[3] = 1.0;
                }
            }
            channels.push(Channel {
                node: rng.below(nodes),
                path,
                interpolation,
                times,
                values,
            });
        }
        let mut clip = Clip {
            name: String::new(),
            duration: 0.0,
            channels,
        };
        clip.duration = clip.end()
            + if rng.below(2) == 0 {
                rng.f32().abs()
            } else {
                0.0
            };
        clip.validate().expect("a generated clip is valid");
        clip
    }

    #[test]
    fn random_clips_never_panic_and_stay_finite() {
        let mut rng = Rng(0x5EED_1234_ABCD_0001);
        let mut samples = 0;
        for i in 0..3000 {
            let nodes = 1 + rng.below(6);
            let rest: Vec<Trs> = (0..nodes)
                .map(|_| {
                    let mut t = Trs {
                        translation: [rng.f32(), rng.f32(), rng.f32()],
                        rotation: [rng.f32(), rng.f32(), rng.f32(), rng.f32()],
                        scale: [rng.f32(), rng.f32(), rng.f32()],
                    };
                    if t.rotation.iter().all(|c| *c == 0.0) {
                        t.rotation[3] = 1.0;
                    }
                    t
                })
                .collect();
            let clip = random_clip(&mut rng, nodes);
            for looped in [false, true] {
                for t in [
                    rng.f32() * 3.0 * clip.duration.max(1.0),
                    -rng.f32().abs(),
                    clip.duration,
                    clip.duration + 0.25,
                ] {
                    let got = sample(&clip, t, looped, &rest);
                    assert_eq!(got.len(), nodes);
                    for trs in &got {
                        assert!(trs.is_finite(), "clip {i} at {t}: {trs:?}");
                    }
                    samples += 1;
                }
            }
            // A forest of parents (each under an earlier node), then a loop in it: both end.
            let parents: Vec<Option<usize>> = (0..nodes)
                .map(|i| (i > 0 && rng.below(4) != 0).then(|| rng.below(i)))
                .collect();
            let matrices = model_space(&parents, &rest);
            assert_eq!(matrices.len(), nodes);
            for m in &matrices {
                assert!(m.iter().all(|c| c.is_finite()), "{m:?}");
            }
            let mut looping = parents.clone();
            if nodes > 1 {
                looping[0] = Some(nodes - 1);
                looping[nodes - 1] = Some(0);
            }
            assert_eq!(model_space(&looping, &rest).len(), nodes);
        }
        assert!(samples >= 24_000, "{samples} samples");
    }
}
