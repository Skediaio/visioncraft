//! Plan underlays: images drawn flat on the ground, unlit and see-through (`underlay.wgsl`).

use crate::texture::TextureId;
use wgpu::util::DeviceExt;

/// Bytes of one vertex: position (3 floats), texture coordinate (2), opacity (1).
pub(crate) const VERTEX_BYTES: u64 = 24;

/// One image on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnderlayQuad {
    /// An image uploaded with `Renderer::add_texture` (sRGB, clamped).
    pub texture: TextureId,
    /// The image's corners in world space: top-left, top-right, bottom-right, bottom-left.
    pub corners: [[f32; 3]; 4],
    /// 0 (invisible) to 1 (opaque).
    pub opacity: f32,
}

/// What is drawn: one vertex buffer (six vertices a quad) and one bind group a quad.
#[derive(Default)]
pub(crate) struct Drawn {
    pub buffer: Option<wgpu::Buffer>,
    pub binds: Vec<wgpu::BindGroup>,
}

/// The six vertices of a quad, two triangles.
pub(crate) fn vertices(quad: &UnderlayQuad) -> [[f32; 6]; 6] {
    let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let opacity = quad.opacity.clamp(0.0, 1.0);
    [0, 1, 2, 0, 2, 3].map(|i| {
        let p = quad.corners[i];
        [p[0], p[1], p[2], uv[i][0], uv[i][1], opacity]
    })
}

pub(crate) fn build<'a>(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    quads: &[UnderlayQuad],
    view: impl Fn(TextureId) -> Option<&'a wgpu::TextureView>,
) -> Drawn {
    let mut bytes = Vec::new();
    let mut binds = Vec::new();
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("underlay"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    for quad in quads {
        let Some(view) = view(quad.texture) else {
            continue;
        };
        for v in vertices(quad).iter().flatten() {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        binds.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("underlay"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        }));
    }
    let buffer = (!bytes.is_empty()).then(|| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("underlays"),
            contents: &bytes,
            usage: wgpu::BufferUsages::VERTEX,
        })
    });
    Drawn { buffer, binds }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quad_is_two_triangles_with_the_image_the_right_way_up() {
        let q = UnderlayQuad {
            texture: TextureId(0),
            corners: [
                [0.0, 0.1, 0.0],
                [4.0, 0.1, 0.0],
                [4.0, 0.1, 3.0],
                [0.0, 0.1, 3.0],
            ],
            opacity: 0.5,
        };
        let v = vertices(&q);
        // top-left of the image (uv 0, 0) at the first corner, bottom-right (1, 1) at the third
        assert_eq!(v[0], [0.0, 0.1, 0.0, 0.0, 0.0, 0.5]);
        assert_eq!(v[2], [4.0, 0.1, 3.0, 1.0, 1.0, 0.5]);
        assert_eq!(v[5], [0.0, 0.1, 3.0, 0.0, 1.0, 0.5]);
    }
}
