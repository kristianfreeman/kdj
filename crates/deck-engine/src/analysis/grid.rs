//! Tempo and beat-grid detection for fixed-tempo electronic music.
//!
//! 1. Onset curves: spectral flux of a 12 kHz mono copy. Full band, log
//!    compressed, for tempo; below 200 Hz, linear, for where the kicks are
//!    (linear so a loud kick beats a rolling offbeat bassline).
//! 2. Coarse tempo: autocorrelation of the full-band curve, scored as a comb
//!    over 8 beats, searched across one octave (88–176 BPM by default) so
//!    half- and double-time can't be confused. 3:2 and 4:3 relatives are
//!    checked too, since offbeat hats make them look periodic.
//! 3. Refine: fold the whole track at candidate tempos. The right tempo stacks
//!    every beat into one phase bin; a wrong one smears them. Over a few
//!    minutes this resolves well under 0.01 BPM.
//! 4. Phase: average the low-passed audio around every beat to get one clean
//!    picture of the kick, move to its loudest point (the kick, not an
//!    offbeat bass note) and put the beat where its attack starts rising,
//!    which is where Rekordbox puts it too.
//! 5. Downbeat: the first beat with a real kick becomes bar 1.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

use crate::grid::BeatGrid;

#[derive(Clone, Copy, Debug)]
pub struct GridConfig {
    /// Bottom of the one-octave search range.
    pub min_bpm: f64,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self { min_bpm: 88.0 }
    }
}

const ENV_RATE: f64 = 12_000.0;
const FRAME: usize = 512;
const HOP: usize = 64;
/// `FRAME` and `HOP` as floats, for converting onset frames to seconds.
#[expect(clippy::cast_precision_loss, reason = "a small constant, exact in f64")]
const FRAME_F64: f64 = FRAME as f64;
#[expect(clippy::cast_precision_loss, reason = "a small constant, exact in f64")]
const HOP_F64: f64 = HOP as f64;
const LOW_HZ: f64 = 200.0;
/// Offset between where spectral flux peaks and the kick's attack, in
/// seconds. Calibrated against synthetic kicks (see tests).
const ONSET_DELAY: f64 = 0.004;

struct Onsets {
    full: Vec<f32>,
    low: Vec<f32>,
    /// Onset frames per second.
    rate: f64,
    /// Time of onset frame 0, in seconds.
    t0: f64,
}

/// Detect tempo and grid from mono audio at `sample_rate`.
#[must_use]
pub fn detect(mono: &[f32], sample_rate: u32, cfg: GridConfig) -> BeatGrid {
    let fallback = BeatGrid::new(120.0, 0.0, sample_rate);
    if mono.len() < sample_rate as usize * 4 {
        return fallback;
    }
    let on = onsets(mono, sample_rate);
    let Some(coarse) = coarse_tempo(&on, cfg.min_bpm) else { return fallback };
    let bpm = refine_tempo(&on, coarse, cfg.min_bpm);

    // Phase from the kicks if there are any, otherwise from everything.
    let odf = if on.low.iter().sum::<f32>() > 0.05 * on.full.iter().sum::<f32>() { &on.low } else { &on.full };
    let mut hist = vec![0.0f32; 256];
    fold(odf, on.rate, bpm, &mut hist);
    let phase = peak_phase(&hist); // fraction of a beat, relative to onset frame 0
    let period_s = 60.0 / bpm;
    let rough_s = on.t0 + phase * period_s + ONSET_DELAY;
    let first_beat_s = refine_phase(mono, sample_rate, period_s, rough_s);

    // Bar 1 lines up with the first kick; then step back whole bars so an
    // intro counts up from bar 1 too (phrases run from the top of the track).
    let anchor_s = walk_back(first_downbeat(odf, &on, first_beat_s, period_s), 4.0 * period_s);
    BeatGrid::new(bpm, anchor_s * f64::from(sample_rate), sample_rate)
}

