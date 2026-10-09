//! Compare our beat-grid detector with Rekordbox's grids for the same files.

use std::ops::Range;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use kdj_engine::BeatGrid;
use kdj_engine::analysis;
use kdj_library::anlz;

struct Candidate {
    audio: PathBuf,
    rb: BeatGrid,
    secs: f64,
}

/// Grids in the local collection whose audio file is on disk and whose
/// tempo is steady.
fn candidates(sample_rate: u32) -> Result<Vec<Candidate>> {
    let dir = anlz::local_usbanlz().ok_or_else(|| anyhow!("no local Rekordbox collection found"))?;
    let mut out = Vec::new();
    for dat in anlz::scan(&dir) {
        let Ok(a) = anlz::read(&dat) else { continue };
        let (Some(audio), Some(fit)) = (a.audio_path(None), a.grid(sample_rate)) else { continue };
        if fit.variable {
            continue;
        }
        let secs = a.beats.last().map_or(0.0, |b| f64::from(b.ms) / 1000.0);
        out.push(Candidate { audio, rb: fit.grid, secs });
    }
    Ok(out)
}

pub fn list(sample_rate: u32) -> Result<bool> {
    let c = candidates(sample_rate)?;
    for t in &c {
        println!("{:>7.2} bpm {:>5.1} min  {}", t.rb.bpm, t.secs / 60.0, t.audio.display());
    }
    println!("{} tracks with a steady grid and audio on disk", c.len());
    Ok(true)
}

/// Benchmark `count` tracks spread across tempo, mixing lengths.
pub fn bench(sample_rate: u32, count: usize) -> Result<bool> {
    let mut c = candidates(sample_rate)?;
    if c.is_empty() {
        return Err(anyhow!("no usable tracks"));
    }
    c.sort_by(|a, b| a.rb.bpm.total_cmp(&b.rb.bpm));
    let picks = spread(&c, count);

    println!(
        "{:<40} {:>4} {:>5} {:>8} {:>8} {:>7} {:>9} {:>9} {:>5}",
        "track", "ext", "min", "rb bpm", "our bpm", "Δbpm", "Δ start", "Δ end", "bar"
    );
    let mut good = 0;
    let mut offsets = Vec::new();
    let mut bars_ok = 0;
    for t in &picks {
        let (track, _) = analysis::load_file(&t.audio, sample_rate)?;
        let ours = track.grid;
        let frames = frames_f64(track.frames());
        let end = frames * 0.95;
        let start = t.rb.frame_of(t.rb.beat_at(frames * 0.05).round());
        let (d0, d1) = (offset_ms(&ours, &t.rb, start), offset_ms(&ours, &t.rb, end));
        let bar = bar_shift(&ours, &t.rb, start);
        let ok = (ours.bpm - t.rb.bpm).abs() < 0.02 && d0.abs() < 5.0 && d1.abs() < 5.0 && bar == 0;
        good += usize::from(ok);
        offsets.push(d0);
        bars_ok += usize::from(bar == 0);
        let name = t.audio.file_stem().unwrap_or_default().to_string_lossy();
        let ext = t.audio.extension().unwrap_or_default().to_string_lossy().to_lowercase();
        println!(
            "{:<40} {:>4} {:>5.1} {:>8.3} {:>8.3} {:>+7.3} {:>+7.1}ms {:>+7.1}ms {:>+5} {}",
            truncate(&name, 40),
            ext,
            track.duration_secs() / 60.0,
            t.rb.bpm,
            ours.bpm,
            ours.bpm - t.rb.bpm,
            d0,
            d1,
            bar,
            if ok { "ok" } else { "MISS" }
        );
    }
    println!("\n{good}/{} within 0.02 bpm, 5 ms and the same bar 1", picks.len());
    let mut abs: Vec<f64> = offsets.iter().map(|v| v.abs()).collect();
    abs.sort_by(f64::total_cmp);
    let mut signed = offsets.clone();
    signed.sort_by(f64::total_cmp);
    println!(
        "phase vs Rekordbox: median {:+.1} ms, median |offset| {:.1} ms, {}/{} within 5 ms; bar 1 agrees on {}/{}",
        signed[signed.len() / 2],
        abs[abs.len() / 2],
        abs.iter().filter(|v| **v < 5.0).count(),
        abs.len(),
        bars_ok,
        abs.len()
    );
    Ok(true)
}

