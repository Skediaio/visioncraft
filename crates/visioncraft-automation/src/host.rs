//! What the servers run on: a [`Host`] is a core [`Session`] plus what the session does not
//! hold (the open file, the folders automation may touch, the models' geometry, the catalogue)
//! and, when it has a GPU, pictures. [`Headless`] is the host of `visioncraft-cli`; the desktop
//! window implements [`Host`] over its own studio.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use visioncraft_core::{Catalog, ImportedModel, ModelKind, Project, Session};

/// What a server needs from the program it runs in.
pub trait Host {
    /// The session and the state beside it, borrowed together.
    fn parts(&mut self) -> (&mut Session, &mut HostState);
    fn session(&mut self) -> &mut Session {
        self.parts().0
    }
    fn state(&mut self) -> &mut HostState {
        self.parts().1
    }
    /// Draws `project` (the session's, or a copy with the time of the request) with the
    /// geometry in `models`. A host without a GPU refuses.
    fn render(
        &mut self,
        project: &Project,
        models: &BTreeMap<String, LoadedModel>,
        request: &RenderRequest,
    ) -> Result<Rendered, String> {
        let _ = (project, models, request);
        Err("this host has no renderer; start visioncraft-cli on a machine with a GPU".into())
    }
    /// Methods of the host's own besides the protocol's (a window's `ui.*`), as `methods`
    /// lists them; `call` answers them.
    fn methods(&self) -> &[crate::rpc::Method] {
        &[]
    }
    /// `None` when `method` is not one of [`Host::methods`].
    fn call(&mut self, method: &str, params: &Value) -> Option<Result<Value, String>> {
        let _ = (method, params);
        None
    }
}

/// Draws pictures for [`Headless`] (the CLI's GPU renderer).
pub trait Pictures {
    fn render(
        &mut self,
        project: &Project,
        models: &BTreeMap<String, LoadedModel>,
        request: &RenderRequest,
    ) -> Result<Rendered, String>;
}

/// What `view.render` asks for. The camera is the viewpoint's, else `camera`, else the whole
/// scene framed from the default orbit.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderRequest {
    pub width: u32,
    pub height: u32,
    /// `[position, target, fov_y_deg]`.
    pub camera: Option<([f32; 3], [f32; 3], f32)>,
    /// `low`, `medium`, `high` or `photo`.
    pub quality: String,
    /// With quality `photo`: how to take it.
    pub photo: Option<PhotoRequest>,
}

/// A photo: `samples` frames averaged; depth of field when `aperture_m` is over 0, focused at
/// `focus_m` (else the camera target).
#[derive(Clone, Debug, PartialEq)]
pub struct PhotoRequest {
    pub samples: u32,
    pub aperture_m: f32,
    pub focus_m: Option<f32>,
}

/// A picture: PNG bytes and what the renderer says about it (adapter, plants drawn, ...).
pub struct Rendered {
    pub png: Vec<u8>,
    pub info: Value,
}

/// A model's geometry as loaded from its `source`.
#[derive(Clone)]
pub struct LoadedModel {
    /// The `source` of the model reference it was loaded for.
    pub source: String,
    pub path: PathBuf,
    pub bytes: Arc<[u8]>,
    pub model: Arc<ImportedModel>,
}

/// The folders automation may read from and write to. A relative path is beneath the root; an
/// absolute one must lie beneath it. `None` denies that direction; [`Access::any`] is the CLI's
/// own (`run`, `render`), for a trusted local user.
#[derive(Clone, Debug, Default)]
pub struct Access {
    pub read: Option<PathBuf>,
    pub write: Option<PathBuf>,
    pub any: bool,
}

impl Access {
    pub fn any() -> Self {
        Self {
            any: true,
            ..Self::default()
        }
    }

    pub fn roots(read: Option<PathBuf>, write: Option<PathBuf>) -> Self {
        Self {
            read,
            write,
            any: false,
        }
    }

    /// Where `path` is on disk if the direction allows it. `base` is the folder relative
    /// paths start from when access is unrestricted (the open file's folder).
    pub fn resolve(&self, path: &str, write: bool, base: Option<&Path>) -> Result<PathBuf, String> {
        let given = Path::new(path);
        if self.any {
            return Ok(match base {
                Some(base) if given.is_relative() => base.join(given),
                _ => given.to_path_buf(),
            });
        }
        let (root, flag) = if write {
            (&self.write, "--automation-write-root")
        } else {
            (&self.read, "--automation-read-root")
        };
        let direction = if write { "write" } else { "read" };
        let root = root.as_ref().ok_or_else(|| {
            format!("`{path}`: this server may not {direction} files; start it with {flag} DIR")
        })?;
        if given
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(format!("`{path}`: `..` is not allowed in a path"));
        }
        let joined = if given.is_relative() {
            root.join(given)
        } else {
            given.to_path_buf()
        };
        // The deepest part that exists, links resolved, must lie beneath the resolved root.
        let real_root = std::fs::canonicalize(root)
            .map_err(|e| format!("the {direction} root `{}`: {e}", root.display()))?;
        let mut probe = joined.clone();
        let real = loop {
            if let Ok(real) = std::fs::canonicalize(&probe) {
                break real;
            }
            if !probe.pop() {
                break PathBuf::new();
            }
        };
        let written_beneath = joined.starts_with(root) || joined.starts_with(&real_root);
        if !real.starts_with(&real_root) || !written_beneath {
            return Err(format!(
                "`{path}` is outside the {direction} root `{}`",
                root.display()
            ));
        }
        Ok(joined)
    }
}

