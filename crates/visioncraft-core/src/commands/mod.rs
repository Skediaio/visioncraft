//! The commands of the document families added after the first three (models, site, time):
//! layers, shapes, materials, planting, lights, viewpoints, style.

use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

mod layers;
mod lights;
mod materials;
mod planting;
mod queries;
mod shapes;
mod species;
mod terrain;
mod viewpoints;

pub(crate) fn register(s: &mut Session) {
    layers::register(s);
    shapes::register(s);
    species::register(s);
    queries::register(s);
    terrain::register(s);
    materials::register(s);
    planting::register(s);
    lights::register(s);
    viewpoints::register(s);
}

pub(super) fn fail(why: impl Into<String>) -> CommandError {
    CommandError::Failed(why.into())
}

/// `prefix` plus the lowest number that is not taken yet.
pub(super) fn next_id(prefix: &str, taken: impl Fn(&str) -> bool) -> String {
    (1..)
        .map(|n| format!("{prefix}{n}"))
        .find(|id| !taken(id))
        .expect("unbounded range")
}

pub(super) fn check_layer(project: &Project, layer: &str) -> Result<(), CommandError> {
    if project.layers.iter().any(|l| l.id == layer) {
        Ok(())
    } else {
        Err(fail(format!("no layer `{layer}`")))
    }
}

/// A partial update: the top-level fields in `params` (without `id`) replace those of `item`;
/// `absent_ok` names fields that may be set although they are not serialised while empty.
/// Unknown fields and `type` are refused, so a typo does not pass silently.
pub(super) fn merge_update<T: Serialize + DeserializeOwned>(
    item: &T,
    params: &Value,
    absent_ok: &[&str],
) -> Result<T, CommandError> {
    let mut value = serde_json::to_value(item).expect("project items serialise");
    let (Some(fields), Some(changes)) = (value.as_object_mut(), params.as_object()) else {
        return Err(bad("expected an object"));
    };
    for (k, v) in changes.iter().filter(|(k, _)| *k != "id") {
        if k == "type" || (!fields.contains_key(k) && !absent_ok.contains(&k.as_str())) {
            let near = crate::spec::nearest(
                k,
                fields.keys().map(String::as_str).filter(|f| *f != "type"),
            );
            return Err(bad(&format!(
                "cannot update `{k}`{}",
                near.map_or(String::new(), |n| format!("; did you mean `{n}`?"))
            )));
        }
        fields.insert(k.clone(), v.clone());
    }
    parse(&value)
}
