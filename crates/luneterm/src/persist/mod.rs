//! Persistence (plan M10 step 5, D5, D17): `settings.json` and `workspace.json` in the platform
//! config directory, read through the toolkit's bounded JSON reader and written by [`json_write`].
//!
//! The rules, all fail-closed:
//!  - the directory is `dirs::config_dir()/Luneterm`, created `0700` on Unix; a file is created
//!    `0600` with `create_new` under a unique temporary name in the same directory, synced, then
//!    renamed over the target ([`write_atomic`]); a symlink or any file already at the temporary
//!    name is never followed or touched;
//!  - a missing file is the defaults and writes nothing;
//!  - a file that cannot be read as a valid document of a version this build knows (not JSON, over
//!    the reader's limits, not an object, no integer `version`, an older or a newer one) is copied
//!    to `<name>.bak` (or `.bak.1`, `.bak.2`, ...: an existing backup is never overwritten), the
//!    defaults run, and the file is not rewritten until the user changes something;
//!  - a file over the size cap, or one that is not a regular file, is not read at all and not
//!    saved over for the rest of the session.
//!
//! Nothing read from these files is executed. The one value that names a program, `shell` in
//! `settings.json`, is the user's own setting, applied by [`crate::spawn`] as before; the workspace
//! holds directories and titles only, and each directory is checked ([`crate::cwd::validated`])
//! before a tab starts in it.

pub mod json_write;
pub mod settings_file;
pub mod workspace;

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lune_ui::json::{MAX_INPUT, Value};

pub use json_write::{Json, WriteError};
pub use settings_file::{SETTINGS_VERSION, SettingsDoc, parse_settings, render_settings};
pub use workspace::{
    MAX_TABS, SaveGuard, TabInfo, TabState, WORKSPACE_VERSION, Workspace, parse_workspace,
    render_workspace, restore_dirs, snapshot,
};

pub const SETTINGS_FILE: &str = "settings.json";
pub const WORKSPACE_FILE: &str = "workspace.json";

/// The most a backup copies: a file past this is not kept, and not saved over.
pub const MAX_BACKUP_BYTES: u64 = 64 << 20;
/// How many `.bak.N` names are tried before giving up.
const MAX_BACKUPS: u32 = 1000;

/// Why a file is not used. Never carries any of its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Not JSON, or over one of the reader's limits.
    Unreadable,
    NotAnObject,
    /// No `version`, or one that is not an integer.
    MissingVersion,
    OldVersion(i64),
    NewerVersion(i64),
}

/// The integer `version` of an object, at most `current`.
pub fn read_version(members: &[(String, Value)], current: i64) -> Result<i64, Reject> {
    let version = members
        .iter()
        .find(|(k, _)| k == "version")
        .and_then(|(_, v)| v.as_int().ok())
        .ok_or(Reject::MissingVersion)?;
    if version > current {
        Err(Reject::NewerVersion(version))
    } else if version < 1 {
        Err(Reject::OldVersion(version))
    } else {
        Ok(version)
    }
}

// ------------------------------------------------------------------------------- the store

/// A config directory and the files in it.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

/// What loading a file found.
#[derive(Debug, PartialEq)]
pub enum Loaded<T> {
    /// No such file: the defaults, and nothing is written.
    Missing,
    Ok(T),
    /// The file is not usable: the defaults run. `backup` is where a copy was kept (`None` when
    /// none could be); `save_blocked` says the file must not be written this session (a copy
    /// could not be kept, or it was never read).
    Rejected {
        reason: Reject,
        backup: Option<PathBuf>,
        save_blocked: bool,
    },
}

/// What reading a file's bytes found.
#[derive(Debug)]
enum Bytes {
    Missing,
    Data(Vec<u8>),
    /// Over the reader's input limit: not read.
    TooLarge,
    /// A directory, a pipe, a device: not opened.
    NotAFile,
}

