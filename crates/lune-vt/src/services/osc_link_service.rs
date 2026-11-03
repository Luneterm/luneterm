//! Port of xterm.js src/common/services/OscLinkService.ts @ f447274 (tests:
//! OscLinkService.test.ts).
//!
//! Rust notes (see `docs/porting-xterm.md` and plan M5 D1):
//!  - Upstream holds `IMarker` objects and registers `marker.onDispose(() => _removeMarkerFromLink)`
//!    on each. Markers here are [`MarkerId`]s in the buffer's [`MarkerSet`], and disposal is a
//!    log the buffer keeps. **Host duty (done by `InputHandler::forward_disposed_markers`, which
//!    `CoreTerminal` calls after every public mutation):** after anything that can dispose markers (a scroll
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
//!    JS number keeps counting but `urlId` would truncate it as well). `// LUNETERM:` (review
//!    F5) an id still in use is skipped, so a wrap never replaces a live link (the caps keep at
//!    most `MAX_LINKS + 1` ids in use, so the search ends); ids after a wrap differ from
//!    upstream's, which would overwrite the live entry.
//!  - `// LUNETERM:` (review S2) upstream frees a link only when every line it marks is trimmed, so
//!    a program that repeats `ESC ] 8 ; ; uri BEL` on one line grows the table (and the
//!    buffer's marker set) without bound. Here live links are capped at [`MAX_LINKS`] and the
//!    markers they hold at [`MAX_LINK_MARKERS`].
//!    Past either cap the least recently extended link (registered, or given a new line) is
//!    evicted: its markers are disposed and its data dropped, so cells that still carry its id
//!    keep their attributes but [`OscLinkService::get_link_data`] returns `None` for them (they
//!    render as plain text). The link being registered or extended is never the one evicted,
//!    so one link open across the whole buffer keeps every line (one marker per line).
//!  - `_entriesWithId` keys are upstream's `` `${id};;${uri}` `` strings, so a `;;` inside an id
//!    or uri can make two different links share a key, as upstream.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::buffer::marker::MarkerId;
use crate::services::buffer_service::BufferService;

/// LUNETERM: most links alive at once; registering one more evicts the least recently extended
/// (module notes, review S2). Far above what real output keeps in a buffer: `ls --hyperlink`
/// makes one link per file name, and the default buffer holds about 1000 lines.
pub const MAX_LINKS: usize = 4096;

