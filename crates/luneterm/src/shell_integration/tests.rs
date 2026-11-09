use super::*;

fn vars<'a>(set: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Var + 'a {
    move |k| {
        set.iter()
            .find(|(name, _)| *name == k)
            .map_or(Var::Unset, |(_, v)| Var::Set((*v).to_owned()))
    }
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

/// An absolute directory on this OS (`/a/b`, or `C:\a\b` on Windows, where `/a/b` is not absolute).
fn abs(p: &str) -> String {
    if cfg!(windows) {
        format!("C:{}", p.replace('/', "\\"))
    } else {
        p.to_owned()
    }
}

fn fake_install() -> Install {
    let root = format!("{0}cfg{0}shell-integration", sep());
    Install { root }
}

fn value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[test]
fn the_shell_kind_is_the_file_name_without_exe_and_without_case() {
    use ShellKind::*;
    for (program, os, want) in [
        ("/bin/zsh", Os::Macos, Zsh),
        ("/usr/local/bin/bash", Os::Linux, Bash),
        ("zsh", Os::Macos, Zsh),
        ("/opt/homebrew/bin/fish", Os::Macos, Fish),
        ("/usr/bin/pwsh", Os::Linux, PowerShell),
        ("pwsh.exe", Os::Windows, PowerShell),
        (
            "C:\\Program Files\\PowerShell\\7\\PWSH.EXE",
            Os::Windows,
            PowerShell,
        ),
        (
            "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe",
            Os::Windows,
            PowerShell,
        ),
        ("C:\\Windows\\System32\\cmd.exe", Os::Windows, Cmd),
        ("/bin/sh", Os::Linux, Unknown),
        ("/bin/dash", Os::Linux, Unknown),
        ("", Os::Linux, Unknown),
        // A directory called zsh is not the shell; a name that only ends in one is not either.
        ("/usr/zsh/bin/tool", Os::Linux, Unknown),
        ("/bin/zsh5", Os::Linux, Unknown),
        ("/bin/xbash", Os::Linux, Unknown),
        // `.exe` is a Windows suffix only.
        ("/bin/zsh.exe", Os::Linux, Unknown),
        // A backslash splits a path on Windows only.
        ("C:\\tools\\bash.exe", Os::Windows, Bash),
        ("C:\\tools\\bash", Os::Linux, Unknown),
    ] {
        assert_eq!(detect_kind(program, os), want, "{program:?} on {os:?}");
    }
}

#[test]
fn a_directory_or_file_must_be_real_ours_and_not_writable_by_others() {
    let dir = |kind, uid, mode| Facts {
        kind,
        uid,
        mode,
        len: 0,
    };
    let me = 501;
    assert_eq!(check_dir(&dir(FileKind::Dir, me, 0o700), me), Ok(()));
    assert_eq!(check_dir(&dir(FileKind::Dir, me, 0o755), me), Ok(()));
    assert_eq!(check_file(&dir(FileKind::File, me, 0o600), me), Ok(()));
    assert_eq!(check_file(&dir(FileKind::File, me, 0o644), me), Ok(()));
    // Not ours.
    assert_eq!(
        check_dir(&dir(FileKind::Dir, 0, 0o700), me),
        Err(Unsafe::NotOwned)
    );
    assert_eq!(
        check_file(&dir(FileKind::File, me + 1, 0o600), me),
        Err(Unsafe::NotOwned)
    );
    // Group or other write, either bit.
    for mode in [0o770, 0o707, 0o720, 0o702, 0o777, 0o722] {
        assert_eq!(
            check_dir(&dir(FileKind::Dir, me, mode), me),
            Err(Unsafe::Writable),
            "{mode:o}"
        );
        assert_eq!(
            check_file(&dir(FileKind::File, me, mode), me),
            Err(Unsafe::Writable),
            "{mode:o}"
        );
    }
    // A symlink is never a directory or a file here, whoever owns it and whatever its mode.
    for kind in [FileKind::Symlink, FileKind::Other, FileKind::File] {
        assert_eq!(
            check_dir(&dir(kind, me, 0o700), me),
            Err(Unsafe::NotADirectory)
        );
    }
    for kind in [FileKind::Symlink, FileKind::Other, FileKind::Dir] {
        assert_eq!(check_file(&dir(kind, me, 0o600), me), Err(Unsafe::NotAFile));
    }
}

#[test]
fn a_powershell_literal_doubles_every_kind_of_single_quote() {
    assert_eq!(pwsh_literal("C:\\Users\\ann\\cfg"), "'C:\\Users\\ann\\cfg'");
    assert_eq!(pwsh_literal("O'Brien"), "'O''Brien'");
    assert_eq!(pwsh_literal("''"), "''''''");
    // PowerShell treats the curly single quotes as quotes too.
    assert_eq!(
        pwsh_literal("a\u{2018}b\u{2019}c\u{201A}d\u{201B}e"),
        "'a\u{2018}\u{2018}b\u{2019}\u{2019}c\u{201A}\u{201A}d\u{201B}\u{201B}e'"
    );
    // Double quotes, dollars and backticks are literal inside single quotes: left alone.
    assert_eq!(pwsh_literal("a\"$b`c"), "'a\"$b`c'");
}

/// Undoes what PowerShell's parser does to a single-quoted literal, to check the round trip.
fn pwsh_unquote(lit: &str) -> String {
    let inner = &lit[1..lit.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            assert!(chars.next().is_some(), "a quote is always doubled");
        }
    }
    out
}

