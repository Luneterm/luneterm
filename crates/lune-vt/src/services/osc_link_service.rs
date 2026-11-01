//! Port of xterm.js src/common/services/OscLinkService.ts @ f447274 (tests:
//! OscLinkService.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D1):
//!  - Upstream holds `IMarker` objects and registers `marker.onDispose(() => _removeMarkerFromLink)`
//!    on each. Markers here are [`MarkerId`]s in the buffer's [`MarkerSet`], and disposal is a
//!    log the buffer keeps. **Host duty (M6):** after anything that can dispose markers (a scroll
//!    that trims, `clear_markers`, resize, activating the normal buffer), drain
//!    `Buffer::take_disposed_markers()` of *both* buffers of the [`BufferService`] and pass the
//!    ids to [`OscLinkService::on_markers_disposed`], before the next call to this service.
//!    Once that is done the state equals upstream's. Ids the service does not know are ignored,
//!    so it is safe to forward everything.
//!  - Upstream's service reads `bufferService.buffer` (the active buffer) in `registerLink` and
//!    `addLineToLink`; here the [`BufferService`] is a parameter. A marker lives in the buffer
//!    that was active when it was added, so each entry remembers which ([`LinkMarker::alt`]):
//!    that is how `addLineToLink` reads a marker's `line` and which buffer's disposal log holds
//!    its id.
//!  - `registerLink` returns the link id, a `u32` (it is stored in a cell's `urlId`, a 32-bit
//!    field). Ids start at 1. After 2^32 - 1 registrations the counter wraps to 1 (upstream's
//!    JS number keeps counting but `urlId` would truncate it as well); an id still in use is not
//!    skipped.
//!  - `_entriesWithId` keys are upstream's `` `${id};;${uri}` `` strings, so a `;;` inside an id
//!    or uri can make two different links share a key, as upstream.

use std::collections::HashMap;

use crate::buffer::marker::MarkerId;
use crate::services::buffer_service::BufferService;

/// Upstream `IOscLinkData`: the `id=` parameter of the OSC 8 sequence (if any) and its URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OscLinkData {
    pub id: Option<String>,
    pub uri: String,
}

/// A marker that keeps a link alive, and the buffer that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkMarker {
    pub id: MarkerId,
    /// The marker is in the alternate buffer (else the normal one).
    pub alt: bool,
}

#[derive(Debug)]
struct Entry {
    data: OscLinkData,
    /// `Some` for a link with an id: the `_entriesWithId` key.
    key: Option<String>,
    lines: Vec<LinkMarker>,
}

/// Maps OSC 8 hyperlinks to the link ids stored in cells and frees them when the lines that
/// carry them are trimmed off the buffer.
#[derive(Debug)]
pub struct OscLinkService {
    next_id: u32,
    /// The link key to the link id; used to add further lines to links with ids.
    entries_with_id: HashMap<String, u32>,
    /// The link id (a number, not to be confused with the string `id=` of the sequence) to its
    /// entry.
    data_by_link_id: HashMap<u32, Entry>,
    /// The link each marker keeps alive (upstream: the closure of `marker.onDispose`).
    link_of_marker: HashMap<MarkerId, u32>,
}

impl Default for OscLinkService {
    fn default() -> Self {
        Self::new()
    }
}

impl OscLinkService {
    pub fn new() -> Self {
        OscLinkService {
            next_id: 1,
            entries_with_id: HashMap::new(),
            data_by_link_id: HashMap::new(),
            link_of_marker: HashMap::new(),
        }
    }

