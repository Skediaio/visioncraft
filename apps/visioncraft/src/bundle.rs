//! `.visioncraft.zip`: a project with the files it needs, so it opens anywhere (a browser has no
//! folders to find them in). Inside: `project.visioncraft` (the JSON), `objects/*.glb` (imported
//! objects, at the `file` their `UserObject` names) and `models/*` (the sources of imported
//! models, the project's `source` pointing at them). Stored without compression.

use std::io::Read;
use std::sync::Arc;
use visioncraft_core::Project;

pub const PROJECT_FILE: &str = "project.visioncraft";
/// A bundle larger than this is not read (an untrusted file).
pub const MAX_BUNDLE_BYTES: usize = 512 * 1024 * 1024;

/// Packs `project` with the bytes `object_file` and `model_source` give (`None`: not held, left
/// out). Model sources are renamed to `models/<their file name>` in the packed project.
pub fn pack(
    project: &Project,
    object_file: &dyn Fn(&str) -> Option<Arc<[u8]>>,
    model_source: &dyn Fn(&str) -> Option<Arc<[u8]>>,
) -> Result<Vec<u8>, String> {
    let mut p = project.clone();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for m in &mut p.models {
        if let Some(bytes) = model_source(&m.id) {
            let base = m
                .source
                .rsplit(['/', '\\'])
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("model")
                .to_string();
            let mut name = format!("models/{base}");
            while files.iter().any(|(f, _)| *f == name) {
                name = format!("models/{}-{base}", files.len());
            }
            m.source = name.clone();
            files.push((name, bytes.to_vec()));
        }
    }
    for o in &project.objects {
        if let Some(bytes) = object_file(&o.file) {
            files.push((o.file.clone(), bytes.to_vec()));
        }
    }
    files.insert(0, (PROJECT_FILE.into(), p.to_json_bytes()));
    crate::files::zip_stored(&files)
}

/// A bundle read back: the project and every other file in it, by path.
pub struct Unpacked {
    pub project: Project,
    pub files: Vec<(String, Arc<[u8]>)>,
}

/// What a bundle may hold. A bundle is untrusted input: a zip bomb, a flood of entries or a
/// path that climbs out must end in a message, not in exhausted memory.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub entries: usize,
    /// Per entry, declared and actually read.
    pub entry_bytes: u64,
    /// All entries together, declared and actually read.
    pub total_bytes: u64,
}

pub const LIMITS: Limits = Limits {
    entries: 10_000,
    entry_bytes: 256 * 1024 * 1024,
    total_bytes: MAX_BUNDLE_BYTES as u64,
};

/// A name a bundle may use: relative, no `..`, inside `project.visioncraft`, `objects/` or
/// `models/`. `Err` for a name that tries to leave the bundle; `Ok(false)` for one to ignore.
fn allowed(name: &str) -> Result<bool, String> {
    let climbs = name.split(['/', '\\']).any(|part| part == "..");
    let absolute = name.starts_with(['/', '\\'])
        || name.as_bytes().get(1) == Some(&b':')
        || name.contains('\0');
    if climbs || absolute {
        return Err(format!("the bundle names a file outside itself: {name}"));
    }
    Ok(name == PROJECT_FILE || name.starts_with("objects/") || name.starts_with("models/"))
}

/// Reads an untrusted bundle: a message for anything wrong, never a panic.
pub fn unpack(bytes: &[u8]) -> Result<Unpacked, String> {
    unpack_with(bytes, LIMITS)
}