#[test]
fn the_powershell_command_carries_a_directory_with_a_quote_back_exactly() {
    for dir in [
        "C:\\Users\\O'Brien\\AppData\\Roaming\\Luneterm\\shell-integration",
        "C:\\Users\\x\u{2019}y\\a",
        "/home/o'b/.config/Luneterm",
    ] {
        let script = format!("{dir}\\pwsh\\luneterm-integration.ps1");
        let cmd = pwsh_command(&script);
        let lit = cmd
            .strip_prefix("Invoke-Expression (Get-Content -Raw -Encoding UTF8 -LiteralPath ")
            .and_then(|r| r.strip_suffix(')'))
            .expect("the fixed form");
        assert!(lit.starts_with('\'') && lit.ends_with('\''));
        assert_eq!(pwsh_unquote(lit), script, "{dir}");
    }
}

#[test]
fn zsh_gets_zdotdir_and_the_users_directory_and_its_arguments_are_untouched() {
    let i = fake_install();
    let login =
        |env: &[(&str, &str)], args: &[&str]| augment(&i, None, &strs(args), &vars(env), Os::Macos);
    let a = login(
        &[("SHELL", "/bin/zsh"), ("HOME", abs("/Users/ann").as_str())],
        &[],
    )
    .expect("zsh");
    assert_eq!(a.program, None);
    assert!(a.args.is_empty());
    assert_eq!(value(&a.env, "ZDOTDIR"), Some(i.zsh_dir().as_str()));
    assert_eq!(
        value(&a.env, "LUNETERM_USER_ZDOTDIR"),
        Some(abs("/Users/ann").as_str())
    );
    assert_eq!(a.env.len(), 2);
    // The user's own ZDOTDIR wins over HOME.
    let a = login(
        &[
            ("SHELL", "/bin/zsh"),
            ("HOME", abs("/Users/ann").as_str()),
            ("ZDOTDIR", abs("/Users/ann/zdot").as_str()),
        ],
        &[],
    )
    .expect("zsh");
    assert_eq!(
        value(&a.env, "LUNETERM_USER_ZDOTDIR"),
        Some(abs("/Users/ann/zdot").as_str())
    );
    // An explicit zsh program keeps its arguments, whatever they are.
    let a = augment(
        &i,
        Some("/opt/zsh"),
        &strs(&["-l", "-f"]),
        &vars(&[("HOME", abs("/h").as_str())]),
        Os::Linux,
    )
    .expect("zsh");
    assert_eq!(a.args, ["-l", "-f"]);
    // Nothing to point the wrappers at, or not a path: no integration.
    assert_eq!(login(&[("SHELL", "/bin/zsh")], &[]), None);
    assert_eq!(
        login(&[("SHELL", "/bin/zsh"), ("HOME", "relative")], &[]),
        None
    );
    assert_eq!(login(&[("SHELL", "/bin/zsh"), ("HOME", "")], &[]), None);
    let invalid = |k: &str| match k {
        "SHELL" => Var::Set("/bin/zsh".into()),
        "ZDOTDIR" => Var::Invalid,
        "HOME" => Var::Set(abs("/h")),
        _ => Var::Unset,
    };
    assert_eq!(
        augment(&i, None, &[], &invalid, Os::Macos),
        None,
        "a ZDOTDIR that is not UTF-8 would point the wrappers at the wrong files"
    );
}

