//! The wind sway, headless: a CPU mirror (src/wind.rs) of common.wgsl's `wind_sway`, checked
//! against the shader through a tiny compute probe (tests/wind_probe.wgsl), and the properties
//! the animation has to keep: still air is still, a trunk's base is fixed, a crown top moves the
//! same whichever level draws it, phases never jump, the lean grows with the wind up to
//! saturation and never leaves the culling bounds. Run with `--nocapture` for the numbers.

use glam::Vec3;
use visioncraft_render::headless_device;
use visioncraft_render::wind::{self, Phases, Uniforms, Wind};

/// Beaufort force to m/s (WMO): v = 0.836 B^1.5.
fn mps(force: u32) -> f32 {
    0.836 * (force as f32).powf(1.5)
}

fn wind_at(speed: f32, gustiness: f32) -> Wind {
    Wind {
        direction: Vec3::new(0.6, 0.0, 0.8),
        speed_mps: speed,
        gustiness,
        gusts: 0.0,
    }
}

/// The uniforms at time `t` after a constant wind with gusts since 0.
fn gusty(speed: f32, gusts: f32, t: f64) -> Uniforms {
    let mut p = Phases::default();
    p.advance(t, speed);
    Uniforms::new(
        &Wind {
            gusts,
            ..wind_at(speed, 0.5)
        },
        &p,
    )
}

/// The uniforms at time `t` after a constant wind since 0.
fn at(speed: f32, gustiness: f32, t: f64) -> Uniforms {
    let mut p = Phases::default();
    p.advance(t, speed);
    Uniforms::new(&wind_at(speed, gustiness), &p)
}

const TREE_ORIGIN: Vec3 = Vec3::new(12.3, 0.0, -4.7);

/// Crown-top point of a tree `h` metres tall (bend weight 1, limb phase 0).
fn crown(h: f32) -> Vec3 {
    Vec3::new(0.0, h, 0.0)
}

#[test]
fn still_air_is_perfectly_still() {
    for t in [0.0, 1.7, 55.5, 5000.0] {
        let u = at(0.0, 1.0, t);
        for i in 0..200 {
            let f = i as f32 / 199.0;
            let rel = Vec3::new(f * 3.0 - 1.0, f * 20.0, f * 2.0);
            assert_eq!(wind::sway(&u, rel, TREE_ORIGIN, f, f, 0.3), rel, "{t} {i}");
            assert_eq!(wind::impostor_sway(&u, rel, TREE_ORIGIN, f, 0.3), rel);
        }
    }
}

#[test]
fn the_trunk_base_stays_at_every_speed() {
    for force in 0..=10 {
        for t in [0.0, 3.3, 41.0, 3000.0] {
            let u = at(mps(force), 1.0, t);
            // Weight 0 (the foot) and a point at the origin: both exactly fixed.
            let foot = Vec3::new(0.2, 0.0, -0.1);
            assert_eq!(wind::sway(&u, foot, TREE_ORIGIN, 0.0, 0.4, 0.3), foot);
            assert_eq!(
                wind::sway(&u, Vec3::ZERO, TREE_ORIGIN, 1.0, 0.4, 0.3),
                Vec3::ZERO
            );
            // A little above the base the weight is small and so is the motion: w^2 has no slope
            // at 0 (Crysis' (1+x)^4 - (1+x)^2 form has the same zero), so it stays a sliver.
            let low = Vec3::new(0.0, 0.5, 0.0);
            let moved = (wind::sway(&u, low, TREE_ORIGIN, 0.02, 0.4, 0.3) - low).length();
            assert!(moved < 0.03, "force {force}: {moved}");
        }
    }
}

#[test]
fn the_crown_top_moves_the_same_in_a_mesh_and_an_impostor() {
    for force in [2, 5, 8] {
        for t in [0.0, 1.7, 5.0, 13.0, 300.0] {
            let u = at(mps(force), 0.5, t);
            for h in [3.0, 12.0, 25.0] {
                let hz = wind::plant_hz(h, false);
                // A mesh tip (weight 1) and the impostor quad's top edge (height share 1).
                let mesh = wind::sway(&u, crown(h), TREE_ORIGIN, 1.0, 0.0, hz);
                let card = wind::impostor_sway(&u, crown(h), TREE_ORIGIN, 1.0, hz);
                assert!((mesh - card).length() < 1e-6, "{force} {t} {h}");
            }
        }
    }
}

