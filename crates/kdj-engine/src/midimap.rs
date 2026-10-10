//! MIDI message → action, configured in TOML. Pure: the app hands it raw
//! bytes from the MIDI thread, so the mapping is testable without hardware.
//!
//! ```toml
//! [midi]
//! port = "XONE:K3"          # part of the input port's name
//!
//! [[midi.map]]               # a pad or button: press and release
//! note = 36
//! action = "PlayPause"
//!
//! [[midi.map]]               # an endless encoder: one action per tick
//! cc = 0
//! up = "JogForward"
//! down = "JogBack"
//!
//! [[midi.map]]               # a fader (absolute CC): the tempo fader
//! cc = 4
//! fader = "Tempo"
//!
//! [[midi.map]]               # channel (1–16) and deck (0 = A, 1 = B) are optional
//! channel = 2
//! note = 36
//! action = "Cue"
//! deck = 1
//! ```

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::action::{Action, Edge};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MidiConfig {
    /// Part of the input port name to connect to; the first port if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(default)]
    pub map: Vec<Binding>,
    /// Added to a button's note to address its LED (Xone:K2/K3: 0 red, 36
    /// amber, 72 green). See `feedback`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_offset: Option<u8>,
    /// Light the controller's LEDs with the deck's state (on unless false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leds: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// 1–16; any channel if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<u8>,
    /// For buttons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    /// For encoders: per clockwise / anticlockwise tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down: Option<Action>,
    /// For faders (absolute CCs): what it controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fader: Option<FaderKind>,
    /// A fader that's faster pushed away from you (up) rather than pulled
    /// towards you, as on a CDJ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// How the encoder encodes direction; two's complement if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<Encoding>,
    /// 0 = deck A, 1 = deck B; the focused deck if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deck: Option<usize>,
    /// The note that lights this button's LED, if not its own note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<u8>,
}

/// What a fader controls.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub enum FaderKind {
    /// Tempo across the deck's range, with soft takeover.
    Tempo,
}

/// How relative encoders say which way they turned.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    /// 1..63 clockwise, 127..65 anticlockwise (Xone:K2/K3 and most others).
    #[default]
    Twos,
    /// 65..127 clockwise, 63..1 anticlockwise, around 64.
    Offset,
}

/// Which deck a control drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Focused,
    Deck(usize),
}

/// One decoded control event: `repeat` presses for an encoder turn of
/// several ticks, otherwise one press or release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub action: Action,
    pub edge: Edge,
    pub target: Target,
    pub repeat: u8,
}

/// What a MIDI message means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Hit(Hit),
    /// A fader moved: `position` from -1 (slowest, for tempo) to 1.
    Fader {
        kind: FaderKind,
        position: f64,
        target: Target,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Source {
    Note(u8),
    Cc(u8),
}

#[derive(Clone, Copy, Debug)]
enum Control {
    Button(Action),
    Encoder { up: Action, down: Action, encoding: Encoding },
    Fader { kind: FaderKind, invert: bool },
}

/// (0-based channel, or None for any channel; which note or CC).
type Key = (Option<u8>, Source);

#[derive(Debug, Default)]
pub struct MidiMap {
    controls: HashMap<Key, (Control, Target)>,
}

impl MidiMap {
    /// # Errors
    /// If a binding names neither or both of note/cc, has no action, or
    /// has an out-of-range channel.
    pub fn new(cfg: &MidiConfig) -> Result<Self> {
        let mut controls = HashMap::new();
        for (i, b) in cfg.map.iter().enumerate() {
            let source = match (b.note, b.cc) {
                (Some(n), None) => Source::Note(n),
                (None, Some(c)) => Source::Cc(c),
                _ => bail!("midi mapping {}: give exactly one of note or cc", i + 1),
            };
            let control = match (b.action, b.up, b.down, b.fader) {
                (Some(a), None, None, None) => Control::Button(a),
                (None, Some(up), Some(down), None) => {
                    Control::Encoder { up, down, encoding: b.encoding.unwrap_or_default() }
                }
                (None, None, None, Some(kind)) if matches!(source, Source::Cc(_)) => {
                    Control::Fader { kind, invert: b.invert.unwrap_or(false) }
                }
                _ => bail!("midi mapping {}: give one of action, up and down, or fader (on a cc)", i + 1),
            };
            let channel = match b.channel {
                None => None,
                Some(c @ 1..=16) => Some(c - 1),
                Some(c) => bail!("midi mapping {}: channel {c} isn't 1–16", i + 1),
            };
            let target = b.deck.map_or(Target::Focused, Target::Deck);
            controls.insert((channel, source), (control, target));
        }
        Ok(Self { controls })
    }

    /// Parse a `[midi]` table out of a TOML config file's text.
    ///
    /// # Errors
    /// If the TOML doesn't parse or a mapping is invalid.
    pub fn from_toml(src: &str) -> Result<(Self, MidiConfig)> {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            midi: MidiConfig,
        }
        let file: File = toml::from_str(src).context("parsing midi config")?;
        Ok((Self::new(&file.midi)?, file.midi))
    }

    /// Decode one MIDI message.
    #[must_use]
    pub fn decode(&self, msg: &[u8]) -> Option<Event> {
        let (&status, data) = msg.split_first()?;
        let channel = status & 0x0f;
        let (source, value) = match (status & 0xf0, data) {
            (0x90, &[note, vel, ..]) => (Source::Note(note), vel),
            (0x80, &[note, ..]) => (Source::Note(note), 0),
            (0xb0, &[cc, val, ..]) => (Source::Cc(cc), val),
            _ => return None,
        };
        let &(control, target) =
            self.controls.get(&(Some(channel), source)).or_else(|| self.controls.get(&(None, source)))?;
        match control {
            // Notes: velocity 0 is a release. CCs as buttons: 64 and up is pressed.
            Control::Button(action) => {
                let pressed = if matches!(source, Source::Note(_)) { value > 0 } else { value >= 64 };
                let edge = if pressed { Edge::Press } else { Edge::Release };
                Some(Event::Hit(Hit { action, edge, target, repeat: 1 }))
            }
            Control::Fader { kind, invert } => Some(Event::Fader { kind, position: fader(value, invert), target }),
            Control::Encoder { up, down, encoding } => {
                let ticks = ticks(value, encoding);
                if ticks == 0 {
                    return None;
                }
                let action = if ticks > 0 { up } else { down };
                Some(Event::Hit(Hit { action, edge: Edge::Press, target, repeat: ticks.unsigned_abs() }))
            }
        }
    }
}

