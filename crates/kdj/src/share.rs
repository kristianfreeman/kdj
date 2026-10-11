//! Sharing this kdj's sticks with the other kdjs, and loading from theirs.
//!
//! Like CDJs on Pro DJ Link: the kdj with the stick serves it, and every kdj
//! browses every source. Each kdj runs a small read-only HTTP service, and
//! says where it is in its hello (see `net`):
//!
//! | Route | Answer |
//! | --- | --- |
//! | `GET /sources` | The shared sources (`SourceInfo`s, JSON) |
//! | `GET /sources/{id}/library` | What's in one (a `Listing`, JSON) |
//! | `GET /sources/{id}/tracks/{n}/audio` | A track's audio file |
//! | `GET /sources/{id}/tracks/{n}/{dat,ext,2ex}` | Its Rekordbox analysis |
//!
//! Tracks go by their place in the listing, so only files a scan listed are
//! ever served, and only to kdjs that have said hello. A loaded track is
//! all in memory: once it's on a deck, the network and the stick it came
//! from can go.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use kdj_library::files::MAX_ANLZ_BYTES;
use kdj_library::library::Library;
use kdj_library::shared::{Listing, Remote, SourceInfo};

#[cfg(test)]
use crate::net::Use;
use crate::net::{Net, PeerInfo, role};

/// A request's head, at most.
const MAX_HEAD: usize = 8 << 10;
/// A shared library's listing, at most (a 10,000-track stick is ~5 MB).
const MAX_LISTING: u64 = 64 << 20;
/// An audio file, at most, when free memory isn't known (not on the Pi).
const MAX_AUDIO: u64 = 1 << 30;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// A stalled transfer gives up after this long without a byte.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// One of this kdj's sources, as it's shared.
struct Shelved {
    info: SourceInfo,
    listing: Arc<[u8]>,
    /// Each track's audio file and its `.DAT`, by listing place.
    files: Vec<(PathBuf, PathBuf)>,
}

/// This kdj's library service.
pub struct Server {
    port: u16,
    net: Option<Arc<Net>>,
    shelf: Mutex<Vec<Shelved>>,
}

impl Server {
    /// Start serving (nothing, until `share`), to kdjs `net` has heard from.
    ///
    /// # Errors
    /// If no TCP port can be had.
    pub fn start(net: Option<Arc<Net>>) -> std::io::Result<Arc<Self>> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let server = Arc::new(Self { port: listener.local_addr()?.port(), net, shelf: Mutex::new(Vec::new()) });
        let s = Arc::clone(&server);
        std::thread::Builder::new().name("share".into()).spawn(move || {
            for conn in listener.incoming().flatten() {
                let s = Arc::clone(&s);
                let _ = std::thread::Builder::new().name("share-conn".into()).spawn(move || s.answer(conn));
            }
        })?;
        Ok(server)
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Share `libs`, this kdj's own sources (never ones from other kdjs).
    pub fn share(&self, libs: &[Library]) {
        let shared: Vec<_> = libs
            .iter()
            .filter(|l| l.source.remote.is_none())
            .filter_map(|l| {
                let listing = Listing::of(l).to_json().ok()?;
                let info = SourceInfo {
                    id: source_id(&l.source.usbanlz),
                    name: l.source.name.clone(),
                    stamp: hash(&listing),
                    tracks: l.entries.len(),
                };
                let files = l.entries.iter().map(|e| (e.audio.clone(), e.anlz.clone())).collect();
                Some(Shelved { info, listing: listing.into(), files })
            })
            .collect();
        let stamp = u32::try_from(hash(&shared.iter().map(|s| (s.info.id, s.info.stamp)).collect::<Vec<_>>()) >> 32)
            .unwrap_or_default();
        if !shared.is_empty() {
            let names: Vec<_> = shared.iter().map(|s| format!("{} ({} tracks)", s.info.name, s.info.tracks)).collect();
            eprintln!("share: sharing {}", names.join(", "));
        }
        *self.shelf.lock().unwrap_or_else(PoisonError::into_inner) = shared;
        if let Some(net) = &self.net {
            net.set_library(self.port, stamp);
        }
    }

