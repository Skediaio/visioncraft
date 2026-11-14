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
        // Agents.
        let mut h = Headless::new(Access::default());
        h.session = Session::new(p.clone());
        h.state.catalog.assets = crate::fixtures::assets();
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
            return;
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
