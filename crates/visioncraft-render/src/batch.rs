//! Instances of every item in one buffer, and the frame's batches gathered from it on the GPU.
//!
//! Every item keeps its instance matrices in a range of one shared buffer ([`InstancePool`])
//! instead of a buffer of its own. A frame plans its passes' draws first (`lib.rs`, `plan_items`):
//! the visible instances of all items that share a mesh, a material and a pipeline become one
//! draw, their pool indices listed one after another. [`Gather`] copies those instances into a
//! frame buffer in that order with one compute dispatch before the first pass, so a scene of
//! hundreds of plantings, rocks and furniture pieces costs a draw per kind of thing instead of a
//! draw per item (each draw is a few calls into the browser's WebGPU).

use std::ops::Range;

/// Bytes of one instance (a 4x4 float matrix).
pub const INSTANCE_BYTES: u64 = 64;

/// Ranges of a buffer of `capacity` instances handed out and given back.
#[derive(Default)]
pub struct Ranges {
    capacity: u32,
    /// The first instance never handed out.
    end: u32,
    free: Vec<Range<u32>>,
}

impl Ranges {
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    /// The start of `n` free instances, or `None` when there is no room.
    pub fn alloc(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        // The smallest free range that fits.
        if let Some((i, _)) = self
            .free
            .iter()
            .enumerate()
            .filter(|(_, r)| r.len() >= n as usize)
            .min_by_key(|(_, r)| r.len())
        {
            let r = self.free.swap_remove(i);
            if r.len() > n as usize {
                self.free.push(r.start + n..r.end);
            }
            return Some(r.start);
        }
        (self.end + n <= self.capacity).then(|| {
            self.end += n;
            self.end - n
        })
    }

    pub fn release(&mut self, start: u32, n: u32) {
        if n == 0 {
            return;
        }
        if start + n == self.end {
            self.end = start;
            // Free ranges now at the end join it.
            while let Some(i) = self.free.iter().position(|r| r.end == self.end) {
                self.end = self.free.swap_remove(i).start;
            }
        } else {
            self.free.push(start..start + n);
        }
    }
}

/// One buffer for all items' instances, handed out in ranges.
pub struct InstancePool {
    pub buffer: wgpu::Buffer,
    pub ranges: Ranges,
    /// Bumped whenever `buffer` is replaced (bind groups that hold it are made again).
    pub generation: u32,
}

impl InstancePool {
    pub fn new(device: &wgpu::Device, capacity: u32) -> Self {
        Self {
            buffer: pool_buffer(device, capacity),
            ranges: Ranges::new(capacity),
            generation: 0,
        }
    }

    /// A new, empty buffer of at least `needed` instances (every range is handed out again:
    /// the caller writes all its instances anew).
    pub fn grow(&mut self, device: &wgpu::Device, needed: u32) {
        let capacity = needed
            .saturating_mul(3)
            .div_ceil(2)
            .max(self.ranges.capacity.saturating_mul(2))
            .next_power_of_two();
        self.buffer = pool_buffer(device, capacity);
        self.ranges = Ranges::new(capacity);
        self.generation = self.generation.wrapping_add(1);
    }
}

fn pool_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instance pool"),
        size: u64::from(capacity.max(1)) * INSTANCE_BYTES,
        usage: wgpu::BufferUsages::VERTEX
            | wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

const GATHER_WGSL: &str = r#"
// The frame's batched instances: out[i] = pool[idx[i + 4]], idx[0] the count. Copied as bits.
@group(0) @binding(0) var<storage, read> pool: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read> idx: array<u32>;
@group(0) @binding(2) var<storage, read_write> out: array<vec4<u32>>;

@compute @workgroup_size(64)
fn cs_gather(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x + id.y * 4194240u;
    if i >= idx[0] {
        return;
    }
    let at = idx[i + 4u] * 4u;
    for (var k = 0u; k < 4u; k++) {
        out[i * 4u + k] = pool[at + k];
    }
}
"#;