#[test]
fn phases_do_not_jump_in_two_hours() {
    // Frame by frame at 60 fps for 2 h of simulated time at Beaufort 5, against the exact phases
    // (f64, from time 0): the integrated ones agree at every frame, including at the 64 s clock
    // wrap and at 4096 s, where the old `time` wrapped and every plant jumped.
    let speed = mps(5);
    let (hz, h) = (wind::plant_hz(15.0, false), 15.0);
    let wind = wind_at(speed, 0.7);
    let mut phases = Phases::default();
    let (mut last, mut worst_step, mut worst_gap) = (None::<Vec3>, 0.0f32, 0.0f32);
    let dt = 1.0 / 60.0;
    for frame in 1..=(7200 * 60) {
        let t = f64::from(frame) * dt;
        phases.advance(t, speed);
        let u = Uniforms::new(&wind, &phases);
        let p = wind::sway(&u, crown(h), TREE_ORIGIN, 1.0, 0.3, hz);
        if let Some(l) = last {
            worst_step = worst_step.max((p - l).length());
        }
        last = Some(p);
        // The exact phases, from scratch, every ~10 s (plus around the wraps).
        if frame % 600 == 0 || (4090..4102).contains(&((t as u32) % 4096)) && frame % 7 == 0 {
            let exact = at(speed, 0.7, t);
            let q = wind::sway(&exact, crown(h), TREE_ORIGIN, 1.0, 0.3, hz);
            worst_gap = worst_gap.max((p - q).length());
        }
    }
    println!(
        "2 h at B5: worst step {worst_step:.4} m per frame, worst gap to exact {worst_gap:.5} m"
    );
    assert!(worst_gap < 5e-3, "{worst_gap}");
    assert!(worst_step < 0.1, "{worst_step}");
    // The same at the moment 4096 s was crossed: the old time-wrap jump was the whole sway.
    let before = wind::sway(
        &at(speed, 0.7, 4095.99),
        crown(h),
        TREE_ORIGIN,
        1.0,
        0.3,
        hz,
    );
    let after = wind::sway(
        &at(speed, 0.7, 4096.01),
        crown(h),
        TREE_ORIGIN,
        1.0,
        0.3,
        hz,
    );
    assert!((after - before).length() < 2.0 * worst_step);
}

#[test]
fn a_change_of_speed_does_not_jump_the_phases() {
    // Beaufort 2 to 8 at t = 10 s: the phases at the instant are the same; only the rates change.
    let mut p = Phases::default();
    p.advance(10.0, mps(2));
    let before = p;
    p.advance(10.0, mps(8));
    assert_eq!(p, before);
    // After it the leaf flutter runs faster: more cycles per second.
    let (mut slow, mut fast) = (Phases::default(), Phases::default());
    slow.advance(1.0, mps(2));
    fast.advance(1.0, mps(8));
    assert!(wind::flutter_hz(mps(8)) > wind::flutter_hz(mps(2)));
    assert_ne!(slow.flutter, fast.flutter);
}

/// Mean and largest downwind displacement of a crown top over 5 minutes.
fn crown_lean(h: f32, speed: f32) -> (f32, f32) {
    let hz = wind::plant_hz(h, false);
    let dir = Vec3::new(0.6, 0.0, 0.8);
    let mut p = Phases::default();
    let wind = wind_at(speed, 0.5);
    let (mut sum, mut peak, n) = (0.0, 0.0f32, 300 * 20);
    for i in 1..=n {
        p.advance(f64::from(i) * 0.05, speed);
        let u = Uniforms::new(&wind, &p);
        let d = wind::sway(&u, crown(h), TREE_ORIGIN, 1.0, 0.0, hz) - crown(h);
        sum += d.dot(dir);
        peak = peak.max(d.length());
    }
    (sum / n as f32, peak)
}

#[test]
fn the_lean_grows_with_the_wind_up_to_saturation() {
    let mut last = 0.0;
    println!("20 m tree, crown top: Beaufort, m/s, mean downwind lean (m), peak (m)");
    for force in 0..=10 {
        let (mean, peak) = crown_lean(20.0, mps(force));
        println!("  B{force:<2} {:5.1} {mean:7.3} {peak:7.3}", mps(force));
        if force == 0 {
            assert_eq!((mean, peak), (0.0, 0.0));
        } else {
            assert!(mean > last, "B{force}: {mean} after {last}");
        }
        last = mean;
    }
    // Saturating: from a gale to a storm the lean grows by far less than the drag does.
    let (g, _) = crown_lean(20.0, mps(8));
    let (s, _) = crown_lean(20.0, mps(10));
    assert!(s / g < (mps(10) / mps(8)).powi(2) * 0.6, "{g} {s}");
}

