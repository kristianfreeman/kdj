//! Controllers as drawings: what a MIDI device's controls are and where they
//! sit, joined with a mapping to say what each one does in kdj.
//!
//! Devices are data (`config/devices/*.toml`, compiled in): see
//! `config/devices/xone-k3.toml` for the format. A mapping finds its device
//! by its `port`. [`Diagram`] is the join; [`Diagram::shapes`] lays it out
//! as plain shapes that the screen and the SVG for docs both draw, so the
//! two can't disagree.

mod layout;
mod svg;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::action::Action;
use crate::midimap::{Binding, FaderKind, MidiConfig, Target};

pub use layout::{Anchor, CANVAS, Paint, Shape, TextSize};

/// The devices kdj knows the shape of.
const BUILT_IN: &[&str] = &[include_str!("../../../../config/devices/xone-k3.toml")];

/// What a control physically is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Endless rotary encoder; may also push like a button.
    Encoder,
    /// Rotary pot with end stops.
    Pot,
    Button,
    /// A velocity pad: drawn as a button.
    Pad,
    Fader,
}

/// Which message a control sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Address {
    Note(u8),
    Cc(u8),
}

/// One physical control.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub kind: Kind,
    /// x, y, w, h in the device's units.
    pub rect: [f32; 4],
    /// MIDI channel, 1–16.
    pub channel: u8,
    pub address: Address,
    /// An encoder's push note.
    pub push: Option<u8>,
    /// What's printed on it (LAYER, SHIFT), if anything.
    pub name: Option<String>,
}

/// A controller's layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub maker: Option<String>,
    /// Parts of MIDI port names this device shows up as.
    pub ports: Vec<String>,
    /// Width and height in units.
    pub size: [f32; 2],
    pub controls: Vec<Control>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    device: Header,
    #[serde(default)]
    row: Vec<RowSpec>,
    #[serde(default)]
    control: Vec<ControlSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    name: String,
    maker: Option<String>,
    #[serde(default)]
    ports: Vec<String>,
    /// The default channel for every control.
    channel: u8,
    size: [f32; 2],
}

/// One control per column: lists of notes or CCs, left to right.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RowSpec {
    kind: Kind,
    /// Each control's x; 0, 1, 2… if omitted.
    x: Option<Vec<f32>>,
    y: f32,
    #[serde(default = "one")]
    w: f32,
    #[serde(default = "one")]
    h: f32,
    channel: Option<u8>,
    note: Option<Vec<u8>>,
    cc: Option<Vec<u8>>,
    push: Option<Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlSpec {
    kind: Kind,
    name: Option<String>,
    x: f32,
    y: f32,
    #[serde(default = "one")]
    w: f32,
    #[serde(default = "one")]
    h: f32,
    channel: Option<u8>,
    note: Option<u8>,
    cc: Option<u8>,
    push: Option<u8>,
}

const fn one() -> f32 {
    1.0
}

impl Device {
    /// Parse a device description.
    ///
    /// # Errors
    /// If the TOML doesn't parse, a control has neither or both of note and
    /// cc, a row's lists differ in length, or a channel isn't 1–16.
    pub fn from_toml(src: &str) -> Result<Self> {
        let file: File = toml::from_str(src).context("parsing device description")?;
        let h = file.device;
        let channel = |c: Option<u8>| -> Result<u8> {
            let c = c.unwrap_or(h.channel);
            ensure!((1..=16).contains(&c), "{}: channel {c} isn't 1–16", h.name);
            Ok(c)
        };
        let mut controls = Vec::new();
        for (i, r) in file.row.iter().enumerate() {
            let addresses: Vec<Address> = match (&r.note, &r.cc) {
                (Some(n), None) => n.iter().map(|&n| Address::Note(n)).collect(),
                (None, Some(c)) => c.iter().map(|&c| Address::Cc(c)).collect(),
                _ => bail!("{} row {}: give one of note or cc", h.name, i + 1),
            };
            let xs = r.x.clone().unwrap_or_else(|| (0..addresses.len()).map(units).collect());
            let same = |len: Option<usize>| len.is_none_or(|l| l == addresses.len());
            ensure!(
                xs.len() == addresses.len() && same(r.push.as_ref().map(Vec::len)),
                "{} row {}: x, note/cc and push need one entry per control",
                h.name,
                i + 1
            );
            for (k, (&address, x)) in addresses.iter().zip(xs).enumerate() {
                controls.push(Control {
                    kind: r.kind,
                    rect: [x, r.y, r.w, r.h],
                    channel: channel(r.channel)?,
                    address,
                    push: r.push.as_ref().map(|p| p[k]),
                    name: None,
                });
            }
        }
        for (i, c) in file.control.into_iter().enumerate() {
            let address = match (c.note, c.cc) {
                (Some(n), None) => Address::Note(n),
                (None, Some(c)) => Address::Cc(c),
                _ => bail!("{} control {}: give one of note or cc", h.name, i + 1),
            };
            controls.push(Control {
                kind: c.kind,
                rect: [c.x, c.y, c.w, c.h],
                channel: channel(c.channel)?,
                address,
                push: c.push,
                name: c.name,
            });
        }
        Ok(Self { name: h.name, maker: h.maker, ports: h.ports, size: h.size, controls })
    }

