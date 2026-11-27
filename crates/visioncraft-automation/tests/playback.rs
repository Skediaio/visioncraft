//! `model.play_clip`, `model.stop_clip`, `query.part_pose` and `effect.flash`: a hinge with four
//! clips, placed as one object, through the same host the command line and the MCP server use.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;
use visioncraft_automation::commands;
use visioncraft_automation::{Access, Animations, Headless, RenderRequest, Rendered};
use visioncraft_core::keyframes::{self, Channel, Interpolation, Matrix, Path, Rig, Sampler, Trs};
use visioncraft_core::{Animation, Clip, MAX_FLASHES, Project};

/// The sine and the cosine of 45 degrees.
const Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// The hinge's animation as its file is read: one part, the lid 1 m east (its own origin, where
/// it turns), and four clips, as `query.clips` lists them: 0 `swing` (a quarter turn about Z in
/// a second, linear), 1 unnamed (nothing, a step), 2 `reload` (2 cm forward in 2 s), 3 `swing`
/// again (a step: nothing). Of the doubled name the lowest index plays.
fn hinge() -> Animation {
    let channel =
        |path: Path, interpolation: Interpolation, times: &[f32], values: &[f32]| Channel {
            node: 0,
            path,
            interpolation,
            times: times.to_vec(),
            values: values.to_vec(),
        };
    let clips = vec![
        Clip {
            name: "swing".into(),
            duration: 1.0,
            channels: vec![channel(
                Path::Rotation,
                Interpolation::Linear,
                &[0.0, 1.0],
                &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, Q, Q],
            )],
        },
        Clip {
            name: String::new(),
            duration: 0.0,
            channels: vec![channel(
                Path::Translation,
                Interpolation::Step,
                &[0.0],
                &[0.0, 0.0, 0.0],
            )],
        },
        Clip {
            name: "reload".into(),
            duration: 2.0,
            channels: vec![channel(
                Path::Translation,
                Interpolation::Linear,
                &[0.0, 2.0],
                &[0.0, 0.0, 0.0, 0.0, 0.2, 0.0],
            )],
        },
        Clip {
            name: "swing".into(),
            duration: 0.0,
            channels: vec![channel(
                Path::Rotation,
                Interpolation::Step,
                &[0.0],
                &[0.0, 0.0, 0.0, 1.0],
            )],
        },
    ];
    Animation {
        rig: Some(Rig {
            rest: vec![Trs {
                translation: [1.0, 0.0, 0.0],
                ..Trs::IDENTITY
            }],
            parents: vec![None],
            clips: clips.clone(),
        }),
        clips,
        warnings: Vec::new(),
    }
}

/// The models' animations as a host has them from their files: the hinge's (an imported object
/// `user:uo1`), and the rock's (nothing: it stands still).
struct Files;

impl Animations for Files {
    fn animation(
        &mut self,
        model: &str,
        _project: &Project,
        _state: &visioncraft_automation::HostState,
    ) -> Result<Animation, String> {
        match model {
            "user:uo1" => Ok(hinge()),
            "user:uo2" => Ok(Animation::default()),
            _ => Err(format!("no model `{model}`")),
        }
    }
}

/// A host with the hinge (`user:uo1`) placed once as `g1`, the rock (`user:uo2`) as `g2`.
fn host() -> Headless {
    let mut h = Headless::new(Access::any());
    for (name, file, size_m) in [
        ("Hinge", "objects/hinge.glb", [3.0, 1.0, 0.1]),
        ("Rock", "objects/rock.glb", [2.0, 1.0, 0.1]),
    ] {
        h.session
            .run(
                "object.add",
                json!({"name": name, "file": file, "size_m": size_m}),
            )
            .unwrap();
    }
    for (name, asset) in [("Hinge", "user:uo1"), ("Rock", "user:uo2")] {
        h.session
            .run(
                "planting.add_single",
                json!({"name": name, "asset": asset, "position": [0, 0, 0]}),
            )
            .unwrap();
    }
    h.animations = Some(Box::new(Files));
    h
}

fn run(h: &mut Headless, command: &str, params: Value) -> Result<Value, String> {
    commands::execute(h, command, &params)
}

/// The pose `query.part_pose` answers for the hinge (`g1`) in model space, at clock time `t`.
fn lid(h: &mut Headless, t: f64) -> Matrix {
    let r = run(
        h,
        "query.part_pose",
        json!({"object": "g1#0", "t": t, "space": "model"}),
    )
    .unwrap();
    let m: Vec<f32> = serde_json::from_value(r["plants"][0]["parts"][0].clone()).unwrap();
    std::array::from_fn(|i| m[i])
}

/// The same by hand: clip `clip` of the hinge sampled at `t` from `start`, over the rig's rest.
fn by_hand(clip: usize, start: f64, t: f64, looped: bool) -> Matrix {
    let rig = hinge().rig.unwrap();
    let local = keyframes::sample(&rig.clips[clip], (t - start) as f32, looped, &rig.rest);
    keyframes::model_space(&rig.parents, &local)[0]
}

fn near(a: Matrix, b: Matrix) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

