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

// ---- byte codec and serde ----

/// A byte string that is not an [`ImportedModel`] written by [`ImportedModel::to_bytes`].
#[derive(Clone, Debug, PartialEq)]
pub struct ModelCodecError(pub String);

impl fmt::Display for ModelCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a model: {}", self.0)
    }
}

impl std::error::Error for ModelCodecError {}

const MAGIC: &[u8; 4] = b"VCM1";

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], ModelCodecError> {
        if n > self.0.len() {
            return Err(ModelCodecError("the data is cut short".into()));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, ModelCodecError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn f32(&mut self) -> Result<f32, ModelCodecError> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// `n` elements of `size` bytes each must still fit before anything is allocated for them.
    fn count(&mut self, size: usize) -> Result<usize, ModelCodecError> {
        let n = self.u32()? as usize;
        if n.checked_mul(size).is_none_or(|bytes| bytes > self.0.len()) {
            return Err(ModelCodecError("a count is larger than the data".into()));
        }
        Ok(n)
    }

    fn floats<const N: usize>(&mut self, n: usize) -> Result<Vec<[f32; N]>, ModelCodecError> {
        (0..n)
            .map(|_| {
                let mut v = [0.0; N];
                for c in &mut v {
                    *c = self.f32()?;
                }
                Ok(v)
            })
            .collect()
    }

    fn string(&mut self) -> Result<String, ModelCodecError> {
        let n = self.count(1)?;
        String::from_utf8(self.take(n)?.to_vec())
            .map_err(|_| ModelCodecError("a name is not UTF-8".into()))
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend(s.as_bytes());
}

impl ImportedModel {
    /// The model as bytes: `VCM1`, the counts, then positions, normals, triangles and per-triangle
    /// faces as little-endian arrays, then faces, bodies and warnings. About 24 bytes per vertex
    /// and 16 per triangle; no compression, so a worker can hand it over cheaply.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            64 + self.positions.len() * 24 + self.triangles.len() * 16 + self.faces.len() * 32,
        );
        out.extend(MAGIC);
        for n in [self.positions.len(), self.triangles.len()] {
            put_u32(&mut out, n as u32);
        }
        for v in self.positions.iter().chain(&self.normals).flatten() {
            out.extend(v.to_le_bytes());
        }
        for i in self.triangles.iter().flatten().chain(&self.triangle_faces) {
            put_u32(&mut out, *i);
        }
        put_u32(&mut out, self.faces.len() as u32);
        for f in &self.faces {
            put_str(&mut out, &f.name);
            put_u32(&mut out, f.body);
        }
        put_u32(&mut out, self.bodies.len() as u32);
        for b in &self.bodies {
            put_str(&mut out, &b.id);
            put_str(&mut out, &b.name);
            out.push(u8::from(b.color.is_some()));
            for c in b.color.unwrap_or([0.0; 4]) {
                out.extend(c.to_le_bytes());
            }
            put_u32(&mut out, b.triangles.start);
            put_u32(&mut out, b.triangles.end);
            for c in b.bounds.iter().flatten() {
                out.extend(c.to_le_bytes());
            }
        }
        put_u32(&mut out, self.warnings.len() as u32);
        for w in &self.warnings {
            put_str(&mut out, w);
        }
        out
    }

    /// Reads what [`ImportedModel::to_bytes`] wrote, checking every index so that a damaged
    /// model is refused instead of panicking later.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ModelCodecError> {
        let bad = |why: &str| ModelCodecError(why.into());
        let mut r = Reader(bytes);
        if r.take(4)? != MAGIC {
            return Err(bad("wrong header"));
        }
        let vertices = r.count(24)?;
        let triangles = r.count(16)?;
        let positions = r.floats::<3>(vertices)?;
        let normals = r.floats::<3>(vertices)?;
        let tri_indices = (0..triangles)
            .map(|_| Ok([r.u32()?, r.u32()?, r.u32()?]))
            .collect::<Result<Vec<_>, ModelCodecError>>()?;
        let triangle_faces = (0..triangles)
            .map(|_| r.u32())
            .collect::<Result<Vec<_>, _>>()?;
        let face_count = r.count(8)?;
        let faces = (0..face_count)
            .map(|_| {
                Ok(Face {
                    name: r.string()?,
                    body: r.u32()?,
                })
            })
            .collect::<Result<Vec<_>, ModelCodecError>>()?;
        let body_count = r.count(1)?;
        let mut bodies = Vec::with_capacity(body_count);
        for _ in 0..body_count {
            let (id, name) = (r.string()?, r.string()?);
            let has_color = r.take(1)?[0] == 1;
            let color = r.floats::<4>(1)?[0];
            let (start, end) = (r.u32()?, r.u32()?);
            let b = r.floats::<3>(2)?;
            bodies.push(Body {
                id,
                name,
                color: has_color.then_some(color),
                triangles: start..end,
                bounds: [b[0], b[1]],
            });
        }
        let warning_count = r.count(4)?;
        let warnings = (0..warning_count)
            .map(|_| r.string())
            .collect::<Result<Vec<_>, _>>()?;
        if !r.0.is_empty() {
            return Err(bad("trailing bytes"));
        }
        let model = Self {
            positions,
            normals,
            triangles: tri_indices,
            triangle_faces,
            faces,
            bodies,
            warnings,
        };
        if model
            .triangles
            .iter()
            .flatten()
            .any(|&i| i as usize >= vertices)
            || model
                .triangle_faces
                .iter()
                .any(|&f| f as usize >= model.faces.len())
            || model
                .faces
                .iter()
                .any(|f| f.body as usize >= model.bodies.len())
            || model.bodies.iter().any(|b| {
                b.triangles.start > b.triangles.end || b.triangles.end as usize > triangles
            })
        {
            return Err(bad("an index points outside its list"));
        }
        Ok(model)
    }
}

