//! Generated plant meshes and textures, and compressed library maps, kept on disk (native only;
//! on the web only the mesh layout is used, for the texture worker's answers)
//! in the platform's cache folder (`cache_folder`): a picture rendered again, or the window
//! started again, loads them instead of generating every species anew. A file's name is a hash
//! of everything that made it (the parameters, variant, month or texture scale and the scans'
//! bytes; a map's whole file) and of `FORMAT` and `GENERATOR_FINGERPRINT` (a hash of the source
//! of the generators, the encoding and the compression, listed in `fingerprint-inputs.txt`), so
//! a changed generator never reads an old result while a rebuild that leaves them alone (a UI
//! change) keeps the cache. Every file carries a checksum of its contents: a damaged one is
//! removed and made again. Least recently used files go once the folder is above its limit.
//! `visioncraft-cli cache clear` (or deleting the folder) empties it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use visioncraft_flora as flora;
use visioncraft_render::ColorSpace;
use visioncraft_render::bc::Compressed;

/// The folder stays under this size by default (`VISIONCRAFT_CACHE_MB` sets another).
pub const MAX_BYTES: u64 = 2 << 30;

/// The layout of the files; bump it when it (or what a key covers) changes.
const FORMAT: u32 = 4;
const MAGIC: &[u8; 4] = b"VCC\0";

/// The platform's cache folder for VisionCraft: `VISIONCRAFT_CACHE_DIR` when set, else
/// `$XDG_CACHE_HOME/visioncraft` (`~/.cache/visioncraft`) on Linux, `~/Library/Caches/visioncraft`
/// on macOS, `%LOCALAPPDATA%\visioncraft\cache` on Windows.
pub fn cache_folder(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("VISIONCRAFT_CACHE_DIR") {
        return Some(dir);
    }
    Some(if cfg!(target_os = "windows") {
        set("LOCALAPPDATA")?.join("visioncraft").join("cache")
    } else if cfg!(target_os = "macos") {
        set("HOME")?.join("Library/Caches/visioncraft")
    } else {
        set("XDG_CACHE_HOME")
            .or_else(|| set("HOME").map(|h| h.join(".cache")))?
            .join("visioncraft")
    })
}

/// One cache folder with its size limit.
pub struct Disk {
    dir: PathBuf,
    max_bytes: u64,
    /// Bytes in the folder as far as this process knows (`u64::MAX`: not counted yet).
    total: AtomicU64,
}

/// This process' cache, `None` where there is no folder to use.
pub fn disk() -> Option<&'static Disk> {
    static DISK: std::sync::OnceLock<Option<Disk>> = std::sync::OnceLock::new();
    DISK.get_or_init(|| {
        let env = |k: &str| std::env::var(k).ok();
        // The cache lived in the state folder before: that copy is never read again.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(old) = visioncraft_automation::discovery::state_folder() {
            let _ = std::fs::remove_dir_all(old.join("plants"));
        }
        let max = env("VISIONCRAFT_CACHE_MB")
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(MAX_BYTES, |mb| mb << 20);
        Some(Disk::new(cache_folder(&env)?.join("generated"), max))
    })
    .as_ref()
}

/// Hash of the source of everything that determines the cached bytes (the files in
/// `fingerprint-inputs.txt`, computed by `build.rs`): a rebuild that leaves them alone keeps
/// its cache, a changed generator never reads an old result.
pub const GENERATOR_FINGERPRINT: u64 =
    include!(concat!(env!("OUT_DIR"), "/generator_fingerprint.rs"));

/// The files `GENERATOR_FINGERPRINT` covers (also read by `build.rs`).
#[cfg(test)]
const FINGERPRINT_INPUTS: &str = include_str!("../fingerprint-inputs.txt");

const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// FNV-1a over `parts` (each closed by a separator), the format and the generator fingerprint.
pub fn key(parts: &[&[u8]]) -> u64 {
    let mut h = fnv(FNV0, &FORMAT.to_le_bytes());
    h = fnv(h, &GENERATOR_FINGERPRINT.to_le_bytes());
    for p in parts {
        h = fnv(fnv(h, p), &[0xff]);
    }
    h
}

/// A file's bytes as a short key part (all of them: an edited picture of the same size and
/// name is another key).
pub fn digest(bytes: &[u8]) -> [u8; 8] {
    fnv(FNV0, bytes).to_le_bytes()
}