#[test]
fn bash_is_started_with_an_rcfile_only_for_the_default_arguments() {
    let i = fake_install();
    // The login shell: a program is named so arguments can be given.
    let a = augment(&i, None, &[], &vars(&[("SHELL", "/bin/bash")]), Os::Linux).expect("bash");
    assert_eq!(a.program, Some(PathBuf::from("/bin/bash")));
    assert_eq!(a.args, ["--rcfile", i.bash_rc().as_str(), "-i"]);
    assert!(a.env.is_empty());
    // An explicit bash with none or exactly -l: `-l` is dropped, the rc file emulates login.
    for args in [&[][..], &["-l"][..]] {
        let a = augment(&i, Some("bash"), &strs(args), &vars(&[]), Os::Macos).expect("bash");
        assert_eq!(a.program, None, "the program is the spawn's own");
        assert_eq!(a.args, ["--rcfile", i.bash_rc().as_str(), "-i"]);
    }
    // Any other arguments are the user's: untouched.
    for args in [&["-x"][..], &["-l", "-x"][..], &["--norc"][..]] {
        assert_eq!(
            augment(&i, Some("/bin/bash"), &strs(args), &vars(&[]), Os::Linux),
            None,
            "{args:?}"
        );
    }
}

#[test]
fn powershell_is_integrated_only_without_arguments() {
    let i = fake_install();
    for (program, os) in [
        ("pwsh.exe", Os::Windows),
        ("powershell.exe", Os::Windows),
        ("pwsh", Os::Macos),
    ] {
        let a = augment(&i, Some(program), &[], &vars(&[]), os).expect(program);
        assert_eq!(a.program, None);
        assert_eq!(a.args[..2], ["-NoExit", "-Command"]);
        assert_eq!(a.args.len(), 3);
        assert_eq!(a.args[2], pwsh_command(&i.pwsh_script()));
        assert!(a.env.is_empty());
        assert_eq!(
            augment(&i, Some(program), &strs(&["-NoProfile"]), &vars(&[]), os),
            None
        );
    }
}

#[test]
fn other_shells_and_a_missing_shell_degrade() {
    let i = fake_install();
    for program in ["/bin/sh", "/usr/bin/fish", "/bin/dash", "/bin/tcsh", ""] {
        assert_eq!(
            augment(&i, Some(program), &[], &vars(&[("HOME", "/h")]), Os::Linux),
            None,
            "{program:?}"
        );
    }
    // The login shell with no SHELL, an empty one, or one that is not UTF-8.
    assert_eq!(augment(&i, None, &[], &vars(&[]), Os::Linux), None);
    assert_eq!(
        augment(&i, None, &[], &vars(&[("SHELL", "")]), Os::Linux),
        None
    );
    assert_eq!(augment(&i, None, &[], &|_| Var::Invalid, Os::Linux), None);
    // Windows: the default is cmd; zsh and bash are not integrated there.
    assert_eq!(
        augment(&i, None, &[], &vars(&[("SHELL", "/bin/zsh")]), Os::Windows),
        None
    );
    assert_eq!(
        augment(
            &i,
            Some("zsh.exe"),
            &[],
            &vars(&[("HOME", "C:\\h")]),
            Os::Windows
        ),
        None
    );
    assert_eq!(
        augment(&i, Some("bash.exe"), &[], &vars(&[]), Os::Windows),
        None
    );
}

#[test]
fn the_scripts_are_luneterms_and_name_no_lunegit_identifier() {
    for (sub, name, text) in FILES {
        assert!(!text.is_empty(), "{sub}/{name}");
        for bad in ["LUNEGIT", "_lunegit", "lunegit-", "__lunegit"] {
            assert!(!text.contains(bad), "{sub}/{name} still has {bad}");
        }
        assert!(
            text.contains("# Source: Lunegit's TerminalShellIntegrator.cs"),
            "{sub}/{name}"
        );
        assert!(!text.contains('\r'), "{sub}/{name}: LF line endings");
    }
    assert!(ZSHRC.contains("LUNETERM_USER_ZDOTDIR") && ZSHENV.contains("LUNETERM_USER_ZDOTDIR"));
    assert!(BASHRC.contains("_luneterm_osc7") && PWSH.contains("__luneterm"));
    // The markers the app reads: OSC 7 (zsh, bash), OSC 9;9 (PowerShell) and OSC 133 C, D, A.
    assert!(ZSHRC.contains("]7;file://") && BASHRC.contains("]7;file://"));
    assert!(PWSH.contains("]9;9;"));
    for text in [ZSHRC, BASHRC, PWSH] {
        for mark in ["133;C", "133;D", "133;A"] {
            assert!(text.contains(mark), "{mark}");
        }
    }
}

