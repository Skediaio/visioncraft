//! Objects a person brings along: their own `.glb` files, placed like catalogue assets.

use serde::{Deserialize, Serialize};

/// A user's own 3D model. Wherever a catalogue asset id is accepted (a single placement, a
/// palette, a generator) `user:<id>` names it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserObject {
    /// `uo1`, `uo2`...
    pub id: String,
    pub name: String,
    /// A path relative to the project file, or `objects/<id>.glb` inside a bundle.
    pub file: String,
    /// Width, height, depth in metres.
    pub size_m: [f32; 3],
}

impl UserObject {
    /// The asset id that places this object.
    pub fn asset_id(&self) -> String {
        format!("user:{}", self.id)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.file.trim().is_empty() {
            return Err("an object needs a name and a file".into());
        }
        if !self.size_m.iter().all(|c| c.is_finite() && *c > 0.0) {
            return Err("size_m must be three numbers above zero".into());
        }
        Ok(())
    }
}
