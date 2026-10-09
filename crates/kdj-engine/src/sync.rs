//! Following a shared tempo and beat timeline (Ableton Link), CDJ-style.
//!
//! A deck on SYNC matches the timeline's tempo (or its half or double) and,
//! when it starts playing or SYNC goes on, lines its bars up with the
//! timeline's 4-beat phase. A big phase error is fixed with one click-free
//! jump; small ones are trimmed with gentle tempo corrections. After that it
//! holds the beat phase: a deliberate beat jump or a short loop moves the bar
//! alignment with it instead of being undone.
//! A deck on MASTER leads instead: its tempo becomes the timeline's tempo.
//!
//! Pure: the driver hands the engine a `Timeline` each block, so the
//! simulator can stand in for Link.

/// Beats in a phrase unit: bars line up, not just beats.
pub const QUANTUM: f64 = 4.0;
/// Beyond this phase error (beats) a follower jumps instead of bending.
const SNAP_BEATS: f64 = 0.1;
/// Rate correction per beat of phase error: settles in about 0.75 s at 134 BPM.
const GAIN: f64 = 0.6;
/// Never bend more than this to correct phase.
const MAX_BEND: f64 = 0.02;
/// How far (beats) a master may drift from the timeline: about 2 ms.
///
/// Past this the driver re-asserts the master's phase. Drift comes from nudging the master or
/// changing its tempo, whose sound follows the stretcher's latency later.
pub const MASTER_DRIFT_BEATS: f64 = 0.005;
/// Furthest a follower stretches to match tempo, after folding to the
/// nearest half or double: beyond this it holds tempo and doesn't lock.
const MAX_STRETCH: f64 = 0.5;

/// The shared timeline, as heard at the first frame of this block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    pub bpm: f64,
    /// Beat number on the shared timeline when this block reaches the speakers.
    pub beat: f64,
    /// Other Link participants (not counting this app); None for a session
    /// local to this process.
    pub peers: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncMode {
    #[default]
    Off,
    /// Match the timeline's tempo and bar phase.
    Follow,
    /// Set the timeline's tempo (and its phase, on engaging).
    Lead,
}

/// What a following deck should do this block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Follow {
    /// Playback rate to use (tempo match plus phase trim).
    pub rate: f64,
    /// The rate that matches tempo alone, for the tempo readout.
    pub base_rate: f64,
    /// Move the playhead by this many beats first, if far out.
    pub snap_beats: Option<f64>,
    /// Phase error in beats, deck minus timeline, within ±2.
    pub error: f64,
}

/// How a deck at `deck_beat` (on a `deck_bpm` grid) follows `tl`, aligning
/// phase modulo `quantum` beats (`QUANTUM` for bars, 1 for beats).
#[must_use]
pub fn follow(deck_beat: f64, deck_bpm: f64, tl: &Timeline, quantum: f64) -> Follow {
    // A 70 BPM grid follows 140 at its own tempo, two timeline beats a beat.
    let fold = fold(tl.bpm / deck_bpm);
    let base_rate = tl.bpm / (deck_bpm * fold);
    if !base_rate.is_finite() || (base_rate - 1.0).abs() > MAX_STRETCH {
        let hold = if base_rate.is_finite() { base_rate.clamp(1.0 - MAX_STRETCH, 1.0 + MAX_STRETCH) } else { 1.0 };
        return Follow { rate: hold, base_rate: hold, snap_beats: None, error: 0.0 };
    }
    let error = wrap(deck_beat.mul_add(fold, -tl.beat), quantum);
    if error.abs() > SNAP_BEATS {
        return Follow { rate: base_rate, base_rate, snap_beats: Some(-error / fold), error };
    }
    // Behind (negative error): speed up a touch; ahead: slow down.
    let trim = (-GAIN * error).clamp(-MAX_BEND, MAX_BEND);
    Follow { rate: base_rate * (1.0 + trim), base_rate, snap_beats: None, error }
}

