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
    /// Asset ids the host knows beyond [`HostState::catalog`] (a window's downloaded library):
    /// planting commands refuse an id that is in neither.
    fn extra_assets(&self) -> Vec<String> {
        Vec::new()
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
    /// A photo (quality `photo`, or frames averaged, a lens or a shutter at another): how to
    /// take it. `None`: the plain still.
    pub photo: Option<PhotoRequest>,
    /// The animation clock (s) the picture is taken at: wind and water move with it.
    pub animation_s: f64,
    /// How much of the lamps' light is on, 0..1 (1: all of it; whether the lamps are on at all
    /// is the project's `view.lights` or the sun's).
    pub lamp_level: f32,
    /// Exposure compensation in stops relative to the automatic exposure: 0 none, positive
    /// brighter.
    pub exposure_offset_ev: f32,
    /// A second grade mixed in: the grade and how much of it, 0..1 (the project's grade is the
    /// first).
    pub grade_mix: Option<(visioncraft_core::Grade, f32)>,
    /// `view.render`'s `shadow_filter`, over the preset's sun shadow filter for this picture:
    /// the comparison taps (1: one hardware tap, the hard edge; 4, 9 or 16: percentage-closer
    /// filtering); `None`: the preset's.
    pub shadow_taps: Option<u32>,
    /// `view.render`'s `shadow_radius_m`: the penumbra's radius in the world (m, 0..1) over the
    /// preset's; `None`: the presets'. Only a 16-tap filter spreads over it (the command refuses
    /// it together with hard, pcf4 or pcf9).
    pub shadow_radius_m: Option<f32>,
    /// The project's image files (plan underlays, wall photos) by their `file` (see
    /// [`HostState::images`]).
    pub images: Vec<(String, Arc<[u8]>)>,
    /// The host's plant catalogue (what `query.sun_hours` sizes crowns from), for the sun-hours
    /// map.
    pub assets: Vec<visioncraft_core::Asset>,
}

/// A photo: `samples` frames averaged; depth of field with an f-number (or a lens radius
/// `aperture_m` over 0) focused at `focus`; motion blur over `shutter_s`.
#[derive(Clone, Debug, PartialEq)]
pub struct PhotoRequest {
    pub samples: u32,
    /// The lens' f-number; `None` (and `aperture_m` 0): all sharp.
    pub f_number: Option<f32>,
    /// Instead of an f-number: the lens' radius (m); 0 none.
    pub aperture_m: f32,
    pub focus: visioncraft_core::photo::Focus,
    /// Where an autofocus looks, 0..1 from the top left.
    pub focus_point: [f32; 2],
    pub shutter_s: f32,
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
    /// The project's image files (PNG or JPEG bytes of plan underlays and wall photos) by the
    /// `file` the project names, read when one is added or the project opened.
    pub images: BTreeMap<String, Arc<[u8]>>,
    /// Image files that could not be read, by `file`: why.
    pub image_missing: BTreeMap<String, String>,
    /// The project as it was last opened or saved, for `dirty`.
    pub saved: Option<Project>,
    /// The agent's name (MCP `clientInfo.name`), for the agent log.
    pub client: Option<String>,
    /// What agents did, oldest first (`agent.log`); session state, not saved.
    pub log: std::collections::VecDeque<Value>,
    /// The host imports the models examples carry itself and hands them over (the browser
    /// window, off its page's thread): `sync_models` leaves them to it instead of importing
    /// them here, which held the page for seconds.
    pub host_imports_examples: bool,
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
        // An example's model travels inside the program.
        // Files saved before example sources were kept as names have them as a path in the
        // saved file's folder: `<folder>/example:<file>`.
        let example = visioncraft_core::example_file(source).or_else(|| {
            let name = Path::new(source).file_name()?.to_str()?;
            visioncraft_core::example_file(name)
        });
        let (path, bytes) = match example {
            Some(b) => (std::path::PathBuf::from(source), b.to_vec()),
            None => {
                let path = self.access.resolve(source, false, self.base().as_deref())?;
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
                (path, bytes)
            }
        };
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

    /// Reads a project image's file (relative to the project file) and its size. A message for
    /// a file that is missing, cannot be read or is not a PNG or JPEG.
    pub fn load_image(&self, file: &str) -> Result<(Arc<[u8]>, [u32; 2]), String> {
        if file.contains("://") {
            return Err(format!("`{file}`: images from a URL are not loaded here"));
        }
        let path = self.access.resolve(file, false, self.base().as_deref())?;
        let len = std::fs::metadata(&path)
            .map_err(|e| format!("cannot read the image `{}`: {e}", path.display()))?
            .len();
        if len > visioncraft_core::underlay::MAX_FILE_BYTES {
            return Err(format!(
                "`{file}` is {} MB; an image may be at most {} MB",
                len / (1024 * 1024),
                visioncraft_core::underlay::MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("cannot read the image `{}`: {e}", path.display()))?;
        let size =
            visioncraft_core::underlay::image_size(&bytes).map_err(|e| format!("`{file}`: {e}"))?;
        Ok((Arc::from(bytes), size))
    }

    /// Reads the images `project` names (underlays, wall photos) and that are not held yet.
    pub fn sync_images(&mut self, project: &Project) {
        for file in project.image_files() {
            if self.images.contains_key(file) || self.image_missing.contains_key(file) {
                continue;
            }
            match self.load_image(file) {
                Ok((bytes, _)) => {
                    self.images.insert(file.to_string(), bytes);
                }
                Err(why) => {
                    self.image_missing.insert(file.to_string(), why);
                }
            }
        }
    }

    /// Loads what `project` refers to and is not loaded yet (or was loaded for another source).
    pub fn sync_models(&mut self, project: &Project) {
        self.sync_images(project);
        for m in &project.models {
            let have = self.models.get(&m.id).is_some_and(|l| l.source == m.source)
                || self.missing.get(&m.id).is_some_and(|(s, _)| *s == m.source);
            if have
                || (self.host_imports_examples && visioncraft_core::is_example_source(&m.source))
            {
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
