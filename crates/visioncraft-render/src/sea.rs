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
}

/// Waves on MEDIUM and above; LOW has the first (longest) half.
pub const WAVES: usize = 16;
const OUTLINE: usize = 64;
const UNIFORM_SIZE: u64 = 4 * 16 + (WAVES as u64) * 32 + (OUTLINE as u64 / 2) * 16;
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

/// The wave set for a sea: wavelengths from 1.8 lambda_p down to lambda_p / 30 in a geometric
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
            let lambda = 1.8 * lp * ratio.powi(i as i32);
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
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
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
                multiply,
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

    /// The height the planar reflection mirrors about while the sea is seen from above it.
    pub(crate) fn sea_plane(&self) -> Option<f32> {
        self.sea
            .sea
            .as_ref()
            .filter(|s| self.eye.y > s.level)
            .map(|s| s.level)
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
            0.15 + 0.6 * s.wave_height_m,
        ]);
        b.floats(&[
            s.flow[0],
            s.flow[1],
            f32::from(u8::from(s.freezes)),
            n as f32,
        ]);
        // Wind ripples grow with the wind, and are all there is on a glassy sea.
        let ripple = (0.4 + 0.6 * self.wind.strength.clamp(0.0, 2.0)).min(1.5);
        b.floats(&[
            f32::from(u8::from(ground_above)),
            ripple,
            s.wave_height_m,
            1.0,
        ]);
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
        // SeaParams: four vec4, 16 waves of two vec4, 32 vec4 of outline points.
        assert_eq!(UNIFORM_SIZE, 64 + 16 * 32 + 32 * 16);
    }
}
