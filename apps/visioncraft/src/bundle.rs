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

/// Reads an untrusted bundle: a message for anything wrong, never a panic.
pub fn unpack(bytes: &[u8]) -> Result<Unpacked, String> {
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err("the bundle is larger than 512 MB".into());
    }
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("not a VisionCraft bundle: {e}"))?;
    let mut project = None;
    let mut files = Vec::new();
    let mut total = 0usize;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| {
            format!("cannot read the bundle (only uncompressed bundles are read): {e}")
        })?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        total = total.saturating_add(f.size() as usize);
        if total > MAX_BUNDLE_BYTES {
            return Err("the bundle unpacks to more than 512 MB".into());
        }
        let mut data = Vec::with_capacity(f.size().min(1 << 28) as usize);
        f.read_to_end(&mut data)
            .map_err(|e| format!("{name}: {e}"))?;
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
}
