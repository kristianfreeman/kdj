//! Listening test for key lock: the same passage of a real track rendered
//! several ways, to compare by ear.
//!
//!   deck-sim ab TRACK [--tempo PCT] [--at SECS] [--secs N] [--out DIR]
//!
//! Writes, for a tempo change of PCT percent (default +6):
//! - `original.wav`: the passage as it is;
//! - `vinyl.wav`: sped up, pitch and all (key lock off);
//! - `keylock-cheaper.wav`: key lock as the deck plays it;
//! - `keylock-default.wav`: Signalsmith's heavier preset, about 7× the
//!   worst-case CPU, for comparison.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use deck_engine::analysis;
use deck_engine::stretch::{Quality, Stretcher};

use crate::Opts;

/// Output frames per stretcher call, about a callback's worth.
const BLOCK: usize = 512;

pub fn run(opts: &Opts) -> Result<bool> {
    let Some(track) = opts.positional.first() else {
        bail!("usage: deck-sim ab TRACK [--tempo PCT] [--at SECS] [--secs N] [--out DIR]")
    };
    let tempo: f64 = opts.flag("tempo").unwrap_or("6").parse()?;
    let at: f64 = opts.flag("at").unwrap_or("60").parse()?;
    let secs: f64 = opts.flag("secs").unwrap_or("30").parse()?;
    let out = PathBuf::from(opts.flag("out").unwrap_or("target/ab"));
    let rate = 1.0 + tempo / 100.0;
    if !(0.5..=1.5).contains(&rate) {
        bail!("--tempo {tempo}: keep it within ±50%");
    }
    let decoded = analysis::decode::decode(Path::new(track), None)?;
    let sr = decoded.sample_rate;
    let src = decoded.samples;
    let frames = src.len() / 2;
    let start = frames_at(at, sr).min(frames);
    let out_frames = frames_at(secs, sr);
    std::fs::create_dir_all(&out)?;

    let original = src.get(start * 2..(start + out_frames).min(frames) * 2).unwrap_or_default();
    write(&out.join("original.wav"), original, sr)?;
    write(&out.join("vinyl.wav"), &vinyl(&src, start, out_frames, rate), sr)?;
    for (name, quality) in [("keylock-cheaper.wav", Quality::Cheaper), ("keylock-default.wav", Quality::Default)] {
        write(&out.join(name), &keylock(&src, start, out_frames, rate, sr, quality), sr)?;
    }
    println!("wrote {} at {tempo:+}% from {at} s: original, vinyl, keylock-cheaper, keylock-default", out.display());
    Ok(true)
}

/// Read straight through at `rate`, pitch and all, interpolating linearly.
fn vinyl(src: &[f32], start: usize, out_frames: usize, rate: f64) -> Vec<f32> {
    let frame = |i: usize| src.get(i * 2..i * 2 + 2).map_or([0.0; 2], |f| [f[0], f[1]]);
    (0..out_frames)
        .flat_map(|n| {
            let pos = as_f64(n).mul_add(rate, as_f64(start));
            let i = whole(pos);
            #[expect(clippy::cast_possible_truncation, reason = "a fraction in 0..1")]
            let t = (pos - pos.floor()) as f32;
            let (a, b) = (frame(i), frame(i + 1));
            [(b[0] - a[0]).mul_add(t, a[0]), (b[1] - a[1]).mul_add(t, a[1])]
        })
        .collect()
}

/// Key lock as the engine does it: prime with history, then feed whole
/// source frames per block and carry the fraction.
fn keylock(src: &[f32], start: usize, out_frames: usize, rate: f64, sr: u32, quality: Quality) -> Vec<f32> {
    let mut st = Stretcher::with_quality(sr, quality);
    let slice = |from: usize, n: usize| -> Vec<f32> {
        (from..from + n).flat_map(|i| src.get(i * 2..i * 2 + 2).map_or([0.0; 2], |f| [f[0], f[1]])).collect()
    };
    // History ending where the output should start, plus the stretcher's lead.
    let lead = whole(st.lead(rate));
    let pre = st.preroll();
    let mut feed = (start + lead).saturating_sub(pre);
    st.seek(&slice(feed, pre), rate);
    feed += pre;
    let (mut out, mut carry) = (Vec::with_capacity(out_frames * 2), 0.0);
    let mut block = vec![0.0f32; BLOCK * 2];
    while out.len() < out_frames * 2 {
        let exact = as_f64(BLOCK).mul_add(rate, carry);
        let n_in = whole(exact);
        carry = exact - as_f64(n_in);
        st.process(&slice(feed, n_in), &mut block);
        feed += n_in;
        out.extend_from_slice(&block);
    }
    out.truncate(out_frames * 2);
    out
}

fn write(path: &Path, stereo: &[f32], sr: u32) -> Result<()> {
    crate::perf::write_recording(path, stereo, sr)
}

fn frames_at(secs: f64, sr: u32) -> usize {
    whole((secs * f64::from(sr)).max(0.0))
}

#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "non-negative frame positions in a track")]
const fn whole(x: f64) -> usize {
    x as usize
}

#[expect(clippy::cast_precision_loss, reason = "frame counts in a track, far below 2^52")]
const fn as_f64(n: usize) -> f64 {
    n as f64
}
