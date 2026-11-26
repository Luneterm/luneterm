//! The channel a copy follows (auto-update plan step 9, Q17).
//!
//! A lunepack pack carries the channel it was published on, and an update replaces the tree's
//! update config with the new pack's. A beta user who receives a stable pack (every stable tag is
//! also added to beta, D3) would then follow stable. So the first run of an installed copy records
//! the channel its tree was installed with in the settings (`update_channel`), and every
//! [`Updater`](super::Updater) is built with that channel, whatever the tree's config says later.
//!
//! A reinstall from another channel's setup must reset it, and from the outside it looks the same
//! as an update (the tree's channel differs from the followed one). They are told apart by what
//! the tree said when it was last known good: `update_tree_channel`. The first launch of an
//! updated version (`pending`) only *defers* writing the tree's new channel there, until
//! `confirm_healthy` has run (a version that rolls back never records it); any later launch that
//! finds the tree saying something else than recorded was not reached by an update, it is a fresh
//! setup, and the copy follows that setup's channel.

use lunepack::{Channel, Installation};

use super::Launch;

/// The channel the running tree's update config names; `None` when this copy is not installed or
/// the config cannot be read. Local files only.
pub fn installed_channel() -> Option<String> {
    let inst = Installation::from_current_exe().ok()?;
    Some(inst.config().channel.as_str().to_owned())
}

/// What the app does with the channel at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    /// The channel the updater follows; `None` when this copy cannot update itself (nothing is
    /// recorded for it).
    pub channel: Option<String>,
    /// The tree's channel to record in `update_tree_channel`, when it is not what is recorded.
    pub tree: Option<String>,
    /// `tree` is recorded once the version confirmed itself healthy, not now (this launch is the
    /// first of an updated version).
    pub tree_after_confirm: bool,
    /// The settings must be saved now: `channel` differs from the persisted one, or `tree` is to be
    /// recorded now.
    pub rewrite: bool,
}

impl Settled {
    const NOT_INSTALLED: Settled = Settled {
        channel: None,
        tree: None,
        tree_after_confirm: false,
        rewrite: false,
    };
}

/// [`resolve`] for this launch: reads the installed tree's channel only when `launch` found an
/// install.
pub fn settle(launch: &Launch, followed: &str, recorded_tree: &str) -> Settled {
    let installed = launch.is_installed().then(installed_channel).flatten();
    resolve(
        followed,
        recorded_tree,
        installed.as_deref(),
        launch.pending(),
    )
}

/// The channel to follow: the persisted `followed` one when it is a valid channel name and this is
/// not a fresh setup of another channel, else the installed tree's (and the settings are
/// rewritten). A fresh setup is a launch that is not `pending` (just updated to) where the tree
/// names another channel than `recorded_tree`; an empty or garbled `recorded_tree` is not known,
/// so it never resets. Not installed (`installed` is `None`): nothing is followed and nothing is
/// written, whatever the settings hold.
pub fn resolve(
    followed: &str,
    recorded_tree: &str,
    installed: Option<&str>,
    pending: bool,
) -> Settled {
    let Some(installed) = installed else {
        return Settled::NOT_INSTALLED;
    };
    let recorded = Channel::parse(recorded_tree)
        .is_ok()
        .then_some(recorded_tree);
    let fresh_setup = !pending && recorded.is_some_and(|r| r != installed);
    let channel = if Channel::parse(followed).is_ok() && !fresh_setup {
        followed
    } else {
        installed
    };
    let record_tree = recorded != Some(installed);
    Settled {
        channel: Some(channel.to_owned()),
        tree: record_tree.then(|| installed.to_owned()),
        tree_after_confirm: record_tree && pending,
        rewrite: channel != followed || (record_tree && !pending),
    }
}

/// `builder` following `channel` when it names a valid channel, else the one the builder has.
pub(super) fn follow(
    builder: lunepack::UpdaterBuilder,
    channel: Option<&str>,
) -> lunepack::UpdaterBuilder {
    match channel.map(Channel::parse) {
        Some(Ok(channel)) => builder.channel(channel),
        _ => builder,
    }
}
