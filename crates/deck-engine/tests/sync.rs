//! SYNC and MASTER against a simulated Link timeline, driven block by block
//! the way the real Link driver does it.

use std::sync::Arc;

use anyhow::{Result, ensure};
use deck_engine::analysis::{self, synth::SynthSpec};
use deck_engine::session::{self, LocalSession, Session};
use deck_engine::sync::{SyncMode, phase_error};
use deck_engine::track::{GridSource, HOT_CUES, Waveform};
use deck_engine::{Action, BeatGrid, Edge, Engine, EngineConfig, EngineHandle, Snapshot, Track};

const SR: u32 = 48_000;
const BLOCK: usize = 128;
const BLOCK_SECS: f64 = 128.0 / 48_000.0;

/// A local session at `bpm`, at `beat` now: what the app uses without Link,
/// and driven by the same code as Link.
fn session(bpm: f64, beat: f64) -> LocalSession {
    let mut s = LocalSession::new(bpm, SR);
    s.force_beat(beat);
    s
}

trait Advance {
    fn tick(&mut self);
}

impl Advance for LocalSession {
    /// One block's worth of time passes.
    fn tick(&mut self) {
        self.advance(BLOCK as u64);
    }
}

struct Deck {
    engine: Engine,
    handle: EngineHandle,
    out: Vec<f32>,
    grid: BeatGrid,
}

impl Deck {
    fn new(bpm: f64) -> Result<Self> {
        Self::with_block(bpm, 512)
    }

    fn with_block(bpm: f64, max_block: usize) -> Result<Self> {
        let spec = SynthSpec { bpm, bars: 64, intro_bars: 0, breakdown: None, lead_in: 0.0, ..Default::default() };
        let grid = BeatGrid::new(bpm, 0.0, SR);
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
        let (engine, mut handle, _) = Engine::new(EngineConfig { sample_rate: SR, max_block, ..Default::default() });
        ensure!(handle.load(Arc::new(track)).is_ok(), "load refused");
        let mut d = Self { engine, handle, out: vec![0.0; BLOCK * 2], grid };
        d.press(Action::PlayPause, None);
        Ok(d)
    }

    /// One block, following / leading `link` like the audio callback does.
    fn step(&mut self, link: Option<&mut LocalSession>) -> Snapshot {
        if let Some(link) = link {
            session::drive(std::slice::from_mut(&mut self.engine), link, Some(1));
        }
        self.engine.process(&mut self.out);
        self.handle.snapshot()
    }

    fn press(&mut self, a: Action, link: Option<&mut LocalSession>) -> Snapshot {
        self.handle.send(a, Edge::Press);
        self.handle.send(a, Edge::Release);
        self.step(link)
    }

    fn beat(&self, s: &Snapshot) -> f64 {
        self.grid.beat_at(s.pos)
    }
}

/// Run `secs` of audio with `deck` on `link`; returns the last snapshot.
fn run(deck: &mut Deck, link: &mut LocalSession, secs: f64) -> Snapshot {
    let mut s = deck.step(Some(link));
    link.tick();
    for _ in 1..blocks(secs) {
        s = deck.step(Some(link));
        link.tick();
    }
    s
}

#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a few seconds of blocks")]
fn blocks(secs: f64) -> usize {
    (secs / BLOCK_SECS).ceil() as usize
}

#[test]
fn a_follower_far_out_snaps_in_then_locks() -> Result<()> {
    let mut deck = Deck::new(134.0)?;
    let mut link = session(136.0, 1.37);
    deck.press(Action::Sync, Some(&mut link));
    link.tick();
    let s = run(&mut deck, &mut link, 2.0);
    assert_eq!(s.sync, SyncMode::Follow);
    let err = phase_error(deck.beat(&s), link.beat());
    // Allow one block of skew: the snapshot is after the block, link.beat() is too.
    assert!(err.abs() < 0.002, "phase error {err:.4} beats");
    assert!((s.bpm() - 136.0).abs() < 0.05, "tempo {:.3}", s.bpm());
    assert!((136.0_f64 / 134.0 - 1.0).mul_add(-100.0, s.tempo_pct).abs() < 1e-9, "readout shows the synced tempo");
    Ok(())
}

