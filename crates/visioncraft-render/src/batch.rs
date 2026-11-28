//! Instances of every item in one buffer, and the frame's batches gathered from it on the GPU;
//! the meshes' vertices and indices in shared arenas.
//!
//! Every item keeps its instance matrices in a range of one shared buffer ([`InstancePool`])
//! instead of a buffer of its own. A frame plans its passes' draws first (`lib.rs`, `plan_items`):
//! the visible instances of all items that share a mesh, a material and a pipeline become one
//! draw, their pool indices listed one after another. [`Gather`] copies those instances into a
//! frame buffer in that order with one compute dispatch before the first pass, so a scene of
//! hundreds of plantings, rocks and furniture pieces costs a draw per kind of thing instead of a
//! draw per item (each draw is a few calls into the browser's WebGPU).
//!
//! The static meshes' vertices and indices lie in one vertex buffer and one index buffer too
//! ([`MeshRanges`]): a mesh holds ranges of both, and a draw names its index range and its base
//! vertex, so a pass binds the two buffers once instead of per mesh.
//!
//! A batch spans items with several materials (the same texture set): the gather gives each
//! copied instance its material's row and the batch's mesh in the spare components of its
//! matrix ([`GatherList`]), so a draw needs no per-material binding.

use std::ops::Range;

/// Bytes of one instance (a 4x4 float matrix).
pub const INSTANCE_BYTES: u64 = 64;

/// Ranges of a buffer of `capacity` elements handed out and given back. A freed range joins its
/// free neighbours, so what is handed out again is never more than what is free.
#[derive(Default)]
pub struct Ranges {
    capacity: u32,
    /// The first element never handed out.
    end: u32,
    /// By start; no two touch.
    free: Vec<Range<u32>>,
}

impl Ranges {
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    /// The start of `n` free elements, or `None` when there is no room.
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
            let r = self.free.remove(i);
            if r.len() > n as usize {
                self.free.insert(i, r.start + n..r.end);
            }
            return Some(r.start);
        }
        (self.end + n <= self.capacity).then(|| {
            self.end += n;
            self.end - n
        })
    }

    /// Whether `alloc(n)` has room now.
    pub fn fits(&self, n: u32) -> bool {
        n == 0
            || self.end.saturating_add(n) <= self.capacity
            || self.free.iter().any(|r| r.len() >= n as usize)
    }

    /// The elements handed out so far from the start (the high-water mark).
    #[cfg(test)]
    pub fn used(&self) -> u32 {
        self.end
    }

    /// A capacity that has room for `n` more elements: this one doubled, or just enough, at
    /// most `max`; `None` when even the elements handed out do not fit `max`.
    pub fn grown(&self, n: u32, max: u32) -> Option<u32> {
        let need = self.end.saturating_add(n);
        (need <= max).then(|| need.max(self.capacity.saturating_mul(2)).max(1))
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// The buffer grew (or was made): `capacity` elements can be handed out.
    pub fn set_capacity(&mut self, capacity: u32) {
        self.capacity = capacity;
    }

    pub fn release(&mut self, start: u32, n: u32) {
        if n == 0 {
            return;
        }
        let (mut lo, mut hi) = (start, start + n);
        // The free range just before it, then the one just after, join it.
        if let Some(i) = self.free.iter().position(|r| r.end == lo) {
            lo = self.free.remove(i).start;
        }
        if let Some(i) = self.free.iter().position(|r| r.start == hi) {
            hi = self.free.remove(i).end;
        }
        if hi == self.end {
            self.end = lo;
            return;
        }
        let at = self.free.partition_point(|r| r.start < lo);
        self.free.insert(at, lo..hi);
    }
}

/// `count` elements from `first`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub first: u32,
    pub count: u32,
}

impl Span {
    pub fn new(first: u32, count: u32) -> Self {
        Self { first, count }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.count as usize
    }

    pub fn range(&self) -> Range<u32> {
        self.first..self.first + self.count
    }
}

/// Where a mesh's data lies: the arena (`Renderer::arenas`) and its spans, in vertices and in
/// indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    pub arena: u32,
    pub vertices: Span,
    pub indices: Span,
}

/// Vertices, and indices, an arena is given at least when it first grows (so a scene of many
/// small meshes does not regrow its buffers at every one).
const ARENA_FLOOR: u32 = 1 << 16;