/// Everything a host keeps beside its session.
#[derive(Default)]
pub struct HostState {
    /// The `.visioncraft` file the project was opened from or last saved to.
    pub file: Option<PathBuf>,
    pub access: Access,
    /// Geometry by model id.
    pub models: BTreeMap<String, LoadedModel>,
    /// Models that could not be loaded, by id: (source, why).
    pub missing: BTreeMap<String, (String, String)>,
    pub catalog: Catalog,
    /// The project as it was last opened or saved, for `dirty`.
    pub saved: Option<Project>,
    /// The agent's name (MCP `clientInfo.name`), for the agent log.
    pub client: Option<String>,
    /// What agents did, oldest first (`agent.log`); session state, not saved.
    pub log: std::collections::VecDeque<Value>,
}

impl HostState {
    pub fn new(access: Access) -> Self {
        Self {
            access,
            catalog: builtin_catalog(),
            saved: Some(Project::default()),
            ..Self::default()
        }
    }

    /// The folder relative model sources start from.
    pub fn base(&self) -> Option<PathBuf> {
        self.file
            .as_ref()
            .and_then(|f| f.parent())
            .map(Path::to_path_buf)
    }

    /// Reads and imports a model file. `source` is as the model reference has it.
    pub fn load(&self, kind: ModelKind, source: &str) -> Result<LoadedModel, String> {
        if source.contains("://") {
            return Err(format!("`{source}`: models from a URL are not loaded here"));
        }
        let path = self.access.resolve(source, false, self.base().as_deref())?;
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
        let model = match kind {
            ModelKind::Craft3d => visioncraft_core::import_3dcraft(&bytes),
            ModelKind::Obj => visioncraft_core::import_obj(&bytes),
        }
        .map_err(|e| format!("`{source}`: {e}"))?;
        Ok(LoadedModel {
            source: source.to_string(),
            path,
            bytes: Arc::from(bytes),
            model: Arc::new(model),
        })
    }

    /// Loads what `project` refers to and is not loaded yet (or was loaded for another source).
    pub fn sync_models(&mut self, project: &Project) {
        for m in &project.models {
            let have = self.models.get(&m.id).is_some_and(|l| l.source == m.source)
                || self.missing.get(&m.id).is_some_and(|(s, _)| *s == m.source);
            if have {
                continue;
            }
            self.models.remove(&m.id);
            match self.load(m.kind, &m.source) {
                Ok(loaded) => {
                    self.missing.remove(&m.id);
                    self.models.insert(m.id.clone(), loaded);
                }
                Err(why) => {
                    self.missing.insert(m.id.clone(), (m.source.clone(), why));
                }
            }
        }
    }
}

/// The built-in asset catalogue (`assets/catalog.json`), compiled in.
pub fn builtin_catalog() -> Catalog {
    Catalog::from_json_bytes(include_bytes!("../../../assets/catalog.json")).unwrap_or_default()
}

/// The host of `visioncraft-cli` (and of tests): a session, its state, and pictures when
/// [`Headless::pictures`] is set.
pub struct Headless {
    pub session: Session,
    pub state: HostState,
    pub pictures: Option<Box<dyn Pictures>>,
}

impl Headless {
    pub fn new(access: Access) -> Self {
        Self {
            session: Session::new(Project::default()),
            state: HostState::new(access),
            pictures: None,
        }
    }
}

impl Host for Headless {
    fn parts(&mut self) -> (&mut Session, &mut HostState) {
        (&mut self.session, &mut self.state)
    }

    fn render(
        &mut self,
        project: &Project,
        models: &BTreeMap<String, LoadedModel>,
        request: &RenderRequest,
    ) -> Result<Rendered, String> {
        match &mut self.pictures {
            Some(p) => p.render(project, models, request),
            None => Err(
                "this session has no renderer: no GPU was set up (visioncraft-cli sets one up \
                 when the machine has an adapter)"
                    .into(),
            ),
        }
    }
}

/// `{"missing": [...]}` entries for messages and inspect.
pub(crate) fn missing_json(state: &HostState, project: &Project) -> Vec<Value> {
    project
        .models
        .iter()
        .filter_map(|m| {
            state
                .missing
                .get(&m.id)
                .map(|(_, why)| json!({ "id": m.id, "source": m.source, "error": why }))
        })
        .collect()
}
