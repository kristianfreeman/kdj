//! Constant-tempo beat grid: one BPM and one downbeat anchor.
//!
//! Hardgroove and hypnotic techno is produced on a fixed tempo, so a single
//! period is both correct and what Rekordbox stores. Everything is in frames
//! at the engine's sample rate.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatGrid {
    pub bpm: f64,
    /// Frame of a downbeat (beat 1 of a bar). Beat `k` sits at `anchor + k * period`.
    pub anchor: f64,
    pub sample_rate: u32,
}

pub const BEATS_PER_BAR: f64 = 4.0;
/// `BEATS_PER_BAR` for integer beat arithmetic.
const BEATS_PER_BAR_INT: i64 = 4;

impl BeatGrid {
    #[must_use]
    pub const fn new(bpm: f64, anchor: f64, sample_rate: u32) -> Self {
        Self { bpm, anchor, sample_rate }
    }

    /// Frames per beat.
    #[must_use]
    pub fn period(&self) -> f64 {
        60.0 * f64::from(self.sample_rate) / self.bpm
    }

    /// Fractional beat index at `frame`; 0.0 is the anchor downbeat.
    #[must_use]
    pub fn beat_at(&self, frame: f64) -> f64 {
        (frame - self.anchor) / self.period()
    }

    #[must_use]
    pub fn frame_of(&self, beat: f64) -> f64 {
        beat.mul_add(self.period(), self.anchor)
    }

    /// The beat at or just before `frame`. A press a hair early still counts as on the beat.
    #[must_use]
    pub fn floor_beat(&self, frame: f64) -> f64 {
        (self.beat_at(frame) + 1e-3).floor()
    }

    #[must_use]
    pub fn nearest_beat(&self, frame: f64) -> f64 {
        self.beat_at(frame).round()
    }

    /// Change tempo while keeping the beat nearest `pivot` where it is, so the
    /// grid doesn't slide away under the playhead while you adjust it.
    #[must_use]
    pub fn with_bpm(self, bpm: f64, pivot: f64) -> Self {
        let beat = self.nearest_beat(pivot);
        let fixed = self.frame_of(beat);
        let mut g = Self { bpm, ..self };
        g.anchor = beat.mul_add(-g.period(), fixed);
        g
    }

    /// Make `frame` a downbeat, keeping the tempo.
    #[must_use]
    pub const fn anchored_at(self, frame: f64) -> Self {
        Self { anchor: frame, ..self }
    }

    #[must_use]
    pub fn shifted(self, frames: f64) -> Self {
        Self { anchor: self.anchor + frames, ..self }
    }

    /// (bar, beat-in-bar) counting from 1, like a CDJ display. Bar 1 starts at the anchor.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the floored beat index is a whole number far inside i64, and rem_euclid(4) is always 0..4, so it fits a u8"
    )]
    pub fn bar_beat(&self, frame: f64) -> (i64, u8) {
        let beat = self.beat_at(frame).floor() as i64;
        let bar = beat.div_euclid(BEATS_PER_BAR_INT);
        let in_bar = beat.rem_euclid(BEATS_PER_BAR_INT);
        (bar + 1, in_bar as u8 + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_round_trip() {
        let g = BeatGrid::new(134.0, 1000.0, 48_000);
        assert!((g.frame_of(g.beat_at(12_345.0)) - 12_345.0).abs() < 1e-9);
        assert!((g.floor_beat(g.period().mul_add(2.5, 1000.0)) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bpm_change_pivots_on_nearest_beat() {
        let g = BeatGrid::new(134.0, 0.0, 48_000);
        let pivot = g.frame_of(64.0) + 10.0;
        let g2 = g.with_bpm(134.5, pivot);
        assert!((g2.frame_of(64.0) - g.frame_of(64.0)).abs() < 1e-6);
    }

    #[test]
    fn bar_beat_counts_from_one() {
        let g = BeatGrid::new(120.0, 0.0, 48_000);
        assert_eq!(g.bar_beat(0.0), (1, 1));
        assert_eq!(g.bar_beat(g.frame_of(5.5)), (2, 2));
        assert_eq!(g.bar_beat(g.frame_of(-1.0)), (0, 4));
    }
}