impl Store {
    /// The platform config directory of the app, `None` when the OS has none.
    pub fn default_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("Luneterm"))
    }

    /// A store over `dir` (nothing is touched until a file is read or written).
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Reads `name` and parses it with `parse`; a file `parse` refuses is kept as a backup.
    pub fn load<T>(&self, name: &str, parse: impl FnOnce(&[u8]) -> Result<T, Reject>) -> Loaded<T> {
        let path = self.path(name);
        match read_bounded(&path) {
            Ok(Bytes::Missing) => Loaded::Missing,
            Ok(Bytes::Data(bytes)) => match parse(&bytes) {
                Ok(v) => Loaded::Ok(v),
                Err(reason) => self.rejected(&path, reason),
            },
            Ok(Bytes::TooLarge) => self.rejected(&path, Reject::Unreadable),
            Ok(Bytes::NotAFile) => Loaded::Rejected {
                reason: Reject::Unreadable,
                backup: None,
                save_blocked: true,
            },
            // Present but unreadable (permissions): do not write over it either.
            Err(_) => Loaded::Rejected {
                reason: Reject::Unreadable,
                backup: None,
                save_blocked: true,
            },
        }
    }

    fn rejected<T>(&self, path: &Path, reason: Reject) -> Loaded<T> {
        match quarantine(path) {
            Ok(backup) => Loaded::Rejected {
                reason,
                backup: Some(backup),
                save_blocked: false,
            },
            Err(_) => Loaded::Rejected {
                reason,
                backup: None,
                save_blocked: true,
            },
        }
    }

    /// Writes `bytes` as `name`, atomically, creating the directory if needed.
    pub fn save(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        write_atomic(&self.dir, name, bytes)
    }
}

/// Reads at most [`MAX_INPUT`] bytes of a regular file.
fn read_bounded(path: &Path) -> io::Result<Bytes> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Bytes::Missing),
        Err(e) => return Err(e),
    };
    if !meta.is_file() {
        return Ok(Bytes::NotAFile);
    }
    if meta.len() > MAX_INPUT as u64 {
        return Ok(Bytes::TooLarge);
    }
    let mut data = Vec::new();
    // One byte past the limit: a file that grew since the metadata is still refused.
    File::open(path)?
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() > MAX_INPUT {
        return Ok(Bytes::TooLarge);
    }
    Ok(Bytes::Data(data))
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Copies `path` to `<name>.bak`, or to the first of `<name>.bak.1`, `.bak.2`, ... that does not
/// exist: a backup is never overwritten. Returns where the copy is.
pub fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(io::ErrorKind::InvalidInput)?;
    let meta = fs::metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_BACKUP_BYTES {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    for n in 0..MAX_BACKUPS {
        let backup = if n == 0 {
            dir.join(format!("{name}.bak"))
        } else {
            dir.join(format!("{name}.bak.{n}"))
        };
        let mut out = match private_options().open(&backup) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let copied = File::open(path)
            .and_then(|src| io::copy(&mut src.take(MAX_BACKUP_BYTES + 1), &mut out))
            .and_then(|n| {
                if n > MAX_BACKUP_BYTES {
                    Err(io::ErrorKind::InvalidInput.into())
                } else {
                    out.sync_all()
                }
            });
        if let Err(e) = copied {
            // The copy is ours (created just now): remove the part of it.
            drop(out);
            let _ = fs::remove_file(&backup);
            return Err(e);
        }
        return Ok(backup);
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary name no other writer is likely to pick: this process, the clock, a counter.
fn temp_name(target: &str, attempt: u32) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".{target}.{}.{nanos}.{n}.{attempt}.tmp", std::process::id())
}

fn create_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Writes `bytes` as `dir/target`: a new file under a temporary name in `dir`, written and
/// synced, then renamed over the target. A name that already exists (a stale file, a symlink
/// planted at the guess) is never opened: the next name is tried.
pub fn write_atomic(dir: &Path, target: &str, bytes: &[u8]) -> io::Result<()> {
    write_atomic_named(dir, target, bytes, &mut |attempt| {
        temp_name(target, attempt)
    })
}

