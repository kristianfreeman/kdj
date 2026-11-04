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
//! Hellos go to a multicast group, on every network this kdj is on (Wi-Fi
//! and a cable can both be up, and come and go), and straight to every kdj
//! already heard, since multicast on Wi-Fi gets lost. Controls and LEDs go
//! straight to the one kdj that needs them (multicast on Wi-Fi can wait for
//! the access point's next beacon), and are only taken from kdjs that have
//! said hello.
//!
//! Who holds a letter: a kdj starting up listens before it claims one, so it
//! never takes a letter that's in use. Two claiming the same letter (started
//! together, or set the same) settle it by the claim's stamp: picking a letter
//! in the settings outranks every claim seen so far; equal stamps go by name.
//! The other moves to the first letter free.
//!
//! Hellos also say which kdj holds MASTER (see `kdj_engine::session::Master`):
//! one taking it says so straight away, and the others' masters go back to
//! SYNC.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use kdj_engine::session::Master;
use kdj_library::key::Camelot;
use socket2::{Domain, Protocol, SockRef, Socket, Type};

/// The hello group and port.
const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 75, 74);
const PORT: u16 = 47_474;
/// Every packet starts with this (and a kind byte), so stray traffic on the
/// port is ignored, and a future incompatible version can be told apart.
const MAGIC: [u8; 4] = *b"kdj2";
const HELLO_EVERY: Duration = Duration::from_secs(1);
/// Silent this long: gone.
const PEER_TIMEOUT: Duration = Duration::from_secs(5);
/// A kdj's hellos stop coming from the address its controls go to for this
/// long (its cable unplugged, say): send them to one they still come from.
const ADDR_TIMEOUT: Duration = Duration::from_secs(2);
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
    /// Its library service's port (0: it shares nothing), and a stamp that
    /// changes whenever what it shares does.
    library: u16,
    library_stamp: u32,
    /// Where its decks' tracks came from, or are coming from (see `Use`).
    using: Vec<Use>,
    /// Its claim on MASTER (0: no deck there is master), and whether that
    /// master is stopped and handing over. Missing from an older kdj's.
    master: u64,
    master_stopped: bool,
    /// What each of its decks has on, in deck order. Missing from an older
    /// kdj's.
    on_deck: Vec<OnDeck>,
}

/// What's on a deck, for marking the tracks that go with it in key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OnDeck {
    /// The track's key, if it has one.
    pub key: Option<Camelot>,
    pub playing: bool,
    pub master: bool,
}

impl OnDeck {
    /// One byte: the key's code (0 for none), then playing and master.
    fn encode(self) -> u8 {
        self.key.map_or(0, Camelot::code) | (u8::from(self.playing) << 6) | (u8::from(self.master) << 7)
    }

    const fn decode(b: u8) -> Self {
        Self { key: Camelot::from_code(b & 0x3f), playing: b & 0x40 != 0, master: b & 0x80 != 0 }
    }
}

/// A track on a deck that came, or is coming, from a source on a kdj: so
/// that kdj knows its stick is in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Use {
    /// The kdj the source is on.
    pub kdj: String,
    /// Its number there.
    pub source: u32,
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

/// A hello's longest name (host names are at most 63 bytes a label).
const MAX_NAME: usize = 63;

fn encode_hello(h: &Hello) -> Vec<u8> {
    let mut b = Vec::with_capacity(64 + h.name.len());
    b.extend_from_slice(&MAGIC);
    b.extend_from_slice(&[HELLO, h.first, h.count, u8::from(h.controller)]);
    b.extend_from_slice(&h.port.to_be_bytes());
    b.extend_from_slice(&h.stamp.to_be_bytes());
    b.extend_from_slice(&h.library.to_be_bytes());
    b.extend_from_slice(&h.library_stamp.to_be_bytes());
    put_name(&mut b, &h.name);
    let using = &h.using[..h.using.len().min(MAX_DECKS)];
    b.push(u8::try_from(using.len()).unwrap_or(0));
    for u in using {
        put_name(&mut b, &u.kdj);
        b.extend_from_slice(&u.source.to_be_bytes());
    }
    b.extend_from_slice(&h.master.to_be_bytes());
    b.push(u8::from(h.master_stopped));
    let on_deck = &h.on_deck[..h.on_deck.len().min(MAX_DECKS)];
    b.push(u8::try_from(on_deck.len()).unwrap_or(0));
    b.extend(on_deck.iter().map(|d| d.encode()));
    b
}

