//! Transport behaviour, checked block by block like the audio callback runs it.
//! The allocator aborts if `process` allocates.

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use deck_engine::analysis::{self, synth::SynthSpec};
use deck_engine::track::{GridSource, HOT_CUES, Waveform};
use deck_engine::{Action, BeatGrid, Edge, Engine, EngineConfig, EngineHandle, Snapshot, Track};

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

const SR: u32 = 48_000;
const BLOCK: usize = 128;

/// Frame and block counts as `f64`.
#[expect(clippy::cast_precision_loss, reason = "the counts in these tests are tiny, so exact in f64")]
const fn as_f64(n: usize) -> f64 {
    n as f64
}

/// Equal to well under a sample: these positions are meant to be exact.
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

struct Rig {
    engine: Engine,
    handle: EngineHandle,
    out: Vec<f32>,
    grid: BeatGrid,
}

impl Rig {
    fn new(keylock: bool) -> Self {
        let spec = SynthSpec { bars: 24, intro_bars: 0, breakdown: None, lead_in: 0.0, ..Default::default() };
        // Transport is tested against the true grid, so skip analysis entirely.
        let grid = BeatGrid::new(spec.bpm, 0.0, SR);
        let track = Track {
            id: 1,
            title: "synth".into(),
            sample_rate: SR,
            samples: analysis::synth::render(&spec),
            grid,
            grid_source: GridSource::Detected,
            load_cue: None,
            hot_cues: [None; HOT_CUES],
            waveform: Waveform { hop: 320, columns: Vec::new() },
        };
        let (engine, mut handle, _) =
            Engine::new(EngineConfig { sample_rate: SR, max_block: 512, extra_inputs: 0, ..Default::default() });
        assert!(handle.load(Arc::new(track)).is_ok(), "load refused");
        let mut rig = Self { engine, handle, out: vec![0.0; BLOCK * 2], grid };
        if !keylock {
            rig.press(Action::KeyLock);
        }
        rig.run(1);
        rig
    }

    fn run(&mut self, blocks: usize) -> Snapshot {
        for _ in 0..blocks {
            let (engine, out) = (&mut self.engine, &mut self.out);
            assert_no_alloc(|| engine.process(out));
        }
        self.handle.snapshot()
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a few seconds of whole blocks: small, positive and already rounded up"
    )]
    fn run_secs(&mut self, secs: f64) -> Snapshot {
        self.run((secs * f64::from(SR) / as_f64(BLOCK)).ceil() as usize)
    }

    fn press(&mut self, a: Action) -> Snapshot {
        self.handle.send(a, Edge::Press);
        self.handle.send(a, Edge::Release);
        self.run(1)
    }

    fn hold(&mut self, a: Action) -> Snapshot {
        self.handle.send(a, Edge::Press);
        self.run(1)
    }

    fn release(&mut self, a: Action) -> Snapshot {
        self.handle.send(a, Edge::Release);
        self.run(1)
    }

    fn beat(&self, s: &Snapshot) -> f64 {
        self.grid.beat_at(s.pos)
    }
}

#[test]
fn play_advances_in_real_time() {
    for keylock in [false, true] {
        let mut rig = Rig::new(keylock);
        rig.press(Action::PlayPause);
        let s = rig.run(99);
        // 100 blocks played (the press block included).
        assert!((s.pos - as_f64(100 * BLOCK)).abs() < 1.0, "keylock={keylock} pos={}", s.pos);
    }
}

#[test]
fn cue_sets_on_beat_previews_and_returns() {
    let mut rig = Rig::new(true);
    rig.press(Action::BeatJumpForward); // 4 beats in, paused
    let s = rig.press(Action::NudgeForward); // 1/8 beat off the grid
    assert!((rig.beat(&s) - 4.125).abs() < 1e-6);

    let s = rig.hold(Action::Cue);
    assert!(s.playing && s.previewing);
    assert!((s.cue - rig.grid.frame_of(4.0)).abs() < 1e-6, "quantized to the nearest beat");
    let s = rig.run_secs(1.0);
    assert!(rig.beat(&s) > 5.5);

    let s = rig.release(Action::Cue);
    assert!(!s.playing);
    assert!(same(s.pos, s.cue), "{} vs {}", s.pos, s.cue);
}

#[test]
fn cue_while_playing_returns_and_stops() {
    let mut rig = Rig::new(true);
    rig.press(Action::PlayPause);
    rig.run_secs(2.0);
    let s = rig.press(Action::Cue);
    assert!(!s.playing);
    assert!(same(s.pos, 0.0), "{}", s.pos);
}

