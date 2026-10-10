//! deck: a DJ deck. Plays a track to the default output and draws the
//! 800×480 screen, with a full-screen browser for the Rekordbox library.
//!
//!   deck [TRACK] [--decks 2] [--fullscreen] [--buffer FRAMES] [--device NAME] [--keymap FILE]
//!        [--hud] [--midi FILE] [--midi-port NAME] [--no-link] [--deck LETTER]
//!   deck --midi-learn FILE [--midi-port NAME]  (write a MIDI mapping by touching each control)
//!   deck --synth            (a synthetic techno track, no file needed)
//!
//! `--device` picks the audio output whose name contains NAME (the default
//! output otherwise); `--midi-port` picks the MIDI input the same way,
//! overriding the mapping file's `port`; `--no-link` stays out of Ableton
//! Link sessions. `--deck B` makes this deck B among the kdjs on the network
//! (else it takes the first letter free); a controller plugged into any of
//! them plays each deck from its side of the mapping (see `net`).
//!
//! With no track it opens in the browser: sources, then playlists, then
//! tracks (↵ opens or loads, ← backs out, Tab closes). Dropping a file on the
//! window loads it too. Loading is refused while playing.
//!
//! `--decks 2` is the beatmatching harness: two decks stacked, mixed to the
//! output like two mixer channels. Keys go to the focused deck (`\` swaps);
//! hold Shift to send them to the other one.

mod audio;
mod leds;
#[cfg(feature = "link")]
mod link;
mod midi;
mod net;
mod rt;
mod touch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow};
use kdj_engine::analysis::{self, synth::SynthSpec};
use kdj_engine::keymap::{Keymap, Layer};
use kdj_engine::midimap::MidiMap;
use kdj_engine::session::LocalSession;
use kdj_engine::{Action, Edge, Engine, EngineConfig, EngineHandle, InputSender, Snapshot, Track};
use kdj_library::nav::{self, Node};
use kdj_library::{anlz, library};
use kdj_ui::Ui;
use kdj_ui::browser::{self, Browser};
use kdj_ui::settings::{self, Settings};
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod};
use sdl2::mouse::MouseButton;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

struct Args {
    track: Option<PathBuf>,
    synth: bool,
    decks: usize,
    fullscreen: bool,
    buffer: u32,
    keymap: Option<PathBuf>,
    hud: bool,
    /// A MIDI mapping file (`[midi]` table).
    midi: Option<PathBuf>,
    /// Write a MIDI mapping by asking for each control, then exit.
    midi_learn: Option<PathBuf>,
    /// Part of the MIDI input's name; overrides the mapping file's `port`.
    midi_port: Option<String>,
    /// Part of the audio output device's name; the default device if None.
    device: Option<String>,
    /// Don't join Ableton Link sessions.
    no_link: bool,
    /// This kdj's (first) deck among those on the network: 0 = A.
    deck: Option<usize>,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        track: None,
        synth: false,
        decks: 1,
        fullscreen: false,
        buffer: 128,
        keymap: None,
        hud: false,
        midi: None,
        midi_learn: None,
        midi_port: None,
        device: None,
        no_link: false,
        deck: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--synth" => a.synth = true,
            "--fullscreen" => a.fullscreen = true,
            "--hud" => a.hud = true,
            "--decks" => a.decks = value()?.parse::<usize>()?.clamp(1, 2),
            "--buffer" => a.buffer = value()?.parse()?,
            "--keymap" => a.keymap = Some(value()?.into()),
            "--midi" => a.midi = Some(value()?.into()),
            "--midi-learn" => a.midi_learn = Some(value()?.into()),
            "--midi-port" => a.midi_port = Some(value()?),
            "--device" => a.device = Some(value()?),
            "--no-link" => a.no_link = true,
            "--deck" => a.deck = Some(deck_letter(&value()?)?),
            s if s.starts_with("--") => return Err(anyhow!("unknown option {s}")),
            s => a.track = Some(s.into()),
        }
    }
    Ok(a)
}

/// "A"–"D" (or "a"–"d") as a deck index.
fn deck_letter(s: &str) -> Result<usize> {
    match s.to_ascii_uppercase().as_bytes() {
        &[c @ b'A'..=b'D'] => Ok(usize::from(c - b'A')),
        _ => Err(anyhow!("--deck is a letter, A to {}", net::letters(net::MAX_DECKS - 1..net::MAX_DECKS))),
    }
}

/// A finished background load, tagged with its deck and where in the browser
/// it came from.
struct Loaded {
    deck: usize,
    track: Result<Track, String>,
    /// The browser entry it came from (`nav::track_id`), to mark as loaded.
    entry_id: Option<u64>,
}

/// Where a load should go and come back to.
struct LoadJob {
    deck: usize,
    path: PathBuf,
    anlz_file: Option<PathBuf>,
    loaded_id: Option<u64>,
}

