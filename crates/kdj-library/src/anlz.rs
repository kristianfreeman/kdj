//! Rekordbox analysis files (`ANLZ0000.DAT` and siblings): the beat grid,
//! cues, the 3-band waveform and the path of the audio file. The same format
//! lives on USB exports and in the local collection.
//!
//! Like a CDJ, the deck uses these instead of analysing a prepared track.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use kdj_engine::BeatGrid;
use kdj_engine::track::{Bands, Waveform};
use kdj_engine::track::{HOT_CUES, HotCue};

use crate::files;

#[derive(Clone, Copy, Debug)]
pub struct RbBeat {
    /// 1–4, position in the bar.
    pub number: u16,
    pub bpm: f64,
    /// Milliseconds from the start of the audio, as Rekordbox decodes it.
    pub ms: u32,
}

#[derive(Debug, Default)]
pub struct Analysis {
    /// Audio file path as Rekordbox stored it.
    pub audio: Option<String>,
    pub beats: Vec<RbBeat>,
    /// Memory cue times, ms.
    pub memory_cues: Vec<u32>,
    /// (slot 1–8, start ms, loop end ms) for hot cues A–H.
    pub hot_cues: Vec<(u32, u32, Option<u32>)>,
}

/// Read one analysis file's beat grid, cues and audio path.
///
/// Layout per Deep Symmetry's reverse engineering (and rekordcrate, which the
/// tests compare against): big-endian sections, each a 4-byte tag, header
/// length and total length.
///
/// # Errors
/// If the file can't be read or isn't an `ANLZ` file.
pub fn read(dat: &Path) -> Result<Analysis> {
    let bytes = files::read_capped(dat, files::MAX_ANLZ_BYTES)?;
    let ext = files::read_capped(&dat.with_extension("EXT"), files::MAX_ANLZ_BYTES).ok();
    parse(&bytes, ext.as_deref()).with_context(|| dat.display().to_string())
}

/// An analysis from its files' bytes: the `.DAT`, and the `.EXT` if there is
/// one (a track on another kdj, or `read`).
///
/// # Errors
/// If it isn't a Rekordbox analysis file.
pub fn parse(bytes: &[u8], ext: Option<&[u8]>) -> Result<Analysis> {
    if bytes.get(..4) != Some(b"PMAI".as_slice()) {
        bail!("not a Rekordbox analysis file");
    }
    let mut a = Analysis::default();
    for (tag, header, body) in sections(bytes) {
        match &tag {
            b"PPTH" => a.audio = utf16_be(body),
            b"PQTZ" => a.beats = beats(body),
            b"PCOB" => cue_list(header, body, &mut a),
            _ => {}
        }
    }
    // The `.EXT` file's extended list has all eight hot cues (the `.DAT`
    // list stops at C), so it wins whenever it has any.
    if let Some((memory_cues, hot_cues)) = ext.and_then(extended_cues) {
        a.memory_cues = memory_cues;
        a.hot_cues = hot_cues;
    }
    Ok(a)
}

/// Memory cue times and hot cues, as in [`Analysis`].
type Cues = (Vec<u32>, Vec<(u32, u32, Option<u32>)>);

/// The cues in an `.EXT` file's extended cue lists (`PCO2`), or None if it
/// has none. Each `PCP2` entry has the hot-cue slot (0 for a memory cue) at
/// 0x0c, the type at 0x10 (2 = loop) and start / loop-end times in ms at
/// 0x14 / 0x18; colours and comments follow and are ignored.
fn extended_cues(bytes: &[u8]) -> Option<Cues> {
    if bytes.get(..4) != Some(b"PMAI".as_slice()) {
        return None;
    }
    let (mut memory, mut hot) = (Vec::new(), Vec::new());
    let lists = sections(bytes).filter(|(t, _, _)| t == b"PCO2");
    for (_, cue) in lists.flat_map(|(_, _, body)| entries(body, 0)).filter(|(t, _)| t == b"PCP2") {
        let Some((slot, kind, time, end)) = cue_fields(cue, [0x0c, 0x10, 0x14, 0x18]) else {
            continue;
        };
        if slot == 0 {
            memory.push(time);
        } else if (1..=8).contains(&slot) {
            hot.push((slot, time, (kind == 2 && end > time).then_some(end)));
        }
    }
    (!memory.is_empty() || !hot.is_empty()).then_some((memory, hot))
}

