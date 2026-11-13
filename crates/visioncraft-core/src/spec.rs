//! What every command says about itself: its parameters with types, units, defaults and allowed
//! values, its result and a runnable example. The command list for people, the CLI and the MCP
//! server is built from these; a command is checked against its spec before its handler runs.

use serde_json::Value;

/// The kind of value a parameter takes, as JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    String,
    Number,
    Integer,
    Boolean,
    /// `[x, z]` in metres on the ground plane.
    Point2,
    /// `[x, y, z]` in metres, Y up.
    Point3,
    /// `[r, g, b]` or `[r, g, b, a]`, linear, 0..1.
    Color,
    /// An array of `[x, z]` pairs in metres.
    Outline,
    /// An array of `[x, y, z]` triples in metres.
    Points3,
    Object,
    Array,
    Any,
}

impl ParamType {
    /// The type as written in documentation.
    pub fn describe(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::Point2 => "[x, z]",
            Self::Point3 => "[x, y, z]",
            Self::Color => "[r, g, b] or [r, g, b, a]",
            Self::Outline => "[[x, z], ...]",
            Self::Points3 => "[[x, y, z], ...]",
            Self::Object => "object",
            Self::Array => "array",
            Self::Any => "any",
        }
    }

    /// Whether `v` has this type (null is the caller's business).
    pub fn accepts(self, v: &Value) -> bool {
        let numbers = |v: &Value, n: &[usize]| {
            v.as_array()
                .is_some_and(|a| n.contains(&a.len()) && a.iter().all(|x| x.as_f64().is_some()))
        };
        let list = |v: &Value, n: usize| {
            v.as_array()
                .is_some_and(|a| a.iter().all(|p| numbers(p, &[n])))
        };
        match self {
            Self::String => v.is_string(),
            Self::Number => v.is_number(),
            Self::Integer => v.is_i64() || v.is_u64(),
            Self::Boolean => v.is_boolean(),
            Self::Point2 => numbers(v, &[2]),
            Self::Point3 => numbers(v, &[3]),
            Self::Color => numbers(v, &[3, 4]),
            Self::Outline => list(v, 2),
            Self::Points3 => list(v, 3),
            Self::Object => v.is_object(),
            Self::Array => v.is_array(),
            Self::Any => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub ty: ParamType,
    /// `m`, `deg`, `lm`, `K`, `m2`, `1/m2`, `h`.
    pub unit: Option<String>,
    pub required: bool,
    /// The value used when the parameter is left out, when that is a plain constant.
    pub default: Option<Value>,
    /// The only values accepted, when it is an enumeration.
    pub allowed: Option<Vec<Value>>,
    pub doc: String,
}

/// An optional parameter; chain [`ParamSpec::required`], [`ParamSpec::unit`],
/// [`ParamSpec::default`] and [`ParamSpec::allowed`].
pub fn param(name: &str, ty: ParamType, doc: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        ty,
        unit: None,
        required: false,
        default: None,
        allowed: None,
        doc: doc.into(),
    }
}