    /// The built-in devices.
    ///
    /// # Panics
    /// If a built-in description doesn't parse; they're compiled in and
    /// tested, so this can't happen in a working build.
    #[must_use]
    #[expect(clippy::expect_used, reason = "built-in descriptions are compiled in and checked by the tests")]
    pub fn built_in() -> Vec<Self> {
        BUILT_IN.iter().map(|src| Self::from_toml(src).expect("built-in device description")).collect()
    }

    /// The built-in device for a mapping, by its port name.
    #[must_use]
    pub fn for_mapping(cfg: &MidiConfig) -> Option<Self> {
        let port = cfg.port.as_deref()?.to_uppercase();
        Self::built_in()
            .into_iter()
            .find(|d| d.ports.iter().map(|p| p.to_uppercase()).any(|p| port.contains(&p) || p.contains(&port)))
    }

    /// A built-in device by name or port ("xone:k3", "Xone:K3").
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        let want = name.to_uppercase();
        Self::built_in()
            .into_iter()
            .find(|d| d.name.to_uppercase() == want || d.ports.iter().any(|p| p.to_uppercase() == want))
    }
}

/// What one way of using a control does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// Short and uppercase: PLAY, JOG, LOOP ×2/÷2.
    pub label: String,
    /// The deck it drives.
    pub target: Target,
    /// Counts for every deck (SHIFT), so no one deck's legend.
    pub both: bool,
}

/// A control and what it does; both empty if it's unmapped.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub control: Control,
    /// Pressing a button, turning an encoder or moving a fader.
    pub main: Option<Part>,
    /// Pushing an encoder.
    pub push: Option<Part>,
}

/// A binding that isn't on the drawing, for the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Where it comes from: "N24", "CC16", "ch2 N36".
    pub address: String,
    pub part: Part,
}

/// A mapping drawn on its device, or as a table if kdj doesn't know the
/// device's layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagram {
    /// The device's name, or the mapping's port.
    pub title: String,
    pub device: Option<Device>,
    /// Every control on the device, mapped or not.
    pub controls: Vec<Placed>,
    /// Bindings for controls the device doesn't have (all of them without a device).
    pub unplaced: Vec<Entry>,
}

impl Diagram {
    /// `cfg` on the built-in device for its port, if there is one.
    #[must_use]
    pub fn new(cfg: &MidiConfig) -> Self {
        Self::on(Device::for_mapping(cfg), cfg)
    }

    /// `cfg` on `device` (or as a table, without one).
    #[must_use]
    pub fn on(device: Option<Device>, cfg: &MidiConfig) -> Self {
        let mut controls: Vec<Placed> = device.iter().flat_map(|d| &d.controls).map(Placed::unmapped).collect();
        let mut unplaced = Vec::new();
        // Any-channel bindings first, so a channel's own wins as in `MidiMap`;
        // otherwise a later binding replaces an earlier one, as there too.
        let (any, specific): (Vec<&Binding>, Vec<&Binding>) = cfg.map.iter().partition(|b| b.channel.is_none());
        for b in any.into_iter().chain(specific) {
            let Some(part) = part(b) else { continue };
            let mut placed = false;
            for c in &mut controls {
                if let Some(slot) = c.slot(b) {
                    *slot = Some(part.clone());
                    placed = true;
                }
            }
            if !placed {
                unplaced.retain(|e: &Entry| e.address != address(b));
                unplaced.push(Entry { address: address(b), part });
            }
        }
        let title = device
            .as_ref()
            .map(|d| d.name.clone())
            .or_else(|| cfg.port.clone())
            .unwrap_or_else(|| "MIDI mapping".to_owned());
        Self { title, device, controls, unplaced }
    }