#[test]
fn a_clip_plays_from_its_start_and_the_pose_is_the_sampler_s() {
    let mut h = host();
    let before = (
        h.session.revision(),
        h.session.project().clone(),
        h.session.can_undo(),
    );
    assert_eq!(
        run(&mut h, "query.clips", json!({"model": "user:uo1"})).unwrap(),
        json!({"clips": [
            {"name": "swing", "duration_s": 1.0, "channels": 1},
            {"name": "", "duration_s": 0.0, "channels": 1},
            {"name": "reload", "duration_s": 2.0, "channels": 1},
            {"name": "swing", "duration_s": 0.0, "channels": 1},
        ], "warnings": []})
    );
    let rest = lid(&mut h, 0.0);
    assert_eq!(
        (rest[12], rest[13], rest[0]),
        (1.0, 0.0, 1.0),
        "the lid 1 m east"
    );
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing", "start_s": 2.0}),
    )
    .unwrap();
    assert_eq!(
        r,
        json!({
            "object": "g1", "planting": "g1", "plant": null, "plants": 1,
            "clip": "swing", "start_s": 2.0, "loop": false, "duration_s": 1.0,
            "restarted": false, "playing": 1,
        })
    );
    // Before it starts: at rest. Half way: half open, as the sampler has it, round its own
    // origin (1 m east). At and after the end: the last pose, held.
    assert_eq!(lid(&mut h, 1.0), rest);
    let half = lid(&mut h, 2.5);
    assert_eq!(half, by_hand(0, 2.0, 2.5, false));
    assert_ne!(half, rest);
    assert!(
        (half[0] - Q).abs() < 1e-5 && (half[1] - Q).abs() < 1e-5,
        "{half:?}"
    );
    assert_eq!((half[12], half[13]), (1.0, 0.0));
    for t in [3.0, 3.001, 100.0] {
        assert_eq!(lid(&mut h, t), by_hand(0, 2.0, t, false), "t = {t}");
    }
    // Two readings at the same time are the same.
    assert_eq!(lid(&mut h, 2.5), lid(&mut h, 2.5));
    // Stopped: at rest, whatever the time.
    let r = run(&mut h, "model.stop_clip", json!({"object": "g1"})).unwrap();
    assert_eq!(r, json!({"object": "g1", "stopped": 1, "playing": 0}));
    assert_eq!(lid(&mut h, 2.5), rest);
    assert_eq!(lid(&mut h, 100.0), rest);
    // The pose query names the clips on a posed object, and defaults `t` to the clock.
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1#0", "clip": 0, "start_s": 0}),
    )
    .unwrap();
    h.state.playback.clock = 0.25;
    let r = run(&mut h, "query.part_pose", json!({"object": "g1"})).unwrap();
    assert_eq!(r["t"], json!(0.25));
    assert_eq!(r["object"], json!("g1"));
    assert_eq!(r["plants"][0]["posed"], json!(true));
    assert_eq!(r["plants"][0]["clips"], json!(["swing"]));
    assert_eq!(r["plants"][0]["asset"], json!("user:uo1"));
    assert_eq!(r["plants"][0]["plant"], json!(0));
    let e = run(
        &mut h,
        "query.part_pose",
        json!({"object": "g1", "t": -1.0}),
    )
    .unwrap_err();
    assert!(
        e.contains("`t` must be a time in seconds, 0 or more"),
        "{e}"
    );
    // None of it is an edit.
    assert_eq!(h.session.revision(), before.0);
    assert_eq!(*h.session.project(), before.1);
    assert_eq!(h.session.can_undo(), before.2, "no undo step");
}

#[test]
fn the_clip_rule_is_name_then_index_and_the_start_the_clock() {
    let mut h = host();
    let rest = lid(&mut h, 0.0);
    // `swing` is the first of the two named so: a quarter turn, not the step that does nothing.
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing", "start_s": 0}),
    )
    .unwrap();
    assert_eq!(r["duration_s"], json!(1.0));
    assert_eq!(lid(&mut h, 0.5), by_hand(0, 0.0, 0.5, false));
    // An index, as a number or a numeric string; the unnamed clip by its index.
    for (clip, name, duration) in [
        (json!(2), "reload", 2.0),
        (json!("2"), "reload", 2.0),
        (json!("1"), "", 0.0),
    ] {
        let r = run(
            &mut h,
            "model.play_clip",
            json!({"object": "g1", "clip": clip, "start_s": 0}),
        )
        .unwrap();
        assert_eq!(
            (r["clip"].clone(), r["duration_s"].clone()),
            (json!(name), json!(duration))
        );
    }
    let e = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "nope"}),
    )
    .unwrap_err();
    assert!(
        e.contains("model.play_clip: `clip` for `user:uo1`: no clip `nope`"),
        "{e}"
    );
    assert!(
        e.contains("`swing`") && e.contains("`reload`") && e.contains("1 (unnamed)"),
        "{e}"
    );
    let e = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": 4}),
    )
    .unwrap_err();
    assert!(e.contains("clip 4: the model has 4 clips, from 0"), "{e}");
    let e = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": [0]}),
    )
    .unwrap_err();
    assert!(e.contains("a clip's name or its index"), "{e}");
    let e = run(&mut h, "model.play_clip", json!({"object": "g1"})).unwrap_err();
    assert!(e.contains("missing `clip`"), "{e}");
    let e = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing", "loop": "yes"}),
    )
    .unwrap_err();
    assert!(e.contains("`loop` must be true or false"), "{e}");
    // The start defaults to the session's clock (the `animation_s` of the last picture).
    h.state.playback.stop_all();
    h.state.playback.clock = 7.5;
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "reload"}),
    )
    .unwrap();
    assert_eq!(r["start_s"], json!(7.5));
    // The same clip on the same object starts again; another clip joins it.
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "reload", "start_s": 8}),
    )
    .unwrap();
    assert_eq!(
        (r["restarted"].clone(), r["playing"].clone()),
        (json!(true), json!(1))
    );
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing", "start_s": 8}),
    )
    .unwrap();
    assert_eq!(h.state.playback.plays().len(), 2);
    // Stopping one clip leaves the other; by index or by name it is the same clip.
    let r = run(
        &mut h,
        "model.stop_clip",
        json!({"object": "g1", "clip": "2"}),
    )
    .unwrap();
    assert_eq!(r["stopped"], json!(1));
    assert_eq!(h.state.playback.plays()[0].name, "swing");
    let r = run(
        &mut h,
        "model.stop_clip",
        json!({"object": "g1", "clip": "reload"}),
    )
    .unwrap();
    assert_eq!(r["stopped"], json!(0));
    // Where two clips move the same property, the one started later wins.
    run(&mut h, "model.stop_clip", json!({"object": "g1"})).unwrap();
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": 0, "start_s": 0}),
    )
    .unwrap();
    let open = by_hand(0, 0.0, 2.0, false);
    assert_eq!(lid(&mut h, 2.0), open);
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": 3, "start_s": 1.0}),
    )
    .unwrap();
    assert_eq!(lid(&mut h, 2.0), by_hand(3, 1.0, 2.0, false));
    assert_eq!(
        lid(&mut h, 2.0),
        rest,
        "the later clip is a step that leaves the lid shut"
    );
    run(
        &mut h,
        "model.stop_clip",
        json!({"object": "g1", "clip": 3}),
    )
    .unwrap();
    assert_eq!(lid(&mut h, 2.0), open);
    // Looped: 1.5 s in is half open again, and a loop later the same.
    run(&mut h, "model.stop_clip", json!({"object": "g1"})).unwrap();
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing", "start_s": 0, "loop": true}),
    )
    .unwrap();
    assert_eq!(lid(&mut h, 0.5), lid(&mut h, 1.5));
    assert_eq!(lid(&mut h, 0.5), lid(&mut h, 10.5));
    assert_ne!(lid(&mut h, 0.5), rest);
}

