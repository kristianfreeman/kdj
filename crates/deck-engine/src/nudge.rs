//! Nudging: pushing a playing deck's phase into line with another.
//!
//! - Hold: bends tempo while held, starting gentle and building, so a tap is
//!   a small correction and a hold catches up a big drift.
//! - Jog: each tick (an encoder detent, a key repeat) adds a little speed that
//!   decays away, like touching the edge of a jog wheel.
//! - Step: an instant shift by a few milliseconds (handled by the engine as a
//!   jump), for exact trims.

#[derive(Clone, Copy, Debug)]
pub struct NudgeConfig {
    /// Bend when a hold starts, as a fraction of speed (0.01 = 1%).
    pub hold_start: f64,
    /// Bend after holding for `hold_ramp_secs`.
    pub hold_max: f64,
    pub hold_ramp_secs: f64,
    /// Speed added per jog tick.
    pub jog_impulse: f64,
    /// Time constant the jog speed decays with.
    pub jog_decay_secs: f64,
    /// Size of a phase step.
    pub step_secs: f64,
    /// Total bend never exceeds this.
    pub max_bend: f64,
}

impl Default for NudgeConfig {
    fn default() -> Self {
        // A 0.3 s tap moves ~3 ms; a 1 s hold ~25 ms; a jog tick ~1 ms.
        Self {
            hold_start: 0.01,
            hold_max: 0.04,
            hold_ramp_secs: 1.0,
            jog_impulse: 0.005,
            jog_decay_secs: 0.2,
            step_secs: 0.002,
            max_bend: 0.08,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Nudge {
    pub cfg: NudgeConfig,
    forward: bool,
    back: bool,
    held_secs: f64,
    jog: f64,
}

impl Nudge {
    #[must_use]
    pub const fn new(cfg: NudgeConfig) -> Self {
        Self { cfg, forward: false, back: false, held_secs: 0.0, jog: 0.0 }
    }

    pub fn hold(&mut self, dir: f64, held: bool) {
        if dir > 0.0 {
            self.forward = held;
        } else {
            self.back = held;
        }
        self.held_secs = 0.0;
    }

    pub const fn jog(&mut self, dir: f64) {
        self.jog = dir.mul_add(self.cfg.jog_impulse, self.jog);
    }

    /// Current bend, as a fraction of speed.
    #[must_use]
    pub fn bend(&self) -> f64 {
        let dir = i8::from(self.forward) - i8::from(self.back);
        let ramp = (self.held_secs / self.cfg.hold_ramp_secs).min(1.0);
        let hold = f64::from(dir) * (self.cfg.hold_max - self.cfg.hold_start).mul_add(ramp, self.cfg.hold_start);
        (hold + self.jog).clamp(-self.cfg.max_bend, self.cfg.max_bend)
    }

    /// Move time on by `secs` of playback: holds ramp up, jogs decay.
    pub fn advance(&mut self, secs: f64) {
        if self.forward != self.back {
            self.held_secs += secs;
        }
        self.jog *= (-secs / self.cfg.jog_decay_secs).exp();
        if self.jog.abs() < 1e-6 {
            self.jog = 0.0;
        }
    }

    /// Forget any jog speed (e.g. on stop).
    pub const fn settle(&mut self) {
        self.jog = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase moved (seconds) by a nudge, integrated in 1 ms steps.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a short positive duration in 1 ms steps; truncation is the intended step count"
    )]
    fn phase_moved(n: &mut Nudge, secs: f64) -> f64 {
        let dt = 0.001;
        (0..(secs / dt) as usize)
            .map(|_| {
                let b = n.bend();
                n.advance(dt);
                b * dt
            })
            .sum()
    }

    #[test]
    fn a_tap_is_small_and_a_hold_is_large() {
        let mut n = Nudge::new(NudgeConfig::default());
        n.hold(1.0, true);
        let tap = phase_moved(&mut n, 0.3);
        assert!((0.003..0.006).contains(&tap), "tap moved {:.1} ms", tap * 1000.0);
        let mut n = Nudge::new(NudgeConfig::default());
        n.hold(1.0, true);
        let hold = phase_moved(&mut n, 1.0);
        assert!((0.02..0.03).contains(&hold), "hold moved {:.1} ms", hold * 1000.0);
    }

    #[test]
    fn a_jog_tick_moves_about_a_millisecond_and_fades() {
        let mut n = Nudge::new(NudgeConfig::default());
        n.jog(1.0);
        let moved = phase_moved(&mut n, 2.0);
        assert!((0.0009..0.0011).contains(&moved), "tick moved {:.2} ms", moved * 1000.0);
        assert!(n.bend().abs() < 1e-4);
    }

    #[test]
    fn releasing_stops_the_bend() {
        let mut n = Nudge::new(NudgeConfig::default());
        n.hold(-1.0, true);
        phase_moved(&mut n, 0.5);
        n.hold(-1.0, false);
        assert!(n.bend().abs() < f64::EPSILON);
    }
}