/// A length-prefixed name, cut to `MAX_NAME` bytes on a character boundary.
fn put_name(b: &mut Vec<u8>, name: &str) {
    let end = (0..=name.len().min(MAX_NAME)).rev().find(|&i| name.is_char_boundary(i)).unwrap_or(0);
    b.push(u8::try_from(end).unwrap_or(0));
    b.extend_from_slice(&name.as_bytes()[..end]);
}

/// Reads a hello's fields in turn; None once it runs short.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2)?.try_into().ok().map(u16::from_be_bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_be_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8)?.try_into().ok().map(u64::from_be_bytes)
    }

    fn name(&mut self) -> Option<String> {
        let n = self.u8()?;
        std::str::from_utf8(self.take(n.into())?).ok().map(str::to_owned)
    }
}

fn decode_hello(b: &[u8]) -> Option<Hello> {
    let mut r = Reader(b);
    let (first, count, controller) = (r.u8()?, r.u8()?, r.u8()? != 0);
    let (port, stamp, library, library_stamp) = (r.u16()?, r.u32()?, r.u16()?, r.u32()?);
    let name = r.name()?;
    let uses = r.u8()?;
    let using = (0..uses).map(|_| Some(Use { kdj: r.name()?, source: r.u32()? })).collect::<Option<_>>()?;
    let master = r.u64().unwrap_or(0);
    let master_stopped = r.u8().is_some_and(|b| b != 0);
    let on_deck =
        r.u8().and_then(|n| r.take(n.into())).map_or_else(Vec::new, |b| b.iter().map(|&b| OnDeck::decode(b)).collect());
    Some(Hello {
        name,
        first,
        count,
        port,
        stamp,
        controller,
        library,
        library_stamp,
        using,
        master,
        master_stopped,
        on_deck,
    })
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
        HELLO => decode_hello(rest).map(Packet::Hello),
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
    /// Where it takes controls and LEDs: where its hellos come from, kept
    /// while they keep coming from there.
    addr: SocketAddr,
    /// Every address it has said hello from lately, and when (a kdj on a
    /// cable and Wi-Fi at once has two, and sends from either).
    ips: Vec<(IpAddr, Instant)>,
    seen: Instant,
}

impl Peer {
    fn has(&self, ip: IpAddr) -> bool {
        self.ips.iter().any(|&(i, _)| i == ip)
    }

    fn claim(&self) -> Claim<'_> {
        let h = &self.hello;
        Claim { name: &h.name, port: h.port, first: h.first.into(), count: h.count.into(), stamp: h.stamp }
    }
}

/// Another kdj, for showing and for fetching its library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerInfo {
    pub name: String,
    /// Its decks; none for a kdj that only shares a library (`--serve`).
    pub decks: Range<usize>,
    /// The controller is plugged into it.
    pub controller: bool,
    /// Its library service, if it shares anything, and the stamp of what.
    pub library: Option<(SocketAddr, u32)>,
    /// Where its decks' tracks came from.
    pub using: Vec<Use>,
}

#[derive(Default)]
struct State {
    peers: Vec<Peer>,
    stamp: u32,
    /// The highest stamp seen, ours included.
    top_stamp: u32,
    /// Holding a letter (and saying so); not while still listening at start.
    claimed: bool,
    /// Something in our hello changed: say it now, not at the next one.
    hello_now: bool,
    /// This kdj's library service and what's in it (see `Hello`).
    library: (u16, u32),
    /// The kdj last logged as holding MASTER elsewhere.
    master_logged: Option<String>,
    using: Vec<Use>,
    on_deck: Vec<OnDeck>,
}

/// Something to run on a controller message from another kdj.
type MidiSink = Box<dyn FnMut(&[u8]) + Send>;

