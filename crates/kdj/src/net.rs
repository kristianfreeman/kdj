//! Decks on one network sharing a controller.
//!
//! Each kdj holds a deck letter (A, B…: `--deck`, or the first one free) and
//! says so to the others once a second. A controller plugged into any of them
//! plays them all: a mapping's `deck = 1` controls go to whichever kdj is
//! deck B, as the raw MIDI message, which that kdj decodes with its own copy
//! of the mapping; its LEDs come back the same way to the kdj the controller
//! is plugged into. So one Xone:K3 split down the middle plays two Pis, each
//! with its own output into its own mixer channel.
//!
//! Hellos go to a multicast group. Controls and LEDs go straight to the one
//! kdj that needs them (multicast on Wi-Fi can wait for the access point's
//! next beacon), and are only taken from kdjs that have said hello.
//!
//! Who holds a letter: a kdj starting up listens before it claims one, so it
//! never takes a letter that's in use. Two claiming the same letter (started
//! together, or set the same) settle it by the claim's stamp: picking a letter
//! in the settings outranks every claim seen so far; equal stamps go by name.
//! The other moves to the first letter free.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

/// The hello group and port.
const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 75, 74);
const PORT: u16 = 47_474;
/// Every packet starts with this (and a kind byte), so stray traffic on the
/// port is ignored, and a future incompatible version can be told apart.
const MAGIC: [u8; 4] = *b"kdj1";
const HELLO_EVERY: Duration = Duration::from_secs(1);
/// Silent this long: gone.
const PEER_TIMEOUT: Duration = Duration::from_millis(3500);
/// How long a kdj listens before claiming a letter (over a hello's interval,
/// so everyone already up has been heard).
const CLAIM_WAIT: Duration = Duration::from_millis(1500);
/// Letters A–D, as on a four-player CDJ setup.
pub const MAX_DECKS: usize = 4;
/// How long to wait before trying the network again after it failed.
const RETRY: Duration = Duration::from_secs(2);

/// What a kdj says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hello {
    name: String,
    /// Its first deck (0 = A) and how many it runs.
    first: u8,
    count: u8,
    /// Where to send it controls and LEDs.
    port: u16,
    /// Its claim's rank (see the module docs).
    stamp: u32,
    /// The controller is plugged into it.
    controller: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Packet<'a> {
    Hello(Hello),
    /// A controller message for the receiver's deck.
    Midi(&'a [u8]),
    /// An LED message for the receiver's controller.
    Led(&'a [u8]),
}

const HELLO: u8 = b'H';
const MIDI: u8 = b'M';
const LED: u8 = b'L';

fn encode_hello(h: &Hello) -> Vec<u8> {
    let mut b = Vec::with_capacity(MAGIC.len() + 10 + h.name.len());
    b.extend_from_slice(&MAGIC);
    b.extend_from_slice(&[HELLO, h.first, h.count, u8::from(h.controller)]);
    b.extend_from_slice(&h.port.to_be_bytes());
    b.extend_from_slice(&h.stamp.to_be_bytes());
    b.extend_from_slice(h.name.as_bytes());
    b
}

/// A MIDI or LED message; None if it's too long to be one.
fn encode(kind: u8, msg: &[u8]) -> Option<([u8; 8], usize)> {
    let mut b = [0; 8];
    let len = MAGIC.len() + 1 + msg.len();
    b.get_mut(MAGIC.len() + 1..len)?.copy_from_slice(msg);
    b[..MAGIC.len()].copy_from_slice(&MAGIC);
    b[MAGIC.len()] = kind;
    Some((b, len))
}

fn decode(b: &[u8]) -> Option<Packet<'_>> {
    let rest = b.strip_prefix(&MAGIC)?;
    let (&kind, rest) = rest.split_first()?;
    match kind {
        HELLO => {
            let (head, name) = rest.split_at_checked(9)?;
            let &[first, count, controller, p0, p1, s0, s1, s2, s3] = head else { return None };
            Some(Packet::Hello(Hello {
                name: std::str::from_utf8(name).ok()?.to_owned(),
                first,
                count,
                port: u16::from_be_bytes([p0, p1]),
                stamp: u32::from_be_bytes([s0, s1, s2, s3]),
                controller: controller != 0,
            }))
        }
        MIDI if !rest.is_empty() => Some(Packet::Midi(rest)),
        LED if !rest.is_empty() => Some(Packet::Led(rest)),
        _ => None,
    }
}

