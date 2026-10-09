//! Synthetic hypnotic-techno tracks with known ground truth.
//!
//! Kick on every beat, offbeat hats, claps on 2 and 4, rolling 16th bass.
//! Used by tests, the simulator and perf runs, so nothing depends on
//! copyrighted audio.

#[derive(Clone, Debug)]
pub struct SynthSpec {
    pub bpm: f64,
    pub sample_rate: u32,
    /// Total bars, including intro and breakdown.
    pub bars: u32,
    /// Bars of hats only at the start; the first kick is bar 1 of the grid.
    pub intro_bars: u32,
    /// (first bar, length) with the kick and bass muted.
    pub breakdown: Option<(u32, u32)>,
    /// Silence before the intro, in seconds.
    pub lead_in: f64,
    pub seed: u32,
}

impl Default for SynthSpec {
    fn default() -> Self {
        Self {
            bpm: 134.0,
            sample_rate: 48_000,
            bars: 64,
            intro_bars: 8,
            breakdown: Some((40, 8)),
            lead_in: 0.137,
            seed: 1,
        }
    }
}

impl SynthSpec {
    /// Where the first kick lands, in seconds: the true grid anchor.
    #[must_use]
    pub fn first_kick(&self) -> f64 {
        self.lead_in + f64::from(self.intro_bars) * 4.0 * 60.0 / self.bpm
    }

    /// Length of the render, in seconds: every bar plus a second of tail.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.lead_in + f64::from(self.bars) * 4.0 * 60.0 / self.bpm + 1.0
    }
}

/// Render to interleaved stereo.
#[must_use]
pub fn render(spec: &SynthSpec) -> Vec<f32> {
    let sr = f64::from(spec.sample_rate);
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a positive length of a few minutes")]
    let frames = (spec.duration() * sr) as usize;
    let mut out = vec![0.0f32; frames];
    let beat = 60.0 / spec.bpm;
    let mut rng = spec.seed.max(1);
    let mut noise = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (f64::from(rng) / f64::from(u32::MAX)).mul_add(2.0, -1.0)
    };

    let total_beats = spec.bars * 4;
    for b in 0..total_beats {
        let bar = b / 4;
        let t = f64::from(b).mul_add(beat, spec.lead_in);
        let intro = bar < spec.intro_bars;
        let broken = spec.breakdown.is_some_and(|(s, l)| bar >= s && bar < s + l);
        if !intro && !broken {
            add(&mut out, sr, t, 0.35, kick);
            for s in 1..4 {
                add(&mut out, sr, t + f64::from(s) * beat / 4.0, 0.12, bass);
            }
        }
        add(&mut out, sr, t + beat / 2.0, 0.04, |x| 0.22 * hat(x, noise()));
        if !intro && b % 2 == 1 {
            add(&mut out, sr, t, 0.08, |x| 0.25 * clap(x, noise()));
        }
    }
    out.iter().flat_map(|&v| [v, v]).collect()
}

/// Mix `len` seconds of `voice` into `out`, starting at `at` seconds. Voices
/// are computed in f64 and stored as f32 samples.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "positive sample positions of a few minutes, and f64 voices mixed into f32 audio"
)]
fn add(out: &mut [f32], sr: f64, at: f64, len: f64, mut voice: impl FnMut(f64) -> f64) {
    let start = (at * sr).round() as usize;
    let n = (len * sr) as usize;
    for i in 0..n {
        if let Some(s) = out.get_mut(start + i) {
            *s += voice(i as f64 / sr) as f32;
        }
    }
}

/// Pitch-swept sine: 160 Hz falling to 45 Hz, 1 ms attack.
fn kick(t: f64) -> f64 {
    let f0 = 45.0;
    let sweep = 115.0;
    let k = 35.0;
    let phase = std::f64::consts::TAU * (f0 * t + sweep * (1.0 - (-k * t).exp()) / k);
    let env = (t / 0.001).min(1.0) * (-t * 9.0).exp();
    0.8 * env * phase.sin()
}

fn bass(t: f64) -> f64 {
    let env = (t / 0.003).min(1.0) * (-t * 18.0).exp();
    0.3 * env * (std::f64::consts::TAU * 55.0 * t).sin()
}

fn hat(t: f64, n: f64) -> f64 {
    n * (-t * 90.0).exp()
}

fn clap(t: f64, n: f64) -> f64 {
    n * (-t * 30.0).exp()
}
