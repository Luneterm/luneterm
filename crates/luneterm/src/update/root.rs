//! The root of trust compiled into Luneterm: the update feed is accepted only when its signature
//! chain verifies from these keys (lunepack FORMAT.md 5.1).
//!
//! **This is a TEST root** (auto-update plan D9): its public key belongs to a throwaway Ed25519 key
//! whose private half was discarded when it was made (2026-11-26), so no feed can be signed for it
//! and an installed copy built with it never accepts an update. It is replaced by the user's real
//! root public key (generated offline, docs/updates.md) before the first release with updates;
//! the publish job refuses to run while `IS_TEST_ROOT` holds (plan step 6), and
//! `packaging/lunepack.toml` must name the same keys (plan step 5).

/// The lunepack app id of Luneterm (`packaging/lunepack.toml` `app_id`).
pub const APP_ID: &str = "my.ijat.luneterm";

/// The root version these keys are (FORMAT.md 5.1: `1 ..= 2^53 - 1`).
pub const ROOT_VERSION: u64 = 1;

/// How many of `ROOT_KEYS` must sign a root or key-set document.
pub const ROOT_THRESHOLD: u64 = 1;

/// The root's Ed25519 public keys (32 raw bytes each).
pub const ROOT_KEYS: &[[u8; 32]] = &[TEST_ROOT_KEY];

/// Whether `ROOT_KEYS` is the throwaway test root (no release may ship with it).
pub const IS_TEST_ROOT: bool = true;

/// The throwaway test key: `7c4f44f5…f998e996` (private half discarded).
const TEST_ROOT_KEY: [u8; 32] = [
    0x7c, 0x4f, 0x44, 0xf5, 0x4d, 0xaf, 0x49, 0xca, 0x87, 0x3f, 0x3a, 0xac, 0x93, 0x7f, 0x0f, 0x19,
    0xe6, 0x68, 0x36, 0x4a, 0x01, 0x17, 0x8a, 0xa3, 0x39, 0xf6, 0x46, 0x90, 0xf9, 0x98, 0xe9, 0x96,
];

/// The compiled-in root as lunepack's `TrustedRoot` (checked like a root document).
pub fn trusted_root() -> Result<lunepack::TrustedRoot, lunepack::Error> {
    let app_id = lunepack::AppId::parse(APP_ID)
        .map_err(|_| lunepack::Error::Config("the compiled-in app id is not valid"))?;
    lunepack::trusted_root(app_id, ROOT_VERSION, ROOT_THRESHOLD, ROOT_KEYS)
}

