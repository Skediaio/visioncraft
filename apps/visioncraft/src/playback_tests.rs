//! What playback draws, on the software adapter: a lid that swings on its clip, and a flash
//! that lights the ground for two frames and is gone. The pictures are written as
//! `kfb_<case>.png` to `$VISIONCRAFT_RENDER_OUT` when it is set, to look at.

use crate::studio::Studio;
use glam::{Mat4, Vec3};
use std::collections::BTreeMap;
use visioncraft_core::{ClipTarget, Flash, Life};

const W: u32 = 320;
const H: u32 = 180;

/// A studio on the headless adapter, or `None` where there is none.
fn studio() -> Option<(wgpu::Device, wgpu::Queue, Studio)> {
    let (device, queue) = pollster::block_on(visioncraft_render::headless_device()).ok()?;
    let renderer =
        visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut s = Studio::new(renderer, visioncraft_core::Project::default());
    s.resize(W, H);
    Some((device, queue, s))
}

/// One frame as RGBA8.
fn frame(s: &mut Studio, device: &wgpu::Device, queue: &wgpu::Queue, clock: f64) -> Vec<u8> {
    s.clock = clock;
    s.render_pixels(device, queue, W, H).expect("a picture")
}

/// A world point in pixels.
fn pixel(s: &Studio, p: Vec3) -> (f32, f32) {
    let (view, proj, _) = s.camera.matrices(W as f32 / H as f32);
    let c = proj * view * p.extend(1.0);
    (
        (c.x / c.w * 0.5 + 0.5) * W as f32,
        (0.5 - c.y / c.w * 0.5) * H as f32,
    )
}

/// The pixels where two frames differ by more than `tolerance` in a channel.
fn changed(a: &[u8], b: &[u8], tolerance: u8) -> Vec<(u32, u32)> {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .enumerate()
        .filter(|(_, (a, b))| {
            a.iter()
                .zip(b.iter())
                .any(|(x, y)| x.abs_diff(*y) > tolerance)
        })
        .map(|(i, _)| (i as u32 % W, i as u32 / W))
        .collect()
}

/// The smallest box round `points` in pixels, grown by `pad`.
fn box_of(points: &[(f32, f32)], pad: f32) -> [f32; 4] {
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in points {
        lo = [lo[0].min(p.0), lo[1].min(p.1)];
        hi = [hi[0].max(p.0), hi[1].max(p.1)];
    }
    [lo[0] - pad, lo[1] - pad, hi[0] + pad, hi[1] + pad]
}

fn inside(b: [f32; 4], (x, y): (u32, u32)) -> bool {
    x as f32 >= b[0] && y as f32 >= b[1] && x as f32 <= b[2] && y as f32 <= b[3]
}

/// Mean linear luminance of the pixels within `radius` of `at`, as the display shows them
/// (0..1).
fn light_around(px: &[u8], at: (f32, f32), radius: f32) -> f32 {
    let (mut sum, mut n) = (0.0, 0);
    for (i, p) in px.as_chunks::<4>().0.iter().enumerate() {
        let d = ((i as u32 % W) as f32 - at.0).hypot((i as u32 / W) as f32 - at.1);
        if d <= radius {
            sum += 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
            n += 1;
        }
    }
    sum / (n.max(1) * 255) as f32
}

fn save(name: &str, px: &[u8]) {
    if let Ok(dir) = std::env::var("VISIONCRAFT_RENDER_OUT") {
        let path = std::path::Path::new(&dir).join(format!("kfb_{name}.png"));
        if let Ok(png) = crate::studio::png_rgb(px, W, H) {
            let _ = std::fs::write(path, png);
        }
    }
}

