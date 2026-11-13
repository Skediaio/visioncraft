//! Files: asynchronous open/save dialogs (native and web), the inbox their results arrive in,
//! and on native a watcher on the linked model files.

use eframe::egui;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub enum Msg {
    /// A file the user chose or dropped. `target` is the model it should replace, if any.
    Opened {
        name: String,
        bytes: Vec<u8>,
        path: Option<PathBuf>,
        target: Option<String>,
    },
    /// A watched model file changed on disk.
    FileChanged(String),
    Saved(String),
    Failed(String),
}

#[derive(Clone, Default)]
pub struct Inbox(Arc<Mutex<Vec<Msg>>>);

impl Inbox {
    pub fn push(&self, m: Msg, ctx: &egui::Context) {
        self.0.lock().expect("inbox").push(m);
        ctx.request_repaint();
    }

    pub fn take(&self) -> Vec<Msg> {
        std::mem::take(&mut *self.0.lock().expect("inbox"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    std::thread::spawn(move || pollster::block_on(fut));
}

#[cfg(target_arch = "wasm32")]
fn spawn(fut: impl Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(fut);
}

pub const OPEN_EXTENSIONS: [&str; 5] = ["3dcraft", "obj", "visioncraft", "mcmat", "json"];

/// Opens a file picker; the chosen file arrives in `inbox`.
pub fn pick(
    inbox: &Inbox,
    ctx: &egui::Context,
    target: Option<String>,
    extensions: &'static [&'static str],
) {
    let (inbox, ctx) = (inbox.clone(), ctx.clone());
    spawn(async move {
        let Some(h) = rfd::AsyncFileDialog::new()
            .add_filter("VisionCraft files", extensions)
            .pick_file()
            .await
        else {
            return;
        };
        let bytes = h.read().await;
        #[cfg(not(target_arch = "wasm32"))]
        let path = Some(h.path().to_path_buf());
        #[cfg(target_arch = "wasm32")]
        let path = None;
        inbox.push(
            Msg::Opened {
                name: h.file_name(),
                bytes,
                path,
                target,
            },
            &ctx,
        );
    });
}

/// Saves `bytes` through a dialog (a download on the web).
pub fn save(inbox: &Inbox, ctx: &egui::Context, suggested: String, bytes: Vec<u8>) {
    save_as(inbox, ctx, suggested, bytes, "visioncraft");
}

/// Saves through a dialog with the given file extension filter.
pub fn save_as(
    inbox: &Inbox,
    ctx: &egui::Context,
    suggested: String,
    bytes: Vec<u8>,
    extension: &'static str,
) {
    let (inbox, ctx) = (inbox.clone(), ctx.clone());
    spawn(async move {
        let Some(h) = rfd::AsyncFileDialog::new()
            .set_file_name(suggested)
            .add_filter("File", &[extension])
            .save_file()
            .await
        else {
            return;
        };
        match h.write(&bytes).await {
            Ok(()) => inbox.push(Msg::Saved(h.file_name()), &ctx),
            Err(e) => inbox.push(Msg::Failed(format!("could not save: {e}")), &ctx),
        }
    });
}

/// Watches linked model files and reports changes in the inbox. On the web it does nothing:
/// a page cannot see files on disk, "Reload model" re-opens the picker instead.
#[cfg(not(target_arch = "wasm32"))]
pub struct Watcher {
    watcher: Option<notify::RecommendedWatcher>,
    files: Arc<Mutex<Vec<(PathBuf, String)>>>,
    inbox: Inbox,
    ctx: egui::Context,
}

#[cfg(not(target_arch = "wasm32"))]
impl Watcher {
    pub fn new(inbox: Inbox, ctx: egui::Context) -> Self {
        let files: Arc<Mutex<Vec<(PathBuf, String)>>> = Arc::default();
        let (f, i, c) = (files.clone(), inbox.clone(), ctx.clone());
        let watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
            let Ok(ev) = ev else { return };
            if !matches!(
                ev.kind,
                notify::EventKind::Modify(_) | notify::EventKind::Create(_)
            ) {
                return;
            }
            let files = f.lock().expect("files");
            for (path, id) in files.iter() {
                if ev.paths.iter().any(|p| {
                    p == path || (p.file_name() == path.file_name() && p.parent() == path.parent())
                }) {
                    i.push(Msg::FileChanged(id.clone()), &c);
                }
            }
        })
        .ok();
        Self {
            watcher,
            files,
            inbox,
            ctx,
        }
    }

    /// Starts watching `path` for model `id`. The folder is watched, not the file, so editors
    /// that save by replacing the file are seen too.
    pub fn watch(&mut self, id: &str, path: PathBuf) {
        use notify::{RecursiveMode, Watcher as _};
        let path = path.canonicalize().unwrap_or(path);
        if let (Some(w), Some(dir)) = (self.watcher.as_mut(), path.parent())
            && let Err(e) = w.watch(dir, RecursiveMode::NonRecursive)
        {
            self.inbox.push(
                Msg::Failed(format!("cannot watch {}: {e}", path.display())),
                &self.ctx,
            );
        }
        let mut files = self.files.lock().expect("files");
        files.retain(|(_, i)| i != id);
        files.push((path, id.to_string()));
    }

    pub fn forget_all(&mut self) {
        self.files.lock().expect("files").clear();
    }
}

#[cfg(target_arch = "wasm32")]
pub struct Watcher;

#[cfg(target_arch = "wasm32")]
impl Watcher {
    pub fn new(_: Inbox, _: egui::Context) -> Self {
        Self
    }
    pub fn watch(&mut self, _: &str, _: PathBuf) {}
    pub fn forget_all(&mut self) {}
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn a_changed_file_is_reported() {
        let dir = std::env::temp_dir().join(format!("vc-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("model.obj");
        std::fs::write(&file, "v 0 0 0\n").unwrap();
        let inbox = Inbox::default();
        let mut w = Watcher::new(inbox.clone(), egui::Context::default());
        w.watch("m1", file.clone());
        std::thread::sleep(std::time::Duration::from_millis(300));
        std::fs::write(&file, "v 1 1 1\n").unwrap();
        let mut seen = false;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            seen |= inbox
                .take()
                .iter()
                .any(|m| matches!(m, Msg::FileChanged(id) if id == "m1"));
            if seen {
                break;
            }
        }
        std::fs::remove_dir_all(&dir).ok();
        assert!(seen, "no change event within four seconds");
    }
}

/// Reads a file dropped on the window and delivers it to the inbox (asynchronously on the web).
pub fn read_dropped(inbox: &Inbox, ctx: &egui::Context, f: egui::DroppedFileHandle) {
    let (inbox, ctx) = (inbox.clone(), ctx.clone());
    let name = f
        .path()
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    #[cfg(not(target_arch = "wasm32"))]
    match f.bytes() {
        Ok(bytes) => inbox.push(
            Msg::Opened {
                name,
                bytes,
                path: Some(f.path().to_path_buf()),
                target: None,
            },
            &ctx,
        ),
        Err(e) => inbox.push(Msg::Failed(format!("cannot read {name}: {e}")), &ctx),
    }
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        match f.bytes_async().await {
            Ok(bytes) => inbox.push(
                Msg::Opened {
                    name,
                    bytes,
                    path: None,
                    target: None,
                },
                &ctx,
            ),
            Err(e) => inbox.push(Msg::Failed(format!("cannot read {name}: {e}")), &ctx),
        }
    });
}
