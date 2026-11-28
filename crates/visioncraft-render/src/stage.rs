//! The CPU stage times of a frame for native tools (the bench averages them per camera path).
//! When `VISIONCRAFT_STAGE_TIMINGS` names a file, each frame appends one line to it:
//! `frame <n> lods <ms> plan <ms> visible <ms> gather <ms> record <ms>`, where `plan` is every
//! pass's `plan_items` (with `visible`, its cluster visibility tests, inside), `gather` the
//! batched instances' dispatch and `record` the geometry passes' recording. Off (no file, no
//! line) unless the variable is set; never on the web, which has no clock of its own.

use std::time::Instant;

/// Milliseconds since `t`.
pub(crate) fn cpu_ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

pub(crate) struct StageLog {
    file: std::fs::File,
}

impl StageLog {
    /// The log `VISIONCRAFT_STAGE_TIMINGS` names, appended to; `None` without the variable.
    pub(crate) fn from_env() -> Option<Box<Self>> {
        let path = std::env::var_os("VISIONCRAFT_STAGE_TIMINGS")?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some(Box::new(Self { file }))
    }

    pub(crate) fn frame(
        &mut self,
        n: u64,
        lods: f64,
        plan: f64,
        visible: f64,
        gather: f64,
        record: f64,
    ) {
        use std::io::Write;
        let line = format!(
            "frame {n} lods {lods:.3} plan {plan:.3} visible {visible:.3} gather {gather:.3} record {record:.3}\n"
        );
        let _ = self.file.write_all(line.as_bytes());
    }
}
