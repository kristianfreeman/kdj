//! Ableton Link, driven from the audio callback (feature "link").
//!
//! Each callback: work out when this block reaches the speakers on Link's
//! clock, let a MASTER deck set the session tempo (and phase, on engaging),
//! then hand every deck the session's tempo and beat at that moment. Decks on
//! SYNC follow it inside the engine. All of it is realtime-safe: Link's audio
//! functions are, and the session state and clock filter are made up front.

use std::sync::Arc;

use deck_engine::Engine;
use deck_engine::sync::{MASTER_DRIFT_BEATS, QUANTUM, Timeline, phase_error};
use rusty_link::{AblLink, SessionState};

pub struct LinkDriver {
    link: Arc<AblLink>,
    state: SessionState,
    clock: ClockFilter,
    sample_clock: u64,
}

impl LinkDriver {
    /// Not realtime-safe: call before starting audio.
    pub fn new(link: Arc<AblLink>, sample_rate: u32) -> Self {
        Self { link, state: SessionState::new(), clock: ClockFilter::new(sample_rate), sample_clock: 0 }
    }

    /// Before rendering a block: sync the decks to the session.
    /// `latency_us` is how long until this block is heard.
    fn sync_decks(&mut self, engines: &mut [Engine], latency_us: i64) {
        let host = self.clock.host_time(self.sample_clock, self.link.clock_micros());
        let heard = host + latency_us;
        self.link.capture_audio_session_state(&mut self.state);
        if lead(engines, &mut self.state, heard) {
            self.link.commit_audio_session_state(&self.state);
        }
        let peers = u32::try_from(self.link.num_peers()).unwrap_or(u32::MAX);
        let timeline = Timeline { bpm: self.state.tempo(), beat: self.state.beat_at_time(heard, QUANTUM), peers };
        for e in engines.iter_mut() {
            e.set_timeline(Some(timeline));
        }
    }
}

/// Let the deck on MASTER set the session's tempo, and its phase when it
/// takes over or drifts. MASTER is one deck at a time: a deck taking it
/// hands the others back to SYNC. Returns whether the session changed.
fn lead(engines: &mut [Engine], state: &mut SessionState, heard: i64) -> bool {
    let taking = engines.iter_mut().position(|e| e.lead().is_some_and(|l| l.align));
    let mut changed = false;
    for i in 0..engines.len() {
        let Some(e) = engines.get_mut(i) else { break };
        if taking.is_some_and(|t| t != i) {
            e.yield_master();
            continue;
        }
        let Some(l) = e.lead() else { continue };
        if (state.tempo() - l.bpm).abs() > 1e-4 {
            state.set_tempo(l.bpm, heard);
            changed = true;
        }
        let Some(beat) = l.beat else { continue };
        let drift = phase_error(state.beat_at_time(heard, QUANTUM), beat);
        if taking == Some(i) || drift.abs() > MASTER_DRIFT_BEATS {
            state.force_beat_at_time(beat, heard, QUANTUM);
            changed = true;
        }
    }
    changed
}

impl crate::audio::BlockHook for LinkDriver {
    fn before(&mut self, engines: &mut [Engine], latency_us: i64) {
        self.sync_decks(engines, latency_us);
    }

    fn after(&mut self, frames: u64) {
        self.sample_clock += frames;
    }
}

/// Maps the audio sample clock to Link's microsecond clock with a running
/// least-squares fit, smoothing out callback jitter. Fixed-size, allocation-
/// and panic-free.
struct ClockFilter {
    points: [(f64, f64); 256],
    len: usize,
    next: usize,
    nominal_us_per_frame: f64,
}

impl ClockFilter {
    fn new(sample_rate: u32) -> Self {
        Self { points: [(0.0, 0.0); 256], len: 0, next: 0, nominal_us_per_frame: 1e6 / f64::from(sample_rate) }
    }

    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "sample clocks and microsecond times stay far below 2^53, and whole microseconds are Link's unit"
    )]
    fn host_time(&mut self, sample_clock: u64, now_us: i64) -> i64 {
        let (x, y) = (sample_clock as f64, now_us as f64);
        if let Some(slot) = self.points.get_mut(self.next) {
            *slot = (x, y);
        }
        self.next = (self.next + 1) % self.points.len();
        self.len = (self.len + 1).min(self.points.len());
        let pts = self.points.get(..self.len).unwrap_or(&[]);
        let n = pts.len() as f64;
        let (mx, my) = pts.iter().fold((0.0, 0.0), |(a, b), &(px, py)| (a + px / n, b + py / n));
        let (sxy, sxx) = pts.iter().fold((0.0, 0.0), |(a, b), &(px, py)| {
            let dx = px - mx;
            (dx.mul_add(py - my, a), dx.mul_add(dx, b))
        });
        let slope = if sxx > 0.0 { sxy / sxx } else { self.nominal_us_per_frame };
        slope.mul_add(x - mx, my).round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::ClockFilter;

    #[test]
    fn the_clock_filter_smooths_jitter() {
        let mut f = ClockFilter::new(48_000);
        let mut last = 0;
        for i in 0..400_i64 {
            // 128-frame blocks at 48 kHz are 2666.67 µs apart; add ±300 µs jitter.
            let jitter = if i % 2 == 0 { 300 } else { -300 };
            let frames = u64::try_from(i * 128).unwrap_or_default();
            last = f.host_time(frames, 1_000_000 + i * 2667 + jitter);
        }
        let ideal = 1_000_000 + 399 * 2667;
        assert!((last - ideal).abs() < 60, "{last} vs {ideal}");
    }

    #[test]
    fn the_first_point_doesnt_divide_by_zero() {
        let mut f = ClockFilter::new(48_000);
        assert_eq!(f.host_time(0, 5_000), 5_000);
    }
}
