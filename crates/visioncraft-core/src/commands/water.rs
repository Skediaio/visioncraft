//! The sea level (`water.set_sea`, `water.remove_sea`) and the coast that goes with it
//! (`terrain.shore`). The sea is part of the design, not a view setting: it cuts the terrain,
//! keeps plants off the bed and is one per project, so it has its own family rather than living
//! in `view.set` or being an area.

use super::fail;
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use crate::terrain::ShoreProfile;
use crate::water::Sea;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "water.set_sea",
            "Set Sea Level",
            "Sets the project's sea level: a water surface at `level` that fills everything below it out to the horizon (or inside `outline`: a lake or wide river), cut by the terrain and the endless ground where they rise above it. Creates it with the defaults if there is none; only the fields given change. Plants and objects are kept off the bed.",
            vec![
                param("level", T::Number, "Height of the mean water surface.").unit("m").default(json!(0)),
                param("kind", T::String, "sea (long waves, never freezes), lake (short steep waves, freezes when still in a snowy winter) or river (small waves carried by the flow).")
                    .allowed(&["sea", "lake", "river"])
                    .default(json!("sea")),
                param("sea_state", T::Number, "WMO sea state 0 (calm, glassy) to 6 (very rough): 1 rippled (0-0.1 m waves), 2 smooth (0.1-0.5 m), 3 slight (0.5-1.25 m), 4 moderate (1.25-2.5 m), 5 rough (2.5-4 m). A lake stops at 1 m, a river at 0.3 m.").default(json!(2)),
                param("direction_deg", T::Number, "Direction the waves travel towards, clockwise from north; null: the wind's (view.set wind_direction_deg).").unit("deg"),
                param("color", T::Array, "Linear RGB of deep water, each 0..1.").default(json!([0.01, 0.05, 0.07])),
                param("clarity", T::Number, "0 (murky) to 1 (clear: the bed shows through metres of water).").default(json!(0.6)),
                param("flow_deg", T::Number, "River: direction the water runs towards, clockwise from north.").unit("deg").default(json!(0)),
                param("flow_speed", T::Number, "River: surface speed, 0..10.").unit("m/s").default(json!(0)),
                param("foam", T::Number, "Foam on crests and along the shore, 0..1.").default(json!(0.5)),
                param("outline", T::Outline, "Limits the water to this polygon ([x, z], 3..64 points); [] for water to the horizon.").default(json!([])),
            ],
            "null",
            json!({"level": 0, "sea_state": 3}),
        ),
        set_sea,
    );
    s.register_with_spec(
        spec(
            "water.remove_sea",
            "Remove Sea",
            "Removes the sea level.",
            vec![],
            "null",
            json!({}),
        ),
        remove_sea,
    );
    s.register_with_spec(
        spec(
            "terrain.shore",
            "Shape Shore",
            "Shapes a coast across the whole terrain along a line, the sea to its right (travelling north, the sea lies east): from the line the land drops over `bank_m` to the beach (a bluff or bank), the beach runs down to the waterline over `beach_m`, then the bed deepens to `depth_m` over `shelf_m` (Dean's equilibrium profile). Heights are relative to the water level. It only lowers the ground (the coast is cut out of the land, so stretches of coast can cross): land behind the line higher than `top_m` comes down to it over 6 m; raise the land first for a higher bluff.",
            vec![
                param("points", T::Outline, "The line, [x, z] pairs, at least 2.").required(),
                param("level", T::Number, "Water level the profile is measured from; default the sea level, else 0.").unit("m"),
                param("top_m", T::Number, "Height of the land at the line above the water.").unit("m").default(json!(1.5)),
                param("bank_m", T::Number, "Width of the bank or bluff from the line to the beach; 0 for none.").unit("m").default(json!(0)),
                param("berm_m", T::Number, "Height of the beach's top above the water (where the bank ends); with no bank the line's height.").unit("m"),
                param("beach_m", T::Number, "Width of the beach down to the waterline, above zero.").unit("m").default(json!(15)),
                param("shelf_m", T::Number, "Distance from the waterline to full depth, above zero.").unit("m").default(json!(40)),
                param("depth_m", T::Number, "Depth of the bed beyond the shelf, 0 or more.").unit("m").default(json!(4)),
                param("material", T::String, "Paints the beach and the bed with this material (a terrain layer is added if needed), e.g. sand."),
            ],
            "null",
            json!({"points": [[-50, 10], [50, 10]], "top_m": 3, "bank_m": 6, "berm_m": 1, "beach_m": 20, "material": "sand"}),
        ),
        shore,
    );
}

fn set_sea(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let base = project.sea.clone().unwrap_or_default();
    // `direction_deg` is not written while it follows the wind, `outline` while empty.
    let sea: Sea = super::merge_update(&base, params, &["direction_deg", "outline"])?;
    sea.validate().map_err(|e| bad(&e))?;
    project.sea = Some(sea);
    Ok(Value::Null)
}

fn remove_sea(project: &mut Project, _: &Value) -> Result<Value, CommandError> {
    if project.sea.take().is_none() {
        return Err(fail("there is no sea"));
    }
    Ok(Value::Null)
}