#[test]
fn nothing_leaves_its_bounds_in_a_storm() {
    // Every point of a plant with any bend weight, in full gusts, for 10 minutes per speed,
    // with and without the gust field (and as lawn grass with it).
    for (extent, force, gusts) in [0.3_f32, 1.0, 2.0, 6.0, 20.0, 40.0]
        .into_iter()
        .flat_map(|e| [0, 2, 5, 8, 10].map(|f| (e, f)))
        .flat_map(|(e, f)| [0.0, 1.0].map(|g| (e, f, g)))
    {
        let speed = mps(force);
        // Lawn grass (a negative frequency) only as tufts.
        let grass = extent <= 0.3 && gusts > 0.0;
        let pad = wind::bounds_pad(extent, speed, gusts, grass);
        let hz = wind::plant_hz(extent, false);
        let hz = if grass { -hz } else { hz };
        let mut seed = 12345u32;
        let mut rand = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let points: Vec<(Vec3, f32, f32)> = (0..64)
            .map(|_| {
                let rel = Vec3::new(rand() - 0.5, rand(), rand() - 0.5) * extent;
                (rel, rand(), rand())
            })
            .collect();
        let mut p = Phases::default();
        let wind = Wind {
            gusts,
            ..wind_at(speed, 1.0)
        };
        let mut worst = 0.0f32;
        for i in 1..=12_000 {
            p.advance(f64::from(i) * 0.05, speed);
            let u = Uniforms::new(&wind, &p);
            for &(rel, w, phase) in &points {
                let d = wind::sway(&u, rel, TREE_ORIGIN, w, phase, hz) - rel;
                worst = worst.max(d.abs().max_element());
                assert!(d.is_finite());
            }
        }
        println!(
            "extent {extent:5.1} m B{force} gusts {gusts}: worst axis excursion {worst:.3} m of pad {pad:.3} m"
        );
        assert!(worst <= pad, "extent {extent}: {worst} over {pad}");
    }
}

/// Counts the sway wave's zero crossings over 128 s (bend only: no gusts, no detail).
fn sway_hz_measured(h: f32, conifer: bool) -> f32 {
    let hz = wind::plant_hz(h, conifer);
    let wind = wind_at(mps(4), 0.0);
    let mut p = Phases::default();
    let mut samples = Vec::new();
    for i in 1..=(128 * 50) {
        p.advance(f64::from(i) * 0.02, mps(4));
        let mut u = Uniforms::new(&wind, &p);
        u.amp[1] = 0.0;
        let d = wind::sway(&u, crown(h), Vec3::ZERO, 1.0, 0.0, hz) - crown(h);
        samples.push(d.dot(Vec3::new(0.6, 0.0, 0.8)));
    }
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    let crossings = samples
        .windows(2)
        .filter(|w| (w[0] - mean) * (w[1] - mean) < 0.0)
        .count();
    crossings as f32 / 2.0 / 128.0
}

#[test]
fn tall_trees_sway_slowly_and_small_plants_fast() {
    println!("natural sway period (s): planned, measured on the wave");
    let cases = [
        ("20 m conifer", 20.0, true),
        ("20 m broadleaf", 20.0, false),
        ("10 m broadleaf", 10.0, false),
        ("2 m shrub", 2.0, false),
        ("0.3 m grass", 0.3, false),
    ];
    let mut last = 0.0;
    for (name, h, conifer) in cases {
        let planned = wind::plant_hz(h, conifer);
        let measured = sway_hz_measured(h, conifer);
        println!("  {name:<16} {:.2} {:.2}", 1.0 / planned, 1.0 / measured);
        // Quantised to whole cycles per 64 s; the wave's crossings count that.
        assert!(
            (measured / planned - 1.0).abs() < 0.08,
            "{name}: {planned} {measured}"
        );
        if name == "20 m conifer" {
            assert!(1.0 / planned > 3.0);
        }
        last = planned;
    }
    assert!(last >= 2.0, "grass sways fast");
    assert!(wind::plant_hz(20.0, false) < wind::plant_hz(2.0, false));
    assert!(wind::plant_hz(20.0, true) > wind::plant_hz(20.0, false));
}

