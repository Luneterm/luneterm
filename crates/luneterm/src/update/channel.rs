//! The channel a copy follows (auto-update plan step 9, Q17).
//!
//! A lunepack pack carries the channel it was published on, and an update replaces the tree's
//! update config with the new pack's. A beta user who receives a stable pack (every stable tag is
//! also added to beta, D3) would then follow stable. So the first run of an installed copy records
//! the channel its tree was installed with in the settings (`update_channel`), and every
//! [`Updater`](super::Updater) is built with that channel, whatever the tree's config says later.
//!
//! A setup file of another channel must reset it: `settings.json` outlives an uninstall, so a
//! copy installed from the stable setup after a beta one would otherwise go on following beta. An
//! update and a setup are told apart by lunepack itself, not by the tree's channel (a beta copy's
//! tree names stable after every stable pack): a setup's first install, or its reinstall over a
//! removed tree, queues an `Installed` event that [`lunepack::startup`] delivers once
//! ([`Launch::fresh_install`]); an update queues `Updated`. On that launch the tree's channel is
//! recorded whatever was persisted. A per-machine install queues no events: it records on its
//! first run only, like before (docs/updates.md).

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
    /// The settings must be written with `channel` (it differs from the persisted one).
    pub rewrite: bool,
}

/// [`resolve`] for this launch: reads the installed tree's channel only when `launch` found an
/// install.
pub fn settle(launch: &Launch, persisted: &str) -> Settled {
    let installed = launch.is_installed().then(installed_channel).flatten();
    resolve(persisted, installed.as_deref(), launch.fresh_install())
}

/// The channel to follow: the installed tree's on the first start after a setup file installed
/// this copy (`fresh_install`), else the persisted one when it is a valid channel name, else the
/// tree's. The settings are rewritten when that differs from what was persisted. Not installed
/// (`installed` is `None`): nothing is followed and nothing is written, whatever the settings
/// hold.
pub fn resolve(persisted: &str, installed: Option<&str>, fresh_install: bool) -> Settled {
    let Some(installed) = installed else {
        return Settled {
            channel: None,
            rewrite: false,
        };
    };
    let channel = if !fresh_install && Channel::parse(persisted).is_ok() {
        persisted
    } else {
        installed
    };
    Settled {
        channel: Some(channel.to_owned()),
        rewrite: channel != persisted,
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
