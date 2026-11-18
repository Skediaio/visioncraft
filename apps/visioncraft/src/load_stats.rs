//! The load timeline: where loading time goes. Every stage of making a scene ready (plant
//! meshes, leaf textures, block compression, impostor bakes, model and map decoding, uploads,
//! pipeline builds, the scene sync) adds its time here, from whatever thread it runs on, and a
//! load's milestones (first frame, first frame on screen, plants done, everything done) are kept
//! since the last `reset` (opening a project or an example). The program's start (renderer and
//! pipelines, calibration, the first frame) is kept apart, never reset.
//!
//! Always on and cheap: a mutex, a map of stage totals and at most `MAX_EVENTS` timed events.
//! Agents read it with `ui.load` (JSON, `json`), the smoke script's `stats` step prints the
//! table (`report`), and in the browser every milestone is also a console line (`[load] ...`).
//! docs/performance.md explains how to read it.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// Timed events kept per load (the longest win when there are more).
const MAX_EVENTS: usize = 400;

/// Events shorter than this are only summed into their stage, not kept on the timeline.
const EVENT_MIN_MS: f64 = 2.0;

struct Stats {
    /// stage -> (count, total ms, longest ms)
    stages: BTreeMap<String, (u32, f64, f64)>,
    /// (milestone, ms since the reset), first time each was reached
    marks: Vec<(String, f64)>,
    /// (stage, start ms since the reset, ms): the longer steps, in the order they ended
    events: Vec<(String, f64, f64)>,
    /// (what, ms since the program started): the start, never reset
    startup: Vec<(String, f64)>,
    /// Frames since the reset: count, longer than 50 ms, longest ms.
    frames: (u32, u32, f64),
    start_ms: f64,
    start_cpu: Option<f64>,
    /// What the load was (`reset`).
    what: String,
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    stages: BTreeMap::new(),
    marks: Vec::new(),
    events: Vec::new(),
    startup: Vec::new(),
    frames: (0, 0, 0.0),
    start_ms: 0.0,
    start_cpu: None,
    what: String::new(),
});