#[test]
fn the_shader_agrees_with_the_cpu() {
    let (device, queue) = pollster::block_on(headless_device()).expect("headless device");
    let source = format!(
        "{}\n{}",
        include_str!("../src/common.wgsl"),
        include_str!("wind_probe.wgsl")
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("wind probe"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("wind probe"),
        layout: None,
        module: &module,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    // Points: tips and limbs of plants of several sizes, mesh and impostor, at several winds.
    let mut points: Vec<[f32; 12]> = Vec::new();
    for i in 0..300 {
        let f = i as f32 / 300.0;
        let h = [0.4, 2.0, 12.0, 25.0][i % 4];
        let origin = Vec3::new(f * 90.0 - 30.0, 0.0, 17.0 - f * 55.0);
        let rel = Vec3::new((f * 7.3).sin() * 0.3 * h, f * h, (f * 5.1).cos() * 0.3 * h);
        let impostor = i % 5 == 0;
        let w = if impostor { f } else { (f * 1.3).fract() };
        // Every seventh small one is lawn grass (a negative frequency).
        let grass = !impostor && h < 1.0 && i % 7 == 0;
        let hz = wind::plant_hz(h, i % 3 == 0);
        points.push([
            rel.x,
            rel.y,
            rel.z,
            w,
            origin.x,
            origin.y,
            origin.z,
            (f * 3.1).fract(),
            if grass { -hz } else { hz },
            f32::from(impostor),
            0.0,
            0.0,
        ]);
    }
    let bytes: Vec<u8> = points
        .iter()
        .flatten()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let storage = |label, size: u64, usage| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let input = storage(
        "points",
        bytes.len() as u64,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    let out_size = points.len() as u64 * 16;
    let output = storage(
        "out",
        out_size,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let params = storage(
        "params",
        64,
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let read = storage(
        "read",
        out_size,
        wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    );
    queue.write_buffer(&input, 0, &bytes);
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 40,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 41,
                resource: output.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 42,
                resource: params.as_entire_binding(),
            },
        ],
    });
    let mut worst = 0.0f32;
    for (force, t, gusts) in [
        (0, 3.0, 0.0),
        (2, 1.7, 0.0),
        (5, 40.0, 0.0),
        (8, 5000.5, 0.0),
        (10, 123.456, 0.0),
        (2, 1.7, 0.4),
        (5, 40.0, 1.0),
        (8, 5000.5, 1.0),
        (10, 123.456, 0.7),
    ] {
        let mut p = Phases::default();
        p.advance(t, mps(force));
        let u = Uniforms::new(
            &Wind {
                gusts,
                ..wind_at(mps(force), 0.6)
            },
            &p,
        );
        let uniform: Vec<u8> = [u.wind, u.phase, u.amp, u.gust]
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        queue.write_buffer(&params, 0, &uniform);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(points.len().div_ceil(64) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &read, 0, out_size);
        queue.submit([encoder.finish()]);
        read.slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let data: Vec<f32> = read
            .slice(..)
            .get_mapped_range()
            .expect("mapped")
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        read.unmap();
        for (i, p) in points.iter().enumerate() {
            let rel = Vec3::new(p[0], p[1], p[2]);
            let origin = Vec3::new(p[4], p[5], p[6]);
            let cpu = if p[9] > 0.5 {
                wind::impostor_sway(&u, rel, origin, p[3], p[8])
            } else {
                wind::sway(&u, rel, origin, p[3], p[7], p[8])
            };
            let gpu = Vec3::new(data[i * 4], data[i * 4 + 1], data[i * 4 + 2]);
            worst = worst.max((cpu - gpu).length());
            assert!(
                (cpu - gpu).length() < 2e-3,
                "B{force} point {i}: cpu {cpu} gpu {gpu}"
            );
        }
    }
    println!("shader vs CPU mirror: worst difference {worst:.6} m over 2700 points");
}

#[test]
fn gusts_off_is_the_sway_as_before() {
    // Gusts 0 runs none of the gust field: the same as a wind without the field, also for a
    // point flagged as grass.
    for (force, t) in [(2, 1.7), (5, 40.0), (8, 5000.5)] {
        let plain = at(mps(force), 0.5, t);
        let off = gusty(mps(force), 0.0, t);
        for i in 0..50 {
            let f = i as f32 / 49.0;
            let rel = Vec3::new(f - 0.5, f * 12.0, 0.3 - f);
            let hz = wind::plant_hz(12.0, false);
            let a = wind::sway(&plain, rel, TREE_ORIGIN, f, f, hz);
            assert_eq!(a, wind::sway(&off, rel, TREE_ORIGIN, f, f, hz));
            assert_eq!(a, wind::sway(&off, rel, TREE_ORIGIN, f, f, -hz));
        }
    }
}

#[test]
fn gusts_travel_downwind_at_the_wind_speed() {
    // The field one second on, a wind's speed further downwind, is the field now (Taylor's
    // frozen turbulence), for the coarse layer that carries the gusts; the fine one drifts
    // slower, so the pair is close, not equal.
    let speed = mps(7);
    let dir = [0.6f32, 0.8f32];
    let (mut same, mut n) = (0.0f32, 0);
    let mut spread = (f32::MAX, f32::MIN);
    for i in 0..400 {
        let x = (i % 20) as f32 * 3.1 - 20.0;
        let z = (i / 20) as f32 * 2.7 - 25.0;
        let now = wind::gust_field(&gusty(speed, 1.0, 10.0), [x, z]);
        let later = wind::gust_field(
            &gusty(speed, 1.0, 11.0),
            [x + dir[0] * speed, z + dir[1] * speed],
        );
        same += (now - later).abs();
        n += 1;
        spread = (spread.0.min(now), spread.1.max(now));
    }
    let mean_gap = same / n as f32;
    println!("gust field: mean change following the flow {mean_gap:.3}, range {spread:?}");
    assert!(mean_gap < 0.15, "{mean_gap}");
    // There are gusts and lulls: the field is not flat.
    assert!(spread.0 < 0.05 && spread.1 > 0.8, "{spread:?}");
    // Standing still, the field changes: gusts pass.
    let (mut moved, mut m) = (0.0f32, 0);
    for i in 0..400 {
        let x = (i % 20) as f32 * 3.1 - 20.0;
        let z = (i / 20) as f32 * 2.7 - 25.0;
        let now = wind::gust_field(&gusty(speed, 1.0, 10.0), [x, z]);
        let later = wind::gust_field(&gusty(speed, 1.0, 11.0), [x, z]);
        moved += (now - later).abs();
        m += 1;
    }
    assert!(
        moved / m as f32 > 1.5 * mean_gap,
        "{} {mean_gap}",
        moved / m as f32
    );
}

#[test]
fn a_gust_bends_grass_far_more_than_wood() {
    // At a gust's core lawn grass bends tens of degrees; the same small plant as wood barely.
    let speed = mps(7);
    // Without the slow swell, so only the gust field differs between the two places.
    let mut u = gusty(speed, 1.0, 10.0);
    u.wind[3] = 0.0;
    // Find a gust's core and a lull.
    let mut core = None;
    let mut lull = None;
    for i in 0..4000 {
        let p = [(i % 80) as f32 * 1.3 - 50.0, (i / 80) as f32 * 1.3 - 30.0];
        let g = wind::gust_field(&u, p);
        if g > 0.95 && core.is_none() {
            core = Some(p);
        }
        if g < 0.02 && lull.is_none() {
            lull = Some(p);
        }
    }
    let (core, lull) = (core.expect("a gust"), lull.expect("a lull"));
    let tip = Vec3::new(0.0, 0.08, 0.0);
    let hz = wind::plant_hz(0.08, false);
    let angle = |at: [f32; 2], hz: f32| {
        let q = wind::sway(&u, tip, Vec3::new(at[0], 0.0, at[1]), 1.0, 0.0, hz);
        (q.y / q.length()).clamp(-1.0, 1.0).acos().to_degrees()
    };
    let (grass_core, grass_lull, wood_core) = (angle(core, -hz), angle(lull, -hz), angle(core, hz));
    println!(
        "blade tip lean: gust {grass_core:.1} deg, lull {grass_lull:.1} deg, wood {wood_core:.1} deg"
    );
    assert!(grass_core > 20.0 && grass_core < 60.0, "{grass_core}");
    assert!(grass_core > 1.8 * grass_lull, "{grass_core} {grass_lull}");
    assert!(wood_core < grass_core / 2.0, "{wood_core}");
}