/// `count` candidates (sorted by tempo) evenly spaced through the tempo
/// range, alternating the shortest and longest of each neighbourhood so
/// lengths vary too.
fn spread(c: &[Candidate], count: usize) -> Vec<&Candidate> {
    if count >= c.len() {
        return c.iter().collect();
    }
    (0..count)
        .filter_map(|i| {
            let centre = i * (c.len() - 1) / (count.max(2) - 1).max(1);
            let lo = centre.saturating_sub(2);
            let hi = (centre + 3).min(c.len());
            let near = c[lo..hi].iter();
            if i % 2 == 0 {
                near.min_by(|a, b| a.secs.total_cmp(&b.secs))
            } else {
                near.max_by(|a, b| a.secs.total_cmp(&b.secs))
            }
        })
        .collect()
}

/// How far our nearest beat is from Rekordbox's beat near `frame`, in ms
/// (positive: ours is later).
fn offset_ms(ours: &BeatGrid, rb: &BeatGrid, frame: f64) -> f64 {
    let rb_beat = rb.frame_of(rb.beat_at(frame).round());
    let our_beat = ours.frame_of(ours.beat_at(rb_beat).round());
    (our_beat - rb_beat) / f64::from(ours.sample_rate) * 1000.0
}

/// Beats between our bar 1 and Rekordbox's, -1..=2 (0 means they agree).
fn bar_shift(ours: &BeatGrid, rb: &BeatGrid, frame: f64) -> i64 {
    let rb_beat = rb.frame_of(rb.beat_at(frame).round());
    let s = (beat_index(ours.beat_at(rb_beat)) - beat_index(rb.beat_at(rb_beat))).rem_euclid(4);
    if s > 2 { s - 4 } else { s }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_owned() } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

/// Where the kick's attack lands relative to each grid's beats: average the
/// low-passed audio around every beat and find the half-rise point.
pub fn probe(sample_rate: u32) -> Result<bool> {
    println!("{:<40} {:>13} {:>13} {:>10} {:>12}", "track", "kick vs rb", "kick vs ours", "rb − ours", "lame delay");
    for t in candidates(sample_rate)? {
        let (track, _) = analysis::load_file(&t.audio, sample_rate)?;
        let env = low_envelope(&track.samples, sample_rate);
        let name = t.audio.file_stem().unwrap_or_default().to_string_lossy();
        let rb = attack_ms(&env, &t.rb, track.frames(), sample_rate);
        let ours = attack_ms(&env, &track.grid, track.frames(), sample_rate);
        let d = analysis::decode::decode(&t.audio, None)?;
        let delay = d
            .trimmed_delay
            .map_or_else(|| "none".into(), |f| format!("{:.1} ms", f64::from(f) / f64::from(d.sample_rate) * 1000.0));
        let grid_gap = (t.rb.anchor - track.grid.anchor) / f64::from(sample_rate) * 1000.0;
        let p = track.grid.period() / f64::from(sample_rate) * 1000.0;
        let gap = (grid_gap + p / 2.0).rem_euclid(p) - p / 2.0;
        println!("{:<40} {:>+10.1} ms {:>+10.1} ms {:>+7.1} ms {:>12}", truncate(&name, 40), rb, ours, gap, delay);
    }
    Ok(true)
}

