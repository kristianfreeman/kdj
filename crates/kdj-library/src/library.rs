//! What's in the crate: a Rekordbox USB export (tracks, metadata and playlists
//! from `export.pdb`) or the local collection.
//!
//! The local collection's database is encrypted, so there we scan the
//! analysis files instead: each stores its audio path, tempo and length, and
//! artist and title come from the path (`Artist/Album/NN Title.ext`). That
//! gives every track but no playlists.

use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::anlz;

#[derive(Clone, Debug)]
pub struct Source {
    /// "Collection", or the USB volume's name.
    pub name: String,
    pub usbanlz: PathBuf,
    /// Stick root that relative USB paths resolve against; None locally.
    pub root: Option<PathBuf>,
    /// The export database, on a USB export.
    pub pdb: Option<PathBuf>,
    /// On another kdj: its tracks come from there (see `shared`).
    pub remote: Option<crate::shared::Remote>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub title: String,
    pub artist: String,
    /// Musical key as Rekordbox names it ("Fm"); empty if unknown.
    pub key: String,
    pub audio: PathBuf,
    /// The track's `ANLZ0000.DAT`, for its beat grid.
    pub anlz: PathBuf,
    pub bpm: Option<f64>,
    pub secs: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Playlist {
    pub id: u32,
    /// 0 at the top level.
    pub parent: u32,
    pub name: String,
    pub folder: bool,
    pub sort: u32,
    /// Indexes into `Library::entries`, in playlist order.
    pub tracks: Vec<usize>,
    /// Entries whose track isn't there (audio not on the stick).
    pub missing: usize,
}

#[derive(Debug)]
pub struct Library {
    pub source: Source,
    /// Sorted by artist, then title.
    pub entries: Vec<Entry>,
    pub playlists: Vec<Playlist>,
    /// Analysis files whose audio isn't there (moved, streaming, unplugged).
    pub missing: usize,
}

/// Everything there is to browse right now: the local collection, then any
/// mounted Rekordbox USB exports.
#[must_use]
pub fn sources() -> Vec<Source> {
    let mut out = Vec::new();
    if let Some(dir) = anlz::local_usbanlz() {
        out.push(Source { name: "Collection".into(), usbanlz: dir, root: None, pdb: None, remote: None });
    }
    for mount in mount_points() {
        let usbanlz = mount.join("PIONEER/USBANLZ");
        let pdb = Some(mount.join("PIONEER/rekordbox/export.pdb")).filter(|p| p.is_file());
        if usbanlz.is_dir() || pdb.is_some() {
            let name = mount.file_name().map_or_else(|| "USB".into(), |n| n.to_string_lossy().into_owned());
            out.push(Source { name, usbanlz, root: Some(mount), pdb, remote: None });
        }
    }
    out
}

/// Where removable drives show up: /Volumes on macOS, /media and friends on Linux.
fn mount_points() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for base in ["/Volumes", "/media", "/run/media", "/mnt"] {
        let Ok(rd) = std::fs::read_dir(base) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            // Linux mounts one level deeper, under the user's name.
            if p.join("PIONEER").is_dir() {
                out.push(p);
            } else if let Ok(inner) = std::fs::read_dir(&p) {
                out.extend(inner.flatten().map(|e| e.path()).filter(|q| q.join("PIONEER").is_dir()));
            }
        }
    }
    out
}

/// Every track (and, on a USB export, every playlist) of `source`. A broken
/// `export.pdb` falls back to the analysis files.
#[must_use]
pub fn scan(source: &Source) -> Library {
    if let (Some(pdb), Some(root)) = (&source.pdb, &source.root) {
        match crate::pdb::read(pdb) {
            Ok(db) => return from_export(source, root, db),
            Err(e) => eprintln!("{}: {e:#}; falling back to analysis files", pdb.display()),
        }
    }
    let mut entries = Vec::new();
    let mut missing = 0;
    for dat in anlz::scan(&source.usbanlz) {
        let Ok(a) = anlz::read(&dat) else { continue };
        let Some(audio) = a.audio_path(source.root.as_deref()) else {
            missing += 1;
            continue;
        };
        let (artist, title) = names(&audio);
        let bpm = a.grid(48_000).map(|g| g.grid.bpm);
        let secs = a.beats.last().map(|b| f64::from(b.ms) / 1000.0);
        entries.push(Entry { title, artist, key: String::new(), audio, anlz: dat, bpm, secs });
    }
    entries.sort_by_cached_key(|e| (e.artist.to_lowercase(), e.title.to_lowercase()));
    Library { source: source.clone(), entries, playlists: Vec::new(), missing }
}

