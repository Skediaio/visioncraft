//! What plays on the scene beside the project: animation clips on placed objects, and short
//! flashes. Like the camera and the selection this is session state: not saved with the
//! project and not an undo step. Everything is sampled at the session's clock
//! ([`Playback::clock`]): the window's frame time live, a film's time for its frames, so a film
//! is the same every time it is rendered.

use crate::keyframes::{Clip, Played, Rig, Sampler};
use std::collections::BTreeMap;

/// Most flashes at once; over it the oldest goes.
pub const MAX_FLASHES: usize = 16;

/// What a clip plays on: a planting (`g4`: every object it places) or one object of it
/// (`g4#12`, counted from 0 as `planting.replace_placement` counts them).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClipTarget {
    pub planting: String,
    pub plant: Option<u32>,
}

impl ClipTarget {
    /// Every object of the planting.
    pub fn all(planting: &str) -> Self {
        Self {
            planting: planting.to_string(),
            plant: None,
        }
    }

    /// `g4` or `g4#12`; a message for anything else.
    pub fn parse(object: &str) -> Result<Self, String> {
        let what = "an object is a planting (`g4`) or one of its objects (`g4#12`)";
        let object = object.trim();
        let Some((planting, plant)) = object.split_once('#') else {
            if object.is_empty() {
                return Err(what.to_string());
            }
            return Ok(Self::all(object));
        };
        let plant = plant
            .parse::<u32>()
            .map_err(|_| format!("{what}, not `{object}`"))?;
        if planting.is_empty() {
            return Err(what.to_string());
        }
        Ok(Self {
            planting: planting.to_string(),
            plant: Some(plant),
        })
    }

    /// As it is written: `g4` or `g4#12`.
    pub fn name(&self) -> String {
        match self.plant {
            Some(n) => format!("{}#{n}", self.planting),
            None => self.planting.clone(),
        }
    }
}

/// One drawn object: plant `plant` of planting `planting` (every one of them with `None`),
/// drawn as `asset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot<'a> {
    pub planting: &'a str,
    pub plant: Option<u32>,
    pub asset: &'a str,
}

/// A clip playing on a target.
#[derive(Clone, Debug, PartialEq)]
pub struct Play {
    pub target: ClipTarget,
    /// Per asset the clip's index among that model's clips (as `query.clips` lists them).
    pub clips: BTreeMap<String, usize>,
    /// The clip's name, for a model that knows it under another index.
    pub name: String,
    /// Seconds on the session's clock when the clip started.
    pub start_s: f32,
    pub looped: bool,
    /// When it started, among all plays: of two that move the same property, the later wins.
    serial: u64,
}

/// How long a flash lasts: a number of frames, or a number of seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Life {
    /// Frames of the frame rate in force where the flash is drawn ([`Playback::fps`]).
    Frames(u32),
    Seconds(f32),
}

/// A short-lived flash: a point light, and a glowing quad unless its side is 0.
#[derive(Clone, Debug, PartialEq)]
pub struct Flash {
    pub id: u64,
    /// Seconds on the session's clock when it starts.
    pub start_s: f32,
    pub position: [f32; 3],
    /// Linear colour.
    pub color: [f32; 3],
    /// Luminous flux in lumens, as a lamp's.
    pub intensity: f32,
    /// Where its light has faded to zero (m).
    pub range_m: f32,
    /// Side of the glowing quad (m); 0: no quad.
    pub size_m: f32,
    pub life: Life,
}

impl Flash {
    /// Whether the frame at clock time `t` shows it. A frame lasts `1 / fps` seconds of the
    /// frame rate in force: a film's, so the same frames show it every time.
    pub fn shows_at(&self, t: f64, fps: f64) -> bool {
        let since = t - f64::from(self.start_s);
        let frames = match self.life {
            Life::Seconds(s) => return (0.0..f64::from(s)).contains(&since),
            Life::Frames(n) => f64::from(n),
        };
        let frame = (since * fps.max(1.0)).round();
        (0.0..frames).contains(&frame)
    }
}

/// The clips and flashes playing, with the clock they are sampled at.
#[derive(Clone, Debug, PartialEq)]
pub struct Playback {
    /// The session's clock in seconds: the frame's time in a window, the film's time for a
    /// film's frames, the `animation_s` of the last picture in a session without a window.
    pub clock: f64,
    /// Frames a second where it is drawn: what a flash's `frames` last (a window's own rate, a
    /// film's `fps`; 60 in a still).
    pub fps: f64,
    plays: Vec<Play>,
    flashes: Vec<Flash>,
    serial: u64,
    next_flash: u64,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            clock: 0.0,
            fps: 60.0,
            plays: Vec::new(),
            flashes: Vec::new(),
            serial: 0,
            next_flash: 1,
        }
    }
}