/// Whether an installed copy's update config names exactly the compiled-in root: the same app id,
/// root version, threshold and key set (order aside). The updater verifies with the config's root,
/// so it is used only when it equals this one.
pub fn matches(app_id: &str, version: u64, threshold: u64, keys: &[[u8; 32]]) -> bool {
    let mut ours: Vec<[u8; 32]> = ROOT_KEYS.to_vec();
    let mut theirs: Vec<[u8; 32]> = keys.to_vec();
    ours.sort_unstable();
    theirs.sort_unstable();
    app_id == APP_ID && version == ROOT_VERSION && threshold == ROOT_THRESHOLD && ours == theirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_in_root_is_a_valid_trusted_root() {
        let root = trusted_root().expect("valid root");
        assert_eq!(root.app_id().as_str(), APP_ID);
        assert_eq!(root.version(), ROOT_VERSION);
    }

    /// The release build's `packaging/lunepack-<os>.toml` files, as text.
    const CONFIGS: [(&str, &str); 3] = [
        (
            "linux",
            include_str!("../../../../packaging/lunepack-linux.toml"),
        ),
        (
            "windows",
            include_str!("../../../../packaging/lunepack-windows.toml"),
        ),
        (
            "macos",
            include_str!("../../../../packaging/lunepack-macos.toml"),
        ),
    ];

    /// Reads the small TOML subset of those files: `[section]` headers, `key = "text"`,
    /// `key = number`, `key = true|false` and multi-line `key = [ "text", ]` lists, as
    /// `("section.key", value)` with a list's items joined by a comma. Comments are cut.
    fn read_config(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let (mut section, mut list): (String, Option<(String, Vec<String>)>) =
            (String::new(), None);
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some((key, items)) = &mut list {
                if line == "]" {
                    out.push((key.clone(), items.join(",")));
                    list = None;
                } else {
                    items.push(line.trim_end_matches(',').trim_matches('"').to_owned());
                }
            } else if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_owned();
            } else if let Some((key, value)) = line.split_once('=') {
                let (key, value) = (key.trim(), value.trim());
                let full = if section.is_empty() {
                    key.to_owned()
                } else {
                    format!("{section}.{key}")
                };
                if value == "[" {
                    list = Some((full, Vec::new()));
                } else {
                    out.push((full, value.trim_matches('"').to_owned()));
                }
            } else {
                panic!("a line this reader does not know: {line}");
            }
        }
        out
    }

    fn hex(key: &[u8; 32]) -> String {
        key.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The root every `packaging/lunepack-<os>.toml` names is the root compiled in here (app id,
    /// version, threshold, keys), so a copy installed from the feed accepts what the feed signs and
    /// the other way round; they also agree on the feed and the signer, and hold no key.
    #[test]
    fn the_packaging_configs_name_the_compiled_in_root() {
        let keys: Vec<String> = ROOT_KEYS.iter().map(hex).collect();
        for (os, text) in CONFIGS {
            let c = read_config(text);
            let get = |k: &str| {
                let found: Vec<&str> = c
                    .iter()
                    .filter(|(key, _)| key == k)
                    .map(|(_, v)| v.as_str())
                    .collect();
                assert_eq!(found.len(), 1, "{os}: {k} must be set exactly once");
                found[0].to_owned()
            };
            assert_eq!(get("app_id"), APP_ID, "{os}");
            assert_eq!(get("root.version"), ROOT_VERSION.to_string(), "{os}");
            assert_eq!(get("root.threshold"), ROOT_THRESHOLD.to_string(), "{os}");
            assert_eq!(get("root.keys"), keys.join(","), "{os}");
            assert_eq!(get("update.feed"), "https://updates.izzat.dev/luneterm/");
            assert_eq!(get("update.channel"), "stable", "{os}");
            assert_eq!(get("signers.release.env"), "LUNEPACK_RELEASE_KEY", "{os}");
            // No signer holds a key file or runs a command: the release key is in the environment.
            assert!(
                c.iter()
                    .all(|(k, _)| !k.starts_with("signers.") || k == "signers.release.env"),
                "{os}"
            );
            assert!(!text.contains("PRIVATE") && !text.contains("BEGIN"), "{os}");
        }
    }

    /// The entry and the icons of each OS (plan Revision 3): the files they name exist.
    #[test]
    fn the_packaging_configs_name_each_oss_entry_and_icons() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let want = [
            ("linux", "luneterm", "install.icon", "luneterm-256.png"),
            (
                "windows",
                "luneterm.exe",
                "windows.icon",
                "../assets/icon/luneterm.ico",
            ),
            (
                "macos",
                "Contents/MacOS/luneterm",
                "macos.icon",
                "../assets/icon/Luneterm.icns",
            ),
        ];
        for ((os, text), (os2, entry, icon_key, icon)) in CONFIGS.into_iter().zip(want) {
            assert_eq!(os, os2);
            let c = read_config(text);
            let get = |k: &str| c.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
            assert_eq!(get("install.entry").as_deref(), Some(entry), "{os}");
            assert_eq!(get("install.name").as_deref(), Some("Luneterm"), "{os}");
            assert_eq!(get(icon_key).as_deref(), Some(icon), "{os}");
            // An icon of a config is relative to packaging/; the Linux one is a file of the tree,
            // staged from assets/icon by the build job.
            let file = if os == "linux" {
                repo.join("assets/icon").join(icon)
            } else {
                repo.join("packaging").join(icon)
            };
            assert!(file.is_file(), "{os}: {}", file.display());
        }
    }

    #[test]
    fn matches_needs_every_part_equal() {
        let key = ROOT_KEYS[0];
        assert!(matches(APP_ID, ROOT_VERSION, ROOT_THRESHOLD, &[key]));
        assert!(!matches(
            "org.example.other",
            ROOT_VERSION,
            ROOT_THRESHOLD,
            &[key]
        ));
        assert!(!matches(APP_ID, ROOT_VERSION + 1, ROOT_THRESHOLD, &[key]));
        assert!(!matches(APP_ID, ROOT_VERSION, ROOT_THRESHOLD + 1, &[key]));
        let mut other = key;
        other[0] ^= 1;
        assert!(!matches(APP_ID, ROOT_VERSION, ROOT_THRESHOLD, &[other]));
        assert!(!matches(
            APP_ID,
            ROOT_VERSION,
            ROOT_THRESHOLD,
            &[key, other]
        ));
        assert!(!matches(APP_ID, ROOT_VERSION, ROOT_THRESHOLD, &[]));
    }
}
