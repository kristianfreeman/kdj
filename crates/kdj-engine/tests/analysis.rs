//! Beat-grid detection against synthetic tracks with known ground truth.

use anyhow::Result;
use kdj_engine::analysis::{self, synth::SynthSpec};

const SR: u32 = 48_000;

/// (bpm error, anchor error in ms) for one synthetic track.
fn detect(spec: &SynthSpec) -> (f64, f64) {
    let (track, _) = analysis::build("synth".into(), analysis::synth::render(spec), SR);
    let g = track.grid;
    let truth = spec.first_kick() * f64::from(SR);
    // Compare anchor to the true first kick, allowing for whole-bar offsets.
    let bar = 4.0 * g.period();
    let off = (g.anchor - truth + bar / 2.0).rem_euclid(bar) - bar / 2.0;
    (g.bpm - spec.bpm, off / f64::from(SR) * 1000.0)
}

fn check_tempos(tempos: &[f64], bars: u32) {
    let mut worst_ms: f64 = 0.0;
    for (seed, &bpm) in (1u32..).zip(tempos) {
        // Vary the lead-in so the anchor isn't always at the same offset.
        let lead_in = 0.173f64.mul_add(f64::from(seed - 1), 0.05);
        let spec = SynthSpec { bpm, bars, lead_in, seed, ..Default::default() };
        let (bpm_err, ms) = detect(&spec);
        println!("{bpm:>7.2} bpm: tempo error {bpm_err:+.4}, anchor error {ms:+.2} ms");
        assert!(bpm_err.abs() < 0.01, "{bpm}: tempo off by {bpm_err}");
        worst_ms = worst_ms.max(ms.abs());
    }
    assert!(worst_ms < 3.0, "anchor off by up to {worst_ms:.2} ms");
}

/// Fast: a few tempos on short tracks, including 135, which offbeat hats
/// once made read as 90.
#[test]
fn grid_matches_ground_truth() {
    check_tempos(&[128.0, 135.0, 142.0], 32);
}

/// Full sweep across the techno range: `cargo test -- --ignored`.
#[test]
#[ignore = "full suite"]
fn grid_matches_ground_truth_across_the_techno_range() {
    let tempos = [
        124.0, 126.0, 128.0, 130.0, 131.5, 132.0, 133.0, 134.0, 135.0, 136.0, 137.25, 138.0, 140.0, 142.0, 145.0, 150.0,
    ];
    check_tempos(&tempos, 64);
}

#[test]
fn first_kick_after_an_intro_is_bar_one() {
    let spec = SynthSpec { intro_bars: 8, bars: 32, breakdown: None, lead_in: 0.4, ..Default::default() };
    let (track, _) = analysis::build("synth".into(), analysis::synth::render(&spec), SR);
    let g = track.grid;
    // The first kick lands on a downbeat, and bars count from the top of the track.
    let kick_beat = g.beat_at(spec.first_kick() * f64::from(SR));
    let off_ms = (kick_beat - kick_beat.round()) * g.period() / f64::from(SR) * 1000.0;
    assert!(off_ms.abs() < 3.0, "first kick {off_ms:.2} ms off the grid");
    assert_eq!(g.bar_beat(g.frame_of(kick_beat.round()) + 1.0), (9, 1), "first kick after 8 intro bars is bar 9");
    assert!(g.anchor / f64::from(SR) <= spec.lead_in + 0.003, "bar 1 is at the start of the music");
}

/// A 16-bit mono WAV of `frames` frames of silence.
fn silent_wav(path: &std::path::Path, frames: u32, rate: u32) -> std::io::Result<()> {
    let data = frames * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    b.resize(44 + data as usize, 0);
    std::fs::write(path, b)
}

#[test]
fn a_track_too_long_for_memory_is_refused_not_loaded() -> Result<()> {
    let path = std::env::temp_dir().join(format!("kdj-too-long-{}.wav", std::process::id()));
    silent_wav(&path, 96_000, 48_000)?;
    let refused = analysis::decode::decode(&path, Some(48_000));
    let fits = analysis::decode::decode(&path, Some(96_000));
    std::fs::remove_file(&path)?;
    let err = refused.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("too long"), "refused with {err:?}");
    assert_eq!(fits?.samples.len(), 96_000 * 2, "mono comes out as stereo");
    Ok(())
}

/// An MP3 of `frames` silent frames (MPEG-1, 44.1 kHz, 128 kbps) after an
/// Info frame from `encoder` declaring a delay of 576 (1105 with the decoder's).
fn silent_mp3(encoder: &[u8; 9], frames: usize) -> Vec<u8> {
    const FRAME: usize = 417;
    let header = [0xff, 0xfb, 0x90, 0x00];
    let mut info = vec![0; FRAME];
    info[..4].copy_from_slice(&header);
    // After the header and 32 bytes of side information; flags stay 0.
    info[36..40].copy_from_slice(b"Info");
    info[44..53].copy_from_slice(encoder);
    let trim = 576u32 << 12;
    info[65..68].copy_from_slice(&trim.to_be_bytes()[1..]);
    let mut audio = vec![0; FRAME];
    audio[..4].copy_from_slice(&header);
    let mut b = info;
    for _ in 0..frames {
        b.extend_from_slice(&audio);
    }
    b
}

#[test]
fn an_info_frame_without_a_delay_gets_no_lead_in() -> Result<()> {
    // Most of a real library: an Info frame from an encoder that declares no
    // delay, which Rekordbox's grids fit as decoded.
    let plain = analysis::decode::decode_bytes(silent_mp3(b"Fraunhof\0", 40), Some("mp3"), None)?;
    assert_eq!(plain.lead_in, 0);
    assert_eq!(plain.samples.len() / 2, 40 * 1152);
    Ok(())
}

/// Rekordbox's timeline for an MP3 with a LAME tag runs the encoder delay
/// (1105 frames) plus one MPEG frame (1152, the tag's) behind the audio as
/// Symphonia decodes it: measured on 12 such tracks, kicks came 47.0 ms ahead
/// of their grids at 48 kHz and about 50 ms at 44.1 kHz, every other track
/// was on its grid. Sample 20 000 of the fixture's source (a click in 1 s of
/// silence, `lame -q 0 -b 64 -m m`) must play at 20 000 + 2257.
#[test]
fn a_lame_mp3_sits_where_rekordbox_expects_it() -> Result<()> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/click-lame.mp3");
    let d = analysis::decode::decode(&path, None)?;
    assert_eq!((d.sample_rate, d.lead_in), (44_100, 1105 + 1152));
    let (loudest, _) = d
        .samples
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |best, (i, &[l, _])| if l.abs() > best.1 { (i, l.abs()) } else { best });
    assert!(loudest.abs_diff(20_000 + 2257) <= 2, "the click plays at frame {loudest}");
    Ok(())
}