#[test]
fn play_during_preview_keeps_playing() {
    let mut rig = Rig::new(true);
    rig.hold(Action::Cue);
    rig.press(Action::PlayPause);
    let s = rig.release(Action::Cue);
    assert!(s.playing && !s.previewing);
}

#[test]
fn loops_are_sample_exact_over_many_passes() {
    for keylock in [false, true] {
        let mut rig = Rig::new(keylock);
        rig.press(Action::PlayPause);
        rig.run_secs(3.3); // somewhere inside beat 7
        let s = rig.press(Action::BeatLoop1);
        assert!(s.loop_active);
        assert!(same(s.loop_start, rig.grid.frame_of(rig.grid.floor_beat(s.pos))), "{}", s.loop_start);
        let len = s.loop_end - s.loop_start;

        let before = rig.run(1);
        let blocks = 2000; // ~5.3 s, ~12 passes
        let after = rig.run(blocks);
        let advanced = as_f64(blocks * BLOCK);
        let expected = s.loop_start + (before.pos - s.loop_start + advanced).rem_euclid(len);
        assert!((after.pos - expected).abs() < 1e-6, "keylock={keylock}: {} vs {}", after.pos, expected);
        assert!(after.pos >= s.loop_start && after.pos < s.loop_end);
    }
}

#[test]
fn halving_a_loop_wraps_the_playhead_into_it() {
    let mut rig = Rig::new(true);
    rig.press(Action::PlayPause);
    rig.press(Action::BeatLoop4);
    rig.run_secs(1.5); // ~3.3 beats into the loop
    let s = rig.press(Action::LoopHalve);
    assert!(same(s.loop_beats, 2.0), "{}", s.loop_beats);
    assert!(s.pos < s.loop_end, "{} < {}", s.pos, s.loop_end);
}

#[test]
fn beat_jump_keeps_phase_and_moves_the_loop() {
    let mut rig = Rig::new(true);
    rig.press(Action::PlayPause);
    rig.run_secs(1.0);
    let before = rig.run(0);
    let s = rig.press(Action::BeatJumpForward);
    let jumped = rig.beat(&s) - rig.beat(&before);
    // 4 beats plus the one block that elapsed.
    assert!((jumped - 4.0 - as_f64(BLOCK) / rig.grid.period()).abs() < 1e-3, "{jumped}");

    let s = rig.press(Action::BeatLoop2);
    let start = s.loop_start;
    let s = rig.press(Action::BeatJumpBack);
    let two_beats = 2.0 * rig.grid.period();
    assert!((start - s.loop_start - two_beats).abs() < 1e-6);
}

#[test]
fn tempo_steps_clamp_to_range() {
    let mut rig = Rig::new(true);
    for _ in 0..20 {
        rig.press(Action::TempoUpCoarse);
    }
    let s = rig.run(1);
    assert!(same(s.tempo_pct, 8.0), "{}", s.tempo_pct);
    let s = rig.press(Action::TempoRange);
    assert!(s.wide_range);
    rig.press(Action::TempoUpCoarse);
    let s = rig.press(Action::TempoDown);
    assert!((s.tempo_pct - 8.95).abs() < 1e-9, "{}", s.tempo_pct);
}

#[test]
fn nudge_bends_only_while_held_and_builds() {
    let mut rig = Rig::new(true);
    rig.press(Action::PlayPause);
    let s = rig.hold(Action::NudgeForward);
    assert!(s.rate > 1.0099 && s.rate < 1.011, "starts gentle: {}", s.rate);
    let s = rig.run_secs(1.0);
    assert!((s.rate - 1.04).abs() < 1e-3, "builds to 4%: {}", s.rate);
    let s = rig.release(Action::NudgeForward);
    assert!(same(s.rate, 1.0), "{}", s.rate);
}

