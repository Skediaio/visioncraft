//! Distance haze (CPU twin of common.wgsl's fog, for tests) and the physical
//! sky's look-up tables (atmosphere.wgsl): transmittance and multiple
//! scattering once, the sky-view LUT whenever the sun moves (a 192x108
//! pass, well under a millisecond).

use crate::{PipelineDesc, pipeline};

/// Haze in the air between the camera and what it sees: exponential height
/// fog (see common.wgsl). Applied to every surface, transparent ones too, and
/// to the sky near the horizon, so the horizon line dissolves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Atmosphere {
    /// Extinction at the ground (the ground height, or 0) per kilometre:
    /// 0 none, 0.4 clear summer haze (visibility ~10 km, Koschmieder
    /// 3.912 / extinction), 2.5 (default) a hazy summer day that fades a
    /// flat horizon and gives a 100 m view depth, 4 mist, 20+ fog.
    pub haze: f32,
    /// Height over which the haze thins by a factor e (metres).
    pub haze_height_m: f32,
    /// How much sunlight the haze scatters towards the camera (0..1; the
    /// bright glow around a low sun).
    pub sun_glow: f32,
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self {
            haze: 2.5,
            haze_height_m: 150.0,
            sun_glow: 0.5,
        }
    }
}

/// Fraction of light that survives `t` metres from height `origin_y` along
/// a ray with vertical component `dir_y`, through haze of extinction
/// `sigma` (1/m) at `base_y` thinning with `falloff` (1/m): the closed form
/// of the optical depth integral (common.wgsl has the same).
pub fn fog_transmittance(
    origin_y: f32,
    dir_y: f32,
    t: f32,
    sigma: f32,
    falloff: f32,
    base_y: f32,
) -> f32 {
    if sigma <= 0.0 {
        return 1.0;
    }
    let start = sigma * (-falloff * (origin_y - base_y)).exp();
    let k = falloff * dir_y;
    let path = if (k * t).abs() > 1e-4 {
        (1.0 - (-k * t).min(80.0).exp()) / k
    } else {
        t
    };
    (-start * path).exp()
}

pub(crate) const SKY_VIEW: (u32, u32) = (192, 108);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(crate) struct PhysicalSky {
    pub sky_view: wgpu::TextureView,
    targets: [wgpu::TextureView; 3],
    binds: [wgpu::BindGroup; 3],
    pipelines: [wgpu::RenderPipeline; 3],
    luts_done: bool,
}

impl PhysicalSky {
    pub fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        globals_only: &wgpu::BindGroupLayout,
    ) -> Self {
        let texture = |label, (w, h): (u32, u32)| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let transmittance = texture("transmittance LUT", (256, 64));
        let multi = texture("multiple scattering LUT", (32, 32));
        let sky_view = texture("sky-view LUT", SKY_VIEW);
        let dummy = texture("LUT placeholder", (1, 1));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atmosphere"),
            entries: &[
                crate::texture_entry(
                    0,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D2,
                    false,
                ),
                crate::texture_entry(
                    1,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D2,
                    false,
                ),
                crate::sampler_entry(2, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atmosphere"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind = |a: &wgpu::TextureView, b: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("atmosphere"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(a),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(b),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let binds = [
            bind(&dummy, &dummy),
            bind(&transmittance, &dummy),
            bind(&transmittance, &multi),
        ];
        let pipelines = ["fs_transmittance", "fs_multi_scattering", "fs_sky_view"].map(|fs| {
            pipeline(
                device,
                PipelineDesc {
                    label: "atmosphere",
                    module,
                    vs: "vs_fullscreen",
                    fs: Some(fs),
                    groups: &[Some(globals_only), Some(&layout)],
                    buffers: &[],
                    target: Some(FORMAT.into()),
                    depth: None,
                    cull: None,
                    samples: 1,
                    constants: &[],
                    front_cw: false,
                },
            )
        });
        Self {
            targets: [transmittance, multi, sky_view.clone()],
            sky_view,
            binds,
            pipelines,
            luts_done: false,
        }
    }

    /// Records the sky-view pass (and, the first time, the two fixed LUTs);
    /// the globals must hold this frame's sun.
    pub fn update(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        globals: &wgpu::BindGroup,
        timer: Option<&crate::timing::Timer>,
    ) {
        let first = if self.luts_done { 2 } else { 0 };
        self.luts_done = true;
        for i in first..3 {
            let mut pass = crate::color_pass(
                encoder,
                "atmosphere",
                &self.targets[i],
                None,
                None,
                timer.and_then(|t| t.pass("sky")),
            );
            pass.set_pipeline(&self.pipelines[i]);
            pass.set_bind_group(0, globals, &[]);
            pass.set_bind_group(1, &self.binds[i], &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The closed form against a midpoint sum of the extinction.
    #[test]
    fn fog_integral_matches_numerical_integration() {
        let (sigma, falloff, base) = (0.002, 1.0 / 150.0, 0.0);
        for (origin, dir, t) in [
            (1.7, 0.0, 300.0),
            (1.7, -0.02, 80.0),
            (30.0, 0.3, 2000.0),
            (2.0, -0.5, 3.0),
            (5.0, 1.0, 1e-3),
        ] {
            let n = 20_000;
            let dt = t / n as f32;
            let mut depth = 0.0f64;
            for i in 0..n {
                let y = origin + dir * (i as f32 + 0.5) * dt;
                depth += (sigma * (-falloff * (y - base)).exp() * dt) as f64;
            }
            let numeric = (-depth).exp() as f32;
            let closed = fog_transmittance(origin, dir, t, sigma, falloff, base);
            assert!(
                (numeric - closed).abs() < 1e-4,
                "{origin} {dir} {t}: {numeric} vs {closed}"
            );
        }
        // Looking up, the haze thins out: a finite optical depth to infinity.
        let up = fog_transmittance(1.7, 0.5, 1e9, sigma, falloff, base);
        assert!(up > 0.4 && up < 1.0, "{up}");
        // Along the horizon it never ends.
        assert!(fog_transmittance(1.7, 0.0, 1e6, sigma, falloff, base) < 1e-6);
        assert_eq!(fog_transmittance(1.7, 0.0, 1e6, 0.0, falloff, base), 1.0);
    }
}