/// Load off the UI thread. Like a CDJ: a track Rekordbox prepared brings its
/// own grid, waveform and load cue, so it's only decoded; anything else gets
/// our fallback analysis and shows as "auto grid".
fn load_in_background(job: LoadJob, sr: u32, tx: mpsc::Sender<Loaded>) {
    std::thread::spawn(move || {
        let dat = job.anlz_file.clone().or_else(|| anlz::find(&job.path));
        let known = dat
            .as_deref()
            .and_then(|d| anlz::prepared(d, sr))
            .map(|p| analysis::Known { grid: p.grid, waveform: p.waveform, load_cue: p.load_cue, hot_cues: p.hot_cues })
            .unwrap_or_default();
        let source = match (known.grid.is_some(), known.waveform.is_some()) {
            (true, true) => "Rekordbox grid and waveform",
            (true, false) => "Rekordbox grid, our waveform",
            _ => "not in Rekordbox: analysed here",
        };
        let track = analysis::load_file_known(&job.path, sr, known).map(|(t, timings)| {
            eprintln!("loaded {} in {:.0} ms ({source})", t.title, timings.total().as_secs_f64() * 1000.0);
            t
        });
        let _ = tx.send(Loaded { deck: job.deck, track: track.map_err(|e| format!("{e:#}")), entry_id: job.loaded_id });
    });
}

fn synth_in_background(deck: usize, bpm: f64, sr: u32, tx: mpsc::Sender<Loaded>) {
    std::thread::spawn(move || {
        let seed = u32::try_from(deck).map_or(1, |d| d.saturating_add(1));
        let spec = SynthSpec { sample_rate: sr, bars: 160, bpm, seed, ..Default::default() };
        let (t, _) = analysis::build(format!("Synthetic hardgroove {bpm} bpm"), analysis::synth::render(&spec), sr);
        let _ = tx.send(Loaded { deck, track: Ok(t), entry_id: None });
    });
}

/// A finished library scan, with the sources as they were when it started.
struct Scan {
    stamps: Vec<SourceStamp>,
    libs: Vec<library::Library>,
}

/// What tells one library source from another: where it is, and its export
/// database's modified time and size (another stick with the same volume
/// name mounts at the same path).
type SourceStamp = (PathBuf, Option<(Option<SystemTime>, u64)>);

fn stamp(s: &library::Source) -> SourceStamp {
    let pdb = s.pdb.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| (m.modified().ok(), m.len()));
    (s.usbanlz.clone(), pdb)
}

fn source_stamps() -> Vec<SourceStamp> {
    library::sources().iter().map(stamp).collect()
}

/// Read every library source (collection, USB sticks) in the background.
fn scan_in_background(tx: mpsc::Sender<Scan>) {
    std::thread::spawn(move || {
        let sources = library::sources();
        let stamps = sources.iter().map(stamp).collect();
        let libs = sources.iter().map(library::scan).collect();
        let _ = tx.send(Scan { stamps, libs });
    });
}

fn main() {
    if cfg!(target_os = "linux") {
        match rt::lock_memory() {
            Ok(()) => eprintln!("memory: locked"),
            Err(e) => eprintln!("memory: not locked ({e})"),
        }
    }
    if let Err(e) = run() {
        eprintln!("kdj: {e:#}");
        std::process::exit(1);
    }
}

/// One deck as the UI thread sees it.
struct Slot {
    deck: EngineHandle,
    ui: Ui,
    /// The track on the deck, once the engine has taken it.
    current: Option<Arc<Track>>,
    loaded_id: Option<u64>,
    /// A track handed to the engine and not yet on the deck. The engine
    /// still drops it if the deck started playing in the meantime.
    pending: Option<Pending>,
}

struct Pending {
    track: Arc<Track>,
    entry_id: Option<u64>,
    since: Instant,
}

/// How long a power off / restart press waits for its confirming second press.
const POWER_CONFIRM: Duration = Duration::from_secs(3);

/// How long a load may take to show up on the deck before it counts as
/// refused.
const LOAD_TIMEOUT: Duration = Duration::from_secs(1);

/// Everything the UI thread owns.
struct App {
    slots: Vec<Slot>,
    /// The deck the keyboard (and the browser) controls.
    focus: usize,
    /// The same, for the MIDI thread.
    shared_focus: Arc<AtomicUsize>,
    hud: Hud,
    browser: Browser,
    settings: Settings,
    /// A power off / restart pressed once, waiting for the second press.
    armed: Option<(settings::Key, Instant)>,
    libs: Vec<library::Library>,
    /// The library node behind each browser item, level by level.
    nav: Vec<Vec<Node>>,
    /// Sources the current scan covered, to notice a stick going in.
    scanned: Vec<SourceStamp>,
    scan_tx: mpsc::Sender<Scan>,
    load_tx: mpsc::Sender<Loaded>,
    sample_rate: u32,
    status: String,
    /// This kdj among the others on the network.
    net: Option<Arc<net::Net>>,
    /// The network's `epoch` the screen last showed, and the deck letter the
    /// settings last showed (a change there is a tap).
    net_shown: (u64, usize),
}

impl App {
    /// Handle an action aimed at deck `target`.
    fn act(&mut self, a: Action, e: Edge, target: usize) {
        if !a.is_ui() {
            self.slots[target].deck.send(a, e);
            return;
        }
        if e == Edge::Release {
            return;
        }
        if a == Action::Settings {
            self.settings.open = !self.settings.open;
            return;
        }
        if self.settings.open && self.settings_key(a) {
            return;
        }
        let b = &mut self.browser;
        match a {
            Action::ZoomIn => self.slots[target].ui.zoom(false),
            Action::ZoomOut => self.slots[target].ui.zoom(true),
            Action::ToggleHud => self.hud.on = !self.hud.on,
            Action::SwitchDeck => self.set_focus((self.focus + 1) % self.slots.len()),
            Action::Browse => {
                b.open = !b.open;
                if b.open {
                    self.rescan_if_sources_changed();
                }
            }
            Action::BrowseUp => b.move_by(-1),
            Action::BrowseDown => b.move_by(1),
            Action::BrowsePageUp => b.move_by(-browser::ROWS.cast_signed()),
            Action::BrowsePageDown => b.move_by(browser::ROWS.cast_signed()),
            Action::BrowseLoad => self.open_selected(Some(target)),
            Action::BrowseEnter => self.open_selected(None),
            // One button browses: opens the browser, backs out a level, and
            // at the top closes it (a controller side has no room for two).
            Action::BrowseBack if !b.open => {
                b.open = true;
                self.rescan_if_sources_changed();
            }
            Action::BrowseBack => {
                if b.pop() {
                    self.nav.pop();
                } else {
                    b.open = false;
                }
            }
            _ => {}
        }
    }

