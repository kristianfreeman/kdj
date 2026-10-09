//! Beat-grid detection against synthetic tracks with known ground truth.

use deck_engine::analysis::{self, synth::SynthSpec};

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
