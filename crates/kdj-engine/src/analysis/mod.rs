//! Turning a file into a playable `Track`: decode, resample, beat grid,
//! waveform. Runs on a loader thread, never the audio thread.

pub mod decode;
pub mod grid;
pub mod resample;
pub mod synth;
pub mod waveform;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::track::{GridSource, Track};

/// Analysis we already have (from Rekordbox), so loading can skip it.
#[derive(Debug, Default)]
pub struct Known {
    pub grid: Option<crate::grid::BeatGrid>,
    pub waveform: Option<crate::track::Waveform>,
    pub load_cue: Option<f64>,
    pub hot_cues: [Option<crate::track::HotCue>; crate::track::HOT_CUES],
}

/// How long each load stage took, for perf budgets.
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadTimings {
    pub decode: Duration,
    pub resample: Duration,
    pub grid: Duration,
    pub waveform: Duration,
}

impl LoadTimings {
    /// Wall time: the grid runs alongside resampling and the waveform.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.decode + self.grid.max(self.resample + self.waveform)
    }
}

/// Load and analyse an audio file for playback at `sample_rate`. The beat
/// grid is found on the decoded audio at its own rate, in parallel with
/// resampling and the waveform, so it's off the critical path.
///
/// # Errors
///
/// If the file can't be read or decoded, or resampling fails.
///
/// # Panics
///
/// If the beat-grid thread panics; the panic is passed on.
pub fn load_file(path: &Path, sample_rate: u32) -> Result<(Track, LoadTimings)> {
    load_file_known(path, sample_rate, Known::default())
}

/// As `load_file`, skipping whatever `known` already provides. A track
/// Rekordbox has prepared only gets decoded and resampled.
///
/// # Errors
///
/// If the file can't be read or decoded, or resampling fails.
///
/// # Panics
///
/// If the beat-grid thread panics; the panic is passed on.
pub fn load_file_known(path: &Path, sample_rate: u32, known: Known) -> Result<(Track, LoadTimings)> {
    let t = Instant::now();
    let decoded = decode::decode(path, frame_budget(known.grid.is_none()))?;
    let title = path.file_stem().map_or_else(|| "untitled".into(), |s| s.to_string_lossy().into_owned());
    prepare(decoded, t.elapsed(), title, sample_rate, known)
}

/// As `load_file_known`, from a whole file in memory (a track on another
/// kdj): `ext` is its file type.
///
/// # Errors
///
/// If it can't be decoded, or resampling fails.
///
/// # Panics
///
/// If the beat-grid thread panics; the panic is passed on.
pub fn load_bytes_known(
    title: String,
    bytes: Vec<u8>,
    ext: Option<&str>,
    sample_rate: u32,
    known: Known,
) -> Result<(Track, LoadTimings)> {
    let t = Instant::now();
    let decoded = decode::decode_bytes(bytes, ext, frame_budget(known.grid.is_none()))?;
    prepare(decoded, t.elapsed(), title, sample_rate, known)
}

/// The rest of a load once the audio is decoded: resample, grid, waveform.
fn prepare(
    decoded: decode::Decoded,
    decode_time: Duration,
    title: String,
    sample_rate: u32,
    known: Known,
) -> Result<(Track, LoadTimings)> {
    let mut timings = LoadTimings { decode: decode_time, ..Default::default() };
    let src_rate = decoded.sample_rate;
    // The grid runs on mono at the file's rate; the stereo samples move on
    // to resampling without a copy.
    let mono_src = known.grid.is_none().then(|| mono(&decoded.samples));

    let grid_source = if known.grid.is_some() { GridSource::Rekordbox } else { GridSource::Detected };
    let (grid, samples, waveform) = std::thread::scope(|s| -> Result<_> {
        let grid = s.spawn(|| {
            let (Some(m), None) = (&mono_src, known.grid) else {
                return (
                    known.grid.unwrap_or_else(|| crate::grid::BeatGrid::new(120.0, 0.0, sample_rate)),
                    Duration::ZERO,
                );
            };
            let t = Instant::now();
            let g = grid::detect(m, src_rate, grid::GridConfig::default());
            // Same beats, counted in frames at the deck's rate.
            let g =
                crate::grid::BeatGrid::new(g.bpm, g.anchor * f64::from(sample_rate) / f64::from(src_rate), sample_rate);
            (g, t.elapsed())
        });
        let t = Instant::now();
        let samples = resample::resample(decoded.samples, src_rate, sample_rate)?;
        timings.resample = t.elapsed();
        let t = Instant::now();
        let waveform = known.waveform.unwrap_or_else(|| waveform::analyze(&samples, sample_rate));
        timings.waveform = t.elapsed();
        let (g, gt) = grid.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        timings.grid = gt;
        Ok((g, samples, waveform))
    })?;

    let track = Track {
        id: next_id(),
        title,
        sample_rate,
        samples,
        grid,
        grid_source,
        load_cue: known.load_cue,
        hot_cues: known.hot_cues,
        waveform,
    };
    Ok((track, timings))
}