    /// The browser's keys, on the settings screen. False for keys it doesn't use.
    fn settings_key(&mut self, a: Action) -> bool {
        let s = &mut self.settings;
        match a {
            Action::BrowseUp => s.move_by(-1),
            Action::BrowseDown => s.move_by(1),
            Action::BrowseEnter => s.sideways(1),
            Action::BrowseBack => s.sideways(-1),
            Action::BrowseLoad => {
                if let Some(key) = s.enter() {
                    self.setting_action(key);
                }
            }
            // Tab / Escape: back to the pages, then close.
            Action::Browse => {
                if !s.back() {
                    s.open = false;
                }
            }
            _ => return false,
        }
        true
    }

    /// A settings row's button.
    fn setting_action(&mut self, key: settings::Key) {
        if matches!(key, settings::Key::PowerOff | settings::Key::Restart) {
            self.power(key);
            return;
        }
        match key {
            settings::Key::Rescan => {
                self.browser.scanning = true;
                scan_in_background(self.scan_tx.clone());
                "Reading the library…".clone_into(&mut self.status);
            }
            settings::Key::Learn => "To map a controller, run: deck --midi-learn FILE".clone_into(&mut self.status),
            _ => {}
        }
    }

    /// Power off or restart the Pi, on a second press within a few seconds.
    fn power(&mut self, key: settings::Key) {
        let (verb, what) =
            if key == settings::Key::PowerOff { ("poweroff", "power off") } else { ("reboot", "restart") };
        let confirmed = self.armed.take().is_some_and(|(k, at)| k == key && at.elapsed() < POWER_CONFIRM);
        if !confirmed {
            self.armed = Some((key, Instant::now()));
            self.status = format!("Press again to {what}");
            return;
        }
        if !cfg!(target_os = "linux") {
            self.status = format!("Can't {what} from here: that's for the Pi");
            return;
        }
        // Allowed by /etc/sudoers.d/kdj, which the Pi setup installs.
        match std::process::Command::new("sudo").args(["-n", "/usr/bin/systemctl", verb]).status() {
            Ok(s) if s.success() => self.status = format!("Going to {what}…"),
            Ok(s) => self.status = format!("Couldn't {what} ({s})"),
            Err(e) => self.status = format!("Couldn't {what}: {e}"),
        }
    }

    fn set_focus(&mut self, i: usize) {
        self.focus = i;
        self.shared_focus.store(i, Ordering::Relaxed);
        self.browser.loaded = self.slots[i].loaded_id;
        self.label_decks();
    }

    /// Each deck's letter on screen, once there's more than one deck here or
    /// on the network.
    fn label_decks(&mut self) {
        let n = self.slots.len();
        let (first, shared) = self.net.as_ref().map_or((0, false), |net| (net.decks().start, !net.peers().is_empty()));
        for (j, s) in self.slots.iter_mut().enumerate() {
            let letter = net::letters(first + j..first + j + 1).chars().next().unwrap_or('A');
            s.ui.view.tag = (n > 1 || shared).then_some((letter, j == self.focus));
        }
    }

    /// Keep up with the network: a letter picked in the settings is claimed;
    /// the screen and settings follow letters and kdjs coming and going.
    fn follow_net(&mut self) {
        let Some(net) = self.net.clone() else { return };
        let (epoch, shown) = self.net_shown;
        if let Some(picked) = self.settings.choice(settings::Key::Deck).filter(|&d| d != shown) {
            net.claim(picked);
        }
        let deck = net.decks().start;
        if net.epoch() == epoch && deck == shown {
            return;
        }
        self.net_shown = (net.epoch(), deck);
        self.settings.set_choice(settings::Key::Deck, deck);
        self.settings.set_info(settings::Key::Decks, describe_peers(&net.peers()));
        self.label_decks();
    }

    /// Open the highlighted folder or playlist; with `load`, a highlighted
    /// track goes onto that deck.
    fn open_selected(&mut self, load: Option<usize>) {
        let Some(node) = self.browser.selected().and_then(|i| self.nav.last()?.get(i).copied()) else { return };
        match (node, load) {
            (Node::Track(s, i), Some(deck)) => {
                if self.slots[deck].deck.snapshot().playing {
                    self.status = "Stop the deck to load".into();
                    return;
                }
                let Some(e) = self.libs.get(s).and_then(|l| l.entries.get(i)) else { return };
                self.status = format!("Loading {}…", e.title);
                let job = LoadJob {
                    deck,
                    path: e.audio.clone(),
                    anlz_file: Some(e.anlz.clone()),
                    loaded_id: Some(nav::track_id(e)),
                };
                load_in_background(job, self.sample_rate, self.load_tx.clone());
            }
            (Node::Track(..), None) => {}
            (folder, _) => {
                let (level, nodes) = browser::level_for(&self.libs, folder);
                self.browser.push(level);
                self.nav.push(nodes);
            }
        }
    }

