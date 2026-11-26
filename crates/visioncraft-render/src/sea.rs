//! The sea level: a water surface at one height out to the horizon (or inside an outline), see
//! sea.wgsl. Drawn in the forward pass after the opaque scene, the endless ground and the sky,
//! before the transparent items: first the wet band on the shore (multiplied in), then the
//! water. The planar reflection follows the sea's level while there is one.
//!
//! Wave model: a sum of 16 Gerstner waves (8 at LOW) rather than an FFT ocean. Both run in
//! browsers today (WebGPU has compute), but an FFT needs a 256^2 spectrum per cascade and
//! several passes per frame, a few milliseconds on an integrated GPU, and still tiles; a short
//! analytic sum costs well under a millisecond, has no period to tile with, evaluates exactly
//! per pixel for distance-filtered normals, and is what a garden-scale coast needs.

use crate::{Bytes, Layouts, PipelineDesc, Renderer, depth_state, module, pipeline};

/// The water surface, in physical terms (the core's `Sea` maps sea state to these).
#[derive(Clone, Debug, PartialEq)]
pub struct Sea {
    /// Height of the mean surface (m).
    pub level: f32,
    /// Significant wave height (m).
    pub wave_height_m: f32,
    /// Wavelength of the most energetic waves (m).
    pub peak_wavelength_m: f32,
    /// Unit direction the waves travel towards (world x, z).
    pub direction: [f32; 2],
    /// Linear RGB of deep water.
    pub color: [f32; 3],
    /// 0..1, as `water_transmittance`.
    pub turbidity: f32,
    /// Surface flow (m/s, world x, z).
    pub flow: [f32; 2],
    /// Foam amount 0..1.
    pub foam: f32,
    /// Freezes in a snowy winter.
    pub freezes: bool,
    /// The water only inside this polygon (`[x, z]`, up to 64 points); empty: everywhere.
    pub outline: Vec<[f32; 2]>,
    /// A coastline to the horizon (`[x, z]`, up to 16 points, the sea to its right): the endless
    /// ground stays land on its left and drops under the water on its right.
    pub coast: Vec<[f32; 2]>,
}