    /// What's shared: each source's id and name.
    pub fn sources(&self) -> Vec<(u32, String)> {
        let shared = self.shelf.lock().unwrap_or_else(PoisonError::into_inner);
        shared.iter().map(|s| (s.info.id, s.info.name.clone())).collect()
    }

    fn answer(&self, mut conn: TcpStream) {
        let _ = conn.set_read_timeout(Some(IO_TIMEOUT));
        let _ = conn.set_write_timeout(Some(IO_TIMEOUT));
        let allowed =
            conn.peer_addr().is_ok_and(|a| a.ip().is_loopback() || self.net.as_ref().is_none_or(|n| n.knows(a.ip())));
        let reply = if allowed {
            read_head(&mut conn).map_or(Reply::Status(400, "Bad Request"), |head| self.route(&head))
        } else {
            Reply::Status(403, "Forbidden")
        };
        let _ = reply.send(&mut conn);
    }

    fn route(&self, head: &str) -> Reply {
        let Some(path) = head.strip_prefix("GET ").and_then(|r| r.split(' ').next()) else {
            return Reply::Status(405, "Method Not Allowed");
        };
        let parts: Vec<_> = path.trim_matches('/').split('/').collect();
        let shared = self.shelf.lock().unwrap_or_else(PoisonError::into_inner);
        let find = |id: &str| id.parse::<u32>().ok().and_then(|id| shared.iter().find(|s| s.info.id == id));
        let track = |id: &str, n: &str| find(id).and_then(|s| s.files.get(n.parse::<usize>().ok()?)).cloned();
        let reply = match parts.as_slice() {
            ["sources"] => {
                let infos: Vec<_> = shared.iter().map(|s| s.info.clone()).collect();
                serde_json::to_vec(&infos)
                    .map_or(Reply::Status(500, "Internal Server Error"), |b| Reply::Json(b.into()))
            }
            ["sources", id, "library"] => find(id).map_or(NOT_FOUND, |s| Reply::Json(Arc::clone(&s.listing))),
            ["sources", id, "tracks", n, what] => match (track(id, n), *what) {
                (Some((audio, _)), "audio") => Reply::File(audio, u64::MAX),
                (Some((_, dat)), "dat") => Reply::File(dat.with_extension("DAT"), MAX_ANLZ_BYTES),
                (Some((_, dat)), "ext") => Reply::File(dat.with_extension("EXT"), MAX_ANLZ_BYTES),
                (Some((_, dat)), "2ex") => Reply::File(dat.with_extension("2EX"), MAX_ANLZ_BYTES),
                _ => NOT_FOUND,
            },
            _ => NOT_FOUND,
        };
        drop(shared);
        reply
    }
}

const NOT_FOUND: Reply = Reply::Status(404, "Not Found");