/// `shell -n` over each script: the shell parses it and runs nothing. A shell that is absent is
/// reported, not failed.
#[test]
fn the_scripts_parse_in_their_shells_when_the_shells_are_here() {
    use std::process::Command;
    let dir = scratch("syntax");
    std::fs::create_dir_all(&dir).expect("create");
    let mut checked = 0;
    // zsh and bash are not integrated on Windows (and `bash -n` there takes POSIX paths).
    let shells: Vec<(&str, Vec<(&str, &str)>)> = if cfg!(windows) {
        Vec::new()
    } else {
        vec![
            (
                "zsh",
                vec![("zsh", ".zshenv"), ("zsh", ".zprofile"), ("zsh", ".zshrc")],
            ),
            ("bash", vec![("bash", "luneterm-rc.bash")]),
        ]
    };
    for (shell, files) in shells {
        for (sub, name) in files {
            let text = FILES
                .iter()
                .find(|(s, n, _)| *s == sub && *n == name)
                .map(|(_, _, t)| *t)
                .expect("a script");
            let file = dir.join(format!("{sub}-{}", name.trim_start_matches('.')));
            std::fs::write(&file, text).expect("write");
            match Command::new(shell).arg("-n").arg(&file).output() {
                Ok(out) => {
                    assert!(
                        out.status.success(),
                        "{shell} -n {sub}/{name}: {}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    checked += 1;
                }
                Err(e) => eprintln!(
                    "skipped {shell} -n {sub}/{name}: {shell} not run: {:?}",
                    e.kind()
                ),
            }
        }
    }
    // PowerShell: the script is parsed as a script block (nothing runs).
    let ps1 = dir.join("integration.ps1");
    std::fs::write(&ps1, PWSH).expect("write");
    let probe = format!(
        "$e = $null; $null = [System.Management.Automation.Language.Parser]::ParseFile({}, [ref]$null, [ref]$e); if ($e.Count -gt 0) {{ $e | ForEach-Object {{ $_.Message }}; exit 1 }}",
        pwsh_literal(ps1.to_str().expect("utf-8"))
    );
    match Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", &probe])
        .output()
    {
        Ok(out) => {
            assert!(
                out.status.success(),
                "pwsh parse of the script: {}",
                String::from_utf8_lossy(&out.stdout)
            );
            checked += 1;
        }
        Err(e) => eprintln!("skipped the pwsh parse: pwsh not run: {:?}", e.kind()),
    }
    eprintln!("syntax-checked {checked} scripts");
    std::fs::remove_dir_all(&dir).expect("remove");
}

// ------------------------------------------------------------------------ installing (Unix)

/// A scratch directory name no earlier run left behind (the caller creates it).
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("luneterm-si-{}-{tag}", std::process::id()));
    assert!(!dir.exists(), "{} already exists", dir.display());
    dir
}

#[cfg(unix)]
mod fs {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    use super::*;

