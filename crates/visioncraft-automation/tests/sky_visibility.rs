//! The sky visibility of the example garden as the window bakes it (`sky_scene`, `SkyVisJob`):
//! where it is darker, that a change of the design changes the bake, and what a bake costs.

use std::time::Instant;
use visioncraft_automation::{SkyVisJob, sky_scene};
use visioncraft_core::{Session, example};

#[test]
fn the_example_garden_is_darker_under_its_trees_and_by_its_house() {
    let garden = example("garden").unwrap();
    let scene = sky_scene(&garden, &|_| None, &[], None, 6);
    assert!(!scene.triangles.is_empty() && scene.crowns.len() > 100);
    let start = Instant::now();
    let v = SkyVisJob::new(scene.clone()).run().expect("a grid");
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let [nx, nl, nz] = v.dims;
    eprintln!(
        "garden bake: {} probes ({nx} x {nl} x {nz}, {} m), {} triangles, {} crowns, {ms:.0} ms",
        v.sh.len(),
        v.cell,
        scene.triangles.len(),
        scene.crowns.len()
    );
    // One thread, as the web window does it, a slice at a time.
    let start = Instant::now();
    let mut job = SkyVisJob::new(scene.clone());
    while !job.step(400) {}
    let single = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "garden bake on one thread: {single:.0} ms, {:.1} ms per 400 probes",
        single / v.sh.len() as f64 * 400.0
    );
    assert_eq!(job.finish(), Some(v.clone()));
    let up = [0.0, 1.0, 0.0];
    let lawn = v.sky_view([12.0, 0.0, 18.0], up);
    let maple = v.sky_view([6.5, 0.0, 26.5], up);
    let corner = v.sky_view([16.6, 0.0, 9.6], up);
    assert!(lawn > 0.6, "lawn {lawn}");
    assert!(maple < lawn - 0.1, "maple {maple} lawn {lawn}");
    assert!(corner < lawn, "corner {corner} lawn {lawn}");
    // Inside the house: no outdoor probe round the point.
    assert!(v.sample([10.0, 1.5, 5.0]).is_none());
    // The same design bakes the same; another tree changes the scene and the bake.
    let again = SkyVisJob::new(scene.clone()).run().unwrap();
    assert_eq!(again, v);
    let mut s = Session::new(garden);
    s.run(
        "planting.add_single",
        serde_json::json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [12, 0, 18]}),
    )
    .unwrap();
    let grown = sky_scene(s.project(), &|_| None, &[], None, 6);
    assert_ne!(grown.key(), scene.key());
    let w = SkyVisJob::new(grown).run().unwrap();
    assert!(w.sky_view([12.0, 0.0, 18.0], up) < lawn - 0.2);
    // Winter: the deciduous crowns let more sky through.
    let winter = sky_scene(s.project(), &|_| None, &[], None, 1);
    assert_ne!(
        winter.key(),
        sky_scene(s.project(), &|_| None, &[], None, 6).key()
    );
}