    /// The control that sends `address` (on any channel).
    #[must_use]
    pub fn control(&self, address: Address) -> Option<&Placed> {
        self.controls.iter().find(|p| p.control.address == address)
    }
}

impl Placed {
    fn unmapped(c: &Control) -> Self {
        Self { control: c.clone(), main: None, push: None }
    }

    /// Where `b` lands on this control, if it does.
    fn slot(&mut self, b: &Binding) -> Option<&mut Option<Part>> {
        let c = &self.control;
        if b.channel.is_some_and(|ch| ch != c.channel) {
            return None;
        }
        match (b.note, b.cc) {
            (Some(n), None) if c.address == Address::Note(n) => Some(&mut self.main),
            (Some(n), None) if c.push == Some(n) => Some(&mut self.push),
            (None, Some(n)) if c.address == Address::Cc(n) => Some(&mut self.main),
            _ => None,
        }
    }
}

/// What a binding does, or None if it's malformed (`MidiMap` rejects those).
fn part(b: &Binding) -> Option<Part> {
    let label = match (b.action, b.up, b.down, b.fader) {
        _ if b.modifier.is_some() => "SHIFT".to_owned(),
        (Some(a), None, None, None) => a.label().to_owned(),
        (None, Some(up), Some(down), None) => Action::pair_label(up, down),
        (None, None, None, Some(FaderKind::Tempo)) => "TEMPO".to_owned(),
        (None, None, None, Some(FaderKind::FineTempo)) => "FINE TEMPO".to_owned(),
        _ => return None,
    };
    Some(Part { label, target: b.deck.map_or(Target::Focused, Target::Deck), both: b.modifier.is_some() })
}

fn address(b: &Binding) -> String {
    let what = match (b.note, b.cc) {
        (Some(n), _) => format!("N{n}"),
        (_, Some(c)) => format!("CC{c}"),
        _ => "?".to_owned(),
    };
    b.channel.map_or_else(|| what.clone(), |ch| format!("ch{ch} {what}"))
}