/// The ranges of every arena: its vertex ranges and its index ranges. A mesh takes a range of
/// both in the same arena.
#[derive(Default)]
pub struct MeshRanges {
    arenas: Vec<(Ranges, Ranges)>,
}

impl MeshRanges {
    pub fn len(&self) -> usize {
        self.arenas.len()
    }

    /// Elements handed out from the starts of all arenas (the high-water mark).
    #[cfg(test)]
    pub fn used(&self) -> u32 {
        self.arenas.iter().map(|(v, i)| v.used() + i.used()).sum()
    }

    /// Starts arena `len()` without capacity (`grow` gives it one).
    pub fn push(&mut self) -> usize {
        self.arenas.push((Ranges::default(), Ranges::default()));
        self.arenas.len() - 1
    }

    /// Where a mesh of `vertices` vertices and `indices` indices fits: a free range of an earlier
    /// arena first, then the room at the end of the last one; `None` in none.
    pub fn alloc(&mut self, vertices: u32, indices: u32) -> Option<Placement> {
        let (n, (last, earlier)) = (self.arenas.len(), self.arenas.split_last_mut()?);
        let place = |a: usize, (v, i): &mut (Ranges, Ranges)| {
            if !(v.fits(vertices) && i.fits(indices)) {
                return None;
            }
            let vs = v.alloc(vertices)?;
            let is = i.alloc(indices)?;
            Some(Placement {
                arena: a as u32,
                vertices: Span::new(vs, vertices),
                indices: Span::new(is, indices),
            })
        };
        for (a, ranges) in earlier.iter_mut().enumerate() {
            if let Some(p) = place(a, ranges) {
                return Some(p);
            }
        }
        place(n - 1, last)
    }

    pub fn release(&mut self, p: &Placement) {
        let Some((v, i)) = self.arenas.get_mut(p.arena as usize) else {
            return;
        };
        v.release(p.vertices.first, p.vertices.count);
        i.release(p.indices.first, p.indices.count);
    }

    /// The capacities arena `i` needs to take a mesh of `vertices` vertices and `indices` indices
    /// more (as `Ranges::grown`, `max` the vertex and the index cap); `None` when it cannot, and
    /// another arena has to start.
    pub fn grow(
        &self,
        i: usize,
        vertices: u32,
        indices: u32,
        [max_v, max_i]: [u32; 2],
    ) -> Option<(u32, u32)> {
        let (v, ix) = self.arenas.get(i)?;
        let cv = if v.fits(vertices) {
            v.capacity()
        } else {
            v.grown(vertices, max_v)?
        };
        let ci = if ix.fits(indices) {
            ix.capacity()
        } else {
            ix.grown(indices, max_i)?
        };
        Some((
            cv.max(ARENA_FLOOR).min(max_v),
            ci.max(ARENA_FLOOR).min(max_i),
        ))
    }

    /// Arena `i`'s buffers were made with these capacities.
    pub fn set_capacity(&mut self, i: usize, vertices: u32, indices: u32) {
        let Some((v, ix)) = self.arenas.get_mut(i) else {
            return;
        };
        v.set_capacity(vertices);
        ix.set_capacity(indices);
    }
}

/// Bytes of a terrain chunk's layer weights per vertex (eight u8, vertex slot 2).
pub const WEIGHT_BYTES: u64 = 8;

/// One vertex buffer and one index buffer for many meshes: an arena of [`MeshRanges`], with a
/// buffer of layer weights for the terrain chunks among them.
pub struct MeshArena {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    /// A terrain chunk's layer weights, eight u8 per vertex of `vertices` at the vertex's own
    /// index (zero for every other mesh): the fetch of vertex slot 2 follows the same vertex
    /// index as the others, so they cannot live in a buffer of the chunk's own.
    pub weights: wgpu::Buffer,
    /// Bytes of one vertex.
    stride: u64,
    /// Vertices and indices the buffers hold.
    capacity: (u32, u32),
}

