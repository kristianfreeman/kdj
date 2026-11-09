//! The sync view: two decks' beats, drawn to beatmatch by ear and eye with
//! SYNC off.
//!
//! Across the top, a narrow strip of both decks' beat markers around now,
//! CDJ-style: the first deck's above, the second's below, bars tall. In
//! phase, each beat's two markers meet in one line through the middle.
//!
//! Below, a Lissajous figure: the first deck's beat drives x and the
//! second's y (the sine of each beat's phase), traced over the last second.
//! In phase it closes to the rising diagonal; half a beat out, the falling
//! one; a quarter out, a circle, turning one way when the first deck is
//! ahead and the other way when it's behind. With the tempos apart the
//! figure keeps turning through those shapes, slower the closer they are;
//! matched, it holds still. Each axis swells a little on its deck's beat,
//! so the figure pulses with the music.
//!
//! The decks keep the same axes on every kdj (by letter), so both screens
//! draw the same figure.

use std::fmt::Write;

use crate::draw::{Frame, Rgba, alpha, grey};
use crate::scene::{HEIGHT, WIDTH};
use crate::text::{Align, Atlas, Style};

const BG: Rgba = grey(11);
const FG: Rgba = grey(242);
const DIM: Rgba = grey(140);
const FAINT: Rgba = grey(70);
const LANE: Rgba = grey(24);
/// The figure's glow, and the playhead: the browser's amber.
const AMBER: Rgba = [255, 178, 36, 255];

/// The beat strip: two lanes meeting at `STRIP_MID`.
const STRIP_MID: f32 = 34.0;
const LANE_H: f32 = 24.0;
/// Strip scale: about two beats either side of now at house tempos.
const PX_PER_SEC: f64 = 360.0;
const STRIP_X: f32 = 40.0;

/// The figure's centre and radius.
const CX: f32 = WIDTH / 2.0;
const CY: f32 = 254.0;
const R: f32 = 150.0;
/// How far back the figure's trace goes, and in how many steps.
const TRAIL_SECS: f64 = 1.0;
const STEPS: u16 = 160;
/// How much an axis swells on its deck's beat.
const PULSE: f64 = 0.07;

/// A deck's beat, now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckBeat {
    pub letter: char,
    /// Its grid's beat: whole numbers are beats, multiples of 4 downbeats.
    pub beat: f64,
    /// Its tempo, as it plays.
    pub bpm: f64,
    pub playing: bool,
}

impl DeckBeat {
    /// Its beat `secs` from now (negative: ago).
    fn at(self, secs: f64) -> f64 {
        if self.playing { (secs / 60.0).mul_add(self.bpm, self.beat) } else { self.beat }
    }
}

#[derive(Debug, Default)]
pub struct SyncView {
    pub open: bool,
    /// The deck this screen is for.
    pub mine: Option<DeckBeat>,
    /// The deck to match it with.
    pub other: Option<DeckBeat>,
    text: String,
}

impl SyncView {
    pub fn draw(&mut self, f: &mut Frame, atlas: &Atlas) {
        f.rect(0.0, 0.0, WIDTH, HEIGHT, BG);
        // By letter: the same axes on every kdj.
        let (first, second) = match (self.mine, self.other) {
            (Some(a), Some(b)) if b.letter < a.letter => (Some(b), Some(a)),
            pair => pair,
        };
        strip(f, atlas, first, second);
        match (first, second) {
            (Some(a), Some(b)) => {
                guides(f);
                figure(f, a, b);
                self.readout(f, atlas, a, b);
            }
            (Some(d), None) | (None, Some(d)) => {
                let hint = "Nothing to match yet: play a track on another deck";
                atlas.draw(f, Style::Body, CX, CY, Align::Center, hint, DIM);
                self.text.clear();
                let _ = write!(self.text, "{}  {:.2} BPM", d.letter, d.bpm);
                atlas.draw(f, Style::Body, 16.0, HEIGHT - 20.0, Align::Left, &self.text, FG);
            }
            (None, None) => {
                atlas.draw(f, Style::Body, CX, CY, Align::Center, "Load a track to match", DIM);
            }
        }
        atlas.draw(f, Style::Label, WIDTH - 16.0, HEIGHT - 20.0, Align::Right, "double tap or hold back ×", DIM);
    }