#[test]
fn a_whole_planting_and_one_plant_of_it() {
    let mut h = host();
    // `g2` is a rock: nothing moves there.
    let e = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g2", "clip": 0}),
    )
    .unwrap_err();
    assert!(
        e.contains("`clip` for `user:uo2`: the model has no animation clips"),
        "{e}"
    );
    let e = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g2", "clip": "fire"}),
    )
    .unwrap_err();
    assert!(e.contains("the model has no animation clips"), "{e}");
    let e = run(&mut h, "query.part_pose", json!({"object": "g2#0"})).unwrap_err();
    assert!(
        e.contains("`user:uo2` (plant 0 of `g2`) stands still"),
        "{e}"
    );
    // The hinge twice: a line of two, plants 0 and 1; one plays, the other does not.
    h.session
        .run(
            "planting.add_cluster",
            json!({"name": "Hinges", "center": [8, 0], "radius_m": 1.0, "count": 2,
                "asset_palette": [{"asset": "user:uo1", "weight": 1.0}]}),
        )
        .unwrap();
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g3#1", "clip": "swing", "start_s": 0}),
    )
    .unwrap();
    assert_eq!(
        (r["plant"].clone(), r["plants"].clone()),
        (json!(1), json!(1))
    );
    let r = run(
        &mut h,
        "query.part_pose",
        json!({"object": "g3", "t": 0.5, "space": "model"}),
    )
    .unwrap();
    let plants = r["plants"].as_array().unwrap();
    assert_eq!(plants.len(), 2);
    assert_eq!(
        (plants[0]["plant"].clone(), plants[0]["posed"].clone()),
        (json!(0), json!(false))
    );
    assert_eq!(
        (plants[1]["plant"].clone(), plants[1]["posed"].clone()),
        (json!(1), json!(true))
    );
    assert_eq!(plants[1]["parts"][0], json!(by_hand(0, 0.0, 0.5, false)));
    // Only plant 1 of it.
    let r = run(
        &mut h,
        "query.part_pose",
        json!({"object": "g3#1", "t": 0.5}),
    )
    .unwrap();
    assert_eq!(r["plants"].as_array().unwrap().len(), 1);
    // The whole planting: both.
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g3", "clip": 2}),
    )
    .unwrap();
    assert_eq!(r["plants"], json!(2));
    h.state.playback.clock = 1.0;
    let r = run(&mut h, "query.part_pose", json!({"object": "g3"})).unwrap();
    assert_eq!(r["plants"][0]["posed"], json!(true));
    assert_eq!(r["plants"][0]["clips"], json!(["reload"]));
    assert_eq!(r["plants"][1]["clips"], json!(["swing", "reload"]));
    // Stopping one clip of the whole planting leaves the one plant's play.
    let r = run(
        &mut h,
        "model.stop_clip",
        json!({"object": "g3", "clip": "reload"}),
    )
    .unwrap();
    assert_eq!(r["stopped"], json!(1));
    assert_eq!(h.state.playback.plays().len(), 1);
}

/// The matrix of a placement by hand, as the window draws a plant with it: scale, then
/// rotation about +Y, then translation (column-major).
fn placement_at(p: [f32; 3], rotation_y_deg: f32, scale: f32) -> Matrix {
    let (s, c) = rotation_y_deg.to_radians().sin_cos();
    let k = scale;
    [
        c * k,
        0.0,
        -s * k,
        0.0,
        0.0,
        k,
        0.0,
        0.0,
        s * k,
        0.0,
        c * k,
        0.0,
        p[0],
        p[1],
        p[2],
        1.0,
    ]
}

/// `a` after `b` (both column-major).
fn times(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|i| {
        let (col, row) = (i / 4, i % 4);
        (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum()
    })
}

fn matrix(value: &Value) -> Matrix {
    serde_json::from_value(value.clone()).unwrap()
}

/// A hinge (`user:uo1`) as `g3`, turned a quarter, twice as big, 3 m east and 20 cm up.
fn turned(h: &mut Headless) {
    h.session
        .run(
            "planting.add_single",
            json!({"name": "Turned", "asset": "user:uo1", "position": [3.0, 0.2, -1.5],
                "rotation_y_deg": 90.0, "scale": 2.0}),
        )
        .unwrap();
}

/// `space` (the default, `world`) answers every part as the window draws it: the plant's
/// placement (scale, then Y rotation, then translation) times the model-space pose; `model` is
/// the pose on its own. Each plant tells its placement, so a caller can compose or invert it.
#[test]
fn world_space_is_the_placement_times_the_model_pose() {
    let mut h = host();
    turned(&mut h);
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g3", "clip": "swing", "start_s": 0}),
    )
    .unwrap();
    let placement = placement_at([3.0, 0.2, -1.5], 90.0, 2.0);
    let model = by_hand(0, 0.0, 0.5, false);
    let world = times(&placement, &model);
    assert_ne!(world, model, "the placement is not the identity");
    // The default is world space: as drawn, not the model-space pose.
    let r = run(&mut h, "query.part_pose", json!({"object": "g3", "t": 0.5})).unwrap();
    let plant = &r["plants"][0];
    assert_eq!(
        (plant["plant"].clone(), plant["asset"].clone()),
        (json!(0), json!("user:uo1"))
    );
    assert!(
        near(matrix(&plant["parts"][0]), world),
        "world {:?} vs the pose on its own {model:?}",
        plant["parts"][0]
    );
    assert_eq!(r["space"], json!("world"));
    assert!(near(matrix(&plant["placement"]), placement));
    // Said in as many words: the same.
    let said = run(
        &mut h,
        "query.part_pose",
        json!({"object": "g3", "t": 0.5, "space": "world"}),
    )
    .unwrap();
    assert_eq!(said["plants"][0]["parts"][0], plant["parts"][0]);
    // Model space: the pose on its own, as before; the placement is there too.
    let r = run(
        &mut h,
        "query.part_pose",
        json!({"object": "g3", "t": 0.5, "space": "model"}),
    )
    .unwrap();
    assert_eq!(r["space"], json!("model"));
    assert_eq!(r["plants"][0]["parts"][0], json!(model));
    assert!(near(matrix(&r["plants"][0]["placement"]), placement));
    // And the one composes into the other.
    assert!(near(
        times(
            &matrix(&r["plants"][0]["placement"]),
            &matrix(&r["plants"][0]["parts"][0])
        ),
        world
    ));
}

