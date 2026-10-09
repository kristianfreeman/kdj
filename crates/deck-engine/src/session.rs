//! Driving decks from a shared session: Ableton Link's, or a local one when
//! there's no network (`--no-link`, or a build without Link).
//!
//! Each block, before rendering: the deck on MASTER sets the session's tempo,
//! and its phase when it takes over or drifts; then every deck gets the
//! session's tempo and beat for that block. Realtime-safe.

use crate::engine::Engine;
use crate::sync::{MASTER_DRIFT_BEATS, Timeline, phase_error};

/// A shared tempo and beat timeline, as of the moment the coming block is
/// heard.
pub trait Session {
    fn tempo(&self) -> f64;
    fn set_tempo(&mut self, bpm: f64);
    /// Beat number when the coming block is heard.
    fn beat(&self) -> f64;
    /// Make the coming block's first frame this beat, phase and all.
    fn force_beat(&mut self, beat: f64);
}

/// Let the deck on MASTER lead `session`, then hand every deck its timeline.
///
/// MASTER is one deck at a time: a deck taking it hands the others
/// back to SYNC. `peers`: Link peers, or None for a local session.
/// Returns whether the session changed (Link wants a commit).
pub fn drive(engines: &mut [Engine], session: &mut impl Session, peers: Option<u32>) -> bool {
    let taking = engines.iter_mut().position(|e| e.lead().is_some_and(|l| l.align));
    let mut changed = false;
    for (i, e) in engines.iter_mut().enumerate() {
        if taking.is_some_and(|t| t != i) {
            e.yield_master();
            continue;
        }
        let Some(l) = e.lead() else { continue };
        if (session.tempo() - l.bpm).abs() > 1e-4 {
            session.set_tempo(l.bpm);
            changed = true;
        }
        let Some(beat) = l.beat else { continue };
        if taking == Some(i) || phase_error(session.beat(), beat).abs() > MASTER_DRIFT_BEATS {
            session.force_beat(beat);
            changed = true;
        }
    }
    let timeline = Timeline { bpm: session.tempo(), beat: session.beat(), peers };
    for e in engines.iter_mut() {
        e.set_timeline(Some(timeline));
    }
    changed
}

/// A session that lives in this process, clocked by the audio it renders.
#[derive(Clone, Copy, Debug)]
pub struct LocalSession {
    bpm: f64,
    beat: f64,
    sample_rate: f64,
}

impl LocalSession {
    #[must_use]
    pub fn new(bpm: f64, sample_rate: u32) -> Self {
        Self { bpm, beat: 0.0, sample_rate: f64::from(sample_rate) }
    }

    /// `frames` were rendered.
    pub fn advance(&mut self, frames: u64) {
        #[expect(clippy::cast_precision_loss, reason = "a block's frame count, far below 2^52")]
        let secs = frames as f64 / self.sample_rate;
        self.beat = (secs * self.bpm).mul_add(1.0 / 60.0, self.beat);
    }
}

impl Session for LocalSession {
    fn tempo(&self) -> f64 {
        self.bpm
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.bpm = bpm;
    }

    fn beat(&self) -> f64 {
        self.beat
    }

    fn force_beat(&mut self, beat: f64) {
        self.beat = beat;
    }
}
