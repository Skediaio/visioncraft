//! Material textures: RGBA8 in, a full mip chain generated on the GPU.

/// How the RGBA8 bytes are encoded. `Srgb` for colour (base colour,
/// emissive), `Linear` for data (normal, roughness, metallic, occlusion).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorSpace {
    #[default]
    Srgb,
    Linear,
}

/// An RGBA8 image, rows top to bottom, not premultiplied.
#[derive(Clone, Copy, Debug)]
pub struct TextureDesc<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba8: &'a [u8],
    pub color_space: ColorSpace,
    /// Repeat (tile) outside 0..1; otherwise clamp to the edge.
    pub repeat: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(pub(crate) u32);

pub(crate) struct GpuTexture {
    pub view: wgpu::TextureView,
    pub repeat: bool,
}

/// Levels of a full mip chain down to 1x1.
pub fn mip_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

pub(crate) fn format(space: ColorSpace) -> wgpu::TextureFormat {
    match space {
        ColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        ColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
    }
}

/// The mip blit: one pipeline per texture format.
pub(crate) struct Mipmapper {
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::RenderPipeline; 2],
    sampler: wgpu::Sampler,
}

impl Mipmapper {
    pub fn new(device: &wgpu::Device, module: &wgpu::ShaderModule) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mip"),
            entries: &[
                crate::texture_entry(
                    0,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D2,
                    false,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipelines = [ColorSpace::Srgb, ColorSpace::Linear].map(|space| {
            crate::pipeline(
                device,
                crate::PipelineDesc {
                    label: "mip",
                    module,
                    vs: "vs_fullscreen",
                    fs: Some("fs_main"),
                    groups: &[Some(&layout)],
                    buffers: &[],
                    target: Some(format(space).into()),
                    depth: None,
                    cull: None,
                    samples: 1,
                    constants: &[],
                },
            )
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mip"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            pipelines,
            sampler,
        }
    }

    /// Uploads level 0 and draws every further level from the one above.
    pub fn upload(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        desc: &TextureDesc,
    ) -> Result<GpuTexture, String> {
        let (w, h) = (desc.width, desc.height);
        let max = device.limits().max_texture_dimension_2d;
        if w == 0 || h == 0 || w > max || h > max {
            return Err(format!("texture size {w}x{h} outside 1..={max}"));
        }
        if desc.rgba8.len() != (w * h * 4) as usize {
            return Err(format!(
                "{} bytes for a {w}x{h} RGBA8 texture (expected {})",
                desc.rgba8.len(),
                w * h * 4
            ));
        }
        let levels = mip_count(w, h);
        let fmt = format(desc.color_space);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("material texture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            desc.rgba8,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let level = |i| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("mip level"),
                base_mip_level: i,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let pipeline = &self.pipelines[(desc.color_space == ColorSpace::Linear) as usize];
        let mut encoder = device.create_command_encoder(&Default::default());
        for i in 1..levels {
            let source = level(i - 1);
            let target = level(i);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mip"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            let mut pass = crate::color_pass(&mut encoder, "mip", &target, None, None);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
        Ok(GpuTexture {
            view: texture.create_view(&Default::default()),
            repeat: desc.repeat,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_counts() {
        assert_eq!(mip_count(1, 1), 1);
        assert_eq!(mip_count(2, 2), 2);
        assert_eq!(mip_count(256, 256), 9);
        assert_eq!(mip_count(1024, 1), 11);
        assert_eq!(mip_count(3, 5), 3); // 5 -> 2 -> 1
        assert_eq!(mip_count(0, 0), 1);
    }
}