/// From a coarse tempo to a precise one: pick the right musical relative,
/// then narrow in by folding the whole track at finer and finer steps.
fn refine_tempo(on: &Onsets, coarse: f64, min_bpm: f64) -> f64 {
    let mut hist = vec![0.0f32; 128];
    // Offbeat hats make 2/3 of the tempo look as periodic as the tempo itself
    // (every 1.5 beats lands on a kick or a hat). Try the musical relatives
    // too, and keep whichever stacks the most onset energy into one phase:
    // at the true tempo every kick lands in the same bin.
    let max_bpm = 2.0 * min_bpm;
    // A coarse look at each relative picks the family; only the winner gets
    // the fine search.
    let (family, _) = [1.0, 1.5, 2.0 / 3.0, 4.0 / 3.0, 0.75]
        .into_iter()
        .map(|r| coarse * r)
        .filter(|b| (min_bpm..max_bpm).contains(b))
        .map(|b| search(on, b, 0.6, 0.05, &mut hist[..64]))
        .fold((coarse, f32::MIN), higher_score);
    let (bpm, _) = search(on, family, 0.06, 0.005, &mut hist[..64]);
    let (bpm, score) = search(on, bpm, 0.006, 0.0005, &mut hist);

    // Tracks made in a DAW sit on whole BPMs. Prefer that when it fits as well.
    let whole = bpm.round();
    if (bpm - whole).abs() < 0.03 && fold(&on.full, on.rate, whole, &mut hist) >= 0.9 * score { whole } else { bpm }
}

/// The tempo within `span` BPM of `centre` (in steps of `step`) that folds
/// the full-band onsets most sharply, with its score.
fn search(on: &Onsets, centre: f64, span: f64, step: f64, hist: &mut [f32]) -> (f64, f32) {
    #[expect(clippy::cast_possible_truncation, reason = "a few hundred steps at most")]
    let steps = (span / step).round() as i32;
    (-steps..=steps)
        .map(|i| f64::from(i).mul_add(step, centre))
        .map(|bpm| (bpm, fold(&on.full, on.rate, bpm, hist)))
        .fold((centre, f32::MIN), higher_score)
}

/// Keep whichever `(bpm, score)` scores higher; a tie keeps the first.
fn higher_score(best: (f64, f32), candidate: (f64, f32)) -> (f64, f32) {
    if candidate.1 > best.1 { candidate } else { best }
}

/// Step `t` back by whole `step`s as far as it goes without passing 20 ms
/// before the start (a kick right at 0 s may come out a hair negative).
fn walk_back(t: f64, step: f64) -> f64 {
    let steps = ((t + 0.02) / step).floor().max(0.0);
    steps.mul_add(-step, t)
}

fn onsets(mono: &[f32], sample_rate: u32) -> Onsets {
    // Crude decimation: boxcar average. Onset detection doesn't need clean highs.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small positive ratio of sample rates"
    )]
    let factor = (f64::from(sample_rate) / ENV_RATE).round().max(1.0) as u32;
    let env_sr = f64::from(sample_rate) / f64::from(factor);
    let x = decimate(mono, factor as usize);
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a few dozen bins, always positive")]
    let low_bins = (LOW_HZ / (env_sr / FRAME_F64)).ceil() as usize;
    let (mut full, mut low) = spectral_flux(&x, low_bins);
    detrend(&mut full, 16);
    detrend(&mut low, 16);
    // Frame 0 is centred half a frame into the padding, i.e. before 0 s.
    Onsets { full, low, rate: env_sr / HOP_F64, t0: -(FRAME_F64 / 2.0) / env_sr }
}

/// Boxcar-average `mono` down by `factor`, after one frame of silence so a
/// kick right at 0 s still registers as an onset.
#[expect(clippy::cast_precision_loss, reason = "chunks are at most `factor` (about 4) samples long")]
fn decimate(mono: &[f32], factor: usize) -> Vec<f32> {
    std::iter::repeat_n(0.0, FRAME).chain(mono.chunks(factor).map(|c| c.iter().sum::<f32>() / c.len() as f32)).collect()
}

/// Hann window, `len` samples long.
#[expect(clippy::cast_precision_loss, reason = "window indices are far below 2^24")]
fn hann(len: usize) -> Vec<f32> {
    (0..len).map(|i| 0.5f32.mul_add(-(std::f32::consts::TAU * i as f32 / len as f32).cos(), 0.5)).collect()
}

