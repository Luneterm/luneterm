//! The update worker's state machine against a real signed feed (plan step 3, P5).
//!
//! The feed is made at test time by lunepack's own writers at the pinned rev (the core's
//! `ReleaseLayout` and LZMA codec, documents signed by fresh Ed25519 keys through the testkit's
//! `ChainFixture`), written to a temporary directory and read over a `file:` URL; nothing signed
//! is checked in and the keys never leave the process. The install state is committed by the
//! core's own writer. Time is the injected clock's: the feed is issued at `SUITE_T0` and the
//! updater's clock reads `SUITE_T0 + 1 h` (or past the feed's expiry), never the real clock.
//! Only the cancel cases read the same documents through the testkit's fake transport, which can
//! hold a pack body forever.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use lunepack::{AppId, Channel, Layout, Target, TrustedRoot, UpdaterBuilder, Version, blocking};
use lunepack_codec_lzma::LzmaCodec;
use lunepack_core::state::{InstallState, Scope, UserScope};
use lunepack_core::{PayloadKind, RelPath, ReleaseLayout, Signer, pack_file};
use lunepack_crypto::{Ed25519Signer, Ed25519Verifier, Sha256Hasher, generate_key_file};
use lunepack_fs::StdFs;
use lunepack_testkit::{
    ChainFixture, FAKE_BASE_URL, FakeClock, FakeOrigin, FakeTransport, SUITE_APP_ID, SUITE_CHANNEL,
    SUITE_T0, TransportFault,
};

use super::*;

const DAY: u64 = 86_400;
const TARGET: Target = Target::LinuxX86_64;
const INSTALL_ID: [u8; 16] = [7; 16];
const FEED_NAME: &str = "feed-stable.json";
const V090: &str = "0.9.0";
const V100: &str = "1.0.0";
/// The feed's expiry: 30 days after it is issued.
const EXPIRES: u64 = SUITE_T0 + 30 * DAY;
/// How long a test waits for an event before it fails.
const EVENT_WAIT: Duration = Duration::from_secs(20);

fn app_id() -> AppId {
    AppId::parse(SUITE_APP_ID).expect("app id")
}

fn channel() -> Channel {
    Channel::parse(SUITE_CHANNEL).expect("channel")
}