    /// Load a file dropped on the window onto the focused deck.
    fn load_dropped(&mut self, p: PathBuf) {
        if self.slots[self.focus].deck.snapshot().playing {
            self.status = "Stop the deck to load".into();
            return;
        }
        self.status = format!("Analysing {}…", file_name(&p));
        let job = LoadJob { deck: self.focus, path: p, anlz_file: None, loaded_id: None };
        load_in_background(job, self.sample_rate, self.load_tx.clone());
    }

    fn rescan_if_sources_changed(&mut self) {
        if !self.browser.scanning && source_stamps() != self.scanned {
            self.browser.scanning = true;
            scan_in_background(self.scan_tx.clone());
        }
    }

    fn take_libraries(&mut self, scan: Scan) {
        self.scanned = scan.stamps;
        self.libs = scan.libs;
        self.browser.scanning = false;
        if self.libs.is_empty() {
            self.browser.clear();
            self.nav.clear();
            return;
        }
        let (level, nodes) = browser::level_for(&self.libs, nav::root(&self.libs));
        self.browser.reset(level);
        self.nav = vec![nodes];
    }

    /// Hand a finished load to its deck. The screen follows once the engine
    /// has taken it (see `settle_loads`).
    fn take_loaded(&mut self, loaded: Loaded) {
        let slot = &mut self.slots[loaded.deck];
        match loaded.track {
            Ok(t) => {
                let t = Arc::new(t);
                if slot.deck.load(Arc::clone(&t)).is_err() {
                    self.status = "Stop the deck to load".into();
                    return;
                }
                slot.pending = Some(Pending { track: t, entry_id: loaded.entry_id, since: Instant::now() });
            }
            Err(e) => self.status = format!("Couldn't load: {e}"),
        }
    }

    /// Show each pending track once its deck has it. One that hasn't arrived
    /// in time was dropped by the engine (the deck started playing before it
    /// got there), and the screen keeps showing what's really on the deck.
    /// True if a track went on this frame.
    fn settle_loads(&mut self) -> bool {
        let mut any = false;
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(p) = slot.pending.take() else { continue };
            if slot.deck.snapshot().track_id == p.track.id {
                slot.ui.set_track(&p.track);
                slot.current = Some(p.track);
                slot.loaded_id = p.entry_id;
                if i == self.focus {
                    self.browser.loaded = p.entry_id;
                    self.browser.open = false;
                }
                self.status.clear();
                any = true;
            } else if p.since.elapsed() > LOAD_TIMEOUT {
                self.status = "Stop the deck to load".into();
            } else {
                slot.pending = Some(p);
            }
        }
        any
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    if let Some(out) = &args.midi_learn {
        return midi::learn(out, args.midi_port.as_deref());
    }
    let keymap = match &args.keymap {
        Some(p) => Keymap::with_overrides(&std::fs::read_to_string(p)?)?,
        None => Keymap::default(),
    };
    print_keys(&keymap);

    // Audio first: the device decides the sample rate everything runs at.
    let output = audio::Output::open(args.buffer, args.device.as_deref())?;
    let sample_rate = output.sample_rate();

    let (engines, handles, midi_inputs) = engines(args.decks, sample_rate);
    eprintln!("audio: {} at {} Hz, asked for {} frames, {} deck(s)", output.name, sample_rate, args.buffer, args.decks);
    let info = system_info(&args, &output.name, sample_rate);
    let mut audio = output.start(engines, Some(sync_hook(&args, sample_rate)))?;

    let (load_tx, load_rx) = mpsc::channel::<Loaded>();
    let (scan_tx, scan_rx) = mpsc::channel();
    scan_in_background(scan_tx.clone());
    let status = start_loading(&args, sample_rate, &load_tx);

    let screen = Screen::open(&args)?;
    let slots = screen.slots(handles)?;
    let mut events = screen.sdl.event_pump().map_err(|e| anyhow!(e))?;

    let mut app = App {
        slots,
        focus: 0,
        shared_focus: Arc::new(AtomicUsize::new(0)),
        hud: Hud::new(args.hud),
        browser: Browser::new(args.track.is_none() && !args.synth),
        settings: Settings::new(settings::pages(&settings::Prefs::default(), &info)),
        armed: None,
        libs: Vec::new(),
        nav: Vec::new(),
        scanned: Vec::new(),
        scan_tx,
        load_tx,
        sample_rate,
        status,
        net: join_network(&args),
        net_shown: (u64::MAX, args.deck.unwrap_or(0)),
    };
    app.set_focus(0);
    let mut midi = start_midi(&args, midi_inputs, &app.shared_focus, app.net.clone(), &mut app.settings)?;
    let debug = std::env::var_os("KDJ_DEBUG_INPUT").is_some();
    let mut input = Input { keymap, held: HashMap::new(), touch: touch::Touch::default(), finger: None, debug };
    let mut frames = Frames { started: Instant::now(), loaded: false, snaps: Vec::with_capacity(app.slots.len()) };
    let mut audio_watch = AudioWatch { errors: 0, lost: false, logged: Instant::now(), rt_logged: false };
    // Without vsync, pace the loop to about 60 fps instead of spinning a core.
    let pace = (!screen.vsync).then(|| Duration::from_micros(16_667));