/// A claim on some decks, for settling who holds which.
#[derive(Clone, Copy, Debug)]
struct Claim<'a> {
    name: &'a str,
    port: u16,
    first: usize,
    count: usize,
    stamp: u32,
}

impl Claim<'_> {
    const fn decks(&self) -> Range<usize> {
        self.first..self.first + self.count
    }

    const fn overlaps(&self, other: &Claim) -> bool {
        self.first < other.first + other.count && other.first < self.first + self.count
    }

    /// Whether this claim keeps its decks against `other`'s.
    fn outranks(&self, other: &Claim) -> bool {
        match self.stamp.cmp(&other.stamp) {
            std::cmp::Ordering::Equal => (self.name, self.port) < (other.name, other.port),
            higher_or_lower => higher_or_lower.is_gt(),
        }
    }
}

/// Where `me` should move to, if it has to: when it shares a deck with a
/// claim that outranks it, the first decks nobody else holds (None to stay,
/// or if there are none).
fn settle(me: &Claim, others: &[Claim]) -> Option<usize> {
    if !others.iter().any(|o| me.overlaps(o) && o.outranks(me)) {
        return None;
    }
    first_free(me.count, others)
}

/// The first deck of `count` in a row that none of `others` holds.
fn first_free(count: usize, others: &[Claim]) -> Option<usize> {
    (0..=MAX_DECKS.checked_sub(count)?).find(|&first| {
        let c = Claim { name: "", port: 0, first, count, stamp: 0 };
        !others.iter().any(|o| c.overlaps(o))
    })
}

/// Another kdj on the network.
#[derive(Clone, Debug)]
struct Peer {
    hello: Hello,
    /// Where it sends from and takes controls and LEDs.
    addr: SocketAddr,
    seen: Instant,
}

impl Peer {
    fn claim(&self) -> Claim<'_> {
        let h = &self.hello;
        Claim { name: &h.name, port: h.port, first: h.first.into(), count: h.count.into(), stamp: h.stamp }
    }
}

/// Another kdj, for showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerInfo {
    pub name: String,
    pub decks: Range<usize>,
    /// The controller is plugged into it.
    pub controller: bool,
}

#[derive(Default)]
struct State {
    peers: Vec<Peer>,
    stamp: u32,
    /// The highest stamp seen, ours included.
    top_stamp: u32,
    /// Holding a letter (and saying so); not while still listening at start.
    claimed: bool,
}

/// Something to run on a controller message from another kdj.
type MidiSink = Box<dyn FnMut(&[u8]) + Send>;

/// This kdj's place on the network.
pub struct Net {
    name: String,
    count: usize,
    /// Sends controls, LEDs and hellos; takes controls and LEDs.
    data: UdpSocket,
    first: AtomicUsize,
    /// The controller is plugged in here.
    controller: AtomicBool,
    /// Changes whenever who holds which deck, or the controller, does.
    epoch: AtomicU64,
    state: Mutex<State>,
    midi_sink: Mutex<Option<MidiSink>>,
    led_sink: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
}

impl Net {
    /// Join the network as `name`, running `count` decks: from `deck` if
    /// given, else the first free once the others have been heard.
    ///
    /// # Errors
    /// If no UDP socket can be opened at all (the network itself coming and
    /// going is handled: it's tried again until it's there).
    pub fn start(name: String, count: usize, deck: Option<usize>) -> std::io::Result<Arc<Self>> {
        let data = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let receiver = data.try_clone()?;
        let net = Arc::new(Self {
            name,
            count: count.clamp(1, MAX_DECKS),
            data,
            first: AtomicUsize::new(deck.unwrap_or(0)),
            controller: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            state: Mutex::new(State { claimed: deck.is_some(), ..State::default() }),
            midi_sink: Mutex::new(None),
            led_sink: Mutex::new(None),
        });
        let n = Arc::clone(&net);
        std::thread::Builder::new().name("net-hello".into()).spawn(move || n.hellos())?;
        let n = Arc::clone(&net);
        std::thread::Builder::new().name("net".into()).spawn(move || n.receive(&receiver))?;
        Ok(net)
    }