    /// Tempos at the sides; between them how the second deck stands against
    /// the first, in words.
    fn readout(&mut self, f: &mut Frame, atlas: &Atlas, a: DeckBeat, b: DeckBeat) {
        let text = &mut self.text;
        let y = HEIGHT - 48.0;
        for (d, x, align) in [(a, 16.0, Align::Left), (b, WIDTH - 16.0, Align::Right)] {
            text.clear();
            let _ = write!(text, "{}  {:.2}", d.letter, d.bpm);
            atlas.draw(f, Style::Body, x, y, align, text, if d.playing { FG } else { DIM });
        }
        text.clear();
        let dbpm = b.bpm - a.bpm;
        if dbpm.abs() < 0.005 {
            text.push_str("same tempo");
        } else {
            let _ = write!(text, "{} {:.2} BPM {}", b.letter, dbpm.abs(), if dbpm > 0.0 { "faster" } else { "slower" });
        }
        if a.playing && b.playing {
            // Where B's beat falls against A's, within half a beat.
            let apart = (b.beat.fract() - a.beat.fract() + 0.5).rem_euclid(1.0) - 0.5;
            let ms = apart * 60_000.0 / a.bpm;
            if ms.abs() < 0.5 {
                text.push_str("   ·   in phase");
            } else {
                let _ = write!(
                    text,
                    "   ·   {} {:.1} ms {}",
                    b.letter,
                    ms.abs(),
                    if ms > 0.0 { "ahead" } else { "behind" }
                );
            }
        }
        atlas.draw(f, Style::Body, CX, y, Align::Center, text, FG);
    }
}

/// Both decks' beat markers around now: the first's hang above the middle
/// line, the second's below, so markers in phase meet in one line. Bars
/// are full height and bright, beats half and grey.
fn strip(f: &mut Frame, atlas: &Atlas, first: Option<DeckBeat>, second: Option<DeckBeat>) {
    let (left, right) = (STRIP_X, WIDTH - 16.0);
    f.rect(left, STRIP_MID - LANE_H, right - left, 2.0 * LANE_H, LANE);
    for (deck, up) in [(first, true), (second, false)] {
        let Some(d) = deck else { continue };
        let label_y = if up { STRIP_MID - 7.0 } else { STRIP_MID + 18.0 };
        atlas.draw(f, Style::Label, 16.0, label_y, Align::Left, &d.letter.to_string(), FG);
        let beats_per_sec = d.bpm / 60.0;
        if beats_per_sec <= 0.0 {
            continue;
        }
        let half = f64::from(right - CX) / PX_PER_SEC * beats_per_sec;
        let first_beat = (d.beat - half).ceil();
        // The beats on screen, at most a few dozen.
        let count = whole((d.beat + half - first_beat).floor()) + 1;
        for beat in (0..count).map(|i| first_beat + f64::from(i)) {
            let x = narrow(((beat - d.beat) / beats_per_sec).mul_add(PX_PER_SEC, f64::from(CX)));
            if x < left {
                continue;
            }
            let bar = beat.rem_euclid(4.0) < 0.5;
            let (height, width, color) = if bar { (LANE_H - 2.0, 3.0, FG) } else { (LANE_H * 0.5, 2.0, DIM) };
            let top = if up { STRIP_MID - height } else { STRIP_MID };
            f.rect(x - width / 2.0, top, width, height, color);
        }
    }
    f.rect(left, STRIP_MID - 0.5, right - left, 1.0, FAINT);
    f.rect(CX - 1.0, STRIP_MID - LANE_H - 4.0, 2.0, 2.0f32.mul_add(LANE_H, 8.0), AMBER);
}