/// A whole cue entry's slot, type, start and loop end, read at the given
/// offsets. None if it's too short.
fn cue_fields(cue: &[u8], at: [usize; 4]) -> Option<(u32, u8, u32, u32)> {
    let [slot, kind, time, end] = at;
    Some((be_u32(cue, slot)?, *cue.get(kind)?, be_u32(cue, time)?, be_u32(cue, end)?))
}

/// Beat grid entries: beat in bar (u16), tempo ×100 (u16), time in ms (u32).
fn beats(body: &[u8]) -> Vec<RbBeat> {
    body.as_chunks::<8>()
        .0
        .iter()
        .map(|b| RbBeat {
            number: u16::from_be_bytes([b[0], b[1]]),
            bpm: f64::from(u16::from_be_bytes([b[2], b[3]])) / 100.0,
            ms: u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
        })
        .collect()
}

/// A cue list: header word 0x0c says memory (0) or hot (1) cues; the body is
/// `PCPT` entries with the hot-cue slot at 0x0c, the type at 0x1c (2 = loop)
/// and start / loop-end times in ms at 0x20 / 0x24.
/// Those fields lie past the entry's own 0x1c-byte header, so the whole
/// entry is read.
fn cue_list(header: &[u8], body: &[u8], a: &mut Analysis) {
    let hot = be_u32(header, 0x0c) == Some(1);
    for (_, cue) in entries(body, 0).filter(|(t, _)| t == b"PCPT") {
        let Some((slot, kind, time, end)) = cue_fields(cue, [0x0c, 0x1c, 0x20, 0x24]) else {
            continue;
        };
        if !hot {
            a.memory_cues.push(time);
        } else if (1..=8).contains(&slot) {
            let looped = kind == 2 && end > time;
            a.hot_cues.push((slot, time, looped.then_some(end)));
        }
    }
}

/// A NUL-terminated big-endian UTF-16 string.
fn utf16_be(body: &[u8]) -> Option<String> {
    let units: Vec<u16> =
        body.as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).take_while(|&u| u != 0).collect();
    String::from_utf16(&units).ok()
}

fn be_u32(bytes: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

/// The sections of an analysis file: (tag, full header, body). Offsets come
/// from the file, so every step is checked; a bad one ends the walk.
fn sections(bytes: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8], &[u8])> {
    let start = be_u32(bytes, 4).and_then(|v| usize::try_from(v).ok()).unwrap_or(usize::MAX);
    sections_from(bytes, start)
}

fn sections_from(bytes: &[u8], start: usize) -> impl Iterator<Item = ([u8; 4], &[u8], &[u8])> {
    walk(bytes, start).map(|(tag, header_len, whole)| {
        let (header, body) = whole.split_at_checked(header_len).unwrap_or((whole, &[]));
        (tag, header, body)
    })
}

/// Like [`sections_from`], but each entry whole (header and body together).
fn entries(bytes: &[u8], start: usize) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    walk(bytes, start).map(|(tag, _, whole)| (tag, whole))
}

/// (tag, header length, whole entry) for each tagged entry from `start`.
/// The header length is at most the entry's length.
fn walk(bytes: &[u8], start: usize) -> impl Iterator<Item = ([u8; 4], usize, &[u8])> {
    let mut o = Some(start);
    std::iter::from_fn(move || {
        let at = o.take()?;
        let field = |k: usize| be_u32(bytes, at.checked_add(k)?).and_then(|v| usize::try_from(v).ok());
        let (header_len, tag_len) = (field(4)?, field(8)?);
        if tag_len < 12 || header_len > tag_len {
            return None;
        }
        let whole = bytes.get(at..at.checked_add(tag_len)?)?;
        let tag: [u8; 4] = whole.first_chunk::<4>().copied()?;
        o = at.checked_add(tag_len);
        Some((tag, header_len, whole))
    })
}

/// A Rekordbox grid as one tempo and a downbeat anchor.
#[derive(Clone, Copy, Debug)]
pub struct FittedGrid {
    pub grid: BeatGrid,
    /// The grid changes tempo; only the first steady section was used.
    pub variable: bool,
}