    /// This kdj's decks (0 = A).
    pub fn decks(&self) -> Range<usize> {
        let first = self.first.load(Ordering::Relaxed);
        first..first + self.count
    }

    /// Bumped whenever who holds which deck or the controller changes, so
    /// LEDs know to send everything again.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Relaxed)
    }

    /// The other kdjs, by deck.
    pub fn peers(&self) -> Vec<PeerInfo> {
        let mut peers: Vec<_> = self
            .state()
            .peers
            .iter()
            .map(|p| PeerInfo { name: p.hello.name.clone(), decks: p.claim().decks(), controller: p.hello.controller })
            .collect();
        peers.sort_by_key(|p| p.decks.start);
        peers
    }

    /// Take deck `first` (and the ones after it, running several): the
    /// settings' choice, which wins over whoever holds it now.
    pub fn claim(&self, first: usize) {
        let first = first.min(MAX_DECKS - self.count);
        let mut s = self.state();
        s.top_stamp += 1;
        s.stamp = s.top_stamp;
        s.claimed = true;
        drop(s);
        self.move_to(first);
        self.say_hello();
    }

    /// Say whether the controller is plugged in here.
    pub fn set_controller(&self, here: bool) {
        if self.controller.swap(here, Ordering::Relaxed) != here {
            self.epoch.fetch_add(1, Ordering::Relaxed);
            self.say_hello();
        }
    }

    /// Where controller messages from other kdjs go.
    pub fn on_midi(&self, sink: MidiSink) {
        *self.midi_sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    /// Where LED messages from other kdjs go (for the controller plugged in here).
    pub fn on_led(&self, sink: mpsc::Sender<Vec<u8>>) {
        *self.led_sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    /// Send a controller message to the kdj that's deck `deck`. False if
    /// nobody is.
    pub fn send_midi(&self, deck: usize, msg: &[u8]) -> bool {
        let to = self.state().peers.iter().find(|p| p.claim().decks().contains(&deck)).map(|p| p.addr);
        let (Some(to), Some((b, len))) = (to, encode(MIDI, msg)) else { return false };
        self.data.send_to(&b[..len], to).is_ok()
    }

    /// Send an LED message to the kdjs the controller is plugged into.
    pub fn send_led(&self, msg: &[u8]) {
        let Some((b, len)) = encode(LED, msg) else { return };
        for p in self.state().peers.iter().filter(|p| p.hello.controller) {
            let _ = self.data.send_to(&b[..len], p.addr);
        }
    }

    /// Any other kdj holds the controller.
    pub fn controller_elsewhere(&self) -> bool {
        self.state().peers.iter().any(|p| p.hello.controller)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn move_to(&self, first: usize) {
        if self.first.swap(first, Ordering::Relaxed) != first {
            self.epoch.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Where this kdj takes controls and LEDs.
    fn port(&self) -> u16 {
        self.data.local_addr().map_or(0, |a| a.port())
    }

    fn hello(&self) -> Hello {
        Hello {
            name: self.name.clone(),
            first: u8::try_from(self.decks().start).unwrap_or(0),
            count: u8::try_from(self.count).unwrap_or(1),
            port: self.port(),
            stamp: self.state().stamp,
            controller: self.controller.load(Ordering::Relaxed),
        }
    }

    /// Say hello now (after a change), if holding a letter.
    fn say_hello(&self) {
        if self.state().claimed {
            let _ = self.data.send_to(&encode_hello(&self.hello()), SocketAddrV4::new(GROUP, PORT));
        }
    }

    /// Hear and say hellos, for as long as the app runs; the network can
    /// come and go (a Pi starts before its network is up).
    fn hellos(&self) {
        let started = Instant::now();
        let mut last_error = None;
        loop {
            match group_socket() {
                Ok(sock) => {
                    last_error = None;
                    let e = self.hello_loop(&sock, started);
                    eprintln!("net: {e}; trying again");
                }
                Err(e) => {
                    let e = e.to_string();
                    if last_error.as_ref() != Some(&e) {
                        eprintln!("net: no network yet ({e}); decks on other kdjs can't share a controller");
                        last_error = Some(e);
                    }
                }
            }
            std::thread::sleep(RETRY);
        }
    }

    /// Until the socket fails.
    fn hello_loop(&self, sock: &UdpSocket, started: Instant) -> std::io::Error {
        let mut buf = [0; 512];
        let mut next_hello = Instant::now();
        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if let Some(Packet::Hello(h)) = decode(&buf[..n]) {
                        self.heard(h, from);
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => return e,
            }
            self.forget_silent();
            self.settle(started);
            if Instant::now() >= next_hello && self.state().claimed {
                let to = SocketAddrV4::new(GROUP, PORT);
                if let Err(e) = self.data.send_to(&encode_hello(&self.hello()), to) {
                    return e;
                }
                next_hello = Instant::now() + HELLO_EVERY;
            }
        }
    }

    fn heard(&self, h: Hello, from: SocketAddr) {
        let addr = SocketAddr::new(from.ip(), h.port);
        let mine = h.port == self.port() && h.name == self.name;
        if mine || h.count == 0 {
            return;
        }
        let mut s = self.state();
        s.top_stamp = s.top_stamp.max(h.stamp);
        let changed = if let Some(p) = s.peers.iter_mut().find(|p| p.addr == addr) {
            let moved = (p.hello.first, p.hello.count) != (h.first, h.count);
            let changed = moved || p.hello.controller != h.controller;
            p.hello = h;
            p.seen = Instant::now();
            if moved {
                eprintln!("net: {} is deck {}", p.hello.name, letters(p.claim().decks()));
            }
            changed
        } else {
            let p = Peer { hello: h, addr, seen: Instant::now() };
            eprintln!("net: found {} (deck {})", p.hello.name, letters(p.claim().decks()));
            s.peers.push(p);
            true
        };
        drop(s);
        if changed {
            self.epoch.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn forget_silent(&self) {
        let mut s = self.state();
        let before = s.peers.len();
        s.peers.retain(|p| {
            let alive = p.seen.elapsed() < PEER_TIMEOUT;
            if !alive {
                eprintln!("net: lost {}", p.hello.name);
            }
            alive
        });
        let gone = s.peers.len() != before;
        drop(s);
        if gone {
            self.epoch.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Claim a letter once everyone's been heard; move off one that clashes.
    fn settle(&self, started: Instant) {
        let mut s = self.state();
        let others: Vec<_> = s.peers.iter().map(Peer::claim).collect();
        let decks = self.decks();
        let me = Claim { name: &self.name, port: self.port(), first: decks.start, count: self.count, stamp: s.stamp };
        let to = if s.claimed {
            settle(&me, &others)
        } else if started.elapsed() >= CLAIM_WAIT {
            Some(first_free(self.count, &others).unwrap_or(0))
        } else {
            return;
        };
        drop(others);
        s.claimed = true;
        drop(s);
        if let Some(first) = to {
            eprintln!("net: this is deck {}", letters(first..first + self.count));
            self.move_to(first);
        }
    }

    /// Take controls and LEDs from the other kdjs.
    fn receive(&self, sock: &UdpSocket) {
        let mut buf = [0; 64];
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf) else {
                std::thread::sleep(RETRY);
                continue;
            };
            let known = self.state().peers.iter().any(|p| p.addr.ip() == from.ip());
            match decode(&buf[..n]) {
                Some(Packet::Midi(msg)) if known => {
                    if let Some(sink) = self.midi_sink.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
                        sink(msg);
                    }
                }
                Some(Packet::Led(msg)) if known => {
                    if let Some(sink) = self.led_sink.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                        let _ = sink.send(msg.to_vec());
                    }
                }
                _ => {}
            }
        }
    }
}

/// The hello group's socket: shared with any other kdj on this machine,
/// hearing our own hellos too (a second kdj here is how it's tested).
fn group_socket() -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(unix)]
    s.set_reuse_port(true)?;
    s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT).into())?;
    s.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    s.set_multicast_loop_v4(true)?;
    s.set_multicast_ttl_v4(1)?;
    s.set_read_timeout(Some(Duration::from_millis(100)))?;
    Ok(s.into())
}