impl serde::Serialize for ImportedModel {
    /// One byte string ([`ImportedModel::to_bytes`]): compact in binary formats; JSON would turn it
    /// into an array of numbers, so keep this model out of JSON for anything large.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.to_bytes())
    }
}

impl<'de> serde::Deserialize<'de> for ImportedModel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = ImportedModel;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("the bytes of an imported model")
            }

            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                ImportedModel::from_bytes(v).map_err(E::custom)
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(1 << 24));
                while let Some(b) = seq.next_element::<u8>()? {
                    bytes.push(b);
                }
                self.visit_bytes(&bytes)
            }
        }
        d.deserialize_bytes(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of `n` by `n` quads (2 n^2 triangles) on one face and one body.
    fn grid(n: u32) -> ImportedModel {
        let mut m = ImportedModel::default();
        for z in 0..=n {
            for x in 0..=n {
                m.positions
                    .push([x as f32 * 0.1, ((x * z) % 7) as f32 * 0.01, z as f32 * 0.1]);
                m.normals.push([0.0, 1.0, 0.0]);
            }
        }
        for z in 0..n {
            for x in 0..n {
                let a = z * (n + 1) + x;
                m.triangles.push([a, a + n + 1, a + n + 2]);
                m.triangles.push([a, a + n + 2, a + 1]);
            }
        }
        m.triangle_faces = vec![0; m.triangles.len()];
        m.faces.push(Face {
            name: "F1:top".into(),
            body: 0,
        });
        let end = m.triangles.len() as u32;
        let body = Body::new(
            &m,
            ("F1".into(), "Plate".into()),
            Some([0.2, 0.4, 0.6, 1.0]),
            0..end,
        );
        m.bodies.push(body);
        m.warnings.push("meshed coarsely".into());
        m
    }

    #[test]
    fn bytes_round_trip_and_size() {
        let m = grid(224); // 100,352 triangles
        assert!(m.triangles.len() > 100_000);
        let bytes = m.to_bytes();
        eprintln!(
            "100k-triangle model: {} bytes ({} vertices)",
            bytes.len(),
            m.positions.len()
        );
        assert_eq!(ImportedModel::from_bytes(&bytes).unwrap(), m);
        // 24 bytes per vertex, 16 per triangle, plus a little for the names.
        assert!(bytes.len() < m.positions.len() * 24 + m.triangles.len() * 16 + 200);
        let none = ImportedModel::default();
        assert_eq!(ImportedModel::from_bytes(&none.to_bytes()).unwrap(), none);
    }

    #[test]
    fn serde_goes_through_the_same_bytes() {
        let m = grid(4);
        let json = serde_json::to_string(&m).unwrap();
        let back: ImportedModel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn damaged_bytes_are_refused() {
        let bytes = grid(3).to_bytes();
        assert!(ImportedModel::from_bytes(&bytes[..bytes.len() - 3]).is_err());
        assert!(ImportedModel::from_bytes(b"nope").is_err());
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(ImportedModel::from_bytes(&extra).is_err());
        let mut broken = bytes.clone();
        broken[12 + 16 * 24 + 8] = 0xff; // a triangle index far outside the vertices
        assert!(ImportedModel::from_bytes(&broken).is_err());
        let mut huge = bytes;
        huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(ImportedModel::from_bytes(&huge).is_err());
    }
}
