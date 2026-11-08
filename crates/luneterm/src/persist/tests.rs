//! The store's tests: real files in a scratch directory made for each test (it must not exist
//! before; nothing outside it is touched). No test prints a settings file or the environment.

use std::fs;
use std::path::PathBuf;

use super::workspace::{SaveGuard, TabInfo, snapshot};
use super::*;
use crate::settings::Settings;

/// A fresh scratch directory path (not created).
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("luneterm-persist-{}-{tag}", std::process::id()));
    assert!(!dir.exists(), "{} already exists", dir.display());
    dir
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = scratch(tag);
        fs::create_dir_all(&dir).expect("create the scratch dir");
        Scratch(dir)
    }

    fn store(&self) -> Store {
        Store::new(self.0.clone())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn load_settings(store: &Store) -> Loaded<SettingsDoc> {
    store.load(SETTINGS_FILE, parse_settings)
}

#[test]
fn a_missing_file_is_the_defaults_and_nothing_is_written() {
    let dir = scratch("missing");
    let store = Store::new(dir.clone());
    assert_eq!(load_settings(&store), Loaded::Missing);
    assert!(!dir.exists(), "reading created nothing");
    let s = Scratch::new("missing-in-dir");
    assert_eq!(load_settings(&s.store()), Loaded::Missing);
    assert_eq!(fs::read_dir(&s.0).expect("dir").count(), 0);
}

#[test]
fn saved_settings_load_back_and_the_file_is_private() {
    let s = Scratch::new("round");
    let store = s.store();
    let doc = SettingsDoc {
        settings: Settings {
            font_size: 19,
            palette: "Nord".into(),
            ..Settings::default()
        },
        unknown: vec![("later".into(), Json::Int(7))],
    };
    store
        .save(
            SETTINGS_FILE,
            render_settings(&doc).expect("writes").as_bytes(),
        )
        .expect("saves");
    assert_eq!(load_settings(&store), Loaded::Ok(doc));
    // Only the file itself is left: no temporary file.
    let names: Vec<_> = fs::read_dir(&s.0)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
        .collect();
    assert_eq!(names, [SETTINGS_FILE]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(store.path(SETTINGS_FILE))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}

#[test]
fn the_directory_is_created_private_and_a_save_replaces_the_file_whole() {
    let dir = scratch("create");
    let store = Store::new(dir.join("nested").join("Luneterm"));
    store.save(WORKSPACE_FILE, b"first\n").expect("saves");
    store
        .save(WORKSPACE_FILE, b"second\n")
        .expect("saves again");
    assert_eq!(
        fs::read(store.path(WORKSPACE_FILE)).expect("read"),
        b"second\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(store.dir())
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_file_is_copied_to_bak_and_the_original_is_left_alone() {
    let s = Scratch::new("corrupt");
    let store = s.store();
    let bad = b"{ this is not json".to_vec();
    fs::write(store.path(SETTINGS_FILE), &bad).expect("write");
    let loaded = load_settings(&store);
    let Loaded::Rejected {
        reason,
        backup,
        save_blocked,
    } = loaded
    else {
        panic!("expected a rejection");
    };
    assert_eq!(reason, Reject::Unreadable);
    assert!(!save_blocked);
    let backup = backup.expect("a copy was kept");
    assert_eq!(backup, store.path("settings.json.bak"));
    assert_eq!(fs::read(&backup).expect("backup"), bad);
    assert_eq!(
        fs::read(store.path(SETTINGS_FILE)).expect("original"),
        bad,
        "not rewritten"
    );
}

#[test]
fn a_newer_or_wrong_version_is_kept_too() {
    for (tag, text, want) in [
        (
            "newer",
            &br#"{"version":99,"font_size":20}"#[..],
            Reject::NewerVersion(99),
        ),
        ("none", br#"{"font_size":20}"#, Reject::MissingVersion),
    ] {
        let s = Scratch::new(tag);
        let store = s.store();
        fs::write(store.path(SETTINGS_FILE), text).expect("write");
        match load_settings(&store) {
            Loaded::Rejected { reason, backup, .. } => {
                assert_eq!(reason, want);
                assert_eq!(fs::read(backup.expect("copy")).expect("backup"), text);
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn an_existing_backup_is_never_overwritten_the_next_is_numbered() {
    let s = Scratch::new("numbered");
    let store = s.store();
    for (i, content) in [&b"bad one"[..], b"bad two", b"bad three"]
        .into_iter()
        .enumerate()
    {
        fs::write(store.path(SETTINGS_FILE), content).expect("write");
        let Loaded::Rejected { backup, .. } = load_settings(&store) else {
            panic!("rejected")
        };
        let want = match i {
            0 => "settings.json.bak".to_owned(),
            n => format!("settings.json.bak.{n}"),
        };
        assert_eq!(backup, Some(store.path(&want)));
    }
    // Every copy is intact.
    assert_eq!(
        fs::read(store.path("settings.json.bak")).expect("0"),
        b"bad one"
    );
    assert_eq!(
        fs::read(store.path("settings.json.bak.1")).expect("1"),
        b"bad two"
    );
    assert_eq!(
        fs::read(store.path("settings.json.bak.2")).expect("2"),
        b"bad three"
    );
}

#[test]
fn a_file_over_the_readers_limit_is_rejected_without_being_parsed_and_kept() {
    let s = Scratch::new("big");
    let store = s.store();
    let big = vec![b' '; MAX_INPUT + 1];
    fs::write(store.path(SETTINGS_FILE), &big).expect("write");
    let called = std::cell::Cell::new(false);
    let loaded = store.load(SETTINGS_FILE, |_| {
        called.set(true);
        parse_settings(b"{}")
    });
    assert!(!called.get(), "the parser never saw it");
    assert!(matches!(
        loaded,
        Loaded::Rejected {
            reason: Reject::Unreadable,
            backup: Some(_),
            save_blocked: false
        }
    ));
}

#[test]
fn a_directory_where_the_file_should_be_is_not_read_and_not_saved_over() {
    let s = Scratch::new("notafile");
    let store = s.store();
    fs::create_dir(store.path(WORKSPACE_FILE)).expect("dir");
    let loaded = store.load(WORKSPACE_FILE, parse_workspace);
    assert_eq!(
        loaded,
        Loaded::Rejected {
            reason: Reject::Unreadable,
            backup: None,
            save_blocked: true
        }
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_temporary_name_is_not_followed_and_not_touched() {
    use std::os::unix::fs::symlink;
    let s = Scratch::new("symlink");
    let victim = s.0.join("victim");
    fs::write(&victim, b"precious").expect("victim");
    let planted = s.0.join(".planted.tmp");
    symlink(&victim, &planted).expect("symlink");
    // A dangling one too.
    let dangling = s.0.join(".dangling.tmp");
    symlink(s.0.join("does-not-exist"), &dangling).expect("symlink");
    let mut names = vec![".planted.tmp", ".dangling.tmp", ".fresh.tmp"].into_iter();
    write_atomic_named(&s.0, "settings.json", b"{}\n", &mut |_| {
        names.next().expect("a name").to_owned()
    })
    .expect("the third name is free");
    assert_eq!(
        fs::read(&victim).expect("victim"),
        b"precious",
        "nothing was written through it"
    );
    assert!(
        fs::symlink_metadata(&planted)
            .expect("still there")
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::symlink_metadata(&dangling)
            .expect("still there")
            .file_type()
            .is_symlink()
    );
    assert!(
        !s.0.join("does-not-exist").exists(),
        "a dangling link was not created through"
    );
    assert_eq!(fs::read(s.0.join("settings.json")).expect("saved"), b"{}\n");
}

#[test]
fn when_every_temporary_name_is_taken_the_save_fails_and_the_target_stands() {
    let s = Scratch::new("taken");
    fs::write(s.0.join("settings.json"), b"old").expect("old");
    fs::write(s.0.join(".taken.tmp"), b"stale").expect("stale");
    let result = write_atomic_named(&s.0, "settings.json", b"new", &mut |_| {
        ".taken.tmp".to_owned()
    });
    assert_eq!(
        result.expect_err("fails").kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(s.0.join("settings.json")).expect("target"), b"old");
    assert_eq!(
        fs::read(s.0.join(".taken.tmp")).expect("stale"),
        b"stale",
        "not ours: untouched"
    );
}

#[test]
fn a_failed_rename_leaves_the_target_and_removes_only_our_temporary_file() {
    let s = Scratch::new("renamefail");
    // The target is a non-empty directory: rename over it fails.
    fs::create_dir(s.0.join("settings.json")).expect("dir");
    fs::write(s.0.join("settings.json").join("inside"), b"x").expect("inside");
    let result = write_atomic(&s.0, "settings.json", b"{}\n");
    assert!(result.is_err());
    assert!(s.0.join("settings.json").join("inside").exists());
    let leftovers: Vec<_> = fs::read_dir(&s.0)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
        .collect();
    assert_eq!(
        leftovers,
        ["settings.json"],
        "no temporary file left behind"
    );
}

#[test]
fn the_persister_writes_the_latest_and_flushes() {
    let s = Scratch::new("persister");
    let persister = Persister::start(s.store()).expect("starts");
    for i in 0..20 {
        persister.submit(WORKSPACE_FILE, format!("text {i}\n").into_bytes());
    }
    persister.submit(SETTINGS_FILE, b"settings\n".to_vec());
    assert!(persister.flush(Duration::from_secs(10)));
    assert_eq!(
        fs::read(s.0.join(WORKSPACE_FILE)).expect("ws"),
        b"text 19\n",
        "the last one wins"
    );
    assert_eq!(
        fs::read(s.0.join(SETTINGS_FILE)).expect("settings"),
        b"settings\n"
    );
}

// ----------------------------------------------------------------------------- ledger 12

fn info(started: bool, dir: &str) -> TabInfo {
    TabInfo {
        started,
        launch_dir: Some(dir.into()),
        live_dir: None,
        title: None,
    }
}

/// The app's restore, with the shell start injected: tab `i` of `saved` starts when `spawn(i)`.
/// Returns the tabs as the app would hold them.
fn restore(
    saved: &Workspace,
    spawn: &dyn Fn(usize) -> bool,
    guard: &mut SaveGuard,
) -> Vec<TabInfo> {
    saved
        .tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let started = spawn(i);
            if !started {
                guard.spawn_failed(i as u64);
            }
            TabInfo {
                started,
                launch_dir: t.working_directory.clone(),
                live_dir: t.current_directory.clone(),
                title: t.manual_title.clone(),
            }
        })
        .collect()
}

#[test]
fn tabs_that_fail_to_start_never_shrink_the_saved_workspace() {
    let s = Scratch::new("ledger12");
    let store = s.store();
    let saved = Workspace {
        tabs: ["/a", "/b", "/c"]
            .iter()
            .map(|d| TabState {
                working_directory: Some((*d).into()),
                ..TabState::default()
            })
            .collect(),
        active: 2,
        unknown: Vec::new(),
    };
    let text = render_workspace(&saved).expect("writes");
    store.save(WORKSPACE_FILE, text.as_bytes()).expect("saves");

    // The next start: tab 1's shell cannot start.
    let mut guard = SaveGuard::new();
    let tabs = restore(&saved, &|i| i != 1, &mut guard);
    // Every trigger the app has asks `snapshot`; none of them writes while the failure shows.
    for active in 0..3 {
        assert_eq!(snapshot(&tabs, active, &guard, &[]), None);
    }
    // The previous file stands, byte for byte.
    assert_eq!(
        fs::read(store.path(WORKSPACE_FILE)).expect("file"),
        text.as_bytes()
    );

    // The user closes the failed tab: what remains really started, and now it is saved.
    guard.tab_closed(1);
    let remaining: Vec<TabInfo> = tabs.iter().filter(|t| t.started).cloned().collect();
    let ws = snapshot(&remaining, 1, &guard, &[]).expect("writes again");
    assert_eq!(ws.tabs.len(), 2);
    assert_eq!(
        ws.tabs
            .iter()
            .map(|t| t.working_directory.as_deref().unwrap_or(""))
            .collect::<Vec<_>>(),
        ["/a", "/c"]
    );
    assert_eq!(ws.active, 1);
}

#[test]
fn a_snapshot_holds_only_started_tabs_and_maps_the_active_index() {
    let guard = SaveGuard::new();
    let tabs = [info(true, "/a"), info(false, "/b"), info(true, "/c")];
    let ws = snapshot(&tabs, 2, &guard, &[]).expect("snapshot");
    assert_eq!(ws.tabs.len(), 2);
    assert_eq!(ws.active, 1, "the third tab is the second saved one");
    // An active tab that did not start falls back to the first.
    assert_eq!(snapshot(&tabs, 1, &guard, &[]).expect("snapshot").active, 0);
    assert_eq!(
        snapshot(&[], 0, &guard, &[]).expect("snapshot").tabs.len(),
        0
    );
}

#[test]
fn a_quick_exit_blocks_the_write_until_the_user_acts() {
    let mut guard = SaveGuard::new();
    let tabs = [info(true, "/a")];
    guard.child_exited(7, Duration::from_millis(20));
    assert!(snapshot(&tabs, 0, &guard, &[]).is_none());
    guard.user_acted();
    assert!(snapshot(&tabs, 0, &guard, &[]).is_some());
}

// ------------------------------------------------------------------------------ Persist

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
        .collect();
    v.sort();
    v
}

use std::path::Path;

#[test]
fn a_start_with_no_files_writes_nothing_and_changes_nothing() {
    let s = Scratch::new("startup-none");
    let (persist, start) = Persist::load(s.store());
    assert_eq!(start.settings, Settings::default());
    assert!(start.workspace.is_none() && start.notices.is_empty());
    assert!(persist.flush(Duration::from_secs(5)));
    assert!(names(&s.0).is_empty(), "{:?}", names(&s.0));
}

#[test]
fn a_start_with_corrupt_files_keeps_copies_uses_defaults_and_rewrites_nothing() {
    let s = Scratch::new("startup-corrupt");
    fs::write(s.0.join(SETTINGS_FILE), b"{oops").expect("write");
    fs::write(s.0.join(WORKSPACE_FILE), br#"{"version":9}"#).expect("write");
    let (persist, start) = Persist::load(s.store());
    assert_eq!(start.settings, Settings::default());
    assert!(start.workspace.is_none());
    assert_eq!(start.notices.len(), 2);
    assert!(
        start
            .notices
            .iter()
            .all(|n| !n.contains("oops") && n.contains("copy was kept"))
    );
    assert!(persist.flush(Duration::from_secs(5)));
    assert_eq!(
        names(&s.0),
        [
            "settings.json",
            "settings.json.bak",
            "workspace.json",
            "workspace.json.bak"
        ]
    );
    assert_eq!(fs::read(s.0.join(SETTINGS_FILE)).expect("file"), b"{oops");
    // The workspace stays untouched until the user acts on the tabs, then it may be rewritten.
    let ws = Workspace::default();
    persist.save_workspace(&ws);
    assert!(persist.flush(Duration::from_secs(5)));
    assert_eq!(
        fs::read(s.0.join(WORKSPACE_FILE)).expect("file"),
        br#"{"version":9}"#
    );
}

#[test]
fn settings_saved_after_a_rejection_replace_the_file_and_the_backup_stays() {
    let s = Scratch::new("startup-then-save");
    fs::write(s.0.join(SETTINGS_FILE), b"garbage").expect("write");
    let (persist, start) = Persist::load(s.store());
    let changed = Settings {
        confirm_close: false,
        ..start.settings
    };
    persist.save_settings(&changed);
    assert!(persist.flush(Duration::from_secs(5)));
    assert_eq!(
        fs::read(s.0.join("settings.json.bak")).expect("backup"),
        b"garbage"
    );
    let text = fs::read(s.0.join(SETTINGS_FILE)).expect("file");
    assert_eq!(parse_settings(&text).expect("valid").settings, changed);
}

#[test]
fn a_start_with_good_files_restores_them_and_keeps_unknown_fields_through_a_save() {
    let s = Scratch::new("startup-good");
    fs::write(
        s.0.join(SETTINGS_FILE),
        br#"{"version":1,"font_size":15,"extra":{"k":[1]}}"#,
    )
    .expect("write");
    fs::write(
        s.0.join(WORKSPACE_FILE),
        br#"{"version":1,"active":1,"tabs":[{"cwd":"/a"},{"cwd":"/b","title":"two"}],"layout":3}"#,
    )
    .expect("write");
    let (persist, start) = Persist::load(s.store());
    assert_eq!(start.settings.font_size, 15);
    let ws = start.workspace.expect("a workspace");
    assert_eq!((ws.tabs.len(), ws.active), (2, 1));
    assert!(start.notices.is_empty());
    persist.save_settings(&start.settings);
    persist.save_workspace(&ws);
    assert!(persist.flush(Duration::from_secs(5)));
    let settings_text =
        String::from_utf8(fs::read(s.0.join(SETTINGS_FILE)).expect("file")).expect("utf-8");
    assert!(settings_text.contains(r#""extra":{"k":[1]}"#));
    let ws_text =
        String::from_utf8(fs::read(s.0.join(WORKSPACE_FILE)).expect("file")).expect("utf-8");
    assert!(ws_text.contains(r#""layout":3"#));
}

#[test]
fn an_empty_saved_workspace_is_no_workspace() {
    let s = Scratch::new("startup-empty");
    fs::write(s.0.join(WORKSPACE_FILE), br#"{"version":1,"tabs":[]}"#).expect("write");
    let (_, start) = Persist::load(s.store());
    assert!(start.workspace.is_none());
}

#[test]
fn no_persistence_writes_and_reads_nothing() {
    let p = Persist::none();
    p.save_settings(&Settings::default());
    p.save_workspace(&Workspace::default());
    assert!(!p.workspace_writable());
    assert!(p.flush(Duration::from_millis(10)));
}