fn with<T>(f: impl FnOnce(&mut Stats) -> T) -> T {
    f(&mut STATS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Milliseconds since the program started (`performance.now()` on the web, in the page and in
/// a worker alike).
pub fn now_ms() -> f64 {
    crate::assets::now_ms()
}

/// Adds `ms` to `stage`; a long step also lands on the timeline (ending now).
pub fn add(stage: &str, ms: f64) {
    let now = now_ms();
    with(|s| {
        let e = s.stages.entry(stage.to_string()).or_default();
        e.0 += 1;
        e.1 += ms;
        e.2 = e.2.max(ms);
        if ms >= EVENT_MIN_MS && !stage.starts_with('~') {
            let at = (now - ms - s.start_ms).max(0.0);
            if s.events.len() >= MAX_EVENTS {
                // Keep the longest: drop the shortest kept.
                if let Some((i, _)) = s
                    .events
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.2.total_cmp(&b.1.2))
                    .filter(|(_, e)| e.2 < ms)
                {
                    s.events.remove(i);
                } else {
                    return;
                }
            }
            s.events.push((stage.to_string(), at, ms));
        }
    });
}

/// Runs `f` and adds its time to `stage`.
pub fn time<T>(stage: &str, f: impl FnOnce() -> T) -> T {
    let t = now_ms();
    let v = f();
    add(stage, now_ms() - t);
    v
}

/// Starts a new load (`what`: the project or example): stages, milestones, events, frames and
/// the clocks from zero.
pub fn reset(what: &str) {
    let cpu = cpu_seconds();
    with(|s| {
        s.stages.clear();
        s.marks.clear();
        s.events.clear();
        s.frames = (0, 0, 0.0);
        s.start_ms = now_ms();
        s.start_cpu = cpu;
        s.what = what.to_string();
    });
    console(&format!("[load] {what}: started"));
}

/// Notes the first time `name` is reached since the reset.
pub fn mark(name: &str) {
    let now = now_ms();
    let first = with(|s| {
        if s.marks.iter().any(|m| m.0 == name) {
            return None;
        }
        let at = now - s.start_ms;
        s.marks.push((name.to_string(), at));
        Some((s.what.clone(), at))
    });
    if let Some((what, at)) = first {
        console(&format!("[load] {what}: {name} at {:.2} s", at / 1000.0));
    }
}

/// Whether `name` was reached since the reset.
pub fn reached(name: &str) -> bool {
    with(|s| s.marks.iter().any(|m| m.0 == name))
}

/// Notes a step of the program's start (once each), at the time since the program started.
pub fn startup(name: &str) {
    let now = now_ms();
    let first = with(|s| {
        if s.startup.iter().any(|m| m.0 == name) {
            return false;
        }
        s.startup.push((name.to_string(), now));
        true
    });
    if first {
        console(&format!("[start] {name} at {:.2} s", now / 1000.0));
    }
}

/// A drawn frame of `ms` (the time since the last one).
pub fn frame(ms: f64) {
    with(|s| {
        s.frames.0 += 1;
        if ms > 50.0 {
            s.frames.1 += 1;
        }
        s.frames.2 = s.frames.2.max(ms);
    });
}

/// After the first frame of the program and of a load is submitted: notes when the GPU has
/// finished it (`first frame on screen`), which includes compiling the pipelines it uses.
pub fn first_frame_submitted(queue: &wgpu::Queue) {
    let start = !with(|s| s.startup.iter().any(|m| m.0 == "first frame submitted"));
    if start {
        startup("first frame submitted");
        queue.on_submitted_work_done(|| startup("first frame on screen"));
    }
    if !reached("first frame") {
        mark("first frame");
        queue.on_submitted_work_done(|| mark("first frame on screen"));
    }
}

fn console(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    web_sys::console::info_1(&msg.into());
    #[cfg(not(target_arch = "wasm32"))]
    let _ = msg;
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
    let now = now_ms();
    with(|s| {
        let mut out = String::new();
        let wall = (now - s.start_ms) / 1000.0;
        for (m, at) in &s.startup {
            out += &format!("start: {m} at {:.2} s\n", at / 1000.0);
        }
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
        let mut rows: Vec<(&String, &(u32, f64, f64))> = s.stages.iter().collect();
        rows.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));
        let total: f64 = rows
            .iter()
            .filter(|r| !r.0.starts_with('~'))
            .map(|r| r.1.1)
            .sum();
        out += "load stage | count | total s | share | longest ms\n";
        for (k, (n, ms, max)) in rows.iter().filter(|r| !r.0.starts_with('~')) {
            out += &format!(
                "load stage {k} | {n} | {:.2} | {:.1} % | {max:.0}\n",
                ms / 1000.0,
                100.0 * ms / total.max(1e-9)
            );
        }
        for (k, (n, ms, _)) in rows.iter().filter(|r| r.0.starts_with('~')).take(14) {
            out += &format!("load detail {} | {n} | {:.2}\n", &k[1..], ms / 1000.0);
        }
        out
    })
}

/// Everything as JSON for agents (`ui.load`): the start, the load's milestones, its stages by
/// total time, the frames, and the longest steps on a timeline (ms since the load started).
pub fn json() -> serde_json::Value {
    let now = now_ms();
    with(|s| {
        let mut rows: Vec<(&String, &(u32, f64, f64))> = s.stages.iter().collect();
        rows.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));
        let mut events = s.events.clone();
        events.sort_by(|a, b| a.1.total_cmp(&b.1));
        let ms = |v: f64| (v * 10.0).round() / 10.0;
        serde_json::json!({
            "startup": s.startup.iter().map(|(m, at)| serde_json::json!({"name": m, "at_ms": ms(*at)})).collect::<Vec<_>>(),
            "load": {
                "what": s.what,
                "since_ms": ms(now - s.start_ms),
                "milestones": s.marks.iter().map(|(m, at)| serde_json::json!({"name": m, "at_ms": ms(*at)})).collect::<Vec<_>>(),
                "frames": {"count": s.frames.0, "over_50ms": s.frames.1, "longest_ms": ms(s.frames.2)},
                "stages": rows.iter().map(|(k, (n, total, max))| serde_json::json!({
                    "name": k.trim_start_matches('~'),
                    "breakdown": k.starts_with('~'),
                    "count": n,
                    "total_ms": ms(*total),
                    "longest_ms": ms(*max),
                })).collect::<Vec<_>>(),
                "timeline": events.iter().map(|(k, at, d)| serde_json::json!({"name": k, "start_ms": ms(*at), "ms": ms(*d)})).collect::<Vec<_>>(),
            },
        })
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
        let j = super::json();
        let stage = j["load"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "test stage x")
            .unwrap()
            .clone();
        assert_eq!(stage["count"], 2);
        assert_eq!(stage["longest_ms"], 10.0);
        assert!(
            j["load"]["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"] == "test stage x" && e["ms"] == 10.0)
        );
    }
}