/// Where the sea shows in a view: what the planar reflection's ranking (`water.rs`) needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SeaSight {
    /// The cells of the view's ray grid that met the water: how many, and their bounds (rows
    /// and columns of the grid, inclusive).
    pub hits: u32,
    pub cells: Cells,
    /// From the eye to the nearest point a ray met (m).
    pub distance: f32,
    /// The least view-space depth of those points (m): what sea.wgsl divides the ripples'
    /// shift by.
    pub depth: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cells {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// The cells the rays met, in pixels of a render target of `size` (top-left origin).
impl SeaSight {
    pub(crate) fn rect(self, size: (u32, u32)) -> crate::water::Rect {
        let cell = |c: u32, n: u32, size: u32| {
            (
                ((c as f32 - 0.5).max(0.0) * size as f32 / n as f32).floor() as u32,
                ((c as f32 + 0.5) * size as f32 / n as f32)
                    .ceil()
                    .min(size as f32) as u32,
            )
        };
        let (x0, _) = cell(self.cells.x0, COLUMNS, size.0);
        let (_, x1) = cell(self.cells.x1, COLUMNS, size.0);
        let (y0, _) = cell(self.cells.y0, ROWS, size.1);
        let (_, y1) = cell(self.cells.y1, ROWS, size.1);
        crate::water::Rect {
            x: x0,
            y: y0,
            w: x1.saturating_sub(x0),
            h: y1.saturating_sub(y0),
        }
    }
}

/// The grid of rays through the view that looks for the sea: 32 x 24 cells.
pub(crate) const COLUMNS: u32 = 32;
pub(crate) const ROWS: u32 = 24;

/// Where `sea`'s surface shows in the view `view_proj` of `eye`: rays through a grid over the
/// view meet its level on the sea side of the coast (with a margin growing with the distance)
/// and inside the outline, or a corner of the outline lies in the view. Hills and dunes between
/// are not tested: what the view frames, it counts.
pub(crate) fn sea_sight(sea: &Sea, eye: glam::Vec3, view_proj: glam::Mat4) -> Option<SeaSight> {
    use glam::Vec3;
    let inverse = view_proj.inverse();
    let level = sea.level;
    let mut sight: Option<SeaSight> = None;
    // A corner of the outline in view: some of it shows where no ray through the grid lands.
    let mut corner_seen = false;
    for &[x, z] in sea.outline.iter().take(if sea.outline.len() >= 3 {
        usize::MAX
    } else {
        0
    }) {
        let c = view_proj * glam::Vec4::new(x, level, z, 1.0);
        if c.w > 0.0 && c.x.abs() <= c.w && c.y.abs() <= c.w {
            let ndc_x = (c.x / c.w * 0.5 + 0.5) * COLUMNS as f32;
            let ndc_y = (0.5 - c.y / c.w * 0.5) * ROWS as f32;
            see(
                &mut sight,
                (ndc_y as u32).min(ROWS),
                (ndc_x as u32).min(COLUMNS),
                Vec3::new(x, level, z),
                eye,
                c.w,
            );
            corner_seen = true;
        }
    }
    if corner_seen {
        return sight;
    }
    for row in 0..=ROWS {
        for column in 0..=COLUMNS {
            let x = -1.0 + 2.0 * column as f32 / COLUMNS as f32;
            let y = -1.0 + 2.0 * row as f32 / ROWS as f32;
            let near = inverse.project_point3(Vec3::new(x, y, 0.0));
            let far = inverse.project_point3(Vec3::new(x, y, 1.0));
            // The ray from the near plane on (past the far plane too: the sea runs on to the
            // horizon) meets the level ahead of it.
            let (a, b) = (near.y - level, far.y - level);
            if a <= 0.0 || b >= a {
                continue;
            }
            let p = near + (far - near) * (a / (a - b));
            let q = [p.x, p.z];
            let margin = 5.0 + 0.1 * p.distance(near);
            let in_outline = sea.outline.len() < 3 || covers(&sea.outline, q);
            let sea_side = sea.coast.len() < 2 || coast_distance(&sea.coast, q) > -margin;
            if in_outline && sea_side {
                let depth = (view_proj * p.extend(1.0)).w;
                see(&mut sight, row, column, p, eye, depth);
            }
        }
    }
    sight
}

/// Notes that the ray through grid cell `(row, column)` met the water at `at`, `depth` deep in
/// the view.
fn see(
    sight: &mut Option<SeaSight>,
    row: u32,
    column: u32,
    at: glam::Vec3,
    eye: glam::Vec3,
    depth: f32,
) {
    let c = sight.get_or_insert(SeaSight {
        hits: 0,
        cells: Cells {
            x0: column,
            y0: row,
            x1: column,
            y1: row,
        },
        distance: f32::INFINITY,
        depth: f32::INFINITY,
    });
    c.hits += 1;
    c.cells = Cells {
        x0: c.cells.x0.min(column),
        y0: c.cells.y0.min(row),
        x1: c.cells.x1.max(column),
        y1: c.cells.y1.max(row),
    };
    c.distance = c.distance.min(at.distance(eye));
    c.depth = c.depth.min(depth);
}

/// Even-odd rule over an outline (as sea.wgsl `sea_covers`).
fn covers(outline: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut inside = false;
    let mut j = outline.len() - 1;
    for i in 0..outline.len() {
        let (a, b) = (outline[i], outline[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Signed distance (m) from `p` to a coastline, positive on the sea side (its right), the first
/// and last stretches running on without end (as sea.wgsl `coast_distance`).
fn coast_distance(coast: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let n = coast.len();
    let mut best = f32::INFINITY;
    let mut side = 1.0;
    for i in 0..n.saturating_sub(1) {
        let a = coast[i];
        let d = [coast[i + 1][0] - a[0], coast[i + 1][1] - a[1]];
        let q = [p[0] - a[0], p[1] - a[1]];
        let lo = if i == 0 { f32::NEG_INFINITY } else { 0.0 };
        let hi = if i + 2 == n { f32::INFINITY } else { 1.0 };
        let t = ((q[0] * d[0] + q[1] * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-9)).clamp(lo, hi);
        let dist = (q[0] - t * d[0]).hypot(q[1] - t * d[1]);
        if dist < best {
            best = dist;
            side = if d[0] * q[1] - d[1] * q[0] >= 0.0 {
                1.0
            } else {
                -1.0
            };
        }
    }
    best * side
}

/// Waves on MEDIUM and above; LOW has the first (longest) half.
pub const WAVES: usize = 16;
const OUTLINE: usize = 64;
/// The bed slope the surf's crest spacing assumes (1:25, a typical sandy nearshore).
// shortcut: one slope for every beach; upgrade by measuring it from the terrain under the water.
const SURF_SLOPE: f32 = 0.04;
/// Coastline points in the globals.
pub(crate) const COAST: usize = 16;
const UNIFORM_SIZE: u64 = 5 * 16 + (WAVES as u64) * 32 + (OUTLINE as u64 / 2) * 16;
const RINGS: u32 = 96;
const SEGMENTS: u32 = 128;

/// One Gerstner wave: direction (x, z), wavenumber k (rad/m), amplitude (m), angular frequency
/// (rad/s), phase (rad), steepness Q.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wave {
    pub direction: [f32; 2],
    pub k: f32,
    pub amplitude: f32,
    pub omega: f32,
    pub phase: f32,
    pub q: f32,
}

/// The wave set for a sea: wavelengths (two near the peak, see below) from 1.8 lambda_p down to lambda_p / 30 in a geometric
/// series, amplitudes from the Pierson-Moskowitz spectrum S(w) ~ w^-5 exp(-1.25 (w_p / w)^4)
/// sampled per log-spaced band (A^2 ~ S(w) dw, dw ~ w, and w ~ lambda^-1/2 in deep water give
/// A ~ lambda exp(-0.625 (lambda / lambda_p)^2)), scaled so that H_s = 4 sqrt(sum A^2 / 2)
/// matches; directions spread round the main one, wider for the shorter waves; Gerstner
/// steepness so that the sum of Q k A stays at 0.6 (no loops).
pub fn waves(sea: &Sea) -> Vec<Wave> {
    let lp = sea.peak_wavelength_m.max(0.1);
    let ratio = (1.0f32 / 54.0).powf(1.0 / (WAVES as f32 - 1.0));
    let base = sea.direction[1].atan2(sea.direction[0]);
    // A river's waves are a confused chop, not a swell from one side.
    let river = sea.flow[0].hypot(sea.flow[1]) > 0.05;
    let mut out: Vec<Wave> = (0..WAVES)
        .map(|i| {
            // The second and third waves sit near the peak, 15 % apart, so that they beat into
            // groups of about seven waves instead of running as even stripes.
            let lambda = match i {
                1 => 1.15 * lp,
                2 => lp,
                _ => 1.8 * lp * ratio.powi(i as i32),
            };
            let k = std::f32::consts::TAU / lambda;
            // Deterministic spread: a golden-ratio sequence in -1..1.
            let u = (i as f32 * 0.618_034 + 0.31).fract() * 2.0 - 1.0;
            let spread = if river {
                1.4
            } else {
                0.45 + 0.9 * i as f32 / WAVES as f32
            };
            let a = base + u * spread;
            Wave {
                direction: [a.cos(), a.sin()],
                k,
                amplitude: lambda * (-0.625 * (lambda / lp).powi(2)).exp(),
                omega: (9.81 * k).sqrt(),
                phase: (i as f32 * 2.399_963).rem_euclid(std::f32::consts::TAU),
                q: 0.0,
            }
        })
        .collect();
    let sum2: f32 = out.iter().map(|w| w.amplitude * w.amplitude).sum();
    let scale = sea.wave_height_m.max(0.0) / (4.0 * (sum2 / 2.0).sqrt()).max(1e-6);
    let mut steep = 0.0;
    for w in &mut out {
        w.amplitude *= scale;
        steep += w.k * w.amplitude;
    }
    // Q_i k_i A_i shares 0.6 in proportion to each wave's steepness: Q is the same for all.
    let q = if steep > 0.0 {
        (0.6 / steep).min(1.0)
    } else {
        0.0
    };
    for w in &mut out {
        w.q = q;
    }
    out
}

pub(crate) struct SeaGpu {
    buffer: wgpu::Buffer,
    bind: wgpu::BindGroup,
    sea: Option<Sea>,
    waves: Vec<Wave>,
}

pub(crate) struct SeaPipelines {
    surface: wgpu::RenderPipeline,
    wet: wgpu::RenderPipeline,
}

pub(crate) fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sea"),
        entries: &[crate::uniform_entry(
            30,
            wgpu::ShaderStages::VERTEX_FRAGMENT,
            None,
        )],
    })
}

impl SeaGpu {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sea"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sea"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 30,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            buffer,
            bind,
            sea: None,
            waves: Vec::new(),
        }
    }
}

