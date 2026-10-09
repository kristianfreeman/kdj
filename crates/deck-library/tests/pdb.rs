//! Our `export.pdb` reader against rekordcrate's, row for row: the 2-track
//! demo export always, the real ~3,900-track export in the full suite
//! (`cargo test -- --ignored`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use binrw::BinRead;
use deck_library::pdb;
use rekordcrate::pdb::{Header, Row};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Every row rekordcrate finds, in any table.
fn oracle_rows(path: &Path) -> Result<Vec<Row>> {
    let mut f = std::fs::File::open(path)?;
    let header = Header::read(&mut f)?;
    let mut rows = Vec::new();
    for table in &header.tables {
        let pages = header.read_pages(&mut f, binrw::Endian::NATIVE, (&table.first_page, &table.last_page))?;
        for page in pages {
            for group in page.row_groups {
                rows.extend(group.present_rows());
            }
        }
    }
    Ok(rows)
}

/// The number after `key` in a Debug string.
fn num(dbg: &str, key: &str) -> Result<u64> {
    let rest = &dbg[dbg.find(key).with_context(|| format!("{key} in {dbg}"))? + key.len()..];
    Ok(rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse()?)
}

/// `num`, for a field that is 32 bits wide.
fn num32(dbg: &str, key: &str) -> Result<u32> {
    Ok(u32::try_from(num(dbg, key)?)?)
}

/// The string literal after `key` in a Debug string, unescaped.
fn text(dbg: &str, key: &str) -> Result<String> {
    let rest = &dbg[dbg.find(key).with_context(|| format!("{key} in {dbg}"))? + key.len()..];
    let mut chars = rest.chars().skip(1); // opening quote
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('u') => {
                    let hex: String = chars.by_ref().skip(1).take_while(|&c| c != '}').collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16)?).context("escaped char")?);
                }
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    Ok(out)
}

fn tracks_match(name: &str) -> Result<()> {
    let path = fixture(name);
    let ours = pdb::read(&path)?;
    let by_id: HashMap<u32, &pdb::PdbTrack> = ours.tracks.iter().map(|t| (t.id, t)).collect();
    let mut checked = 0;
    for row in oracle_rows(&path)? {
        let Row::Track(t) = row else { continue };
        let dbg = format!("{t:?}");
        let id = num32(&dbg, "id: TrackId(")?;
        let mine = by_id.get(&id).with_context(|| format!("{name}: track {id} missing"))?;
        assert_eq!(mine.title, text(&dbg, "title: DeviceSQLString(")?, "{name}: track {id} title");
        assert_eq!(mine.file_path, text(&dbg, "file_path: DeviceSQLString(")?, "{name}: track {id} path");
        assert_eq!(mine.analyze_path, text(&dbg, "analyze_path: DeviceSQLString(")?, "{name}: track {id} anlz");
        let tempo = f64::from(num32(&dbg, "tempo: ")?);
        assert!((mine.bpm - tempo / 100.0).abs() < 1e-9, "{name}: track {id} tempo {} vs {tempo}", mine.bpm);
        assert_eq!(u64::from(mine.secs), num(&dbg, "duration: ")?, "{name}: track {id} duration");
        checked += 1;
    }
    assert_eq!(checked, ours.tracks.len(), "{name}: same number of tracks");
    println!("{name}: {checked} tracks identical");
    Ok(())
}

#[test]
fn demo_tracks_match_rekordcrate() -> Result<()> {
    tracks_match("demo_export/PIONEER/rekordbox/export.pdb")
}

#[test]
#[ignore = "full suite"]
fn large_export_tracks_match_rekordcrate() -> Result<()> {
    tracks_match("large_export.pdb")
}

#[test]
#[ignore = "full suite"]
fn large_export_playlists_match_rekordcrate() -> Result<()> {
    let path = fixture("large_export.pdb");
    let ours = pdb::read(&path)?;
    let mut nodes = 0;
    let mut entries: HashMap<u32, Vec<(u64, u32)>> = HashMap::new();
    for row in oracle_rows(&path)? {
        match row {
            Row::PlaylistTreeNode(n) => {
                let mine = ours.playlists.iter().find(|p| p.id == n.id.0).expect("node present");
                assert_eq!(mine.parent, n.parent_id.0);
                assert_eq!(mine.folder, n.is_folder());
                assert_eq!(mine.name, n.name.clone().into_string().unwrap());
                nodes += 1;
            }
            Row::PlaylistEntry(e) => {
                let dbg = format!("{e:?}");
                let p = num32(&dbg, "playlist_id: PlaylistTreeNodeId(")?;
                entries.entry(p).or_default().push((num(&dbg, "entry_index: ")?, num32(&dbg, "track_id: TrackId(")?));
            }
            _ => {}
        }
    }
    assert_eq!(nodes, ours.playlists.len());
    assert!(nodes > 100, "a real tree: {nodes} nodes");
    for (p, mut v) in entries {
        v.sort_unstable();
        let expected: Vec<u32> = v.into_iter().map(|(_, t)| t).collect();
        assert_eq!(ours.entries.get(&p), Some(&expected), "playlist {p}");
    }
    Ok(())
}

#[test]
fn demo_export_has_artist_and_key() {
    let db = pdb::read(&fixture("demo_export/PIONEER/rekordbox/export.pdb")).unwrap();
    let t = db.tracks.iter().find(|t| t.title == "Demo Track 1").unwrap();
    assert_eq!((t.artist.as_str(), t.key.as_str(), t.bpm, t.secs), ("Loopmasters", "Fm", 128.0, 172));
}
