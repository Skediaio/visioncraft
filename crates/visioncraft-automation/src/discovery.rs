//! How a bridge finds a running window that allows AI agents, with nothing on its command line.
//!
//! VisionCraft's state folder (`$VISIONCRAFT_STATE_DIR`, else `visioncraft` in the per-user state
//! directory: `$XDG_STATE_HOME` or `~/.local/state` on Linux) holds:
//!
//! - `control.token`: the default control token (64 hexadecimal characters, mode 0600);
//! - `windows/<pid>.json`: one entry per listening window, `{pid, port, tokenFile, focusedAt,
//!   startedAt}`, times in milliseconds since the Unix epoch. No secret.
//!
//! Ported from 3dcraft `crates/automation/src/discovery.rs` (MIT), without the agents
//! preference file (the window's switch is not built yet). Every file here is read as hostile
//! input: size-capped, a bad one skipped, never a panic.

use serde_json::{Value, json};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const TOKEN_FILE: &str = "control.token";
pub const WINDOWS_DIR: &str = "windows";
/// The ports a window tries, 7920 to 7939: next to 3DCraft's 7878 to 7897, so both run at once.
pub const FIRST_PORT: u16 = 7920;
pub const PORT_TRIES: u16 = 20;

const MAX_FILE_BYTES: u64 = 4096;
const MAX_ENTRIES: usize = 256;

/// VisionCraft's state folder, from the environment `env` reads.
pub fn default_state_folder(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("VISIONCRAFT_STATE_DIR") {
        return Some(dir);
    }
    let base = if cfg!(target_os = "windows") {
        set("LOCALAPPDATA")?
    } else if cfg!(target_os = "macos") {
        set("HOME")?.join("Library/Application Support")
    } else {
        set("XDG_STATE_HOME").or_else(|| set("HOME").map(|h| h.join(".local/state")))?
    };
    Some(base.join("visioncraft"))
}

/// [`default_state_folder`] of this process's environment.
pub fn state_folder() -> Option<PathBuf> {
    default_state_folder(&|k| std::env::var(k).ok())
}

pub fn default_token_file(folder: &Path) -> PathBuf {
    folder.join(TOKEN_FILE)
}

/// One window that listens for agents, as its entry says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowEntry {
    pub pid: u32,
    pub port: u16,
    /// Absolute.
    pub token_file: PathBuf,
    pub focused_at: u64,
    pub started_at: u64,
}

impl WindowEntry {
    pub fn to_json(&self) -> Value {
        json!({
            "pid": self.pid, "port": self.port, "tokenFile": self.token_file.to_string_lossy(),
            "focusedAt": self.focused_at, "startedAt": self.started_at,
        })
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
        let port = u16::try_from(v.get("port")?.as_u64()?).ok()?;
        let token_file = PathBuf::from(v.get("tokenFile")?.as_str()?);
        if port == 0 || !token_file.is_absolute() {
            return None;
        }
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Some(Self {
            pid,
            port,
            token_file,
            focused_at: n("focusedAt"),
            started_at: n("startedAt"),
        })
    }

    pub fn describe(&self) -> String {
        format!("VisionCraft (process {}) on port {}", self.pid, self.port)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn read_capped(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

static TEMPORARIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Writes `text` to `path` whole: a new private temporary file beside it, renamed over it.
/// With `keep`, an existing `path` is left as it is (two windows agree on the first token).
pub fn write_atomically(path: &Path, text: &str, keep: bool) -> Result<(), String> {
    use std::io::Write;
    let shown = path.display();
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| format!("`{shown}` has no folder"))?;
    make_private_dir(parent).map_err(|e| format!("cannot make the folder of `{shown}`: {e}"))?;
    let count = TEMPORARIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}-{count}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&temporary).and_then(|mut f| {
        f.write_all(text.as_bytes())?;
        f.sync_all()
    });
    let linked = written.and_then(|()| {
        if keep {
            // A hard link fails when the name exists: the first writer wins.
            match std::fs::hard_link(&temporary, path) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                other => other,
            }
        } else {
            std::fs::rename(&temporary, path)
        }
    });
    let _ = std::fs::remove_file(&temporary);
    linked.map_err(|e| format!("cannot write `{shown}`: {e}"))
}