impl SeaPipelines {
    pub fn new(
        device: &wgpu::Device,
        l: &Layouts,
        modules: &[wgpu::ShaderModule],
        samples: u32,
    ) -> Self {
        let groups = [Some(&l.globals), Some(&l.sea), None, Some(&l.forward)];
        let make = |label, vs, fs, blend, compare| {
            pipeline(
                device,
                PipelineDesc {
                    label,
                    module: module(modules, "scene"),
                    vs,
                    fs: Some(fs),
                    groups: &groups,
                    buffers: &[],
                    target: Some(wgpu::ColorTargetState {
                        format: crate::HDR,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    depth: Some(depth_state(false, compare, Default::default())),
                    cull: None,
                    samples,
                    constants: &[],
                    front_cw: false,
                },
            )
        };
        Self {
            surface: make(
                "sea",
                "vs_sea",
                "fs_sea",
                wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                wgpu::CompareFunction::LessEqual,
            ),
            wet: make(
                "sea wet band",
                "vs_sea_wet",
                "fs_sea_wet",
                wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                wgpu::CompareFunction::Always,
            ),
        }
    }
}

impl Renderer {
    /// The sea level (off by default).
    pub fn set_sea(&mut self, sea: Option<Sea>) {
        if sea == self.sea.sea {
            return;
        }
        self.sea.waves = sea.as_ref().map(waves).unwrap_or_default();
        self.sea.sea = sea;
    }

    pub fn sea(&self) -> Option<&Sea> {
        self.sea.sea.as_ref()
    }

    /// Where the sea shows while it is seen from above it and some of it lies in the view
    /// `view_proj` (`sea_sight`): the surface the planar reflection's ranking (`water.rs`)
    /// takes it for, its rectangle padded for the ripple distortion. Looking inland, away from
    /// the water, the pass drew the whole mirrored scene for nothing.
    pub(crate) fn sea_surface(
        &self,
        view_proj: glam::Mat4,
        size: (u32, u32),
        tilt: f32,
    ) -> Option<crate::water::Surface> {
        let s = self.sea.sea.as_ref().filter(|s| self.eye.y > s.level)?;
        let sight = sea_sight(s, self.eye, view_proj)?;
        let coverage = f64::from(sight.hits) / f64::from((ROWS + 1) * (COLUMNS + 1));
        let pad = crate::water::ripple_pad(sight.depth, crate::water::SEA_DISTORTION, tilt, size);
        Some(crate::water::Surface {
            height: s.level,
            rect: sight.rect(size).padded(pad, size),
            coverage: coverage as f32,
            distance: sight.distance,
            sea: true,
        })
    }

    /// Writes this frame's sea parameters (the wave count follows the quality).
    pub(crate) fn write_sea(&self) {
        let Some(s) = &self.sea.sea else { return };
        let count = if self.quality.water_reflection == 0 {
            WAVES / 2
        } else {
            WAVES
        };
        let ground_above = self.ground.is_some_and(|g| g.height > s.level);
        let n = s.outline.len().min(OUTLINE);
        let mut b = Bytes::default();
        b.floats(&[
            s.level,
            count as f32,
            s.foam.clamp(0.0, 1.0),
            s.turbidity.clamp(0.0, 1.0),
        ]);
        b.floats(&[
            s.color[0],
            s.color[1],
            s.color[2],
            0.15 + 0.4 * s.wave_height_m,
        ]);
        b.floats(&[
            s.flow[0],
            s.flow[1],
            f32::from(u8::from(s.freezes)),
            n as f32,
        ]);
        // Wind ripples grow with the wind, and are all there is on a glassy sea.
        let ripple = (0.4 + 0.6 * self.wind.strength().clamp(0.0, 2.0)).min(1.5);
        b.floats(&[
            f32::from(u8::from(ground_above)),
            ripple,
            s.wave_height_m,
            1.0,
        ]);
        // The surf: the peak frequency (deep-water dispersion), a nominal nearshore slope and the
        // swell's direction.
        let omega_p = (9.81 * std::f32::consts::TAU / s.peak_wavelength_m.max(0.1)).sqrt();
        b.floats(&[omega_p, SURF_SLOPE, s.direction[0], s.direction[1]]);
        for i in 0..WAVES {
            let w = self.sea.waves.get(i).copied().unwrap_or(Wave {
                direction: [1.0, 0.0],
                k: 1.0,
                amplitude: 0.0,
                omega: 0.0,
                phase: 0.0,
                q: 0.0,
            });
            b.floats(&[w.direction[0], w.direction[1], w.k, w.amplitude]);
            b.floats(&[w.omega, w.phase, w.q, 0.0]);
        }
        for i in 0..OUTLINE {
            b.floats(&s.outline.get(i).copied().unwrap_or([0.0; 2]));
        }
        debug_assert_eq!(b.0.len() as u64, UNIFORM_SIZE);
        self.queue.write_buffer(&self.sea.buffer, 0, &b.0);
    }

    /// The wet band, then the water (forward pass, after the sky).
    pub(crate) fn draw_sea(&self, pass: &mut wgpu::RenderPass) {
        if self.sea.sea.is_none() {
            return;
        }
        pass.set_bind_group(0, &self.globals_bind, &[]);
        pass.set_bind_group(1, &self.sea.bind, &[]);
        pass.set_bind_group(3, &self.targets.forward_bind, &[]);
        pass.set_pipeline(&self.sampled.sea.wet);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&self.sampled.sea.surface);
        pass.draw(0..RINGS * SEGMENTS * 6, 0..1);
        self.counters.add(1, 1, false);
        self.counters.add((RINGS * SEGMENTS * 2) as u64, 1, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sea_counts_in_view_only_where_the_view_frames_its_side_of_the_coast() {
        use glam::{Mat4, Vec3};
        // A coast along x = 0 running north (-z): the sea to its right, east (+x).
        let mut s = sea(1.0, 20.0);
        s.coast = vec![[0.0, 100.0], [0.0, -100.0]];
        let proj = Mat4::perspective_rh(50f32.to_radians(), 16.0 / 9.0, 0.1, 3000.0);
        let look = |eye: Vec3, target: Vec3| proj * Mat4::look_at_rh(eye, target, Vec3::Y);
        let eye = Vec3::new(-50.0, 30.0, 0.0);
        let seen = |s: &Sea, target: Vec3| sea_sight(s, eye, look(eye, target));
        // Towards the sea, and inland away from it.
        assert!(seen(&s, Vec3::new(50.0, 0.0, 0.0)).is_some());
        assert!(seen(&s, Vec3::new(-150.0, 0.0, 0.0)).is_none());
        // Along the coast the sea's side shows at the edge of the view: a few rays, in a strip.
        let edge = seen(&s, Vec3::new(-50.0, 0.0, -100.0)).expect("seen along the coast");
        assert!(edge.hits < 8 * 33, "{edge:?}");
        // Straight up at the sky: the level is behind every ray.
        assert!(seen(&s, Vec3::new(-50.0, 100.0, 1.0)).is_none());
        // No coast and no outline: the sea is everywhere below the horizon: most of the view,
        // and the nearest water is not far from the eye looking down the shore.
        s.coast.clear();
        let open = seen(&s, Vec3::new(-150.0, 0.0, 0.0)).expect("open sea");
        assert!(open.hits > 25 * 33 / 2, "{open:?}");
        assert!(open.distance > 30.0 && open.distance < 200.0, "{open:?}");
        // A small outline in a corner of the view: the corner counts where no ray lands.
        s.outline = vec![[-160.0, -10.0], [-150.0, -10.0], [-150.0, -5.0]];
        let corner = seen(&s, Vec3::new(-150.0, 0.0, 0.0)).expect("the outline in the corner");
        assert!(corner.hits >= 1, "{corner:?}");
        s.outline = vec![[400.0, 400.0], [410.0, 400.0], [410.0, 410.0]];
        assert!(seen(&s, Vec3::new(-150.0, 0.0, 0.0)).is_none());
    }

    fn sea(h: f32, l: f32) -> Sea {
        Sea {
            level: 0.0,
            wave_height_m: h,
            peak_wavelength_m: l,
            direction: [0.0, 1.0],
            color: [0.0; 3],
            turbidity: 0.5,
            flow: [0.0; 2],
            foam: 0.5,
            freezes: false,
            outline: Vec::new(),
            coast: Vec::new(),
        }
    }

    #[test]
    fn the_waves_carry_the_wave_height_and_never_loop() {
        for (h, l) in [(0.02, 3.0), (0.875, 34.0), (5.0, 195.0)] {
            let w = waves(&sea(h, l));
            assert_eq!(w.len(), WAVES);
            let hs = 4.0 * (w.iter().map(|w| w.amplitude.powi(2)).sum::<f32>() / 2.0).sqrt();
            assert!((hs - h).abs() < 1e-3 * h.max(1.0), "{hs} for {h}");
            let qka: f32 = w.iter().map(|w| w.q * w.k * w.amplitude).sum();
            assert!(qka <= 0.6 + 1e-4, "{qka}");
            // The longest waves come first and carry the most height.
            assert!(w[0].k < w[WAVES - 1].k);
            let top = w.iter().map(|w| w.amplitude).fold(0.0, f32::max);
            assert!(w[..6].iter().any(|x| x.amplitude == top));
            // Deep-water dispersion.
            assert!(
                w.iter()
                    .all(|w| (w.omega * w.omega - 9.81 * w.k).abs() < 1e-3 * w.k)
            );
        }
    }

    #[test]
    fn the_uniform_has_the_shaders_layout() {
        // SeaParams: five vec4, 16 waves of two vec4, 32 vec4 of outline points.
        assert_eq!(UNIFORM_SIZE, 80 + 16 * 32 + 32 * 16);
    }
}
