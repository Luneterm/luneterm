//! The channel a copy follows (auto-update plan step 9, Q17).
//!
//! A lunepack pack carries the channel it was published on, and an update replaces the tree's
//! update config with the new pack's. A beta user who receives a stable pack (every stable tag is
//! also added to beta, D3) would then follow stable. So the first run of an installed copy records
//! the channel its tree was installed with in the settings (`update_channel`), and every
//! [`Updater`](super::Updater) is built with that channel, whatever the tree's config says later.

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
    /// The settings must be written with `channel` (it was empty or garbled).
    pub rewrite: bool,
}

/// [`resolve`] for this launch: reads the installed tree's channel only when `launch` found an
/// install.
pub fn settle(launch: &Launch, persisted: &str) -> Settled {
    let installed = launch.is_installed().then(installed_channel).flatten();
    resolve(persisted, installed.as_deref())
}

/// The persisted channel when it is a valid channel name, else the installed tree's (and the
/// settings are rewritten with it). Not installed (`installed` is `None`): nothing is followed and
/// nothing is written, whatever the settings hold.
pub fn resolve(persisted: &str, installed: Option<&str>) -> Settled {
    let Some(installed) = installed else {
        return Settled {
            channel: None,
            rewrite: false,
        };
    };
    if Channel::parse(persisted).is_ok() {
        Settled {
            channel: Some(persisted.to_owned()),
            rewrite: false,
        }
    } else {
        Settled {
            channel: Some(installed.to_owned()),
            rewrite: true,
        }
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
