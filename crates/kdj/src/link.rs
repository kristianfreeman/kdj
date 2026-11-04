//! Ableton Link, driven from the audio callback (feature "link").
//!
//! Each callback: work out when this block reaches the speakers on Link's
//! clock, let a MASTER deck set the session tempo (and phase, on engaging),
//! then hand every deck the session's tempo and beat at that moment. Decks on
//! SYNC follow it inside the engine. All of it is realtime-safe: Link's audio
//! functions are, and the session state and clock filter are made up front.

use std::sync::Arc;

use kdj_engine::Engine;
use kdj_engine::session::{self, Master, Session};
use kdj_engine::sync::QUANTUM;
use rusty_link::{AblLink, SessionState};

pub struct LinkDriver {
    link: Arc<AblLink>,
    /// MASTER across the kdjs on the session (the network tells the others).
    master: Arc<Master>,
    state: SessionState,
    clock: ClockFilter,
    sample_clock: u64,
}

impl LinkDriver {
    /// Not realtime-safe: call before starting audio.
    pub fn new(link: Arc<AblLink>, master: Arc<Master>, sample_rate: u32) -> Self {
        Self { link, master, state: SessionState::new(), clock: ClockFilter::new(sample_rate), sample_clock: 0 }
    }

    /// Before rendering a block: sync the decks to the session.
    /// `latency_us` is how long until this block is heard.
    fn sync_decks(&mut self, engines: &mut [Engine], latency_us: i64) {
        let host = self.clock.host_time(self.sample_clock, self.link.clock_micros());
        self.link.capture_audio_session_state(&mut self.state);
        let peers = Some(u32::try_from(self.link.num_peers()).unwrap_or(u32::MAX));
        let mut at = LinkAt { state: &mut self.state, heard: host + latency_us };
        if session::drive(engines, &mut at, peers, &self.master) {
            self.link.commit_audio_session_state(&self.state);
        }
    }
}

/// Link's session state, seen from the moment a block is heard.
struct LinkAt<'a> {
    state: &'a mut SessionState,
    heard: i64,
}

impl Session for LinkAt<'_> {
    fn tempo(&self) -> f64 {
        self.state.tempo()
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.state.set_tempo(bpm, self.heard);
    }

    fn beat(&self) -> f64 {
        self.state.beat_at_time(self.heard, QUANTUM)
    }

    fn force_beat(&mut self, beat: f64) {
        self.state.force_beat_at_time(beat, self.heard, QUANTUM);
    }
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