/// LUNETERM: most markers all links hold, not counting the link being registered or extended
/// (module notes, review S2). It bounds the buffer's marker set (and the link-list removals of
/// a bulk disposal); one link that spans many lines holds one marker per line.
pub const MAX_LINK_MARKERS: usize = 8192;

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
    /// In the order the markers were added, which is ascending id (ids only grow).
    /// LUNETERM: (review S7 residual 1) a `VecDeque`: a trim disposes a link's oldest marker,
    /// which is the front, so removing it is O(1) where a `Vec` moved every other marker.
    lines: VecDeque<LinkMarker>,
    /// LUNETERM: the marker [`OscLinkService::add_line_to_link`] last found on, or added to,
    /// the line it was asked about. `print` asks once per character for the cursor line, so
    /// checking this marker first answers almost every call with one lookup instead of a walk
    /// over every marker of the link (a link open across the buffer holds one per line;
    /// review F1/S1). The answer is the same as the walk's: a hit is a live marker of this
    /// link on that line.
    last: Option<LinkMarker>,
    /// LUNETERM: the key of this entry in [`OscLinkService::by_age`].
    age: u64,
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
    /// LUNETERM: live links, least recently extended first (eviction order; module notes).
    by_age: BTreeMap<u64, u32>,
    next_age: u64,
    /// Markers `add_line_to_link` looked at (tests pin that it stays O(1) per call on the
    /// same line).
    #[cfg(test)]
    pub(crate) marker_visits: u64,
    /// Entries of links' marker lists `on_markers_disposed` looked at (tests pin one pass per
    /// affected link per batch).
    #[cfg(test)]
    pub(crate) line_visits: u64,
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
            by_age: BTreeMap::new(),
            // 0 is the placeholder of an entry not yet touched, never a key
            next_age: 1,
            #[cfg(test)]
            marker_visits: 0,
            #[cfg(test)]
            line_visits: 0,
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
                    lines: VecDeque::from([LinkMarker { id: marker, alt }]),
                    last: Some(LinkMarker { id: marker, alt }),
                    age: 0,
                },
            );
            self.touch(link_id);
            self.enforce_caps(buffers, link_id);
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
                lines: VecDeque::from([LinkMarker { id: marker, alt }]),
                last: Some(LinkMarker { id: marker, alt }),
                age: 0,
            },
        );
        self.touch(link_id);
        self.enforce_caps(buffers, link_id);
        link_id
    }

    /// Adds line `y` of the active buffer to a link, if the link is known and does not hold
    /// that line yet.
    pub fn add_line_to_link(&mut self, buffers: &mut BufferService, link_id: u32, y: usize) {
        let Some(entry) = self.data_by_link_id.get_mut(&link_id) else {
            return;
        };
        let y = to_i64(y);
        #[cfg(test)]
        let visits = &mut self.marker_visits;
        #[cfg(test)]
        let mut visit = |n: usize| *visits += n as u64;
        #[cfg(not(test))]
        let visit = |_: usize| {};
        let (normal, alt) = (buffers.buffers.normal(), buffers.buffers.alt());
        let on_line = |m: &LinkMarker| {
            // A marker disposed but not yet forwarded is already gone upstream.
            let buffer = if m.alt { alt } else { normal };
            buffer.marker(m.id).is_some_and(|marker| marker.line == y)
        };
        // LUNETERM: (review F1/S1, S7 residual 1) upstream walks every marker of the link, each
        // a lookup here. The same answer, cheaper: the cached marker first (see `Entry::last`);
        // then the shorter of the link's markers and the markers on line `y` of both buffers
        // (counted in O(log n) by the buffers' line index), so a miss costs
        // `min(link markers, markers on y)` checks.
        visit(1);
        if entry.last.as_ref().is_some_and(on_line) {
            return;
        }
        let (on_normal, on_alt) = (
            normal.markers().count_on_line(y),
            alt.markers().count_on_line(y),
        );
        let found = if entry.lines.len() <= on_normal.saturating_add(on_alt) {
            visit(entry.lines.len());
            entry.lines.iter().copied().find(|m| on_line(m))
        } else {
            visit(on_normal.saturating_add(on_alt));
            let link_of_marker = &self.link_of_marker;
            // any live marker of this link on `y` is the answer; which one only steers the cache
            let mine = |ids: &mut dyn Iterator<Item = MarkerId>| -> Option<MarkerId> {
                ids.filter(|id| link_of_marker.get(id) == Some(&link_id))
                    .min()
            };
            mine(&mut normal.markers().ids_on_line(y))
                .map(|id| LinkMarker { id, alt: false })
                .or_else(|| {
                    mine(&mut alt.markers().ids_on_line(y)).map(|id| LinkMarker { id, alt: true })
                })
        };
        if let Some(found) = found {
            entry.last = Some(found);
            return;
        }
        let alt = buffers.buffers.is_alt_active();
        let marker = buffers.buffer_mut().add_marker(y);
        entry.lines.push_back(LinkMarker { id: marker, alt });
        entry.last = Some(LinkMarker { id: marker, alt });
        self.link_of_marker.insert(marker, link_id);
        self.touch(link_id);
        self.enforce_caps(buffers, link_id);
    }

    /// The data of a link id, `None` if unknown or freed.
    pub fn get_link_data(&self, link_id: u32) -> Option<&OscLinkData> {
        self.data_by_link_id.get(&link_id).map(|entry| &entry.data)
    }

    /// Upstream `_removeMarkerFromLink` for each disposed marker (the host forwards
    /// `Buffer::take_disposed_markers()`; see the module notes). A link whose last marker is
    /// gone is freed.
    ///
    /// LUNETERM: (review F6) upstream removes each marker from its link's list on its own
    /// dispose event, a search and a removal per marker: quadratic when ED 3, RIS or a trim
    /// disposes the markers of a link over a long buffer at once. Here the batch is grouped by
    /// link and each affected list is filtered once, by a merge of the sorted ids against the
    /// list, which is in creation order and so sorted by id (`Entry::lines`); a single trimmed
    /// line removes the link's oldest marker from the front of its list in O(1) (review S7
    /// residual 1). The end state is the same; links freed by one batch are freed in another
    /// order, which nothing observes.
    pub fn on_markers_disposed(&mut self, ids: &[MarkerId]) {
        let mut gone: HashMap<u32, Vec<MarkerId>> = HashMap::new();
        for &marker in ids {
            if let Some(link_id) = self.link_of_marker.remove(&marker) {
                gone.entry(link_id).or_default().push(marker);
            }
        }
        for (link_id, mut markers) in gone {
            let Some(entry) = self.data_by_link_id.get_mut(&link_id) else {
                continue;
            };
            markers.sort_unstable();
            markers.dedup();
            let before = entry.lines.len();
            if let [marker] = markers.as_slice() {
                // the common case (one trimmed line): the link's oldest marker, at the front;
                // else one search and one removal (which moves the shorter side)
                if entry.lines.front().is_some_and(|m| m.id == *marker) {
                    entry.lines.pop_front();
                    #[cfg(test)]
                    {
                        self.line_visits += 1;
                    }
                } else if let Ok(index) = entry.lines.binary_search_by_key(marker, |m| m.id) {
                    #[cfg(test)]
                    {
                        self.line_visits += 1 + index.min(before - 1 - index) as u64;
                    }
                    entry.lines.remove(index);
                }
            } else {
                #[cfg(test)]
                {
                    self.line_visits += before as u64;
                }
                let mut next = markers.iter().peekable();
                entry.lines.retain(|m| {
                    while next.next_if(|&&id| id < m.id).is_some() {}
                    next.next_if(|&&id| id == m.id).is_none()
                });
            }
            if before - entry.lines.len() != markers.len() {
                // not sorted after all (never expected): remove the rest by lookup
                let markers: HashSet<MarkerId> = markers.into_iter().collect();
                entry.lines.retain(|m| !markers.contains(&m.id));
            }
            if entry.lines.len() != before && entry.lines.is_empty() {
                if let Some(key) = entry.key.take() {
                    self.entries_with_id.remove(&key);
                }
                self.by_age.remove(&entry.age);
                self.data_by_link_id.remove(&link_id);
            }
        }
    }

    /// The markers that keep a link alive, in the order they were added.
    pub fn link_markers(
        &self,
        link_id: u32,
    ) -> Option<impl ExactSizeIterator<Item = LinkMarker> + DoubleEndedIterator + '_> {
        self.data_by_link_id
            .get(&link_id)
            .map(|entry| entry.lines.iter().copied())
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

    /// LUNETERM: marks a link as the most recently extended (module notes).
    fn touch(&mut self, link_id: u32) {
        let Some(entry) = self.data_by_link_id.get_mut(&link_id) else {
            return;
        };
        self.by_age.remove(&entry.age);
        entry.age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1);
        self.by_age.insert(entry.age, link_id);
    }

    /// LUNETERM: evicts the least recently extended links other than `keep` until both caps
    /// hold (module notes, review S2).
    fn enforce_caps(&mut self, buffers: &mut BufferService, keep: u32) {
        while self.data_by_link_id.len() > MAX_LINKS || self.link_of_marker.len() > MAX_LINK_MARKERS
        {
            let Some((age, victim)) = self
                .by_age
                .iter()
                .map(|(&age, &id)| (age, id))
                .find(|&(_, id)| id != keep)
            else {
                return;
            };
            // Taken out here, not only by `evict`: every iteration removes a key, so the loop
            // ends even if a key outlived its entry (review F5).
            self.by_age.remove(&age);
            self.evict(buffers, victim);
        }
    }

    /// LUNETERM: drops a link and disposes its markers. Their disposal is logged by the buffer
    /// like any other; forwarded later, the ids are unknown here and ignored. The markers of
    /// each buffer go in one call (review F6), logged in creation order, which is the order the
    /// link added them.
    fn evict(&mut self, buffers: &mut BufferService, link_id: u32) {
        let Some(entry) = self.data_by_link_id.remove(&link_id) else {
            return;
        };
        self.by_age.remove(&entry.age);
        if let Some(key) = entry.key {
            self.entries_with_id.remove(&key);
        }
        let (mut normal, mut alt) = (HashSet::new(), HashSet::new());
        for marker in entry.lines {
            self.link_of_marker.remove(&marker.id);
            if marker.alt {
                alt.insert(marker.id);
            } else {
                normal.insert(marker.id);
            }
        }
        buffers.buffers.normal_mut().dispose_markers(&normal);
        buffers.buffers.alt_mut().dispose_markers(&alt);
    }

    /// LUNETERM: (review F5) the next id not in use (module notes).
    fn take_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            if !self.data_by_link_id.contains_key(&id) {
                return id;
            }
        }
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