/// Positive spectral flux for each hop of `x`: full band, and the bins up
/// to `low_bins` (the kick and bass).
fn spectral_flux(x: &[f32], low_bins: usize) -> (Vec<f32>, Vec<f32>) {
    let window = hann(FRAME);
    let fft = FftPlanner::new().plan_fft_forward(FRAME);
    let frames = x.len().saturating_sub(FRAME) / HOP + 1;
    let mut flux = Flux { prev_log: vec![0.0; FRAME / 2], prev_low: vec![0.0; low_bins + 1] };
    let mut full = Vec::with_capacity(frames);
    let mut low = Vec::with_capacity(frames);
    let mut buf = vec![Complex::new(0.0f32, 0.0); FRAME];
    for frame in 0..frames {
        let samples = &x[frame * HOP..frame * HOP + FRAME];
        for ((cell, &sample), &gain) in buf.iter_mut().zip(samples).zip(&window) {
            *cell = Complex::new(sample * gain, 0.0);
        }
        fft.process(&mut buf);
        let (full_rise, low_rise) = flux.step(&buf, frame > 0);
        full.push(full_rise);
        low.push(low_rise);
    }
    (full, low)
}

/// The previous frame's magnitudes, for spectral flux: log-compressed for
/// the full band, linear for the low band.
struct Flux {
    prev_log: Vec<f32>,
    prev_low: Vec<f32>,
}

impl Flux {
    /// Total rise (full band, low band) since the last spectrum. The first
    /// frame only primes the state, so `count` is false for it.
    fn step(&mut self, spectrum: &[Complex<f32>], count: bool) -> (f32, f32) {
        let (mut full, mut low) = (0.0, 0.0);
        for (k, (bin, prev_log)) in spectrum.iter().zip(&mut self.prev_log).enumerate().skip(1) {
            let mag = bin.norm();
            // Full band: log-compressed, so quiet hats count toward tempo.
            let m = (100.0 * mag).ln_1p();
            let rise = m - *prev_log;
            if rise > 0.0 && count {
                full += rise;
            }
            *prev_log = m;
            // Low band: linear, so the kick outweighs an offbeat bassline.
            if let Some(prev) = self.prev_low.get_mut(k) {
                let rise = mag - *prev;
                if rise > 0.0 && count {
                    low += rise;
                }
                *prev = mag;
            }
        }
        (full, low)
    }
}

/// Subtract a moving average and keep only what pokes above it.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "windows are at most 2 * radius + 1 long; the mean goes back to f32 like the curve"
)]
fn detrend(x: &mut [f32], radius: usize) {
    let n = x.len();
    let prefix = prefix_sums(x.iter().copied());
    for (i, v) in x.iter_mut().enumerate() {
        let (lo, hi) = (i.saturating_sub(radius), (i + radius + 1).min(n));
        let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
        *v = (*v - mean as f32).max(0.0);
    }
}

/// Running sums in f64, starting from 0, so any window's sum is one subtraction.
fn prefix_sums(x: impl Iterator<Item = f32>) -> Vec<f64> {
    std::iter::once(0.0)
        .chain(x.scan(0.0, |sum, v| {
            *sum += f64::from(v);
            Some(*sum)
        }))
        .collect()
}

fn coarse_tempo(on: &Onsets, min_bpm: f64) -> Option<f64> {
    const COMB: u32 = 8;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a positive lag of a few hundred frames"
    )]
    let max_lag = (f64::from(COMB) * 60.0 * on.rate / min_bpm).ceil() as usize + 2;
    let acf = autocorr(&on.full, max_lag);
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "one octave in 0.02 BPM steps")]
    let steps = (min_bpm / 0.02) as u32;
    (0..steps)
        .map(|i| f64::from(i).mul_add(0.02, min_bpm))
        .map(|bpm| {
            let lag = 60.0 * on.rate / bpm;
            (bpm, (1..=COMB).map(|k| lerp_at(&acf, f64::from(k) * lag)).sum::<f32>())
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|b| b.1 > 0.0)
        .map(|b| b.0)
}

/// `x` at fractional index `pos` (non-negative), linearly interpolated; 0 at
/// or past the last sample.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pos is a non-negative lag of a few thousand frames; the weight only needs f32"
)]
fn lerp_at(x: &[f32], pos: f64) -> f32 {
    let i = pos.floor() as usize;
    let w = (pos - pos.floor()) as f32;
    if i + 1 >= x.len() { 0.0 } else { x[i + 1].mul_add(w, x[i] * (1.0 - w)) }
}