/// A studio with the test hinge imported and placed once at the origin, looking at it from
/// the south, every model loaded.
fn hinge_studio() -> Option<(wgpu::Device, wgpu::Queue, Studio)> {
    let (device, queue, mut s) = studio()?;
    let bytes = crate::gltf_load::tests::hinge().finish();
    let asset = s.import_object("Hinge", bytes).expect("the hinge imports");
    assert_eq!(asset, "user:uo1");
    s.session
        .run(
            "planting.add_single",
            serde_json::json!({"name": "Hinge", "asset": asset, "position": [0, 0, 0]}),
        )
        .unwrap();
    s.camera.fov_y = 30f32.to_radians();
    s.camera
        .look_from(Vec3::new(1.5, 1.6, 8.0), Vec3::new(1.5, 1.6, 0.0));
    s.settle_for_picture();
    assert_eq!(s.scene.library.resident_models(), 1, "the hinge is loaded");
    assert_eq!(s.scene.animated_parts(), 1, "the lid is an item of its own");
    Some((device, queue, s))
}

/// The lid's corners `quarter` of its swing through its pose in model space.
fn lid_corners(s: &Studio, quarter: f32) -> Vec<(f32, f32)> {
    let angle = quarter * std::f32::consts::FRAC_PI_2;
    let pose = Mat4::from_translation(Vec3::X) * Mat4::from_rotation_z(angle);
    [[2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 0.0]]
        .iter()
        .map(|v| pixel(s, pose.transform_point3(Vec3::from(*v))))
        .collect()
}

/// A planting on a hidden layer is still counted, and draws nothing: neither its static items
/// nor its moving parts.
#[test]
fn a_planting_on_a_hidden_layer_has_no_items() {
    let Some((_device, _queue, mut s)) = hinge_studio() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let shown = s.renderer.item_count();
    let layer = s.session.project().layer_of("g1").unwrap().to_string();
    s.session
        .run(
            "layer.set_visible",
            serde_json::json!({"id": layer, "visible": false}),
        )
        .unwrap();
    s.settle_for_picture();
    assert_eq!(s.scene.gen_stats("g1").map(|g| g.count), Some(1), "still counted");
    assert_eq!(s.scene.animated_parts(), 0, "the lid is not drawn");
    // Back to what the scene draws without it: the body and the lid were at least two.
    assert!(
        s.renderer.item_count() + 2 <= shown,
        "{} items of {shown} are left",
        s.renderer.item_count()
    );
}