/// The clip `clip` names among `clips`: by its exact name first (of two with the same name the
/// one with the lowest index), else an index (an integer), else a message that lists the names.
pub fn clip_index(clips: &[Clip], clip: &str) -> Result<usize, String> {
    if let Some(i) = clips.iter().position(|c| c.name == clip) {
        return Ok(i);
    }
    if let Ok(i) = clip.parse::<usize>() {
        return (i < clips.len()).then_some(i).ok_or_else(|| {
            format!(
                "clip {i}: the model has {} clip{}, from 0",
                clips.len(),
                if clips.len() == 1 { "" } else { "s" }
            )
        });
    }
    let names: Vec<String> = clips
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if c.name.is_empty() {
                format!("{i} (unnamed)")
            } else {
                format!("{i} `{}`", c.name)
            }
        })
        .collect();
    Err(match names.is_empty() {
        true => "the model has no animation clips".to_string(),
        false => format!("no clip `{clip}`; the clips are: {}", names.join(", ")),
    })
}

impl Playback {
    /// Starts clip `clips` (per asset, from [`clip_index`]) on `target` at `start_s`. A clip
    /// that plays there already starts again. Returns whether it did.
    pub fn play(
        &mut self,
        target: ClipTarget,
        clips: BTreeMap<String, usize>,
        name: &str,
        start_s: f32,
        looped: bool,
    ) -> bool {
        let again = self
            .plays
            .iter()
            .position(|p| p.target == target && p.clips == clips);
        let restarted = again.is_some();
        if let Some(i) = again {
            self.plays.remove(i);
        }
        self.serial += 1;
        self.plays.push(Play {
            target,
            clips,
            name: name.to_string(),
            start_s,
            looped,
            serial: self.serial,
        });
        restarted
    }

    /// Ends the plays on `target`: of `clips` only, or of every clip. Returns how many ended.
    pub fn stop(&mut self, target: &ClipTarget, clips: Option<&BTreeMap<String, usize>>) -> usize {
        let before = self.plays.len();
        self.plays
            .retain(|p| p.target != *target || clips.is_some_and(|c| p.clips != *c));
        before - self.plays.len()
    }

    /// Ends every play.
    pub fn stop_all(&mut self) {
        self.plays.clear();
    }

    /// The plays, in the order they started.
    pub fn plays(&self) -> &[Play] {
        &self.plays
    }

