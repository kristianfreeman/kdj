//! LED feedback for MIDI controllers.
//!
//! A mapped button's LED is lit while what it does is on (playing, at the
//! cue, SYNC, the active loop size, a stored hot cue…), so the controller
//! shows the deck's state the way a CDJ's buttons do. Pure: which LEDs exist, when they're lit, and the bytes
//! that light them; the app owns the MIDI output.
//!
//! The light goes to the button's own note (or CC) on its channel, which is
//! how most controllers address their LEDs. On the Xone:K2/K3 that's red;
//! `led_offset = 36` makes it amber and `72` green. A binding's `led` names
//! a different note outright.

use crate::action::{Action, LOOP_SIZES};
use crate::engine::Snapshot;
use crate::midimap::{MidiConfig, Target};
use crate::sync::SyncMode;

/// One controller LED that shows a deck's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Led {
    /// 0-based MIDI channel.
    pub channel: u8,
    /// Lit by a CC rather than a note.
    pub cc: bool,
    pub number: u8,
    /// Whose state it shows.
    pub action: Action,
    pub target: Target,
}

impl Led {
    /// The message that sets it: note on (velocity 127 or 0) or CC (127 or 0).
    #[must_use]
    pub const fn message(&self, on: bool) -> [u8; 3] {
        let status = if self.cc { 0xb0 } else { 0x90 };
        [status | (self.channel & 0x0f), self.number & 0x7f, if on { 127 } else { 0 }]
    }
}

/// The LEDs a mapping can light: its buttons whose action has an on state.
/// A binding without a channel lights channel 1.
#[must_use]
pub fn leds(cfg: &MidiConfig) -> Vec<Led> {
    let offset = cfg.led_offset.unwrap_or(0);
    cfg.map
        .iter()
        .filter_map(|b| {
            let action = b.action.filter(|&a| has_state(a))?;
            let (cc, number) = match (b.led, b.note, b.cc) {
                (Some(n), ..) => (false, n),
                (None, Some(n), _) => (false, n.saturating_add(offset)),
                (None, None, Some(c)) => (true, c),
                (None, None, None) => return None,
            };
            let channel = b.channel.unwrap_or(1).saturating_sub(1);
            Some(Led {
                channel,
                cc,
                number: number & 0x7f,
                action,
                target: b.deck.map_or(Target::Focused, Target::Deck),
            })
        })
        .collect()
}

/// Early in a beat: its first quarter, or half on a bar's first beat.
fn on_the_beat(s: &Snapshot) -> bool {
    let beat = s.grid.beat_at(s.pos);
    let into = beat - beat.floor();
    into < if beat.rem_euclid(4.0) < 1.0 { 0.5 } else { 0.25 }
}

/// Whether `action` has an on state to show.
#[must_use]
pub fn has_state(action: Action) -> bool {
    lit(action, &Snapshot::default()).is_some()
}

/// Whether `action`'s LED is lit for a deck in state `s`; None if the action
/// has nothing to show.
#[must_use]
pub fn lit(action: Action, s: &Snapshot) -> Option<bool> {
    if let Some(i) = action.hot_cue_index() {
        return Some(s.hot_cues.get(i).is_some_and(Option::is_some));
    }
    if let Some(i) = action.beat_loop_index() {
        #[expect(clippy::float_cmp, reason = "loop_beats is copied from LOOP_SIZES, so equality picks out the entry")]
        let this_size = LOOP_SIZES.get(i).is_some_and(|&b| b == s.loop_beats);
        return Some(s.loop_active && this_size);
    }
    Some(match action {
        // Playing: a flash on every beat, longer on the bar's first.
        Action::PlayPause => s.playing && !s.previewing && on_the_beat(s),
        Action::Lock => s.locked,
        // At the cue point (or previewing it), as the screen's CUE shows.
        Action::Cue => s.previewing || (!s.playing && s.track_frames > 0 && (s.pos - s.cue).abs() < 1.0),
        Action::Sync => s.sync == SyncMode::Follow,
        Action::Master => s.sync == SyncMode::Lead,
        Action::KeyLock => s.keylock,
        Action::Quantize => s.quantize,
        Action::LoopToggle => s.loop_active,
        Action::TempoRange => s.wide_range,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midimap::MidiMap;

    const CONFIG: &str = r#"
        [midi]
        led_offset = 72

        [[midi.map]]
        channel = 15
        note = 36
        action = "PlayPause"

        [[midi.map]]
        note = 40
        action = "HotCue1"
        led = 100
        deck = 1

        [[midi.map]]
        cc = 20
        action = "Sync"

        [[midi.map]]
        note = 41
        action = "BeatJumpForward"

        [[midi.map]]
        cc = 0
        up = "JogForward"
        down = "JogBack"
    "#;

    #[test]
    fn buttons_with_a_state_get_an_led() {
        let (_, cfg) = MidiMap::from_toml(CONFIG).expect("valid");
        let leds = leds(&cfg);
        assert_eq!(leds.len(), 3, "beat jump and the encoder have nothing to show");
        let play = leds[0];
        assert_eq!(play.message(true), [0x9e, 108, 127], "channel 15, note 36 + 72 (green on a K3)");
        assert_eq!(play.message(false), [0x9e, 108, 0]);
        assert_eq!((leds[1].number, leds[1].target), (100, Target::Deck(1)), "an explicit led note");
        assert_eq!(leds[2].message(true), [0xb0, 20, 127], "a CC button lights by CC");
    }

    #[test]
    fn lit_follows_the_deck() {
        let mut s = Snapshot { track_frames: 1000, ..Snapshot::default() };
        assert_eq!(lit(Action::Cue, &s), Some(true), "parked on the cue");
        assert_eq!(lit(Action::PlayPause, &s), Some(false));
        s.playing = true;
        s.pos = 500.0;
        assert_eq!((lit(Action::PlayPause, &s), lit(Action::Cue, &s)), (Some(true), Some(false)));
        s.loop_active = true;
        s.loop_beats = 8.0;
        assert_eq!((lit(Action::BeatLoop8, &s), lit(Action::BeatLoop4, &s)), (Some(true), Some(false)));
        s.hot_cues[2] = Some(crate::track::HotCue { start: 0.0, end: None });
        assert_eq!((lit(Action::HotCue3, &s), lit(Action::HotCue1, &s)), (Some(true), Some(false)));
        assert_eq!(lit(Action::BeatJumpBack, &s), None);
    }
}