/// |lowpass(mono, 150 Hz)|, two one-pole stages.
fn low_envelope(stereo: &[f32], sample_rate: u32) -> Vec<f32> {
    let a = (-std::f32::consts::TAU * 150.0 / rate_f32(sample_rate)).exp();
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    let (frames, _) = stereo.as_chunks::<2>();
    frames
        .iter()
        .map(|&[l, r]| {
            let x = f32::midpoint(l, r);
            y1 = a.mul_add(y1, (1.0 - a) * x);
            y2 = a.mul_add(y2, (1.0 - a) * y1);
            y2.abs()
        })
        .collect()
}

/// The window around each beat that `attack_ms` averages, in ms.
const WINDOW_MS: Range<i32> = -60..100;
/// How many leading ms of the window (before any attack) set the baseline.
const BASELINE_MS: usize = 40;

/// Half-rise time of the beat-averaged envelope, in ms after the grid's beat.
fn attack_ms(env: &[f32], g: &BeatGrid, frames: usize, sample_rate: u32) -> f64 {
    let avg = beat_average(env, g, frames, sample_rate);
    if avg.is_empty() {
        return f64::NAN;
    }
    let base = avg[..BASELINE_MS].iter().copied().fold(f64::MAX, f64::min);
    let Some(peak_i) = (BASELINE_MS..avg.len()).max_by(|&a, &b| avg[a].total_cmp(&avg[b])) else {
        return f64::NAN;
    };
    let half = 0.5f64.mul_add(avg[peak_i] - base, base);
    let cross = (BASELINE_MS..=peak_i).find(|&i| avg[i] >= half).unwrap_or(peak_i);
    WINDOW_MS.map(f64::from).nth(cross).unwrap_or(f64::NAN)
}

/// The envelope averaged over every beat that has the whole window inside
/// the track, one value per ms of `WINDOW_MS`; empty if no beat does.
fn beat_average(env: &[f32], g: &BeatGrid, frames: usize, sample_rate: u32) -> Vec<f64> {
    let ms = f64::from(sample_rate) / 1000.0;
    let per_ms = (sample_rate / 1000) as usize;
    // Window offsets in frames, computed once.
    let offsets: Vec<f64> = WINDOW_MS.map(|o| f64::from(o) * ms).collect();
    let (lead, tail) = (f64::from(WINDOW_MS.start) * ms, f64::from(WINDOW_MS.end) * ms);
    let end = frames_f64(frames);
    let mut avg = vec![0.0f64; offsets.len()];
    let mut any = false;
    let last = g.beat_at(end).floor();
    let beats = std::iter::successors(Some(g.beat_at(0.0).ceil()), |k| Some(k + 1.0)).take_while(|&k| k < last);
    for b in beats.map(|k| g.frame_of(k)) {
        if b + lead < 0.0 || b + tail >= end {
            continue;
        }
        for (slot, off) in avg.iter_mut().zip(&offsets) {
            let at = frame_index(b + off);
            // Mean over the millisecond.
            let w = &env[at..(at + per_ms).min(env.len())];
            *slot += w.iter().map(|&v| f64::from(v)).sum::<f64>() / frames_f64(w.len().max(1));
        }
        any = true;
    }
    if any { avg } else { Vec::new() }
}

/// A frame count as f64.
#[expect(clippy::cast_precision_loss, reason = "frame counts stay far below 2^52")]
const fn frames_f64(n: usize) -> f64 {
    n as f64
}

/// The frame at a non-negative position (callers check it's inside the track).
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "callers keep `x` in 0..frames")]
const fn frame_index(x: f64) -> usize {
    x as usize
}

/// The nearest whole beat number.
#[expect(clippy::cast_possible_truncation, reason = "beat numbers in a track are tiny next to i64")]
const fn beat_index(beats: f64) -> i64 {
    beats.round() as i64
}

/// A sample rate as f32, for single-precision filter coefficients.
#[expect(clippy::cast_precision_loss, reason = "sample rates are far below 2^24, so exact in f32")]
const fn rate_f32(sample_rate: u32) -> f32 {
    sample_rate as f32
}
