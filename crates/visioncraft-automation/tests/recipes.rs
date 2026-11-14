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
    assert_eq!(recipes.len(), 16);
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
            10 => assert!(
                results[5]["result"]["elevation_deg"].as_f64().unwrap()
                    > results[4]["result"]["elevation_deg"].as_f64().unwrap()
            ),
            _ => {}
        }
    }
}