    loop {
        let frame_start = Instant::now();
        // Input first, straight to the engines, before this frame's render.
        for event in events.poll_iter() {
            if !input.handle(&mut app, &screen.window, event) {
                return Ok(());
            }
        }

        // Controller actions for the browser and screen (deck actions went
        // straight to the engines).
        for (a, e, deck) in midi.iter().flat_map(|m| m.ui.try_iter()) {
            app.act(a, e, deck);
        }
        app.follow_net();
        if let Ok(scan) = scan_rx.try_recv() {
            app.take_libraries(scan);
        }
        while let Ok(loaded) = load_rx.try_recv() {
            app.take_loaded(loaded);
        }
        frames.loaded |= app.settle_loads();
        while let Ok(s) = audio.stats.pop() {
            app.hud.callback(s, sample_rate);
        }
        audio_watch.check(&mut app, &audio);
        frames.render(&mut app, &screen.window, &audio.errors);
        if let Some(leds) = midi.as_mut().and_then(|m| m.leds.as_mut()) {
            leds.update(&frames.snaps, app.focus);
        }
        if let Some(rest) = pace.and_then(|p| p.checked_sub(frame_start.elapsed())) {
            std::thread::sleep(rest);
        }
    }
}

/// Start the controller thread if there's a mapping file, and give the
/// settings screen its drawing.
fn start_midi(
    args: &Args,
    inputs: Vec<InputSender>,
    focus: &Arc<AtomicUsize>,
    net: Option<Arc<net::Net>>,
    settings: &mut Settings,
) -> Result<Option<midi::Midi>> {
    let Some(path) = &args.midi else { return Ok(None) };
    let (map, mut cfg) = MidiMap::from_toml(&std::fs::read_to_string(path)?)?;
    if args.midi_port.is_some() {
        cfg.port.clone_from(&args.midi_port);
    }
    settings.set_mapping(&kdj_engine::device::Diagram::new(&cfg));
    Ok(Some(midi::start(&cfg, map, inputs, Arc::clone(focus), net)))
}

/// Say hello to the other kdjs on the network, to share a controller.
fn join_network(args: &Args) -> Option<Arc<net::Net>> {
    net::Net::start(hostname(), args.decks, args.deck)
        .map_err(|e| eprintln!("net: can't share a controller with other kdjs: {e}"))
        .ok()
}

/// The other kdjs, for the settings: "kdj-oak-pike B (controller)".
fn describe_peers(peers: &[net::PeerInfo]) -> String {
    if peers.is_empty() {
        return "None found".into();
    }
    let each = peers.iter().map(|p| {
        let plugged = if p.controller { " (controller)" } else { "" };
        format!("{} {}{plugged}", p.name, net::letters(p.decks.clone()))
    });
    each.collect::<Vec<_>>().join(", ")
}

/// Reports what the audio side counted (it can't print from the callback).
struct AudioWatch {
    /// Stream errors already logged.
    errors: u64,
    /// The device loss was already logged.
    lost: bool,
    logged: Instant,
    /// The audio thread's realtime status was logged.
    rt_logged: bool,
}

impl AudioWatch {
    const LOST: &str = "Audio device lost";

    /// Log new stream errors (at most once a second), and keep a lost device
    /// on screen: the deck has gone quiet and won't come back by itself.
    fn check(&mut self, app: &mut App, audio: &audio::Audio) {
        let rt = audio.rt.load(Ordering::Relaxed);
        if !self.rt_logged && rt != rt::UNKNOWN {
            eprintln!("audio: {}", rt::describe(rt));
            self.rt_logged = true;
        }
        let errors = audio.errors.load(Ordering::Relaxed);
        if errors > self.errors && self.logged.elapsed() >= Duration::from_secs(1) {
            eprintln!("audio: {} stream error(s) (xruns or worse), {errors} in all", errors - self.errors);
            self.errors = errors;
            self.logged = Instant::now();
        }
        if audio.lost.load(Ordering::Relaxed) {
            if !self.lost {
                eprintln!("audio: the output device is gone");
                self.lost = true;
            }
            if app.status != Self::LOST {
                Self::LOST.clone_into(&mut app.status);
            }
        }
    }
}

/// One engine per deck, its handle for the UI, and its queue for the MIDI thread.
fn engines(decks: usize, sample_rate: u32) -> (Vec<Engine>, Vec<EngineHandle>, Vec<InputSender>) {
    let mut engines = Vec::new();
    let mut handles = Vec::new();
    let mut midi_inputs = Vec::new();
    for _ in 0..decks {
        let (e, h, inputs) = Engine::new(EngineConfig { sample_rate, extra_inputs: 1, ..Default::default() });
        engines.push(e);
        handles.push(h);
        midi_inputs.extend(inputs);
    }
    (engines, handles, midi_inputs)
}

/// What the settings screen shows about this machine.
fn system_info(args: &Args, output: &str, sample_rate: u32) -> settings::SystemInfo {
    let controller = args.midi.as_ref().map_or_else(
        || "No mapping (--midi FILE)".to_owned(),
        |p| args.midi_port.clone().unwrap_or_else(|| p.display().to_string()),
    );
    settings::SystemInfo {
        hostname: hostname(),
        output: output.to_owned(),
        sample_rate,
        link_peers: None,
        controller,
        deck: args.deck.unwrap_or(0),
        decks: "None found".into(),
        audio_errors: 0,
    }
}

/// This machine's name (on the Pi, kdj-word-word).
fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is writable for its whole length, which is what we pass.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    if ok { String::from_utf8_lossy(&buf[..end]).trim_end_matches(".local").to_owned() } else { "kdj".into() }
}