pub fn unpack_with(bytes: &[u8], limits: Limits) -> Result<Unpacked, String> {
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(format!(
            "the bundle is larger than {} MB",
            MAX_BUNDLE_BYTES / (1024 * 1024)
        ));
    }
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("not a VisionCraft bundle: {e}"))?;
    if z.len() > limits.entries {
        return Err(format!(
            "the bundle holds {} files; at most {} are read",
            z.len(),
            limits.entries
        ));
    }
    let mut project = None;
    let mut files = Vec::new();
    let mut total = 0u64;
    for i in 0..z.len() {
        let f = z.by_index(i).map_err(|e| {
            format!("cannot read the bundle (only uncompressed bundles are read): {e}")
        })?;
        if f.is_dir() {
            continue;
        }
        let name = f
            .name()
            .map_err(|e| format!("cannot read the bundle: {e}"))?
            .to_string();
        if !allowed(&name)? {
            continue;
        }
        let too_big = || format!("{name} is larger than a bundle may hold");
        if f.size() > limits.entry_bytes || total.saturating_add(f.size()) > limits.total_bytes {
            return Err(too_big());
        }
        // The declared size may lie: read at most one byte past what is allowed.
        let cap = limits.entry_bytes.min(limits.total_bytes - total);
        let mut data = Vec::new();
        f.take(cap + 1)
            .read_to_end(&mut data)
            .map_err(|e| format!("{name}: {e}"))?;
        if data.len() as u64 > cap {
            return Err(too_big());
        }
        total += data.len() as u64;
        if name == PROJECT_FILE {
            project = Some(Project::from_json_bytes(&data).map_err(|e| e.to_string())?);
        } else {
            files.push((name, Arc::from(data)));
        }
    }
    Ok(Unpacked {
        project: project.ok_or("the bundle has no project.visioncraft")?,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use visioncraft_core::Session;

    #[test]
    fn a_project_with_an_object_and_a_model_survives_the_bundle() {
        let mut s = Session::new(Project::default());
        let r = s
            .run(
                "object.add",
                serde_json::json!({"name": "Shed", "file": "objects/Shed.glb", "size_m": [2, 2.5, 3]}),
            )
            .unwrap();
        s.run(
            "model.add",
            serde_json::json!({"name": "House", "kind": "3dcraft", "source": "/home/x/house.3dcraft"}),
        )
        .unwrap();
        let glb: Arc<[u8]> = Arc::from(vec![1u8, 2, 3]);
        let model: Arc<[u8]> = Arc::from(vec![9u8; 10]);
        let bytes = pack(
            s.project(),
            &|f| (f == "objects/Shed.glb").then(|| glb.clone()),
            &|_| Some(model.clone()),
        )
        .unwrap();
        let u = unpack(&bytes).unwrap();
        assert_eq!(u.project.objects[0].id, r["id"].as_str().unwrap());
        assert_eq!(u.project.models[0].source, "models/house.3dcraft");
        let get = |n: &str| u.files.iter().find(|f| f.0 == n).map(|f| f.1.to_vec());
        assert_eq!(get("objects/Shed.glb"), Some(vec![1, 2, 3]));
        assert_eq!(get("models/house.3dcraft"), Some(vec![9; 10]));
        assert!(unpack(b"PK nonsense").is_err());
        assert!(unpack(&bytes[..bytes.len() / 2]).is_err());
    }

    /// A file from a build with extensions this one does not know keeps their data byte for
    /// byte through open, save as a bundle, open, save.
    #[test]
    fn unknown_extension_namespaces_survive_unchanged() {
        let ext = serde_json::json!({
            "com.other.tool": {"pipes": [{"from": [0.5, 1.25], "to": [3.0, -2.0], "dn": 110}], "rev": 7},
            "zz.vendor": {"note": "kept as is", "nested": {"a": [1, 2.5, null, true]}}
        });
        let mut doc: serde_json::Value =
            serde_json::from_slice(&Project::default().to_json_bytes()).unwrap();
        doc["extensions"] = ext.clone();
        doc["models"] = serde_json::json!([{"id": "m1", "name": "House", "kind": "obj",
            "source": "house.obj", "extensions": {"com.other.tool": {"tag": "facade"}}}]);
        doc["materials"][0]["extensions"] = serde_json::json!({"com.other.tool": {"rough": 0.125}});
        let original = serde_json::to_vec(&doc).unwrap();
        // what the other build wrote, opened here and saved as a bundle
        let mut s = Session::new(Project::default());
        s.replace_project(Project::from_json_bytes(&original).unwrap());
        let first = s.project().to_json_bytes();
        let model: Arc<[u8]> = Arc::from(vec![7u8; 4]);
        let zip = pack(s.project(), &|_| None, &|_| Some(model.clone())).unwrap();
        let back = unpack(&zip).unwrap().project;
        let second = back.to_json_bytes();
        let parsed = |b: &[u8]| serde_json::from_slice::<serde_json::Value>(b).unwrap();
        let (a, b) = (parsed(&first), parsed(&second));
        let bytes = |v: &serde_json::Value| serde_json::to_vec(v).unwrap();
        assert_eq!(bytes(&a["extensions"]), bytes(&ext));
        assert_eq!(bytes(&b["extensions"]), bytes(&ext));
        assert_eq!(
            bytes(&b["models"][0]["extensions"]),
            bytes(&a["models"][0]["extensions"])
        );
        assert_eq!(
            bytes(&b["materials"][0]["extensions"]),
            br#"{"com.other.tool":{"rough":0.125}}"#.to_vec()
        );
        // apart from the model's source moved into the bundle, the two saves are the same file
        let mut b2 = b.clone();
        b2["models"][0]["source"] = a["models"][0]["source"].clone();
        assert_eq!(bytes(&b2), bytes(&a));
    }

    fn zip_of(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let owned: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|(n, b)| (n.to_string(), b.clone()))
            .collect();
        crate::files::zip_stored(&owned).unwrap()
    }

    fn project_json() -> Vec<u8> {
        Project::default().to_json_bytes()
    }

    #[test]
    fn too_many_entries_are_refused() {
        let mut files = vec![(PROJECT_FILE, project_json())];
        let names: Vec<String> = (0..20).map(|i| format!("objects/{i}.glb")).collect();
        files.extend(names.iter().map(|n| (n.as_str(), vec![1u8])));
        let limits = Limits {
            entries: 10,
            ..LIMITS
        };
        assert!(
            unpack_with(&zip_of(&files), limits)
                .err()
                .unwrap()
                .contains("at most 10")
        );
    }

    #[test]
    fn an_entry_or_the_whole_too_large_is_refused() {
        let files = [
            (PROJECT_FILE, project_json()),
            ("objects/a.glb", vec![0u8; 5000]),
            ("objects/b.glb", vec![0u8; 5000]),
        ];
        let zip = zip_of(&files);
        let entry = Limits {
            entry_bytes: 4000,
            ..LIMITS
        };
        assert!(unpack_with(&zip, entry).err().unwrap().contains("larger"));
        let whole = Limits {
            total_bytes: 9000,
            ..LIMITS
        };
        assert!(unpack_with(&zip, whole).err().unwrap().contains("larger"));
        assert!(unpack_with(&zip, LIMITS).is_ok());
    }

    #[test]
    fn a_declared_size_that_lies_is_caught_when_read() {
        let files = [
            (PROJECT_FILE, project_json()),
            ("objects/a.glb", vec![7u8; 3000]),
        ];
        let mut zip = zip_of(&files);
        // Lower the sizes the central directory and the local header declare for the object.
        let declared = 3000u32.to_le_bytes();
        let small = 10u32.to_le_bytes();
        let mut i = 0;
        while i + 4 <= zip.len() {
            if zip[i..i + 4] == declared {
                zip[i..i + 4].copy_from_slice(&small);
            }
            i += 1;
        }
        let limits = Limits {
            entry_bytes: 100,
            ..LIMITS
        };
        // Either the reader notices the lie, or our read cap does: never 3000 bytes in memory.
        match unpack_with(&zip, limits) {
            Err(_) => {}
            Ok(u) => assert!(u.files.iter().all(|f| f.1.len() <= 100)),
        }
    }

    #[test]
    fn climbing_and_absolute_names_are_refused_and_others_ignored() {
        for bad in [
            "objects/../../etc/passwd",
            "/etc/passwd",
            "C:/x.glb",
            "models/..\\x",
        ] {
            let zip = zip_of(&[(PROJECT_FILE, project_json()), (bad, vec![1])]);
            assert!(unpack(&zip).err().unwrap().contains("outside"), "{bad}");
        }
        let zip = zip_of(&[
            (PROJECT_FILE, project_json()),
            ("readme.txt", vec![1]),
            ("objects/a.glb", vec![2]),
        ]);
        let u = unpack(&zip).unwrap();
        assert_eq!(u.files.len(), 1);
        assert_eq!(u.files[0].0, "objects/a.glb");
    }
}