impl Analysis {
    /// Fit a constant-tempo grid to the stored beats. Beat times are whole
    /// milliseconds, but a least-squares fit over hundreds of beats recovers
    /// the tempo far more precisely than that.
    #[must_use]
    pub fn grid(&self, sample_rate: u32) -> Option<FittedGrid> {
        let first = self.beats.first()?;
        let steady = self.beats.iter().take_while(|b| (b.bpm - first.bpm).abs() < 0.05).count();
        let beats = self.beats.get(..steady)?;
        if beats.len() < 8 {
            return None;
        }
        // x is the beat index, counted as a float so it needs no cast; it
        // ends up as the number of beats.
        let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0_f64, 0.0, 0.0, 0.0, 0.0);
        let mut down: Option<f64> = None;
        for b in beats {
            let (x, y) = (n, f64::from(b.ms));
            if down.is_none() && b.number == 1 {
                down = Some(x);
            }
            sx += x;
            sy += y;
            sxx = x.mul_add(x, sxx);
            sxy = x.mul_add(y, sxy);
            n += 1.0;
        }
        let period_ms = sx.mul_add(-sy, n * sxy) / sx.mul_add(-sx, n * sxx);
        // 20–600 BPM; anything else is a broken file, not music.
        if !(100.0..=3000.0).contains(&period_ms) {
            return None;
        }
        let intercept = period_ms.mul_add(-sx, sy) / n;
        let anchor_ms = down.unwrap_or(0.0).mul_add(period_ms, intercept);
        let grid = BeatGrid::new(60_000.0 / period_ms, anchor_ms / 1000.0 * f64::from(sample_rate), sample_rate);
        Some(FittedGrid { grid, variable: steady < self.beats.len() })
    }

    /// Resolve the stored path: on a USB export, relative to the stick (and
    /// never off it); in the local collection, as the absolute path it is.
    #[must_use]
    pub fn audio_path(&self, export_root: Option<&Path>) -> Option<PathBuf> {
        let p = self.audio.as_deref()?;
        let path = match export_root {
            Some(root) => files::under(root, p)?,
            None => PathBuf::from(p),
        };
        path.is_file().then_some(path)
    }
}

/// Everything a prepared track needs, so the deck doesn't analyse it.
#[derive(Debug)]
pub struct Prepared {
    /// None if Rekordbox's grid can't be used as a steady grid (see `grid`):
    /// the deck finds its own, but keeps the waveform and cues.
    pub grid: Option<BeatGrid>,
    /// None if the export has no 3-band waveform (pre-CDJ-3000 exports).
    pub waveform: Option<Waveform>,
    /// Where the cue sits on load: the first memory cue, in frames.
    pub load_cue: Option<f64>,
    pub hot_cues: [Option<HotCue>; HOT_CUES],
}

/// Read a track's Rekordbox analysis for playback at `sample_rate`.
#[must_use]
pub fn prepared(dat: &Path, sample_rate: u32) -> Option<Prepared> {
    let [bytes, ext, ex2] =
        ["DAT", "EXT", "2EX"].map(|x| files::read_capped(&dat.with_extension(x), files::MAX_ANLZ_BYTES).ok());
    prepared_from(&bytes?, ext.as_deref(), ex2.as_deref(), sample_rate)
}

/// As `prepared`, from the bytes of the `.DAT` file and, if there are
/// any, the `.EXT` and `.2EX` (a track on another kdj).
#[must_use]
pub fn prepared_from(dat: &[u8], ext: Option<&[u8]>, ex2: Option<&[u8]>, sample_rate: u32) -> Option<Prepared> {
    let a = parse(dat, ext).ok()?;
    let grid = a.grid(sample_rate).map(|g| g.grid);
    let frames = |ms: u32| f64::from(ms) / 1000.0 * f64::from(sample_rate);
    let load_cue = a.memory_cues.iter().min().map(|&ms| frames(ms));
    let mut hot_cues = [None; HOT_CUES];
    for &(slot, start, end) in &a.hot_cues {
        // Slots are 1-based; one we have no room for is skipped.
        let index = usize::try_from(slot).ok().and_then(|s| s.checked_sub(1));
        if let Some(cue) = index.and_then(|i| hot_cues.get_mut(i)) {
            *cue = Some(HotCue { start: frames(start), end: end.map(frames) });
        }
    }
    Some(Prepared { grid, waveform: ex2.and_then(|b| waveform_3band(b, sample_rate)), load_cue, hot_cues })
}

/// The CDJ-3000 3-band detail waveform (`PWV7` in the `.2EX` file's bytes):
/// 150 columns a second, one byte each for mid, high and low, 0–127.
#[must_use]
pub fn waveform_3band(ex2: &[u8], sample_rate: u32) -> Option<Waveform> {
    let body = section(ex2, *b"PWV7")?;
    let columns = body
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[mid, high, low]| Bands {
            mid: scale(mid, MID_GAIN),
            high: scale(high, HIGH_GAIN),
            low: scale(low, LOW_GAIN),
        })
        .collect();
    Some(Waveform { hop: (sample_rate / 150) as usize, columns })
}