    /// The plays that reach `spot`, in the order they started: of two that move the same
    /// property, the later one wins.
    pub fn played<'a>(&'a self, spot: &'a Spot<'a>) -> impl Iterator<Item = Played> + 'a {
        self.plays
            .iter()
            .filter(move |p| {
                p.target.planting == spot.planting
                    && p.target.plant.is_none_or(|n| Some(n) == spot.plant)
            })
            .filter_map(move |p| {
                p.clips.get(spot.asset).map(|&clip| Played {
                    clip,
                    start_s: p.start_s,
                    looped: p.looped,
                })
            })
    }

    /// Whether anything plays on `planting`.
    pub fn playing_on(&self, planting: &str) -> bool {
        self.plays.iter().any(|p| p.target.planting == planting)
    }

    /// Adds a flash; over [`MAX_FLASHES`] at once the oldest goes. Returns its id.
    pub fn flash(&mut self, flash: Flash) -> u64 {
        let id = self.next_flash;
        self.next_flash += 1;
        self.flashes.insert(0, Flash { id, ..flash });
        self.flashes.truncate(MAX_FLASHES);
        id
    }

    /// Removes the flash `id`; whether there was one.
    pub fn remove_flash(&mut self, id: u64) -> bool {
        let before = self.flashes.len();
        self.flashes.retain(|f| f.id != id);
        before != self.flashes.len()
    }

    /// The flashes, the newest first, those that have ended included: the host drops them.
    pub fn flashes(&self) -> &[Flash] {
        &self.flashes
    }

    /// Whether anything plays or flashes.
    pub fn is_empty(&self) -> bool {
        self.plays.is_empty() && self.flashes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyframes::{Interpolation, Path, Trs, model_space};

    fn clip(name: &str, times: &[f32], values: &[f32]) -> Clip {
        Clip {
            name: name.into(),
            duration: times.last().copied().unwrap_or(0.0),
            channels: vec![crate::keyframes::Channel {
                node: 0,
                path: Path::Rotation,
                interpolation: Interpolation::Linear,
                times: times.to_vec(),
                values: values.to_vec(),
            }],
        }
    }

    /// A lid on a hinge at 1 m up: one node, two clips turning it a quarter about Z in 1 s.
    fn rig() -> Rig {
        Rig {
            rest: vec![Trs {
                translation: [0.0, 1.0, 0.0],
                ..Trs::IDENTITY
            }],
            parents: vec![None],
            clips: vec![
                clip(
                    "open",
                    &[0.0, 1.0],
                    &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.7071068, 0.7071068],
                ),
                clip(
                    "shut",
                    &[0.0, 2.0],
                    &[0.0, 0.0, 0.7071068, 0.7071068, 0.0, 0.0, 0.0, 1.0],
                ),
            ],
        }
    }

    fn play(
        p: &mut Playback,
        object: &str,
        asset: &str,
        clips: &[Clip],
        name: &str,
        start: f32,
        looped: bool,
    ) -> bool {
        let target = ClipTarget::parse(object).unwrap();
        let index = clip_index(clips, name).unwrap();
        p.play(
            target,
            BTreeMap::from([(asset.to_string(), index)]),
            name,
            start,
            looped,
        )
    }

    #[test]
    fn a_target_is_a_planting_or_one_of_its_objects() {
        assert_eq!(ClipTarget::parse("g4").unwrap(), ClipTarget::all("g4"));
        assert_eq!(ClipTarget::parse(" g4#12 ").unwrap().name(), "g4#12");
        assert_eq!(ClipTarget::parse("g4#0").unwrap().plant, Some(0));
        for bad in ["", "#3", "g4#", "g4#x", "g4#-1"] {
            assert!(ClipTarget::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_clip_rule_is_name_then_index_then_an_error() {
        let clips = [
            clip("", &[0.0], &[0.0, 0.0, 0.0, 1.0]),
            clip("fire", &[0.0], &[0.0, 0.0, 0.0, 1.0]),
            clip("reload", &[0.0], &[0.0, 0.0, 0.0, 1.0]),
            clip("fire", &[0.0], &[0.0, 0.0, 0.0, 1.0]),
        ];
        assert_eq!(
            clip_index(&clips, "fire").unwrap(),
            1,
            "the lowest index of a doubled name"
        );
        assert_eq!(clip_index(&clips, "reload").unwrap(), 2);
        assert_eq!(clip_index(&clips, "3").unwrap(), 3, "a number is an index");
        assert_eq!(clip_index(&clips, "0").unwrap(), 0);
        let e = clip_index(&clips, "nope").unwrap_err();
        assert!(
            e.contains("`fire`") && e.contains("`reload`") && e.contains("unnamed"),
            "{e}"
        );
        assert!(clip_index(&clips, "4").unwrap_err().contains("from 0"));
        assert!(
            clip_index(&[], "x")
                .unwrap_err()
                .contains("no animation clips")
        );
    }

    #[test]
    fn play_stop_and_restart() {
        let mut p = Playback::default();
        let (r, asset) = (rig(), "user:uo1");
        play(&mut p, "g1", asset, &r.clips, "open", 10.0, false);
        play(&mut p, "g1", asset, &r.clips, "shut", 12.0, true);
        assert_eq!(p.plays().len(), 2);
        assert_eq!(p.plays()[1].start_s, 12.0);
        // The same clip again restarts it: one play, a new start, now the latest.
        assert!(play(&mut p, "g1", asset, &r.clips, "open", 20.0, false));
        let plays = p.plays();
        assert_eq!(plays.len(), 2);
        assert_eq!((plays[1].name.as_str(), plays[1].start_s), ("open", 20.0));
        assert_eq!(plays[1].serial, 3);
        // Another object of the planting is another play; the whole planting does not reach it twice.
        play(&mut p, "g1#3", asset, &r.clips, "open", 21.0, false);
        assert_eq!(p.plays().len(), 3);
        let played = |p: &Playback, plant: Option<u32>, asset: &str| {
            p.played(&Spot {
                planting: "g1",
                plant,
                asset,
            })
            .count()
        };
        assert_eq!(played(&p, Some(3), asset), 3);
        assert_eq!(played(&p, Some(4), asset), 2);
        assert_eq!(
            played(&p, None, "user:uo2"),
            0,
            "another asset has no clip here"
        );
        assert_eq!(
            p.played(&Spot {
                planting: "g2",
                plant: None,
                asset,
            })
            .count(),
            0
        );
        // Stopping one clip of one object leaves the rest.
        let t = ClipTarget::parse("g1#3").unwrap();
        let open = BTreeMap::from([(asset.to_string(), 0)]);
        assert_eq!(p.stop(&t, Some(&open)), 1);
        assert_eq!(played(&p, Some(3), asset), 2);
        // Without a clip every play on the target goes.
        assert_eq!(p.stop(&ClipTarget::all("g1"), None), 2);
        assert!(p.is_empty());
        assert_eq!(p.stop(&ClipTarget::all("g1"), None), 0);
    }

    #[test]
    fn the_later_play_wins_and_an_ended_clip_holds_its_pose() {
        let mut p = Playback::default();
        let (mut sampler, asset) = (Sampler::default(), "user:uo1");
        let r = rig();
        let spot = Spot {
            planting: "g1",
            plant: None,
            asset,
        };
        let rest = model_space(&r.parents, &r.rest)[0];
        let mut played = Vec::new();
        let pose = |p: &Playback, s: &mut Sampler, played: &mut Vec<Played>, t: f64| {
            played.clear();
            played.extend(p.played(&spot));
            s.pose(&r, played, t)[0]
        };
        assert_eq!(pose(&p, &mut sampler, &mut played, 5.0), rest);
        // `open` from 10 s, not looped: at 10.5 half open, at 11 and at 100 fully open.
        play(&mut p, "g1", asset, &r.clips, "open", 10.0, false);
        let half = pose(&p, &mut sampler, &mut played, 10.5);
        let full = pose(&p, &mut sampler, &mut played, 11.0);
        assert_ne!(half, rest);
        assert_ne!(half, full);
        assert_eq!(
            pose(&p, &mut sampler, &mut played, 100.0),
            full,
            "an ended clip holds its last pose"
        );
        // Before it starts: at rest.
        assert_eq!(pose(&p, &mut sampler, &mut played, 5.0), rest);
        // `shut` started later on the same channel wins: it begins fully open and closes.
        play(&mut p, "g1", asset, &r.clips, "shut", 20.0, false);
        assert_eq!(pose(&p, &mut sampler, &mut played, 20.0), full);
        assert_eq!(pose(&p, &mut sampler, &mut played, 22.0), rest);
        // Stopped, `open` has its say again.
        let shut = BTreeMap::from([(asset.to_string(), 1)]);
        p.stop(&ClipTarget::all("g1"), Some(&shut));
        assert_eq!(pose(&p, &mut sampler, &mut played, 22.0), full);
        // Looped: 1.5 s in is half open again, and a loop later the same.
        play(&mut p, "g1", asset, &r.clips, "open", 30.0, true);
        assert_eq!(pose(&p, &mut sampler, &mut played, 31.5), half);
        assert_eq!(pose(&p, &mut sampler, &mut played, 32.0), rest);
        assert_eq!(pose(&p, &mut sampler, &mut played, 41.5), half);
    }

    #[test]
    fn flashes_come_and_go_by_the_clock() {
        let mut p = Playback::default();
        let flash = |life| Flash {
            id: 0,
            start_s: 2.0,
            position: [1.0, 2.0, 3.0],
            color: [1.0, 0.8, 0.5],
            intensity: 2000.0,
            range_m: 4.0,
            size_m: 0.15,
            life,
        };
        let id = p.flash(flash(Life::Frames(2)));
        assert_eq!(id, 1);
        assert_eq!(p.flashes().len(), 1);
        assert!(!p.playing_on("g1"));
        // Frames at 25 a second: the frames at 2.00 and 2.04 s show it, 2.08 s and later not.
        for (t, shows) in [
            (1.9, false),
            (1.999, true),
            (2.0, true),
            (2.04, true),
            (2.06, false),
            (2.08, false),
            (9.0, false),
        ] {
            assert_eq!(p.flashes()[0].shows_at(t, 25.0), shows, "t = {t}");
        }
        // The same flash at 60 a second lasts 2 frames of 1/60 s.
        assert!(p.flashes()[0].shows_at(2.02, 60.0));
        assert!(!p.flashes()[0].shows_at(2.04, 60.0));
        // Seconds: half a second from 2 s.
        p.flash(flash(Life::Seconds(0.5)));
        assert!(p.flashes()[0].shows_at(2.49, 25.0));
        assert!(!p.flashes()[0].shows_at(2.5, 25.0));
        assert!(p.remove_flash(id));
        assert!(!p.remove_flash(id));
        assert_eq!(p.flashes().len(), 1);
        // The cap: the oldest go.
        for _ in 0..20 {
            p.flash(flash(Life::Frames(1)));
        }
        assert_eq!(p.flashes().len(), MAX_FLASHES);
        assert_eq!(p.flashes()[0].id, 22);
        assert_eq!(p.flashes()[MAX_FLASHES - 1].id, 22 - MAX_FLASHES as u64 + 1);
    }
}