#[test]
fn a_follower_close_by_bends_in_without_jumping() -> Result<()> {
    let mut deck = Deck::new(134.0)?;
    let mut link = session(134.0, -0.06);
    deck.press(Action::Sync, Some(&mut link));
    link.tick();
    let mut last = deck.step(Some(&mut link));
    link.tick();
    for _ in 0..blocks(3.0) {
        let s = deck.step(Some(&mut link));
        link.tick();
        let moved = s.pos - last.pos;
        let max = 1.03 * 128.0;
        assert!(moved > 0.0 && moved < max, "moved {moved:.1} frames in one block: a jump");
        last = s;
    }
    let err = phase_error(deck.beat(&last), link.beat());
    assert!(err.abs() < 0.002, "phase error {err:.4} beats after bending");
    Ok(())
}

#[test]
fn a_master_sets_the_timeline_and_a_follower_lands_on_its_downbeats() -> Result<()> {
    let mut master = Deck::new(134.0)?;
    let mut follower = Deck::new(130.0)?;
    let mut link = session(120.0, 0.0);
    for _ in 0..4 {
        master.press(Action::TempoUpCoarse, None);
    }
    master.press(Action::Master, Some(&mut link));
    follower.press(Action::Sync, Some(&mut link));
    let (mut m, mut f) = (master.step(Some(&mut link)), follower.step(Some(&mut link)));
    for _ in 0..blocks(3.0) {
        link.tick();
        m = master.step(Some(&mut link));
        f = follower.step(Some(&mut link));
    }
    assert!(
        134.0_f64.mul_add(-1.04, link.tempo()).abs() < 1e-6,
        "the master's tempo is the timeline's: {}",
        link.tempo()
    );
    assert!((f.bpm() - m.bpm()).abs() < 0.05, "follower {:.3} vs master {:.3}", f.bpm(), m.bpm());
    let err = phase_error(follower.beat(&f), master.beat(&m));
    assert!(err.abs() < 0.003, "bars apart by {err:.4} beats");
    Ok(())
}

#[test]
fn inside_a_one_beat_loop_a_follower_stays_put() -> Result<()> {
    let mut deck = Deck::new(134.0)?;
    let mut link = session(134.0, 0.0);
    deck.press(Action::Sync, Some(&mut link));
    link.tick();
    run(&mut deck, &mut link, 1.0);
    let s = deck.press(Action::BeatLoop1, Some(&mut link));
    link.tick();
    let (start, end) = (s.loop_start, s.loop_end);
    for _ in 0..blocks(3.0) {
        let s = deck.step(Some(&mut link));
        link.tick();
        assert!(s.pos >= start - 1.0 && s.pos <= end + 1.0, "left the loop: {:.0} not in {start:.0}–{end:.0}", s.pos);
        assert!(s.sync_error.abs() < 0.01, "fighting the loop: error {:.3} beats", s.sync_error);
    }
    Ok(())
}

#[test]
fn a_beat_jump_on_a_synced_deck_moves_exactly_that_far() -> Result<()> {
    let mut deck = Deck::new(134.0)?;
    let mut link = session(134.0, 0.0);
    deck.press(Action::Sync, Some(&mut link));
    link.tick();
    deck.press(Action::LoopHalve, Some(&mut link)); // 2-beat jumps
    link.tick();
    let before = run(&mut deck, &mut link, 1.0);
    let link_before = link.beat();
    deck.press(Action::BeatJumpForward, Some(&mut link));
    link.tick();
    let after = run(&mut deck, &mut link, 1.0);
    let moved = (deck.beat(&after) - deck.beat(&before)) - (link.beat() - link_before);
    assert!((moved - 2.0).abs() < 0.01, "moved {moved:.3} beats against the timeline");
    Ok(())
}

#[test]
fn an_absurd_session_tempo_never_breaks_the_audio() -> Result<()> {
    for (bpm, max_block) in [(999.0, 1024), (1.0, 1024), (175.0, 4096)] {
        let mut deck = Deck::with_block(88.0, max_block)?;
        deck.out = vec![0.0; max_block * 2];
        let mut link = session(bpm, 0.0);
        deck.press(Action::Sync, Some(&mut link));
        let s = run(&mut deck, &mut link, 0.5);
        assert!(s.rate.is_finite() && (0.25..=2.0).contains(&s.rate), "{bpm} BPM: rate {}", s.rate);
    }
    Ok(())
}

