//! Reader for Rekordbox's USB export database (`PIONEER/rekordbox/export.pdb`):
//! tracks, artists, keys and the playlist tree. Only what the browser needs.
//!
//! Layout per Deep Symmetry's reverse engineering
//! (<https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/exports.html>):
//! fixed-size pages chained per table; each page has a 0x28-byte header, a
//! heap of rows from the front, and from the back an index of row groups (16
//! row offsets, a presence bitmask, two bytes padding). Little-endian.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow, bail};

#[derive(Clone, Debug, Default)]
pub struct PdbTrack {
    pub id: u32,
    pub title: String,
    pub artist: String,
    pub key: String,
    pub bpm: f64,
    pub secs: u32,
    /// Audio path relative to the stick root, e.g. `/Contents/Artist/Album/x.mp3`.
    pub file_path: String,
    /// `ANLZ0000.DAT` path relative to the stick root.
    pub analyze_path: String,
}

#[derive(Clone, Debug)]
pub struct PlaylistNode {
    pub id: u32,
    /// 0 at the top level.
    pub parent: u32,
    pub name: String,
    pub folder: bool,
    pub sort: u32,
}

#[derive(Debug, Default)]
pub struct Database {
    pub tracks: Vec<PdbTrack>,
    pub playlists: Vec<PlaylistNode>,
    /// Playlist id → track ids, in playlist order.
    pub entries: HashMap<u32, Vec<u32>>,
}

const TRACKS: u32 = 0;
const ARTISTS: u32 = 2;
const KEYS: u32 = 5;
const PLAYLIST_TREE: u32 = 7;
const PLAYLIST_ENTRIES: u32 = 8;
const PAGE_HEADER: usize = 0x28;
const GROUP_SIZE: usize = 36;

/// Read the tracks and playlist tree of an `export.pdb`. Rows that can't be
/// read are skipped.
///
/// # Errors
/// If the file can't be read, isn't an export database, or a table's page
/// chain is broken.
pub fn read(path: &Path) -> Result<Database> {
    let bytes = crate::files::read_capped(path, crate::files::MAX_PDB_BYTES)?;
    let pdb = Pdb::new(&bytes)?;

    let mut artists = HashMap::new();
    pdb.rows(ARTISTS, |row| {
        let subtype = u16_at(row, 0)?;
        let id = u32_at(row, 4)?;
        let ofs = if subtype == 0x64 { usize::from(u16_at(row, 0x0a)?) } else { usize::from(*row.get(9)?) };
        artists.insert(id, string_at(row, ofs)?);
        Some(())
    })?;
    let mut keys = HashMap::new();
    pdb.rows(KEYS, |row| {
        keys.insert(u32_at(row, 0)?, string_at(row, 8)?);
        Some(())
    })?;

    let mut db = Database::default();
    pdb.rows(TRACKS, |row| {
        let s = |i: usize| -> Option<String> { string_at(row, usize::from(u16_at(row, 0x5e + 2 * i)?)) };
        db.tracks.push(PdbTrack {
            id: u32_at(row, 0x48)?,
            title: s(17)?,
            artist: artists.get(&u32_at(row, 0x44)?).cloned().unwrap_or_default(),
            key: keys.get(&u32_at(row, 0x20)?).cloned().unwrap_or_default(),
            bpm: f64::from(u32_at(row, 0x38)?) / 100.0,
            secs: u32::from(u16_at(row, 0x54)?),
            file_path: s(20)?,
            analyze_path: s(14)?,
        });
        Some(())
    })?;
    pdb.rows(PLAYLIST_TREE, |row| {
        db.playlists.push(PlaylistNode {
            parent: u32_at(row, 0)?,
            sort: u32_at(row, 8)?,
            id: u32_at(row, 0x0c)?,
            folder: u32_at(row, 0x10)? != 0,
            name: string_at(row, 0x14)?,
        });
        Some(())
    })?;
    let mut entries: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    pdb.rows(PLAYLIST_ENTRIES, |row| {
        let (index, track, playlist) = (u32_at(row, 0)?, u32_at(row, 4)?, u32_at(row, 8)?);
        entries.entry(playlist).or_default().push((index, track));
        Some(())
    })?;
    db.entries = entries
        .into_iter()
        .map(|(p, mut v)| {
            v.sort_unstable();
            (p, v.into_iter().map(|(_, t)| t).collect())
        })
        .collect();
    Ok(db)
}