/// What SYNC and MASTER share: the Ableton Link session on the network, or
/// with `--no-link` (or no Link in the build) a session local to this process.
fn sync_hook(args: &Args, sample_rate: u32) -> Box<dyn audio::BlockHook> {
    #[cfg(feature = "link")]
    if !args.no_link {
        let link = Arc::new(rusty_link::AblLink::new(120.0));
        link.set_num_peers_callback(|n| eprintln!("link: {n} peer(s) in the session"));
        link.enable(true);
        eprintln!("link: joined the session on the local network");
        return Box::new(link::LinkDriver::new(link, sample_rate));
    }
    #[cfg(not(feature = "link"))]
    let _ = args;
    Box::new(audio::LocalHook(LocalSession::new(120.0, sample_rate)))
}

/// Start loading whatever the command line asked for; returns the status
/// line to show meanwhile.
fn start_loading(args: &Args, sample_rate: u32, load_tx: &mpsc::Sender<Loaded>) -> String {
    if args.synth {
        // Slightly different tempos, so there's something to beatmatch.
        for (deck, bpm) in [134.0, 135.0].into_iter().take(args.decks).enumerate() {
            synth_in_background(deck, bpm, sample_rate, load_tx.clone());
        }
    } else if let Some(p) = &args.track {
        let job = LoadJob { deck: 0, path: p.clone(), anlz_file: None, loaded_id: None };
        load_in_background(job, sample_rate, load_tx.clone());
        return format!("Analysing {}…", file_name(p));
    }
    String::new()
}

/// The window and its GL context. Fields drop in order: the context before
/// the window, the window before SDL.
struct Screen {
    _gl: sdl2::video::GLContext,
    window: sdl2::video::Window,
    video: sdl2::VideoSubsystem,
    sdl: sdl2::Sdl,
    /// OpenGL ES rather than desktop GL.
    es: bool,
    /// Presenting waits for vertical sync (and so paces the loop).
    vsync: bool,
}

impl Screen {
    /// Window and GL. ES on Linux (the Pi), 3.3 core elsewhere.
    fn open(args: &Args) -> Result<Self> {
        let sdl = sdl2::init().map_err(|e| anyhow!(e))?;
        let video = sdl.video().map_err(|e| anyhow!(e))?;
        let es = cfg!(target_os = "linux");
        {
            let attr = video.gl_attr();
            if es {
                attr.set_context_profile(sdl2::video::GLProfile::GLES);
                attr.set_context_version(3, 0);
            } else {
                attr.set_context_profile(sdl2::video::GLProfile::Core);
                attr.set_context_version(3, 3);
                attr.set_context_flags().forward_compatible().set();
            }
        }
        let height = screen_px(kdj_ui::HEIGHT) * u32::try_from(args.decks)?;
        let mut wb = video.window("kdj", screen_px(kdj_ui::WIDTH), height);
        wb.opengl().allow_highdpi().resizable().position_centered();
        if args.fullscreen {
            wb.fullscreen_desktop();
        }
        let window = wb.build()?;
        let gl = window.gl_create_context().map_err(|e| anyhow!(e))?;
        let vsync = video
            .gl_set_swap_interval(sdl2::video::SwapInterval::VSync)
            .map_err(|e| eprintln!("screen: no vsync ({e}); pacing frames by sleeping"))
            .is_ok();
        if args.fullscreen {
            sdl.mouse().show_cursor(false);
        }
        Ok(Self { _gl: gl, window, video, sdl, es, vsync })
    }

    /// A UI for each deck, each with its own handle on the GL context.
    fn slots(&self, handles: Vec<EngineHandle>) -> Result<Vec<Slot>> {
        // Pixels per point: 2 on a high-DPI screen. Window sizes fit in u16.
        let px = |v: u32| f32::from(u16::try_from(v).unwrap_or(u16::MAX));
        let scale = px(self.window.drawable_size().0) / px(self.window.size().0.max(1));
        handles
            .into_iter()
            .map(|deck| {
                // SAFETY: the context `open` created is current on this
                // thread, and SDL's loader returns that context's functions.
                let gl = unsafe { glow::Context::from_loader_function(|s| self.video.gl_get_proc_address(s).cast()) };
                Ok(Slot { deck, ui: Ui::new(gl, self.es, scale)?, current: None, loaded_id: None, pending: None })
            })
            .collect()
    }
}

/// A screen dimension (a small whole number of points) as window pixels.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the UI's dimensions are small positive whole numbers; a float-to-int `as` saturates anyway"
)]
const fn screen_px(points: f32) -> u32 {
    points as u32
}

/// Keyboard and drag-and-drop, routed to the app.
struct Input {
    keymap: Keymap,
    /// What each held key pressed (and on which deck), so its release goes to
    /// the same place even if the browser or focus changed in between.
    held: HashMap<String, (Action, usize)>,
    touch: touch::Touch,
    /// The finger driving the touch controls (one at a time), by SDL's id.
    finger: Option<i64>,
    /// Log every pointer and touch event (`KDJ_DEBUG_INPUT=1`).
    debug: bool,
}

/// SDL's mouse id for mouse events it makes from touches.
const TOUCH_MOUSE_ID: u32 = u32::MAX;

impl Input {
    /// Handle one window event; false when the window is closing.
    fn handle(&mut self, app: &mut App, window: &sdl2::video::Window, event: Event) -> bool {
        if self.pointer(app, window, &event) {
            return true;
        }
        match event {
            Event::Quit { .. } => return false,
            Event::KeyDown { keycode: Some(k), keymod, repeat, .. } => self.key_down(app, k, keymod, repeat),
            Event::KeyUp { keycode: Some(k), .. } => {
                if let Some((a, target)) = self.held.remove(&k.name()) {
                    app.act(a, Edge::Release, target);
                }
            }
            Event::DropFile { filename, .. } => app.load_dropped(PathBuf::from(filename)),
            _ => {}
        }
        true
    }