/// Two decks, the second 20 ms behind: holding nudge on it until the beats
/// line up has to close the gap, the way you'd do it by ear.
#[test]
fn holding_nudge_closes_a_phase_gap_between_two_decks() {
    let mut a = Rig::new(true);
    let mut b = Rig::new(true);
    a.press(Action::PlayPause);
    b.press(Action::PlayPause);
    // Put B 20 ms behind A: 10 phase steps of 2 ms back.
    for _ in 0..10 {
        b.press(Action::PhaseBack);
    }
    a.run(10);
    let gap_ms = |a: &mut Rig, b: &mut Rig| {
        let (sa, sb) = (a.run(0), b.run(0));
        (a.beat(&sa) - b.beat(&sb)) * a.grid.period() / f64::from(SR) * 1000.0
    };
    let start = gap_ms(&mut a, &mut b);
    assert!((start - 20.0).abs() < 3.0, "start gap {start:.1} ms");

    b.handle.send(Action::NudgeForward, Edge::Press);
    let mut held = 0;
    while gap_ms(&mut a, &mut b) > 0.5 && held < 1000 {
        a.run(1);
        b.run(1);
        held += 1;
    }
    b.handle.send(Action::NudgeForward, Edge::Release);
    a.run(1);
    b.run(1);
    let end = gap_ms(&mut a, &mut b);
    let secs = f64::from(held) * as_f64(BLOCK) / f64::from(SR);
    assert!(end.abs() < 1.5, "gap after nudging {end:.2} ms");
    assert!(secs < 1.5, "took {secs:.2} s of holding");
}

/// With key lock on, what you hear must line up with the displayed position.
/// Compare onset envelopes of the key-locked output against vinyl mode, which
/// reads the source directly and has no latency.
#[test]
fn keylock_output_is_aligned_with_position() {
    for tempo_steps in [0, 4] {
        let mut envs = Vec::new();
        for keylock in [false, true] {
            let mut rig = Rig::new(keylock);
            for _ in 0..tempo_steps {
                rig.press(Action::TempoUpCoarse);
            }
            rig.press(Action::BeatJumpForward);
            rig.press(Action::PlayPause);
            let mut env = Vec::new();
            for _ in 0..1500 {
                rig.run(1);
                env.push(rig.out.as_chunks::<2>().0.iter().map(|f| f[0].abs()).fold(0.0f32, f32::max));
            }
            envs.push(env);
        }
        let lag = best_lag(&envs[0], &envs[1], 40);
        let ms = f64::from(lag) * as_f64(BLOCK) / f64::from(SR) * 1000.0;
        assert!(ms.abs() <= 2.7, "tempo +{tempo_steps}%: key lock is {ms:.1} ms off");
    }
}

/// Changing tempo while playing with key lock on: the stretcher still holds
/// audio fed at the old rate, so the position must follow the audio through
/// the change, not run ahead of it. Kick onsets against the beats the
/// position says are playing must stay put to well under a millisecond.
#[test]
fn keylock_position_follows_the_audio_through_a_tempo_change() {
    let mut rig = Rig::new(true);
    rig.press(Action::PlayPause);
    let mut out = Vec::new();
    let mut pos = Vec::new();
    for block in 0..2000 {
        if block == 700 {
            for _ in 0..8 {
                rig.handle.send(Action::TempoUpCoarse, Edge::Press);
            }
        }
        pos.push(rig.handle.snapshot().pos);
        rig.run(1);
        out.extend(rig.out.as_chunks::<2>().0.iter().map(|f| f[0]));
    }
    let offsets = |from: usize, to: usize| -> f64 {
        let mut d: Vec<f64> = (0..200)
            .filter_map(|k| {
                let target = rig.grid.frame_of(f64::from(k));
                // The output sample where the position reached this beat.
                let b = pos.windows(2).position(|w| w[0] <= target && target < w[1])?;
                let at = ((target - pos[b]) / (pos[b + 1] - pos[b])).mul_add(as_f64(BLOCK), as_f64(b * BLOCK));
                (as_f64(from * BLOCK)..as_f64(to * BLOCK)).contains(&at).then(|| onset_near(&out, at) - at)
            })
            .collect();
        d.sort_by(f64::total_cmp);
        d.get(d.len() / 2).copied().unwrap_or(f64::NAN)
    };
    let before = offsets(200, 700);
    let after = offsets(900, 2000);
    let drift_ms = (after - before) / f64::from(SR) * 1000.0;
    assert!(drift_ms.abs() < 1.0, "kicks moved {drift_ms:.2} ms against the position at +8%");
}

/// First sample within ±20 ms of `at` where the signal reaches half the
/// window's peak: the kick's attack.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "sample indices in a short test")]
fn onset_near(x: &[f32], at: f64) -> f64 {
    let w = f64::from(SR) * 0.02;
    let (lo, hi) = (((at - w).max(0.0)) as usize, ((at + w) as usize).min(x.len()));
    let win = &x[lo..hi];
    let peak = win.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    as_f64(lo + win.iter().position(|v| v.abs() >= peak * 0.5).unwrap_or_default())
}