/// Bytes of memory a load needs per source frame at its peak: the decoded
/// stereo (8) and the resampled stereo (8, about 9 from 44.1 to 48 kHz),
/// plus slack.
#[cfg(any(target_os = "linux", test))]
const LOAD_BYTES_PER_FRAME: u64 = 18;
/// What finding our own beat grid adds: its mono copy (a Rekordbox track
/// brings its grid, so skips it).
#[cfg(any(target_os = "linux", test))]
const GRID_BYTES_PER_FRAME: u64 = 4;
/// Memory left for everything else while a track loads.
#[cfg(any(target_os = "linux", test))]
const LOAD_HEADROOM_BYTES: u64 = 96 << 20;

/// How many frames of audio a load can take without running out of memory:
/// on Linux (the Pi, where memory is locked and can't swap), from what's
/// available now; elsewhere unlimited.
/// `with_grid`: the load finds its own beat grid (it needs more).
#[cfg(target_os = "linux")]
#[must_use]
pub fn frame_budget(with_grid: bool) -> Option<usize> {
    budget_from_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?, with_grid)
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub const fn frame_budget(_with_grid: bool) -> Option<usize> {
    None
}

/// Memory a load can use, in bytes, as `frame_budget` counts it: for
/// refusing a file from another kdj before fetching it.
#[cfg(target_os = "linux")]
#[must_use]
pub fn bytes_free() -> Option<u64> {
    free_from_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub const fn bytes_free() -> Option<u64> {
    None
}

#[cfg(any(target_os = "linux", test))]
fn free_from_meminfo(info: &str) -> Option<u64> {
    let kb: u64 =
        info.lines().find_map(|l| l.strip_prefix("MemAvailable:"))?.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some(kb.saturating_mul(1024).saturating_sub(LOAD_HEADROOM_BYTES))
}

#[cfg(any(target_os = "linux", test))]
/// The frame budget from `/proc/meminfo`'s text.
fn budget_from_meminfo(info: &str, with_grid: bool) -> Option<usize> {
    let bytes = free_from_meminfo(info)?;
    let per_frame = LOAD_BYTES_PER_FRAME + if with_grid { GRID_BYTES_PER_FRAME } else { 0 };
    usize::try_from(bytes / per_frame).ok()
}

/// Interleaved stereo → mono, for the beat grid.
fn mono(stereo: &[f32]) -> Vec<f32> {
    stereo.as_chunks::<2>().0.iter().map(|&[l, r]| f32::midpoint(l, r)).collect()
}

fn next_id() -> u64 {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Analyse stereo samples already at `sample_rate`.
///
/// # Panics
///
/// If the waveform thread panics; the panic is passed on.
#[must_use]
pub fn build(title: String, samples: Vec<f32>, sample_rate: u32) -> (Track, LoadTimings) {
    let mut timings = LoadTimings::default();
    let (grid, waveform) = std::thread::scope(|s| {
        let wf = s.spawn(|| {
            let t = Instant::now();
            (waveform::analyze(&samples, sample_rate), t.elapsed())
        });
        let t = Instant::now();
        let g = grid::detect(&mono(&samples), sample_rate, grid::GridConfig::default());
        timings.grid = t.elapsed();
        let (w, wt) = wf.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        timings.waveform = wt;
        (g, w)
    });
    let track = Track {
        id: next_id(),
        title,
        sample_rate,
        samples,
        grid,
        grid_source: GridSource::Detected,
        load_cue: None,
        hot_cues: [None; crate::track::HOT_CUES],
        waveform,
    };
    (track, timings)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_frame_budget_comes_from_available_memory() {
        let info = "MemTotal:        3884488 kB\nMemFree:          212345 kB\nMemAvailable:    1310720 kB\n";
        // 1.25 GiB available, 96 MiB kept back; 18 bytes a frame, 22 with a grid to find.
        assert_eq!(super::budget_from_meminfo(info, false), Some(((1_310_720 << 10) - (96 << 20)) / 18));
        assert_eq!(super::budget_from_meminfo(info, true), Some(((1_310_720 << 10) - (96 << 20)) / 22));
        assert_eq!(super::budget_from_meminfo("MemTotal: 1 kB\n", true), None);
    }
}
