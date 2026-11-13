//! GPU pass timings from timestamp queries, used only when the device was
//! created with `Features::TIMESTAMP_QUERY` (the renderer never requests it).
//! Each timed render pass writes a timestamp at its start and end; the pairs
//! are resolved at the end of the frame into one of three read-back buffers,
//! mapped at the start of the next `render` (after the caller submitted the
//! previous frame) and read once the map completes, so the numbers lag one
//! to three frames. Passes with the same label are summed.

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

/// Timestamps per frame (two per pass).
const MAX_QUERIES: u32 = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    /// Holds this frame's copy; map at the next `begin`.
    Copied,
    Mapping,
    Ready,
    Failed,
}

struct Slot {
    buffer: wgpu::Buffer,
    labels: Vec<&'static str>,
    state: Arc<Mutex<State>>,
}

pub(crate) struct Timer {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    ring: Vec<Slot>,
    /// Nanoseconds per timestamp tick.
    period: f32,
    current: Option<usize>,
    labels: RefCell<Vec<&'static str>>,
    /// Off for a frame that must not touch the ring (stills).
    pub paused: Cell<bool>,
    /// Milliseconds per label of the latest frame read back, first-seen order.
    pub last: Vec<(&'static str, f32)>,
}

impl Timer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Timer> {
        let on = device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        #[cfg(not(target_arch = "wasm32"))]
        if std::env::var_os("VISIONCRAFT_GPU_TIMINGS").is_some() {
            eprintln!(
                "gpu timings: {}",
                if on {
                    "on"
                } else {
                    "off: no TIMESTAMP_QUERY on this device"
                }
            );
        }
        if !on {
            return None;
        }
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("pass timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: MAX_QUERIES,
        });
        let bytes = MAX_QUERIES as u64 * 8;
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp resolve"),
            size: bytes,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let ring = (0..3)
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp read-back"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                labels: Vec::new(),
                state: Arc::new(Mutex::new(State::Free)),
            })
            .collect();
        Some(Timer {
            set,
            resolve,
            ring,
            period: queue.get_timestamp_period(),
            current: None,
            labels: RefCell::new(Vec::new()),
            paused: Cell::new(false),
            last: Vec::new(),
        })
    }

    /// Starts a frame: maps last frame's copy, reads finished ones and
    /// picks a free buffer for this frame (none free: this frame is untimed).
    pub fn begin(&mut self) {
        self.current = None;
        self.labels.borrow_mut().clear();
        if self.paused.get() {
            return;
        }
        for slot in &mut self.ring {
            let state = *slot.state.lock().expect("timer state");
            match state {
                State::Copied => {
                    *slot.state.lock().expect("timer state") = State::Mapping;
                    let flag = slot.state.clone();
                    slot.buffer
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |r| {
                            if let Ok(mut s) = flag.lock() {
                                *s = if r.is_ok() {
                                    State::Ready
                                } else {
                                    State::Failed
                                };
                            }
                        });
                }
                State::Ready => {
                    let mut out: Vec<(&'static str, f32)> = Vec::new();
                    if let Ok(view) = slot.buffer.slice(..).get_mapped_range() {
                        let ticks: Vec<u64> = view
                            .as_chunks::<8>()
                            .0
                            .iter()
                            .map(|b| u64::from_le_bytes(*b))
                            .collect();
                        for (i, label) in slot.labels.iter().enumerate() {
                            let ns =
                                ticks[2 * i + 1].saturating_sub(ticks[2 * i]) as f32 * self.period;
                            match out.iter_mut().find(|(l, _)| l == label) {
                                Some(e) => e.1 += ns * 1e-6,
                                None => out.push((label, ns * 1e-6)),
                            }
                        }
                    }
                    slot.buffer.unmap();
                    *slot.state.lock().expect("timer state") = State::Free;
                    self.last = out;
                }
                State::Failed => *slot.state.lock().expect("timer state") = State::Free,
                State::Free | State::Mapping => {}
            }
        }
        self.current = self
            .ring
            .iter()
            .position(|s| *s.state.lock().expect("timer state") == State::Free);
    }

    /// Timestamp writes for one pass, or `None` when this frame is untimed
    /// or full.
    pub fn pass(&self, label: &'static str) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.current?;
        let mut labels = self.labels.borrow_mut();
        let i = labels.len() as u32 * 2;
        if i + 2 > MAX_QUERIES {
            return None;
        }
        labels.push(label);
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some(i),
            end_of_pass_write_index: Some(i + 1),
        })
    }

    /// Resolves this frame's timestamps into its read-back buffer.
    pub fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(k) = self.current.take() else { return };
        let labels = std::mem::take(&mut *self.labels.borrow_mut());
        if labels.is_empty() {
            return;
        }
        let n = labels.len() as u32 * 2;
        encoder.resolve_query_set(&self.set, 0..n, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.ring[k].buffer, 0, n as u64 * 8);
        self.ring[k].labels = labels;
        *self.ring[k].state.lock().expect("timer state") = State::Copied;
    }
}