fn shore(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        points: Vec<[f32; 2]>,
        level: Option<f32>,
        #[serde(default = "top")]
        top_m: f32,
        #[serde(default)]
        bank_m: f32,
        berm_m: Option<f32>,
        #[serde(default = "beach")]
        beach_m: f32,
        #[serde(default = "shelf")]
        shelf_m: f32,
        #[serde(default = "depth")]
        depth_m: f32,
        material: Option<String>,
    }
    fn top() -> f32 {
        1.5
    }
    fn beach() -> f32 {
        15.0
    }
    fn shelf() -> f32 {
        40.0
    }
    fn depth() -> f32 {
        4.0
    }
    let p: P = parse(params)?;
    let level = p
        .level
        .unwrap_or_else(|| project.sea.as_ref().map_or(0.0, |s| s.level));
    let profile = ShoreProfile {
        level,
        top_m: p.top_m,
        bank_m: p.bank_m,
        berm_m: p.berm_m.unwrap_or(p.top_m),
        beach_m: p.beach_m,
        shelf_m: p.shelf_m,
        depth_m: p.depth_m,
    };
    let numbers = [
        level,
        p.top_m,
        p.bank_m,
        profile.berm_m,
        p.beach_m,
        p.shelf_m,
        p.depth_m,
    ];
    if p.points.len() < 2
        || !p.points.iter().flatten().all(|c| c.is_finite())
        || !numbers.iter().all(|v| v.is_finite())
        || p.bank_m < 0.0
        || p.beach_m <= 0.0
        || p.shelf_m <= 0.0
        || p.depth_m < 0.0
        || profile.berm_m < 0.0
    {
        return Err(bad(
            "a line of 2 points, finite heights, a bank and depth of 0 or more, a beach and shelf above zero",
        ));
    }
    if let Some(m) = &p.material
        && !project.materials.iter().any(|x| &x.id == m)
    {
        return Err(fail(format!("no material `{m}`")));
    }
    let t = project
        .terrain
        .as_mut()
        .map(Arc::make_mut)
        .ok_or_else(|| fail("there is no terrain: terrain.create first"))?;
    let layer = match &p.material {
        None => None,
        Some(m) => Some(match t.layers.iter().position(|l| &l.material == m) {
            Some(i) => i,
            None => t
                .add_layer(m)
                .ok_or_else(|| fail("the terrain has no room for another material layer"))?,
        }),
    };
    t.shore(&p.points, &profile, layer);
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use crate::{Project, Session};
    use serde_json::json;

    #[test]
    fn the_sea_is_set_in_parts_undone_saved_and_absent_from_old_files() {
        let mut s = Session::new(Project::default());
        s.run("water.set_sea", json!({"level": -0.5, "sea_state": 4}))
            .unwrap();
        s.run("water.set_sea", json!({"kind": "lake"})).unwrap();
        let sea = s.project().sea.clone().unwrap();
        assert_eq!((sea.level, sea.sea_state), (-0.5, 4.0));
        assert_eq!(sea.kind, crate::WaterKind::Lake);
        assert!(s.run("water.set_sea", json!({"sea_state": 9})).is_err());
        assert!(s.run("water.set_sea", json!({"levl": 1})).is_err());
        let saved = Project::from_json_bytes(&s.project().to_json_bytes()).unwrap();
        assert_eq!(saved.sea, s.project().sea);
        s.undo();
        assert_eq!(
            s.project().sea.as_ref().unwrap().kind,
            crate::WaterKind::Sea
        );
        s.run("water.remove_sea", json!({})).unwrap();
        assert!(s.project().sea.is_none());
        assert!(s.run("water.remove_sea", json!({})).is_err());
        // A project without a sea writes nothing for it.
        let json = String::from_utf8(Project::default().to_json_bytes()).unwrap();
        assert!(!json.contains("\"sea\""));
    }

    #[test]
    fn a_shore_runs_from_the_land_down_under_the_sea() {
        let mut s = Session::new(Project::default());
        s.run(
            "terrain.create",
            json!({"origin": [-50, -50], "width_m": 100, "depth_m": 100, "cell_m": 1, "base_material": "lawn"}),
        )
        .unwrap();
        // The land at 0, the sea 3 m below it.
        s.run("water.set_sea", json!({"level": -3})).unwrap();
        // Travelling east along z = 0: the sea lies south (+z).
        s.run(
            "terrain.shore",
            json!({"points": [[-60, 0], [60, 0]], "top_m": 3, "bank_m": 5, "berm_m": 1, "beach_m": 10, "shelf_m": 20, "depth_m": 3, "material": "soil"}),
        )
        .unwrap();
        let t = s.project().terrain.clone().unwrap();
        let h = |t: &crate::Terrain, z: f32| t.height_at(0.0, z).unwrap();
        assert!(h(&t, 0.0).abs() < 0.01, "the line at the land's height");
        assert!(h(&t, -30.0).abs() < 0.01, "the land side keeps its ground");
        assert!((h(&t, 5.0) + 2.0).abs() < 0.01, "the berm");
        assert!((h(&t, 15.0) + 3.0).abs() < 0.01, "the waterline");
        assert!((h(&t, 35.0) + 6.0).abs() < 0.01, "full depth");
        assert!(h(&t, 25.0) < -3.0 && h(&t, 25.0) > -6.0);
        assert!(t.layers.iter().any(|l| l.material == "soil"));
        // A second stretch of coast across the first only lowers: no ridge over the beach.
        s.run(
            "terrain.shore",
            json!({"points": [[20, 60], [20, -60]], "top_m": 3, "bank_m": 10, "beach_m": 5}),
        )
        .unwrap();
        let u = s.project().terrain.clone().unwrap();
        for (x, z) in [(25.0, 12.0), (35.0, 30.0), (0.0, 10.0)] {
            assert!(u.height_at(x, z) <= t.height_at(x, z), "raised at {x}, {z}");
        }
        assert!(
            s.run("terrain.shore", json!({"points": [[0, 0]]})).is_err(),
            "a line needs two points"
        );
    }
}