// Rekordbox's 3-band heights aren't drawn linearly. Deep Symmetry's beat-link
// (the reference CDJ waveform renderer) scales each band onto a 0–31 height:
// lows ×0.4, mids ×0.3, highs ×0.06. Same ratios, onto our 0–255.
const LOW_GAIN: f32 = 0.4 / 31.0 * 255.0;
const MID_GAIN: f32 = 0.3 / 31.0 * 255.0;
const HIGH_GAIN: f32 = 0.06 / 31.0 * 255.0;

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0–255 first, and a float-to-int `as` saturates anyway"
)]
fn scale(v: u8, gain: f32) -> u8 {
    (f32::from(v) * gain).round().clamp(0.0, 255.0) as u8
}

/// The body (after its header) of the first section tagged `tag`.
fn section(bytes: &[u8], tag: [u8; 4]) -> Option<&[u8]> {
    sections(bytes).find(|(t, _, _)| *t == tag).map(|(_, _, body)| body)
}

/// The analysis file for `audio` in the local collection, if Rekordbox has one.
#[must_use]
pub fn find(audio: &Path) -> Option<PathBuf> {
    let target = audio.canonicalize().ok()?;
    let dir = local_usbanlz()?;
    scan(&dir).into_iter().find(|dat| {
        read(dat).ok().and_then(|a| a.audio_path(None)).and_then(|p| p.canonicalize().ok()).is_some_and(|p| p == target)
    })
}