    fn mode(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).expect("stat").mode() & 0o777
    }

    #[test]
    fn install_creates_private_directories_and_private_files_with_the_scripts() {
        let cfg = scratch("fresh");
        let i = install(&cfg).expect("installs");
        let root = cfg.join(DIR_NAME);
        assert_eq!(i.root(), root.to_str().expect("utf-8"));
        for d in [
            root.clone(),
            root.join("zsh"),
            root.join("bash"),
            root.join("pwsh"),
        ] {
            assert_eq!(mode(&d), 0o700, "{}", d.display());
        }
        for (sub, name, text) in FILES {
            let f = root.join(sub).join(name);
            assert_eq!(mode(&f), 0o600, "{}", f.display());
            assert_eq!(std::fs::read_to_string(&f).expect("read"), text);
        }
        // Again: nothing is rewritten (the files keep their inode).
        let ino = |p: PathBuf| std::fs::metadata(p).expect("stat").ino();
        let before = ino(root.join("zsh").join(".zshrc"));
        install(&cfg).expect("installs again");
        assert_eq!(ino(root.join("zsh").join(".zshrc")), before);
        std::fs::remove_dir_all(&cfg).expect("remove");
    }

    #[test]
    fn a_changed_file_is_replaced_atomically_and_a_loose_one_is_made_private() {
        let cfg = scratch("changed");
        install(&cfg).expect("installs");
        let rc = cfg.join(DIR_NAME).join("bash").join("luneterm-rc.bash");
        std::fs::write(&rc, "echo tampered\n").expect("write");
        install(&cfg).expect("installs");
        assert_eq!(std::fs::read_to_string(&rc).expect("read"), BASHRC);
        // The same text but group-writable: replaced by a private file.
        std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o664)).expect("chmod");
        install(&cfg).expect("installs");
        assert_eq!(mode(&rc), 0o600);
        std::fs::remove_dir_all(&cfg).expect("remove");
    }

    #[test]
    fn a_symlink_at_a_script_path_is_replaced_and_what_it_pointed_at_is_untouched() {
        let cfg = scratch("link-file");
        install(&cfg).expect("installs");
        let target = cfg.join("victim.txt");
        std::fs::write(&target, "keep me\n").expect("write");
        let zshrc = cfg.join(DIR_NAME).join("zsh").join(".zshrc");
        std::fs::remove_file(&zshrc).expect("rm");
        symlink(&target, &zshrc).expect("link");
        install(&cfg).expect("installs");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "keep me\n");
        let meta = std::fs::symlink_metadata(&zshrc).expect("stat");
        assert!(
            meta.file_type().is_file(),
            "the link was replaced by a file"
        );
        assert_eq!(std::fs::read_to_string(&zshrc).expect("read"), ZSHRC);
        std::fs::remove_dir_all(&cfg).expect("remove");
    }

    #[test]
    fn a_symlink_a_loose_mode_or_a_file_where_a_directory_belongs_skips_integration() {
        // The root is a symlink to a real directory.
        let cfg = scratch("link-root");
        std::fs::create_dir_all(&cfg).expect("create");
        let real = cfg.join("real");
        std::fs::create_dir(&real).expect("create");
        symlink(&real, cfg.join(DIR_NAME)).expect("link");
        assert_eq!(install(&cfg), Err(Skip::Unsafe(Unsafe::NotADirectory)));
        assert!(
            std::fs::read_dir(&real).expect("read").next().is_none(),
            "nothing was written through the link"
        );
        std::fs::remove_dir_all(&cfg).expect("remove");

        // A subdirectory is a symlink.
        let cfg = scratch("link-sub");
        install(&cfg).expect("installs");
        let sub = cfg.join(DIR_NAME).join("bash");
        std::fs::remove_dir_all(&sub).expect("rm");
        let elsewhere = cfg.join("elsewhere");
        std::fs::create_dir(&elsewhere).expect("create");
        symlink(&elsewhere, &sub).expect("link");
        assert_eq!(install(&cfg), Err(Skip::Unsafe(Unsafe::NotADirectory)));
        assert!(
            std::fs::read_dir(&elsewhere)
                .expect("read")
                .next()
                .is_none()
        );
        std::fs::remove_dir_all(&cfg).expect("remove");

        // The root is group-writable.
        let cfg = scratch("loose");
        install(&cfg).expect("installs");
        let root = cfg.join(DIR_NAME);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o770)).expect("chmod");
        assert_eq!(install(&cfg), Err(Skip::Unsafe(Unsafe::Writable)));
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        // A subdirectory writable by others.
        let zsh = root.join("zsh");
        std::fs::set_permissions(&zsh, std::fs::Permissions::from_mode(0o757)).expect("chmod");
        assert_eq!(install(&cfg), Err(Skip::Unsafe(Unsafe::Writable)));
        std::fs::remove_dir_all(&cfg).expect("remove");

        // A file where the root belongs.
        let cfg = scratch("file-root");
        std::fs::create_dir_all(&cfg).expect("create");
        std::fs::write(cfg.join(DIR_NAME), "x").expect("write");
        assert_eq!(install(&cfg), Err(Skip::Unsafe(Unsafe::NotADirectory)));
        std::fs::remove_dir_all(&cfg).expect("remove");
    }

    #[test]
    fn a_directory_that_is_not_ours_is_refused_by_the_real_check() {
        // `check_dir` against the real facts of a directory we made, with another euid.
        let cfg = scratch("owner");
        std::fs::create_dir_all(&cfg).expect("create");
        let facts = platform::facts(&cfg).expect("stat");
        let me = platform::euid();
        assert_eq!(check_dir(&facts, me), Ok(()));
        assert_eq!(check_dir(&facts, me.wrapping_add(1)), Err(Unsafe::NotOwned));
        std::fs::remove_dir_all(&cfg).expect("remove");
    }
}

#[test]
fn a_config_path_with_a_control_character_is_not_used() {
    assert_eq!(install(Path::new("/tmp/a\nb")), Err(Skip::Path));
    assert_eq!(install(Path::new("/tmp/a\u{1b}b")), Err(Skip::Path));
}