pub fn entry_path(folder: &Path, pid: u32) -> PathBuf {
    folder.join(WINDOWS_DIR).join(format!("{pid}.json"))
}

pub fn write_entry(folder: &Path, entry: &WindowEntry) -> Result<(), String> {
    write_atomically(
        &entry_path(folder, entry.pid),
        &entry.to_json().to_string(),
        false,
    )
}

pub fn remove_entry(folder: &Path, pid: u32) {
    let _ = std::fs::remove_file(entry_path(folder, pid));
}

/// The entries in `folder`, by process id; files that are not whole entries are skipped.
pub fn read_entries(folder: &Path) -> Vec<WindowEntry> {
    let Ok(dir) = std::fs::read_dir(folder.join(WINDOWS_DIR)) else {
        return Vec::new();
    };
    let mut entries: Vec<WindowEntry> = dir
        .filter_map(Result::ok)
        .take(MAX_ENTRIES)
        .filter_map(|item| {
            let path = item.path();
            let pid: u32 = path
                .file_name()?
                .to_str()?
                .strip_suffix(".json")?
                .parse()
                .ok()?;
            let entry = WindowEntry::from_json(&read_capped(&path)?)?;
            (entry.pid == pid).then_some(entry)
        })
        .collect();
    entries.sort_by_key(|e| e.pid);
    entries
}

fn process_may_run(pid: u32) -> bool {
    if cfg!(target_os = "linux") && Path::new("/proc/self").exists() {
        return Path::new("/proc").join(pid.to_string()).exists();
    }
    true
}

fn port_answers(port: u16, timeout: Duration) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), timeout).is_ok()
}

/// On Linux, whether the socket listening on 127.0.0.1:`port` is process `pid`'s and nothing
/// else could take 127.0.0.1 connections on that port; `None` where it cannot be told.
pub fn listener_belongs_to(pid: u32, port: u16) -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let table = std::fs::read_to_string("/proc/net/tcp").ok()?;
    let listening = |table: &str| -> Vec<(String, String)> {
        table
            .lines()
            .skip(1)
            .filter_map(|line| {
                let f: Vec<&str> = line.split_whitespace().collect();
                let (local, state, node) = (f.get(1)?, f.get(3)?, f.get(9)?);
                let (address, at) = local.rsplit_once(':')?;
                let at = u16::from_str_radix(at, 16).ok()?;
                (*state == "0A" && at == port)
                    .then(|| (address.to_ascii_uppercase(), node.to_string()))
            })
            .collect()
    };
    let mut inode = None;
    for (address, node) in listening(&table) {
        match address.as_str() {
            "0100007F" => inode = Some(node),
            "00000000" => return Some(false),
            _ => {}
        }
    }
    if let Ok(table6) = std::fs::read_to_string("/proc/net/tcp6") {
        for (address, _) in listening(&table6) {
            if address == "00000000000000000000000000000000"
                || address == "0000000000000000FFFF00000100007F"
            {
                return Some(false);
            }
        }
    }
    let Some(inode) = inode else {
        return Some(false);
    };
    let socket = format!("socket:[{inode}]");
    let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Some(false);
    };
    Some(
        fds.filter_map(Result::ok).take(65_536).any(|fd| {
            std::fs::read_link(fd.path()).is_ok_and(|t| t.as_os_str() == socket.as_str())
        }),
    )
}

pub fn is_live(entry: &WindowEntry) -> bool {
    process_may_run(entry.pid)
        && port_answers(entry.port, Duration::from_millis(500))
        && listener_belongs_to(entry.pid, entry.port) != Some(false)
}