/// Every analysis file under a `USBANLZ` directory.
#[must_use]
pub fn scan(usbanlz: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![usbanlz.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("DAT")) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The local Rekordbox collection's analysis directory on this machine.
#[must_use]
pub fn local_usbanlz() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = Path::new(&home).join("Library/Pioneer/rekordbox/share/PIONEER/USBANLZ");
    p.is_dir().then_some(p)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use binrw::BinRead;
    use rekordcrate::anlz::{ANLZ, Content, CueType};

    use super::*;

    /// A section or entry: tag, header length, total length, then the rest
    /// of the header and the body.
    fn tagged(tag: [u8; 4], header: &[u8], body: &[u8]) -> Vec<u8> {
        let header_len = u32::try_from(12 + header.len()).unwrap();
        let total = header_len + u32::try_from(body.len()).unwrap();
        [tag.as_slice(), &header_len.to_be_bytes(), &total.to_be_bytes(), header, body].concat()
    }

    /// An analysis file: the `PMAI` header (with its 16 unknown bytes), then
    /// the sections.
    fn file(sections: &[Vec<u8>]) -> Vec<u8> {
        let body = sections.concat();
        let total = u32::try_from(0x1c + body.len()).unwrap();
        [b"PMAI".as_slice(), &0x1c_u32.to_be_bytes(), &total.to_be_bytes(), &[0; 16], &body].concat()
    }

    /// A `PCP2` entry as Rekordbox writes it, with an empty comment.
    fn pcp2(slot: u32, kind: u8, time: u32, end: u32) -> Vec<u8> {
        let mut rest = vec![kind, 0, 0x03, 0xe8];
        rest.extend([time.to_be_bytes(), end.to_be_bytes()].concat());
        rest.extend([0; 8]); // colour, unknowns
        rest.extend([0, 1, 0, 4]); // loop 1/4
        rest.extend(2_u32.to_be_bytes()); // comment length, its NUL included
        rest.extend([0; 2 + 4 + 20]); // comment, hot cue colour, unknowns
        tagged(*b"PCP2", &slot.to_be_bytes(), &rest)
    }

    fn pco2(list_type: u32, cues: &[Vec<u8>]) -> Vec<u8> {
        let header =
            [list_type.to_be_bytes().as_slice(), &u16::try_from(cues.len()).unwrap().to_be_bytes(), &[0, 0]].concat();
        tagged(*b"PCO2", &header, &cues.concat())
    }

    /// A `PCPT` entry (the `.DAT` file's cue list).
    fn pcpt(slot: u32, kind: u8, time: u32, end: u32) -> Vec<u8> {
        let header =
            [slot.to_be_bytes(), 0_u32.to_be_bytes(), 0x0010_0000_u32.to_be_bytes(), [0xff, 0xff, 0, 1]].concat();
        let body =
            [[kind, 0, 0x03, 0xe8], time.to_be_bytes(), end.to_be_bytes(), [0; 4], [0; 4], [0; 4], [0; 4]].concat();
        tagged(*b"PCPT", &header, &body)
    }

    fn pcob(list_type: u32, cues: &[Vec<u8>]) -> Vec<u8> {
        let count = u16::try_from(cues.len()).unwrap();
        let header = [list_type.to_be_bytes().as_slice(), &[0, 0], &count.to_be_bytes(), &[0; 4]].concat();
        tagged(*b"PCOB", &header, &cues.concat())
    }

    /// Hot cue A, a hot loop on H and a memory cue, in two lists as
    /// Rekordbox writes them.
    fn ext() -> Vec<u8> {
        file(&[pco2(0, &[pcp2(0, 0, 1_000, 0)]), pco2(1, &[pcp2(1, 0, 2_500, 0), pcp2(8, 2, 60_000, 64_000)])])
    }

    #[test]
    fn extended_cue_list() {
        let (memory, hot) = extended_cues(&ext()).unwrap();
        assert_eq!(memory, [1_000]);
        assert_eq!(hot, [(1, 2_500, None), (8, 60_000, Some(64_000))]);
    }

    #[test]
    fn extended_cue_list_matches_rekordcrate() {
        let theirs = ANLZ::read(&mut Cursor::new(ext())).unwrap();
        let (mut memory, mut hot) = (Vec::new(), Vec::new());
        for section in theirs.sections {
            let Content::ExtendedCueList(list) = section.content else { panic!("not a PCO2 section") };
            for c in list.cues {
                let end = (c.cue_type == CueType::Loop).then_some(c.loop_time);
                if c.hot_cue == 0 {
                    memory.push(c.time);
                } else {
                    hot.push((c.hot_cue, c.time, end));
                }
            }
        }
        assert_eq!(extended_cues(&ext()), Some((memory, hot)));
    }

    #[test]
    fn cue_list_matches_rekordcrate() {
        let dat = file(&[pcob(1, &[pcpt(2, 2, 400, 900), pcpt(3, 0, 700, 0)])]);
        let theirs = ANLZ::read(&mut Cursor::new(&dat)).unwrap();
        let Some(Content::CueList(list)) = theirs.sections.into_iter().next().map(|s| s.content) else {
            panic!("not a PCOB section")
        };
        let hot: Vec<_> = list
            .cues
            .iter()
            .map(|c| (c.hot_cue, c.time, (c.cue_type == CueType::Loop).then_some(c.loop_time)))
            .collect();
        let mut ours = Analysis::default();
        for (_, header, body) in sections(&dat) {
            cue_list(header, body, &mut ours);
        }
        assert_eq!(ours.hot_cues, hot);
        assert_eq!(hot.len(), 2);
    }

    #[test]
    fn broken_extended_cue_lists_dont_panic() {
        let good = ext();
        for n in 0..good.len() {
            let _ = extended_cues(&good[..n]);
        }
        // Every length field (and anything else) at its extremes.
        for at in 0..good.len().saturating_sub(4) {
            for v in [0, 1, 11, 12, 0x7fff_ffff, u32::MAX] {
                let mut bad = good.clone();
                bad[at..at + 4].copy_from_slice(&v.to_be_bytes());
                let _ = extended_cues(&bad);
            }
        }
        assert_eq!(extended_cues(&file(&[pco2(1, &[])])), None);
        assert_eq!(extended_cues(b"nope"), None);
    }

    #[test]
    fn extended_list_wins_over_the_dat_list() {
        let dir = std::env::temp_dir().join(format!("kdj-library-anlz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dat = dir.join("ANLZ0000.DAT");
        let ext_path = dat.with_extension("EXT");
        std::fs::write(&dat, file(&[pcob(0, &[pcpt(0, 0, 300, 0)]), pcob(1, &[pcpt(2, 2, 400, 900)])])).unwrap();
        let _ = std::fs::remove_file(&ext_path);

        // No .EXT: the .DAT's lists.
        let a = read(&dat).unwrap();
        assert_eq!((a.memory_cues, a.hot_cues), (vec![300], vec![(2, 400, Some(900))]));

        // An .EXT with no cues changes nothing; one with cues replaces them.
        std::fs::write(&ext_path, file(&[pco2(1, &[])])).unwrap();
        assert_eq!(read(&dat).unwrap().hot_cues, [(2, 400, Some(900))]);
        std::fs::write(&ext_path, ext()).unwrap();
        let a = read(&dat).unwrap();
        assert_eq!((a.memory_cues, a.hot_cues), (vec![1_000], vec![(1, 2_500, None), (8, 60_000, Some(64_000))]));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
