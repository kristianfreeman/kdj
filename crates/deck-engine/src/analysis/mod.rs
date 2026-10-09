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
    let decoded = decode::decode(path)?;
    let mut timings = LoadTimings { decode: t.elapsed(), ..Default::default() };
    let src_rate = decoded.sample_rate;

    let grid_source = if known.grid.is_some() { GridSource::Rekordbox } else { GridSource::Detected };
    let (grid, samples, waveform) = std::thread::scope(|s| -> Result<_> {
        let grid = s.spawn(|| {
            if let Some(g) = known.grid {
                return (g, Duration::ZERO);
            }
            let t = Instant::now();
            let g = grid::detect(&mono(&decoded.samples), src_rate, grid::GridConfig::default());
            // Same beats, counted in frames at the deck's rate.
            let g =
                crate::grid::BeatGrid::new(g.bpm, g.anchor * f64::from(sample_rate) / f64::from(src_rate), sample_rate);
            (g, t.elapsed())
        });
        let t = Instant::now();
        let samples = resample::resample(decoded.samples.clone(), src_rate, sample_rate)?;
        timings.resample = t.elapsed();
        let t = Instant::now();
        let waveform = known.waveform.unwrap_or_else(|| waveform::analyze(&samples, sample_rate));
        timings.waveform = t.elapsed();
        let (g, gt) = grid.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        timings.grid = gt;
        Ok((g, samples, waveform))
    })?;

    let title = path.file_stem().map_or_else(|| "untitled".into(), |s| s.to_string_lossy().into_owned());
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