/// Refuses a token file other users could read or change, or that is not this user's.
pub fn check_private(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let shown = path.display();
        let meta = std::fs::metadata(path)
            .map_err(|e| format!("cannot read the control token file `{shown}`: {e}"))?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!(
                "the control token file `{shown}` can be read or changed by other users (mode {mode:o}); make it private with `chmod 600` and try again"
            ));
        }
        let me = std::fs::metadata("/proc/self")
            .map(|m| m.uid())
            .ok()
            .or_else(|| {
                std::env::var_os("HOME")
                    .and_then(|h| std::fs::metadata(h).ok())
                    .map(|m| m.uid())
            });
        if me.is_some_and(|me| meta.uid() != me) {
            return Err(format!(
                "the control token file `{shown}` belongs to another user"
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The entries of windows that still listen; the others' files are removed.
pub fn live_entries(folder: &Path, except: Option<u32>) -> Vec<WindowEntry> {
    read_entries(folder)
        .into_iter()
        .filter(|e| {
            let live = Some(e.pid) == except || is_live(e);
            if !live {
                remove_entry(folder, e.pid);
            }
            live
        })
        .collect()
}

/// Which window a bridge talks to: the only one (on `port` when given), else the one focused
/// most recently; none, or several equally recent, is an error that names them.
pub fn choose(live: &[WindowEntry], port: Option<u16>) -> Result<WindowEntry, String> {
    let mut sorted: Vec<&WindowEntry> = live
        .iter()
        .filter(|e| port.is_none_or(|p| e.port == p))
        .collect();
    sorted.sort_by_key(|e| std::cmp::Reverse((e.focused_at, e.started_at)));
    match sorted.as_slice() {
        [] => Err(match port {
            Some(port) => format!("no VisionCraft window that allows AI agents listens on port {port}; open VisionCraft and allow AI agents"),
            None => "no VisionCraft window allows AI agents right now; open VisionCraft and allow AI agents".to_string(),
        }),
        [only] => Ok((*only).clone()),
        [first, second, ..] if first.focused_at > second.focused_at => Ok((*first).clone()),
        several => {
            let names: Vec<String> = several.iter().map(|e| e.describe()).collect();
            Err(format!(
                "several VisionCraft windows allow AI agents and none was used last: {}; click into the window the agent should use and try again, or give its port (`visioncraft-cli mcp --bridge PORT`)",
                names.join(", ")
            ))
        }
    }
}

/// Reads a control token: 64 hexadecimal characters. The error never shows what the file holds.
pub fn read_token(path: &Path) -> Result<String, String> {
    let shown = path.display();
    let text = read_capped(path).ok_or_else(|| {
        format!(
            "cannot read the control token file `{shown}`: it is missing, not a file or too large"
        )
    })?;
    let token = text.trim();
    if token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
        check_private(path)?;
        Ok(token.to_ascii_lowercase())
    } else {
        Err(format!(
            "`{shown}` does not hold a control token (64 hexadecimal characters)"
        ))
    }
}

/// The token in `path`, made (fresh, private) when the file does not exist.
pub fn ensure_token(path: &Path) -> Result<String, String> {
    if !path.exists() {
        write_atomically(path, &crate::proof::nonce()?, true)?;
    }
    read_token(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, port: u16, focused_at: u64) -> WindowEntry {
        WindowEntry {
            pid,
            port,
            token_file: PathBuf::from("/tmp/t"),
            focused_at,
            started_at: 1,
        }
    }

    #[test]
    fn entries_round_trip_and_the_last_focused_window_is_chosen() {
        let e = entry(42, 7920, 5);
        assert_eq!(
            WindowEntry::from_json(&e.to_json().to_string()),
            Some(e.clone())
        );
        assert!(WindowEntry::from_json(r#"{"pid": 1, "port": 0, "tokenFile": "/t"}"#).is_none());
        let two = [entry(1, 7920, 5), entry(2, 7921, 9)];
        assert_eq!(choose(&two, None).unwrap().pid, 2);
        assert_eq!(choose(&two, Some(7920)).unwrap().pid, 1);
        assert!(
            choose(&[entry(1, 7920, 0), entry(2, 7921, 0)], None)
                .unwrap_err()
                .contains("several")
        );
        assert!(choose(&[], None).is_err());
    }

    #[test]
    fn state_folder_follows_the_environment() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        if cfg!(target_os = "linux") {
            assert_eq!(
                default_state_folder(&env),
                Some(PathBuf::from("/home/u/.local/state/visioncraft"))
            );
        }
        let set = |k: &str| (k == "VISIONCRAFT_STATE_DIR").then(|| "/s".to_string());
        assert_eq!(default_state_folder(&set), Some(PathBuf::from("/s")));
    }
}
