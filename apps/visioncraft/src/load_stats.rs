//! Where loading time goes: every stage of making a scene ready (plant meshes, leaf textures,
//! block compression, impostor bakes, model and map decoding, uploads, the scene sync) adds
//! its time here, from whatever thread it runs on, and a load's milestones (first frame, plants
//! done, everything done) are kept since the last `reset` (opening a project). The smoke
//! script's `stats` step prints the table.

use std::collections::BTreeMap;
use std::sync::Mutex;

struct Stats {
    /// stage -> (count, total ms)
    stages: BTreeMap<String, (u32, f64)>,
    /// (milestone, ms since the reset), first time each was reached
    marks: Vec<(String, f64)>,
    start_ms: f64,
    start_cpu: Option<f64>,
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    stages: BTreeMap::new(),
    marks: Vec::new(),
    start_ms: 0.0,
    start_cpu: None,
});

fn with<T>(f: impl FnOnce(&mut Stats) -> T) -> T {
    f(&mut STATS.lock().unwrap_or_else(|e| e.into_inner()))
}

pub fn add(stage: &str, ms: f64) {
    with(|s| {
        let e = s.stages.entry(stage.to_string()).or_default();
        e.0 += 1;
        e.1 += ms;
    });
}

/// Runs `f` and adds its time to `stage`.
pub fn time<T>(stage: &str, f: impl FnOnce() -> T) -> T {
    let t = crate::assets::now_ms();
    let v = f();
    add(stage, crate::assets::now_ms() - t);
    v
}

/// Starts a new load: stages, milestones and the clocks from zero.
pub fn reset() {
    let cpu = cpu_seconds();
    with(|s| {
        s.stages.clear();
        s.marks.clear();
        s.start_ms = crate::assets::now_ms();
        s.start_cpu = cpu;
    });
}

/// Notes the first time `name` is reached since the reset.
pub fn mark(name: &str) {
    let now = crate::assets::now_ms();
    with(|s| {
        if !s.marks.iter().any(|m| m.0 == name) {
            let at = now - s.start_ms;
            s.marks.push((name.to_string(), at));
        }
    });
}

/// The process' CPU time (user and system, every thread) in seconds, where the system says.
pub fn cpu_seconds() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        // Fields after the command name (which is in parentheses and may hold spaces).
        let rest = &stat[stat.rfind(')')? + 2..];
        let f: Vec<&str> = rest.split(' ').collect();
        let (utime, stime): (f64, f64) = (f.get(11)?.parse().ok()?, f.get(12)?.parse().ok()?);
        // shortcut: assumes 100 clock ticks per second (Linux' USER_HZ everywhere in practice)
        Some((utime + stime) / 100.0)
    }
    #[cfg(not(target_os = "linux"))]
    None
}

/// The milestones, then a table of stages by total time: count, seconds, share.
pub fn report() -> String {
    let cpu_now = cpu_seconds();
    let now = crate::assets::now_ms();
    with(|s| {
        let mut out = String::new();
        let wall = (now - s.start_ms) / 1000.0;
        for (m, at) in &s.marks {
            out += &format!("load milestone: {m} at {:.2} s\n", at / 1000.0);
        }
        if let (Some(a), Some(b)) = (s.start_cpu, cpu_now) {
            out += &format!(
                "load cpu: {:.1} cpu-seconds over {wall:.1} s wall since open ({:.1} cores)\n",
                b - a,
                (b - a) / wall.max(1e-3)
            );
        }
        // Rows starting `~` are a breakdown or a total of others: listed apart, out of the share.
        let mut rows: Vec<(&String, &(u32, f64))> = s.stages.iter().collect();
        rows.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));
        let total: f64 = rows
            .iter()
            .filter(|r| !r.0.starts_with('~'))
            .map(|r| r.1.1)
            .sum();
        out += "load stage | count | total s | share\n";
        for (k, (n, ms)) in rows.iter().filter(|r| !r.0.starts_with('~')) {
            out += &format!(
                "load stage {k} | {n} | {:.2} | {:.1} %\n",
                ms / 1000.0,
                100.0 * ms / total.max(1e-9)
            );
        }
        for (k, (n, ms)) in rows.iter().filter(|r| r.0.starts_with('~')).take(14) {
            out += &format!("load detail {} | {n} | {:.2}\n", &k[1..], ms / 1000.0);
        }
        out
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn stages_add_up_and_marks_keep_the_first_time() {
        super::add("test stage x", 10.0);
        super::add("test stage x", 5.0);
        super::mark("test mark");
        super::mark("test mark");
        let r = super::report();
        assert_eq!(r.matches("test mark").count(), 1);
        assert!(r.contains("load stage test stage x | 2 | 0.01"), "{r}");
    }
}