    /// Mouse and touchscreen: the touch controls. A touchscreen arrives as
    /// finger events (SDL's direct-to-display mode makes no mouse events
    /// from them); where SDL does make mouse events from touches too, those
    /// are skipped so nothing happens twice. True if `event` was one of these.
    fn pointer(&mut self, app: &mut App, window: &sdl2::video::Window, event: &Event) -> bool {
        let slots = app.slots.len();
        let (w, h) = window.size();
        let at = |x: f32, y: f32| logical(window, slots, x, y);
        // Fingers come normalised to the window, 0 to 1.
        let finger_at = |x: f32, y: f32| at(x * points(w), y * points(h));
        let (handled, report) = match *event {
            Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, which, .. } if which != TOUCH_MOUSE_ID => {
                if let Some((slot, p)) = at(coord(x), coord(y)) {
                    self.touch.down(app, slot, p);
                }
                (true, true)
            }
            Event::MouseMotion { mousestate, x, y, which, .. } if mousestate.left() && which != TOUCH_MOUSE_ID => {
                if let Some((_, p)) = at(coord(x), coord(y)) {
                    self.touch.motion(app, p);
                }
                (true, false)
            }
            Event::MouseButtonUp { mouse_btn: MouseButton::Left, which, .. } if which != TOUCH_MOUSE_ID => {
                self.touch.up(app);
                (true, true)
            }
            Event::FingerDown { finger_id, x, y, .. } if self.finger.is_none() => {
                self.finger = Some(finger_id);
                if let Some((slot, p)) = finger_at(x, y) {
                    self.touch.down(app, slot, p);
                }
                (true, true)
            }
            Event::FingerMotion { finger_id, x, y, .. } if self.finger == Some(finger_id) => {
                if let Some((_, p)) = finger_at(x, y) {
                    self.touch.motion(app, p);
                }
                (true, false)
            }
            Event::FingerUp { finger_id, .. } if self.finger == Some(finger_id) => {
                self.finger = None;
                self.touch.up(app);
                (true, true)
            }
            Event::MouseButtonDown { .. }
            | Event::MouseButtonUp { .. }
            | Event::MouseMotion { .. }
            | Event::FingerDown { .. }
            | Event::FingerUp { .. }
            | Event::FingerMotion { .. } => (true, false),
            _ => (false, false),
        };
        if self.debug && handled && (report || !matches!(event, Event::MouseMotion { .. })) {
            eprintln!("input: {event:?}");
        }
        handled
    }

    fn key_down(&mut self, app: &mut App, k: Keycode, keymod: Mod, repeat: bool) {
        let layer = if app.browser.open || app.settings.open { Layer::Browser } else { Layer::Deck };
        let name = k.name();
        let Some(a) = self.keymap.get_in(&name, layer) else { return };
        let shift = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);
        let target = if shift { (app.focus + 1) % app.slots.len() } else { app.focus };
        if !repeat {
            self.held.insert(name, (a, target));
            app.act(a, Edge::Press, target);
        } else if a.repeats() {
            app.act(a, Edge::Press, target);
        }
    }
}

/// A window point (as SDL reports mouse and touch positions) as a deck slot
/// and a logical position on its screen.
fn logical(window: &sdl2::video::Window, slots: usize, x: f32, y: f32) -> Option<(usize, [f32; 2])> {
    let (w, _) = window.size();
    let size = window.drawable_size();
    // Pixels per point: 2 on a high-DPI screen.
    let scale = points(size.0) / points(w.max(1));
    kdj_ui::to_logical(size, slots, [x * scale, y * scale])
}

/// A window dimension as a float (they fit in u16).
fn points(v: u32) -> f32 {
    f32::from(u16::try_from(v).unwrap_or(u16::MAX))
}

/// A mouse coordinate as a float (they fit in i16).
fn coord(v: i32) -> f32 {
    f32::from(i16::try_from(v).unwrap_or(i16::MAX))
}

/// What the render loop keeps from frame to frame.
struct Frames {
    started: Instant,
    /// A track finished loading since the last frame (for slow-frame reports).
    loaded: bool,
    /// This frame's engine snapshots, one per deck; reused.
    snaps: Vec<Snapshot>,
}

impl Frames {
    /// Draw every deck and present, reporting a frame that came late.
    fn render(&mut self, app: &mut App, window: &sdl2::video::Window, audio_errors: &AtomicU64) {
        self.snaps.clear();
        for slot in &mut app.slots {
            slot.deck.collect_garbage();
            let snap = slot.deck.snapshot();
            // Don't show the previous track's frame before the engine switches.
            let current = slot.current.as_ref().is_some_and(|t| t.id == snap.track_id);
            self.snaps.push(if current { snap } else { Snapshot { track_frames: 0, ..snap } });
        }
        let hud_text = app.hud.text(audio_errors.load(Ordering::Relaxed));
        let size = window.drawable_size();
        let t = Instant::now();
        let cpu = draw_decks(app, &self.snaps, size, &hud_text);
        let draw = t.elapsed().saturating_sub(cpu);
        window.gl_swap_window();
        let swap = t.elapsed().saturating_sub(draw).saturating_sub(cpu);
        if let Some(gap) = app.hud.frame(cpu)
            && gap > 20.0
        {
            self.report_slow(gap, cpu, draw, swap);
        }
        self.loaded = false;
    }