/// Lag (in blocks) that best aligns `b` to `a`.
fn best_lag(a: &[f32], b: &[f32], max: i32) -> i32 {
    (-max..=max).max_by(|&x, &y| corr(a, b, x).total_cmp(&corr(a, b, y))).unwrap_or_default()
}

fn corr(a: &[f32], b: &[f32], lag: i32) -> f32 {
    let lag = isize::try_from(lag).unwrap_or_default();
    a.iter().enumerate().filter_map(|(i, &x)| Some(x * *b.get(i.checked_add_signed(lag)?)?)).sum()
}

#[test]
fn beat_jump_back_inside_a_loop_moves_exactly_one_loop() {
    let mut rig = Rig::new(true);
    rig.press(Action::PlayPause);
    rig.run_secs(3.0);
    let s = rig.press(Action::BeatLoop2);
    rig.run_secs(0.6); // ~1.3 beats into the loop
    let before = rig.run(0);
    let s2 = rig.press(Action::BeatJumpBack);
    let moved = rig.beat(&s2) - rig.beat(&before) - as_f64(BLOCK) / rig.grid.period();
    assert!((moved + 2.0).abs() < 1e-6, "moved {moved} beats");
    let two_beats = 2.0 * rig.grid.period();
    assert!((s.loop_start - s2.loop_start - two_beats).abs() < 1e-6);
    assert!(s2.pos >= s2.loop_start && s2.pos < s2.loop_end);
}

#[test]
fn an_empty_hot_cue_stores_the_position_on_the_beat() {
    let mut rig = Rig::new(true);
    rig.press(Action::BeatJumpForward); // beat 4
    let s = rig.press(Action::NudgeForward); // beat 4.125, paused
    assert!((rig.beat(&s) - 4.125).abs() < 1e-6);
    let s = rig.press(Action::HotCue2);
    let hc = s.hot_cues[1].expect("stored");
    assert!((hc.start - rig.grid.frame_of(4.0)).abs() < 1e-6, "snapped to beat 4");
    assert!(!s.playing, "storing doesn't start playback");
}

#[test]
fn a_hot_cue_jump_while_playing_keeps_the_beat_phase() {
    let mut rig = Rig::new(true);
    rig.press(Action::BeatJumpForward);
    rig.press(Action::BeatJumpForward); // beat 8
    rig.press(Action::HotCue1); // store A at beat 8
    rig.press(Action::PlayPause);
    rig.run_secs(5.3);
    let before = rig.run(0);
    let s = rig.press(Action::HotCue1);
    assert!(s.playing);
    // Same position within the beat, one block later, now near beat 8.
    let phase = |b: f64| b - b.round();
    let expected = phase(rig.beat(&before)) + as_f64(BLOCK) / rig.grid.period();
    assert!((phase(rig.beat(&s)) - expected).abs() < 1e-3, "{} vs {}", rig.beat(&s), expected);
    assert!((rig.beat(&s) - 8.0).abs() < 0.6);
}

#[test]
fn a_hot_cue_held_while_stopped_previews_and_returns() {
    let mut rig = Rig::new(true);
    rig.press(Action::BeatJumpForward);
    rig.press(Action::HotCue3); // store C at beat 4
    rig.press(Action::BeatJumpForward); // move away to beat 8
    let s = rig.hold(Action::HotCue3);
    assert!(s.playing && s.previewing);
    rig.run_secs(0.5);
    let s = rig.release(Action::HotCue3);
    assert!(!s.playing);
    assert!((rig.beat(&s) - 4.0).abs() < 1e-6, "back at C: beat {}", rig.beat(&s));
    // Releasing a different button doesn't end someone else's preview.
    rig.hold(Action::HotCue3);
    let s = rig.release(Action::Cue);
    assert!(s.previewing);
}

#[test]
fn needle_search_moves_a_stopped_deck_and_leaves_a_playing_one() {
    let mut rig = Rig::new(true);
    let end = rig.handle.snapshot().track_frames;
    rig.handle.needle(0.5);
    let s = rig.run(1);
    assert!(same(s.pos, as_f64(usize::try_from(end).unwrap_or_default()) * 0.5), "at {}", s.pos);
    rig.press(Action::PlayPause);
    let before = rig.run(1).pos;
    rig.handle.needle(0.0);
    let s = rig.run(1);
    assert!(s.pos > before, "a playing deck ignores the needle");
}
