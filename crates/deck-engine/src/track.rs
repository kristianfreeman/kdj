//! A fully decoded, analysed track, ready to play. Immutable once built and
//! shared between threads behind an `Arc`.

use crate::grid::BeatGrid;

pub struct Track {
    /// Changes on every load so the UI can tell tracks apart.
    pub id: u64,
    pub title: String,
    pub sample_rate: u32,
    /// Interleaved stereo at `sample_rate`, which always matches the output device.
    pub samples: Vec<f32>,
    /// The analysed grid. The engine keeps its own copy, which manual edits change.
    pub grid: BeatGrid,
    /// Where `grid` came from.
    pub grid_source: GridSource,
    /// Where the cue sits on load, in frames (Rekordbox's first memory cue).
    pub load_cue: Option<f64>,
    /// Hot cues A–H.
    pub hot_cues: [Option<HotCue>; HOT_CUES],
    pub waveform: Waveform,
}

impl std::fmt::Debug for Track {
    // Not derived: the sample buffer is millions of floats.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Track")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("sample_rate", &self.sample_rate)
            .field("frames", &self.frames())
            .field("grid", &self.grid)
            .field("grid_source", &self.grid_source)
            .field("load_cue", &self.load_cue)
            .field("hot_cues", &self.hot_cues)
            .field("waveform", &self.waveform)
            .finish_non_exhaustive()
    }
}

impl Track {
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "frame counts are far below f64's 2^52")]
    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate)
    }
}

pub const HOT_CUES: usize = 8;

/// A hot cue, or a hot loop if it has an end. Frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HotCue {
    pub start: f64,
    pub end: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridSource {
    Detected,
    Rekordbox,
}

/// Pioneer-style 3-band waveform: per column, the peak level of the low, mid
/// and high bands, 0..=255.
pub struct Waveform {
    /// Frames per column.
    pub hop: usize,
    pub columns: Vec<Bands>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bands {
    pub low: u8,
    pub mid: u8,
    pub high: u8,
}

impl std::fmt::Debug for Waveform {
    // Not derived: there are thousands of columns.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Waveform").field("hop", &self.hop).field("columns", &self.columns.len()).finish()
    }
}

impl Waveform {
    /// Reduce to `width` columns by averaging each band, for the overview
    /// strip. (Peaks would saturate: over seconds of music, every band hits
    /// its max somewhere.)
    #[must_use]
    pub fn overview(&self, width: usize) -> Vec<Bands> {
        let n = self.columns.len();
        if n == 0 {
            return vec![Bands::default(); width];
        }
        (0..width)
            .map(|i| {
                let a = (i * n / width).min(n - 1);
                let b = ((i + 1) * n / width).clamp(a + 1, n);
                let span = &self.columns[a..b];
                // A mean of u8s always fits a u8; the fallback is unreachable.
                let mean = |f: fn(&Bands) -> u8| {
                    u8::try_from(span.iter().map(|c| usize::from(f(c))).sum::<usize>() / span.len()).unwrap_or(u8::MAX)
                };
                Bands { low: mean(|c| c.low), mid: mean(|c| c.mid), high: mean(|c| c.high) }
            })
            .collect()
    }
}