impl ParamSpec {
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(unit.into());
        self
    }

    pub fn default(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }

    pub fn allowed(mut self, values: &[&str]) -> Self {
        self.allowed = Some(values.iter().map(|v| Value::from(*v)).collect());
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommandSpec {
    pub name: String,
    /// A short title, as a menu would show it.
    pub label: String,
    /// One sentence.
    pub summary: String,
    pub params: Vec<ParamSpec>,
    /// Keys that are not listed are passed on to the handler instead of being refused.
    pub open: bool,
    /// The shape of the result, `null` when there is none.
    pub result: String,
    /// A params object that runs on [`crate::example_project`].
    pub example: Value,
    /// Whether a successful run is one undo step (false for queries, which never touch history).
    pub undoable: bool,
}

/// A spec for a command that changes the project.
pub fn spec(
    name: &str,
    label: &str,
    summary: &str,
    params: Vec<ParamSpec>,
    result: &str,
    example: Value,
) -> CommandSpec {
    CommandSpec {
        name: name.into(),
        label: label.into(),
        summary: summary.into(),
        params,
        open: false,
        result: result.into(),
        example,
        undoable: true,
    }
}

impl CommandSpec {
    /// Unlisted keys pass.
    pub fn open(mut self) -> Self {
        self.open = true;
        self
    }

    /// A query: never changes the project or its history.
    pub fn read_only(mut self) -> Self {
        self.undoable = false;
        self
    }

    /// The minimal spec of a command registered with just a description: open, no parameters listed.
    pub(crate) fn minimal(name: &str, description: &str) -> Self {
        spec(
            name,
            name,
            description,
            Vec::new(),
            "any",
            Value::Object(Default::default()),
        )
        .open()
    }

    /// Checks `params` against the spec: an object, no unknown keys (unless open), every required
    /// key present, every given value of its type and among the allowed ones. Returns the
    /// message of the first problem.
    pub fn check(&self, params: &Value) -> Result<(), String> {
        let empty = serde_json::Map::new();
        let given = match params {
            Value::Null => &empty,
            Value::Object(m) => m,
            other => {
                return Err(format!(
                    "`{}` takes an object of parameters, got {}",
                    self.name,
                    kind_of(other)
                ));
            }
        };
        if !self.open {
            for key in given.keys() {
                if self.params.iter().all(|p| &p.name != key) {
                    return Err(
                        match nearest(key, self.params.iter().map(|p| p.name.as_str())) {
                            Some(near) => format!(
                                "unknown parameter `{key}` for `{}`; did you mean `{near}`?",
                                self.name
                            ),
                            None => format!("`{}` takes no parameters, got `{key}`", self.name),
                        },
                    );
                }
            }
        }
        for p in &self.params {
            match given.get(&p.name) {
                None if p.required => {
                    return Err(format!(
                        "`{}` needs the parameter `{}` ({})",
                        self.name,
                        p.name,
                        p.ty.describe()
                    ));
                }
                Some(Value::Null) if p.required => {
                    return Err(format!(
                        "parameter `{}` must be {}, got null",
                        p.name,
                        p.ty.describe()
                    ));
                }
                Some(v) if !v.is_null() => {
                    if !p.ty.accepts(v) {
                        return Err(format!(
                            "parameter `{}` must be {}, got {}",
                            p.name,
                            p.ty.describe(),
                            kind_of(v)
                        ));
                    }
                    if let Some(allowed) = &p.allowed
                        && !allowed.contains(v)
                    {
                        let list: Vec<String> = allowed.iter().map(Value::to_string).collect();
                        return Err(format!(
                            "parameter `{}` must be one of {}, got {v}",
                            p.name,
                            list.join(", ")
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn kind_of(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "a boolean".into(),
        Value::Number(_) => "a number".into(),
        Value::String(s) => format!("the string {s:?}"),
        Value::Array(_) => "an array of the wrong shape".into(),
        Value::Object(_) => "an object".into(),
    }
}

/// The candidate closest to `key` by edit distance; `None` when there are none.
pub fn nearest<'a>(key: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    candidates.min_by_key(|c| edit_distance(key, c))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let up = row[j];
            row[j] = (row[j] + 1)
                .min(row[j - 1] + 1)
                .min(diagonal + usize::from(a[i - 1] != b[j - 1]));
            diagonal = up;
        }
    }
    row[b.len()]
}

/// A small project the examples of [`CommandSpec::example`] run on: model `m1`; layer `l1` (empty);
/// shapes `S1` (area, material `lawn`), `S2` (closed extrusion "House") and `S3` (open extrusion);
/// an unassigned material `mat1`; generators `g1` (brush) and `g2` (single); custom species `sp1`; user object `uo1`;
/// light `li1`; viewpoints `v1` and `v2`; a 40 by 30 m terrain with layers `lawn` and `gravel`.
/// For `terrain.create` the terrain is left out.
pub fn example_project(command: &str) -> crate::project::Project {
    use serde_json::json;
    let mut s = crate::Session::new(Default::default());
    let square = |lo: f32, hi: f32| json!([[lo, lo], [hi, lo], [hi, hi], [lo, hi]]);
    let steps = [
        (
            "model.add",
            json!({"name": "House model", "kind": "obj", "source": "house.obj"}),
        ),
        ("layer.add", json!({"name": "Spare"})),
        (
            "shape.add_area",
            json!({"name": "Lawn", "outline": square(0.0, 10.0), "material": "lawn"}),
        ),
        (
            "shape.add_extrusion",
            json!({"name": "House", "outline": square(2.0, 8.0), "height": 3.0}),
        ),
        (
            "shape.add_extrusion",
            json!({"name": "Fence", "outline": [[0, 0], [10, 0]], "closed": false, "height": 1.8}),
        ),
        ("material.add", json!({"name": "Spare material"})),
        (
            "planting.add_brush",
            json!({"name": "Woodland", "asset_palette": [{"asset": "flora:quercus-robur", "weight": 1}], "density_per_m2": 0.02, "strokes": [{"points": [[0, 0], [10, 0]], "radius_m": 4.0, "erase": false}]}),
        ),
        (
            "planting.add_single",
            json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [4, 0, 3]}),
        ),
        (
            "species.add",
            json!({"name": "Tall birch", "base": "betula-pendula", "params": {"height": 14.0}}),
        ),
        (
            "object.add",
            json!({"name": "Bench", "file": "objects/bench.glb", "size_m": [1.8, 0.9, 0.6]}),
        ),
        ("light.add", json!({"name": "Lamp", "position": [3, 2, 1]})),
        (
            "viewpoint.add",
            json!({"name": "Front", "position": [0, 1.6, 8], "target": [0, 1.6, 0]}),
        ),
        (
            "viewpoint.add",
            json!({"name": "Back", "position": [0, 1.6, -8], "target": [0, 1.6, 0]}),
        ),
        (
            "terrain.create",
            json!({"origin": [0, 0], "width_m": 40, "depth_m": 30, "base_material": "lawn"}),
        ),
        ("terrain.add_layer", json!({"material": "gravel"})),
    ];
    for (name, params) in steps {
        if name == "terrain.create" && command == "terrain.create" {
            break;
        }
        if name == "terrain.add_layer" && command == "terrain.create" {
            continue;
        }
        s.run(name, params).expect("the example project builds");
    }
    s.project().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nearest_key_and_types() {
        assert_eq!(
            nearest("lenght", ["length", "area", "kind"].into_iter()),
            Some("length")
        );
        let s = spec(
            "demo.run",
            "Demo",
            "Demo.",
            vec![
                param("outline", ParamType::Outline, "x").required(),
                param("mode", ParamType::String, "m").allowed(&["a", "b"]),
            ],
            "null",
            json!({}),
        );
        assert!(s.check(&json!({"outline": [[0.0, 1.0]]})).is_ok());
        assert!(
            s.check(&json!({"outlin": []}))
                .unwrap_err()
                .contains("did you mean `outline`")
        );
        assert!(
            s.check(&json!({}))
                .unwrap_err()
                .contains("needs the parameter `outline`")
        );
        assert!(
            s.check(&json!({"outline": [[0.0]]}))
                .unwrap_err()
                .contains("`outline` must be [[x, z], ...]")
        );
        assert!(
            s.check(&json!({"outline": [], "mode": "c"}))
                .unwrap_err()
                .contains("one of")
        );
        assert!(s.check(&json!({"outline": [], "mode": null})).is_ok());
        assert!(s.check(&json!([1])).is_err());
    }
}