impl MeshArena {
    /// Buffers for `capacity` vertices of `stride` bytes and as many indices.
    pub fn new(device: &wgpu::Device, capacity: (u32, u32), stride: u64) -> Self {
        let buffer = |label: &str, bytes: u64, usage: wgpu::BufferUsages| {
            arena_buffer(device, label, bytes, usage)
        };
        Self {
            vertices: buffer(
                "vertex arena",
                u64::from(capacity.0) * stride,
                wgpu::BufferUsages::VERTEX,
            ),
            indices: buffer(
                "index arena",
                u64::from(capacity.1) * 4,
                wgpu::BufferUsages::INDEX,
            ),
            weights: buffer(
                "weight arena",
                u64::from(capacity.0) * WEIGHT_BYTES,
                wgpu::BufferUsages::VERTEX,
            ),
            stride,
            capacity,
        }
    }

    /// Larger buffers (for `capacity` vertices and indices); what the old ones hold is copied to
    /// the start of the new ones, so every range keeps its place. Recorded in `encoder`: submit
    /// it before a frame uses the arena.
    pub fn grow(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        capacity: (u32, u32),
    ) {
        let mut grown = MeshArena::new(device, capacity, self.stride);
        std::mem::swap(&mut grown.vertices, &mut self.vertices);
        std::mem::swap(&mut grown.indices, &mut self.indices);
        std::mem::swap(&mut grown.weights, &mut self.weights);
        encoder.copy_buffer_to_buffer(&grown.vertices, 0, &self.vertices, 0, grown.vertices.size());
        encoder.copy_buffer_to_buffer(&grown.indices, 0, &self.indices, 0, grown.indices.size());
        encoder.copy_buffer_to_buffer(&grown.weights, 0, &self.weights, 0, grown.weights.size());
        self.capacity = capacity;
    }

    /// The vertices `first..` of the arena get `bytes` (whole vertices).
    pub fn write_vertices(&self, queue: &wgpu::Queue, first: u32, bytes: &[u8]) {
        if !bytes.is_empty() {
            queue.write_buffer(&self.vertices, u64::from(first) * self.stride, bytes);
        }
    }

    /// The layer weights of the vertices `first..` of the arena get `bytes` (eight u8 each).
    pub fn write_weights(&self, queue: &wgpu::Queue, first: u32, bytes: &[u8]) {
        if !bytes.is_empty() {
            queue.write_buffer(&self.weights, u64::from(first) * WEIGHT_BYTES, bytes);
        }
    }

    /// The indices `first..` of the arena get `bytes` (u32 each).
    pub fn write_indices(&self, queue: &wgpu::Queue, first: u32, bytes: &[u8]) {
        if !bytes.is_empty() {
            queue.write_buffer(&self.indices, u64::from(first) * 4, bytes);
        }
    }
}

fn arena_buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage: usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
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

/// Instances per batch in the gathered buffer: one workgroup of the gather copies one batch,
/// so a batch starts every `BATCH` instances and the lanes past its end are never drawn.
pub const BATCH: u32 = 64;

const GATHER_WGSL: &str = r#"
// The frame's batched instances, one workgroup of 64 lanes per batch b (lib.rs `GatherList`):
// out[b * 64 + lane] = pool[p] as bits, with the material's row in m1.w and the batch's mesh
// in m2.w (scene.wgsl `model`). idx: [batches, instances, 0, 0], then per batch (its first
// instance in the stream, its count), then per batch its mesh, then per instance its pool
// index, then per instance its material row.
@group(0) @binding(0) var<storage, read> pool: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read> idx: array<u32>;
@group(0) @binding(2) var<storage, read_write> out: array<vec4<u32>>;

@compute @workgroup_size(64)
fn cs_gather(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let b = group.x + group.y * 65535u;
    let batches = idx[0];
    if b >= batches {
        return;
    }
    let first = idx[4u + 2u * b];
    let n = idx[5u + 2u * b];
    if lane >= n {
        return;
    }
    let s = first + lane;
    let base = 4u + 3u * batches;
    let at = idx[base + s] * 4u;
    let o = (b * 64u + lane) * 4u;
    for (var k = 0u; k < 4u; k++) {
        out[o + k] = pool[at + k];
    }
    out[o + 1u].w = idx[base + idx[1] + s];
    out[o + 2u].w = idx[4u + 2u * batches + b];
}
"#;