enum Reply {
    Status(u16, &'static str),
    Json(Arc<[u8]>),
    /// A file, if it's a regular file of at most this many bytes.
    File(PathBuf, u64),
}

impl Reply {
    fn send(self, conn: &mut TcpStream) -> std::io::Result<()> {
        match self {
            Self::Status(code, why) => head(conn, code, why, "text/plain", 0),
            Self::Json(body) => {
                head(conn, 200, "OK", "application/json", body.len() as u64)?;
                conn.write_all(&body)
            }
            Self::File(path, max) => {
                let Some((mut file, len)) = open_file(&path, max) else {
                    return head(conn, 404, "Not Found", "text/plain", 0);
                };
                head(conn, 200, "OK", "application/octet-stream", len)?;
                std::io::copy(&mut (&mut file).take(len), conn).map(drop)
            }
        }
    }
}

fn head(conn: &mut TcpStream, code: u16, why: &str, kind: &str, len: u64) -> std::io::Result<()> {
    write!(conn, "HTTP/1.1 {code} {why}\r\nContent-Type: {kind}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n")
}

/// A regular file and its length, if it's at most `max` bytes.
fn open_file(path: &Path, max: u64) -> Option<(std::fs::File, u64)> {
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    (meta.is_file() && meta.len() <= max).then_some((file, meta.len()))
}

/// A request's head (up to the blank line), if it's one.
fn read_head(conn: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD || conn.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    String::from_utf8(head).ok()
}

/// A source's id: stable while the same stick is at the same place, so a
/// load in flight never lands on another stick after a rescan.
fn source_id(at: &Path) -> u32 {
    u32::try_from(hash(&at) >> 32).unwrap_or_default()
}

fn hash<T: Hash + ?Sized>(t: &T) -> u64 {
    let mut h = DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// `GET path` from a library service: the body, if the answer is 200 and
/// at most `max` bytes.
///
/// # Errors
/// If it can't be reached, answers otherwise, or the transfer stalls.
pub fn get(addr: SocketAddr, path: &str, max: u64) -> Result<Vec<u8>> {
    let mut conn = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).with_context(|| format!("reaching {addr}"))?;
    conn.set_read_timeout(Some(IO_TIMEOUT))?;
    conn.set_write_timeout(Some(IO_TIMEOUT))?;
    write!(conn, "GET {path} HTTP/1.1\r\nHost: kdj\r\nConnection: close\r\n\r\n")?;
    let mut r = BufReader::new(conn);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let code = line.split(' ').nth(1).and_then(|c| c.parse::<u16>().ok()).context("not an HTTP answer")?;
    let mut len = None;
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            len = value.trim().parse::<u64>().ok();
        }
    }
    match code {
        200 => {}
        404 => bail!("not there any more"),
        403 => bail!("refused (it hasn't heard from this kdj)"),
        _ => bail!("answered {code}"),
    }
    let len = len.context("no length")?;
    if len > max {
        bail!("too big: {} MB, and {} MB fit in free memory", len >> 20, max >> 20);
    }
    let mut body = vec![0; usize::try_from(len)?];
    r.read_exact(&mut body).context("the transfer stopped")?;
    Ok(body)
}

/// Every library the other kdjs share, and the library services (with their
/// stamps) that answered: one that didn't is worth asking again.
pub fn fetch_libraries(peers: &[PeerInfo]) -> (Vec<Library>, Vec<(SocketAddr, u32)>) {
    let mut libs = Vec::new();
    let mut answered = Vec::new();
    for p in peers {
        let Some((addr, stamp)) = p.library else { continue };
        let infos = get(addr, "/sources", MAX_LISTING)
            .and_then(|b| serde_json::from_slice::<Vec<SourceInfo>>(&b).context("reading its sources"));
        let infos = match infos {
            Ok(i) => i,
            Err(e) => {
                eprintln!("share: {}'s library: {e:#}", p.name);
                continue;
            }
        };
        answered.push((addr, stamp));
        for info in infos {
            let listing =
                get(addr, &format!("/sources/{}/library", info.id), MAX_LISTING).and_then(|b| Listing::from_json(&b));
            match listing {
                Ok(l) => {
                    let name = format!("{} · {}", info.name, label(p));
                    libs.push(l.into_library(name, Remote { kdj: p.name.clone(), addr, id: info.id }));
                }
                Err(e) => eprintln!("share: {} on {}: {e:#}", info.name, p.name),
            }
        }
    }
    (libs, answered)
}

/// How a kdj is named on screen: its deck letter, or its name if it has no
/// deck (`--serve`).
pub fn label(p: &PeerInfo) -> String {
    if p.decks.is_empty() { p.name.clone() } else { role(p.decks.clone()) }
}

/// A kdj as a deck shows it ("from B"): its letter, or its name if it has no
/// deck (`--serve`) or has gone.
pub fn who(peers: &[PeerInfo], kdj: &str) -> String {
    match peers.iter().find(|p| p.name == kdj) {
        Some(p) if !p.decks.is_empty() => crate::net::letters(p.decks.clone()),
        _ => kdj.to_owned(),
    }
}