    /// Where a slow frame's time went, in ms.
    fn report_slow(&self, gap: f32, cpu: Duration, draw: Duration, swap: Duration) {
        let ms = |d: Duration| d.as_secs_f32() * 1000.0;
        let other = gap - ms(cpu) - ms(draw) - ms(swap);
        eprintln!(
            "slow frame at {:.1} s: {gap:.1} ms (build {:.2}, draw {:.2}, swap {:.1}, input/loading/other {other:.1}{})",
            self.started.elapsed().as_secs_f64(),
            ms(cpu),
            ms(draw),
            ms(swap),
            if self.loaded { "; track loaded this frame" } else { "" }
        );
    }
}

/// Build and draw each deck's slot of the screen; returns the CPU time spent
/// building.
fn draw_decks(app: &mut App, snaps: &[Snapshot], size: (u32, u32), hud_text: &str) -> Duration {
    let n = app.slots.len();
    let mut cpu = Duration::ZERO;
    app.slots[0].ui.gpu.clear_all(size);
    for i in 0..n {
        let focused = i == app.focus;
        let other = (n > 1).then(|| &snaps[(i + 1) % n]);
        let empty = app.slots[i].current.is_none();
        let status = if focused && empty && app.status.is_empty() {
            "Tap here or press Tab to browse"
        } else if focused {
            &app.status
        } else {
            ""
        };
        let overlay = match (focused, app.settings.open) {
            (false, _) => kdj_ui::Overlay::None,
            (true, true) => kdj_ui::Overlay::Settings(&mut app.settings),
            (true, false) => kdj_ui::Overlay::Browser(&mut app.browser),
        };
        let hud = if i == 0 { hud_text } else { "" };
        cpu += app.slots[i].ui.build(&snaps[i], other, overlay, status, hud);
        app.slots[i].ui.draw_slot(size, i, n);
    }
    cpu
}

/// Live perf overlay: audio callback cost against its period, UI frame cost,
/// frame pacing, audio errors.
struct Hud {
    on: bool,
    cb_ns: Vec<u32>,
    period_us: f64,
    frame_cpu: Vec<f32>,
    last_frame: Instant,
    worst_gap_ms: f32,
    text: String,
    refreshed: Instant,
    frames_seen: u32,
}

impl Hud {
    fn new(on: bool) -> Self {
        Self {
            on,
            cb_ns: Vec::with_capacity(8192),
            period_us: 0.0,
            frame_cpu: Vec::with_capacity(512),
            last_frame: Instant::now(),
            worst_gap_ms: 0.0,
            text: String::new(),
            refreshed: Instant::now(),
            frames_seen: 0,
        }
    }

    fn callback(&mut self, s: audio::CallbackStat, sample_rate: u32) {
        if self.cb_ns.len() < self.cb_ns.capacity() {
            self.cb_ns.push(s.ns);
        }
        self.period_us = f64::from(s.frames) / f64::from(sample_rate) * 1e6;
    }

    /// Record a presented frame; returns the time since the previous one, in ms.
    fn frame(&mut self, cpu: Duration) -> Option<f32> {
        if self.frame_cpu.len() < self.frame_cpu.capacity() {
            self.frame_cpu.push(cpu.as_secs_f32() * 1000.0);
        }
        let gap = self.last_frame.elapsed().as_secs_f32() * 1000.0;
        self.worst_gap_ms = self.worst_gap_ms.max(gap);
        self.last_frame = Instant::now();
        // The first few frames include window setup; don't report them.
        if self.frames_seen < 3 {
            self.frames_seen += 1;
            return None;
        }
        Some(gap)
    }

    /// Refreshed twice a second from the samples gathered since.
    fn text(&mut self, audio_errors: u64) -> String {
        if !self.on {
            return String::new();
        }
        if self.refreshed.elapsed() > Duration::from_millis(500) {
            self.refresh(audio_errors);
        }
        self.text.clone()
    }

    /// Summarise the samples gathered so far, if there are any, and start over.
    fn refresh(&mut self, audio_errors: u64) {
        self.cb_ns.sort_unstable();
        let (Some(&p99), Some(&max)) = (self.cb_ns.get(self.cb_ns.len() * 99 / 100), self.cb_ns.last()) else {
            return;
        };
        let (p99, max) = (f64::from(p99) / 1000.0, f64::from(max) / 1000.0);
        self.frame_cpu.sort_by(f32::total_cmp);
        let ui = self.frame_cpu.get(self.frame_cpu.len() * 99 / 100).copied().unwrap_or(0.0);
        self.text = format!(
            "audio p99 {p99:.0} µs · max {max:.0} µs of {:.0} µs · ui {ui:.2} ms · worst frame {:.1} ms · errors {audio_errors}",
            self.period_us, self.worst_gap_ms
        );
        self.cb_ns.clear();
        self.frame_cpu.clear();
        self.worst_gap_ms = 0.0;
        self.refreshed = Instant::now();
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn print_keys(km: &Keymap) {
    eprintln!("keys (Tab opens the browser):");
    let mut line = String::new();
    for (key, action) in km.bindings() {
        let item = format!("  {key:>8} {action:?}");
        if line.len() + item.len() > 100 {
            eprintln!("{line}");
            line.clear();
        }
        line.push_str(&item);
    }
    eprintln!("{line}");
}