/// A frame's gathered instances (`lib.rs` `plan_items`, `Batches::emit`): each one's index in
/// the pool and its material's row in the material table; and per batch, in draw order, where
/// its instances start in the stream, how many there are and its mesh.
#[derive(Default)]
pub struct GatherList {
    pub indices: Vec<u32>,
    pub materials: Vec<u32>,
    /// (first in the stream, count, mesh id).
    pub batches: Vec<(u32, u32, u32)>,
}

impl GatherList {
    /// Closes the batch whose instances were appended since there were `start` in the stream:
    /// where they lie in the gathered buffer. The gather copies one workgroup of `BATCH` lanes
    /// per entry, so the batch gets an entry per 64 of its instances, in order; its range is
    /// contiguous all the same.
    pub fn batch(&mut self, start: u32, mesh: u32) -> std::ops::Range<u32> {
        let n = self.indices.len() as u32 - start;
        let first = self.batches.len() as u32 * BATCH;
        for at in (start..start + n).step_by(BATCH as usize) {
            let count = (start + n - at).min(BATCH);
            self.batches.push((at, count, mesh));
        }
        first..first + n
    }
}

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

    /// Copies every instance of `list` out of the pool into `out`, batch by batch (nothing
    /// when there is none).
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        pool: &InstancePool,
        list: &GatherList,
    ) {
        let batches = list.batches.len() as u32;
        if batches == 0 {
            return;
        }
        let words = 4 + 3 * batches as usize + 2 * list.indices.len();
        if words > self.idx_capacity as usize {
            self.idx_capacity = words.next_power_of_two() as u32;
            self.idx = idx_buffer(device, self.idx_capacity);
            self.bind = None;
        }
        let out_instances = batches * BATCH;
        if out_instances > self.out_capacity {
            self.out_capacity = out_instances.next_power_of_two();
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
        let mut words = Vec::with_capacity(words);
        words.extend_from_slice(&[batches, list.indices.len() as u32, 0, 0]);
        for &(first, n, _) in &list.batches {
            words.extend_from_slice(&[first, n]);
        }
        words.extend(list.batches.iter().map(|&(_, _, mesh)| mesh));
        words.extend_from_slice(&list.indices);
        words.extend_from_slice(&list.materials);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        queue.write_buffer(&self.idx, 0, &bytes);
        let Some((_, bind)) = &self.bind else { return };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gather"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups(batches.min(ROW), batches.div_ceil(ROW), 1);
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

    /// A freed range joins its free neighbours: what is handed out again never needs more room
    /// than what is free.
    #[test]
    fn freed_ranges_join_their_neighbours() {
        let mut r = Ranges::new(100);
        let a = r.alloc(30).unwrap();
        let b = r.alloc(30).unwrap();
        let c = r.alloc(30).unwrap();
        assert_eq!((a, b, c), (0, 30, 60));
        r.release(a, 30);
        r.release(c, 30);
        // Two free ranges of 30, with 30 handed out between them: nothing of 60 fits.
        assert_eq!(r.alloc(60), None);
        r.release(b, 30);
        assert_eq!(r.used(), 0, "everything given back to the start");
        assert_eq!(r.alloc(90), Some(0));
        // A replacement that needs more than its range: a new one elsewhere, then the old freed.
        r.release(0, 90);
        let a = r.alloc(40).unwrap();
        let b = r.alloc(50).unwrap();
        r.release(a, 40);
        assert!(!r.fits(60));
        r.release(b, 50);
        assert!(r.fits(60));
        assert_eq!(r.alloc(60), Some(0));
    }

    /// A mesh's vertices and indices share an arena; an arena full at its cap is left for a new
    /// one, and a freed range of an earlier arena is used first.
    #[test]
    fn mesh_ranges_keep_each_arena_for_a_mesh_s_vertices_and_indices() {
        let cap = [24u32, 36];
        let mut m = MeshRanges::default();
        assert!(m.alloc(24, 36).is_none(), "no arena yet");
        m.push();
        assert_eq!(m.grow(0, 24, 36, cap), Some((24, 36)));
        m.set_capacity(0, 24, 36);
        let a = m.alloc(24, 36).unwrap();
        assert_eq!(
            a,
            Placement {
                arena: 0,
                vertices: Span::new(0, 24),
                indices: Span::new(0, 36)
            }
        );
        assert!(m.alloc(1, 1).is_none(), "full");
        // Full at its cap: a second arena.
        assert_eq!(m.grow(0, 1, 1, cap), None);
        m.push();
        // A new arena is given the cap it may have.
        assert_eq!(m.grow(1, 1, 1, cap), Some((24, 36)));
        m.set_capacity(1, 1, 1);
        let b = m.alloc(1, 1).unwrap();
        assert_eq!(b.arena, 1);
        // A freed range of the first arena is used first, split, until its last elements go.
        m.release(&a);
        let c = m.alloc(20, 30).unwrap();
        assert_eq!(
            (c.arena, c.vertices, c.indices),
            (0, Span::new(0, 20), Span::new(0, 30))
        );
        let d = m.alloc(4, 6).unwrap();
        assert_eq!(
            (d.arena, d.vertices, d.indices),
            (0, Span::new(20, 4), Span::new(30, 6))
        );
        assert!(m.alloc(1, 1).is_none(), "all handed out again");
        assert_eq!(m.used(), 24 + 36 + 2);
        // Given back out of order, everything returns to the start.
        for p in [c, b, d] {
            m.release(&p);
        }
        assert_eq!(m.used(), 0);
        // Growth keeps the ranges: at least the floor, or twice the capacity, or just enough,
        // and never past the cap.
        let big = [1 << 20, 1 << 20];
        let a = m.alloc(10, 12).unwrap();
        assert_eq!(
            m.grow(0, 1 << 16, 1 << 16, [1 << 20, 45]),
            None,
            "the indices cannot"
        );
        assert_eq!(m.grow(0, 1 << 16, 1 << 16, big), Some((65546, 65548)));
        m.set_capacity(0, 65546, 65548);
        let b = m.alloc(1 << 16, 1 << 16).unwrap();
        assert_eq!(
            (a.vertices, a.indices, b.vertices.first, b.indices.first),
            (Span::new(0, 10), Span::new(0, 12), 10, 12)
        );
        assert_eq!(
            m.grow(0, 1 << 16, 1 << 16, big),
            Some((131092, 131096)),
            "twice"
        );
        assert_eq!(m.grow(0, 1 << 20, 10, big), None, "past the cap");
    }

    /// Replacing meshes thousands of times (the new range first, then the old one freed) leaves
    /// the high-water mark within twice the live data plus three meshes.
    #[test]
    fn fragmentation_stays_bounded_after_many_replacements() {
        let mut m = MeshRanges::default();
        m.push();
        m.set_capacity(0, 1 << 26, 1 << 26);
        let mut seed = 1u32;
        let mut next = |lo: u32, hi: u32| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            lo + seed % (hi - lo)
        };
        let mut live: Vec<Placement> = Vec::new();
        for _ in 0..2000 {
            let v = next(24, 4800);
            live.push(m.alloc(v, v / 2).unwrap());
        }
        let live_sum = |live: &[Placement]| {
            live.iter()
                .map(|p| p.vertices.len() + p.indices.len())
                .sum::<usize>()
        };
        for step in 0..20_000 {
            let i = next(0, live.len() as u32) as usize;
            let v = next(24, 4800);
            let n = m.alloc(v, v / 2).unwrap();
            m.release(&live[i]);
            live[i] = n;
            if step % 2000 == 1999 {
                assert!(
                    m.used() as usize <= live_sum(&live) * 2 + 4800 * 3,
                    "{} used for {} live (step {step})",
                    m.used(),
                    live_sum(&live)
                );
            }
        }
    }

    /// A batch takes one entry per 64 of its instances, each a block of `BATCH` gathered
    /// instances, in the order the batches close.
    #[test]
    fn batches_lie_in_blocks_of_64_gathered_instances() {
        let mut list = GatherList::default();
        let add = |list: &mut GatherList, mesh: u32, rows: &[u32]| {
            let start = list.indices.len() as u32;
            for (i, &row) in rows.iter().enumerate() {
                list.indices.push(100 + i as u32);
                list.materials.push(row);
            }
            list.batch(start, mesh)
        };
        assert_eq!(add(&mut list, 7, &[1, 1, 2]), 0..3);
        assert_eq!(add(&mut list, 9, &[3; 70]), 64..134);
        assert_eq!(add(&mut list, 7, &[4]), 192..193);
        assert_eq!(
            list.batches,
            vec![(0, 3, 7), (3, 64, 9), (67, 6, 9), (73, 1, 7)]
        );
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
