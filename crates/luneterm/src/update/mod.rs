//! Self-update through lunepack (auto-update plan step 3).
//!
//! [`startup`] runs first thing in `main` (lunepack's lifecycle: a rolled-back version exits at
//! once). [`Updater`] owns one worker thread that runs lunepack's blocking API; the UI thread
//! sends it commands and gets every state as an [`Event`] through a sink (the app posts them as
//! `UserEvent`s). Rules the code keeps:
//!
//! - **No network unless asked.** Nothing reaches the feed but a [`Updater::check`] or a
//!   [`Updater::download`]; a copy lunepack did not install (a `cargo run`, an old portable zip)
//!   has no worker at all and reports [`UpdateState::Unavailable`].
//! - **One operation at a time.** A command sent while one runs is dropped, not queued
//!   ([`Updater::check`] and the others return `false`).
//! - **Cancel on quit.** [`Updater::shutdown`] cancels the operation in flight and waits for the
//!   worker at most the time it is given; the worker never blocks on the UI.
//! - **Errors are text.** A failure is [`UpdateState::Failed`] with lunepack's message, any URL in
//!   it cut to scheme, host and path (no query, fragment or user info); nothing here panics.
//! - **Trusted root.** The update config of an installed copy must name exactly the root compiled
//!   in from [`root`], or the copy reports [`Unavailable::ForeignRoot`] and never checks.

pub mod auto;
pub mod channel;
pub mod root;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use lunepack::{
    ApplyOptions, CancelToken, DownloadOptions, Error, Installation, LifecycleEvent, Phase,
    Progress, StartupAction, UiChoice, UpdateInfo, Version, blocking,
};

/// Download progress reaches the UI at most this often (the last value always does).
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// The longest error text shown.
const MAX_ERROR_CHARS: usize = 240;

/// Where the update machinery is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    /// This copy cannot update itself.
    Unavailable(Unavailable),
    /// Nothing done yet.
    Idle,
    /// A check is running.
    Checking,
    /// The last check found nothing newer.
    UpToDate,
    /// The last check found this version; `auto` says an automatic check found it (the UI shows a
    /// toast only then).
    Available { version: String, auto: bool },
    /// Downloading, in percent of the download (`0..=100`).
    Downloading(u8),
    /// This version is downloaded and staged: a restart applies it.
    Ready(String),
    /// The last operation failed; the text is safe to show.
    Failed(String),
}

/// Why a copy cannot update itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// Not installed by lunepack's installer (no update config beside the executable).
    NotInstalled,
    /// The update config names another root of trust than the one compiled in.
    ForeignRoot,
    /// The install could not be read; the text is safe to show.
    Broken(String),
}

/// What the worker tells the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The state changed.
    State(UpdateState),
    /// The runtime will apply the staged version once this process exits: quit now, through the
    /// normal path (the workspace is saved).
    Restart,
}

/// What [`startup`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    exit: bool,
    pending: bool,
    fresh_install: bool,
    installed: Result<(), Unavailable>,
}

impl Launch {
    /// A launch that found no install (the frame dump, tests).
    pub fn not_installed() -> Launch {
        Launch {
            exit: false,
            pending: false,
            fresh_install: false,
            installed: Err(Unavailable::NotInstalled),
        }
    }

    /// This version was rolled back and the previous one started in its place: exit now.
    pub fn exit(&self) -> bool {
        self.exit
    }

    /// This copy was installed by lunepack (its update config was found).
    pub fn is_installed(&self) -> bool {
        self.installed.is_ok()
    }

    /// This version waits for [`confirm_healthy`] (it was just updated to).
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// This is the first start after a setup file installed this copy (lunepack delivered its
    /// `Installed` event, once): a per-user first install, or a reinstall over a record whose
    /// tree was removed. Never after an update. A per-machine install queues no events, so it is
    /// never seen as fresh.
    pub fn fresh_install(&self) -> bool {
        self.fresh_install
    }
}

/// Whether lunepack's lifecycle events for this start say a setup file just installed it.
pub(crate) fn installed_fresh(events: &[LifecycleEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, LifecycleEvent::Installed { .. }))
}

/// lunepack's start of the running app: rolls back a version that failed its health check
/// (the result then says [`Launch::exit`]) and finds out whether this copy is installed. Local
/// files only; never the network.
pub fn startup() -> Launch {
    match lunepack::startup() {
        Ok(started) => Launch {
            exit: started.action == StartupAction::Exit,
            pending: started.pending,
            fresh_install: installed_fresh(&started.events),
            installed: Ok(()),
        },
        Err(Error::NoUpdateConfig) => Launch::not_installed(),
        Err(e) => {
            let text = error_text(&e);
            eprintln!("luneterm: update startup: {text}");
            Launch {
                exit: false,
                pending: false,
                fresh_install: false,
                installed: Err(Unavailable::Broken(text)),
            }
        }
    }
}