/// Autocorrelation for lags `0..max_lag`, normalised by overlap length.
#[expect(clippy::cast_precision_loss, reason = "onset curves run ~190 frames/s, so lengths stay far below 2^24")]
fn autocorr(x: &[f32], max_lag: usize) -> Vec<f32> {
    let n = (x.len() * 2).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let mut buf: Vec<Complex<f32>> =
        x.iter().map(|&v| Complex::new(v, 0.0)).chain(std::iter::repeat(Complex::new(0.0, 0.0))).take(n).collect();
    planner.plan_fft_forward(n).process(&mut buf);
    for c in &mut buf {
        *c = Complex::new(c.norm_sqr(), 0.0);
    }
    planner.plan_fft_inverse(n).process(&mut buf);
    (0..max_lag.min(x.len())).map(|lag| buf[lag].re / (n as f32 * (x.len() - lag) as f32)).collect()
}

/// Stack the onset curve into phase bins one beat long. Returns the tallest bin.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "hot loop: a few hundred bins at most, and `p` is a non-negative bin position"
)]
fn fold(odf: &[f32], rate: f64, bpm: f64, hist: &mut [f32]) -> f32 {
    hist.fill(0.0);
    let bins = hist.len();
    let step = bpm / (60.0 * rate); // beats per onset frame
    let mut phase = 0.0f64;
    for &v in odf {
        if v > 0.0 {
            let p = phase * bins as f64;
            let i = p as usize % bins;
            let next = (i + 1) % bins;
            let w = (p - p.floor()) as f32;
            hist[i] = v.mul_add(1.0 - w, hist[i]);
            hist[next] = v.mul_add(w, hist[next]);
        }
        phase += step;
        if phase >= 1.0 {
            phase -= 1.0;
        }
    }
    hist.iter().copied().fold(0.0, f32::max)
}

/// Phase of the tallest bin, refined between neighbours, as a fraction of a beat.
#[expect(clippy::cast_precision_loss, reason = "histograms have a few hundred bins at most")]
fn peak_phase(hist: &[f32]) -> f64 {
    let n = hist.len();
    let i = (0..n).max_by(|&a, &b| hist[a].total_cmp(&hist[b])).unwrap_or(0);
    let (before, peak, after) = (f64::from(hist[(i + n - 1) % n]), f64::from(hist[i]), f64::from(hist[(i + 1) % n]));
    let denom = 2.0f64.mul_add(-peak, before) + after;
    let offset = if denom.abs() > 1e-12 { 0.5 * (before - after) / denom } else { 0.0 };
    ((i as f64 + offset) / n as f64).rem_euclid(1.0)
}

/// The first beat that carries a real hit, so bar 1 lands where the music starts.
fn first_downbeat(odf: &[f32], on: &Onsets, first_beat_s: f64, period_s: f64) -> f64 {
    let strength = |t: f64| {
        #[expect(clippy::cast_possible_truncation, reason = "onset frame indices fit easily in i64")]
        let centre = ((t - on.t0 - ONSET_DELAY) * on.rate).round() as i64;
        (centre - 2..=centre + 2)
            .filter_map(|j| usize::try_from(j).ok())
            .filter_map(|j| odf.get(j))
            .copied()
            .fold(0.0, f32::max)
    };
    // Walk back to the earliest beat in the file.
    let earliest = walk_back(first_beat_s, period_s);
    #[expect(clippy::cast_precision_loss, reason = "onset curves run ~190 frames/s, far below 2^52")]
    let duration = odf.len() as f64 / on.rate;
    let beats = beat_times(earliest, period_s, duration);
    let levels: Vec<f32> = beats.iter().map(|&t| strength(t)).collect();
    let mut sorted = levels.clone();
    sorted.sort_by(f32::total_cmp);
    let loud = sorted.get(sorted.len() * 9 / 10).copied().unwrap_or(0.0);
    let first = levels.iter().position(|v| *v > 0.35 * loud).unwrap_or(0);
    beats.get(first).copied().unwrap_or(earliest)
}

/// Beat times from `first`, every `period`, before `end` (all in seconds).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a non-negative beat count, far below u32::MAX"
)]
fn beat_times(first: f64, period: f64, end: f64) -> Vec<f64> {
    // An upper bound on the count; the exact test below decides.
    let count = ((end - first) / period).ceil().max(0.0) as u32;
    (0..=count).map(|k| f64::from(k).mul_add(period, first)).take_while(|&t| t < end).collect()
}

