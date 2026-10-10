//! Lights the controller's LEDs with the decks' state (see
//! `kdj_engine::feedback`). Runs on the UI thread once a frame: only LEDs
//! whose state changed are sent, and everything is re-sent after the
//! controller (re)connects, so it never shows stale lights.
//!
//! With other kdjs on the network (see `net`), each lights only its own
//! decks' buttons: on the controller if it's plugged in here, else across
//! the network to the kdj it is plugged into, which passes them on.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use kdj_engine::Snapshot;
use kdj_engine::feedback::{self, Led};
use kdj_engine::midimap::{MidiConfig, Target};
use midir::{MidiOutput, MidiOutputConnection};

use crate::midi::base_name;
use crate::net::Net;

/// How often to look for the controller's output port.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// How often to send every LED again across the network, in case some were
/// lost on the way (it doesn't say).
const RESEND_EVERY: Duration = Duration::from_secs(2);

pub struct Leds {
    wanted: Option<String>,
    lights: Vec<Led>,
    /// What each LED was last set to; None when unknown (just connected).
    sent: Vec<Option<bool>>,
    out: Option<(String, MidiOutputConnection)>,
    looked: Option<Instant>,
    net: Option<Arc<Net>>,
    /// Other kdjs' LEDs, for the controller plugged in here.
    others: mpsc::Receiver<Vec<u8>>,
    /// The network's `epoch` when everything was last sent.
    epoch: u64,
    resent: Instant,
}

impl Leds {
    /// None if the mapping has no LEDs to light, or turns them off.
    pub fn new(cfg: &MidiConfig, net: Option<Arc<Net>>) -> Option<Self> {
        let lights = feedback::leds(cfg);
        if cfg.leds == Some(false) || lights.is_empty() {
            return None;
        }
        let (tx, others) = mpsc::channel();
        if let Some(n) = &net {
            n.on_led(tx);
        }
        let sent = vec![None; lights.len()];
        let epoch = net.as_ref().map_or(0, |n| n.epoch());
        Some(Self {
            wanted: cfg.port.clone(),
            lights,
            sent,
            out: None,
            looked: None,
            net,
            others,
            epoch,
            resent: Instant::now(),
        })
    }

    /// Bring the LEDs up to date with this kdj's decks. `focus`: the deck
    /// that unassigned controls drive.
    pub fn update(&mut self, decks: &[Snapshot], focus: usize) {
        self.keep_connected();
        self.pass_on_others();
        let away = self.out.is_none() && self.net.as_ref().is_some_and(|n| n.controller_elsewhere());
        if let Some(n) = &self.net {
            // Decks changed hands, or the controller moved: say everything again.
            let epoch = n.epoch();
            if epoch != self.epoch || (away && self.resent.elapsed() >= RESEND_EVERY) {
                self.epoch = epoch;
                self.resent = Instant::now();
                self.sent.fill(None);
            }
        }
        if self.out.is_none() && !away {
            return;
        }
        let here = self.net.as_ref().map_or(0..decks.len(), |n| n.decks());
        for (led, sent) in self.lights.iter().zip(&mut self.sent) {
            let deck = match led.target {
                Target::Focused => focus,
                Target::Deck(d) if here.contains(&d) => d - here.start,
                // Another kdj's: it lights them.
                Target::Deck(_) => continue,
            };
            let on = decks.get(deck).and_then(|s| feedback::lit(led.action, s)).unwrap_or(false);
            if *sent == Some(on) {
                continue;
            }
            let msg = led.message(on);
            match (&mut self.out, &self.net) {
                (Some((_, conn)), _) => {
                    if conn.send(&msg).is_err() {
                        // Gone: look again next time round.
                        self.out = None;
                        return;
                    }
                }
                (None, Some(n)) => n.send_led(&msg),
                (None, None) => return,
            }
            *sent = Some(on);
        }
    }

    /// Other kdjs' LEDs onto the controller plugged in here.
    fn pass_on_others(&mut self) {
        for msg in self.others.try_iter() {
            if let Some((_, conn)) = &mut self.out
                && conn.send(&msg).is_err()
            {
                self.out = None;
            }
        }
    }

    /// Connect to the controller's output port when it appears, and again if
    /// it's been replugged (the port changed).
    fn keep_connected(&mut self) {
        if self.looked.is_some_and(|t| t.elapsed() < LOOK_EVERY) {
            return;
        }
        self.looked = Some(Instant::now());
        let Ok(output) = MidiOutput::new("kdj") else { return };
        let wanted = self.wanted.as_deref().map(|w| base_name(w).to_lowercase());
        let port = output.ports().into_iter().find(|p| {
            output.port_name(p).is_ok_and(|n| {
                let base = base_name(&n).to_lowercase();
                wanted.as_ref().map_or_else(|| !base.contains("midi through"), |w| base.contains(w.as_str()))
            })
        });
        let Some(port) = port else {
            self.out = None;
            return;
        };
        let id = port.id();
        if self.out.as_ref().is_some_and(|(current, _)| *current == id) {
            return;
        }
        let name = output.port_name(&port).unwrap_or_default();
        match output.connect(&port, "kdj-leds") {
            Ok(conn) => {
                eprintln!("midi: lighting LEDs on {name}");
                self.out = Some((id, conn));
                self.sent.fill(None);
            }
            Err(e) => eprintln!("midi: couldn't open {name} for LEDs: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use anyhow::{Result, anyhow};
    use kdj_engine::midimap::MidiMap;
    use midir::MidiInput;
    use midir::os::unix::VirtualInput;

    use super::*;

    /// Through the OS's MIDI system: a virtual "controller" input receives
    /// the LED messages as the deck's state changes.
    #[test]
    #[ignore = "uses the OS MIDI system (CoreMIDI / ALSA)"]
    fn a_virtual_controller_sees_its_leds_follow_the_deck() -> Result<()> {
        let name = format!("kdj-test-leds-{}", std::process::id());
        let (tx, rx) = mpsc::channel();
        let _controller = MidiInput::new("kdj-test")?
            .create_virtual(&name, move |_, msg, ()| drop(tx.send(msg.to_vec())), ())
            .map_err(|e| anyhow!("{e}"))?;
        let src = format!("[midi]\nport = \"{name}\"\n[[midi.map]]\nnote = 36\naction = \"PlayPause\"\n");
        let (_, cfg) = MidiMap::from_toml(&src)?;
        let mut leds = Leds::new(&cfg, None).ok_or_else(|| anyhow!("no LEDs"))?;

        let stopped = Snapshot::default();
        let playing = Snapshot { playing: true, ..Snapshot::default() };
        leds.update(&[stopped], 0);
        leds.update(&[stopped], 0); // unchanged: nothing more sent
        leds.update(&[playing], 0);
        let got: Vec<_> = (0..2).filter_map(|_| rx.recv_timeout(Duration::from_secs(2)).ok()).collect();
        assert_eq!(got, [vec![0x90, 36, 0], vec![0x90, 36, 127]]);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "nothing else was sent");
        Ok(())
    }
}