impl Disk {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        Self {
            dir,
            max_bytes,
            total: AtomicU64::new(u64::MAX),
        }
    }

    fn path(&self, kind: &str, key: u64) -> PathBuf {
        self.dir.join(format!("{kind}-{key:016x}.bin"))
    }

    /// The bytes stored under `key`, marking them used; a damaged file is removed.
    pub fn read(&self, kind: &str, key: u64) -> Option<Vec<u8>> {
        let p = self.path(kind, key);
        let file = std::fs::read(&p).ok()?;
        let Some(body) = unwrap_checked(&file) else {
            let _ = std::fs::remove_file(&p);
            // shortcut: a racy update of an estimate; `trim` recounts from the folder
            let t = self.total.load(Ordering::Relaxed);
            if t != u64::MAX {
                self.total
                    .store(t.saturating_sub(file.len() as u64), Ordering::Relaxed);
            }
            return None;
        };
        // Touch: the eviction goes by modification time.
        if let Ok(f) = std::fs::File::options().append(true).open(&p) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        Some(body.to_vec())
    }

    pub fn write(&self, kind: &str, key: u64, body: &[u8]) {
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let mut file = Vec::with_capacity(body.len() + 16);
        file.extend(MAGIC);
        file.extend(FORMAT.to_le_bytes());
        file.extend(fnv(FNV0, body).to_le_bytes());
        file.extend(body);
        let p = self.path(kind, key);
        // A file of its own per writer, renamed into place: readers never see half a file.
        let tmp = self.dir.join(format!(
            "{kind}-{key:016x}.{}-{:?}.tmp",
            std::process::id(),
            std::thread::current().id()
        ));
        if std::fs::write(&tmp, &file).is_err() || std::fs::rename(&tmp, &p).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if self.total.load(Ordering::Relaxed) == u64::MAX {
            self.total.store(self.scan().1, Ordering::Relaxed);
        } else {
            self.total.fetch_add(file.len() as u64, Ordering::Relaxed);
        }
        if self.total.load(Ordering::Relaxed) > self.max_bytes {
            self.trim();
        }
    }

    /// (files with their modification time and size, total bytes).
    fn scan(&self) -> (Vec<(std::time::SystemTime, u64, PathBuf)>, u64) {
        let files: Vec<_> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                m.is_file().then_some(())?;
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect();
        let total = files.iter().map(|f| f.1).sum();
        (files, total)
    }

    /// Removes the least recently used files until the folder is a tenth below its limit.
    pub fn trim(&self) {
        let (mut files, mut total) = self.scan();
        if total > self.max_bytes {
            files.sort_by_key(|f| f.0);
            for (_, len, p) in files {
                if total <= self.max_bytes / 10 * 9 {
                    break;
                }
                if std::fs::remove_file(&p).is_ok() {
                    total -= len;
                }
            }
        }
        self.total.store(total, Ordering::Relaxed);
    }

    /// Removes every file; returns (files, bytes) freed.
    pub fn clear(&self) -> (usize, u64) {
        let (files, total) = self.scan();
        let n = files.len();
        for (_, _, p) in files {
            let _ = std::fs::remove_file(p);
        }
        self.total.store(0, Ordering::Relaxed);
        (n, total)
    }

    /// (files, bytes) in the folder.
    pub fn usage(&self) -> (usize, u64) {
        let (files, total) = self.scan();
        (files.len(), total)
    }

    pub fn folder(&self) -> &Path {
        &self.dir
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
}

/// A file's contents when its header and checksum hold.
fn unwrap_checked(file: &[u8]) -> Option<&[u8]> {
    let (head, body) = (file.get(..16)?, file.get(16..)?);
    (head[..4] == MAGIC[..]
        && head[4..8] == FORMAT.to_le_bytes()
        && head[8..16] == fnv(FNV0, body).to_le_bytes())
    .then_some(body)
}

fn read(kind: &str, key: u64) -> Option<Vec<u8>> {
    let got = disk()?.read(kind, key);
    let what = if got.is_some() { "hit" } else { "miss" };
    crate::load_stats::add(&format!("disk cache {what} ({kind})"), 0.0);
    got
}

