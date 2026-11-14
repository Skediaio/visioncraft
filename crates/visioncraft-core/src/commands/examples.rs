//! Example projects built from commands (`examples/*.json`, each a list of [command, params]):
//! `project.load_example` replaces the project with one, as one undo step.

use super::fail;
use crate::project::Project;
use crate::session::{CommandError, Session, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

/// (name, commands) of every example.
pub const EXAMPLES: [(&str, &str); 1] = [("garden", include_str!("../examples/garden.json"))];

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "project.load_example",
            "Open Example",
            "Replaces the project with an example built from commands: `garden`, a 20 by 30 m plot with a house block, terrace, lawn, border, path, carved pond, hornbeam hedge, three trees, path lights, a bollard and six viewpoints.",
            vec![param("name", T::String, "The example: garden.").required()],
            "{name, steps}",
            json!({"name": "garden"}),
        ),
        load_example,
    );
}

/// The project an example builds.
pub fn example(name: &str) -> Result<Project, String> {
    let (_, commands) = EXAMPLES
        .iter()
        .find(|e| e.0 == name)
        .ok_or_else(|| format!("no example `{name}`; there is: garden"))?;
    let steps: Vec<(String, Value)> =
        serde_json::from_str(commands).map_err(|e| format!("example {name}: {e}"))?;
    let mut s = Session::new(Project::default());
    for (command, params) in steps {
        s.run(&command, params)
            .map_err(|e| format!("example {name}, {command}: {e}"))?;
    }
    Ok(s.project().clone())
}

fn load_example(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
    }
    let p: P = parse(params)?;
    *project = example(&p.name).map_err(fail)?;
    Ok(
        json!({"name": p.name, "shapes": project.shapes.len(), "plantings": project.generators.len()}),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_garden_builds_small_and_complete() {
        let (_, json) = super::EXAMPLES[0];
        assert!(json.len() < 50_000);
        let p = super::example("garden").unwrap();
        let names: Vec<&str> = p.shapes.iter().map(|s| s.name()).collect();
        for n in ["House", "Terrace", "Lawn", "Border", "Pond"] {
            assert!(names.contains(&n), "{names:?}");
        }
        assert_eq!(p.generators.len(), 7);
        assert_eq!(p.viewpoints.len(), 6);
        assert!(p.terrain.is_some());
        assert!(super::example("park").is_err());
    }

    #[test]
    fn the_pond_is_square_and_its_coping_stands_on_the_lawn() {
        let p = super::example("garden").unwrap();
        let t = p.terrain.as_deref().unwrap();
        let pond = p.shapes.iter().find(|s| s.name() == "Pond").unwrap();
        let crate::Shape::Area(a) = pond else {
            panic!()
        };
        assert_eq!(a.outline.len(), 4, "the outline as drawn");
        let edge = p.shapes.iter().find(|s| s.name() == "Pond edge").unwrap();
        let crate::Shape::Extrusion(e) = edge else {
            panic!()
        };
        let lift = crate::footing(&e.outline, &|x, z| t.height_at(x, z));
        let top = lift + e.base + e.height;
        assert!((top - 0.12).abs() < 0.02, "coping top {top} (lift {lift})");
    }
}