/// A USB export: everything from `export.pdb`, paths relative to the stick.
fn from_export(source: &Source, root: &Path, db: crate::pdb::Database) -> Library {
    let mut missing = 0;
    let mut entries: Vec<(u32, Entry)> = Vec::new();
    for t in db.tracks {
        let paths = crate::files::under(root, &t.file_path).zip(crate::files::under(root, &t.analyze_path));
        let Some((audio, anlz)) = paths.filter(|(audio, _)| audio.is_file()) else {
            missing += 1;
            continue;
        };
        let entry = Entry {
            title: t.title.nfc().collect(),
            artist: t.artist.nfc().collect(),
            key: t.key,
            audio,
            anlz,
            bpm: (t.bpm > 0.0).then_some(t.bpm),
            secs: (t.secs > 0).then_some(f64::from(t.secs)),
        };
        entries.push((t.id, entry));
    }
    entries.sort_by_cached_key(|(_, e)| (e.artist.to_lowercase(), e.title.to_lowercase()));
    let index: std::collections::HashMap<u32, usize> =
        entries.iter().enumerate().map(|(i, (id, _))| (*id, i)).collect();
    let mut playlists: Vec<Playlist> = db
        .playlists
        .into_iter()
        .map(|p| {
            let ids = db.entries.get(&p.id).map_or(&[][..], Vec::as_slice);
            let tracks: Vec<usize> = ids.iter().filter_map(|id| index.get(id).copied()).collect();
            Playlist {
                missing: ids.len() - tracks.len(),
                tracks,
                id: p.id,
                parent: p.parent,
                name: p.name.nfc().collect(),
                folder: p.folder,
                sort: p.sort,
            }
        })
        .collect();
    playlists.sort_by_key(|p| (p.parent, p.sort, p.name.to_lowercase()));
    Library { source: source.clone(), entries: entries.into_iter().map(|(_, e)| e).collect(), playlists, missing }
}

/// (artist, title) from `…/Artist/Album/NN Title.ext`. Composed (NFC), since
/// macOS file names often arrive decomposed ("o" + combining "¨").
fn names(audio: &Path) -> (String, String) {
    let nfc = |s: &std::ffi::OsStr| s.to_string_lossy().nfc().collect::<String>();
    let stem = audio.file_stem().map_or(String::new(), nfc);
    let artist = audio.parent().and_then(Path::parent).and_then(Path::file_name).map_or(String::new(), nfc);
    (artist, strip_track_number(&stem).to_owned())
}

/// "1-01 Kinesis", "01 Kinesis", "01. Kinesis", "01 - Kinesis" → "Kinesis".
/// Longer numbers ("2024 Odyssey", "808") are part of the title.
fn strip_track_number(s: &str) -> &str {
    let token_len = s.find(|c: char| !(c.is_ascii_digit() || c == '-')).unwrap_or(s.len());
    let token = &s[..token_len];
    let is_track_no = !token.is_empty() && (token.contains('-') || token.len() <= 2);
    let rest = &s[token_len..];
    if !is_track_no || !(rest.starts_with(' ') || rest.starts_with('.')) {
        return s;
    }
    let rest = rest.trim_start_matches(['.', ' ', '-']);
    if rest.is_empty() { s } else { rest }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_numbers_are_stripped() {
        assert_eq!(strip_track_number("1-01 Kinesis"), "Kinesis");
        assert_eq!(strip_track_number("01. Evil"), "Evil");
        assert_eq!(strip_track_number("07 - On the Run"), "On the Run");
        assert_eq!(strip_track_number("2024 Odyssey"), "2024 Odyssey");
        assert_eq!(strip_track_number("Inside Me"), "Inside Me");
        assert_eq!(strip_track_number("808"), "808");
    }

    #[test]
    fn names_come_from_the_path() {
        let (a, t) = names(Path::new("/Music/Regent/Arcane/1-01 Kinesis.mp3"));
        assert_eq!((a.as_str(), t.as_str()), ("Regent", "Kinesis"));
        let (a, _) = names(Path::new("/Music/Hedstro\u{308}m & Pflug/Signal/1-01 You Are.mp3"));
        assert_eq!(a, "Hedström & Pflug", "decomposed names are composed");
    }
}
