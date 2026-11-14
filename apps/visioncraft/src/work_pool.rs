//! Background work for loading (plant meshes, plant textures, block compression): natively a
//! pool of threads sized to the machine (its cores less two, so the frame's thread and the
//! system keep one each; at least one), the most urgent job first. On the web there are no
//! threads: a job runs at once (plant textures go to the texture worker instead, `plants`).
//!
//! A job's result does not depend on when or where it runs: callers keep every job a pure
//! function of its inputs, so a load comes out the same however the pool schedules it.

use std::sync::{Arc, Mutex};

/// Where a job's result lands.
pub type Slot<T> = Arc<Mutex<Option<T>>>;

/// Takes a job's result once it is there.
pub fn take<T>(slot: &Slot<T>) -> Option<T> {
    slot.lock().ok()?.take()
}

/// Threads in the pool.
pub fn size() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *N.get_or_init(|| {
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
            std::env::var("VISIONCRAFT_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(cores.saturating_sub(2))
                .max(1)
        })
    }
    #[cfg(target_arch = "wasm32")]
    1
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{Condvar, Mutex};

    type Job = Box<dyn FnOnce() + Send>;

    #[derive(Default)]
    pub struct Jobs {
        /// (priority, order of arrival, job): the smallest priority runs first.
        pub waiting: Vec<(f32, u64, Job)>,
        pub next: u64,
        pub running: usize,
        pub started: bool,
    }

    pub static JOBS: Mutex<Jobs> = Mutex::new(Jobs {
        waiting: Vec::new(),
        next: 0,
        running: 0,
        started: false,
    });
    pub static WAKE: Condvar = Condvar::new();

    pub fn lock() -> std::sync::MutexGuard<'static, Jobs> {
        JOBS.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn worker() {
        loop {
            let job = {
                let mut j = lock();
                loop {
                    let best = j
                        .waiting
                        .iter()
                        .enumerate()
                        .min_by(|a, b| a.1.0.total_cmp(&b.1.0).then(a.1.1.cmp(&b.1.1)))
                        .map(|(i, _)| i);
                    if let Some(i) = best {
                        j.running += 1;
                        break j.waiting.swap_remove(i).2;
                    }
                    j = WAKE.wait(j).unwrap_or_else(|e| e.into_inner());
                }
            };
            // A panicking job loses its result (the caller keeps waiting for nothing) but not
            // the thread.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            lock().running -= 1;
        }
    }

    pub fn push(priority: f32, job: Job) {
        let mut j = lock();
        if !j.started {
            j.started = true;
            for i in 0..super::size() {
                let _ = std::thread::Builder::new()
                    .name(format!("visioncraft work {i}"))
                    .spawn(worker);
            }
        }
        let n = j.next;
        j.next += 1;
        j.waiting.push((priority, n, job));
        drop(j);
        WAKE.notify_one();
    }
}

/// Runs `f` on the pool (`priority`: smaller runs sooner); on the web at once.
pub fn run<T: Send + 'static>(priority: f32, f: impl FnOnce() -> T + Send + 'static) -> Slot<T> {
    let slot = Slot::default();
    let out = slot.clone();
    let job = move || {
        let v = f();
        if let Ok(mut s) = out.lock() {
            *s = Some(v);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    native::push(priority, Box::new(job));
    #[cfg(target_arch = "wasm32")]
    {
        let _ = priority;
        job();
    }
    slot
}

/// Runs `f` on the pool and waits for it (a loader thread's heavy step, so that every heavy
/// step shares the pool's cores); `None` when it panicked. Never from a pool job.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_wait<T: Send + 'static>(
    priority: f32,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    native::push(
        priority,
        Box::new(move || {
            let _ = tx.send(f());
        }),
    );
    rx.recv().ok()
}

/// Jobs waiting or running: callers hand over more only while this is small, so that what is
/// most urgent at the time (the camera moves) is what runs next.
pub fn backlog() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let j = native::lock();
        j.waiting.len() + j.running
    }
    #[cfg(target_arch = "wasm32")]
    0
}

/// Runs `f` over `items` on scoped threads (natively, at most `size()` at once), results in
/// the items' order; on the web in turn.
pub fn map<I: Sync, T: Send>(items: &[I], f: impl Fn(&I) -> T + Sync) -> Vec<T> {
    #[cfg(not(target_arch = "wasm32"))]
    if items.len() > 1 {
        let f = &f;
        let mut out = Vec::with_capacity(items.len());
        for chunk in items.chunks(size().max(1)) {
            std::thread::scope(|s| {
                let hs: Vec<_> = chunk.iter().map(|i| s.spawn(move || f(i))).collect();
                out.extend(hs.into_iter().map(|h| h.join().expect("a scoped job panicked")));
            });
        }
        return out;
    }
    items.iter().map(f).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_job_runs_and_map_keeps_the_order() {
        let slots: Vec<_> = (0..40u64)
            .map(|i| super::run((40 - i) as f32, move || i * i))
            .collect();
        for (i, s) in slots.iter().enumerate() {
            let v = loop {
                if let Some(v) = super::take(s) {
                    break v;
                }
                std::thread::yield_now();
            };
            assert_eq!(v, (i * i) as u64);
        }
        assert_eq!(super::map(&[3, 1, 2], |x| x * 10), vec![30, 10, 20]);
    }
}