/// Fraction of the kick's rise, from the quiet before it to the top of the
/// attack, that marks the beat. Low means "where the attack starts".
const ATTACK_FRACTION: f64 = 0.2;

/// Sharpen a rough beat time using the beat-synchronous average of the
/// low-band envelope: one clean picture of the kick, averaged over the track.
/// Returns the refined time of a beat, in seconds.
fn refine_phase(mono: &[f32], sample_rate: u32, period_s: f64, rough_s: f64) -> f64 {
    let low = Profile::new(&zero_phase_low_envelope(mono, sample_rate, 120.0), sample_rate, period_s, rough_s);

    // On the beat or the offbeat? Whichever carries more low end just after.
    let half = low.bins / 2;
    let centre = if low.mean(half, low.ms(60.0)) > 1.3 * low.mean(0, low.ms(60.0)) { half } else { 0 };

    // The kick's attack: the steepest rise of the click (high band) near the
    // estimate, walked back to where it starts. Hats sit on the offbeats, so
    // a ±30 ms window around the beat only sees the kick (and claps on it).
    let click_env = zero_phase_envelope(&high_pass(mono, sample_rate, 1500.0), sample_rate);
    let click = Profile::new(&click_env, sample_rate, period_s, rough_s);
    click.fraction(attack_start(&click, centre)).mul_add(period_s, rough_s)
}

/// Where the attack nearest `centre` starts, in bins: the steepest rise
/// within ±30 ms, walked back until it drops to `ATTACK_FRACTION` of the way
/// up from the quiet before it.
fn attack_start(click: &Profile, centre: i64) -> i64 {
    let rise = |i: i64| click.at(i + 1) - click.at(i);
    let (lo, hi) = (centre - click.ms(30.0), centre + click.ms(30.0));
    let steepest = (lo..hi).max_by(|&x, &y| rise(x).total_cmp(&rise(y))).unwrap_or(centre);
    let floor = (steepest - click.ms(20.0)..=steepest).map(|i| click.at(i)).fold(f64::MAX, f64::min);
    let top = (steepest..steepest + click.ms(15.0)).map(|i| click.at(i)).fold(f64::MIN, f64::max);
    let threshold = ATTACK_FRACTION.mul_add(top - floor, floor);
    let mut i = steepest;
    while click.at(i - 1) > threshold && i > steepest - click.ms(20.0) {
        i -= 1;
    }
    i
}

/// One beat of an envelope, summed over every whole beat in the track. Bin 0
/// is at `rough_s`; bins are ~0.25 ms and indices wrap.
struct Profile {
    slots: Vec<f64>,
    bins: i64,
    period_s: f64,
}

impl Profile {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        reason = "hot loop over sample positions: beat and bin counts are small, and each index is a \
                  non-negative position inside `env`"
    )]
    fn new(env: &[f32], sample_rate: u32, period_s: f64, rough_s: f64) -> Self {
        let sr = f64::from(sample_rate);
        let period = period_s * sr;
        let bins = (period_s * 4000.0) as usize;
        let origin = rough_s * sr;
        let len = env.len() as f64;
        let first = (-origin / period).ceil() as i64;
        // An upper bound on the beats that fit; the exact test below decides.
        let last = ((len - origin) / period).ceil() as i64;
        let mut slots = vec![0.0f64; bins];
        for k in (first..=last).take_while(|&k| ((k + 1) as f64).mul_add(period, origin) < len) {
            let start = (k as f64).mul_add(period, origin);
            for (b, slot) in slots.iter_mut().enumerate() {
                *slot += f64::from(env[(start + b as f64 * period / bins as f64) as usize]);
            }
        }
        Self { slots, bins: bins as i64, period_s }
    }

    /// The profile at bin `i`, wrapping.
    #[expect(clippy::cast_possible_truncation, reason = "rem_euclid gives a bin in 0..bins")]
    fn at(&self, i: i64) -> f64 {
        self.slots[i.rem_euclid(self.bins) as usize]
    }

    /// Mean of `len` bins from `from`.
    #[expect(clippy::cast_precision_loss, reason = "spans of a few hundred bins")]
    fn mean(&self, from: i64, len: i64) -> f64 {
        (from..from + len).map(|i| self.at(i)).sum::<f64>() / len as f64
    }

    /// `m` milliseconds, in bins.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "a few thousand bins per beat at most"
    )]
    fn ms(&self, m: f64) -> i64 {
        (m * self.bins as f64 / (self.period_s * 1000.0)).round() as i64
    }

    /// Bin `i` as a fraction of a beat.
    #[expect(clippy::cast_precision_loss, reason = "a few thousand bins per beat at most")]
    fn fraction(&self, i: i64) -> f64 {
        i as f64 / self.bins as f64
    }
}