/// This kdj's place on the network.
pub struct Net {
    name: String,
    count: usize,
    /// Sends and takes controls and LEDs.
    data: UdpSocket,
    first: AtomicUsize,
    /// The controller is plugged in here.
    controller: AtomicBool,
    /// Changes whenever who holds which deck, or the controller, does.
    epoch: AtomicU64,
    /// Who holds MASTER, shared with the audio thread.
    master: Arc<Master>,
    state: Mutex<State>,
    midi_sink: Mutex<Option<MidiSink>>,
    led_sink: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
}

impl Net {
    /// Join the network as `name`, running `count` decks: from `deck` if
    /// given, else the first free once the others have been heard. With no
    /// decks it only shares a library (`--serve`). `master` is what the
    /// audio thread claims, and where to put what the others do.
    ///
    /// # Errors
    /// If no UDP socket can be opened at all (the network itself coming and
    /// going is handled: it's tried again until it's there).
    pub fn start(name: String, count: usize, deck: Option<usize>, master: Arc<Master>) -> std::io::Result<Arc<Self>> {
        let data = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let receiver = data.try_clone()?;
        let net = Arc::new(Self {
            name,
            count: count.min(MAX_DECKS),
            data,
            first: AtomicUsize::new(deck.unwrap_or(0)),
            controller: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            master,
            state: Mutex::new(State { claimed: deck.is_some() || count == 0, ..State::default() }),
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

    /// The other kdjs, by deck (those with none last).
    pub fn peers(&self) -> Vec<PeerInfo> {
        let mut peers: Vec<_> = self
            .state()
            .peers
            .iter()
            .map(|p| {
                let h = &p.hello;
                PeerInfo {
                    name: h.name.clone(),
                    decks: p.claim().decks(),
                    controller: h.controller,
                    library: (h.library != 0).then(|| (SocketAddr::new(p.addr.ip(), h.library), h.library_stamp)),
                    using: h.using.clone(),
                }
            })
            .collect();
        peers.sort_by_key(|p| (p.decks.is_empty(), p.decks.start));
        peers
    }

    /// What's on the other kdjs' decks.
    pub fn others_on_deck(&self) -> Vec<OnDeck> {
        self.state().peers.iter().flat_map(|p| p.hello.on_deck.iter().copied()).collect()
    }

    /// This kdj's name on the network.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether `ip` is another kdj's (it has said hello).
    pub fn knows(&self, ip: IpAddr) -> bool {
        self.state().peers.iter().any(|p| p.has(ip))
    }

    /// Say where this kdj's library service is, and the stamp of what's in it.
    pub fn set_library(&self, port: u16, stamp: u32) {
        let changed = std::mem::replace(&mut self.state().library, (port, stamp)) != (port, stamp);
        if changed {
            self.say_hello();
        }
    }

    /// Say where this kdj's decks' tracks come from.
    pub fn set_using(&self, using: Vec<Use>) {
        let changed = {
            let mut s = self.state();
            let changed = s.using != using;
            s.using = using;
            changed
        };
        if changed {
            self.say_hello();
        }
    }

    /// Say what's on this kdj's decks.
    pub fn set_on_deck(&self, on_deck: Vec<OnDeck>) {
        let changed = {
            let mut s = self.state();
            let changed = s.on_deck != on_deck;
            s.on_deck = on_deck;
            changed
        };
        if changed {
            self.say_hello();
        }
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

    /// Send a controller message to every other kdj (a modifier: SHIFT held
    /// on the controller here counts for their decks too).
    pub fn send_midi_all(&self, msg: &[u8]) {
        let Some((b, len)) = encode(MIDI, msg) else { return };
        for p in &self.state().peers {
            let _ = self.data.send_to(&b[..len], p.addr);
        }
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
        let s = self.state();
        Hello {
            name: self.name.clone(),
            first: u8::try_from(self.decks().start).unwrap_or(0),
            count: u8::try_from(self.count).unwrap_or(1),
            port: self.port(),
            stamp: s.stamp,
            controller: self.controller.load(Ordering::Relaxed),
            library: s.library.0,
            library_stamp: s.library.1,
            using: s.using.clone(),
            master: self.master.ours().0,
            master_stopped: self.master.ours().1,
            on_deck: s.on_deck.clone(),
        }
    }

    /// Say hello now (after a change), not at the next one.
    fn say_hello(&self) {
        self.state().hello_now = true;
    }

    /// Say hello on every network in `nets`, and to every kdj heard (or just
    /// `to`), if holding a letter. Sending can fail while a network comes
    /// or goes; the next hello is a second away.
    fn send_hello(&self, sock: &UdpSocket, nets: &[Ipv4Addr], to: Option<IpAddr>) {
        let (claimed, peers) = {
            let s = self.state();
            (s.claimed, s.peers.iter().map(|p| p.addr.ip()).collect::<Vec<_>>())
        };
        if !claimed {
            return;
        }
        let hello = encode_hello(&self.hello());
        if to.is_none() {
            for net in nets {
                if SockRef::from(sock).set_multicast_if_v4(net).is_ok() {
                    let _ = sock.send_to(&hello, SocketAddrV4::new(GROUP, PORT));
                }
            }
        }
        for ip in to.map_or(peers, |ip| vec![ip]) {
            let _ = sock.send_to(&hello, SocketAddr::new(ip, PORT));
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
                        eprintln!("net: can't listen for other kdjs ({e}); trying again");
                        last_error = Some(e);
                    }
                }
            }
            std::thread::sleep(RETRY);
        }
    }

    /// Until the socket fails.
    fn hello_loop(&self, sock: &UdpSocket, started: Instant) -> std::io::Error {
        let mut buf = [0; 1500];
        let mut next_hello = Instant::now();
        let mut nets = Vec::new();
        let mut master_said = None;
        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    // One just started: answer it now, so it hears this one
                    // before it lets anyone in (or claims a letter).
                    if let Some(Packet::Hello(h)) = decode(&buf[..n])
                        && self.heard(h, from)
                    {
                        self.send_hello(sock, &nets, Some(from.ip()));
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => return e,
            }
            self.forget_silent();
            self.settle(started);
            self.hear_master();
            // MASTER changed hands here: tell the others now.
            let master = self.master.ours();
            if master_said.is_some_and(|(said, _)| (said == 0) != (master.0 == 0)) {
                eprintln!("net: MASTER {}", if master.0 == 0 { "went to another deck" } else { "is here" });
            }
            let now = std::mem::take(&mut self.state().hello_now) || Some(master) != master_said;
            master_said = Some(master);
            if now || Instant::now() >= next_hello {
                nets = join(sock, &nets);
                self.send_hello(sock, &nets, None);
                next_hello = Instant::now() + HELLO_EVERY;
            }
        }
    }