/// "A", or "A–B" for a kdj running two decks.
pub fn letters(decks: Range<usize>) -> String {
    let letter = |d: usize| char::from(b'A' + u8::try_from(d).unwrap_or(25).min(25));
    match decks.len() {
        0 | 1 => letter(decks.start).to_string(),
        _ => format!("{}–{}", letter(decks.start), letter(decks.end - 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(name: &str, first: usize, stamp: u32) -> Claim<'_> {
        Claim { name, port: 1, first, count: 1, stamp }
    }

    #[test]
    fn packets_round_trip() {
        let h = Hello { name: "kdj-oak-pike".into(), first: 1, count: 1, port: 50_123, stamp: 7, controller: true };
        assert_eq!(decode(&encode_hello(&h)), Some(Packet::Hello(h)));
        let (b, len) = encode(MIDI, &[0x9e, 24, 127]).expect("fits");
        assert_eq!(decode(&b[..len]), Some(Packet::Midi(&[0x9e, 24, 127])));
        let (b, len) = encode(LED, &[0x9e, 24, 0]).expect("fits");
        assert_eq!(decode(&b[..len]), Some(Packet::Led(&[0x9e, 24, 0])));
        assert_eq!(encode(MIDI, &[0; 4]), None, "not a MIDI message");
        assert_eq!(decode(b"kdj1"), None);
        assert_eq!(decode(b"xyz1M\x90\x24\x7f"), None, "not ours");
        assert_eq!(decode(b"kdj1H\x00\x01"), None, "cut short");
    }

    #[test]
    fn a_kdj_starting_up_takes_the_first_free_letter() {
        assert_eq!(first_free(1, &[]), Some(0));
        assert_eq!(first_free(1, &[claim("a", 0, 0)]), Some(1));
        assert_eq!(first_free(1, &[claim("a", 0, 0), claim("b", 2, 0)]), Some(1));
        let two = Claim { count: 2, ..claim("c", 1, 0) };
        assert_eq!(first_free(2, &[claim("a", 0, 0)]), Some(1));
        assert_eq!(first_free(1, &[claim("a", 0, 0), two]), Some(3));
        let all: Vec<_> = (0..MAX_DECKS).map(|d| claim("x", d, 0)).collect();
        assert_eq!(first_free(1, &all), None);
    }

    #[test]
    fn a_clash_goes_to_the_higher_stamp_then_the_name() {
        // Started together: both took A; the name decides, the other moves.
        assert_eq!(settle(&claim("kdj-a", 0, 0), &[claim("kdj-b", 0, 0)]), None);
        assert_eq!(settle(&claim("kdj-b", 0, 0), &[claim("kdj-a", 0, 0)]), Some(1));
        // Set to A in the settings (a higher stamp): it gets A.
        assert_eq!(settle(&claim("kdj-a", 0, 0), &[claim("kdj-b", 0, 1)]), Some(1));
        assert_eq!(settle(&claim("kdj-b", 0, 1), &[claim("kdj-a", 0, 0)]), None);
        // No clash, no move.
        assert_eq!(settle(&claim("kdj-b", 1, 0), &[claim("kdj-a", 0, 5)]), None);
    }

    #[test]
    fn letters_name_decks() {
        assert_eq!(letters(0..1), "A");
        assert_eq!(letters(1..2), "B");
        assert_eq!(letters(0..2), "A–B");
    }

    /// Two kdjs on this machine find each other, take different letters,
    /// and pass a controller message across.
    #[test]
    #[ignore = "uses the network (multicast on this machine)"]
    fn two_kdjs_find_each_other_and_share_a_controller() {
        let a = Net::start("kdj-test-a".into(), 1, Some(0)).expect("a");
        let b = Net::start("kdj-test-b".into(), 1, None).expect("b");
        let (tx, rx) = mpsc::channel();
        b.on_midi(Box::new(move |m| drop(tx.send(m.to_vec()))));
        let deadline = Instant::now() + Duration::from_secs(5);
        while (a.peers().is_empty() || b.decks() != (1..2)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(b.decks(), 1..2, "b took the free letter");
        assert!(a.send_midi(1, &[0x9e, 26, 127]));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).ok(), Some(vec![0x9e, 26, 127]));
        assert!(!a.send_midi(2, &[0x9e, 26, 127]), "nobody is deck C");
    }
}