/// A small count as a unit coordinate.
#[expect(clippy::cast_precision_loss, reason = "a row has a handful of controls")]
const fn units(i: usize) -> f32 {
    i as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midimap::MidiMap;

    fn k3() -> Diagram {
        let (_, cfg) = MidiMap::from_toml(include_str!("../../../../config/xone-k3.toml")).expect("valid");
        Diagram::new(&cfg)
    }

    fn main(d: &Diagram, a: Address) -> Option<(&str, Target)> {
        d.control(a)?.main.as_ref().map(|p| (p.label.as_str(), p.target))
    }

    #[test]
    fn built_in_devices_parse_with_every_address_once() {
        for d in Device::built_in() {
            let mut seen = std::collections::HashSet::new();
            for c in &d.controls {
                let [x, y, w, h] = c.rect;
                assert!(x >= 0.0 && y >= 0.0 && x + w <= d.size[0] && y + h <= d.size[1] + 1e-3, "{c:?} off {d:?}");
                let push = c.push.map(Address::Note);
                for a in std::iter::once(c.address).chain(push) {
                    assert!(seen.insert(format!("{a:?}")), "{} sends {a:?} twice", d.name);
                }
            }
        }
        let k3 = Device::named("xone:k3").expect("built in");
        assert_eq!(k3.controls.len(), 4 * 12 + 4, "12 controls a column, 4 along the bottom");
    }

    #[test]
    fn the_k3_mapping_lands_on_the_k3_split_by_deck() {
        let d = k3();
        assert_eq!(d.title, "Xone:K3");
        assert!(d.unplaced.is_empty(), "every binding is on the drawing: {:?}", d.unplaced);
        // Grid bottom row: CUE, PLAY on the left for A, again in columns 3–4 for B.
        let cue_a = d.control(Address::Note(24)).expect("on the K3");
        let hot_a = d.control(Address::Note(36)).expect("on the K3");
        assert_eq!(cue_a.control.rect[0], 0.0, "column 1");
        assert!(cue_a.control.rect[1] > hot_a.control.rect[1], "below the hot cues");
        assert_eq!(main(&d, Address::Note(24)), Some(("CUE", Target::Deck(0))));
        assert_eq!(main(&d, Address::Note(26)), Some(("CUE", Target::Deck(1))));
        assert_eq!(d.control(Address::Note(26)).map(|p| p.control.rect[0]), Some(2.0), "column 3");
        assert_eq!(main(&d, Address::Note(36)), Some(("HOT A", Target::Deck(0))));
        // Encoders: the turn and the push.
        let tempo = d.control(Address::Cc(0)).expect("top left encoder");
        assert_eq!(tempo.main.as_ref().map(|p| p.label.as_str()), Some("TEMPO"));
        assert_eq!(tempo.push.as_ref().map(|p| p.label.as_str()), Some("TEMPO 0"));
        assert_eq!(main(&d, Address::Cc(3)), Some(("LOOP ×2/÷2", Target::Deck(1))));
        assert_eq!(main(&d, Address::Cc(16)), Some(("TEMPO", Target::Deck(0))));
        assert_eq!(main(&d, Address::Note(15)), Some(("SHIFT", Target::Focused)), "SHIFT");
        assert_eq!(main(&d, Address::Note(12)), Some(("SHIFT", Target::Focused)), "LAYER");
        // The bottom row is LAYER, the two encoders, SHIFT.
        let x = |a| d.control(a).map(|p| p.control.rect[0]);
        assert_eq!(
            [x(Address::Note(12)), x(Address::Cc(20)), x(Address::Cc(21)), x(Address::Note(15))],
            [0.0, 1.0, 2.0, 3.0].map(Some)
        );
        assert_eq!(main(&d, Address::Cc(19)), Some(("FINE TEMPO", Target::Deck(1))), "fader 2");
        // The pots do nothing.
        for cc in 4..=15 {
            assert_eq!(main(&d, Address::Cc(cc)), None, "CC {cc}");
        }
    }

    #[test]
    fn a_mapping_without_a_known_device_is_a_table() {
        let src = "[midi]\nport = \"nanoKONTROL2\"\n[[midi.map]]\nnote = 36\naction = \"PlayPause\"\n\
                   [[midi.map]]\nchannel = 2\ncc = 1\nup = \"TempoUp\"\ndown = \"TempoDown\"\ndeck = 1";
        let (_, cfg) = MidiMap::from_toml(src).expect("valid");
        let d = Diagram::new(&cfg);
        assert!(d.device.is_none() && d.controls.is_empty());
        assert_eq!(d.title, "nanoKONTROL2");
        let rows: Vec<_> =
            d.unplaced.iter().map(|e| (e.address.as_str(), e.part.label.as_str(), e.part.target)).collect();
        assert_eq!(rows, [("N36", "PLAY", Target::Focused), ("ch2 CC1", "TEMPO", Target::Deck(1))]);
    }

    #[test]
    fn a_channels_own_binding_wins_and_other_channels_miss() {
        let src = "[[midi.map]]\nchannel = 15\nnote = 24\naction = \"Cue\"\n\
                   [[midi.map]]\nnote = 24\naction = \"PlayPause\"\n\
                   [[midi.map]]\nchannel = 3\nnote = 25\naction = \"Sync\"";
        let (_, cfg) = MidiMap::from_toml(src).expect("valid");
        let d = Diagram::on(Device::named("XONE:K3"), &cfg);
        assert_eq!(main(&d, Address::Note(24)), Some(("CUE", Target::Focused)));
        assert_eq!(main(&d, Address::Note(25)), None, "channel 3 isn't the K3's");
        assert_eq!(d.unplaced.len(), 1);
    }
}