#[test]
fn the_lid_swings_on_its_clip_and_the_body_stands_still() {
    let Some((device, queue, mut s)) = hinge_studio() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    // `swing`: a quarter turn about Z in a second, from the identity.
    let clips = BTreeMap::from([("user:uo1".to_string(), 0)]);
    assert!(
        !s.playback
            .play(ClipTarget::all("g1"), clips, "swing", 0.0, false, 1.0)
    );
    let at_start = frame(&mut s, &device, &queue, 0.0);
    let half = frame(&mut s, &device, &queue, 0.5);
    let end = frame(&mut s, &device, &queue, 1.0);
    let held = frame(&mut s, &device, &queue, 9.0);
    save("lid_t0", &at_start);
    save("lid_t05", &half);
    save("lid_t1", &end);
    // Above the horizon the clouds drift with the clock; below it everything still stands
    // still except what the lid and its shadow cover.
    let horizon = pixel(&s, Vec3::new(1.5, 0.0, -1.0e4)).1 + 2.0;
    let below = |a: &[u8], b: &[u8], tolerance: u8| -> Vec<(u32, u32)> {
        changed(a, b, tolerance)
            .into_iter()
            .filter(|p| p.1 as f32 >= horizon)
            .collect()
    };
    // The lid moved between the start and half the clip: where, and only there.
    let moved = below(&at_start, &half, 6);
    assert!(moved.len() > 40, "{} pixels moved", moved.len());
    let lid = [
        box_of(&lid_corners(&s, 0.0), 3.0),
        box_of(&lid_corners(&s, 0.5), 3.0),
    ];
    // What else changed is the lid's shadow on the ground: near its foot.
    let foot = pixel(&s, Vec3::new(2.5, 0.0, 0.0));
    let outside: Vec<_> = moved
        .iter()
        .copied()
        .filter(|p| !lid.iter().any(|b| inside(*b, *p)))
        .collect();
    assert!(
        outside
            .iter()
            .all(|&(x, _)| (x as f32 - foot.0).abs() < 70.0),
        "{} moved pixels elsewhere than the lid and its shadow: {outside:?}",
        outside.len()
    );
    // The body is the same in both: nothing changed within its box.
    let body = box_of(
        &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .iter()
            .map(|v| pixel(&s, Vec3::from(*v)))
            .collect::<Vec<_>>(),
        0.0,
    );
    assert!(moved.iter().all(|p| !inside(body, *p)), "the body moved");
    // Half way is half a quarter (45 degrees), the end a quarter (90 degrees): the lid's tip
    // has left its place at the start and rises further to the end.
    let tip = |q: f32| lid_corners(&s, q)[1];
    let (t0, t5, t1) = (tip(0.0), tip(0.5), tip(1.0));
    assert!(t5.1 < t0.1 - 20.0 && t5.0 < t0.0 - 20.0, "{t0:?} {t5:?}");
    assert!(t1.1 < t5.1 - 10.0, "{t5:?} {t1:?}");
    assert!(below(&at_start, &end, 6).len() > 40);
    // Past the end the last pose holds: the lid is the same picture as at the end, to the
    // pixel (the ground under the drifting clouds is not, quite).
    let lid_end = box_of(&lid_corners(&s, 1.0), -3.0);
    assert!(
        lid_end[0] < lid_end[2] && lid_end[1] < lid_end[3],
        "{lid_end:?}"
    );
    let moved_on: Vec<_> = changed(&end, &held, 2)
        .into_iter()
        .filter(|p| inside(lid_end, *p))
        .collect();
    // (A few pixels catch the sky's reflection, which the drifting clouds change.)
    assert!(
        moved_on.len() < 20,
        "an ended clip holds its pose: {} pixels differ: {moved_on:?}",
        moved_on.len()
    );
    let drift = below(&end, &held, 2).len();
    assert!(drift < 400, "{drift} ground pixels differ under the clouds");
    // Stopped: back to the file's rest, which is neither of these (the file has the lid a
    // quarter open; the clip ends with it shut against the body).
    s.playback.stop(&ClipTarget::all("g1"), None);
    let rest = frame(&mut s, &device, &queue, 9.0);
    save("lid_rest", &rest);
    assert!(!below(&rest, &held, 6).is_empty());
    assert!(!below(&rest, &at_start, 6).is_empty());
    assert_eq!(s.scene.animated_parts(), 1);
}

#[test]
fn a_flash_lights_the_ground_for_two_frames_and_is_gone() {
    let Some((device, queue, mut s)) = studio() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    // Night, lamps off: the ground is dark, so a flash shows.
    s.session
        .run("time.set", serde_json::json!({"hour": 23.0}))
        .unwrap();
    s.camera
        .look_from(Vec3::new(0.0, 1.5, 6.0), Vec3::new(0.0, 0.5, 0.0));
    s.settle_for_picture();
    s.playback.fps = 10.0;
    let at = (0.0f32, 0.8, 0.0);
    let before = frame(&mut s, &device, &queue, 10.0);
    s.playback.flash(Flash {
        id: 0,
        start_s: 10.0,
        position: [at.0, at.1, at.2],
        color: [1.0, 0.8, 0.5],
        intensity: 4000.0,
        range_m: 5.0,
        size_m: 0.15,
        life: Life::Frames(2),
    });
    let first = frame(&mut s, &device, &queue, 10.0);
    let second = frame(&mut s, &device, &queue, 10.1);
    let after = frame(&mut s, &device, &queue, 10.2);
    let later = frame(&mut s, &device, &queue, 10.5);
    save("flash_before", &before);
    save("flash_f0", &first);
    save("flash_f1", &second);
    save("flash_after", &after);
    let where_ = pixel(&s, Vec3::new(at.0, at.1, at.2));
    let (near, far) = (
        light_around(&before, where_, 12.0),
        light_around(&first, where_, 12.0),
    );
    assert!(
        far > near + 0.02,
        "the flash lights its surroundings: {near:.3} before, {far:.3} with it"
    );
    let second_near = light_around(&second, where_, 12.0);
    assert!(
        second_near > near + 0.02,
        "the second frame shows it too: {second_near:.3}"
    );
    // The frame after its last frame is the frame before it, within a small tolerance (the
    // exposure was adapted to the flash for a moment).
    let gone = light_around(&after, where_, 12.0);
    assert!(gone < near + 0.01, "{gone:.3} after, {near:.3} before");
    let big: Vec<_> = changed(&before, &after, 12);
    assert!(
        big.len() < (W * H / 200) as usize,
        "{} pixels differ by much after the flash",
        big.len()
    );
    let later_still = changed(&later, &before, 6);
    assert!(
        later_still.len() < (W * H / 100) as usize,
        "{} pixels differ half a second on",
        later_still.len()
    );
    // And it is forgotten once over.
    s.clock = 10.3;
    s.playback.prune_flashes(10.3);
    assert!(s.playback.flashes().is_empty());
    let no_item = frame(&mut s, &device, &queue, 10.3);
    assert!(changed(&no_item, &after, 6).len() < (W * H / 100) as usize);
}