/// [`write_atomic`] with the temporary names chosen by `names` (tests plant files at them).
pub fn write_atomic_named(
    dir: &Path,
    target: &str,
    bytes: &[u8],
    names: &mut dyn FnMut(u32) -> String,
) -> io::Result<()> {
    create_dir(dir)?;
    let mut created = None;
    for attempt in 0..8 {
        let tmp = dir.join(names(attempt));
        match private_options().open(&tmp) {
            Ok(file) => {
                created = Some((tmp, file));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let Some((tmp, mut file)) = created else {
        return Err(io::ErrorKind::AlreadyExists.into());
    };
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            fs::rename(&tmp, dir.join(target))
        });
    if result.is_err() {
        // The temporary file is ours: it is removed, the target stands.
        let _ = fs::remove_file(&tmp);
    } else {
        // Best effort: make the rename itself durable.
        #[cfg(unix)]
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    result
}

// ---------------------------------------------------------------------------- the persister

enum Job {
    Write { name: &'static str, bytes: Vec<u8> },
    Flush(Sender<()>),
}

/// Writes files on a thread of its own, so the UI never waits on a disk. The latest text of a
/// file wins: while the thread is busy, older queued texts of the same file are dropped.
#[derive(Debug)]
pub struct Persister {
    tx: Sender<Job>,
    handle: Option<JoinHandle<()>>,
}

impl Persister {
    pub fn start(store: Store) -> io::Result<Persister> {
        let (tx, rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("luneterm-persist".into())
            .spawn(move || persist_loop(&store, &rx))?;
        Ok(Persister {
            tx,
            handle: Some(handle),
        })
    }

    /// Queues `bytes` to be saved as `name`.
    pub fn submit(&self, name: &'static str, bytes: Vec<u8>) {
        let _ = self.tx.send(Job::Write { name, bytes });
    }

    /// Waits up to `timeout` for everything queued so far to be written. `true` if it was.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (ack, done) = mpsc::channel();
        self.tx.send(Job::Flush(ack)).is_ok() && done.recv_timeout(timeout).is_ok()
    }
}

impl Drop for Persister {
    fn drop(&mut self) {
        // The channel closes with `tx`; the thread writes what is queued and ends. It is not
        // joined: a disk that hangs must not hang the app's exit.
        drop(self.handle.take());
    }
}

fn persist_loop(store: &Store, rx: &Receiver<Job>) {
    while let Ok(first) = rx.recv() {
        let mut latest: Vec<(&'static str, Vec<u8>)> = Vec::new();
        let mut acks = Vec::new();
        let mut take = |job: Job| match job {
            Job::Write { name, bytes } => {
                latest.retain(|(n, _)| *n != name);
                latest.push((name, bytes));
            }
            Job::Flush(ack) => acks.push(ack),
        };
        take(first);
        while let Ok(job) = rx.try_recv() {
            take(job);
        }
        for (name, bytes) in latest {
            if let Err(e) = store.save(name, &bytes) {
                // The kind only: never a path or content.
                eprintln!("luneterm: could not save {name}: {:?}", e.kind());
            }
        }
        for ack in acks {
            let _ = ack.send(());
        }
    }
}

// ------------------------------------------------------------------------------ the app's part

/// What the app has of persistence: the store, the writer thread, what the files held that the app
/// does not know, and whether each file may be written.
#[derive(Debug)]
pub struct Persist {
    persister: Option<Persister>,
    settings_unknown: Vec<(String, Json)>,
    workspace_unknown: Vec<(String, Json)>,
    settings_blocked: bool,
    workspace_blocked: bool,
    /// The workspace file was rejected: it is not rewritten until the user touches the tabs.
    workspace_armed: bool,
}

/// What a start found: the settings, the workspace to restore (if any), and what the user should be
/// told.
#[derive(Debug)]
pub struct Startup {
    pub settings: Settings,
    pub workspace: Option<Workspace>,
    pub notices: Vec<String>,
}

use crate::settings::Settings;

impl Persist {
    /// Persistence with nothing behind it (no config directory): nothing is read or written.
    pub fn none() -> Persist {
        Persist {
            persister: None,
            settings_unknown: Vec::new(),
            workspace_unknown: Vec::new(),
            settings_blocked: true,
            workspace_blocked: true,
            workspace_armed: false,
        }
    }

    /// Reads both files from `store`. Nothing is written: a missing file stays missing, a rejected
    /// one is copied to a backup and left as it is.
    pub fn load(store: Store) -> (Persist, Startup) {
        let mut notices = Vec::new();
        let mut persist = Persist {
            persister: None,
            settings_unknown: Vec::new(),
            workspace_unknown: Vec::new(),
            settings_blocked: false,
            workspace_blocked: false,
            workspace_armed: true,
        };
        let settings = match store.load(SETTINGS_FILE, parse_settings) {
            Loaded::Missing => Settings::default(),
            Loaded::Ok(doc) => {
                persist.settings_unknown = doc.unknown;
                doc.settings
            }
            Loaded::Rejected {
                backup,
                save_blocked,
                ..
            } => {
                persist.settings_blocked = save_blocked;
                notices.push(rejected_notice("The settings file", backup.is_some()));
                Settings::default()
            }
        };
        let workspace = match store.load(WORKSPACE_FILE, parse_workspace) {
            Loaded::Missing => None,
            Loaded::Ok(ws) => {
                persist.workspace_unknown.clone_from(&ws.unknown);
                (!ws.tabs.is_empty()).then_some(ws)
            }
            Loaded::Rejected {
                backup,
                save_blocked,
                ..
            } => {
                persist.workspace_blocked = save_blocked;
                persist.workspace_armed = false;
                notices.push(rejected_notice("The saved tabs", backup.is_some()));
                None
            }
        };
        persist.persister = match Persister::start(store) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("luneterm: persistence off: {:?}", e.kind());
                None
            }
        };
        (
            persist,
            Startup {
                settings,
                workspace,
                notices,
            },
        )
    }

    /// Saves the settings (the user changed something). Does nothing when the file is blocked.
    pub fn save_settings(&self, settings: &Settings) {
        if self.settings_blocked {
            return;
        }
        let doc = SettingsDoc {
            settings: settings.clone(),
            unknown: self.settings_unknown.clone(),
        };
        match (render_settings(&doc), &self.persister) {
            (Ok(text), Some(p)) => p.submit(SETTINGS_FILE, text.into_bytes()),
            (Err(e), _) => eprintln!("luneterm: settings not saved: {e:?}"),
            _ => {}
        }
    }

    /// The unknown top-level members the workspace file had, to carry through a snapshot.
    pub fn workspace_unknown(&self) -> &[(String, Json)] {
        &self.workspace_unknown
    }

    /// The user acted on the tabs: a rejected workspace file may be rewritten now.
    pub fn arm_workspace(&mut self) {
        self.workspace_armed = true;
    }

    /// Whether a workspace write would be allowed by the file's own state.
    pub fn workspace_writable(&self) -> bool {
        self.workspace_armed && !self.workspace_blocked
    }

    /// Saves the workspace. Does nothing while the file is blocked or not yet armed.
    pub fn save_workspace(&self, workspace: &Workspace) {
        if !self.workspace_writable() {
            return;
        }
        match (render_workspace(workspace), &self.persister) {
            (Ok(text), Some(p)) => p.submit(WORKSPACE_FILE, text.into_bytes()),
            (Err(e), _) => eprintln!("luneterm: workspace not saved: {e:?}"),
            _ => {}
        }
    }

    /// Waits up to `timeout` for the queued writes (at exit).
    pub fn flush(&self, timeout: Duration) -> bool {
        self.persister.as_ref().is_none_or(|p| p.flush(timeout))
    }
}

fn rejected_notice(what: &str, kept: bool) -> String {
    if kept {
        format!("{what} could not be read. A copy was kept next to it and the defaults are used.")
    } else {
        format!("{what} could not be read. The defaults are used and the file is left alone.")
    }
}

#[cfg(test)]
mod tests;