    /// Take a hello `from` somewhere. True if it's from a kdj not heard
    /// till now.
    fn heard(&self, mut h: Hello, from: SocketAddr) -> bool {
        let addr = SocketAddr::new(from.ip(), h.port);
        let mine = h.port == self.port() && h.name == self.name;
        if mine {
            return false;
        }
        let now = Instant::now();
        let mut s = self.state();
        s.top_stamp = s.top_stamp.max(h.stamp);
        // The same kdj by name and port, whichever network it was heard on.
        let known = s.peers.iter_mut().find(|p| (&p.hello.name, p.hello.port) == (&h.name, h.port));
        let (changed, new) = if let Some(p) = known {
            p.ips.retain(|&(ip, at)| ip != addr.ip() && now.duration_since(at) < PEER_TIMEOUT);
            p.ips.push((addr.ip(), now));
            let addr_heard = p.ips.iter().any(|&(ip, at)| ip == p.addr.ip() && now.duration_since(at) < ADDR_TIMEOUT);
            if !addr_heard {
                p.addr = addr;
            }
            let moved = (p.hello.first, p.hello.count) != (h.first, h.count);
            // What's on its decks changes all the time and needs nothing
            // shown or sent again (see `others_on_deck`).
            let on_deck = std::mem::take(&mut h.on_deck);
            p.hello.on_deck.clear();
            let changed = p.hello != h;
            h.on_deck = on_deck;
            p.hello = h;
            p.seen = now;
            if moved {
                eprintln!("net: {} is {}", p.hello.name, role(p.claim().decks()));
            }
            (changed, false)
        } else {
            let p = Peer { hello: h, addr, ips: vec![(addr.ip(), now)], seen: now };
            eprintln!("net: found {} ({})", p.hello.name, role(p.claim().decks()));
            s.peers.push(p);
            (true, true)
        };
        drop(s);
        if changed {
            self.epoch.fetch_add(1, Ordering::Relaxed);
        }
        new
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

    /// Pass on the highest claim on MASTER among the kdjs still heard.
    fn hear_master(&self) {
        let mut s = self.state();
        let top = s.peers.iter().filter(|p| p.hello.master != 0).max_by_key(|p| p.hello.master);
        let (claim, stopped) = top.map_or((0, false), |p| (p.hello.master, p.hello.master_stopped));
        let holder = top.map(|p| p.hello.name.clone());
        if holder != s.master_logged {
            if let Some(name) = &holder {
                eprintln!("net: {name} holds MASTER");
            }
            s.master_logged = holder;
        }
        drop(s);
        self.master.heard(claim, stopped);
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
            let known = self.state().peers.iter().any(|p| p.has(from.ip()));
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
    s.set_multicast_loop_v4(true)?;
    s.set_multicast_ttl_v4(1)?;
    s.set_read_timeout(Some(Duration::from_millis(100)))?;
    Ok(s.into())
}

/// Join the hello group on every network there is now (a cable plugged in,
/// Wi-Fi turned on), given the ones `joined` already; the ones gone drop out,
/// to be joined again if they come back.
fn join(sock: &UdpSocket, joined: &[Ipv4Addr]) -> Vec<Ipv4Addr> {
    let mut nets = networks();
    if nets.is_empty() {
        // Whichever the system picks (and none, it fails, till there's one).
        nets.push(Ipv4Addr::UNSPECIFIED);
    }
    nets.retain(|net| {
        joined.contains(net)
            || sock.join_multicast_v4(&GROUP, net).map_or_else(|e| e.kind() == std::io::ErrorKind::AddrInUse, |()| true)
    });
    nets
}

/// This machine's address on each network that's up and takes multicast
/// (not loopback).
#[cfg(unix)]
fn networks() -> Vec<Ipv4Addr> {
    let mut list = std::ptr::null_mut();
    // SAFETY: `list` is where getifaddrs puts the list it makes; freed below.
    if unsafe { libc::getifaddrs(&raw mut list) } != 0 {
        return Vec::new();
    }
    let mut nets = Vec::new();
    let mut at = list;
    while !at.is_null() {
        // SAFETY: `at` is a node of the list getifaddrs made, not yet freed.
        let ifa = unsafe { &*at };
        let flags = ifa.ifa_flags.cast_signed();
        let up = flags & libc::IFF_UP != 0 && flags & libc::IFF_MULTICAST != 0 && flags & libc::IFF_LOOPBACK == 0;
        // SAFETY: a non-null `ifa_addr` points at a sockaddr, which starts
        // with its family.
        if up && !ifa.ifa_addr.is_null() && i32::from(unsafe { (*ifa.ifa_addr).sa_family }) == libc::AF_INET {
            // SAFETY: an AF_INET address is a sockaddr_in.
            let sin = unsafe { &*ifa.ifa_addr.cast::<libc::sockaddr_in>() };
            let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
            if !nets.contains(&ip) {
                nets.push(ip);
            }
        }
        at = ifa.ifa_next;
    }
    // SAFETY: the list getifaddrs made, freed once.
    unsafe { libc::freeifaddrs(list) };
    nets
}

#[cfg(not(unix))]
fn networks() -> Vec<Ipv4Addr> {
    Vec::new()
}

/// What a kdj is: "deck A", or "library" for one that only shares (`--serve`).
pub fn role(decks: Range<usize>) -> String {
    if decks.is_empty() { "library".into() } else { format!("deck {}", letters(decks)) }
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
        let h = Hello {
            name: "kdj-oak-pike".into(),
            first: 1,
            count: 1,
            port: 50_123,
            stamp: 7,
            controller: true,
            library: 50_124,
            library_stamp: 99,
            using: vec![Use { kdj: "kdj-elm-fox".into(), source: 2 }],
            master: (3 << 32) | 9,
            master_stopped: true,
            on_deck: vec![
                OnDeck { key: Camelot::parse("Fm"), playing: true, master: true },
                OnDeck { key: Camelot::parse("E"), playing: false, master: false },
                OnDeck::default(),
            ],
        };
        assert_eq!(decode(&encode_hello(&h)), Some(Packet::Hello(h.clone())));
        let serve = Hello { count: 0, using: Vec::new(), ..h.clone() };
        assert_eq!(decode(&encode_hello(&serve)), Some(Packet::Hello(serve)));
        let mut older = encode_hello(&h);
        older.truncate(older.len() - 4);
        let Some(Packet::Hello(older)) = decode(&older) else { panic!("an older kdj's hello still decodes") };
        assert_eq!(older.on_deck, [], "it says nothing about its decks");
        let mut oldest = encode_hello(&older);
        oldest.truncate(oldest.len() - 10);
        let Some(Packet::Hello(oldest)) = decode(&oldest) else { panic!("an even older kdj's hello still decodes") };
        assert_eq!((oldest.master, oldest.master_stopped), (0, false), "it claims no MASTER");
        let long = Hello { name: "é".repeat(40), ..h };
        let Some(Packet::Hello(cut)) = decode(&encode_hello(&long)) else { panic!("a long name still decodes") };
        assert_eq!(cut.name, "é".repeat(31), "cut to 63 bytes, on a character boundary");
        let (b, len) = encode(MIDI, &[0x9e, 24, 127]).expect("fits");
        assert_eq!(decode(&b[..len]), Some(Packet::Midi(&[0x9e, 24, 127])));
        let (b, len) = encode(LED, &[0x9e, 24, 0]).expect("fits");
        assert_eq!(decode(&b[..len]), Some(Packet::Led(&[0x9e, 24, 0])));
        assert_eq!(encode(MIDI, &[0; 4]), None, "not a MIDI message");
        assert_eq!(decode(b"kdj2"), None);
        assert_eq!(decode(b"kdj1M\x90\x24\x7f"), None, "an older kdj's");
        assert_eq!(decode(b"kdj2H\x00\x01"), None, "cut short");
    }

    #[test]
    fn a_kdj_on_two_networks_is_one_kdj() {
        let net = Net::start("kdj-test-here".into(), 1, Some(0), Arc::default()).expect("a socket");
        let h = Hello {
            name: "kdj-test-there".into(),
            first: 1,
            count: 1,
            port: 50_123,
            stamp: 0,
            controller: true,
            library: 0,
            library_stamp: 0,
            using: Vec::new(),
            master: 0,
            master_stopped: false,
            on_deck: Vec::new(),
        };
        let cable = SocketAddr::from(([169, 254, 9, 9], PORT));
        let wifi = SocketAddr::from(([10, 9, 9, 9], PORT));
        assert!(net.heard(h.clone(), cable), "found");
        assert!(!net.heard(h.clone(), wifi), "the same kdj, heard on Wi-Fi too");
        let there = |net: &Net| net.peers().into_iter().filter(|p| p.name == h.name).count();
        assert_eq!(there(&net), 1);
        assert!(net.knows(cable.ip()) && net.knows(wifi.ip()), "takes controls from either");
        let to = |net: &Net| net.state().peers.iter().find(|p| p.hello.name == h.name).map(|p| p.addr.ip());
        assert_eq!(to(&net), Some(cable.ip()), "kept while its hellos come from there");
        // The cable goes quiet; its hellos still come over Wi-Fi.
        if let Some(p) = net.state().peers.iter_mut().find(|p| p.hello.name == h.name) {
            for (_, at) in &mut p.ips {
                *at -= ADDR_TIMEOUT;
            }
        }
        net.heard(h.clone(), wifi);
        assert_eq!(to(&net), Some(wifi.ip()), "controls go where it's still heard");
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
        let a = Net::start("kdj-test-a".into(), 1, Some(0), Arc::default()).expect("a");
        let b = Net::start("kdj-test-b".into(), 1, None, Arc::default()).expect("b");
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