fn write(kind: &str, key: u64, bytes: &[u8]) {
    if let Some(d) = disk() {
        d.write(kind, key, bytes);
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
        self.0.reserve(4 * v.len());
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
        // A damaged length cannot ask for more than is there.
        (n <= self.0.len() / 4).then_some(())?;
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

pub fn encode_levels(height: f32, levels: &Levels) -> Vec<u8> {
    // The size up front: a tree's levels with the detail ones are 15 MB.
    let floats: usize = levels
        .iter()
        .flat_map(|(_, parts, _)| parts)
        .map(|(_, p)| 10 * p.positions.len() + p.indices.len() + 6)
        .sum();
    let mut w = W(Vec::with_capacity(4 * floats + 64 * levels.len() + 8));
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

pub fn decode_levels(b: &[u8]) -> Option<(f32, Levels)> {
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
                    let k = r.u32()? as usize;
                    (k <= r.0.len() / 4).then_some(())?;
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

pub fn encode_textures(t: &Textures) -> Vec<u8> {
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

pub fn decode_textures(b: &[u8]) -> Option<Textures> {
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

    fn scratch(name: &str) -> Disk {
        let dir = std::env::temp_dir().join(format!(
            "visioncraft-cache-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Disk::new(dir, 1 << 20)
    }

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
        // a damaged length does not allocate what is not there
        assert!(decode_levels(&[0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn the_key_changes_with_every_input() {
        let params = serde_json::to_vec(&flora::species()[0].params).unwrap();
        let mut other = flora::species()[0].params.clone();
        other.age *= 0.5;
        let other = serde_json::to_vec(&other).unwrap();
        let base = key(&[&params, &1u32.to_le_bytes(), &[6]]);
        assert_eq!(base, key(&[&params, &1u32.to_le_bytes(), &[6]]));
        for k in [
            key(&[&other, &1u32.to_le_bytes(), &[6]]),
            key(&[&params, &2u32.to_le_bytes(), &[6]]),
            key(&[&params, &1u32.to_le_bytes(), &[7]]),
            key(&[&params, &1u32.to_le_bytes(), &[6], b"x"]),
            // parts do not run into each other
            key(&[&params, &1u32.to_le_bytes(), &[], &[6]]),
        ] {
            assert_ne!(k, base);
        }
        assert_ne!(digest(b"picture a"), digest(b"picture b"));
    }

    #[test]
    fn every_fingerprint_input_exists() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let inputs: Vec<_> = FINGERPRINT_INPUTS
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert!(inputs.len() >= 5, "{inputs:?}");
        for i in inputs {
            assert!(root.join(i).exists(), "fingerprint input {i} is gone");
        }
    }

    #[test]
    fn the_key_mixes_in_the_generator_fingerprint() {
        assert_ne!(GENERATOR_FINGERPRINT, 0);
        let expected = fnv(
            fnv(FNV0, &FORMAT.to_le_bytes()),
            &GENERATOR_FINGERPRINT.to_le_bytes(),
        );
        assert_eq!(key(&[]), expected);
    }

    #[test]
    fn a_damaged_entry_is_removed_and_made_again() {
        let d = scratch("damaged");
        d.write("mesh", 7, b"some bytes");
        assert_eq!(d.read("mesh", 7).as_deref(), Some(&b"some bytes"[..]));
        let p = d.path("mesh", 7);
        let mut file = std::fs::read(&p).unwrap();
        *file.last_mut().unwrap() ^= 1;
        std::fs::write(&p, &file).unwrap();
        assert_eq!(d.read("mesh", 7), None);
        assert!(!p.exists(), "a damaged file is removed");
        std::fs::write(&p, b"VCC").unwrap(); // cut short
        assert_eq!(d.read("mesh", 7), None);
        d.write("mesh", 7, b"made again");
        assert_eq!(d.read("mesh", 7).as_deref(), Some(&b"made again"[..]));
        let _ = std::fs::remove_dir_all(d.folder());
    }

    #[test]
    fn eviction_keeps_the_folder_under_its_limit_and_the_recent_files() {
        let d = scratch("evict");
        let body = vec![1u8; 100 << 10];
        for k in 0..8 {
            d.write("tex", k, &body);
            // distinct modification times, oldest first
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(100 - k);
            let f = std::fs::File::options()
                .append(true)
                .open(d.path("tex", k))
                .unwrap();
            f.set_modified(t).unwrap();
        }
        assert!(d.read("tex", 0).is_some()); // used now: the most recent
        for k in 8..14 {
            d.write("tex", k, &body);
        }
        let (_, bytes) = d.usage();
        assert!(bytes <= d.max_bytes(), "{bytes}");
        assert!(d.read("tex", 0).is_some(), "the recently used file stays");
        assert!(d.read("tex", 1).is_none(), "the oldest went");
        assert!(d.read("tex", 13).is_some());
        let files = d.usage().0;
        assert_eq!(d.clear().0, files);
        assert_eq!(d.usage(), (0, 0));
        let _ = std::fs::remove_dir_all(d.folder());
    }
}
