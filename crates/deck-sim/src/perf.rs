//! Perf budgets: measure on this machine, scale to an estimated Pi 4, compare
//! against `perf-budgets.toml`. Exits non-zero if anything is over.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use deck_engine::analysis::{self, synth::SynthSpec};
use deck_engine::{Action, Edge, Track};
use serde::Deserialize;

use crate::sim::Sim;

#[derive(Deserialize)]
pub struct Budgets {
    host: Host,
    audio: Audio,
    load: Load,
    ui: Ui,
}

#[derive(Deserialize)]
struct Host {
    pi_slowdown: f64,
}

#[derive(Deserialize)]
struct Audio {
    sample_rate: u32,
    buffer_frames: u32,
    p99_of_period: f64,
    max_of_period: f64,
    seconds: f64,
}

#[derive(Deserialize)]
struct Load {
    total_seconds: f64,
}

#[derive(Deserialize)]
struct Ui {
    frame_cpu_ms: f64,
}

pub fn load_budgets(path: &Path) -> Result<Budgets> {
    let src = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&src).with_context(|| format!("parsing {}", path.display()))
}

struct Row {
    name: String,
    host_ms: f64,
    budget_ms: f64,
}

/// Measured UI frames, supplied by the caller so this module doesn't need a GPU.
pub struct UiFrames {
    /// CPU ms per warmed-up frame.
    pub ms: Vec<f64>,
    /// Heap allocations (and frees) across all of those frames.
    pub allocs: u64,
}

pub type UiBench = dyn Fn(&Arc<Track>) -> Result<UiFrames>;

pub fn run(b: &Budgets, track: Option<&Path>, ui_bench: Option<&UiBench>) -> Result<bool> {
    let (mut rows, track) = load_rows(b, track)?;
    rows.extend(callback_rows(b, &track)?);
    let ui_allocs = match ui_bench {
        Some(bench) => {
            let (row, allocs) = ui_row(b, &track, bench)?;
            rows.push(row);
            allocs
        }
        None => 0,
    };
    let ok = print_table(&rows, b.host.pi_slowdown);
    if ui_allocs > 0 {
        println!("ui frames allocated {ui_allocs} times once warmed up: OVER (must be zero)");
    }
    Ok(ok && ui_allocs == 0)
}

/// Load: a 7-minute track from disk, at 44.1 kHz so resampling is included,
/// plus `extra` if given. Returns the rows and the synthetic track.
fn load_rows(b: &Budgets, extra: Option<&Path>) -> Result<(Vec<Row>, Arc<Track>)> {
    let spec = SynthSpec { sample_rate: 44_100, bars: 245, ..Default::default() };
    let wav = std::env::temp_dir().join("deck-perf-7min.wav");
    write_wav(&wav, &analysis::synth::render(&spec), spec.sample_rate)?;
    let (row, synth) = measure_load(b, &wav)?;
    let mut rows = vec![row];
    if let Some(p) = extra {
        rows.push(measure_load(b, p)?.0);
    }
    Ok((rows, Arc::new(synth)))
}

/// Load one file, print the stage timings and normalise to a 7-minute track.
fn measure_load(b: &Budgets, p: &Path) -> Result<(Row, Track)> {
    let t = Instant::now();
    let (tr, tm) = analysis::load_file(p, b.audio.sample_rate)?;
    let wall = t.elapsed().as_secs_f64() * 1000.0;
    let mins = tr.duration_secs() / 60.0;
    let label = p.file_name().unwrap_or_default().to_string_lossy();
    println!(
        "load {label} ({mins:.1} min): decode {:.0} ms, resample {:.0}, grid {:.0}, waveform {:.0} (wall {wall:.0} ms) → {:.3} bpm",
        ms(tm.decode),
        ms(tm.resample),
        ms(tm.grid),
        ms(tm.waveform),
        tr.grid.bpm
    );
    let row = Row {
        name: format!("load 7 min ({label})"),
        host_ms: wall * 7.0 / mins,
        budget_ms: b.load.total_seconds * 1000.0,
    };
    Ok((row, tr))
}