    /// Registers a link, returning the link id. The link data is managed by this service and is
    /// freed when the current cursor position (and every line added with
    /// [`add_line_to_link`](Self::add_line_to_link)) is trimmed off the buffer.
    pub fn register_link(&mut self, buffers: &mut BufferService, data: OscLinkData) -> u32 {
        let alt = buffers.buffers.is_alt_active();
        let buffer = buffers.buffer_mut();
        let y = to_i64(buffer.ybase).saturating_add(to_i64(buffer.y));

        // Links with no id will only ever be registered a single time
        let Some(id) = data.id.clone() else {
            let marker = buffer.add_marker(y);
            let link_id = self.take_id();
            self.link_of_marker.insert(marker, link_id);
            self.data_by_link_id.insert(
                link_id,
                Entry {
                    data,
                    key: None,
                    lines: vec![LinkMarker { id: marker, alt }],
                },
            );
            return link_id;
        };

        // Add the line to the link if it already exists
        let key = entry_id_key(&id, &data.uri);
        if let Some(&link_id) = self.entries_with_id.get(&key) {
            self.add_line_to_link(buffers, link_id, usize::try_from(y).unwrap_or(0));
            return link_id;
        }

        // Create the link
        let marker = buffer.add_marker(y);
        let link_id = self.take_id();
        self.link_of_marker.insert(marker, link_id);
        self.entries_with_id.insert(key.clone(), link_id);
        self.data_by_link_id.insert(
            link_id,
            Entry {
                data,
                key: Some(key),
                lines: vec![LinkMarker { id: marker, alt }],
            },
        );
        link_id
    }

    /// Adds line `y` of the active buffer to a link, if the link is known and does not hold
    /// that line yet.
    pub fn add_line_to_link(&mut self, buffers: &mut BufferService, link_id: u32, y: usize) {
        let Some(entry) = self.data_by_link_id.get_mut(&link_id) else {
            return;
        };
        let y = to_i64(y);
        let holds_line = entry.lines.iter().any(|m| {
            // A marker disposed but not yet forwarded is already gone upstream.
            let buffer = if m.alt {
                buffers.buffers.alt()
            } else {
                buffers.buffers.normal()
            };
            buffer.marker(m.id).is_some_and(|marker| marker.line == y)
        });
        if !holds_line {
            let alt = buffers.buffers.is_alt_active();
            let marker = buffers.buffer_mut().add_marker(y);
            entry.lines.push(LinkMarker { id: marker, alt });
            self.link_of_marker.insert(marker, link_id);
        }
    }

    /// The data of a link id, `None` if unknown or freed.
    pub fn get_link_data(&self, link_id: u32) -> Option<&OscLinkData> {
        self.data_by_link_id.get(&link_id).map(|entry| &entry.data)
    }

    /// Upstream `_removeMarkerFromLink` for each disposed marker (the host forwards
    /// `Buffer::take_disposed_markers()`; see the module notes). A link whose last marker is
    /// gone is freed.
    pub fn on_markers_disposed(&mut self, ids: &[MarkerId]) {
        for &marker in ids {
            let Some(link_id) = self.link_of_marker.remove(&marker) else {
                continue;
            };
            let Some(entry) = self.data_by_link_id.get_mut(&link_id) else {
                continue;
            };
            let Some(index) = entry.lines.iter().position(|m| m.id == marker) else {
                continue;
            };
            entry.lines.remove(index);
            if entry.lines.is_empty() {
                if let Some(key) = entry.key.take() {
                    self.entries_with_id.remove(&key);
                }
                self.data_by_link_id.remove(&link_id);
            }
        }
    }

    /// The markers that keep a link alive, in the order they were added.
    pub fn link_markers(&self, link_id: u32) -> Option<&[LinkMarker]> {
        self.data_by_link_id
            .get(&link_id)
            .map(|entry| entry.lines.as_slice())
    }

    /// The ids of the links that are alive (unordered).
    pub fn link_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.data_by_link_id.keys().copied()
    }

    /// The number of links alive.
    pub fn len(&self) -> usize {
        self.data_by_link_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data_by_link_id.is_empty()
    }

    /// The number of markers held by all links.
    pub fn marker_count(&self) -> usize {
        self.link_of_marker.len()
    }

    fn take_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        id
    }
}

fn entry_id_key(id: &str, uri: &str) -> String {
    format!("{id};;{uri}")
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "osc_link_service_tests.rs"]
mod tests;
