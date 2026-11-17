//! Every recipe of the agent guide (docs/agent.md), run through the MCP server as a
//! `command_batch`. Pictures need a GPU: `view.render` steps are left out here and run by the
//! CLI's `recipes_render` test.

use serde_json::{Value, json};
use std::path::PathBuf;
use visioncraft_automation::{Access, Headless, mcp};

/// The ```json recipe blocks of the guide, in order.
pub fn recipes() -> Vec<Value> {
    let guide = include_str!("../docs/agent.md");
    guide
        .split("```json recipe\n")
        .skip(1)
        .map(|block| serde_json::from_str(&block[..block.find("```").unwrap()]).unwrap())
        .collect()
}

#[test]
fn every_recipe_of_the_guide_runs() {
    let recipes = recipes();
    assert_eq!(recipes.len(), 23);
    for (n, recipe) in recipes.into_iter().enumerate() {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("recipe-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../visioncraft-core/tests/data/plate.3dcraft"
            ),
            dir.join("plate.3dcraft"),
        )
        .unwrap();
        let mut host = Headless::new(Access::roots(Some(dir.clone()), Some(dir.clone())));
        let mut args = recipe.clone();
        args["steps"] = recipe["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["id"] != "view.render")
            .cloned()
            .collect();
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "command_batch", "arguments": args}});
        let reply: Value =
            serde_json::from_str(&mcp::handle_line(&mut host, &line.to_string()).unwrap()).unwrap();
        let report: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            reply["result"]["isError"],
            false,
            "recipe {}: {report}",
            n + 1
        );
        let results = report["results"].as_array().unwrap();
        match n + 1 {
            1 => assert_eq!(results[2]["result"]["value"], 600.0),
            2 => {
                let bbox = &results[2]["result"]["models"][0]["bbox"];
                assert!(bbox[1][0].as_f64().unwrap() > 4.0, "{bbox}");
            }
            4..=6 => {
                let i = visioncraft_automation::commands::execute(
                    &mut host,
                    "document.inspect",
                    &json!({"faces": false}),
                )
                .unwrap();
                let count = i["generators"][0]["count"].as_u64().unwrap();
                assert!(count > 10, "recipe {}: {count} plants", n + 1);
            }
            7 => assert!(
                results[3]["result"]["height"].as_f64().unwrap() < -0.2,
                "{report}"
            ),
            11 => {
                let shape = &results[7]["result"]["shapes"][0];
                assert!(shape.to_string().contains("roof"), "{shape}");
            }
            // The ground there is the water surface, below the lawn.
            12 => assert!(
                results[4]["result"]["height"].as_f64().unwrap() < 0.0,
                "{report}"
            ),
            14 => assert_eq!(
                results[1]["result"].as_array().map(Vec::len),
                Some(1),
                "{report}"
            ),
            15 => assert!(
                results[1]["result"]["shapes"].as_array().unwrap().len() > 5,
                "{report}"
            ),
            16 => {
                let years = results[3]["result"]["assets"][0]["years"]
                    .as_array()
                    .unwrap();
                assert_eq!(years.len(), 21, "{report}");
                assert_eq!(
                    results[4]["result"]["conflicts"][0]["kind"], "wall",
                    "{report}"
                );
            }
            19 => {
                // Hidden, not removed: the house model keeps all its bodies.
                assert_eq!(results[1]["result"], Value::Null, "{report}");
                assert!(
                    results[2]["result"]["models"][0]["bodies"]
                        .as_u64()
                        .unwrap()
                        > 20,
                    "{report}"
                );
            }
            20 => {
                let shapes = results[4]["result"]["shapes"].as_array().unwrap();
                assert_eq!(shapes[0]["fence"]["gates"], 1, "{report}");
                assert_eq!(shapes[1]["fence"]["gates"], 1, "{report}");
                assert!(
                    shapes[0]["fence"]["posts"].as_u64().unwrap() > 10,
                    "{report}"
                );
                assert!(
                    shapes[1]["fence"]["pickets"].as_u64().unwrap() > 100,
                    "{report}"
                );
            }
            21 => {
                let r = &results[2]["result"];
                assert_eq!(r["steps"], 3, "{report}");
                let h = r["height_m"].as_f64().unwrap();
                assert!((0.5..0.54).contains(&h), "{report}");
                let shapes = results[3]["result"]["shapes"].as_array().unwrap();
                let row = shapes.iter().find(|s| s["id"] == "S3").unwrap();
                assert_eq!(row["stairs"]["steps"], 3, "{report}");
                assert!(row["volume_m3"].as_f64().unwrap() > 0.5, "{report}");
                // The terrace has a masonry edge 48 m long, about half a metre high.
                let terrace = shapes.iter().find(|s| s["id"] == "S2").unwrap();
                let edge = &terrace["edge"];
                assert_eq!(edge["style"], "masonry", "{report}");
                assert!((edge["length_m"].as_f64().unwrap() - 48.0).abs() < 1e-3);
                assert!((0.5..0.53).contains(&edge["height_m"].as_f64().unwrap()));
                assert!(edge["face_area_m2"].as_f64().unwrap() > 20.0, "{report}");
            }
            22 => {
                assert_eq!(results[1]["result"]["soil"], "S3", "{report}");
                // On the soil (0.35 m, its lift on top), not on the lawn.
                let h = results[3]["result"]["height"].as_f64().unwrap();
                assert!((0.36..0.39).contains(&h), "{report}");
                // A ray down the middle meets the soil, not the hollow ring's walls.
                let hit = &results[4]["result"];
                assert_eq!(hit["owner"], "S3", "{report}");
                let shapes = results[5]["result"]["shapes"].as_array().unwrap();
                let row = shapes.iter().find(|s| s["id"] == "S2").unwrap();
                assert!(
                    row["raised_bed"]["soil_volume_m3"].as_f64().unwrap() > 0.5,
                    "{report}"
                );
                assert!(
                    results[6]["result"]["generators"][0]["count"]
                        .as_u64()
                        .unwrap()
                        > 3,
                    "{report}"
                );
                let rows = results[9]["result"]["shapes"].as_array().unwrap();
                assert!(
                    rows[0]["raised_bed"]["beams"].as_u64().unwrap() >= 18,
                    "{report}"
                );
                assert!(
                    rows[1]["raised_bed"]["plate_area_m2"].as_f64().unwrap() > 3.0,
                    "{report}"
                );
            }
            23 => {
                let rows = results[7]["result"]["shapes"].as_array().unwrap();
                let pi = std::f64::consts::PI;
                let circle = rows.iter().find(|r| r["id"] == "S2").unwrap();
                let area = circle["plan_area_m2"].as_f64().unwrap();
                assert!((area / (36.0 * pi) - 1.0).abs() < 0.001, "{report}");
                let per = circle["perimeter_m"].as_f64().unwrap();
                assert!((per / (12.0 * pi) - 1.0).abs() < 0.001, "{report}");
                // The border's third edge bulges outward, so it is bigger than 8 x 6.
                let border = rows.iter().find(|r| r["id"] == "S3").unwrap();
                assert!(border["plan_area_m2"].as_f64().unwrap() > 48.0, "{report}");
                let shapes = results[8]["result"]["shapes"].as_array().unwrap();
                assert_eq!(
                    shapes[1]["curves"].as_object().unwrap().len(),
                    4,
                    "{report}"
                );
                assert_eq!(shapes[3]["smooth"], true, "{report}");
            }
            18 => {
                // The beach 20 m out: 0.2 m above the water; the grass stops at the waterline.
                let h = results[6]["result"]["height"].as_f64().unwrap();
                assert!((h + 1.8).abs() < 0.05, "{report}");
                let inspect = &results[7]["result"];
                assert_eq!(inspect["sea"]["wave_height_m"], 0.875, "{report}");
                let count = inspect["generators"][0]["count"].as_u64().unwrap();
                assert!((200..450).contains(&count), "{count} plants");
            }
            10 => assert!(
                results[5]["result"]["elevation_deg"].as_f64().unwrap()
                    > results[4]["result"]["elevation_deg"].as_f64().unwrap()
            ),
            _ => {}
        }
    }
}