/// Workgroups along x before the dispatch wraps into y (the limit is 65,535 a dimension).
const ROW: u32 = 65_535;

/// The compute pass that copies a frame's batched instances out of the pool.
pub struct Gather {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    /// `[count, 0, 0, 0, pool index...]`.
    idx: wgpu::Buffer,
    idx_capacity: u32,
    /// The gathered instances, drawn as instance vertex buffer.
    pub out: wgpu::Buffer,
    out_capacity: u32,
    bind: Option<(u32, wgpu::BindGroup)>,
}

impl Gather {
    pub fn new(device: &wgpu::Device) -> Self {
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gather"),
            entries: &[storage(0, true), storage(1, true), storage(2, false)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gather"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gather"),
            source: wgpu::ShaderSource::Wgsl(GATHER_WGSL.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gather"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_gather"),
            compilation_options: Default::default(),
            cache: None,
        });
        let (idx_capacity, out_capacity) = (1024, 1024);
        Self {
            pipeline,
            layout,
            idx: idx_buffer(device, idx_capacity),
            idx_capacity,
            out: out_buffer(device, out_capacity),
            out_capacity,
            bind: None,
        }
    }

    /// Copies `pool[indices[i]]` to `out[i]` (none when `indices` is empty).
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        pool: &InstancePool,
        indices: &[u32],
    ) {
        if indices.is_empty() {
            return;
        }
        let n = indices.len() as u32;
        if n + 4 > self.idx_capacity {
            self.idx_capacity = (n + 4).next_power_of_two();
            self.idx = idx_buffer(device, self.idx_capacity);
            self.bind = None;
        }
        if n > self.out_capacity {
            self.out_capacity = n.next_power_of_two();
            self.out = out_buffer(device, self.out_capacity);
            self.bind = None;
        }
        if self
            .bind
            .as_ref()
            .is_none_or(|(g, _)| *g != pool.generation)
        {
            fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
                wgpu::BindGroupEntry {
                    binding,
                    resource: buffer.as_entire_binding(),
                }
            }
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gather"),
                layout: &self.layout,
                entries: &[
                    entry(0, &pool.buffer),
                    entry(1, &self.idx),
                    entry(2, &self.out),
                ],
            });
            self.bind = Some((pool.generation, bind));
        }
        let mut words = Vec::with_capacity(indices.len() + 4);
        words.extend_from_slice(&[n, 0, 0, 0]);
        words.extend_from_slice(indices);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        queue.write_buffer(&self.idx, 0, &bytes);
        let Some((_, bind)) = &self.bind else { return };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gather"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        let groups = n.div_ceil(64);
        pass.dispatch_workgroups(groups.min(ROW), groups.div_ceil(ROW), 1);
    }
}

fn idx_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gather indices"),
        size: u64::from(capacity) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn out_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gathered instances"),
        size: u64::from(capacity) * INSTANCE_BYTES,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_handed_out_once_and_reused() {
        let mut r = Ranges::new(100);
        let a = r.alloc(40).unwrap();
        let b = r.alloc(40).unwrap();
        assert_eq!((a, b), (0, 40));
        assert_eq!(r.alloc(30), None);
        assert_eq!(r.alloc(0), Some(0));
        r.release(a, 40);
        // The freed range is used again, split.
        assert_eq!(r.alloc(10), Some(0));
        assert_eq!(r.alloc(30), Some(10));
        assert_eq!(r.alloc(25), None);
        // Freeing the last range gives the end back, with free ranges just before it.
        r.release(10, 30);
        r.release(b, 40);
        assert_eq!(r.alloc(90), Some(10));
    }

    #[test]
    fn the_gather_shader_parses() {
        let module = naga::front::wgsl::parse_str(GATHER_WGSL)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(GATHER_WGSL)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("valid");
    }
}