/// Where a deck's beat falls against the timeline's, in timeline beats
/// within ±½ (positive: the deck is ahead), counting a half- or
/// double-tempo grid as matching.
#[must_use]
pub fn beat_phase(deck_beat: f64, deck_bpm: f64, tl: &Timeline) -> f64 {
    wrap(deck_beat.mul_add(fold(tl.bpm / deck_bpm), -tl.beat), 1.0)
}

/// The power of two (¼ to 4) that brings a tempo ratio closest to 1.
fn fold(ratio: f64) -> f64 {
    let mut f = 1.0;
    while ratio / f > std::f64::consts::SQRT_2 && f < 4.0 {
        f *= 2.0;
    }
    while ratio / f < std::f64::consts::FRAC_1_SQRT_2 && f > 0.25 {
        f /= 2.0;
    }
    f
}

/// `a - b` in beats, wrapped to the nearest bar alignment (±half a quantum).
#[must_use]
pub fn phase_error(a: f64, b: f64) -> f64 {
    wrap(a - b, QUANTUM)
}

fn wrap(d: f64, quantum: f64) -> f64 {
    (d + quantum / 2.0).rem_euclid(quantum) - quantum / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tl(bpm: f64, beat: f64) -> Timeline {
        Timeline { bpm, beat, peers: Some(1) }
    }

    #[test]
    fn phase_error_wraps_to_the_nearest_bar() {
        assert!((phase_error(4.1, 0.0) - 0.1).abs() < 1e-9);
        assert!((phase_error(3.9, 0.0) + 0.1).abs() < 1e-9);
        assert!((phase_error(10.0, 7.0) + 1.0).abs() < 1e-9, "3 beats ahead is 1 behind the next bar");
    }

    #[test]
    fn far_out_snaps_and_close_bends() {
        let far = follow(0.5, 130.0, &tl(134.0, 0.0), QUANTUM);
        assert_eq!(far.snap_beats, Some(-0.5));
        assert!((far.rate - 134.0 / 130.0).abs() < 1e-12);
        let behind = follow(-0.02, 134.0, &tl(134.0, 0.0), QUANTUM);
        assert!(behind.snap_beats.is_none());
        assert!(behind.rate > 1.0, "behind speeds up: {}", behind.rate);
        let ahead = follow(0.02, 134.0, &tl(134.0, 0.0), QUANTUM);
        assert!(ahead.rate < 1.0);
    }

    #[test]
    fn the_bend_is_capped() {
        let f = follow(-0.09, 134.0, &tl(134.0, 0.0), QUANTUM);
        assert!((f.rate - 1.02).abs() < 1e-12);
    }

    #[test]
    fn half_and_double_tempo_fold_in() {
        let half = follow(0.0, 70.0, &tl(140.0, 0.0), QUANTUM);
        assert!((half.base_rate - 1.0).abs() < 1e-12, "70 follows 140 at its own speed");
        // Deck beat 1 of a 70 grid is timeline beat 2: two timeline beats out.
        let out = follow(1.0, 70.0, &tl(140.0, 0.0), QUANTUM);
        assert_eq!(out.snap_beats, Some(1.0), "one deck beat back to the bar");
        assert!((follow(0.0, 130.0, &tl(64.0, 0.0), QUANTUM).base_rate - 128.0 / 130.0).abs() < 1e-12);
    }

    #[test]
    fn an_impossible_or_broken_tempo_is_held_not_followed() {
        for bpm in [999.0, 0.0, f64::NAN] {
            let f = follow(0.3, 128.0, &tl(bpm, 0.0), QUANTUM);
            assert!(f.rate.is_finite() && (0.5..=1.5).contains(&f.rate), "{bpm}: {}", f.rate);
            assert_eq!(f.snap_beats, None);
        }
        let f = follow(0.3, 0.0, &tl(128.0, 0.0), QUANTUM);
        assert!(f.rate.is_finite() && f.snap_beats.is_none());
    }

    #[test]
    fn beat_phase_ignores_whole_beats() {
        assert_eq!(follow(2.0, 134.0, &tl(134.0, 0.0), 1.0).snap_beats, None);
        assert_eq!(follow(2.0, 134.0, &tl(134.0, 0.0), QUANTUM).snap_beats, Some(2.0));
    }
}
