//! The animation clips of a model, for `query.clips`. Which file a model is (an imported
//! object's own file, a catalogue asset's first level of detail) is known here; the bytes are
//! the host's, and reading them is the loader's (`gltf_load`).

#[cfg(not(target_arch = "wasm32"))]
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use visioncraft_automation::{Clips, HostState};
use visioncraft_core::catalog::Asset;
use visioncraft_core::{Clip, Project};

/// The file a model is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelFile {
    /// An imported object's file, relative to the project file.
    Object(String),
    /// A library file, relative to the library folder.
    Library(String),
}

/// What the host has of a model file: its bytes, or the clips of a model it loaded already.
#[derive(Clone)]
pub enum Held {
    Bytes(Arc<[u8]>),
    Clips(Vec<Clip>),
}

/// A model's file as the host has it, or why it cannot hand it over.
pub type Reader<'a> = &'a dyn Fn(&str) -> Result<Held, String>;

/// The file of `model`: a catalogue asset id, or `user:<id>` for an imported object.
pub fn model_file(project: &Project, assets: &[&Asset], model: &str) -> Result<ModelFile, String> {
    if let Some(id) = model.strip_prefix("user:") {
        return project
            .objects
            .iter()
            .find(|o| o.id == id)
            .map(|o| ModelFile::Object(o.file.clone()))
            .ok_or_else(|| format!("no imported object `{id}`: object.list names them"));
    }
    match assets.iter().find(|a| a.id == model) {
        Some(a) => a
            .lods
            .first()
            .map(|l| ModelFile::Library(l.file.clone()))
            .ok_or_else(|| format!("`{model}` is not a glTF model")),
        None => Err(format!(
            "no model `{model}`: catalog.search lists the assets, and an imported object is `user:<id>`"
        )),
    }
}

/// The clips of `model`: `object` holds an imported object's file, `library` a library file.
pub fn clips_of(
    project: &Project,
    assets: &[&Asset],
    object: Reader,
    library: Reader,
    model: &str,
) -> Result<Vec<Clip>, String> {
    let (file, held) = match model_file(project, assets, model)? {
        ModelFile::Object(file) => {
            let held = object(&file);
            (file, held)
        }
        ModelFile::Library(file) => {
            let held = library(&file);
            (file, held)
        }
    };
    match held? {
        Held::Clips(clips) => Ok(clips),
        Held::Bytes(bytes) => crate::gltf_load::parse(&bytes)
            .map(|glb| glb.clips)
            .map_err(|e| format!("`{model}` ({file}): {e}")),
    }
}