/// Coefficient of a one-pole filter with its corner at `hz`.
#[expect(clippy::cast_precision_loss, reason = "sample rates are exact in f32")]
fn one_pole(hz: f32, sample_rate: u32) -> f32 {
    (-std::f32::consts::TAU * hz / sample_rate as f32).exp()
}

fn high_pass(mono: &[f32], sample_rate: u32, hz: f32) -> Vec<f32> {
    // One-pole high-pass, forward then backward (zero phase), twice.
    let a = one_pole(hz, sample_rate);
    let mut y = mono.to_vec();
    for _ in 0..2 {
        let (mut lp, mut out) = (0.0f32, y.clone());
        for (o, v) in out.iter_mut().zip(&y) {
            lp = a.mul_add(lp, (1.0 - a) * v);
            *o = v - lp;
        }
        let mut lp = 0.0f32;
        for v in out.iter_mut().rev() {
            lp = a.mul_add(lp, (1.0 - a) * *v);
            *v -= lp;
        }
        y = out;
    }
    y
}

/// Centred 2 ms moving average of |x|: an envelope with no delay.
fn zero_phase_envelope(x: &[f32], sample_rate: u32) -> Vec<f32> {
    centred_mean_abs(x, (sample_rate as usize / 1000).max(1))
}

/// Amplitude envelope of the low band with no filter delay: low-pass (two
/// one-pole stages forward, then backward), rectify, then a centred 4 ms
/// moving average so the kick's cycles don't read as silence between peaks.
fn zero_phase_low_envelope(mono: &[f32], sample_rate: u32, hz: f32) -> Vec<f32> {
    let a = one_pole(hz, sample_rate);
    let mut y = mono.to_vec();
    for _ in 0..2 {
        let mut z = 0.0f32;
        for v in &mut y {
            z = a.mul_add(z, (1.0 - a) * *v);
            *v = z;
        }
        let mut z = 0.0f32;
        for v in y.iter_mut().rev() {
            z = a.mul_add(z, (1.0 - a) * *v);
            *v = z;
        }
    }
    centred_mean_abs(&y, (sample_rate as usize * 2 / 1000).max(1))
}

/// Mean of |x| over `half` samples either side of each sample.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "windows are a few hundred samples; the envelope is f32 like the audio"
)]
fn centred_mean_abs(x: &[f32], half: usize) -> Vec<f32> {
    let prefix = prefix_sums(x.iter().map(|v| v.abs()));
    (0..x.len())
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(half), (i + half + 1).min(x.len()));
            ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32
        })
        .collect()
}

#[cfg(test)]
mod debug {
    use super::*;
    use crate::analysis::synth::{self, SynthSpec};

    #[test]
    #[ignore = "debugging aid: prints phase histograms"]
    #[expect(clippy::cast_possible_truncation, reason = "printing small histogram values")]
    fn dump_phase_histograms() {
        for (bpm, lead, seed) in [(134.0, 0.05, 1), (145.0, 1.088, 7)] {
            let spec = SynthSpec { bpm, lead_in: lead, seed, ..Default::default() };
            let st = synth::render(&spec);
            let mono: Vec<f32> = st.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
            let on = onsets(&mono, 48_000);
            for (name, odf) in [("low", &on.low), ("full", &on.full)] {
                let mut h = vec![0.0f32; 16];
                fold(odf, on.rate, bpm, &mut h);
                let truth = ((spec.first_kick() - on.t0) / (60.0 / bpm)).rem_euclid(1.0);
                println!(
                    "{bpm} {name}: truth phase {truth:.3}  hist {:?}",
                    h.iter().map(|v| v.round() as i64).collect::<Vec<_>>()
                );
            }
        }
    }
}
