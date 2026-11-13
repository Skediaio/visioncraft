//! Typed access to the JSON object of a request (method params, tool arguments, one batch
//! step), with errors that name the key. Unknown keys are refused before anything runs.
//!
//! Ported from 3dcraft `crates/automation/src/fields.rs` (MIT).

use serde_json::{Map, Value};

pub(crate) fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The keys of one JSON object, checked against what is allowed there.
pub(crate) struct Fields<'a> {
    owner: String,
    map: Option<&'a Map<String, Value>>,
}

impl<'a> Fields<'a> {
    /// `value` must be an object, or absent or null for an empty one, and all its keys must be
    /// in `allowed`. `noun` is what a key is called here (`parameter`, `argument`).
    pub fn new(
        owner: &str,
        noun: &str,
        value: Option<&'a Value>,
        allowed: &[&str],
    ) -> Result<Self, String> {
        let map = match value {
            None | Some(Value::Null) => None,
            Some(Value::Object(map)) => Some(map),
            Some(other) => {
                return Err(format!(
                    "{owner}: {noun}s must be a JSON object, got {}",
                    describe(other)
                ));
            }
        };
        if let Some(unknown) = map
            .into_iter()
            .flat_map(Map::keys)
            .find(|key| !allowed.contains(&key.as_str()))
        {
            let expected = if allowed.is_empty() {
                format!("it takes no {noun}s")
            } else {
                format!("its {noun}s are: {}", allowed.join(", "))
            };
            return Err(format!(
                "{owner}: unknown {noun} `{}`; {expected}",
                shorten(unknown)
            ));
        }
        Ok(Self {
            owner: owner.to_string(),
            map,
        })
    }

    fn wrong(&self, key: &str, expected: &str, got: &Value) -> String {
        format!(
            "{}: `{key}` must be {expected}, got {}",
            self.owner,
            describe(got)
        )
    }

    pub fn missing(&self, key: &str) -> String {
        format!("{}: missing `{key}`", self.owner)
    }

    /// The raw value, with JSON null counting as absent.
    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.map?.get(key).filter(|value| !value.is_null())
    }

    pub fn str(&self, key: &str) -> Result<Option<&'a str>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text)),
            Some(other) => Err(self.wrong(key, "a string", other)),
        }
    }

    pub fn required_str(&self, key: &str) -> Result<&'a str, String> {
        self.str(key)?.ok_or_else(|| self.missing(key))
    }

    pub fn bool(&self, key: &str) -> Result<Option<bool>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(flag)) => Ok(Some(*flag)),
            Some(other) => Err(self.wrong(key, "true or false", other)),
        }
    }

    /// A whole number within `min..=max`.
    pub fn integer(&self, key: &str, min: u64, max: u64) -> Result<Option<u64>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(value) => match value.as_u64().filter(|n| (min..=max).contains(n)) {
                Some(n) => Ok(Some(n)),
                None => Err(format!(
                    "{}: `{key}` must be a whole number from {min} to {max}, got {}",
                    self.owner,
                    shorten(&value.to_string())
                )),
            },
        }
    }

    pub fn number(&self, key: &str) -> Result<Option<f64>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) => Ok(n.as_f64()),
            Some(other) => Err(self.wrong(key, "a number", other)),
        }
    }

    /// A JSON object to pass on as it is. Absent or null gives an empty object.
    pub fn object(&self, key: &str) -> Result<Value, String> {
        match self.get(key) {
            None => Ok(Value::Object(Map::new())),
            Some(value @ Value::Object(_)) => Ok(value.clone()),
            Some(other) => Err(self.wrong(key, "a JSON object", other)),
        }
    }

    pub fn array(&self, key: &str) -> Result<Option<&'a [Value]>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Array(items)) => Ok(Some(items)),
            Some(other) => Err(self.wrong(key, "an array", other)),
        }
    }
}

/// Text from a request, cut to a length that is safe to repeat in an error message.
pub(crate) fn shorten(text: &str) -> String {
    const MAX_CHARS: usize = 80;
    if text.chars().count() <= MAX_CHARS {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(MAX_CHARS).collect();
        cut.push('…');
        cut
    }
}