/// A 7-bit fader value as -1..1. Like a CDJ, pulled towards you (0) is
/// fastest unless inverted. The middle two values are exactly 0, a detent
/// where 7 bits have no centre.
fn fader(value: u8, invert: bool) -> f64 {
    let v = f64::from(value & 0x7f);
    let centred = if (63.0..=64.0).contains(&v) { 0.0 } else { (63.5 - v) / 63.5 };
    if invert { -centred } else { centred }
}

/// Signed tick count from a relative encoder's value.
const fn ticks(value: u8, encoding: Encoding) -> i8 {
    let v = value & 0x7f;
    match encoding {
        // 7-bit two's complement (1..63 up, 127..65 down): sign-extend bit 6.
        Encoding::Twos => (v << 1).cast_signed() >> 1,
        Encoding::Offset => v.cast_signed() - 64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
        [midi]
        port = "XONE:K3"

        [[midi.map]]
        note = 36
        action = "PlayPause"

        [[midi.map]]
        cc = 0
        up = "JogForward"
        down = "JogBack"

        [[midi.map]]
        cc = 1
        up = "TempoUp"
        down = "TempoDown"
        encoding = "offset"

        [[midi.map]]
        channel = 2
        note = 36
        action = "Cue"
        deck = 1

        [[midi.map]]
        cc = 20
        action = "LoopToggle"
    "#;

    fn hit(m: &MidiMap, msg: &[u8]) -> Option<Hit> {
        match m.decode(msg)? {
            Event::Hit(h) => Some(h),
            Event::Fader { .. } => None,
        }
    }

    fn map() -> MidiMap {
        MidiMap::from_toml(CONFIG).expect("valid config").0
    }

    #[test]
    fn notes_press_and_release() {
        let m = map();
        let on = hit(&m, &[0x90, 36, 100]).expect("mapped");
        assert_eq!((on.action, on.edge, on.target), (Action::PlayPause, Edge::Press, Target::Focused));
        assert_eq!(hit(&m, &[0x90, 36, 0]).map(|h| h.edge), Some(Edge::Release), "velocity 0 is a release");
        assert_eq!(hit(&m, &[0x80, 36, 64]).map(|h| h.edge), Some(Edge::Release));
        assert_eq!(hit(&m, &[0x90, 37, 100]), None, "unmapped");
    }

    #[test]
    fn a_channel_specific_binding_wins_and_targets_its_deck() {
        let m = map();
        let h = hit(&m, &[0x91, 36, 100]).expect("channel 2");
        assert_eq!((h.action, h.target), (Action::Cue, Target::Deck(1)));
        assert_eq!(hit(&m, &[0x92, 36, 100]).map(|h| h.action), Some(Action::PlayPause), "other channels fall back");
    }

    #[test]
    fn encoders_tick_both_ways_in_both_encodings() {
        let m = map();
        let cw = hit(&m, &[0xb0, 0, 1]).expect("tick");
        assert_eq!((cw.action, cw.repeat), (Action::JogForward, 1));
        let ccw = hit(&m, &[0xb0, 0, 127]).expect("tick");
        assert_eq!((ccw.action, ccw.repeat), (Action::JogBack, 1));
        let fast = hit(&m, &[0xb0, 0, 125]).expect("three ticks back");
        assert_eq!((fast.action, fast.repeat), (Action::JogBack, 3));
        let up = hit(&m, &[0xb0, 1, 66]).expect("offset encoding");
        assert_eq!((up.action, up.repeat), (Action::TempoUp, 2));
        assert_eq!(hit(&m, &[0xb0, 1, 63]).map(|h| h.action), Some(Action::TempoDown));
        assert_eq!(hit(&m, &[0xb0, 1, 64]), None, "no movement");
    }

    #[test]
    fn a_cc_button_presses_at_64_and_up() {
        let m = map();
        assert_eq!(hit(&m, &[0xb0, 20, 127]).map(|h| h.edge), Some(Edge::Press));
        assert_eq!(hit(&m, &[0xb0, 20, 0]).map(|h| h.edge), Some(Edge::Release));
    }

    #[test]
    fn the_example_mapping_parses() {
        let (map, cfg) = MidiMap::from_toml(include_str!("../../../config/midi-example.toml")).expect("valid");
        assert_eq!(cfg.port.as_deref(), Some("XONE:K3"));
        assert_eq!(hit(&map, &[0x90, 36, 127]).map(|h| h.action), Some(Action::PlayPause));
    }

    #[test]
    fn a_fader_runs_fastest_pulled_towards_you_with_a_centre_detent() {
        let src =
            "[[midi.map]]\ncc = 4\nfader = \"Tempo\"\n[[midi.map]]\ncc = 5\nfader = \"Tempo\"\ninvert = true\ndeck = 1";
        let (m, _) = MidiMap::from_toml(src).expect("valid");
        let pos = |msg: &[u8]| match m.decode(msg) {
            Some(Event::Fader { kind: FaderKind::Tempo, position, .. }) => position,
            other => panic!("not a tempo fader: {other:?}"),
        };
        assert!((pos(&[0xb0, 4, 0]) - 1.0).abs() < 1e-12, "bottom: fastest");
        assert!((pos(&[0xb0, 4, 127]) + 1.0).abs() < 1e-12);
        assert!(pos(&[0xb0, 4, 63]).abs() < 1e-12 && pos(&[0xb0, 4, 64]).abs() < 1e-12, "centre detent");
        assert!((pos(&[0xb0, 5, 127]) - 1.0).abs() < 1e-12, "inverted: top is fastest");
        assert!(matches!(m.decode(&[0xb0, 5, 0]), Some(Event::Fader { target: Target::Deck(1), .. })));
    }

    #[test]
    fn the_xone_k3_mapping_matches_the_factory_map() {
        let (m, cfg) = MidiMap::from_toml(include_str!("../../../config/xone-k3.toml")).expect("valid");
        assert_eq!(cfg.port.as_deref(), Some("XONE:K3"));
        // Channel 15 is status nibble 14. Grid bottom row: CUE, PLAY.
        assert_eq!(hit(&m, &[0x9e, 24, 127]).map(|h| h.action), Some(Action::Cue));
        assert_eq!(hit(&m, &[0x9e, 25, 127]).map(|h| h.action), Some(Action::PlayPause));
        assert_eq!(hit(&m, &[0x9e, 36, 127]).map(|h| h.action), Some(Action::HotCue1));
        // Top-left encoder one click clockwise (two's complement 1): jog.
        assert_eq!(hit(&m, &[0xbe, 0, 1]).map(|h| h.action), Some(Action::JogForward));
        assert!(matches!(m.decode(&[0xbe, 16, 0]), Some(Event::Fader { kind: FaderKind::Tempo, .. })));
        // No control is mapped twice.
        let mut seen = std::collections::HashSet::new();
        for b in &cfg.map {
            assert!(seen.insert((b.channel, b.note, b.cc)), "mapped twice: {b:?}");
        }
    }

    #[test]
    fn bad_bindings_are_rejected() {
        assert!(MidiMap::from_toml("[[midi.map]]\nnote = 1\nfader = \"Tempo\"").is_err(), "a fader is a cc");
        assert!(MidiMap::from_toml("[[midi.map]]\nnote = 1\ncc = 2\naction = \"Cue\"").is_err());
        assert!(MidiMap::from_toml("[[midi.map]]\nnote = 1\nup = \"TempoUp\"").is_err());
        assert!(MidiMap::from_toml("[[midi.map]]\nchannel = 17\nnote = 1\naction = \"Cue\"").is_err());
    }
}
