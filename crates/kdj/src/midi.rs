//! MIDI controllers (an Xone:K3, or anything else).
//!
//! A background thread keeps the configured input port connected, and
//! reconnects within a second if it's unplugged or appears late (a Pi can
//! boot before the controller does). Port names are matched without ALSA's
//! trailing "client:port" numbers, which change between boots, and with no
//! port configured the first one that isn't ALSA's "Midi Through" is used.
//! Each message is decoded by the
//! `MidiMap`; deck actions go straight into that deck's engine queue without
//! waiting for a screen frame, and browser/screen actions go to the UI thread.
//!
//! `deck --midi-learn FILE` writes a mapping by asking for each control in turn.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use kdj_engine::midimap::{Binding, Encoding, Event, FaderKind, Hit, MidiConfig, MidiMap, Target};
use kdj_engine::{Action, Edge, InputSender};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use serde::Serialize;

/// A UI-side action from the controller: (action, edge, deck).
pub type UiAction = (Action, Edge, usize);

/// Everything a message needs to reach its destination.
struct Dispatch {
    map: MidiMap,
    /// For each mapped CC, (0-based channel or any, CC): is it an encoder?
    encoders: HashMap<(Option<u8>, u8), bool>,
    decks: Vec<InputSender>,
    ui: mpsc::Sender<UiAction>,
    focus: Arc<AtomicUsize>,
}

impl Dispatch {
    fn new(
        cfg: &MidiConfig,
        map: MidiMap,
        decks: Vec<InputSender>,
        ui: mpsc::Sender<UiAction>,
        focus: Arc<AtomicUsize>,
    ) -> Self {
        // Mirrors `MidiMap::new`: later bindings replace earlier ones.
        let encoders = cfg
            .map
            .iter()
            .filter_map(|b| Some(((b.channel.map(|c| c.saturating_sub(1)), b.cc?), b.up.is_some())))
            .collect();
        Self { map, encoders, decks, ui, focus }
    }

    /// Whether `msg` comes from an encoder (looked up the way `MidiMap`
    /// does: this channel's binding first, then an any-channel one).
    fn is_encoder(&self, msg: &[u8]) -> bool {
        let &[status, cc, ..] = msg else { return false };
        if status & 0xf0 != 0xb0 {
            return false;
        }
        let channel = status & 0x0f;
        self.encoders.get(&(Some(channel), cc)).or_else(|| self.encoders.get(&(None, cc))).copied().unwrap_or(false)
    }

    fn handle(&mut self, msg: &[u8]) {
        match self.map.decode(msg) {
            Some(Event::Hit(hit)) => self.hit(hit, self.is_encoder(msg)),
            Some(Event::Fader { kind: FaderKind::Tempo, position, target }) => {
                let deck = self.deck(target);
                if let Some(engine) = self.decks.get_mut(deck) {
                    engine.tempo_fader(position);
                }
            }
            None => {}
        }
    }

    /// Which deck a control drives.
    fn deck(&self, target: Target) -> usize {
        let last = self.decks.len().saturating_sub(1);
        match target {
            Target::Focused => self.focus.load(Ordering::Relaxed),
            Target::Deck(d) => d,
        }
        .min(last)
    }

    fn hit(&mut self, hit: Hit, encoder: bool) {
        let deck = self.deck(hit.target);
        for _ in 0..hit.repeat {
            if hit.action.is_ui() {
                let _ = self.ui.send((hit.action, hit.edge, deck));
            } else if let Some(engine) = self.decks.get_mut(deck) {
                engine.send(hit.action, hit.edge);
                // An encoder tick is a whole press: release it too, or a
                // hold action (nudge, cue) on an encoder would never let go.
                if encoder {
                    engine.send(hit.action, Edge::Release);
                }
            }
        }
    }
}

/// The running controller link. Dropping it stops nothing; it lives as long
/// as the app.
pub struct Midi {
    pub ui: mpsc::Receiver<UiAction>,
    /// The controller's LEDs, if the mapping has any to light.
    pub leds: Option<crate::leds::Leds>,
}