struct Pdb<'a> {
    bytes: &'a [u8],
    page_size: usize,
    /// (table type, first page, last page)
    tables: Vec<(u32, u32, u32)>,
}

impl<'a> Pdb<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        let header = |o| u32_at(bytes, o).ok_or_else(|| anyhow!("truncated export.pdb header"));
        let page_size = usize::try_from(header(4)?)?;
        let n = usize::try_from(header(8)?)?;
        if page_size < 512 || n > 64 {
            bail!("not a Rekordbox export.pdb");
        }
        let tables = (0..n)
            .map(|i| {
                let o = 0x1c + i * 16;
                Ok((header(o)?, header(o + 8)?, header(o + 12)?))
            })
            .collect::<Result<_>>()?;
        Ok(Pdb { bytes, page_size, tables })
    }

    /// Call `f` with every present row of table `kind`. A row `f` can't read
    /// (returns None) is skipped rather than failing the whole export.
    fn rows(&self, kind: u32, mut f: impl FnMut(&[u8]) -> Option<()>) -> Result<()> {
        let Some(&(_, first, last)) = self.tables.iter().find(|t| t.0 == kind) else { return Ok(()) };
        let mut index = first;
        // A table can't have more pages than the file; guards against loops.
        for _ in 0..=self.bytes.len() / self.page_size {
            let page = self.page(index).ok_or_else(|| anyhow!("page {index} out of range"))?;
            page_rows(page, &mut f);
            if index == last {
                return Ok(());
            }
            index = u32_at(page, 0x0c).ok_or_else(|| anyhow!("bad page header"))?;
        }
        bail!("page chain for table {kind} doesn't end")
    }

    /// Page `index`, if the file has it.
    fn page(&self, index: u32) -> Option<&'a [u8]> {
        let start = usize::try_from(index).ok()?.checked_mul(self.page_size)?;
        self.bytes.get(start..start.checked_add(self.page_size)?)
    }
}

/// Call `f` with every present row of one page. The row index grows down
/// from the end of the page; a count or offset that points outside it stops
/// the page instead of reading out of bounds.
fn page_rows(page: &[u8], f: &mut impl FnMut(&[u8]) -> Option<()>) -> Option<()> {
    let flags = *page.get(0x1b)?;
    if flags & 0x40 != 0 {
        return None; // not a data page
    }
    let small = u16::from(*page.get(0x18)?);
    let large = u16_at(page, 0x22)?;
    let rows = usize::from(if large > small && large != 0x1fff { large } else { small });
    let heap = page.get(PAGE_HEADER..)?;
    for group in 0..rows.div_ceil(16) {
        let flags_at = page.len().checked_sub(group * GROUP_SIZE + 4)?;
        let present = u16_at(page, flags_at)?;
        for i in 0..16 {
            if present & (1 << i) == 0 {
                continue;
            }
            let ofs = usize::from(u16_at(page, flags_at.checked_sub(2 * (i + 1))?)?);
            let _ = f(heap.get(ofs..)?);
        }
    }
    Some(())
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// A `DeviceSQL` string at `o` within `row`: short ASCII (odd first byte holds
/// the length), long ASCII (0x40) or UTF-16LE (0x90) with a 16-bit length.
fn string_at(row: &[u8], o: usize) -> Option<String> {
    let kind = *row.get(o)?;
    if kind & 1 == 1 {
        let len = usize::from(kind >> 1).checked_sub(1)?;
        return Some(String::from_utf8_lossy(row.get(o + 1..o + 1 + len)?).into_owned());
    }
    let len = usize::from(u16_at(row, o + 1)?).checked_sub(4)?;
    let body = row.get(o + 4..o + 4 + len)?;
    match kind {
        0x40 => Some(String::from_utf8_lossy(body).into_owned()),
        0x90 => {
            let units: Vec<u16> = body.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
            // ISRC strings are 0x90 with a 0x03 marker and ASCII; not needed here.
            Some(String::from_utf16_lossy(&units))
        }
        _ => None,
    }
}