fn version(text: &str) -> Version {
    Version::parse(text).expect("version")
}

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("luneterm-update-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        // Resolved, so no symlinked component (macOS `/var` -> `/private/var`).
        TempDir(std::fs::canonicalize(&dir).expect("canonical temp dir"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The published documents, built once per process by lunepack's writers.
struct Fixture {
    /// `keys.json`, the manifest and the pack of 1.0.0, by path under the feed directory.
    files: Vec<(String, Vec<u8>)>,
    /// The feed listing 1.0.0, signed by the release key.
    feed: Vec<u8>,
    /// The same feed signed by a key the key set does not list.
    feed_bad_signature: Vec<u8>,
    /// The same release listed on the `beta` channel (`feed-beta.json`).
    feed_beta: Vec<u8>,
    root_public: [u8; 32],
}

/// The files of the release: an executable, a compressible library and incompressible data.
fn release_files() -> Vec<(&'static str, Vec<u8>, bool)> {
    let mut noise = Vec::with_capacity(64 * 1024);
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    while noise.len() < 64 * 1024 {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        noise.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    vec![
        ("bin/luneterm", b"luneterm 1.0.0 ".repeat(400), true),
        (
            "lib/shared.txt",
            b"a shared line of text\n".repeat(2000),
            false,
        ),
        ("share/data.bin", noise, false),
    ]
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(build_fixture)
}

fn build_fixture() -> Fixture {
    // Seeds: 0 the root key, 1 the release key, 2 a key nobody lists. The private keys live in a
    // temporary directory only while they are loaded.
    let signers: Vec<Ed25519Signer> = {
        let dir = TempDir::new("keys");
        (0..3)
            .map(|i| {
                let key = generate_key_file(&dir.0.join(format!("k{i}"))).expect("key");
                Ed25519Signer::from_key_file(&key.path).expect("load key")
            })
            .collect()
    };
    let sign = |seed: u8, message: &[u8]| {
        let signer = &signers[usize::from(seed)];
        (
            signer.public_key(),
            signer.sign(message).expect("sign with a fresh key"),
        )
    };
    let mut small_order = [0u8; 32];
    small_order[0] = 1;
    let fx = ChainFixture {
        hasher: &Sha256Hasher,
        verifier: &Ed25519Verifier,
        sign: &sign,
        invalid_key: small_order,
    };

    let mut files = Vec::new();
    let keyset = fx.keyset_document(1, 1, None, 1, &[1]);
    files.push((
        "keys.json".to_owned(),
        fx.envelope(PayloadKind::Keys, &keyset.to_payload(), &[0]),
    ));
    let mut layout = ReleaseLayout::new(app_id(), V100, TARGET).expect("layout");
    for (path, bytes, exec) in release_files() {
        let packed = pack_file(&bytes, &LzmaCodec, &LzmaCodec, &Sha256Hasher).expect("pack file");
        layout
            .add_file(RelPath::parse(path).expect("path"), exec, &packed)
            .expect("add file");
    }
    let (doc, pack) = layout.finish(SUITE_T0, &Sha256Hasher).expect("finish");
    let manifest = fx.envelope(PayloadKind::Manifest, &doc.to_payload(), &[1]);
    let release = fx.release(V100, &[(TARGET, &manifest)], None);
    let feed_doc = fx.feed_document(1, Some(EXPIRES), vec![release]);
    let payload = feed_doc.to_payload();
    let feed = fx.envelope(PayloadKind::Feed, &payload, &[1]);
    let feed_bad_signature = fx.envelope(PayloadKind::Feed, &payload, &[2]);
    let mut beta_doc = fx.feed_document(
        1,
        Some(EXPIRES),
        vec![fx.release(V100, &[(TARGET, &manifest)], None)],
    );
    beta_doc.channel = Channel::parse("beta").expect("channel");
    let feed_beta = fx.envelope(PayloadKind::Feed, &beta_doc.to_payload(), &[1]);
    files.push((
        format!("{V100}/{}.manifest.json", TARGET.as_str()),
        manifest,
    ));
    files.push((format!("{V100}/{}.pack", TARGET.as_str()), pack));
    Fixture {
        files,
        feed,
        feed_bad_signature,
        feed_beta,
        root_public: fx.public_key(0),
    }
}

impl Fixture {
    fn root(&self) -> TrustedRoot {
        TrustedRoot::from_parts(
            app_id(),
            1,
            1,
            &[self.root_public],
            &Sha256Hasher,
            &Ed25519Verifier,
        )
        .expect("root")
    }

    /// Writes every file and `feed` into `dir`.
    fn write_dir(&self, dir: &Path, feed: &[u8]) {
        for (path, bytes) in &self.files {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().expect("parent")).expect("dirs");
            std::fs::write(&full, bytes).expect("write");
        }
        std::fs::write(dir.join(FEED_NAME), feed).expect("feed");
    }

    /// An origin of the fake transport serving every file and the good feed.
    fn origin(&self) -> FakeOrigin {
        let origin = FakeOrigin::new();
        for (path, bytes) in &self.files {
            origin.put(path, bytes.clone());
        }
        origin.put(FEED_NAME, self.feed.clone());
        origin
    }
}

/// An install with `current` installed (no tree: every file of a later release is fetched).
struct Site {
    tmp: TempDir,
}

impl Site {
    fn new(current: &str) -> Site {
        let tmp = TempDir::new("site");
        for dir in ["state", "cache", "versions"] {
            std::fs::create_dir_all(tmp.0.join(dir)).expect("dir");
        }
        let site = Site { tmp };
        let store = UserScope {
            dir: site.dir("state"),
            app_id: app_id(),
        };
        let loaded = store.load(&StdFs::new(), &Sha256Hasher).expect("load");
        let record = InstallState::new(1, app_id(), Scope::User, INSTALL_ID, TARGET).install(
            version(current),
            SUITE_T0 - 30 * DAY,
            TARGET,
            INSTALL_ID,
        );
        store
            .commit(&StdFs::new(), &loaded, &record, &Sha256Hasher)
            .expect("commit");
        site
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.tmp.0.join(name)
    }

    fn builder(&self, feed_url: &str, now: u64) -> UpdaterBuilder {
        let layout = Layout::User {
            state_dir: self.dir("state"),
            cache_dir: self.dir("cache"),
            versions_dir: self.dir("versions"),
            current_tree: None,
        };
        UpdaterBuilder::new(app_id(), fixture().root(), feed_url, channel(), layout)
            .clock(Arc::new(FakeClock::new(now)))
            .workers(2)
    }
}

/// A feed directory holding the release and `feed`, read through a `file:` URL.
struct FileFeed {
    dir: TempDir,
}

impl FileFeed {
    fn new(feed: &[u8]) -> FileFeed {
        let dir = TempDir::new("feed");
        fixture().write_dir(&dir.0, feed);
        FileFeed { dir }
    }

    fn url(&self) -> String {
        url::Url::from_file_path(self.dir.0.join(FEED_NAME))
            .expect("file url")
            .to_string()
    }
}

/// The worker under test and the events it posts.
struct Harness {
    updater: Updater,
    events: mpsc::Receiver<Event>,
}

impl Harness {
    fn new(up: blocking::Updater) -> Harness {
        let (tx, events) = mpsc::channel();
        let updater = Updater::start(
            Box::new(move || Ok(up)),
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        Harness { updater, events }
    }

    /// Over the file feed, the updater's clock at `now`.
    fn over_file(site: &Site, feed: &FileFeed, now: u64) -> Harness {
        let up = site.builder(&feed.url(), now).build().expect("build");
        Harness::new(blocking::Updater::from(up))
    }

    /// The states posted until (and including) the first one `last` accepts.
    fn states_until(&self, last: impl Fn(&UpdateState) -> bool) -> Vec<UpdateState> {
        let deadline = Instant::now() + EVENT_WAIT;
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(Event::State(s)) => {
                    let done = last(&s);
                    seen.push(s);
                    if done {
                        return seen;
                    }
                }
                Ok(Event::Restart) => panic!("unexpected restart event; seen {seen:?}"),
                Err(e) => panic!("no final state ({e:?}); seen {seen:?}"),
            }
        }
    }

    /// Nothing is posted for `wait`.
    fn quiet_for(&self, wait: Duration) {
        if let Ok(e) = self.events.recv_timeout(wait) {
            panic!("unexpected event {e:?}");
        }
    }
}

fn settled(s: &UpdateState) -> bool {
    !matches!(s, UpdateState::Checking | UpdateState::Downloading(_))
}

#[test]
fn a_copy_lunepack_did_not_install_is_unavailable_and_ignores_every_command() {
    // The test binary has no update config beside it: what `cargo run` and a portable zip are.
    let launch = startup();
    assert_eq!(launch, Launch::not_installed());
    assert!(!launch.exit());
    let (tx, rx) = mpsc::channel();
    let mut u = Updater::spawn(&launch, None, move |e| {
        let _ = tx.send(e);
    });
    assert_eq!(
        u.initial_state(),
        UpdateState::Unavailable(Unavailable::NotInstalled)
    );
    assert!(!u.check(false));
    assert!(!u.check(true));
    assert!(!u.download());
    assert!(!u.restart());
    // No worker: nothing is ever posted (and nothing reached a network).
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    assert!(u.shutdown(Duration::ZERO));
}

#[test]
fn up_to_date_when_the_feed_lists_the_installed_version() {
    let site = Site::new(V100);
    let feed = FileFeed::new(&fixture().feed);
    let h = Harness::over_file(&site, &feed, SUITE_T0 + 3600);
    assert!(h.updater.check(false));
    assert_eq!(
        h.states_until(settled),
        vec![UpdateState::Checking, UpdateState::UpToDate]
    );
}

#[test]
fn a_newer_version_is_available_and_says_who_asked() {
    let site = Site::new(V090);
    let feed = FileFeed::new(&fixture().feed);
    let h = Harness::over_file(&site, &feed, SUITE_T0 + 3600);
    assert!(h.updater.check(true));
    assert_eq!(
        h.states_until(settled),
        vec![
            UpdateState::Checking,
            UpdateState::Available {
                version: V100.to_owned(),
                auto: true
            }
        ]
    );
    // Checked again by hand: the same version, not marked automatic.
    assert!(h.updater.check(false));
    assert_eq!(
        h.states_until(settled).last(),
        Some(&UpdateState::Available {
            version: V100.to_owned(),
            auto: false
        })
    );
}

#[test]
fn download_stages_the_update_and_a_restart_is_then_offered() {
    let site = Site::new(V090);
    let feed = FileFeed::new(&fixture().feed);
    let h = Harness::over_file(&site, &feed, SUITE_T0 + 3600);
    assert!(h.updater.check(false));
    h.states_until(settled);
    assert!(h.updater.download());
    let states = h.states_until(settled);
    assert_eq!(states.first(), Some(&UpdateState::Downloading(0)));
    assert_eq!(states.last(), Some(&UpdateState::Ready(V100.to_owned())));
    let mut last_pct = 0;
    for s in &states[..states.len() - 1] {
        let UpdateState::Downloading(pct) = s else {
            panic!("unexpected state while downloading: {s:?}");
        };
        assert!(*pct >= last_pct && *pct <= 100, "progress {states:?}");
        last_pct = *pct;
    }
    // The staged tree holds the release's files.
    let tree = site.dir("versions").join(V100);
    for (path, bytes, _) in release_files() {
        assert_eq!(
            std::fs::read(tree.join(path)).expect("staged file"),
            bytes,
            "{path}"
        );
    }
    // Staged: a check looks for nothing more and says so again.
    assert!(h.updater.check(false));
    assert_eq!(
        h.states_until(|_| true),
        vec![UpdateState::Ready(V100.to_owned())]
    );
    // A new worker over the same install finds the staged version (no network).
    drop(h);
    let h = Harness::over_file(&site, &feed, SUITE_T0 + 3600);
    assert_eq!(
        h.states_until(|_| true),
        vec![UpdateState::Ready(V100.to_owned())]
    );
}

#[test]
fn a_feed_signed_by_an_unlisted_key_fails_as_text() {
    let site = Site::new(V090);
    let feed = FileFeed::new(&fixture().feed_bad_signature);
    let h = Harness::over_file(&site, &feed, SUITE_T0 + 3600);
    assert!(h.updater.check(false));
    let states = h.states_until(settled);
    let Some(UpdateState::Failed(text)) = states.last() else {
        panic!("expected a failure, got {states:?}");
    };
    assert!(!text.is_empty());
    // lunepack: "0 of 1 required distinct keys verified".
    assert!(text.contains("verified"), "{text}");
    // Nothing was offered: a download is a no-op that posts the failure again.
    assert!(h.updater.download());
    assert!(matches!(
        h.states_until(|_| true).as_slice(),
        [UpdateState::Failed(_)]
    ));
    assert!(!site.dir("versions").join(V100).exists());
}

#[test]
fn an_expired_feed_fails_as_text() {
    let site = Site::new(V090);
    let feed = FileFeed::new(&fixture().feed);
    let h = Harness::over_file(&site, &feed, EXPIRES + DAY);
    assert!(h.updater.check(false));
    let states = h.states_until(settled);
    let Some(UpdateState::Failed(text)) = states.last() else {
        panic!("expected a failure, got {states:?}");
    };
    assert!(!text.is_empty());
    assert!(text.contains("expired"), "{text}");
}

/// A worker whose pack body never arrives once `hold` is called (the fake transport).
fn held_download() -> (Site, Harness, FakeOrigin) {
    let site = Site::new(V090);
    let origin = fixture().origin();
    let transport = FakeTransport::new(origin.clone());
    let up = site
        .builder(&format!("{FAKE_BASE_URL}/{FEED_NAME}"), SUITE_T0 + 3600)
        .transport(Arc::new(transport))
        .build()
        .expect("build");
    let h = Harness::new(blocking::Updater::from(up));
    assert!(h.updater.check(false));
    assert_eq!(
        h.states_until(settled).last(),
        Some(&UpdateState::Available {
            version: V100.to_owned(),
            auto: false
        })
    );
    origin.clear_requests();
    origin.add_fault(TransportFault::PendingForever);
    (site, h, origin)
}

/// Starts the download and waits until the pack was asked for (the worker is then blocked).
fn start_held(h: &Harness, origin: &FakeOrigin) {
    assert!(h.updater.download());
    assert_eq!(h.states_until(|_| true), vec![UpdateState::Downloading(0)]);
    let deadline = Instant::now() + EVENT_WAIT;
    while !origin.requests().iter().any(|r| r.url.ends_with(".pack")) {
        assert!(Instant::now() < deadline, "the pack was never requested");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn cancel_mid_download_returns_to_the_offer_and_one_operation_runs_at_a_time() {
    let (site, h, origin) = held_download();
    start_held(&h, &origin);
    // Busy: a second command is dropped, not queued.
    assert!(!h.updater.check(false));
    assert!(!h.updater.check(true));
    assert!(!h.updater.download());
    assert!(!h.updater.restart());
    h.quiet_for(Duration::from_millis(100));
    let at = Instant::now();
    h.updater.cancel();
    assert_eq!(
        h.states_until(|_| true),
        vec![UpdateState::Available {
            version: V100.to_owned(),
            auto: false
        }]
    );
    assert!(
        at.elapsed() < Duration::from_secs(1),
        "cancel took {:?}",
        at.elapsed()
    );
    assert!(!site.dir("versions").join(V100).exists());
    // Free again: the next command is taken.
    assert!(h.updater.download());
    assert_eq!(h.states_until(|_| true), vec![UpdateState::Downloading(0)]);
}

#[test]
fn shutdown_mid_download_ends_the_worker_within_a_second() {
    let (_site, mut h, origin) = held_download();
    start_held(&h, &origin);
    let at = Instant::now();
    assert!(h.updater.shutdown(Duration::from_secs(1)));
    assert!(
        at.elapsed() < Duration::from_secs(1),
        "shutdown took {:?}",
        at.elapsed()
    );
    // Stopped: commands are ignored.
    assert!(!h.updater.check(false));
}

#[test]
fn restart_with_nothing_staged_changes_nothing() {
    let site = Site::new(V090);
    let feed = FileFeed::new(&fixture().feed);
    let h = Harness::over_file(&site, &feed, SUITE_T0 + 3600);
    assert!(h.updater.restart());
    assert_eq!(h.states_until(|_| true), vec![UpdateState::Idle]);
}

#[test]
fn error_text_keeps_scheme_host_and_path_only() {
    assert_eq!(
        redact(
            "fetch https://user:pw@updates.example.com/luneterm/feed-stable.json?token=abc#x failed"
        ),
        "fetch https://updates.example.com/luneterm/feed-stable.json failed"
    );
    assert_eq!(
        redact("a \"http://h.example/p?q=1\" and file:///tmp/f?x and ok"),
        "a \"http://h.example/p\" and file:///tmp/f and ok"
    );
    assert_eq!(redact("no url here"), "no url here");
    assert_eq!(redact("https://h.example?k=v"), "https://h.example");
    let long = "x".repeat(MAX_ERROR_CHARS + 50);
    assert_eq!(redact(&long).chars().count(), MAX_ERROR_CHARS + 1);
}

#[test]
fn unavailable_reasons_read_as_text() {
    assert_eq!(
        Unavailable::NotInstalled.text(),
        "Updates need the Luneterm installer"
    );
    assert!(
        Unavailable::Broken("state".to_owned())
            .text()
            .ends_with("state")
    );
}

// ---------------------------------------------------------------------------------- channel (Q17)

#[test]
fn the_first_run_records_the_installed_channel_and_a_later_pack_does_not_change_it() {
    use super::channel::{Settled, resolve};
    let settled = |channel: &str, rewrite: bool| Settled {
        channel: Some(channel.to_owned()),
        rewrite,
    };
    // First run of a beta install: nothing recorded, the tree says beta -> record beta.
    assert_eq!(resolve("", Some("beta")), settled("beta", true));
    // After an update to a stable pack the tree says stable; the recorded beta wins, no rewrite.
    assert_eq!(resolve("beta", Some("stable")), settled("beta", false));
    // A stable install stays stable.
    assert_eq!(resolve("", Some("stable")), settled("stable", true));
    assert_eq!(resolve("stable", Some("stable")), settled("stable", false));
    // Garbled values (not a channel name) fall back to the tree's channel and are rewritten.
    for bad in ["Beta", "-beta", "be ta", "be/ta", &"a".repeat(33), "\u{7}"] {
        assert_eq!(
            resolve(bad, Some("stable")),
            settled("stable", true),
            "{bad:?}"
        );
    }
    // Not installed: nothing followed, nothing written, whatever the settings hold.
    for persisted in ["", "beta", "garbled value"] {
        assert_eq!(
            resolve(persisted, None),
            Settled {
                channel: None,
                rewrite: false
            },
            "{persisted:?}"
        );
    }
    // The copy under test is not installed.
    assert_eq!(
        channel::settle(&Launch::not_installed(), "beta"),
        Settled {
            channel: None,
            rewrite: false
        }
    );
}

/// An updater over the fake origin, the install's own (baked) channel `baked`, following
/// `follow`; the origin serves `feed-stable.json` and `feed-beta.json`.
fn channel_harness(baked: &str, follow: Option<&str>) -> (Harness, FakeOrigin, Site) {
    let site = Site::new(V090);
    let origin = fixture().origin();
    origin.put("feed-beta.json", fixture().feed_beta.clone());
    let transport = FakeTransport::new(origin.clone());
    let layout = Layout::User {
        state_dir: site.dir("state"),
        cache_dir: site.dir("cache"),
        versions_dir: site.dir("versions"),
        current_tree: None,
    };
    let builder = UpdaterBuilder::new(
        app_id(),
        fixture().root(),
        &format!("{FAKE_BASE_URL}/feed-{baked}.json"),
        Channel::parse(baked).expect("channel"),
        layout,
    )
    .clock(Arc::new(FakeClock::new(SUITE_T0 + 3600)))
    .workers(2)
    .transport(Arc::new(transport));
    let up = super::channel::follow(builder, follow)
        .build()
        .expect("build");
    (Harness::new(blocking::Updater::from(up)), origin, site)
}

fn feeds_asked(origin: &FakeOrigin) -> Vec<String> {
    origin
        .requests()
        .iter()
        .filter_map(|r| r.url.rsplit('/').next().map(str::to_owned))
        .filter(|name| name.starts_with("feed-"))
        .collect()
}

#[test]
fn a_beta_install_that_received_a_stable_pack_still_checks_beta() {
    // The tree's own config now says stable (the pack replaced it); the persisted channel is beta.
    let (h, origin, _site) = channel_harness("stable", Some("beta"));
    assert!(h.updater.check(false));
    assert_eq!(
        h.states_until(settled),
        vec![
            UpdateState::Checking,
            UpdateState::Available {
                version: V100.to_owned(),
                auto: false
            }
        ]
    );
    let asked = feeds_asked(&origin);
    assert!(!asked.is_empty(), "no feed was asked for");
    assert!(asked.iter().all(|n| n == "feed-beta.json"), "{asked:?}");
}

#[test]
fn a_stable_install_stays_stable_and_no_channel_keeps_the_trees_own() {
    for follow in [Some("stable"), None] {
        let (h, origin, _site) = channel_harness("stable", follow);
        assert!(h.updater.check(false));
        assert_eq!(
            h.states_until(settled).last(),
            Some(&UpdateState::Available {
                version: V100.to_owned(),
                auto: false
            })
        );
        let asked = feeds_asked(&origin);
        assert!(!asked.is_empty(), "no feed was asked for");
        assert!(asked.iter().all(|n| n == "feed-stable.json"), "{asked:?}");
    }
    // A name that is no channel is ignored at the builder (settle never hands one over).
    let (h, origin, _site) = channel_harness("beta", Some("Not A Channel"));
    assert!(h.updater.check(false));
    assert_eq!(
        h.states_until(settled).last(),
        Some(&UpdateState::Available {
            version: V100.to_owned(),
            auto: false
        })
    );
    assert!(feeds_asked(&origin).iter().all(|n| n == "feed-beta.json"));
}
