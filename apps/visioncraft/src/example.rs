//! The example garden a new user can open (`project.load_example`, built from commands in the
//! core). It doubles as a regression scene: the test below compares the window's placements
//! with what agents count.

/// The window and agents (`document.inspect`) count the same placements on the example garden:
/// rule 9, what an agent counts is what the window shows.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod parity_tests {
    use crate::studio::Studio;
    use serde_json::Value;
    use visioncraft_automation::{Access, Headless};
    use visioncraft_core::Session;

    /// (agents' counts, the window's counts) per planting of `p`; `None` without a GPU.
    #[allow(clippy::type_complexity)]
    fn counts(p: &visioncraft_core::Project) -> Option<(Vec<(String, u64)>, Vec<(String, u64)>)> {
        // Agents.
        let mut h = Headless::new(Access::default());
        h.session = Session::new(p.clone());
        h.state.catalog.assets = crate::fixtures::assets();
        h.state.sync_models(p);
        let line = r#"{"id": 1, "method": "engine.execute", "params": {"command": "document.inspect", "params": {"faces": false}}}"#;
        let reply: Value =
            serde_json::from_str(&visioncraft_automation::rpc::handle_line(&mut h, line).unwrap())
                .unwrap();
        let agents: Vec<(String, u64)> = reply["result"]["generators"]
            .as_array()
            .unwrap_or_else(|| panic!("{reply}"))
            .iter()
            .map(|g| {
                (
                    g["name"].as_str().unwrap().into(),
                    g["count"].as_u64().unwrap(),
                )
            })
            .collect();
        // The window.
        let Ok((device, queue)) = pollster::block_on(visioncraft_render::headless_device()) else {
            eprintln!("no GPU adapter: the window side is skipped");
            return None;
        };
        let renderer =
            visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut studio = Studio::new(renderer, p.clone());
        studio.assets.extend(crate::fixtures::assets());
        studio.resize(320, 200);
        studio.prepare();
        let window: Vec<(String, u64)> = p
            .generators
            .iter()
            .map(|g| {
                let n = studio.scene.gen_stats(g.id()).map_or(0, |s| s.count);
                (g.name().to_string(), n as u64)
            })
            .collect();
        eprintln!("placements: {window:?}");
        Some((agents, window))
    }

    /// The estate: a 3DCraft model the building rule must see, two large lawns, a woodland
    /// brushed from a preset, a planting on a hidden layer (still counted).
    #[test]
    fn the_estate_counts_the_same_for_the_window_and_agents() {
        let p = visioncraft_core::example("estate").unwrap();
        let Some((agents, window)) = counts(&p) else {
            return;
        };
        assert_eq!(window, agents);
        let count = |n: &str| window.iter().find(|w| w.0 == n).unwrap().1;
        assert!(count("Woodland") > 40, "{window:?}");
        assert!(count("Border planting") > 200, "{window:?}");
        assert_eq!(count("Path lights"), 7);
        assert_eq!(
            count("Old walnut"),
            1,
            "a hidden layer's plants are counted"
        );
    }

    #[test]
    fn the_window_and_agents_count_the_same_placements() {
        // The garden plus trees scattered over the house and terrace: the building rule at work.
        let mut s = Session::new(Default::default());
        s.run(
            "project.load_example",
            serde_json::json!({"name": "garden"}),
        )
        .unwrap();
        s.run(
            "planting.add_area",
            serde_json::json!({"name": "Over the house", "asset_palette": [{"asset": "flora:betula-pendula", "weight": 1}], "outline": [[2, 0], [18, 0], [18, 14], [2, 14]], "density_per_m2": 0.1}),
        )
        .unwrap();
        let p = s.project().clone();
        let Some((agents, window)) = counts(&p) else {
            return;
        };
        assert_eq!(window, agents);
        // The regression numbers: the hedge goes round the plot but not through the house.
        let count = |n: &str| window.iter().find(|w| w.0 == n).unwrap().1;
        assert!(count("Hornbeam hedge") > 100, "{window:?}");
        assert_eq!(count("Path lights"), 5);
        assert_eq!(count("Field maple"), 1);
        assert!(count("Border plants") > 40, "{window:?}");
        // 224 m2 at 0.1 per m2, less the 90 m2 house and the trees too tall for its eaves.
        assert!((1..15).contains(&count("Over the house")), "{window:?}");
    }
}

/// Walking with the lawns scattered around the eye: every step moves the eye a cell and draws
/// one frame, so a lawn's scatter is replaced while half drawn. Its items went with it only
/// once drawn in full: the rest stayed, tufts of a lawn no longer there (over the terrace, the
/// border and the pond of the example garden in the window).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod walk_tests {
    use crate::studio::Studio;

    #[test]
    fn walking_leaves_no_tufts_behind() {
        let p = visioncraft_core::example("garden").unwrap();
        let Ok((device, queue)) = pollster::block_on(visioncraft_render::headless_device()) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let renderer =
            visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut studio = Studio::new(renderer, p);
        studio.assets.extend(crate::fixtures::assets());
        studio.resize(320, 200);
        // A budget far under the lawn's need: its tufts follow the eye.
        studio.gpu_lawn_budget = 0.02;
        let start = studio.camera.pivot;
        studio.settle_for_picture();
        let settled = studio.renderer.item_count();
        for i in 0..60 {
            let t = i as f32 / 59.0;
            studio.camera.pivot = start + glam::Vec3::new(14.0 * t, 0.0, 18.0 * t);
            studio.prepare();
        }
        studio.camera.pivot = start;
        studio.settle_for_picture();
        for _ in 0..20 {
            studio.prepare();
        }
        assert_eq!(
            studio.renderer.item_count(),
            settled,
            "items left behind by the walk"
        );

        // The estate opened first, its west lawn (which covers the whole garden's plot) half
        // drawn, then the garden: none of the estate's tufts stay.
        let renderer =
            visioncraft_render::Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut other = Studio::new(renderer, visioncraft_core::example("estate").unwrap());
        other.assets.extend(crate::fixtures::assets());
        other.resize(320, 200);
        other.gpu_lawn_budget = 0.02;
        other.camera.pivot = glam::Vec3::new(10.0, 0.0, 20.0);
        let t0 = std::time::Instant::now();
        while other.scene.lawns_pending() == 0 && t0.elapsed().as_secs() < 60 {
            other.prepare();
        }
        other.prepare();
        other.replace_project(visioncraft_core::example("garden").unwrap());
        other.camera.pivot = start;
        other.settle_for_picture();
        for _ in 0..20 {
            other.prepare();
        }
        assert_eq!(
            other.renderer.item_count(),
            settled,
            "the estate's tufts stayed"
        );
    }
}
