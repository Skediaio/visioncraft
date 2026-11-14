//! Generated plant meshes and textures, and compressed library maps, kept on disk (native only), in `plants/` under the state
//! folder: a picture rendered again, or the window started again, loads them instead of
//! generating every species anew. A file's name is a hash of what made it (the parameters,
//! variant, month or texture scale and scans) and of the executable, so a rebuilt generator
//! never reads an old result. Least recently used files go above `MAX_BYTES`.

use std::path::PathBuf;
use std::sync::Arc;
use visioncraft_flora as flora;
use visioncraft_render::ColorSpace;
use visioncraft_render::bc::Compressed;

/// The folder stays under this size.
const MAX_BYTES: u64 = 2 << 30;

fn folder() -> Option<PathBuf> {
    let dir = visioncraft_automation::discovery::state_folder()?.join("plants");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The executable's identity: its size and modification time.
fn build_id() -> u64 {
    static ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let m = std::env::current_exe().and_then(std::fs::metadata);
        let (len, t) = m.map_or((0, 0), |m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as u64);
            (m.len(), t)
        });
        len ^ t.rotate_left(17)
    })
}

/// FNV-1a over `parts` and the build.
pub fn key(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ build_id();
    for p in parts {
        for b in p.iter().chain(&[0xff]) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn path(kind: &str, key: u64) -> Option<PathBuf> {
    Some(folder()?.join(format!("{kind}-{key:016x}.bin")))
}

/// The bytes stored under `key`, marking them used.
fn read(kind: &str, key: u64) -> Option<Vec<u8>> {
    let p = path(kind, key)?;
    let bytes = std::fs::read(&p).ok()?;
    // Touch: the eviction goes by modification time.
    if let Ok(f) = std::fs::File::options().append(true).open(&p) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
    Some(bytes)
}

fn write(kind: &str, key: u64, bytes: &[u8]) {
    let Some(p) = path(kind, key) else {
        return;
    };
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
    trim();
}

/// Removes the least recently used files above `MAX_BYTES`.
fn trim() {
    let Some(dir) = folder() else {
        return;
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= MAX_BYTES {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, p) in files {
        if total <= MAX_BYTES * 9 / 10 {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend(v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend(b);
    }
    fn floats(&mut self, v: &[f32]) {
        self.u32(v.len() as u32);
        v.iter().for_each(|x| self.f32(*x));
    }
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        Some(self.take(n)?.to_vec())
    }
    fn floats(&mut self) -> Option<Vec<f32>> {
        let n = self.u32()? as usize;
        (0..n).map(|_| self.f32()).collect()
    }
}

const MATERIALS: [flora::Material; 4] = [
    flora::Material::Bark,
    flora::Material::Foliage,
    flora::Material::Flower,
    flora::Material::Twigs,
];

/// A plant variant's levels of detail: (level, parts by material, foliage tint).
pub type Levels = Vec<(usize, Vec<(flora::Material, flora::MeshPart)>, [f32; 3])>;

fn encode_levels(height: f32, levels: &Levels) -> Vec<u8> {
    let mut w = W::default();
    w.f32(height);
    w.u32(levels.len() as u32);
    for (li, parts, tint) in levels {
        w.u32(*li as u32);
        tint.iter().for_each(|t| w.f32(*t));
        w.u32(parts.len() as u32);
        for (m, p) in parts {
            w.u32(MATERIALS.iter().position(|x| x == m).unwrap_or(0) as u32);
            w.floats(p.positions.as_flattened());
            w.floats(p.normals.as_flattened());
            w.floats(p.uvs.as_flattened());
            w.floats(p.wind.as_flattened());
            w.u32(p.indices.len() as u32);
            p.indices.iter().for_each(|i| w.u32(*i));
        }
    }
    w.0
}

fn decode_levels(b: &[u8]) -> Option<(f32, Levels)> {
    let mut r = R(b);
    let height = r.f32()?;
    let n = r.u32()?;
    let mut levels = Vec::new();
    for _ in 0..n {
        let li = r.u32()? as usize;
        let tint = [r.f32()?, r.f32()?, r.f32()?];
        let np = r.u32()?;
        let mut parts = Vec::new();
        for _ in 0..np {
            let m = *MATERIALS.get(r.u32()? as usize)?;
            let v3 = |v: Vec<f32>| v.as_chunks::<3>().0.to_vec();
            let v2 = |v: Vec<f32>| v.as_chunks::<2>().0.to_vec();
            let part = flora::MeshPart {
                positions: v3(r.floats()?),
                normals: v3(r.floats()?),
                uvs: v2(r.floats()?),
                wind: v2(r.floats()?),
                indices: {
                    let k = r.u32()?;
                    (0..k).map(|_| r.u32()).collect::<Option<_>>()?
                },
            };
            parts.push((m, part));
        }
        levels.push((li, parts, tint));
    }
    r.0.is_empty().then_some((height, levels))
}

/// A variant's meshes from the cache, else made by `make` and stored.
pub fn meshes(key: u64, make: impl FnOnce() -> (f32, Levels)) -> (f32, Levels) {
    if let Some(got) = read("mesh", key).and_then(|b| decode_levels(&b)) {
        return got;
    }
    let made = make();
    write("mesh", key, &encode_levels(made.0, &made.1));
    made
}

/// A species' compressed textures (leaf colour, images with whether they repeat), as
/// `texture_worker::PlantCompressed`.
pub type Textures = ([f32; 3], Vec<(Arc<Compressed>, bool)>);

fn encode_textures(t: &Textures) -> Vec<u8> {
    let mut w = W::default();
    t.0.iter().for_each(|v| w.f32(*v));
    w.u32(t.1.len() as u32);
    for (c, repeat) in &t.1 {
        w.u32(c.width);
        w.u32(c.height);
        w.bytes(crate::texture_worker::kind_name(c.kind).as_bytes());
        w.u32(u32::from(c.color_space == ColorSpace::Srgb));
        w.u32(u32::from(*repeat));
        w.u32(c.levels.len() as u32);
        c.levels.iter().for_each(|l| w.bytes(l));
    }
    w.0
}

fn decode_textures(b: &[u8]) -> Option<Textures> {
    let mut r = R(b);
    let mean = [r.f32()?, r.f32()?, r.f32()?];
    let n = r.u32()?;
    let mut images = Vec::new();
    for _ in 0..n {
        let (width, height) = (r.u32()?, r.u32()?);
        let kind = crate::texture_worker::kind_of(std::str::from_utf8(&r.bytes()?).ok()?)?;
        let color_space = if r.u32()? == 1 {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        };
        let repeat = r.u32()? == 1;
        let k = r.u32()?;
        let levels = (0..k).map(|_| r.bytes()).collect::<Option<_>>()?;
        images.push((
            Arc::new(Compressed {
                width,
                height,
                kind,
                color_space,
                levels,
            }),
            repeat,
        ));
    }
    r.0.is_empty().then_some((mean, images))
}

/// A species' textures from the cache, else made by `make` and stored.
pub fn textures(key: u64, make: impl FnOnce() -> Option<Textures>) -> Option<Textures> {
    if let Some(got) = read("tex", key).and_then(|b| decode_textures(&b)) {
        return Some(got);
    }
    let made = make()?;
    write("tex", key, &encode_textures(&made));
    Some(made)
}

/// A library map compressed before.
pub fn compressed(key: u64) -> Option<Arc<Compressed>> {
    let (_, mut images) = read("map", key).and_then(|b| decode_textures(&b))?;
    (images.len() == 1).then(|| images.remove(0).0)
}

pub fn store_compressed(key: u64, c: &Arc<Compressed>) {
    write(
        "map",
        key,
        &encode_textures(&([0.0; 3], vec![(c.clone(), false)])),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_and_textures_go_through_bytes() {
        let part = flora::MeshPart {
            positions: vec![[0.0, 1.0, 2.0], [3.0, 4.0, 5.0], [6.0, 7.0, 8.0]],
            normals: vec![[0.0, 1.0, 0.0]; 3],
            uvs: vec![[0.5, 0.5]; 3],
            wind: vec![[0.1, 0.2]; 3],
            indices: vec![0, 1, 2],
        };
        let levels: Levels = vec![(0, vec![(flora::Material::Foliage, part)], [1.0, 0.9, 0.8])];
        let (h, back) = decode_levels(&encode_levels(2.5, &levels)).unwrap();
        assert_eq!(h, 2.5);
        assert_eq!(back[0].1[0].1, levels[0].1[0].1);
        assert_eq!(back[0].1[0].0, flora::Material::Foliage);
        let tex: Textures = (
            [0.1, 0.2, 0.3],
            vec![(
                Arc::new(Compressed {
                    width: 4,
                    height: 4,
                    kind: visioncraft_render::bc::BcKind::Rgba,
                    color_space: ColorSpace::Srgb,
                    levels: vec![vec![7; 16]],
                }),
                true,
            )],
        );
        let back = decode_textures(&encode_textures(&tex)).unwrap();
        assert_eq!(back.0, tex.0);
        assert_eq!(back.1[0].0.levels, tex.1[0].0.levels);
        assert!(back.1[0].1);
        assert!(decode_textures(&encode_textures(&tex)[..10]).is_none());
        assert_ne!(key(&[b"a"]), key(&[b"b"]));
    }
}
