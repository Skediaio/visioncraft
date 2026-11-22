//! The sun's and the lamps' bounce through a sky volume's rooms (`SkyVolume::sun_bounce`,
//! `SkyVolume::lamp_bounce`) made off the UI thread, again each time the light changes.
//!
//! One `Relighter` per volume holds the volume (and with it the transport table, built once on
//! its first use) and gets only the light; the bounce comes back. Latest wins: a request that
//! was not started yet is replaced by a newer one of its kind, so a drag of the time of day
//! never queues work. Natively a thread per volume; on the web the import worker that made the
//! volume (`sky_task`) stays alive and serves `["skylight", to_sun]` and `["lamplight", lamps]`.

use visioncraft_core::Lamp;

/// A finished relight.
#[derive(Clone, Debug, PartialEq)]
pub enum Done {
    /// The sun's bounce per unit of its illuminance, for the direction it was asked for.
    Sun { to_sun: [f32; 3], bounce: Vec<f32> },
    /// The lamps' bounce for the request made under `lights_key`.
    Lamps {
        lights_key: u64,
        bounce: Vec<[f32; 3]>,
    },
}

/// The web job kinds (replies are tagged with them, so they are told from the volume's).
pub const SKY_LIGHT: &str = "skylight";
pub const LAMP_LIGHT: &str = "lamplight";

/// Floats per lamp in `encode_lamps`: position, colour, lumens, direction, has-direction.
const LAMP_FLOATS: usize = 11;

/// The lamps as floats, eleven each.
pub fn encode_lamps(l: &[Lamp]) -> Vec<f32> {
    l.iter()
        .flat_map(|l| {
            let d = l.direction;
            l.position
                .into_iter()
                .chain(l.color)
                .chain([l.lumens])
                .chain(d.unwrap_or_default())
                .chain([f32::from(u8::from(d.is_some()))])
        })
        .collect()
}

pub fn decode_lamps(f: &[f32]) -> Vec<Lamp> {
    f.as_chunks::<LAMP_FLOATS>()
        .0
        .iter()
        .map(|c| Lamp {
            position: [c[0], c[1], c[2]],
            color: [c[3], c[4], c[5]],
            lumens: c[6],
            direction: (c[10] > 0.5).then_some([c[7], c[8], c[9]]),
        })
        .collect()
}

/// The lamps' bounce as the flat floats a reply carries, and back.
pub fn flatten_rgb(b: &[[f32; 3]]) -> Vec<f32> {
    b.as_flattened().to_vec()
}

pub fn unflatten_rgb(f: &[f32]) -> Vec<[f32; 3]> {
    f.as_chunks::<3>().0.to_vec()
}

/// The kinds of request, served alternately when both wait (a continuous drag of the sun must
/// not starve the lamps).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sun,
    Lamps,
}