/// With years after planting shown, the window scales a model to its size then: so does the
/// world pose (`model` stays the pose on its own).
#[test]
fn with_years_shown_the_world_pose_is_scaled_as_the_window_draws_it() {
    let mut h = host();
    turned(&mut h);
    run(&mut h, "view.set", json!({"years": 1})).unwrap();
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g3", "clip": "swing", "start_s": 0}),
    )
    .unwrap();
    // The hinge is 1 m by 3 m (`object.add`): a plant that size, a year after planting, as a
    // fraction of its full height.
    let growth = visioncraft_core::growth::growth_of("user:uo1", Some((1.0, 3.0)));
    let k = growth.size(growth.planted_age_years + 1.0).height;
    assert!((0.1..0.99).contains(&k), "{k}");
    let placement = placement_at([3.0, 0.2, -1.5], 90.0, 2.0 * k);
    let model = by_hand(0, 0.0, 0.5, false);
    let r = run(&mut h, "query.part_pose", json!({"object": "g3", "t": 0.5})).unwrap();
    assert!(near(matrix(&r["plants"][0]["placement"]), placement));
    assert!(near(
        matrix(&r["plants"][0]["parts"][0]),
        times(&placement, &model)
    ));
    let r = run(
        &mut h,
        "query.part_pose",
        json!({"object": "g3", "t": 0.5, "space": "model"}),
    )
    .unwrap();
    assert_eq!(r["plants"][0]["parts"][0], json!(model));
}

/// Two hinges close together as `g3`, and `water.set_sea`'s parameters for a lake half a metre
/// deep over plant 0 (the western one) with dry land where plant 1 stands.
fn lake_over_plant_0(h: &mut Headless) -> Value {
    // Where they stand is the generator's own.
    h.session
        .run(
            "planting.add_cluster",
            json!({"name": "Hinges", "center": [8, 0], "radius_m": 1.0, "count": 2, "seed": 4,
                "asset_palette": [{"asset": "user:uo1", "weight": 1.0}]}),
        )
        .unwrap();
    let g3 = h
        .session
        .project()
        .generators
        .iter()
        .find(|g| g.id() == "g3")
        .cloned()
        .unwrap();
    let ground = |_: f32, _: f32| Some(0.0f32);
    let numbered = visioncraft_core::planting::evaluate_numbered(&g3, &ground).0;
    assert_eq!(
        numbered.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        vec![0, 1]
    );
    let (a, b) = (numbered[0].1.position, numbered[1].1.position);
    assert!(
        b[0] - a[0] > 0.05,
        "plant 1 is east of plant 0: {a:?} {b:?}"
    );
    let edge = (a[0] + b[0]) / 2.0;
    json!({"level": 0.5, "outline": [[edge - 3.0, -60.0], [edge, -60.0], [edge, 60.0], [edge - 3.0, 60.0]]})
}

/// Only the plants the window draws are played and read: one under the sea is left out, the
/// other keeps its number, and naming the one under the sea is an error that says so.
#[test]
fn a_plant_under_the_sea_is_not_reached() {
    let mut h = host();
    let lake = lake_over_plant_0(&mut h);
    run(&mut h, "water.set_sea", lake.clone()).unwrap();
    // The planting: plant 1 alone, under its own number.
    let r = run(&mut h, "query.part_pose", json!({"object": "g3"})).unwrap();
    assert_eq!(
        r["plants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["plant"].clone())
            .collect::<Vec<_>>(),
        vec![json!(1)],
        "{r}"
    );
    // Plant 0, named: an error that says why.
    for (command, params) in [
        ("query.part_pose", json!({"object": "g3#0"})),
        ("model.play_clip", json!({"object": "g3#0", "clip": 0})),
    ] {
        let e = run(&mut h, command, params).unwrap_err();
        assert!(
            e.contains("plant 0 of planting `g3` is not drawn: it is under the sea"),
            "{command}: {e}"
        );
    }
    // Without the sea, both.
    run(&mut h, "water.remove_sea", json!({})).unwrap();
    let r = run(&mut h, "query.part_pose", json!({"object": "g3"})).unwrap();
    assert_eq!(r["plants"].as_array().unwrap().len(), 2);
}

/// A play is always stopped, also one on a plant that was hidden after its clip started:
/// `model.stop_clip` reaches every plant the recipe places, drawn or not.
#[test]
fn a_clip_on_a_plant_hidden_since_is_still_stopped() {
    let mut h = host();
    let lake = lake_over_plant_0(&mut h);
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g3#0", "clip": "swing", "start_s": 0, "loop": true}),
    )
    .unwrap();
    assert_eq!(r["playing"], json!(1));
    run(&mut h, "water.set_sea", lake.clone()).unwrap();
    // Not drawn any more: not played, not read.
    let e = run(&mut h, "query.part_pose", json!({"object": "g3#0"})).unwrap_err();
    assert!(e.contains("is not drawn: it is under the sea"), "{e}");
    // Stopped all the same.
    let r = run(&mut h, "model.stop_clip", json!({"object": "g3#0"})).unwrap();
    assert_eq!(r, json!({"object": "g3#0", "stopped": 1, "playing": 0}));
    assert!(h.state.playback.is_empty(), "the play is gone");
    // Seen once the sea is lowered again: at rest, no clip on it.
    run(&mut h, "water.remove_sea", json!({})).unwrap();
    let r = run(&mut h, "query.part_pose", json!({"object": "g3#0"})).unwrap();
    assert_eq!(
        (
            r["plants"][0]["posed"].clone(),
            r["plants"][0]["clips"].clone()
        ),
        (json!(false), json!([])),
        "{r}"
    );
}