/// The command line's clips: an imported object's file from where the host may read it (the
/// project's folder), a library file from the library folder, and the garden-light fixtures,
/// made here as the window makes them.
#[cfg(not(target_arch = "wasm32"))]
pub struct FolderClips {
    library: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FolderClips {
    pub fn new(library: PathBuf) -> Self {
        Self { library }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Clips for FolderClips {
    fn clips(
        &mut self,
        model: &str,
        project: &Project,
        state: &HostState,
    ) -> Result<Vec<Clip>, String> {
        let assets: Vec<&Asset> = state.catalog.assets.iter().collect();
        let read = |path: PathBuf| -> Result<Held, String> {
            std::fs::read(&path)
                .map(|b| Held::Bytes(Arc::from(b)))
                .map_err(|e| format!("cannot read `{}`: {e}", path.display()))
        };
        let object = |file: &str| {
            state
                .access
                .resolve(file, false, state.base().as_deref())
                .and_then(read)
        };
        let fixtures = crate::fixtures::files();
        let library = |file: &str| {
            if let Some((_, bytes)) = fixtures.iter().find(|(path, _)| path == file) {
                return Ok(Held::Bytes(Arc::from(bytes.clone())));
            }
            beneath(&self.library, file)
                .ok_or_else(|| {
                    format!(
                        "`{file}` is not a file of the library folder `{}`",
                        self.library.display()
                    )
                })
                .and_then(read)
        };
        clips_of(project, &assets, &object, &library, model)
    }
}

/// `file` under `root`, or `None` when it would leave the folder (`..`, an absolute path, a
/// drive or a backslash).
#[cfg(not(target_arch = "wasm32"))]
fn beneath(root: &Path, file: &str) -> Option<PathBuf> {
    let rel = Path::new(file);
    let leaves = rel.is_absolute()
        || file.contains('\\')
        || file.contains(':')
        || rel.components().any(|c| matches!(c, Component::ParentDir));
    (!leaves).then(|| root.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::Cursor;
    use visioncraft_automation::commands::ListQuery;
    use visioncraft_automation::{Access, Headless, commands, mcp, rpc};
    use visioncraft_core::catalog::{Category, Lod, Origin};

    /// A project folder with an imported hinge (one clip) and an imported rock (a triangle that
    /// stands still), and a library folder with the hinge as a catalogue asset, a catalogue
    /// asset whose file is not there, and an asset that is not a model.
    fn folder(name: &str) -> Headless {
        // A lib test has no `CARGO_TARGET_TMPDIR`, and tests run side by side: a folder of
        // its own under the temp dir.
        let dir = std::env::temp_dir().join(format!("visioncraft-model-clips-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("objects")).unwrap();
        std::fs::create_dir_all(dir.join("models/hinge")).unwrap();
        std::fs::write(
            dir.join("objects/hinge.glb"),
            crate::gltf_load::tests::hinge().finish(),
        )
        .unwrap();
        std::fs::write(
            dir.join("objects/rock.glb"),
            crate::user_objects::tests::triangle_glb(),
        )
        .unwrap();
        std::fs::write(
            dir.join("models/hinge/lod0.glb"),
            crate::gltf_load::tests::hinge().finish(),
        )
        .unwrap();
        let asset = |id: &str, file: &str, lods: bool| Asset {
            id: id.into(),
            name: id.into(),
            category: Category::Other,
            tags: Vec::new(),
            license: String::new(),
            origin: Origin::BuiltIn,
            author: String::new(),
            source_url: String::new(),
            size_m: [1.0; 3],
            lods: lods
                .then(|| Lod {
                    file: file.into(),
                    max_distance_m: f32::INFINITY,
                })
                .into_iter()
                .collect(),
            thumbnail: String::new(),
            plant: None,
            light: None,
        };
        let mut host = Headless::new(Access::any());
        host.state.file = Some(dir.join("garden.visioncraft"));
        host.state.catalog.assets = vec![
            asset("lib_hinge", "models/hinge/lod0.glb", true),
            asset("ghost", "models/ghost/lod0.glb", true),
            asset("not_a_model", "", false),
        ];
        host.clips = Some(Box::new(FolderClips::new(dir)));
        for (name, file, size_m) in [
            ("Hinge", "objects/hinge.glb", [1.0; 3]),
            ("Rock", "objects/rock.glb", [2.0, 1.0, 0.1]),
        ] {
            host.session
                .run(
                    "object.add",
                    json!({"name": name, "file": file, "size_m": size_m}),
                )
                .unwrap();
        }
        host
    }

    fn swing() -> Value {
        json!({"clips": [{"name": "swing", "duration_s": 1.0, "channels": 1}]})
    }

    #[test]
    fn the_clips_of_a_model_come_from_its_file() {
        let mut host = folder("files");
        let before = host.session.revision();
        let ask = |host: &mut Headless, model: &str| {
            commands::execute(host, "query.clips", &json!({"model": model}))
        };
        for model in ["user:uo1", "lib_hinge"] {
            let r = ask(&mut host, model).unwrap_or_else(|e| panic!("{model}: {e}"));
            assert_eq!(r, swing(), "{model}");
        }
        assert_eq!(ask(&mut host, "user:uo2").unwrap(), json!({"clips": []}));
        for (model, word) in [
            ("user:uo9", "no imported object `uo9`"),
            ("nope", "no model `nope`"),
            ("not_a_model", "not a glTF model"),
            ("ghost", "cannot read"),
        ] {
            let e = ask(&mut host, model).expect_err(model);
            assert!(e.contains(word), "{model}: {e}");
        }
        assert!(ask(&mut host, "").is_err());
        // The parameter is checked as everywhere.
        let e = commands::execute(
            &mut host,
            "query.clips",
            &json!({"model": "user:uo1", "lod": 2}),
        )
        .unwrap_err();
        assert!(e.contains("unknown parameter `lod`"), "{e}");
        let e = commands::execute(&mut host, "query.clips", &json!({})).unwrap_err();
        assert!(e.contains("missing `model`"), "{e}");
        // Nothing of it changes the project.
        assert_eq!(host.session.revision(), before);
    }

    #[test]
    fn both_protocols_carry_the_clips() {
        let mut host = folder("protocols");
        let line = r#"{"id": 1, "method": "engine.execute", "params": {"command": "query.clips", "params": {"model": "user:uo1"}}}"#;
        let reply: Value =
            serde_json::from_str(&rpc::handle_line(&mut host, line).unwrap()).unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["result"], swing());

        let input = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "command_run", "arguments": {"id": "query.clips", "params": {"model": "user:uo2"}}}}),
        ]
        .iter()
        .map(|m| format!("{m}\n"))
        .collect::<String>();
        let mut output = Vec::new();
        mcp::serve(&mut host, Cursor::new(input.into_bytes()), &mut output).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let text = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            json!({"clips": []})
        );
    }

    #[test]
    fn a_session_without_model_files_says_so() {
        let mut host = Headless::new(Access::default());
        let e =
            commands::execute(&mut host, "query.clips", &json!({"model": "user:uo1"})).unwrap_err();
        assert!(e.contains("no model files"), "{e}");
    }

    #[test]
    fn the_docs_list_the_command() {
        let host = Headless::new(Access::default());
        let docs = commands::docs(&host.session);
        let d = docs
            .iter()
            .find(|d| d.id == "query.clips")
            .expect("documented");
        let params: Vec<&str> = d.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(params, ["model"]);
        assert_eq!(
            d.result.as_deref(),
            Some("{clips: [{name, duration_s, channels}]}")
        );
        assert!(!d.undoable);
        let list = commands::command_list(
            &host.session,
            &ListQuery {
                id: Some("query.clips"),
                brief: true,
                ..Default::default()
            },
        );
        assert_eq!(list["commands"][0]["id"], "query.clips");
        assert_eq!(list["commands"][0]["params"], "{\"model\": string}");
        let table = include_str!("../../../crates/visioncraft-automation/docs/control-protocol.md");
        assert!(
            table
                .lines()
                .any(|l| l.starts_with("| `query.clips` | `{model}` |")),
            "control-protocol.md has no row for query.clips"
        );
    }

    #[test]
    fn beneath_stays_in_the_folder() {
        let root = Path::new("/tmp/lib");
        assert_eq!(
            beneath(root, "models/a/lod0.glb"),
            Some(PathBuf::from("/tmp/lib/models/a/lod0.glb"))
        );
        for out in [
            "../x.glb",
            "/etc/passwd",
            "a/../../b.glb",
            "C:\\x.glb",
            "a\\b.glb",
        ] {
            assert_eq!(beneath(root, out), None, "{out}");
        }
    }
}