/// A chest that rises while its lid opens: nothing of it stands still, so the model has no
/// static item, only its two parts. At the clips' first keys the picture is the file's rest
/// pose; a second on, the chest is higher and the lid has swung.
#[test]
fn a_model_all_of_which_moves_draws_its_parts_alone() {
    let Some((device, queue, mut s)) = studio() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut b = crate::gltf_load::tests::Builder::new("steel");
    let (chest, lid) = (
        b.triangle([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
        b.triangle([[2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 0.0]]),
    );
    let c = b.node(serde_json::json!({"mesh": chest, "translation": [0.0, 1.0, 0.0]}));
    let l = b.node(serde_json::json!({"mesh": lid, "translation": [1.0, 0.0, 0.0]}));
    b.child_of(c, l);
    b.animation(
        "lift",
        c,
        "translation",
        "LINEAR",
        &[0.0, 1.0],
        &[0.0, 1.0, 0.0, 0.0, 2.0, 0.0],
    );
    let q = std::f32::consts::FRAC_1_SQRT_2;
    b.animation(
        "open",
        l,
        "rotation",
        "LINEAR",
        &[0.0, 1.0],
        &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, q, q],
    );
    let asset = s
        .import_object("Chest", b.finish())
        .expect("the chest imports");
    s.session
        .run(
            "planting.add_single",
            serde_json::json!({"name": "Chest", "asset": asset, "position": [0, 0, 0]}),
        )
        .unwrap();
    s.camera.fov_y = 30f32.to_radians();
    s.camera
        .look_from(Vec3::new(1.5, 1.8, 9.0), Vec3::new(1.5, 1.8, 0.0));
    s.settle_for_picture();
    assert_eq!(s.scene.library.resident_models(), 1);
    assert_eq!(s.scene.animated_parts(), 2, "the chest and the lid");
    let horizon = pixel(&s, Vec3::new(1.5, 0.0, -1.0e4)).1 + 2.0;
    let below = |a: &[u8], b: &[u8]| -> Vec<(u32, u32)> {
        changed(a, b, 6)
            .into_iter()
            .filter(|p| p.1 as f32 >= horizon)
            .collect()
    };
    let rest = frame(&mut s, &device, &queue, 5.0);
    for (clip, duration) in [("lift", 1.0), ("open", 1.0)] {
        let index = usize::from(clip == "open");
        s.playback.play(
            ClipTarget::all("g1"),
            BTreeMap::from([("user:uo1".to_string(), index)]),
            clip,
            5.0,
            false,
            duration,
        );
    }
    let start = frame(&mut s, &device, &queue, 5.0);
    let end = frame(&mut s, &device, &queue, 6.0);
    save("chest_rest", &rest);
    save("chest_end", &end);
    assert!(
        below(&rest, &start).is_empty(),
        "the clips begin where the file has the chest"
    );
    assert!(below(&rest, &end).len() > 40, "a second on it has moved");
}