/// A planting on a hidden layer draws nothing: it is not played or read, naming it says so, and
/// shown again it is. Stopping needs no layer shown.
#[test]
fn a_planting_on_a_hidden_layer_is_not_reached() {
    let mut h = host();
    let layer = h.session.project().generators[0].layer().to_string();
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing", "loop": true}),
    )
    .unwrap();
    run(
        &mut h,
        "layer.set_visible",
        json!({"id": layer, "visible": false}),
    )
    .unwrap();
    for (command, params) in [
        ("query.part_pose", json!({"object": "g1#0"})),
        ("model.play_clip", json!({"object": "g1#0", "clip": 0})),
    ] {
        let e = run(&mut h, command, params).unwrap_err();
        assert!(
            e.contains("plant 0 of planting `g1` is not drawn: its layer is hidden"),
            "{command}: {e}"
        );
    }
    let e = run(&mut h, "query.part_pose", json!({"object": "g1"})).unwrap_err();
    assert!(
        e.contains(&format!(
            "planting `g1` is not drawn: its layer `{layer}` is hidden"
        )),
        "{e}"
    );
    // Stopping needs no layer shown.
    let r = run(&mut h, "model.stop_clip", json!({"object": "g1"})).unwrap();
    assert_eq!(r["stopped"], json!(1));
    // Shown again: reached.
    run(
        &mut h,
        "layer.set_visible",
        json!({"id": layer, "visible": true}),
    )
    .unwrap();
    let r = run(&mut h, "query.part_pose", json!({"object": "g1"})).unwrap();
    assert_eq!(r["plants"].as_array().unwrap().len(), 1);
}

/// `space` is `world` or `model`: anything else is an error that names both.
#[test]
fn a_bad_space_is_an_error() {
    let mut h = host();
    for bad in [json!("mars"), json!(1), json!(true)] {
        let e = run(
            &mut h,
            "query.part_pose",
            json!({"object": "g1", "space": bad}),
        )
        .unwrap_err();
        assert!(
            e.contains("`space` must be `world` or `model`"),
            "{bad}: {e}"
        );
    }
}

/// Stopping a planting stops every play on it, those on one of its objects too; stopping one
/// object while the whole planting plays is an error that says to stop the planting.
#[test]
fn stopping_a_planting_stops_its_objects_too() {
    let mut h = host();
    h.session
        .run(
            "planting.add_cluster",
            json!({"name": "Hinges", "center": [8, 0], "radius_m": 1.0, "count": 2,
                "asset_palette": [{"asset": "user:uo1", "weight": 1.0}]}),
        )
        .unwrap();
    for object in ["g3#0", "g3#1"] {
        run(
            &mut h,
            "model.play_clip",
            json!({"object": object, "clip": "swing", "start_s": 0}),
        )
        .unwrap();
    }
    let r = run(&mut h, "model.stop_clip", json!({"object": "g3"})).unwrap();
    assert_eq!(r["stopped"], json!(2), "{r}");
    assert!(h.state.playback.plays().is_empty());
    // The whole planting plays: one of its objects cannot be stopped on its own.
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g3", "clip": "swing", "start_s": 0}),
    )
    .unwrap();
    let e = run(&mut h, "model.stop_clip", json!({"object": "g3#1"})).unwrap_err();
    assert!(e.contains("stop `g3`"), "{e}");
    assert_eq!(h.state.playback.plays().len(), 1, "nothing was stopped");
}

#[test]
fn errors_name_the_object_the_clip_and_the_parameters() {
    let mut h = host();
    for (object, word) in [
        ("g9", "no planting `g9`; the plantings are: g1, g2"),
        ("g1#5", "planting `g1` has no plant 5"),
        ("m1", "no planting `m1`"),
        ("", "an object is a planting"),
        ("g1#", "an object is a planting"),
        (
            "g1#x",
            "an object is a planting (`g4`) or one of its objects (`g4#12`), not `g1#x`",
        ),
    ] {
        let e = run(
            &mut h,
            "model.play_clip",
            json!({"object": object, "clip": 0}),
        )
        .unwrap_err();
        assert!(
            e.contains(&format!("model.play_clip: `object`: {word}")),
            "{object}: {e}"
        );
        let e = run(&mut h, "model.stop_clip", json!({"object": object})).unwrap_err();
        assert!(e.contains(word), "{object}: {e}");
        let e = run(&mut h, "query.part_pose", json!({"object": object})).unwrap_err();
        assert!(e.contains(word), "{object}: {e}");
    }
    let e = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": 0, "loops": true}),
    )
    .unwrap_err();
    assert!(e.contains("unknown parameter `loops`"), "{e}");
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": 0, "start_s": -7.5}),
    )
    .unwrap();
    assert_eq!(
        r["start_s"],
        json!(-7.5),
        "a start before the clock is a clip already playing"
    );
    let e = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": 0, "start_s": "soon"}),
    )
    .unwrap_err();
    assert!(
        e.contains("`start_s` must be a number, got a string"),
        "{e}"
    );
    // A surface fill's plants follow its owner's faces: not addressed one by one.
    h.session
        .run(
            "shape.add_area",
            json!({"name": "Lawn", "outline": [[0, 0], [4, 0], [4, 3], [0, 3]]}),
        )
        .unwrap();
    let r = run(
        &mut h,
        "planting.add_surface_fill",
        json!({"name": "Tufts", "owner": "S1", "faces": ["S1:top"],
               "asset_palette": [{"asset": "flora:lawn-tuft", "weight": 1.0}], "density_per_m2": 20.0}),
    )
    .unwrap();
    let id = r["id"].as_str().unwrap().to_string();
    let e = run(&mut h, "model.play_clip", json!({"object": id, "clip": 0})).unwrap_err();
    assert!(e.contains("is a surface fill"), "{e}");
    // Nothing known of any model: a bare session.
    let mut bare = Headless::new(Access::default());
    bare.session = visioncraft_core::Session::new(h.session.project().clone());
    let e = run(
        &mut bare,
        "model.play_clip",
        json!({"object": "g1", "clip": 0}),
    )
    .unwrap_err();
    assert!(e.contains("no model files"), "{e}");
    let e = run(&mut bare, "query.part_pose", json!({"object": "g1"})).unwrap_err();
    assert!(e.contains("no model files"), "{e}");
}

