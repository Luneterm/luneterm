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