/// Audio callback under a stress session.
fn callback_rows(b: &Budgets, track: &Arc<Track>) -> Result<[Row; 2]> {
    let period_ms = f64::from(b.audio.buffer_frames) / f64::from(b.audio.sample_rate) * 1000.0;
    let mut sim = Sim::new(Arc::clone(track), b.audio.buffer_frames as usize)?;
    stress(&mut sim, b.audio.seconds);
    let h = &sim.timings;
    let at = |q: f64| us(h.value_at_quantile(q));
    println!(
        "callback cpu ({} frames, {:.0} s, {} blocks): p50 {:.1} µs, p99 {:.1}, p99.9 {:.1}, max {:.1}",
        b.audio.buffer_frames,
        b.audio.seconds,
        h.len(),
        at(0.5),
        at(0.99),
        at(0.999),
        us(h.max())
    );
    println!(
        "callback wall max {:.1} µs (includes OS preemption of this non-real-time thread; not budgeted)",
        us(sim.wall.max())
    );
    Ok([
        Row { name: "callback p99".into(), host_ms: at(0.99) / 1000.0, budget_ms: period_ms * b.audio.p99_of_period },
        Row {
            name: "callback max".into(),
            host_ms: us(h.max()) / 1000.0,
            budget_ms: period_ms * b.audio.max_of_period,
        },
    ])
}

/// UI frame CPU time (build and draw submission), from the caller's
/// measurement; also returns the allocations it saw.
fn ui_row(b: &Budgets, track: &Arc<Track>, bench: &UiBench) -> Result<(Row, u64)> {
    let UiFrames { ms: mut frames, allocs } = bench(track)?;
    frames.sort_by(f64::total_cmp);
    let p99 = *frames.get(frames.len() * 99 / 100).context("the UI bench measured no frames")?;
    let p50 = frames[frames.len() / 2];
    println!(
        "ui frame cpu ({} frames, build + draw): p50 {p50:.3} ms, p99 {p99:.3}, {allocs} allocations",
        frames.len()
    );
    Ok((Row { name: "ui frame cpu p99".into(), host_ms: p99, budget_ms: b.ui.frame_cpu_ms }, allocs))
}

/// Print every row scaled to the Pi; true if all are within budget.
fn print_table(rows: &[Row], slow: f64) -> bool {
    println!("\n{:<28} {:>11} {:>13} {:>12}", "budget (Pi 4)", "host", format!("Pi est ×{slow}"), "budget");
    let mut ok = true;
    for r in rows {
        let pi = r.host_ms * slow;
        let pass = pi <= r.budget_ms;
        ok &= pass;
        println!(
            "{:<28} {:>11} {:>13} {:>12}  {}",
            r.name,
            fmt_ms(r.host_ms),
            fmt_ms(pi),
            fmt_ms(r.budget_ms),
            if pass { "ok" } else { "OVER" }
        );
    }
    ok
}

/// A frantic but plausible session: beat jumps, loop rolls, cue stabs,
/// key-lock flips and tempo rides, every quarter second.
fn stress(sim: &mut Sim, seconds: f64) {
    use Action::{
        BeatJumpBack, BeatJumpForward, BeatLoop1, BeatLoop8, Cue, KeyLock, LoopDouble, LoopHalve, LoopToggle,
        NudgeForward, PlayPause, TempoDownCoarse, TempoUp, TempoUpCoarse,
    };
    let palette = [
        BeatJumpForward,
        BeatJumpBack,
        LoopToggle,
        LoopHalve,
        LoopDouble,
        BeatLoop1,
        BeatLoop8,
        TempoUpCoarse,
        TempoDownCoarse,
        TempoUp,
        KeyLock,
        Cue,
        NudgeForward,
    ];
    let mut rng = 0x2545_f491u32;
    let mut next = 0.0;
    sim.send(PlayPause, Edge::Press);
    let end = sim.frame_at(seconds);
    while sim.frames() < end {
        if sim.secs() >= next {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let a = palette[rng as usize % palette.len()];
            sim.send(a, Edge::Press);
            sim.send(a, Edge::Release);
            if !sim.snapshot().playing || a == Cue {
                sim.send(PlayPause, Edge::Press);
            }
            next += 0.25;
        }
        sim.step();
    }
}

fn write_wav(path: &Path, stereo: &[f32], sample_rate: u32) -> Result<()> {
    let spec =
        hound::WavSpec { channels: 2, sample_rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &s in stereo {
        w.write_sample(to_i16(s))?;
    }
    w.finalize()?;
    Ok(())
}

pub fn write_recording(path: &Path, stereo: &[f32], sample_rate: u32) -> Result<()> {
    write_wav(path, stereo, sample_rate)
}

/// A sample as 16-bit PCM, clipping anything outside ±1.
#[expect(clippy::cast_possible_truncation, reason = "clamped to ±1 first, so the product always fits an i16")]
fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Nanoseconds (as the histograms record them) to microseconds.
fn us(ns: u64) -> f64 {
    Duration::from_nanos(ns).as_secs_f64() * 1e6
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn fmt_ms(v: f64) -> String {
    if v < 1.0 { format!("{:.1} µs", v * 1000.0) } else { format!("{v:.1} ms") }
}