#[test]
fn a_flash_comes_and_goes_by_the_clock() {
    let mut h = host();
    let before = (h.session.revision(), h.session.project().clone());
    h.state.playback.clock = 10.0;
    let r = run(
        &mut h,
        "effect.flash",
        json!({"position": [1, 2, 3], "color": [1, 0.8, 0.5], "intensity": 2000, "range_m": 4}),
    )
    .unwrap();
    assert_eq!(
        r,
        json!({"id": 1, "start_s": 10.0, "size_m": 0.15, "flashes": 1, "frames": 2})
    );
    let f = h.state.playback.flashes()[0].clone();
    assert_eq!(
        (f.position, f.color, f.intensity, f.range_m, f.size_m),
        ([1.0, 2.0, 3.0], [1.0, 0.8, 0.5], 2000.0, 4.0, 0.15)
    );
    assert!(matches!(f.life, visioncraft_core::Life::Frames(2)));
    assert!(f.shows_at(10.0, 60.0) && f.shows_at(10.02, 60.0) && !f.shows_at(10.04, 60.0));
    // Seconds instead, no quad, and a start of its own.
    let r = run(
        &mut h,
        "effect.flash",
        json!({"position": [0, 1, 0], "color": [1, 1, 1], "intensity": 500, "range_m": 2,
               "size_m": 0, "seconds": 0.5, "start_s": 20}),
    )
    .unwrap();
    assert_eq!(r["id"], json!(2));
    assert_eq!(r["seconds"], json!(0.5));
    assert!(r.get("frames").is_none());
    // At most 16 at once: the oldest of those goes (the one at 20 s is not at the same time).
    for _ in 0..20 {
        run(
            &mut h,
            "effect.flash",
            json!({"position": [0, 0, 0], "color": [1, 1, 1], "intensity": 1, "range_m": 1}),
        )
        .unwrap();
    }
    let r = run(
        &mut h,
        "effect.flash",
        json!({"position": [0, 0, 0], "color": [1, 1, 1], "intensity": 1, "range_m": 1}),
    )
    .unwrap();
    assert_eq!(r["flashes"], json!(MAX_FLASHES + 1));
    assert!(r["note"].as_str().unwrap().contains("oldest"), "{r}");
    assert_eq!(h.state.playback.flashes().len(), MAX_FLASHES + 1);
    assert_eq!(h.state.playback.flashes()[0].id, 23);
    assert!(h.state.playback.flashes().iter().any(|f| f.id == 2));
    // The parameters are checked.
    for (params, word) in [
        (
            json!({"position": [0, 0], "color": [1, 1, 1], "intensity": 1, "range_m": 1}),
            "`position`",
        ),
        (
            json!({"position": [0, 0, 0], "color": [2, 1, 1], "intensity": 1, "range_m": 1}),
            "`color` must be three numbers from 0 to 1",
        ),
        (
            json!({"position": [0, 0, 0], "color": [1, 1, 1], "intensity": -1, "range_m": 1}),
            "`intensity` must be a number from 0",
        ),
        (
            json!({"position": [0, 0, 0], "color": [1, 1, 1], "intensity": 1}),
            "missing `range_m`",
        ),
        (
            json!({"position": [0, 0, 0], "color": [1, 1, 1], "intensity": 1, "range_m": 1, "frames": 2, "seconds": 0.1}),
            "give one",
        ),
        (
            json!({"position": [0, 0, 0], "color": [1, 1, 1], "intensity": 1, "range_m": 1, "frames": 0}),
            "`frames` must be a whole number from 1",
        ),
        (
            json!({"position": [0, 0, 0], "color": [1, 1, 1], "intensity": 1, "range_m": 1, "flash": 1}),
            "unknown parameter `flash`",
        ),
    ] {
        let e = run(&mut h, "effect.flash", params.clone()).unwrap_err();
        assert!(e.contains(word), "{params}: {e}");
    }
    // Not an edit.
    assert_eq!(h.session.revision(), before.0);
    assert_eq!(*h.session.project(), before.1);
}

/// A window open for a day has a clock around 86 400 s, where an `f32` steps in 7.8 ms: more
/// than a frame at 144 a second. A one-frame flash started "now" then fell between two frames
/// or showed on the next one, and a clip's start went the same way. The start is the clock
/// itself, or the number given, not its `f32` rounding.
#[test]
fn the_start_is_the_clock_to_its_own_precision() {
    let mut h = host();
    // Not a number `f32` holds: there it rounds up to 86 400.007 812 5, into the next frame.
    let clock = 86_400.004;
    h.state.playback.clock = clock;
    h.state.playback.fps = 144.0;
    let r = run(
        &mut h,
        "effect.flash",
        json!({"position": [0, 1, 0], "color": [1, 1, 1], "intensity": 100, "range_m": 2, "frames": 1}),
    )
    .unwrap();
    assert_eq!(r["start_s"], json!(clock));
    let f = h.state.playback.flashes()[0].clone();
    assert!(f.shows_at(clock, 144.0), "the frame it started on");
    assert!(!f.shows_at(clock + 1.0 / 144.0, 144.0), "not the next one");
    // A clip's start, given or defaulted to the clock now, is kept as given too.
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing", "start_s": 100_000.251}),
    )
    .unwrap();
    assert_eq!(r["start_s"], json!(100_000.251));
    let r = run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing"}),
    )
    .unwrap();
    assert_eq!(
        (r["start_s"].clone(), r["restarted"].clone()),
        (json!(clock), json!(true))
    );
}

/// The cap is on flashes at once: twenty that follow one another over ten seconds are all
/// kept for the film, and a live flash that is over goes when the next one comes.
#[test]
fn the_flash_cap_counts_flashes_at_once() {
    let mut h = host();
    for i in 0..20 {
        let r = run(
            &mut h,
            "effect.flash",
            json!({"position": [0, 1, 0], "color": [1, 1, 1], "intensity": 100, "range_m": 2,
                   "seconds": 0.2, "start_s": i as f64 * 0.5}),
        )
        .unwrap();
        assert!(r.get("note").is_none(), "{r}");
    }
    assert_eq!(
        h.state.playback.flashes().len(),
        20,
        "none overlaps another"
    );
    // Live: one at the clock now, over a second later; the next one drops it.
    h.state.playback.clock = 30.0;
    let live = run(
        &mut h,
        "effect.flash",
        json!({"position": [0, 1, 0], "color": [1, 1, 1], "intensity": 100, "range_m": 2}),
    )
    .unwrap();
    assert_eq!(h.state.playback.flashes().len(), 21);
    h.state.playback.clock = 31.0;
    run(
        &mut h,
        "effect.flash",
        json!({"position": [0, 1, 0], "color": [1, 1, 1], "intensity": 100, "range_m": 2}),
    )
    .unwrap();
    let ids: Vec<u64> = h.state.playback.flashes().iter().map(|f| f.id).collect();
    assert_eq!(ids.len(), 21, "{ids:?}");
    assert!(
        !ids.contains(&live["id"].as_u64().unwrap()),
        "the live one is over"
    );
}