/// What this kdj (`me`) tells the DJ while other decks depend on one of
/// the `sources` it shares (id, name): a track loaded from it, or loading.
/// Like a CDJ's warning before ejecting media another player is using.
pub fn lending(me: &str, sources: &[(u32, String)], peers: &[PeerInfo]) -> Option<String> {
    let lent: Vec<_> = sources
        .iter()
        .filter_map(|(id, name)| {
            let users: Vec<_> = peers
                .iter()
                .filter(|p| p.using.iter().any(|u| u.kdj == me && u.source == *id))
                .map(|p| who(peers, &p.name))
                .collect();
            (!users.is_empty()).then(|| format!("{name} in use by {}: leave it in", users.join(", ")))
        })
        .collect();
    (!lent.is_empty()).then(|| lent.join(" · "))
}

/// A track from another kdj, fetched whole.
pub struct Fetched {
    pub audio: Vec<u8>,
    pub dat: Option<Vec<u8>>,
    pub ext: Option<Vec<u8>>,
    pub ex2: Option<Vec<u8>>,
}

/// Fetch track `n` of a source on another kdj: its analysis if it has one,
/// then the audio, refused if it's bigger than free memory.
///
/// # Errors
/// If the audio can't be fetched.
pub fn fetch_track(remote: &Remote, n: usize) -> Result<Fetched> {
    let at = |what: &str| format!("/sources/{}/tracks/{n}/{what}", remote.id);
    let anlz = |what: &str| get(remote.addr, &at(what), MAX_ANLZ_BYTES).ok();
    let (dat, ext, ex2) = (anlz("dat"), anlz("ext"), anlz("2ex"));
    let max = kdj_engine::analysis::bytes_free().unwrap_or(MAX_AUDIO);
    let audio = get(remote.addr, &at("audio"), max).with_context(|| format!("from {}", remote.kdj))?;
    Ok(Fetched { audio, dat, ext, ex2 })
}

#[cfg(test)]
mod tests {
    use kdj_library::library;

    use super::*;

    /// The demo export on a stick in a temporary folder, each track's audio
    /// a second of silence.
    fn demo_stick(stick: &Path) -> Result<Library> {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../kdj-library/tests/fixtures/demo_export");
        copy_dir(&fixture, stick)?;
        for dat in kdj_library::anlz::scan(&stick.join("PIONEER/USBANLZ")) {
            let stored = kdj_library::anlz::read(&dat)?.audio.context("an audio path")?;
            let audio = stick.join(stored.trim_start_matches('/'));
            std::fs::create_dir_all(audio.parent().context("a folder")?)?;
            std::fs::write(&audio, silent_wav(48_000))?;
        }
        Ok(library::scan(&library::Source {
            name: "DEMO".into(),
            usbanlz: stick.join("PIONEER/USBANLZ"),
            pdb: Some(stick.join("PIONEER/rekordbox/export.pdb")),
            root: Some(stick.to_path_buf()),
            remote: None,
        }))
    }

    /// A 16-bit stereo WAV file of `frames` frames of silence at 48 kHz.
    fn silent_wav(frames: u32) -> Vec<u8> {
        let data = frames * 4;
        let mut b = Vec::with_capacity(44 + data as usize);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        for v in [16u32, 0x0002_0001, 48_000, 48_000 * 4, 0x0010_0004] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        b.resize(44 + data as usize, 0);
        b
    }

    fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)?.flatten() {
            let dst = to.join(e.file_name());
            if e.path().is_dir() { copy_dir(&e.path(), &dst)? } else { std::fs::copy(e.path(), dst).map(drop)? }
        }
        Ok(())
    }

    /// A kdj shares the demo stick on this machine; another lists it and
    /// fetches a track with its analysis. Only listed files come out.
    #[test]
    fn a_stick_is_browsed_and_a_track_fetched_from_another_kdj() -> Result<()> {
        let stick = std::env::temp_dir().join(format!("kdj-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&stick);
        let lib = demo_stick(&stick)?;
        assert!(!lib.entries.is_empty(), "the fixture has tracks");
        let server = Server::start(None)?;
        server.share(std::slice::from_ref(&lib));
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, server.port()));
        let peer = PeerInfo {
            name: "kdj-test".into(),
            decks: 0..1,
            controller: false,
            library: Some((addr, 1)),
            using: Vec::new(),
        };

        let (libs, answered) = fetch_libraries(std::slice::from_ref(&peer));
        assert_eq!(answered, [(addr, 1)]);
        let [remote] = libs.as_slice() else { panic!("one shared source, got {}", libs.len()) };
        assert_eq!(remote.source.name, "DEMO · deck A");
        assert_eq!(remote.entries.len(), lib.entries.len());
        assert_eq!(remote.playlists.len(), lib.playlists.len());

        let at = remote.source.remote.clone().expect("it's remote");
        let track = fetch_track(&at, 0)?;
        assert_eq!(track.audio, std::fs::read(&lib.entries[0].audio)?, "the audio, whole");
        let dat = track.dat.as_deref().context("its analysis")?;
        let prepared = kdj_library::anlz::prepared_from(dat, track.ext.as_deref(), track.ex2.as_deref(), 48_000);
        let grid = prepared.and_then(|p| p.grid).context("Rekordbox's beat grid, from the bytes")?;
        let known = kdj_engine::analysis::Known { grid: Some(grid), ..Default::default() };
        let (loaded, _) =
            kdj_engine::analysis::load_bytes_known("One".into(), track.audio, Some("wav"), 48_000, known)?;
        assert_eq!(loaded.samples.len(), 48_000 * 2, "a second of stereo, decoded from memory");

        let id = at.id;
        assert!(get(addr, &format!("/sources/{id}/tracks/9999/audio"), MAX_AUDIO).is_err(), "no such track");
        assert!(get(addr, "/sources/1/tracks/0/audio", MAX_AUDIO).is_err(), "no such source");
        assert!(get(addr, "/../../etc/passwd", MAX_AUDIO).is_err(), "only routes");
        assert!(get(addr, &format!("/sources/{id}/tracks/0/audio"), 10).is_err(), "too big for memory");
        std::fs::remove_dir_all(&stick)?;
        Ok(())
    }

    #[test]
    fn a_stick_in_use_by_another_deck_says_so() {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 1));
        let peer = |name: &str, decks, using: Vec<Use>| PeerInfo {
            name: name.into(),
            decks,
            controller: false,
            library: Some((addr, 0)),
            using,
        };
        let on = |kdj: &str, source| Use { kdj: kdj.into(), source };
        let sources = [(7, "KRISTIAN".to_owned()), (8, "SPARE".to_owned())];
        let peers = [
            peer("kdj-elm-fox", 1..2, vec![on("kdj-oak-pike", 7)]),
            peer("kdj-ash-owl", 2..3, vec![on("someone-else", 7), on("kdj-oak-pike", 8)]),
            peer("laptop", 0..0, Vec::new()),
        ];
        assert_eq!(
            lending("kdj-oak-pike", &sources, &peers).as_deref(),
            Some("KRISTIAN in use by B: leave it in · SPARE in use by C: leave it in")
        );
        assert_eq!(lending("kdj-oak-pike", &sources, &peers[2..]), None, "nobody's using them");
        assert_eq!(lending("kdj-oak-pike", &[], &peers), None, "nothing shared");
        assert_eq!(who(&peers, "kdj-elm-fox"), "B");
        assert_eq!(who(&peers, "laptop"), "laptop", "a library without a deck goes by name");
        assert_eq!(who(&peers, "gone"), "gone");
    }

    #[test]
    fn a_stick_keeps_its_id_across_rescans() {
        let a = Path::new("/media/kdj/KRISTIAN/PIONEER/USBANLZ");
        assert_eq!(source_id(a), source_id(a));
        assert_ne!(source_id(a), source_id(Path::new("/media/kdj/OTHER/PIONEER/USBANLZ")));
    }
}