/// Faint marks the figure settles onto: the in-phase diagonal and the
/// square it moves in.
fn guides(f: &mut Frame) {
    let c = alpha(FAINT, 150);
    f.line([CX - R, CY + R], [CX + R, CY - R], 1.0, c);
    let arm = 14.0;
    for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let (x, y) = (sx.mul_add(R, CX), sy.mul_add(R, CY));
        f.rect(if sx < 0.0 { x } else { x - arm }, y - 0.5, arm, 1.0, c);
        f.rect(x - 0.5, if sy < 0.0 { y } else { y - arm }, 1.0, arm, c);
    }
}

/// The Lissajous trace over the last `TRAIL_SECS`, brightest at now, with
/// a dot where it is.
fn figure(f: &mut Frame, a: DeckBeat, b: DeckBeat) {
    let point = |secs: f64| {
        let (beat_a, beat_b) = (a.at(secs), b.at(secs));
        let across = f64::from(R) * PULSE.mul_add(pulse(beat_a), 1.0);
        let up = f64::from(R) * PULSE.mul_add(pulse(beat_b), 1.0);
        [
            narrow(across.mul_add(phase_sin(beat_a), f64::from(CX))),
            narrow((-up).mul_add(phase_sin(beat_b), f64::from(CY))),
        ]
    };
    let mut prev = point(-TRAIL_SECS);
    for i in 1..=STEPS {
        let age = f64::from(i) / f64::from(STEPS);
        let p = point(TRAIL_SECS * (age - 1.0));
        let fade = narrow(age * age);
        f.line(prev, p, 10.0, alpha(AMBER, shade(fade * 26.0)));
        f.line(prev, p, 4.5, alpha(AMBER, shade(fade * 70.0)));
        f.line(prev, p, 1.8, alpha(FG, shade(fade * 235.0)));
        prev = p;
    }
    let now = point(0.0);
    let beat = narrow(pulse(a.beat).max(pulse(b.beat)));
    f.disc(now, 4.0f32.mul_add(beat, 9.0), alpha(AMBER, 60));
    f.disc(now, 2.0f32.mul_add(beat, 4.0), FG);
}

/// 1 on a beat, falling away quickly after it.
fn pulse(beat: f64) -> f64 {
    (-5.0 * beat.rem_euclid(1.0)).exp()
}

/// The sine of a beat's phase.
fn phase_sin(beat: f64) -> f64 {
    (beat.rem_euclid(1.0) * std::f64::consts::TAU).sin()
}

/// An alpha from 0–255.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0–255 first")]
const fn shade(v: f32) -> u8 {
    v.clamp(0.0, 255.0) as u8
}

/// A small non-negative whole number of beats, as a count.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a few dozen beats at most")]
const fn whole(v: f64) -> u16 {
    if v < 0.0 { 0 } else { v as u16 }
}

#[expect(clippy::cast_possible_truncation, reason = "screen coordinates")]
const fn narrow(v: f64) -> f32 {
    v as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(letter: char, beat: f64, bpm: f64) -> DeckBeat {
        DeckBeat { letter, beat, bpm, playing: true }
    }

    #[test]
    fn decks_in_phase_trace_the_rising_diagonal() {
        let (a, b) = (deck('A', 16.3, 124.0), deck('B', 40.3, 124.0));
        for secs in [0.0, -0.1, -0.37] {
            assert!((phase_sin(a.at(secs)) - phase_sin(b.at(secs))).abs() < 1e-9);
        }
    }

    #[test]
    fn the_same_figure_on_both_kdjs() {
        let (a, b) = (deck('A', 16.3, 124.0), deck('B', 40.55, 124.2));
        let draw = |mine, other| {
            let mut v = SyncView { open: true, mine: Some(mine), other: Some(other), text: String::new() };
            let mut f = Frame::default();
            v.draw(&mut f, &Atlas::new(1.0));
            f.verts.iter().map(|v| v.pos).collect::<Vec<_>>()
        };
        assert_eq!(draw(a, b), draw(b, a));
    }

    #[test]
    fn a_stopped_deck_stays_put() {
        let d = DeckBeat { playing: false, ..deck('A', 3.25, 124.0) };
        assert!((d.at(-0.5) - 3.25).abs() < f64::EPSILON);
    }
}