/// Flashes on a film's time are kept up to 256; the answer says when one had to go.
#[test]
fn the_answer_says_when_a_film_flash_was_dropped() {
    let mut h = host();
    let flash = |h: &mut Headless, i: usize| {
        run(
            h,
            "effect.flash",
            json!({"position": [0, 1, 0], "color": [1, 1, 1], "intensity": 100, "range_m": 2,
                   "seconds": 0.1, "start_s": i as f64}),
        )
        .unwrap()
    };
    for i in 0..256 {
        assert!(flash(&mut h, i).get("note").is_none());
    }
    let r = flash(&mut h, 256);
    assert!(r["note"].as_str().unwrap().contains("ends first"), "{r}");
    assert_eq!(r["flashes"], json!(256));
}

/// What plays belongs to the project it played on: an opened or a new project starts with
/// nothing playing and no flash.
#[test]
fn an_opened_or_new_project_plays_nothing() {
    let mut h = host();
    let file = dir("reopen").join("hinge.visioncraft");
    run(&mut h, "file.save", json!({"path": file})).unwrap();
    let busy = |h: &mut Headless| {
        run(
            h,
            "model.play_clip",
            json!({"object": "g1", "clip": "swing", "start_s": 0, "loop": true}),
        )
        .unwrap();
        run(
            h,
            "effect.flash",
            json!({"position": [0, 1, 0], "color": [1, 1, 1], "intensity": 100, "range_m": 2, "start_s": 0}),
        )
        .unwrap();
        assert!(!h.state.playback.is_empty());
    };
    busy(&mut h);
    run(&mut h, "file.open", json!({"path": file})).unwrap();
    let r = run(&mut h, "query.part_pose", json!({"object": "g1", "t": 0.5})).unwrap();
    assert_eq!(
        (
            r["plants"][0]["posed"].clone(),
            r["plants"][0]["clips"].clone()
        ),
        (json!(false), json!([])),
        "{r}"
    );
    assert!(
        h.state.playback.is_empty(),
        "opened: nothing plays or flashes"
    );
    busy(&mut h);
    run(&mut h, "file.new", json!({})).unwrap();
    assert!(h.state.playback.is_empty(), "new: nothing plays or flashes");
}

/// Pictures that draw nothing and remember what they were asked for.
struct Recorder(std::rc::Rc<std::cell::RefCell<Vec<RenderRequest>>>);

impl visioncraft_automation::Pictures for Recorder {
    fn render(
        &mut self,
        _project: &Project,
        _models: &BTreeMap<String, visioncraft_automation::LoadedModel>,
        request: &RenderRequest,
    ) -> Result<Rendered, String> {
        self.0.borrow_mut().push(request.clone());
        Ok(Rendered {
            png: one_pixel_png(),
            info: json!({"renderer": "test"}),
        })
    }
}

fn one_pixel_png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, 1, 1);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&[0; 4]).unwrap();
    }
    out
}