#[test]
fn nudging_the_master_moves_its_phase_not_the_session_tempo() -> Result<()> {
    let mut master = Deck::new(134.0)?;
    let mut link = session(120.0, 0.0);
    master.press(Action::Master, Some(&mut link));
    run(&mut master, &mut link, 0.5);
    master.handle.send(Action::NudgeForward, Edge::Press);
    let s = run(&mut master, &mut link, 0.5);
    assert!(s.bend > 0.0, "nudging");
    assert!((link.tempo() - 134.0).abs() < 1e-9, "session tempo {}", link.tempo());
    Ok(())
}

#[test]
fn a_stopped_master_sets_tempo_but_leaves_the_phase_alone() -> Result<()> {
    let mut master = Deck::new(134.0)?;
    let mut link = session(120.0, 0.0);
    master.press(Action::PlayPause, None); // stop
    master.press(Action::Master, Some(&mut link));
    let mut last = link.beat();
    for _ in 0..blocks(1.0) {
        link.tick();
        master.step(Some(&mut link));
        assert!(link.beat() > last, "the session's beat went backwards");
        last = link.beat();
    }
    assert!((link.tempo() - 134.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn after_sync_the_tempo_buttons_move_on_from_the_synced_tempo() -> Result<()> {
    let mut deck = Deck::new(120.0)?;
    let mut link = session(140.0, 0.0);
    deck.press(Action::Sync, Some(&mut link));
    let synced = run(&mut deck, &mut link, 0.5).tempo_pct;
    assert!(synced > 16.0, "synced {synced}");
    deck.press(Action::Sync, None);
    let s = deck.press(Action::TempoDown, None);
    assert!((s.tempo_pct - synced).abs() < 0.05, "jumped from {synced:.2}% to {:.2}%", s.tempo_pct);
    Ok(())
}

#[test]
fn taking_master_hands_the_old_master_back_to_sync() -> Result<()> {
    let (mut a, mut b) = (Deck::new(134.0)?, Deck::new(130.0)?);
    let mut link = session(120.0, 0.0);
    a.handle.send(Action::Master, Edge::Press);
    let mut engines = [a.engine, b.engine];
    let mut step = |engines: &mut [Engine], link: &mut LocalSession| {
        session::drive(engines, link, None);
        for (e, out) in engines.iter_mut().zip([&mut a.out, &mut b.out]) {
            e.process(out);
        }
        link.tick();
    };
    for _ in 0..blocks(0.5) {
        step(&mut engines, &mut link);
    }
    assert!((link.tempo() - 134.0).abs() < 1e-9, "A leads");
    b.handle.send(Action::Master, Edge::Press);
    for _ in 0..blocks(0.5) {
        step(&mut engines, &mut link);
    }
    let (sa, sb) = (a.handle.snapshot(), b.handle.snapshot());
    assert_eq!((sa.sync, sb.sync), (SyncMode::Follow, SyncMode::Lead));
    assert!((link.tempo() - 130.0).abs() < 1e-9, "B leads now: {}", link.tempo());
    let synced = sa.grid.bpm * (1.0 + sa.tempo_pct / 100.0);
    assert!((synced - 130.0).abs() < 1e-6, "A follows at {synced:.2}");
    Ok(())
}

#[test]
fn a_deck_reports_its_phase_against_the_session() -> Result<()> {
    // Not synced, a quarter beat ahead of the session.
    let mut deck = Deck::new(128.0)?;
    let mut link = session(128.0, -0.25);
    let s = run(&mut deck, &mut link, 1.0);
    let p = s.link_phase.unwrap_or(f64::NAN);
    assert!((p - 0.25).abs() < 0.01, "phase {p:.3}");
    // Synced, it reads zero.
    deck.press(Action::Sync, Some(&mut link));
    link.tick();
    let s = run(&mut deck, &mut link, 2.0);
    let p = s.link_phase.unwrap_or(f64::NAN);
    assert!(p.abs() < 0.002, "synced phase {p:.4}");
    Ok(())
}