/// Confirms that this version works (plan D8: after the first frame is presented and the first
/// shell spawned), so lunepack does not roll it back. A failure is logged, never fatal.
pub fn confirm_healthy() {
    if let Err(e) = lunepack::confirm_healthy() {
        eprintln!("luneterm: update health confirm: {}", error_text(&e));
    }
}

/// Opens the running copy's updater: its install, the root check and lunepack's updater, following
/// `channel` (the persisted one, see [`channel`]) when given. Local files only; runs on the
/// worker.
fn open_installed(channel: Option<String>) -> Result<blocking::Updater, Unavailable> {
    let inst = Installation::from_current_exe().map_err(|e| match e {
        Error::NoUpdateConfig => Unavailable::NotInstalled,
        e => Unavailable::Broken(error_text(&e)),
    })?;
    let config = inst.config();
    let keys: Vec<[u8; 32]> = config.root.keys.iter().map(|k| *k.as_bytes()).collect();
    if !root::matches(
        inst.app_id().as_str(),
        config.root.version,
        config.root.threshold,
        &keys,
    ) {
        return Err(Unavailable::ForeignRoot);
    }
    inst.updater_builder()
        .map(|b| channel::follow(b, channel.as_deref()))
        .and_then(|b| b.build())
        .map(blocking::Updater::from)
        .map_err(|e| Unavailable::Broken(error_text(&e)))
}

/// The UI's sink for the worker's events.
type Sink = Arc<Mutex<Box<dyn FnMut(Event) + Send>>>;
/// How the worker gets its updater.
type Open = Box<dyn FnOnce() -> Result<blocking::Updater, Unavailable> + Send>;

enum Cmd {
    Check { auto: bool, cancel: CancelToken },
    Download { cancel: CancelToken },
    Restart,
}

/// What the UI thread and the worker share.
#[derive(Debug, Default)]
struct Shared {
    /// A command was accepted and has not finished. Set by the UI thread when it accepts one,
    /// cleared by the worker before it posts the command's last state.
    busy: AtomicBool,
    /// The token of the operation in flight, so the UI thread can cancel while the worker is
    /// blocked inside lunepack. Set by the UI thread before it sends the command.
    current: Mutex<Option<CancelToken>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The handle the UI thread holds: commands in, events out through the sink.
pub struct Updater {
    initial: UpdateState,
    tx: Option<mpsc::Sender<Cmd>>,
    shared: Arc<Shared>,
    done: Option<mpsc::Receiver<()>>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Updater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updater")
            .field("initial", &self.initial)
            .field("running", &self.tx.is_some())
            .field("busy", &self.shared.busy.load(Ordering::Acquire))
            .finish()
    }
}

impl Updater {
    /// A handle with no worker: every command is ignored and nothing touches the network.
    pub fn unavailable(why: Unavailable) -> Updater {
        Updater {
            initial: UpdateState::Unavailable(why),
            tx: None,
            shared: Arc::default(),
            done: None,
            thread: None,
        }
    }

    /// The updater of the running copy as `launch` found it: a worker when it is installed,
    /// [`Updater::unavailable`] otherwise. `channel` is the channel to follow ([`channel::settle`]);
    /// `None` keeps the one the installed tree names. `sink` gets every event, on the worker
    /// thread.
    pub fn spawn(
        launch: &Launch,
        channel: Option<String>,
        sink: impl FnMut(Event) + Send + 'static,
    ) -> Updater {
        match &launch.installed {
            Ok(()) => Updater::start(Box::new(move || open_installed(channel)), Box::new(sink)),
            Err(why) => Updater::unavailable(why.clone()),
        }
    }

    /// A worker over the updater `open` gives (tests give one built over a fixture feed).
    fn start(open: Open, sink: Box<dyn FnMut(Event) + Send>) -> Updater {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let sink: Sink = Arc::new(Mutex::new(sink));
        let spawned = thread::Builder::new()
            .name("luneterm-update".into())
            .spawn(move || {
                Worker::run(open, rx, worker_shared, sink);
                let _ = done_tx.send(());
            });
        match spawned {
            Ok(thread) => Updater {
                initial: UpdateState::Idle,
                tx: Some(tx),
                shared,
                done: Some(done),
                thread: Some(thread),
            },
            Err(_) => Updater::unavailable(Unavailable::Broken(
                "the update thread could not be started".to_owned(),
            )),
        }
    }