/// Start the controller thread.
pub fn start(cfg: &MidiConfig, map: MidiMap, decks: Vec<InputSender>, focus: Arc<AtomicUsize>) -> Midi {
    let (ui_tx, ui_rx) = mpsc::channel();
    let dispatch = Arc::new(Mutex::new(Dispatch::new(cfg, map, decks, ui_tx, focus)));
    let wanted = cfg.port.clone();
    std::thread::Builder::new()
        .name("midi".into())
        .spawn(move || keep_connected(wanted.as_deref(), &dispatch))
        .map_err(|e| eprintln!("midi: couldn't start: {e}"))
        .ok();
    Midi { ui: ui_rx, leds: crate::leds::Leds::new(cfg) }
}

/// A port as last seen: its full name (on ALSA ending in "client:port"),
/// midir's id for it, and how many ports there were. If any of these change
/// the connection may be dead (unplugged and replugged between two looks), so
/// it's made again.
#[derive(Clone, PartialEq, Eq)]
struct Seen {
    name: String,
    id: String,
    ports: usize,
}

struct Found {
    input: MidiInput,
    port: MidiInputPort,
    seen: Seen,
}

/// Connect when the port shows up; drop the connection when it goes away.
fn keep_connected(wanted: Option<&str>, dispatch: &Arc<Mutex<Dispatch>>) {
    let mut conn: Option<(Seen, MidiInputConnection<()>)> = None;
    let mut waiting = false;
    let mut last_error = None;
    loop {
        let found = match find_port(wanted) {
            Ok(f) => {
                last_error = None;
                f
            }
            Err(e) => {
                if last_error.as_ref() != Some(&e) {
                    eprintln!("midi: {e}");
                    last_error = Some(e);
                }
                None
            }
        };
        let unchanged = matches!((&conn, &found), (Some((seen, _)), Some(f)) if *seen == f.seen);
        if !unchanged {
            if let Some((seen, _)) = conn.take() {
                eprintln!(
                    "midi: {} {}",
                    base_name(&seen.name),
                    if found.is_some() { "changed, reconnecting" } else { "disconnected" }
                );
            }
            match found {
                Some(f) => {
                    conn = connect(f, dispatch);
                    waiting = false;
                }
                None if !waiting => {
                    eprintln!("midi: waiting for {}", wanted.unwrap_or("a controller"));
                    waiting = true;
                }
                None => {}
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn connect(found: Found, dispatch: &Arc<Mutex<Dispatch>>) -> Option<(Seen, MidiInputConnection<()>)> {
    let Found { input, port, seen } = found;
    let d = Arc::clone(dispatch);
    let callback = move |_stamp: u64, msg: &[u8], (): &mut ()| {
        d.lock().unwrap_or_else(PoisonError::into_inner).handle(msg);
    };
    match input.connect(&port, "kdj", callback, ()) {
        Ok(c) => {
            eprintln!("midi: connected to {}", seen.name);
            Some((seen, c))
        }
        Err(e) => {
            eprintln!("midi: couldn't connect to {}: {e}", seen.name);
            None
        }
    }
}

/// The first input port whose name contains `wanted` (ignoring case and
/// ALSA's client:port numbers); with no `wanted`, the first that isn't ALSA's
/// "Midi Through" loopback.
fn find_port(wanted: Option<&str>) -> Result<Option<Found>, String> {
    let input = MidiInput::new("kdj").map_err(|e| format!("can't open the MIDI system: {e}"))?;
    let ports = input.ports();
    let wanted = wanted.map(|w| base_name(w).to_lowercase());
    let hit = ports.iter().find_map(|p| {
        let name = input.port_name(p).ok()?;
        let base = base_name(&name).to_lowercase();
        let ok = wanted.as_ref().map_or_else(|| !base.contains("midi through"), |w| base.contains(w.as_str()));
        ok.then(|| (p.clone(), name))
    });
    Ok(hit.map(|(port, name)| {
        let seen = Seen { name, id: port.id(), ports: ports.len() };
        Found { input, port, seen }
    }))
}

/// A port name without ALSA's trailing "client:port" ("XONE:K3 MIDI 1 24:0"
/// is "XONE:K3 MIDI 1"), which changes between boots.
pub fn base_name(name: &str) -> &str {
    let name = name.trim_end();
    let Some((head, last)) = name.rsplit_once(char::is_whitespace) else { return name };
    let numbers = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match last.split_once(':') {
        Some((client, port)) if numbers(client) && numbers(port) => head.trim_end(),
        _ => name,
    }
}

/// What to ask for in learn mode: a button, or an encoder's two directions.
enum Ask {
    Button(Action, &'static str),
    Encoder(Action, Action, &'static str),
    Fader(FaderKind, &'static str),
}

const LEARN: &[Ask] = &[
    Ask::Button(Action::PlayPause, "PLAY / PAUSE"),
    Ask::Button(Action::Cue, "CUE"),
    Ask::Encoder(Action::JogForward, Action::JogBack, "JOG (nudge) encoder"),
    Ask::Button(Action::NudgeBack, "NUDGE BACK (hold)"),
    Ask::Button(Action::NudgeForward, "NUDGE FORWARD (hold)"),
    Ask::Encoder(Action::TempoUp, Action::TempoDown, "TEMPO encoder"),
    Ask::Fader(FaderKind::Tempo, "TEMPO fader"),
    Ask::Button(Action::TempoReset, "TEMPO RESET"),
    Ask::Button(Action::KeyLock, "KEY LOCK"),
    Ask::Button(Action::LoopToggle, "LOOP on/off"),
    Ask::Encoder(Action::LoopDouble, Action::LoopHalve, "LOOP SIZE encoder"),
    Ask::Button(Action::BeatJumpBack, "BEAT JUMP back"),
    Ask::Button(Action::BeatJumpForward, "BEAT JUMP forward"),
    Ask::Button(Action::HotCue1, "HOT CUE A"),
    Ask::Button(Action::HotCue2, "HOT CUE B"),
    Ask::Button(Action::HotCue3, "HOT CUE C"),
    Ask::Button(Action::HotCue4, "HOT CUE D"),
    Ask::Button(Action::HotCue5, "HOT CUE E"),
    Ask::Button(Action::HotCue6, "HOT CUE F"),
    Ask::Button(Action::HotCue7, "HOT CUE G"),
    Ask::Button(Action::HotCue8, "HOT CUE H"),
    Ask::Button(Action::Browse, "BROWSE (open / close)"),
    Ask::Encoder(Action::BrowseDown, Action::BrowseUp, "BROWSE scroll encoder"),
    Ask::Button(Action::BrowseLoad, "BROWSE LOAD / OPEN (encoder push)"),
    Ask::Button(Action::BrowseBack, "BROWSE BACK"),
    Ask::Button(Action::SwitchDeck, "SWITCH DECK (two-deck setups)"),
];

enum Heard {
    Midi(Vec<u8>),
    Enter,
}

/// Interactive: ask for each control, write the mapping to `out`.
///
/// # Errors
/// If no MIDI input is available or the file can't be written.
pub fn learn(out: &Path, wanted: Option<&str>) -> Result<()> {
    let Found { input, port, seen } =
        find_port(wanted).map_err(|e| anyhow!(e))?.ok_or_else(|| anyhow!("no MIDI input found{}", hint(wanted)))?;
    let name = seen.name;
    let (tx, rx) = mpsc::channel();
    let midi_tx = tx.clone();
    let _conn = input
        .connect(&port, "kdj-learn", move |_, msg, (): &mut ()| drop(midi_tx.send(Heard::Midi(msg.to_vec()))), ())
        .map_err(|e| anyhow!("connecting to {name}: {e}"))?;
    std::thread::spawn(move || {
        for _ in std::io::stdin().lock().lines() {
            if tx.send(Heard::Enter).is_err() {
                break;
            }
        }
    });
    println!("Learning from {name}. Touch each control when asked; press Enter to skip one.\n");
    let mut map = Vec::new();
    for ask in LEARN {
        if let Some(b) = learn_one(ask, &rx) {
            map.push(b);
        }
    }
    // Without the client:port numbers, which change between boots.
    let file = LearnFile { midi: MidiConfig { port: Some(base_name(&name).to_owned()), map, ..Default::default() } };
    let text = toml::to_string_pretty(&file).context("writing mapping")?;
    std::fs::write(out, text).with_context(|| format!("writing {}", out.display()))?;
    println!("\nWrote {} controls to {}. Run: deck --midi {}", file.midi.map.len(), out.display(), out.display());
    Ok(())
}

#[derive(Serialize)]
struct LearnFile {
    midi: MidiConfig,
}

fn learn_one(ask: &Ask, rx: &mpsc::Receiver<Heard>) -> Option<Binding> {
    let (label, prompt) = match ask {
        Ask::Button(_, l) => (*l, "press it"),
        Ask::Encoder(_, _, l) => (*l, "turn it one click clockwise"),
        Ask::Fader(_, l) => (*l, "move it from end to end, finishing at the FASTER end"),
    };
    println!("  {label}: {prompt}");
    if let Ask::Fader(kind, _) = ask {
        return learn_fader(*kind, rx);
    }
    let msg = next_control(rx)?;
    let (status, data) = msg.split_first()?;
    let channel = Some((status & 0x0f) + 1);
    let mut b = Binding { channel, ..Binding::default() };
    match (status & 0xf0, data) {
        (0x90, &[note, ..]) => b.note = Some(note),
        (0xb0, &[cc, value, ..]) => {
            b.cc = Some(cc);
            if matches!(ask, Ask::Encoder(..)) {
                // Clockwise reads 1..63 in two's complement, 65.. in offset.
                b.encoding = Some(if value >= 64 { Encoding::Offset } else { Encoding::Twos });
            }
        }
        _ => return None,
    }
    match ask {
        Ask::Button(a, _) => b.action = Some(*a),
        Ask::Encoder(up, down, _) => (b.up, b.down) = (Some(*up), Some(*down)),
        Ask::Fader(..) => {}
    }
    println!("    → {}", describe(&b));
    Some(b)
}

/// A fader: the CC it sends, and which end is faster from where it stopped
/// (the low end, towards you, as on a CDJ; or the high end: inverted).
fn learn_fader(kind: FaderKind, rx: &mpsc::Receiver<Heard>) -> Option<Binding> {
    let (status, cc, mut last) = loop {
        match rx.recv().ok()? {
            Heard::Enter => return None,
            Heard::Midi(m) => {
                if let &[s, cc, v, ..] = m.as_slice()
                    && s & 0xf0 == 0xb0
                {
                    break (s, cc, v);
                }
            }
        }
    };
    // Follow it until it rests.
    while let Ok(heard) = rx.recv_timeout(Duration::from_millis(700)) {
        if let Heard::Midi(m) = heard
            && let &[s, c, v, ..] = m.as_slice()
            && (s, c) == (status, cc)
        {
            last = v;
        }
    }
    let invert = last >= 64;
    let b = Binding {
        channel: Some((status & 0x0f) + 1),
        cc: Some(cc),
        fader: Some(kind),
        invert: invert.then_some(true),
        ..Binding::default()
    };
    println!("    → {}{}", describe(&b), if invert { ", faster at the top" } else { ", faster towards you" });
    Some(b)
}

/// The next note-on or CC, then a short pause to let the same control's
/// release and repeats go by. None if Enter was pressed (skip).
fn next_control(rx: &mpsc::Receiver<Heard>) -> Option<Vec<u8>> {
    loop {
        match rx.recv().ok()? {
            Heard::Enter => return None,
            Heard::Midi(m) if is_control(&m) => {
                let settle = Instant::now() + Duration::from_millis(400);
                while let Some(left) = settle.checked_duration_since(Instant::now()) {
                    if rx.recv_timeout(left).is_err() {
                        break;
                    }
                }
                return Some(m);
            }
            Heard::Midi(_) => {}
        }
    }
}

/// A note-on with velocity, or any CC.
fn is_control(m: &[u8]) -> bool {
    match m {
        [s, _, v, ..] if s & 0xf0 == 0x90 => *v > 0,
        [s, ..] => s & 0xf0 == 0xb0,
        [] => false,
    }
}

fn describe(b: &Binding) -> String {
    let what = b.note.map_or_else(|| format!("CC {}", b.cc.unwrap_or_default()), |n| format!("note {n}"));
    format!("{what} on channel {}", b.channel.unwrap_or(1))
}

fn hint(wanted: Option<&str>) -> String {
    wanted.map_or_else(String::new, |w| format!(" matching {w:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kdj_engine::{Engine, EngineConfig};
    use midir::MidiOutput;
    use midir::os::unix::VirtualOutput;

    #[test]
    fn alsa_client_port_numbers_are_dropped_from_port_names() {
        assert_eq!(base_name("XONE:K3 MIDI 1 24:0"), "XONE:K3 MIDI 1");
        assert_eq!(base_name("Midi Through Port-0 14:0 "), "Midi Through Port-0");
        assert_eq!(base_name("XONE:K3"), "XONE:K3");
        assert_eq!(base_name("XONE:K3 MIDI 1"), "XONE:K3 MIDI 1");
        assert_eq!(base_name("Deck 1:a"), "Deck 1:a");
    }

    #[test]
    fn encoders_are_told_from_buttons_like_the_map_does() -> Result<()> {
        let src = "[midi]\n\
            [[midi.map]]\ncc = 1\nup = \"NudgeForward\"\ndown = \"NudgeBack\"\n\
            [[midi.map]]\ncc = 2\naction = \"Cue\"\n\
            [[midi.map]]\ncc = 2\nchannel = 3\nup = \"TempoUp\"\ndown = \"TempoDown\"\n";
        let (map, cfg) = MidiMap::from_toml(src)?;
        let d = Dispatch::new(&cfg, map, Vec::new(), mpsc::channel().0, Arc::new(AtomicUsize::new(0)));
        assert!(d.is_encoder(&[0xb0, 1, 1]));
        assert!(!d.is_encoder(&[0xb0, 2, 127]));
        // Channel 3's own binding wins over the any-channel one.
        assert!(d.is_encoder(&[0xb2, 2, 1]));
        assert!(!d.is_encoder(&[0x90, 1, 100]));
        assert!(!d.is_encoder(&[0xb0, 9, 1]));
        Ok(())
    }

    /// End to end through the OS MIDI system: a virtual controller sends a
    /// note, the controller thread maps it and the engine sees the press.
    #[test]
    #[ignore = "needs the OS MIDI system (CoreMIDI or ALSA)"]
    fn a_note_from_a_virtual_controller_reaches_the_engine() -> Result<()> {
        let name = format!("kdj-test-{}", std::process::id());
        let mut controller = MidiOutput::new("kdj-test")?.create_virtual(&name).map_err(|e| anyhow!("{e}"))?;
        let cfg_src = format!("[midi]\nport = \"{name}\"\n[[midi.map]]\nnote = 36\naction = \"Quantize\"\n");
        let (map, cfg) = MidiMap::from_toml(&cfg_src)?;
        let (mut engine, mut handle, senders) =
            Engine::new(EngineConfig { extra_inputs: 1, ..EngineConfig::default() });
        let track = kdj_engine::analysis::build("t".into(), vec![0.0; 48_000 * 8], 48_000).0;
        handle.load(Arc::new(track)).map_err(|_| anyhow!("load refused"))?;
        let mut out = vec![0.0; 256];
        engine.process(&mut out);
        let before = handle.snapshot().quantize;
        let _midi = start(&cfg, map, senders, Arc::new(AtomicUsize::new(0)));

        // The controller thread polls for the port once a second.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            controller.send(&[0x90, 36, 100]).map_err(|e| anyhow!("{e}"))?;
            controller.send(&[0x80, 36, 0]).map_err(|e| anyhow!("{e}"))?;
            std::thread::sleep(Duration::from_millis(200));
            engine.process(&mut out);
            if handle.snapshot().quantize != before {
                return Ok(());
            }
        }
        Err(anyhow!("the note never reached the engine"))
    }
}
