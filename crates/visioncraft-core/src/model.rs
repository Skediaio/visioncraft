use std::fmt;
use std::ops::Range;

/// A design as triangles, every triangle tagged with the face it came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedModel {
    /// Metres, world axes (Y up, see the crate docs).
    pub positions: Vec<[f32; 3]>,
    /// Unit length, one per position.
    pub normals: Vec<[f32; 3]>,
    /// Counter-clockwise seen from outside.
    pub triangles: Vec<[u32; 3]>,
    /// One per triangle: index into `faces`.
    pub triangle_faces: Vec<u32>,
    pub faces: Vec<Face>,
    pub bodies: Vec<Body>,
    /// Features that failed to rebuild and bodies that could not be meshed, in plain words.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// The persistent name, stable across edits upstream, e.g. `F2:side(e7)`. For an OBJ: the
    /// group's name.
    pub name: String,
    /// Index into `bodies`.
    pub body: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    /// The id of the feature that made the body, e.g. `F2`. For an OBJ: the group's name.
    pub id: String,
    /// That feature's name in the history, e.g. `Plate`.
    pub name: String,
    /// Linear RGBA, when the source gives the body a colour.
    pub color: Option<[f32; 4]>,
    /// This body's triangles are `model.triangles[triangles]`.
    pub triangles: Range<u32>,
    /// Axis-aligned bounds `[min, max]` of the body's vertices.
    pub bounds: [[f32; 3]; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImportError {
    /// Not a readable `.3dcraft` file.
    Format(String),
    /// Not a readable OBJ file.
    Obj(String),
    /// A `.3dcraftasm` assembly; only parts are read.
    Assembly,
    /// A face without a persistent name (3dcraft names every face it builds; this means it did not).
    UnnamedFace { body: String, face: usize },
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(reason) => write!(f, "not a readable .3dcraft file: {reason}"),
            Self::Obj(reason) => write!(f, "not a readable OBJ file: {reason}"),
            Self::Assembly => f.write_str("an assembly cannot be read, only a part"),
            Self::UnnamedFace { body, face } => write!(f, "face {face} of body {body} has no name"),
        }
    }
}

impl std::error::Error for ImportError {}

impl Body {
    /// A body whose triangles are `triangles` of `model` (positions already pushed).
    pub(crate) fn new(
        model: &ImportedModel,
        (id, name): (String, String),
        color: Option<[f32; 4]>,
        triangles: Range<u32>,
    ) -> Self {
        let mut bounds = [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]];
        for t in &model.triangles[triangles.start as usize..triangles.end as usize] {
            for &i in t {
                let p = model.positions[i as usize];
                for k in 0..3 {
                    bounds[0][k] = bounds[0][k].min(p[k]);
                    bounds[1][k] = bounds[1][k].max(p[k]);
                }
            }
        }
        Self {
            id,
            name,
            color,
            triangles,
            bounds,
        }
    }
}
