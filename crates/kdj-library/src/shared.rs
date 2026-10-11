//! A library as one kdj shares it with the others, as JSON.
//!
//! What its sources are, and what's in each. Tracks are named by their place
//! in the listing, never by a path, so nothing another kdj sends can point
//! at a file the sharing kdj didn't list.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::library::{Entry, Library, Playlist, Source};

/// One shared source, as `GET /sources` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// Its number on the sharing kdj, for the other routes.
    pub id: u32,
    pub name: String,
    /// Changes whenever what's in it does, so a listing can be kept.
    pub stamp: u64,
    pub tracks: usize,
}

/// What's in a shared source (`GET /sources/{id}/library`).
#[derive(Debug, Serialize, Deserialize)]
pub struct Listing {
    pub entries: Vec<SharedEntry>,
    pub playlists: Vec<SharedPlaylist>,
    pub missing: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SharedEntry {
    pub title: String,
    pub artist: String,
    pub key: String,
    pub bpm: Option<f64>,
    pub secs: Option<f64>,
    /// Where it is on the sharing kdj: only to tell tracks apart (the
    /// loaded marker), never opened here.
    pub path: String,
    /// Its file type ("mp3"), for the decoder.
    pub ext: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SharedPlaylist {
    pub id: u32,
    pub parent: u32,
    pub name: String,
    pub folder: bool,
    pub sort: u32,
    pub tracks: Vec<usize>,
    pub missing: usize,
}

/// Where a source on another kdj is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// The kdj's name.
    pub kdj: String,
    /// Its library service.
    pub addr: SocketAddr,
    /// The source's number there.
    pub id: u32,
}

impl Listing {
    /// How `lib` is shared.
    #[must_use]
    pub fn of(lib: &Library) -> Self {
        let entries = lib
            .entries
            .iter()
            .map(|e| SharedEntry {
                title: e.title.clone(),
                artist: e.artist.clone(),
                key: e.key.clone(),
                bpm: e.bpm,
                secs: e.secs,
                path: e.audio.to_string_lossy().into_owned(),
                ext: e.audio.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default(),
            })
            .collect();
        let playlists = lib
            .playlists
            .iter()
            .map(|p| SharedPlaylist {
                id: p.id,
                parent: p.parent,
                name: p.name.clone(),
                folder: p.folder,
                sort: p.sort,
                tracks: p.tracks.clone(),
                missing: p.missing,
            })
            .collect();
        Self { entries, playlists, missing: lib.missing }
    }

    /// The listing as a library here, under `name`, its tracks on `remote`.
    /// Playlist entries that don't exist are dropped (a listing is data
    /// from the network, not trusted).
    #[must_use]
    pub fn into_library(self, name: String, remote: Remote) -> Library {
        let n = self.entries.len();
        let entries = self
            .entries
            .into_iter()
            .map(|e| Entry {
                title: e.title,
                artist: e.artist,
                key: e.key,
                // A path that's only an id here: `Remote` says where it is.
                audio: PathBuf::from(format!("{}:{}", remote.kdj, e.path)).with_extension(e.ext),
                anlz: PathBuf::new(),
                bpm: e.bpm,
                secs: e.secs,
            })
            .collect();
        let playlists = self
            .playlists
            .into_iter()
            .map(|p| Playlist {
                id: p.id,
                parent: p.parent,
                name: p.name,
                folder: p.folder,
                sort: p.sort,
                tracks: p.tracks.into_iter().filter(|&t| t < n).collect(),
                missing: p.missing,
            })
            .collect();
        let source = Source { name, usbanlz: PathBuf::new(), root: None, pdb: None, remote: Some(remote) };
        Library { source, entries, playlists, missing: self.missing }
    }

    /// # Errors
    /// If it isn't a listing.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("reading a shared library")
    }

    /// # Errors
    /// Never in practice: a listing is plain data.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).context("writing a shared library")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        let entry = |title: &str, audio: &str| Entry {
            title: title.into(),
            artist: "Artist".into(),
            key: "Fm".into(),
            audio: audio.into(),
            anlz: "/stick/PIONEER/USBANLZ/P000/ANLZ0000.DAT".into(),
            bpm: Some(128.0),
            secs: Some(300.0),
        };
        let source = Source {
            name: "STICK".into(),
            usbanlz: "/stick/PIONEER/USBANLZ".into(),
            root: None,
            pdb: None,
            remote: None,
        };
        Library {
            source,
            entries: vec![entry("One", "/stick/Contents/one.mp3"), entry("Two", "/stick/Contents/two.FLAC")],
            playlists: vec![Playlist {
                id: 3,
                parent: 0,
                name: "Set".into(),
                folder: false,
                sort: 0,
                tracks: vec![1, 0],
                missing: 2,
            }],
            missing: 4,
        }
    }

    #[test]
    fn a_library_crosses_the_network_intact() {
        let json = Listing::of(&library()).to_json().expect("serialises");
        let addr = "192.168.1.20:5000".parse().expect("an address");
        let remote = Remote { kdj: "kdj-oak-pike".into(), addr, id: 2 };
        let lib = Listing::from_json(&json).expect("parses").into_library("STICK · deck B".into(), remote.clone());
        assert_eq!(lib.source.name, "STICK · deck B");
        assert_eq!(lib.source.remote, Some(remote));
        assert_eq!(lib.entries.len(), 2);
        assert_eq!((lib.entries[1].title.as_str(), lib.entries[1].bpm), ("Two", Some(128.0)));
        assert_eq!(lib.entries[1].audio.extension().and_then(|x| x.to_str()), Some("flac"), "the decoder's hint");
        assert_ne!(lib.entries[0].audio, lib.entries[1].audio, "tracks stay apart");
        assert_eq!(lib.playlists[0].tracks, [1, 0]);
        assert_eq!((lib.playlists[0].missing, lib.missing), (2, 4));
    }

    #[test]
    fn playlists_pointing_past_the_tracks_are_trimmed() {
        let mut listing = Listing::of(&library());
        listing.playlists[0].tracks = vec![0, 7, 1];
        let addr = "10.0.0.2:1".parse().expect("an address");
        let lib = listing.into_library("X".into(), Remote { kdj: "k".into(), addr, id: 0 });
        assert_eq!(lib.playlists[0].tracks, [0, 1]);
        assert!(Listing::from_json(b"{\"nope\":1}").is_err());
    }
}