/// A temp dir of this test's own.
fn dir(name: &str) -> PathBuf {
    // Tests may run side by side in another job: a folder of this process's own.
    let d = std::env::temp_dir().join(format!(
        "visioncraft-playback-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_film_shows_the_same_frames_every_time() {
    let mut h = host();
    let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    h.pictures = Some(Box::new(Recorder(requests.clone())));
    h.session
        .run("film.add", json!({"name": "F", "fps": 10}))
        .unwrap();
    h.session
        .run(
            "film.add_shot",
            json!({"id": "f1", "shot": {"kind": "hold", "eye": [0, 1, 4], "target": [0, 0, 0], "duration_s": 0.5}}),
        )
        .unwrap();
    run(
        &mut h,
        "model.play_clip",
        json!({"object": "g1", "clip": "swing", "start_s": 0}),
    )
    .unwrap();
    run(
        &mut h,
        "effect.flash",
        json!({"position": [0, 1, 0], "color": [1, 0.8, 0.5], "intensity": 2000, "range_m": 4, "frames": 2}),
    )
    .unwrap();
    let out = dir("film");
    let a = commands::execute(
        &mut h,
        "film.render",
        &json!({"id": "f1", "dir": out.join("a"), "width": 16, "height": 16}),
    )
    .unwrap();
    assert_eq!(a["frames"], json!(5), "{a}");
    let b = commands::execute(
        &mut h,
        "film.render",
        &json!({"id": "f1", "dir": out.join("b"), "width": 16, "height": 16}),
    )
    .unwrap();
    assert_eq!(a["frames"], b["frames"]);
    let first: Vec<RenderRequest> = requests.borrow().iter().take(5).cloned().collect();
    assert_eq!(requests.borrow().len(), 10, "two renders of five frames");
    // Each frame is asked for at the film's time, with the film's rate; the same every time.
    for (i, (r, again)) in first
        .iter()
        .zip(requests.borrow().iter().skip(5))
        .enumerate()
    {
        let t = i as f64 / 10.0;
        assert_eq!(r.animation_s, t, "frame {i}");
        assert_eq!(r.fps, 10.0);
        assert_eq!(r.playback, again.playback, "frame {i}");
        assert_eq!(r.playback.clock, t, "the session's clock is the picture's");
        // The flash stays listed through the film (the host prunes it afterwards); the frames
        // past its end simply do not show it.
        let flash = &r.playback.flashes()[0];
        assert_eq!(
            flash.shows_at(t, r.fps),
            i < 2,
            "frame {i} shows the flash: {}",
            flash.id
        );
    }
    // The playing clip reads the same in both renders: the pose at each frame's time.
    let rig = hinge().rig.unwrap();
    let mut sampler = Sampler::default();
    for (r, again) in first.iter().zip(requests.borrow().iter().skip(5)) {
        let a = sampler
            .pose(&rig, &played_of(&r.playback), r.animation_s)
            .to_vec();
        let b = sampler
            .pose(&rig, &played_of(&again.playback), again.animation_s)
            .to_vec();
        assert_eq!(a, b);
        if r.animation_s == 0.3 {
            assert!(near(a[0], by_hand(0, 0.0, 0.3, false)));
        }
    }
    // The film's rate does not outlast the film: a still is 60 a second again.
    assert_eq!(h.state.playback.fps, 60.0);
}

/// A still's frames are 60 a second whatever rate the host draws at (a window's own): only a
/// film sets the film's.
#[test]
fn a_still_is_sixty_frames_a_second() {
    let mut h = host();
    let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    h.pictures = Some(Box::new(Recorder(requests.clone())));
    // As a window has it: its own rate live.
    h.state.playback.fps = 30.0;
    let out = dir("still");
    run(
        &mut h,
        "view.render",
        json!({"width": 16, "height": 16, "path": out.join("still.png")}),
    )
    .unwrap();
    assert_eq!(requests.borrow()[0].fps, 60.0);
    assert_eq!(requests.borrow()[0].playback.fps, 60.0);
    assert_eq!(
        h.state.playback.fps, 30.0,
        "the host's own rate is left alone"
    );
}

/// Pictures that fail from the third on, and remember what they were asked for.
struct FailingFrom3(std::rc::Rc<std::cell::RefCell<Vec<RenderRequest>>>);

impl visioncraft_automation::Pictures for FailingFrom3 {
    fn render(
        &mut self,
        _project: &Project,
        _models: &BTreeMap<String, visioncraft_automation::LoadedModel>,
        request: &RenderRequest,
    ) -> Result<Rendered, String> {
        let mut asked = self.0.borrow_mut();
        asked.push(request.clone());
        if asked.len() >= 3 {
            return Err("the GPU is gone".into());
        }
        Ok(Rendered {
            png: one_pixel_png(),
            info: json!({"renderer": "test"}),
        })
    }
}

/// A film that fails half way leaves no film rate behind: the next still is 60 a second.
#[test]
fn a_failed_film_leaves_no_rate_behind() {
    let mut h = host();
    let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    h.pictures = Some(Box::new(FailingFrom3(requests.clone())));
    h.session
        .run("film.add", json!({"name": "F", "fps": 10}))
        .unwrap();
    h.session
        .run(
            "film.add_shot",
            json!({"id": "f1", "shot": {"kind": "hold", "eye": [0, 1, 4], "target": [0, 0, 0], "duration_s": 0.5}}),
        )
        .unwrap();
    let out = dir("failed-film");
    let e = commands::execute(
        &mut h,
        "film.render",
        &json!({"id": "f1", "dir": out.join("a"), "width": 16, "height": 16}),
    )
    .unwrap_err();
    assert!(e.contains("the GPU is gone"), "{e}");
    assert_eq!(
        requests.borrow()[1].fps,
        10.0,
        "the film's frames are the film's rate"
    );
    assert_eq!(h.state.playback.fps, 60.0);
    // The next still (it fails too, but is asked for at 60 a second).
    let _ = run(
        &mut h,
        "view.render",
        json!({"width": 16, "height": 16, "path": out.join("still.png")}),
    );
    assert_eq!(requests.borrow().last().unwrap().fps, 60.0);
}

fn played_of(playback: &visioncraft_core::Playback) -> Vec<visioncraft_core::Played> {
    let spot = visioncraft_core::Spot {
        planting: "g1",
        plant: Some(0),
        asset: "user:uo1",
    };
    playback.played(&spot).collect()
}

#[test]
fn both_protocols_carry_the_commands() {
    let mut h = host();
    let line = r#"{"id": 1, "method": "engine.execute", "params": {"command": "model.play_clip", "params": {"object": "g1", "clip": "swing"}}}"#;
    let reply: Value =
        serde_json::from_str(&visioncraft_automation::rpc::handle_line(&mut h, line).unwrap())
            .unwrap();
    assert_eq!(reply["ok"], json!(true), "{reply}");
    assert_eq!(reply["result"]["clip"], json!("swing"));

    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "command_run", "arguments": {"id": "query.part_pose", "params": {"object": "g1", "t": 0.5}}}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "command_run", "arguments": {"id": "effect.flash", "params": {"position": [0, 1, 0], "color": [1, 1, 1], "intensity": 100, "range_m": 2}}}}),
    ]
    .iter()
    .map(|m| format!("{m}\n"))
    .collect::<String>();
    let mut output = Vec::new();
    visioncraft_automation::mcp::serve(&mut h, Cursor::new(input.into_bytes()), &mut output)
        .unwrap();
    let replies: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let text = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
    let pose: Value = serde_json::from_str(text).unwrap();
    assert_eq!(pose["plants"][0]["posed"], json!(true));
    assert_eq!(
        pose["plants"][0]["parts"][0],
        json!(by_hand(0, 0.0, 0.5, false))
    );
    let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    let flash: Value = serde_json::from_str(text).unwrap();
    assert_eq!(flash["id"], json!(1));
}

#[test]
fn the_docs_list_the_commands() {
    let h = Headless::new(Access::default());
    let docs = commands::docs(&h.session);
    for (id, params, result) in [
        (
            "model.play_clip",
            vec!["object", "clip", "start_s", "loop"],
            "{object, planting, plant, plants, clip, start_s, loop, duration_s, restarted, playing}",
        ),
        (
            "model.stop_clip",
            vec!["object", "clip"],
            "{object, stopped, playing}",
        ),
        (
            "query.part_pose",
            vec!["object", "t", "space"],
            "{object, t, space, plants: [{plant, asset, placement, posed, clips, parts: [[16 numbers, column-major], ...]}]}",
        ),
        (
            "effect.flash",
            vec![
                "position",
                "color",
                "intensity",
                "range_m",
                "size_m",
                "frames",
                "seconds",
                "start_s",
            ],
            "{id, start_s, frames | seconds, size_m, flashes, note?}",
        ),
    ] {
        let d = docs
            .iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("documented: {id}"));
        let names: Vec<&str> = d.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, params, "{id}");
        assert_eq!(d.result.as_deref(), Some(result), "{id}");
        assert!(!d.undoable, "{id}");
        let list = commands::command_list(
            &h.session,
            &visioncraft_automation::commands::ListQuery {
                id: Some(id),
                brief: true,
                ..Default::default()
            },
        );
        assert_eq!(list["commands"][0]["id"], id);
    }
    let table = include_str!("../docs/control-protocol.md");
    for id in [
        "model.play_clip",
        "model.stop_clip",
        "query.part_pose",
        "effect.flash",
    ] {
        assert!(
            table.lines().any(|l| l.starts_with(&format!("| `{id}` |"))),
            "control-protocol.md has no row for {id}"
        );
    }
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("| `query.clips` |") && l.contains("warnings"))
    );
}