/// Which waiting request to serve next: the one not served last.
pub fn pick(sun: bool, lamps: bool, last: Option<Kind>) -> Option<Kind> {
    match (sun, lamps) {
        (false, false) => None,
        (true, false) => Some(Kind::Sun),
        (false, true) => Some(Kind::Lamps),
        (true, true) => Some(if last == Some(Kind::Sun) {
            Kind::Lamps
        } else {
            Kind::Sun
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::Relighter;
#[cfg(target_arch = "wasm32")]
pub use web::{Link, Relighter, mark_dead, on_reply};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{Done, Kind, Lamp, pick};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
    use visioncraft_core::SkyVolume;

    #[derive(Default)]
    struct State {
        sun: Option<[f32; 3]>,
        lamps: Option<(u64, Vec<Lamp>)>,
        done: Vec<Done>,
        working: bool,
        closed: bool,
        /// A compute panicked: the thread is gone.
        failed: bool,
        last: Option<Kind>,
        computed: usize,
    }

    enum Request {
        Sun([f32; 3]),
        Lamps(u64, Vec<Lamp>),
    }

    #[derive(Default)]
    struct Shared {
        state: Mutex<State>,
        wake: Condvar,
    }

    impl Shared {
        fn lock(&self) -> MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// A thread that holds one volume and relights it on request.
    pub struct Relighter {
        shared: Arc<Shared>,
    }

    impl Relighter {
        /// The thread for `volume`; `None` when it cannot start.
        pub fn spawn(volume: SkyVolume) -> Option<Self> {
            Self::start(volume, Box::new(|_| {}))
        }

        /// As `spawn`, with `before` called on the thread before each compute (a test gate).
        #[cfg(test)]
        pub fn spawn_gated(
            volume: SkyVolume,
            before: impl Fn(Kind) + Send + 'static,
        ) -> Option<Self> {
            Self::start(volume, Box::new(before))
        }

        fn start(volume: SkyVolume, before: Box<dyn Fn(Kind) + Send>) -> Option<Self> {
            let shared = Arc::<Shared>::default();
            let theirs = shared.clone();
            std::thread::Builder::new()
                .name("sky-relight".into())
                .spawn(move || run(&volume, &theirs, &*before))
                .ok()?;
            Some(Self { shared })
        }

        /// Asks for the sun's bounce, replacing a request not started yet.
        pub fn request_sun(&self, to_sun: [f32; 3]) {
            let mut s = self.shared.lock();
            if !s.failed {
                s.sun = Some(to_sun);
            }
            drop(s);
            self.shared.wake.notify_one();
        }

        /// Asks for the lamps' bounce, replacing a request not started yet.
        pub fn request_lamps(&self, lights_key: u64, lamps: Vec<Lamp>) {
            let mut s = self.shared.lock();
            if !s.failed {
                s.lamps = Some((lights_key, lamps));
            }
            drop(s);
            self.shared.wake.notify_one();
        }

        /// A finished relight, once.
        pub fn take(&self) -> Option<Done> {
            let mut s = self.shared.lock();
            (!s.done.is_empty()).then(|| s.done.remove(0))
        }

        /// A request waits or runs, or a result is not taken yet.
        pub fn busy(&self) -> bool {
            let s = self.shared.lock();
            s.sun.is_some() || s.lamps.is_some() || s.working || !s.done.is_empty()
        }

        /// The relighter cannot serve any more (a compute panicked, or the web worker died).
        pub fn failed(&self) -> bool {
            self.shared.lock().failed
        }

        #[cfg(test)]
        pub fn computed(&self) -> usize {
            self.shared.lock().computed
        }

        #[cfg(test)]
        pub fn probe(&self) -> Arc<dyn std::any::Any + Send + Sync> {
            self.shared.clone()
        }
    }

    impl Drop for Relighter {
        fn drop(&mut self) {
            self.shared.lock().closed = true;
            self.shared.wake.notify_all();
        }
    }

    fn run(volume: &SkyVolume, shared: &Shared, before: &dyn Fn(Kind)) {
        loop {
            let mut s = shared.lock();
            while !s.closed && s.sun.is_none() && s.lamps.is_none() {
                s = shared.wake.wait(s).unwrap_or_else(PoisonError::into_inner);
            }
            if s.closed {
                return;
            }
            let kind = pick(s.sun.is_some(), s.lamps.is_some(), s.last);
            let request = match kind {
                Some(Kind::Sun) => s.sun.take().map(Request::Sun),
                Some(Kind::Lamps) => s.lamps.take().map(|(k, l)| Request::Lamps(k, l)),
                None => None,
            };
            let (Some(kind), Some(request)) = (kind, request) else {
                continue;
            };
            s.last = Some(kind);
            s.working = true;
            drop(s);
            let done = catch_unwind(AssertUnwindSafe(|| {
                before(kind);
                match request {
                    Request::Sun(to_sun) => Done::Sun {
                        to_sun,
                        bounce: volume.sun_bounce(to_sun),
                    },
                    Request::Lamps(lights_key, lamps) => Done::Lamps {
                        lights_key,
                        bounce: volume.lamp_bounce(&lamps),
                    },
                }
            }));
            let mut s = shared.lock();
            s.working = false;
            s.computed += 1;
            match done {
                Ok(d) => {
                    // A newer result of a kind replaces an older one nobody took.
                    s.done
                        .retain(|o| std::mem::discriminant(o) != std::mem::discriminant(&d));
                    s.done.push(d);
                }
                Err(_) => {
                    // Nothing will serve any more: no request may be left waiting.
                    s.failed = true;
                    s.sun = None;
                    s.lamps = None;
                    return;
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::{Done, Kind, LAMP_LIGHT, Lamp, SKY_LIGHT, encode_lamps, pick, unflatten_rgb};
    use crate::assets::now_ms;
    use js_sys::{Array, Float32Array};
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};
    use wasm_bindgen::JsCast;

    enum Flight {
        Sun([f32; 3]),
        Lamps(u64),
    }

    /// A posted request without a reply after this long: the worker is taken as dead.
    const WATCHDOG_MS: f64 = 10_000.0;

    #[derive(Default)]
    struct State {
        in_flight: Option<Flight>,
        sun: Option<[f32; 3]>,
        lamps: Option<(u64, Vec<Lamp>)>,
        done: VecDeque<Done>,
        dead: bool,
        last: Option<Kind>,
        /// `now_ms` when the request in flight was posted.
        posted_at: f64,
    }

    impl State {
        /// Marks the worker dead when a request has had no reply for too long.
        fn expire(&mut self) {
            if self.in_flight.is_some() && now_ms() - self.posted_at > WATCHDOG_MS {
                self.dead = true;
            }
        }
    }

    /// The import worker that made a volume (it keeps the volume), serving relights.
    pub struct Relighter {
        worker: web_sys::Worker,
        state: Rc<RefCell<State>>,
    }

    /// What `sky_task`'s handlers of the worker need to reach a `Relighter`'s state.
    #[derive(Clone)]
    pub struct Link {
        worker: web_sys::Worker,
        state: Rc<RefCell<State>>,
    }

    impl Relighter {
        /// A relighter on `worker`, to serve once the worker has made its volume.
        pub fn on_worker(worker: web_sys::Worker) -> Self {
            Self {
                worker,
                state: Rc::default(),
            }
        }

        pub fn link(&self) -> Link {
            Link {
                worker: self.worker.clone(),
                state: self.state.clone(),
            }
        }

        pub fn request_sun(&self, to_sun: [f32; 3]) {
            self.state.borrow_mut().sun = Some(to_sun);
            pump(&mut self.state.borrow_mut(), &self.worker);
        }

        pub fn request_lamps(&self, lights_key: u64, lamps: Vec<Lamp>) {
            self.state.borrow_mut().lamps = Some((lights_key, lamps));
            pump(&mut self.state.borrow_mut(), &self.worker);
        }

        pub fn take(&self) -> Option<Done> {
            let mut s = self.state.borrow_mut();
            s.expire();
            s.done.pop_front()
        }

        pub fn busy(&self) -> bool {
            self.state.borrow_mut().expire();
            let s = self.state.borrow();
            !s.dead
                && (s.in_flight.is_some()
                    || s.sun.is_some()
                    || s.lamps.is_some()
                    || !s.done.is_empty())
        }

        pub fn failed(&self) -> bool {
            self.state.borrow_mut().expire();
            self.state.borrow().dead
        }
    }

    impl Drop for Relighter {
        fn drop(&mut self) {
            self.worker.terminate();
        }
    }

    /// Posts the newest waiting request when none is in flight (the kind not posted last).
    fn pump(s: &mut State, w: &web_sys::Worker) {
        if s.in_flight.is_some() || s.dead {
            return;
        }
        let kind = pick(s.sun.is_some(), s.lamps.is_some(), s.last);
        let (msg, flight) = match (kind, s.sun.take_if(|_| kind == Some(Kind::Sun))) {
            (Some(Kind::Sun), Some(to_sun)) => {
                let m = Array::of2(&SKY_LIGHT.into(), &Float32Array::from(&to_sun[..]));
                (m, Flight::Sun(to_sun))
            }
            _ => match s.lamps.take_if(|_| kind == Some(Kind::Lamps)) {
                Some((key, lamps)) => {
                    let m = Array::of2(
                        &LAMP_LIGHT.into(),
                        &Float32Array::from(&encode_lamps(&lamps)[..]),
                    );
                    (m, Flight::Lamps(key))
                }
                None => return,
            },
        };
        s.last = kind;
        if w.post_message(&msg).is_ok() {
            s.posted_at = now_ms();
            s.in_flight = Some(flight);
        } else {
            s.dead = true;
        }
    }

    /// A tagged reply of the worker (`["skylight", floats]`, `["lamplight", floats]`).
    pub fn on_reply(link: &Link, reply: &Array) {
        let mut s = link.state.borrow_mut();
        let floats = reply
            .get(1)
            .dyn_into::<Float32Array>()
            .ok()
            .map(|f| f.to_vec());
        match (
            s.in_flight.take(),
            reply.get(0).as_string().as_deref(),
            floats,
        ) {
            (Some(Flight::Sun(to_sun)), Some(SKY_LIGHT), Some(bounce)) => {
                s.done.push_back(Done::Sun { to_sun, bounce });
            }
            (Some(Flight::Lamps(lights_key)), Some(LAMP_LIGHT), Some(f)) => {
                let bounce = unflatten_rgb(&f);
                s.done.push_back(Done::Lamps { lights_key, bounce });
            }
            _ => s.dead = true,
        }
        pump(&mut s, &link.worker);
    }

    /// The worker failed: the relighter serves no more.
    pub fn mark_dead(link: &Link) {
        link.state.borrow_mut().dead = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use visioncraft_core::{SkyVolume, sky_volume};

    fn quad(lo: [f32; 3], hi: [f32; 3]) -> Vec<[[f32; 3]; 3]> {
        let flat = (0..3).find(|&k| lo[k] == hi[k]).unwrap();
        let (u, v) = ((flat + 1) % 3, (flat + 2) % 3);
        let corner = |a: bool, b: bool| {
            let mut p = lo;
            p[u] = if a { hi[u] } else { lo[u] };
            p[v] = if b { hi[v] } else { lo[v] };
            p
        };
        vec![
            [
                corner(false, false),
                corner(true, false),
                corner(true, true),
            ],
            [
                corner(false, false),
                corner(true, true),
                corner(false, true),
            ],
        ]
    }

    /// A 6 x 3 x 6 m room with a 2 m opening in its east wall.
    fn volume() -> SkyVolume {
        let mut t = Vec::new();
        t.extend(quad([0.0, 0.0, 0.0], [6.0, 0.0, 6.0]));
        t.extend(quad([0.0, 3.0, 0.0], [6.0, 3.0, 6.0]));
        t.extend(quad([0.0, 0.0, 0.0], [0.0, 3.0, 6.0]));
        t.extend(quad([0.0, 0.0, 0.0], [6.0, 3.0, 0.0]));
        t.extend(quad([0.0, 0.0, 6.0], [6.0, 3.0, 6.0]));
        t.extend(quad([6.0, 0.0, 0.0], [6.0, 3.0, 2.0]));
        t.extend(quad([6.0, 0.0, 4.0], [6.0, 3.0, 6.0]));
        sky_volume(&t).expect("a volume")
    }

    fn sun(deg: f32) -> [f32; 3] {
        let a = deg.to_radians();
        [a.cos(), a.sin(), 0.0]
    }

    fn wait(r: &Relighter, want: impl Fn(&Done) -> bool) -> Done {
        let end = Instant::now() + Duration::from_secs(60);
        while Instant::now() < end {
            match r.take() {
                Some(d) if want(&d) => return d,
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        panic!("no result in time");
    }

    #[test]
    fn lamps_survive_the_trip_through_floats() {
        let l = vec![
            Lamp {
                position: [1.0, 2.5, -3.0],
                color: [1.0, 0.5, 0.25],
                lumens: 800.0,
                direction: None,
            },
            Lamp {
                position: [0.0, 2.0, 1.0],
                color: [0.2, 0.3, 0.4],
                lumens: 12.5,
                direction: Some([0.0, -1.0, 0.0]),
            },
        ];
        assert_eq!(decode_lamps(&encode_lamps(&l)), l);
        let b = vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        assert_eq!(unflatten_rgb(&flatten_rgb(&b)), b);
    }

    #[test]
    fn a_sun_request_returns_the_volumes_own_bounce_bit_for_bit() {
        let v = volume();
        let r = Relighter::spawn(v.clone()).expect("a thread");
        let dir = sun(30.0);
        r.request_sun(dir);
        let Done::Sun { to_sun, bounce } = wait(&r, |d| matches!(d, Done::Sun { .. })) else {
            unreachable!()
        };
        assert_eq!(to_sun, dir);
        let want = v.sun_bounce(dir);
        assert!(want.iter().any(|&e| e > 0.0));
        let bits = |f: &[f32]| f.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&bounce), bits(&want));
        assert!(!r.busy());
    }

    /// A gate for `spawn_gated`: records the kinds computed and holds the first compute until
    /// `release`.
    #[derive(Default)]
    struct Gate {
        order: std::sync::Mutex<Vec<Kind>>,
        open: std::sync::Mutex<bool>,
        wake: std::sync::Condvar,
    }

    impl Gate {
        fn enter(&self, k: Kind) {
            let first = {
                let mut o = self.order.lock().unwrap();
                o.push(k);
                o.len() == 1
            };
            if first {
                let mut open = self.open.lock().unwrap();
                while !*open {
                    open = self.wake.wait(open).unwrap();
                }
            }
        }
        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.wake.notify_all();
        }
        fn entered(&self) -> usize {
            self.order.lock().unwrap().len()
        }
        fn wait_entered(&self) {
            let end = Instant::now() + Duration::from_secs(60);
            while self.entered() == 0 && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(self.entered() > 0, "the thread never started");
        }
    }

    fn lamp() -> Vec<Lamp> {
        vec![Lamp {
            position: [3.0, 2.0, 3.0],
            color: [1.0, 0.5, 0.25],
            lumens: 1000.0,
            direction: None,
        }]
    }

    #[test]
    fn the_latest_sun_request_wins_and_the_ones_between_are_dropped() {
        let v = volume();
        let gate = std::sync::Arc::new(Gate::default());
        let g = gate.clone();
        let r = Relighter::spawn_gated(v.clone(), move |k| g.enter(k)).expect("a thread");
        let dirs: Vec<[f32; 3]> = (0..20).map(|i| sun(10.0 + 2.0 * i as f32)).collect();
        // The first request is held inside its compute; the other 19 replace each other.
        r.request_sun(dirs[0]);
        gate.wait_entered();
        for d in &dirs[1..] {
            r.request_sun(*d);
        }
        assert!(r.busy());
        gate.release();
        let last = *dirs.last().unwrap();
        let Done::Sun { to_sun, bounce } = wait(
            &r,
            |d| matches!(d, Done::Sun { to_sun, .. } if *to_sun == last),
        ) else {
            unreachable!()
        };
        assert_eq!(to_sun, last);
        assert_eq!(bounce, v.sun_bounce(last));
        assert_eq!(r.computed(), 2, "the first and the last only");
        assert!(!r.busy());
    }

    #[test]
    fn a_sun_request_stream_does_not_starve_the_lamps() {
        let gate = std::sync::Arc::new(Gate::default());
        let g = gate.clone();
        let r = Relighter::spawn_gated(volume(), move |k| g.enter(k)).expect("a thread");
        r.request_sun(sun(10.0));
        gate.wait_entered();
        // Both kinds wait behind the held compute; the one not served last goes first.
        r.request_sun(sun(20.0));
        r.request_lamps(1, lamp());
        gate.release();
        wait(&r, |d| matches!(d, Done::Lamps { .. }));
        wait(
            &r,
            |d| matches!(d, Done::Sun { to_sun, .. } if *to_sun == sun(20.0)),
        );
        assert_eq!(
            *gate.order.lock().unwrap(),
            vec![Kind::Sun, Kind::Lamps, Kind::Sun]
        );
    }

    #[test]
    fn picking_alternates_between_two_waiting_kinds() {
        assert_eq!(pick(false, false, None), None);
        assert_eq!(pick(true, false, Some(Kind::Sun)), Some(Kind::Sun));
        assert_eq!(pick(false, true, Some(Kind::Lamps)), Some(Kind::Lamps));
        assert_eq!(pick(true, true, None), Some(Kind::Sun));
        assert_eq!(pick(true, true, Some(Kind::Sun)), Some(Kind::Lamps));
        assert_eq!(pick(true, true, Some(Kind::Lamps)), Some(Kind::Sun));
    }

    #[test]
    fn a_panicking_compute_fails_the_relighter_instead_of_hanging_busy() {
        let r = Relighter::spawn_gated(volume(), |_| panic!("boom")).expect("a thread");
        r.request_sun(sun(20.0));
        let end = Instant::now() + Duration::from_secs(60);
        while !r.failed() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(r.failed());
        assert!(!r.busy());
        // Later requests are not left waiting for a thread that is gone.
        r.request_lamps(1, lamp());
        assert!(!r.busy());
    }

    #[test]
    fn lamps_come_back_as_the_volumes_own_bounce_under_their_key() {
        let v = volume();
        let r = Relighter::spawn(v.clone()).expect("a thread");
        let lamps = lamp();
        r.request_lamps(42, lamps.clone());
        let Done::Lamps { lights_key, bounce } = wait(&r, |d| matches!(d, Done::Lamps { .. }))
        else {
            unreachable!()
        };
        assert_eq!(lights_key, 42);
        assert_eq!(bounce, v.lamp_bounce(&lamps));
    }

    #[test]
    fn dropping_the_handle_ends_the_thread() {
        let r = Relighter::spawn(volume()).expect("a thread");
        r.request_sun(sun(20.0));
        let probe = r.probe();
        drop(r);
        let end = Instant::now() + Duration::from_secs(60);
        while std::sync::Arc::strong_count(&probe) > 1 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            std::sync::Arc::strong_count(&probe),
            1,
            "the thread still runs"
        );
    }
}
