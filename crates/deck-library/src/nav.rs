//! What's inside what: sources, then "All tracks" and playlist folders, then
//! tracks. UI-agnostic; the browser draws whatever this lists.

use std::hash::{DefaultHasher, Hash, Hasher};

use crate::library::{Entry, Library};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    /// Every connected source (collection, USB sticks).
    Sources,
    Source(usize),
    /// Every track of a source.
    All(usize),
    /// A playlist or playlist folder of a source, by Rekordbox id.
    Playlist(usize, u32),
    /// A track: source, entry index.
    Track(usize, usize),
}

#[derive(Debug)]
pub enum Listing<'a> {
    /// `detail` says what's inside: "12 tracks", "4 lists".
    Folder {
        name: String,
        detail: String,
    },
    Track(&'a Entry),
}

/// Where browsing starts: straight into the only source if there's one.
#[must_use]
pub const fn root(libs: &[Library]) -> Node {
    if libs.len() == 1 { Node::Source(0) } else { Node::Sources }
}

pub fn title(libs: &[Library], node: Node) -> String {
    match node {
        Node::Sources => "Sources".into(),
        Node::Source(s) => libs.get(s).map_or_else(String::new, |l| l.source.name.clone()),
        Node::All(_) => "All tracks".into(),
        Node::Playlist(s, id) => libs
            .get(s)
            .and_then(|l| l.playlists.iter().find(|p| p.id == id))
            .map_or_else(String::new, |p| p.name.clone()),
        Node::Track(s, i) => libs.get(s).and_then(|l| l.entries.get(i)).map_or_else(String::new, |e| e.title.clone()),
    }
}

#[must_use]
pub fn children(libs: &[Library], node: Node) -> Vec<(Node, Listing<'_>)> {
    let folder = |name: &str, n: usize| Listing::Folder { name: name.to_owned(), detail: plural(n, "track") };
    match node {
        Node::Sources => {
            libs.iter().enumerate().map(|(s, l)| (Node::Source(s), folder(&l.source.name, l.entries.len()))).collect()
        }
        Node::Source(s) => {
            let Some(lib) = libs.get(s) else { return Vec::new() };
            std::iter::once((Node::All(s), folder("All tracks", lib.entries.len())))
                .chain(playlists_under(lib, s, 0))
                .collect()
        }
        Node::All(s) => {
            let Some(lib) = libs.get(s) else { return Vec::new() };
            lib.entries.iter().enumerate().map(|(i, e)| (Node::Track(s, i), Listing::Track(e))).collect()
        }
        Node::Playlist(s, id) => {
            let Some(lib) = libs.get(s) else { return Vec::new() };
            match lib.playlists.iter().find(|p| p.id == id) {
                Some(p) if p.folder => playlists_under(lib, s, id).collect(),
                Some(p) => p
                    .tracks
                    .iter()
                    .filter_map(|&i| Some((Node::Track(s, i), Listing::Track(lib.entries.get(i)?))))
                    .collect(),
                None => Vec::new(),
            }
        }
        Node::Track(..) => Vec::new(),
    }
}

fn playlists_under(lib: &Library, s: usize, parent: u32) -> impl Iterator<Item = (Node, Listing<'_>)> {
    lib.playlists.iter().filter(move |p| p.parent == parent).map(move |p| {
        let detail = if p.folder {
            plural(lib.playlists.iter().filter(|c| c.parent == p.id).count(), "list")
        } else {
            plural(p.tracks.len(), "track")
        };
        (Node::Playlist(s, p.id), Listing::Folder { name: p.name.clone(), detail })
    })
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// A stable id for a track, to mark the one that's loaded.
#[must_use]
pub fn track_id(e: &Entry) -> u64 {
    let mut h = DefaultHasher::new();
    e.audio.hash(&mut h);
    h.finish()
}
