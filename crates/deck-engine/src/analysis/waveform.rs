//! Pioneer-style 3-band waveform: low (kick, bass), mid, high (hats), each as
//! a peak level per column, 150 columns per second.

use crate::track::{Bands, Waveform};

pub const COLUMNS_PER_SEC: f64 = 150.0;
const LOW_HZ: f64 = 200.0;
const HIGH_HZ: f64 = 2_500.0;
/// Display gain per band, so hats read as clearly as Pioneer draws them.
const GAIN: [f32; 3] = [1.0, 1.3, 2.0];

/// Peak level per band for each column of `stereo`, normalised for display.
#[must_use]
pub fn analyze(stereo: &[f32], sample_rate: u32) -> Waveform {
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a few hundred frames per column")]
    let hop = (f64::from(sample_rate) / COLUMNS_PER_SEC).round() as usize;
    let mut bands = BandSplit::new(f64::from(sample_rate));
    let mut peaks: Vec<[f32; 3]> = Vec::with_capacity(stereo.len() / 2 / hop + 1);
    for column in stereo.chunks(hop * 2) {
        let mut peak = [0.0f32; 3];
        for &[left, right] in column.as_chunks::<2>().0 {
            for (p, level) in peak.iter_mut().zip(bands.run(f32::midpoint(left, right))) {
                *p = p.max(level.abs());
            }
        }
        peaks.push(peak);
    }

    // Normalise so the loudest ~1% of columns reach full height.
    let mut tops: Vec<f32> = peaks.iter().map(|p| p[0].max(p[1]).max(p[2])).collect();
    tops.sort_by(f32::total_cmp);
    let scale = tops.get(tops.len() * 99 / 100).copied().unwrap_or(1.0).max(1e-6);
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0..=255 first")]
    let q = |v: f32, band: usize| ((v * GAIN[band] / scale).min(1.0) * 255.0).round() as u8;
    let columns = peaks.iter().map(|p| Bands { low: q(p[0], 0), mid: q(p[1], 1), high: q(p[2], 2) }).collect();
    Waveform { hop, columns }
}

/// Splits mono audio into low, mid and high bands, 24 dB/octave each.
struct BandSplit {
    low: [Biquad; 2],
    /// A real band-pass for mids; "signal minus lows minus highs" leaks bass
    /// through the filters' phase shift and buries the lows.
    mid: [Biquad; 4],
    high: [Biquad; 2],
}

impl BandSplit {
    fn new(sr: f64) -> Self {
        Self {
            low: [Biquad::lowpass(sr, LOW_HZ), Biquad::lowpass(sr, LOW_HZ)],
            mid: [
                Biquad::highpass(sr, LOW_HZ),
                Biquad::highpass(sr, LOW_HZ),
                Biquad::lowpass(sr, HIGH_HZ),
                Biquad::lowpass(sr, HIGH_HZ),
            ],
            high: [Biquad::highpass(sr, HIGH_HZ), Biquad::highpass(sr, HIGH_HZ)],
        }
    }

    /// One sample in, `[low, mid, high]` out.
    fn run(&mut self, x: f32) -> [f32; 3] {
        let chain = |filters: &mut [Biquad]| filters.iter_mut().fold(x, |v, f| f.run(v));
        [chain(&mut self.low), chain(&mut self.mid), chain(&mut self.high)]
    }
}

/// RBJ biquad, transposed direct form II.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn lowpass(sr: f64, hz: f64) -> Self {
        let (cos, alpha) = Self::prewarp(sr, hz);
        Self::norm([(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0], 1.0 + alpha, [-2.0 * cos, 1.0 - alpha])
    }

    fn highpass(sr: f64, hz: f64) -> Self {
        let (cos, alpha) = Self::prewarp(sr, hz);
        Self::norm(
            [f64::midpoint(1.0, cos), -(1.0 + cos), f64::midpoint(1.0, cos)],
            1.0 + alpha,
            [-2.0 * cos, 1.0 - alpha],
        )
    }

    fn prewarp(sr: f64, hz: f64) -> (f64, f64) {
        let w0 = std::f64::consts::TAU * hz / sr;
        (w0.cos(), w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2))
    }

    #[expect(clippy::cast_possible_truncation, reason = "designed in f64, run in f32")]
    fn norm(b: [f64; 3], a0: f64, a: [f64; 2]) -> Self {
        Self { b: b.map(|v| (v / a0) as f32), a: a.map(|v| (v / a0) as f32), z: [0.0; 2] }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0].mul_add(x, self.z[0]);
        self.z[0] = self.b[1].mul_add(x, -self.a[0] * y) + self.z[1];
        self.z[1] = self.b[2].mul_add(x, -self.a[1] * y);
        y
    }
}
