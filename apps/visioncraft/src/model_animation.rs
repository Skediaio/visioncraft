//! A model's animation, for `query.clips` and playback. Which file a model is (an imported
//! object's own file, a catalogue asset's first level of detail) is known here; the bytes are
//! the host's, and reading them is the loader's (`gltf_load`).

#[cfg(not(target_arch = "wasm32"))]
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use visioncraft_automation::{Animations, HostState};
use visioncraft_core::catalog::{Asset, Lod};
use visioncraft_core::{Animation, Project};

/// The file a model is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelFile {
    /// An imported object's file, relative to the project file.
    Object(String),
    /// A library file, relative to the library folder.
    Library(String),
}

/// What the host has of a model file: its bytes, or what the window kept of a model it
/// loaded already.
#[derive(Clone)]
pub enum Held {
    Bytes(Arc<[u8]>),
    /// A model file's animation as it was loaded ([`Library::kept`]), and the node of each of
    /// its bones.
    Loaded(Animation, Vec<(usize, String)>),
}

/// The file a host hands over, read: its animation and the node of each of its bones.
type Read = (Animation, Vec<(usize, String)>);

/// `file`'s animation and bones from what the host has of it.
fn read(held: Held) -> Result<Read, String> {
    match held {
        Held::Loaded(animation, nodes) => Ok((animation, nodes)),
        Held::Bytes(bytes) => {
            crate::gltf_load::parse(&bytes).map(|glb| (glb.animation(), glb.nodes()))
        }
    }
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

/// The animation of `model`: `object` holds an imported object's file, `library` a library
/// file. A catalogue asset's is its first level's, with a warning for each other level whose
/// file `library` has and that differs from the first level's.
pub fn animation_of(
    project: &Project,
    assets: &[&Asset],
    object: Reader,
    library: Reader,
    model: &str,
) -> Result<Animation, String> {
    match model_file(project, assets, model)? {
        ModelFile::Object(file) => object(&file)
            .and_then(read)
            .map(|(animation, _)| animation)
            .map_err(|e| format!("`{model}` ({file}): {e}")),
        ModelFile::Library(file) => {
            let (first, nodes) = library(&file)
                .and_then(read)
                .map_err(|e| format!("`{model}` ({file}): {e}"))?;
            let lods = &assets
                .iter()
                .find(|a| a.id == model)
                .expect("`model_file` found the asset")
                .lods;
            Ok(with_level_warnings(first, &nodes, lods, library))
        }
    }
}

/// The first level's animation, with [`level_warnings`] for every other level of the asset
/// whose file `library` has (a browser only those it loaded): one that it cannot hand over is
/// not compared.
fn with_level_warnings(
    mut first: Animation,
    nodes: &[(usize, String)],
    lods: &[Lod],
    library: Reader,
) -> Animation {
    for (index, lod) in lods.iter().enumerate().skip(1) {
        let Ok((level, level_nodes)) = library(&lod.file).and_then(read) else {
            continue;
        };
        let warnings = level_warnings(index, &lod.file, &level, &level_nodes, &first, nodes);
        first.warnings.extend(warnings);
    }
    first
}

/// What level of detail `index` of a catalogue asset (`file` its model file; `level` its
/// animation and the node of each of its bones) does not share with the first level's (`first`,
/// `first_nodes`): one line naming the file, none when they agree. The first level is the
/// authority: its clips are the asset's and its bones pose every level. So a bone of this level
/// whose node it lacks is drawn at rest, one of its own bones this level lacks is not drawn at
/// this level, a clip that is missing here is played all the same, one that lasts another
/// length here is played at the first level's length, and a clip only this level has never
/// plays.
pub fn level_warnings(
    index: usize,
    file: &str,
    level: &Animation,
    level_nodes: &[(usize, String)],
    first: &Animation,
    first_nodes: &[(usize, String)],
) -> Vec<String> {
    let mut diffs = Vec::new();
    for (node, name) in level_nodes {
        if !first_nodes.iter().any(|(n, _)| n == node) {
            diffs.push(format!(
                "{} is not in the first level, drawn at rest",
                named(*node, name)
            ));
        }
    }
    for (node, name) in first_nodes {
        if !level_nodes.iter().any(|(n, _)| n == node) {
            diffs.push(format!(
                "{} of the first level is not in it, so it is not drawn at this level",
                named(*node, name)
            ));
        }
    }
    for clip in &first.clips {
        let own: Vec<&visioncraft_core::Clip> =
            level.clips.iter().filter(|c| c.name == clip.name).collect();
        if own.is_empty() {
            diffs.push(format!(
                "clip `{}` is missing; this level is posed by the first level's clip",
                clip.name
            ));
        } else if own
            .iter()
            .all(|c| (c.duration - clip.duration).abs() > 0.001)
        {
            diffs.push(format!(
                "clip `{}` lasts {} here, {} in the first level; the first level's plays",
                clip.name,
                seconds(own[0].duration),
                seconds(clip.duration)
            ));
        }
    }
    let mut only: Vec<&str> = Vec::new();
    for clip in &level.clips {
        if !first.clips.iter().any(|c| c.name == clip.name) && !only.contains(&clip.name.as_str()) {
            only.push(&clip.name);
            diffs.push(format!("clip `{}` is only here; it never plays", clip.name));
        }
    }
    if diffs.is_empty() {
        return Vec::new();
    }
    vec![format!("level {index} (`{file}`): {}", diffs.join("; "))]
}

/// A node as a warning names it: by its name and index, or by its index alone without a name.
fn named(node: usize, name: &str) -> String {
    if name.is_empty() {
        format!("node {node}")
    } else {
        format!("node `{name}` ({node})")
    }
}

/// A duration in seconds, without trailing zeros (`1 s`, `2.5 s`).
fn seconds(s: f32) -> String {
    let text = format!("{s:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text} s")
}

/// The command line's animations: an imported object's file from where the host may read it (the
/// project's folder), a library file from the library folder, and the garden-light fixtures,
/// made here as the window makes them.
#[cfg(not(target_arch = "wasm32"))]
pub struct FolderAnimations {
    library: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FolderAnimations {
    pub fn new(library: PathBuf) -> Self {
        Self { library }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Animations for FolderAnimations {
    fn animation(
        &mut self,
        model: &str,
        project: &Project,
        state: &HostState,
    ) -> Result<Animation, String> {
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
        animation_of(project, &assets, &object, &library, model)
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
        host.animations = Some(Box::new(FolderAnimations::new(dir)));
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
        json!({"clips": [{"name": "swing", "duration_s": 1.0, "channels": 1}], "warnings": []})
    }

    /// A catalogue asset with one level of detail at `file`, of the size of a box.
    fn asset_of(id: &str) -> Asset {
        Asset {
            id: id.into(),
            name: id.into(),
            category: Category::Other,
            tags: Vec::new(),
            license: String::new(),
            origin: Origin::BuiltIn,
            author: String::new(),
            source_url: String::new(),
            size_m: [1.0; 3],
            lods: vec![Lod {
                file: String::new(),
                max_distance_m: f32::INFINITY,
            }],
            thumbnail: String::new(),
            plant: None,
            light: None,
        }
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
        assert_eq!(
            ask(&mut host, "user:uo2").unwrap(),
            json!({"clips": [], "warnings": []})
        );
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

    /// A catalogue asset's clips are its first level's, and `warnings` has one line per other
    /// level whose file differs from the first level's: a node it lacks (drawn at rest), a clip
    /// that is missing or lasts another length. A level whose file is not there is not compared.
    #[test]
    fn an_asset_s_clips_are_its_first_level_s_and_its_other_levels_are_compared() {
        let dir = std::env::temp_dir().join("visioncraft-model-clips-levels");
        let _ = std::fs::remove_dir_all(&dir);
        for a in ["chest", "alike", "ghost"] {
            std::fs::create_dir_all(dir.join("models").join(a)).unwrap();
        }
        let (first, second) = (
            crate::gltf_load::tests::chest_lod0().finish(),
            crate::gltf_load::tests::chest_lod1().finish(),
        );
        std::fs::write(dir.join("models/chest/lod0.glb"), &first).unwrap();
        std::fs::write(dir.join("models/chest/lod1.glb"), &second).unwrap();
        std::fs::write(dir.join("models/alike/lod0.glb"), &first).unwrap();
        std::fs::write(dir.join("models/alike/lod1.glb"), &first).unwrap();
        std::fs::write(dir.join("models/ghost/lod0.glb"), &first).unwrap();
        let two = |id: &str, second: &str| Asset {
            lods: [
                Lod {
                    file: format!("models/{id}/lod0.glb"),
                    max_distance_m: 6.0,
                },
                Lod {
                    file: second.into(),
                    max_distance_m: f32::INFINITY,
                },
            ]
            .into_iter()
            .collect(),
            ..asset_of(id)
        };
        let mut host = Headless::new(Access::any());
        host.state.file = Some(dir.join("garden.visioncraft"));
        host.state.catalog.assets = vec![
            two("chest", "models/chest/lod1.glb"),
            two("alike", "models/alike/lod1.glb"),
            two("ghost", "models/ghost/missing.glb"),
        ];
        host.animations = Some(Box::new(FolderAnimations::new(dir)));
        let ask = |host: &mut Headless, model: &str| {
            commands::execute(host, "query.clips", &json!({"model": model}))
        };
        let r = ask(&mut host, "chest").unwrap();
        // The first level's clips, with the first level's lengths.
        assert_eq!(
            r["clips"],
            json!([
                {"name": "open", "duration_s": 1.0, "channels": 1},
                {"name": "shake", "duration_s": 1.0, "channels": 1},
            ]),
            "{r}"
        );
        assert_eq!(
            r["warnings"],
            json!([
                "level 1 (`models/chest/lod1.glb`): node `lid2` (2) is not in the first level, drawn at rest; clip `open` lasts 2 s here, 1 s in the first level; the first level's plays; clip `shake` is missing; this level is posed by the first level's clip"
            ]),
            "{r}"
        );
        // A level that agrees says nothing, and one whose file is not there is not compared.
        for model in ["alike", "ghost"] {
            let r = ask(&mut host, model).unwrap();
            assert_eq!(r["warnings"], json!([]), "{model}: {r}");
            assert_eq!(r["clips"][0]["name"], "open", "{model}");
        }
    }

    /// A level's warning names what it does not share with the first level: a bone of its own
    /// (a node without a name by its index), a bone of the first level it lacks, a clip only it
    /// has; files that agree say nothing.
    #[test]
    fn a_level_s_warning_names_what_differs() {
        let clip = |name: &str, duration: f32| visioncraft_core::Clip {
            name: name.into(),
            duration,
            channels: Vec::new(),
        };
        let first = Animation {
            clips: vec![clip("open", 1.0)],
            warnings: vec!["a malformed channel".into()],
            rig: None,
            rest_low_y: None,
        };
        let level = Animation {
            clips: vec![clip("open", 1.0), clip("bounce", 0.5), clip("bounce", 0.25)],
            ..Default::default()
        };
        let first_nodes = [(1, "lid".to_string()), (2, String::new())];
        let level_nodes = [(0, "body".to_string()), (1, "lid".to_string())];
        assert_eq!(
            level_warnings(
                2,
                "oak_lod2.glb",
                &level,
                &level_nodes,
                &first,
                &first_nodes
            ),
            [
                "level 2 (`oak_lod2.glb`): node `body` (0) is not in the first level, drawn at rest; node 2 of the first level is not in it, so it is not drawn at this level; clip `bounce` is only here; it never plays"
            ]
        );
        assert!(
            level_warnings(1, "same.glb", &first, &first_nodes, &first, &first_nodes).is_empty()
        );
        let longer = Animation {
            clips: vec![clip("open", 2.5), clip("open", 2.5)],
            ..Default::default()
        };
        assert_eq!(
            level_warnings(1, "long.glb", &longer, &first_nodes, &first, &first_nodes),
            [
                "level 1 (`long.glb`): clip `open` lasts 2.5 s here, 1 s in the first level; the first level's plays"
            ]
        );
        for (s, text) in [
            (1.0, "1 s"),
            (2.5, "2.5 s"),
            (0.333, "0.333 s"),
            (10.0, "10 s"),
        ] {
            assert_eq!(seconds(s), text);
        }
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
            json!({"clips": [], "warnings": []})
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
            Some("{clips: [{name, duration_s, channels}], warnings: [string]}")
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
