//! Driving decks from a shared session: Ableton Link's, or a local one when
//! there's no network (`--no-link`, or a build without Link).
//!
//! Each block, before rendering: the deck on MASTER sets the session's tempo,
//! and its phase when it takes over or drifts; then every deck gets the
//! session's tempo and beat for that block. Realtime-safe.
//!
//! One deck holds MASTER on the whole network, like CDJs: taking it, here or
//! on another kdj, puts every other master back on SYNC. A master that stops
//! hands it to a deck playing on SYNC, so moving on to the next track moves
//! MASTER with it (one given MASTER while stopped keeps it till it plays).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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

/// Who holds MASTER across kdjs. Lock-free.
///
/// Shared by the audio thread (which decides) and the network's (which tells
/// the others, in its hellos). Each kdj claims it with a number above every
/// claim it has heard; the highest claim stands and the others' masters go
/// back to SYNC.
#[derive(Debug, Default)]
pub struct Master {
    /// Ties between kdjs claiming at once: from the name, so each differs.
    id: u32,
    /// Our claim, `(n << 32) | id`; 0 while no deck here is master.
    ours: AtomicU64,
    /// Ours is stopped and will hand over (see `Engine::master_stopped`).
    ours_stopped: AtomicBool,
    /// The highest claim another kdj makes, and whether that master is
    /// stopped; 0 for none.
    theirs: AtomicU64,
    theirs_stopped: AtomicBool,
}

impl Master {
    /// For the kdj called `name`.
    #[must_use]
    pub fn new(name: &str) -> Self {
        // FNV-1a: stable across builds and machines, unlike std's hasher.
        let id = name.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
        Self { id, ..Self::default() }
    }

    /// Our claim and whether that master is stopped, for our hellos.
    pub fn ours(&self) -> (u64, bool) {
        (self.ours.load(Ordering::Relaxed), self.ours_stopped.load(Ordering::Relaxed))
    }

    /// The highest claim the other kdjs make (0 for none), as heard.
    pub fn heard(&self, claim: u64, stopped: bool) {
        self.theirs_stopped.store(stopped, Ordering::Relaxed);
        self.theirs.store(claim, Ordering::Relaxed);
    }

    fn take(&self) {
        let n = (self.ours.load(Ordering::Relaxed) >> 32).max(self.theirs.load(Ordering::Relaxed) >> 32) + 1;
        self.ours.store((n << 32) | u64::from(self.id), Ordering::Relaxed);
    }

    /// Another kdj has taken MASTER since we did.
    fn outranked(&self) -> bool {
        let ours = self.ours.load(Ordering::Relaxed);
        ours != 0 && self.theirs.load(Ordering::Relaxed) > ours
    }

    fn stopped_elsewhere(&self) -> bool {
        self.theirs.load(Ordering::Relaxed) != 0 && self.theirs_stopped.load(Ordering::Relaxed)
    }

    /// Whether a deck here is master now (and if so, stopped).
    fn here(&self, master: Option<bool>) {
        match master {
            None => self.ours.store(0, Ordering::Relaxed),
            Some(stopped) => {
                if self.ours.load(Ordering::Relaxed) == 0 {
                    self.take();
                }
                self.ours_stopped.store(stopped, Ordering::Relaxed);
            }
        }
    }
}

/// Let the deck on MASTER lead `session`, then hand every deck its timeline.
///
/// MASTER is one deck at a time (see the module docs): a deck taking it hands
/// the others back to SYNC, here and, through `master`, on other kdjs.
/// `peers`: Link peers, or None for a local session.
/// Returns whether the session changed (Link wants a commit).
pub fn drive(engines: &mut [Engine], session: &mut impl Session, peers: Option<u32>, master: &Master) -> bool {
    let mut taking = engines.iter_mut().position(Engine::took_master);
    if taking.is_none() && master.outranked() {
        engines.iter_mut().for_each(Engine::yield_master);
    }
    // The master stopped: a deck here playing on SYNC takes over, in phase.
    let stopped = engines.iter().any(Engine::master_stopped) || master.stopped_elsewhere();
    if taking.is_none()
        && stopped
        && let Some(i) = engines.iter().position(Engine::can_take_master)
    {
        engines[i].take_master();
        engines[i].took_master();
        taking = Some(i);
    }
    if taking.is_some() {
        master.take();
    }
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
        if l.align || phase_error(session.beat(), beat).abs() > MASTER_DRIFT_BEATS {
            session.force_beat(beat);
            changed = true;
        }
    }
    master.here(engines.iter().find(|e| e.is_master()).map(Engine::master_stopped));
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