    /// The state before any event arrives.
    pub fn initial_state(&self) -> UpdateState {
        self.initial.clone()
    }

    /// Checks the feed; `auto` marks an automatic check. `false` when ignored (no worker, or an
    /// operation runs).
    pub fn check(&self, auto: bool) -> bool {
        self.send(|cancel| Cmd::Check { auto, cancel })
    }

    /// Downloads and stages the update the last check found. `false` when ignored.
    pub fn download(&self) -> bool {
        self.send(|cancel| Cmd::Download { cancel })
    }

    /// Hands the staged version to lunepack's runtime; on success the worker sends
    /// [`Event::Restart`] and the app quits. `false` when ignored.
    pub fn restart(&self) -> bool {
        self.send(|_| Cmd::Restart)
    }

    /// Cancels the operation in flight, if any.
    pub fn cancel(&self) {
        if let Some(token) = lock(&self.shared.current).as_ref() {
            token.cancel();
        }
    }

    /// Cancels the operation in flight, stops the worker and waits for it at most `wait`.
    /// `true` when the worker ended in time (it is detached otherwise). Further commands are
    /// ignored.
    pub fn shutdown(&mut self, wait: Duration) -> bool {
        self.cancel();
        self.tx = None;
        let Some(done) = self.done.take() else {
            return true;
        };
        let ended = !matches!(
            done.recv_timeout(wait),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        if let Some(thread) = self.thread.take()
            && ended
        {
            let _ = thread.join();
        }
        ended
    }

    fn send(&self, make: impl FnOnce(CancelToken) -> Cmd) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        if self
            .shared
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let token = CancelToken::new();
        *lock(&self.shared.current) = Some(token.clone());
        if tx.send(make(token)).is_err() {
            *lock(&self.shared.current) = None;
            self.shared.busy.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

impl Drop for Updater {
    /// Cancels and lets the worker end on its own (no wait).
    fn drop(&mut self) {
        self.cancel();
        self.tx = None;
    }
}

/// The worker thread's state.
struct Worker {
    updater: Result<blocking::Updater, Unavailable>,
    shared: Arc<Shared>,
    sink: Sink,
    state: UpdateState,
    /// The update the last check found.
    info: Option<UpdateInfo>,
    /// The version staged for the next restart.
    staged: Option<Version>,
}

impl Worker {
    fn run(open: Open, rx: mpsc::Receiver<Cmd>, shared: Arc<Shared>, sink: Sink) {
        let mut w = Worker {
            updater: open(),
            shared,
            sink,
            state: UpdateState::Idle,
            info: None,
            staged: None,
        };
        // What a previous session left staged (local state only).
        let first = match &w.updater {
            Ok(up) => match up.staged() {
                Ok(Some(staged)) => {
                    let v = staged.version().clone();
                    w.staged = Some(v.clone());
                    Some(UpdateState::Ready(v.to_string()))
                }
                _ => None,
            },
            Err(why) => Some(UpdateState::Unavailable(why.clone())),
        };
        if let Some(state) = first {
            w.post(state);
        }
        while let Ok(cmd) = rx.recv() {
            let last = w.command(cmd);
            w.finish(last);
        }
    }

    fn post(&mut self, state: UpdateState) {
        self.state = state.clone();
        emit(&self.sink, Event::State(state));
    }

    /// Ends a command: frees the worker for the next one, then posts its last state (so the UI
    /// can send the next command as soon as it sees it). `None` re-posts the current state.
    fn finish(&mut self, last: Option<UpdateState>) {
        *lock(&self.shared.current) = None;
        self.shared.busy.store(false, Ordering::Release);
        let state = last.unwrap_or_else(|| self.state.clone());
        self.post(state);
    }

    /// Runs one command; returns its last state, or `None` when it changed nothing (the
    /// current state is posted again).
    fn command(&mut self, cmd: Cmd) -> Option<UpdateState> {
        let up = match &self.updater {
            Ok(up) => up.clone(),
            Err(_) => return None,
        };
        match cmd {
            Cmd::Check { auto, cancel } => self.check(&up, auto, &cancel),
            Cmd::Download { cancel } => self.download(&up, &cancel),
            Cmd::Restart => self.restart(),
        }
    }

    fn check(
        &mut self,
        up: &blocking::Updater,
        auto: bool,
        cancel: &CancelToken,
    ) -> Option<UpdateState> {
        // Already staged: a restart applies it; nothing to look for.
        if self.staged.is_some() || cancel.is_cancelled() {
            return None;
        }
        self.post(UpdateState::Checking);
        Some(match up.check(cancel) {
            Ok(None) => {
                self.info = None;
                UpdateState::UpToDate
            }
            Ok(Some(info)) => {
                let version = info.version_text().to_owned();
                self.info = Some(info);
                UpdateState::Available { version, auto }
            }
            Err(Error::Cancelled) => UpdateState::Idle,
            Err(e) => UpdateState::Failed(error_text(&e)),
        })
    }

    fn download(&mut self, up: &blocking::Updater, cancel: &CancelToken) -> Option<UpdateState> {
        if self.staged.is_some() || cancel.is_cancelled() {
            return None;
        }
        let info = self.info.take()?;
        let version = info.version_text().to_owned();
        self.post(UpdateState::Downloading(0));
        let options = DownloadOptions {
            progress: Some(progress_callback(Arc::clone(&self.sink))),
            ..DownloadOptions::default()
        };
        let result = up
            .download(&info, options, cancel)
            .and_then(|downloaded| up.stage(downloaded, cancel));
        Some(match result {
            Ok(staged) => {
                let v = staged.version().clone();
                self.staged = Some(v.clone());
                UpdateState::Ready(v.to_string())
            }
            Err(Error::Cancelled) => {
                // Still on offer: a later Download resumes the kept part.
                self.info = Some(info);
                UpdateState::Available {
                    version,
                    auto: false,
                }
            }
            Err(Error::Stale) => UpdateState::Idle,
            Err(e) => {
                self.info = Some(info);
                UpdateState::Failed(error_text(&e))
            }
        })
    }

    fn restart(&mut self) -> Option<UpdateState> {
        let version = self.staged.clone()?;
        let options = ApplyOptions {
            expect_version: Some(version),
            // Luneterm shows its own status; lunepack's window lingered after a macOS rollback
            // (plan revision 2).
            ui: UiChoice::Never,
            ..ApplyOptions::default()
        };
        match lunepack::apply_and_restart(options) {
            Ok(_) => {
                emit(&self.sink, Event::Restart);
                None
            }
            Err(e) => Some(UpdateState::Failed(error_text(&e))),
        }
    }
}

fn emit(sink: &Sink, event: Event) {
    (lock(sink))(event);
}

/// Posts the download phase's percentage, when it changed and at most every [`PROGRESS_EVERY`]
/// (100 always goes through). The worker already posted 0.
fn progress_callback(sink: Sink) -> lunepack::ProgressCallback {
    let mut last_pct: u8 = 0;
    let mut last_at: Option<Instant> = None;
    Box::new(move |p: &Progress| {
        if p.phase != Phase::Download || p.total == 0 {
            return;
        }
        let pct = (p.done.min(p.total).saturating_mul(100) / p.total) as u8;
        let now = Instant::now();
        let due = last_at.is_none_or(|t| now.duration_since(t) >= PROGRESS_EVERY) || pct == 100;
        if last_pct != pct && due {
            last_pct = pct;
            last_at = Some(now);
            emit(&sink, Event::State(UpdateState::Downloading(pct)));
        }
    })
}

/// An error as text that is safe to show: lunepack's message, every URL cut to scheme, host and
/// path, at most [`MAX_ERROR_CHARS`] characters.
fn error_text(e: &Error) -> String {
    redact(&e.to_string())
}

fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        // The URL starts at its scheme: the run of scheme characters before `://`.
        let scheme_start = rest[..i]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
            .map_or(0, |j| j + 1);
        out.push_str(&rest[..scheme_start]);
        let url = &rest[scheme_start..];
        let end = url
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`'))
            .unwrap_or(url.len());
        out.push_str(&clean_url(&url[..end]));
        rest = &url[end..];
    }
    out.push_str(rest);
    if out.chars().count() > MAX_ERROR_CHARS {
        out = out.chars().take(MAX_ERROR_CHARS).collect();
        out.push('…');
    }
    out
}

/// `scheme://[user@]host/path?query#fragment` -> `scheme://host/path`.
fn clean_url(url: &str) -> String {
    let url = url.split(['?', '#']).next().unwrap_or("");
    let Some((scheme, after)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (authority, path) = match after.find('/') {
        Some(i) => after.split_at(i),
        None => (after, ""),
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    format!("{scheme}://{host}{path}")
}
