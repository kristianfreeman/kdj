//! The deck: transport, CDJ-style cue, tempo, key lock and beat loops.
//!
//! `Engine::process` runs inside the audio callback. It never allocates, locks
//! or frees: commands arrive over lock-free queues, state leaves through a
//! triple buffer, and replaced tracks are handed back to be freed elsewhere.
//!
//! Two positions are tracked. `pos` is what's audible right now; the display
//! and quantization use it. `feed` is where we read the source. With key lock
//! off they're the same. With key lock on, `feed` runs `feed_lead` frames ahead
//! because the stretcher has latency. Loops wrap both at the same source
//! boundary, so they stay sample-exact either way.

use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::action::{Action, Edge, LOOP_SIZES};
use crate::grid::BeatGrid;
use crate::nudge::{Nudge, NudgeConfig};
use crate::source::{self, Loop};
use crate::stretch::{FeedHistory, Stretcher};
use crate::sync::{self, QUANTUM, SyncMode, Timeline};
use crate::track::{HOT_CUES, HotCue, Track};

/// Fade length around jumps, starts and stops (~1.3 ms at 48 kHz): short
/// enough to feel instant, long enough not to click.
const FADE: usize = 64;
/// Tempo steps in percent, matching CDJ resolution for each range.
const FINE_STEP_NARROW: f64 = 0.02;
const FINE_STEP_WIDE: f64 = 0.05;
const COARSE_STEP: f64 = 1.0;
/// A fine tempo fader's range, in percent either way.
const FINE_RANGE: f64 = 1.0;
const GRID_NUDGE_SECS: f64 = 0.002;
const BPM_STEP: f64 = 0.01;
/// How far a nudge moves the playhead while paused, in beats.
const PAUSED_NUDGE_BEATS: f64 = 0.125;
const DEFAULT_LOOP: usize = 2; // 4 beats
/// A hardware tempo fader takes over within this many percent of the
/// deck's tempo (about one 7-bit MIDI step across ±8%), or once it crosses it.
const TAKEOVER_PCT: f64 = 0.15;
/// Playback rate limits, whatever tempo, sync and nudges ask for. The key
/// lock scratch buffer is sized for `MAX_RATE`.
const MIN_RATE: f64 = 0.25;
const MAX_RATE: usize = 2;
/// Grid tempo limits for manual BPM edits.
const MIN_BPM: f64 = 20.0;
const MAX_BPM: f64 = 400.0;
/// Chunks of feed history kept for the stretcher's latency: ~4,300 frames at
/// 48 kHz, so 1,024 covers callbacks down to 8 frames.
const HISTORY_CHUNKS: usize = 1024;

/// A pending playhead move. Relative moves are applied after the fade-out, so
/// beat jumps and loop wraps keep the beat phase to the sample.
#[derive(Clone, Copy, Debug)]
enum Jump {
    To(f64),
    By(f64),
}

/// Discontinuities requested by this block's commands, applied at a fade.
#[derive(Clone, Copy, Debug, Default)]
struct Pending {
    jump: Option<Jump>,
    /// The loop shrank past the playhead: fold it back in.
    rewrap: bool,
    /// The reader drifted from where it should be: restart it.
    resync: bool,
}

/// A cue or hot-cue preview in progress: plays while the button is held.
#[derive(Clone, Copy, Debug)]
struct Preview {
    /// The button that started it; releasing that one ends it.
    by: Action,
    /// Where releasing returns to.
    from: f64,
}

/// A hardware tempo fader, with soft takeover.
#[derive(Clone, Copy, Debug, Default)]
struct Fader {
    /// Where it is, -1 to 1; None until it first moves.
    pos: Option<f64>,
    /// It's reached the deck's tempo and now sets it.
    engaged: bool,
}

/// Everything about following or leading the shared timeline.
#[derive(Clone, Copy, Debug, Default)]
struct LinkState {
    mode: SyncMode,
    timeline: Option<Timeline>,
    /// This block's rate while following.
    rate: Option<f64>,
    /// Phase error against the timeline, in beats.
    error: f64,
    /// Just became master: the driver should line the timeline up with us.
    align_pending: bool,
    /// Bars have been lined up since SYNC went on or play started; from
    /// then on only the beat phase is held.
    locked: bool,
    /// Our beat against the timeline's at the start of this block.
    phase: Option<f64>,
    /// Beats this deck has been nudged off the timeline while following:
    /// SYNC holds it there instead of pulling it back (a grid a few ms off
    /// stays fixed). Cleared by SYNC, MASTER, or a new track.
    offset: f64,
}

/// What a deck on MASTER tells the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lead {
    /// Tempo, without nudges: nudging the master moves its phase, not everyone's tempo.
    pub bpm: f64,
    /// Beat at the coming block's first audible frame; None while stopped,
    /// when it only sets the tempo.
    pub beat: Option<f64>,
    /// Line the timeline's phase up with us now (MASTER just went on).
    pub align: bool,
}

#[derive(Debug)]
pub enum Command {
    Load(Arc<Track>),
    Input(Action, Edge),
    /// Needle search to this fraction of the track (0–1).
    Needle(f64),
}

/// What the engine looks like right now. Copied out once per audio callback.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flat, Copy record of independent deck flags that the UI and simulator read directly"
)]
pub struct Snapshot {
    pub track_id: u64,
    pub track_frames: u64,
    pub sample_rate: u32,
    /// Audible position in frames.
    pub pos: f64,
    pub playing: bool,
    pub previewing: bool,
    pub cue: f64,
    pub hot_cues: [Option<HotCue>; HOT_CUES],
    pub tempo_pct: f64,
    /// A hardware tempo fader's position, as a tempo, while it hasn't caught
    /// up with the deck's tempo yet (soft takeover); None once it has.
    pub fader: Option<f64>,
    pub wide_range: bool,
    pub keylock: bool,
    pub quantize: bool,
    /// Locked: loads are refused, playing or not (see `Action::Lock`).
    pub locked: bool,
    /// -1, 0 or 1: the direction of any nudge in progress.
    pub nudge: i8,
    /// The nudge's current bend, as a fraction of speed.
    pub bend: f64,
    /// Effective playback rate, including nudges.
    pub rate: f64,
    pub loop_active: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub loop_beats: f64,
    pub grid: BeatGrid,
    /// Frames rendered since start: the audio clock.
    pub clock: u64,
    pub sync: SyncMode,
    /// Phase error against the shared timeline, in beats, while following.
    pub sync_error: f64,
    /// The shared timeline's tempo, if there is one.
    pub link_bpm: Option<f64>,
    /// Ableton Link peers; None without Link (a local session, or none).
    pub link_peers: Option<u32>,
    /// Where our beat falls against the shared timeline's, in its beats
    /// within ±½, while playing on one.
    pub link_phase: Option<f64>,
}

impl Snapshot {
    #[must_use]
    pub fn bpm(&self) -> f64 {
        self.grid.bpm * self.rate
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EngineConfig {
    pub sample_rate: u32,
    /// Largest block processed in one go; bigger callbacks are split.
    pub max_block: usize,
    /// Extra input queues, e.g. one for the MIDI thread.
    pub extra_inputs: usize,
    pub nudge: NudgeConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { sample_rate: 48_000, max_block: 1024, extra_inputs: 0, nudge: NudgeConfig::default() }
    }
}

/// The non-audio side: send commands, read state, free old tracks.
pub struct EngineHandle {
    commands: Producer<Command>,
    snapshot: triple_buffer::Output<Snapshot>,
    garbage: Consumer<Arc<Track>>,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle").finish_non_exhaustive()
    }
}

/// What a controller thread sends the engine.
#[derive(Clone, Copy, Debug)]
enum Input {
    Action(Action, Edge),
    /// The tempo fader's position, -1 (slowest) to 1 (fastest).
    TempoFader(f64),
    FineFader(f64),
}

/// A queue of actions from another thread (e.g. MIDI), straight to the engine.
pub struct InputSender(Producer<Input>);

impl std::fmt::Debug for InputSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("InputSender").finish_non_exhaustive()
    }
}

impl InputSender {
    pub fn send(&mut self, action: Action, edge: Edge) -> bool {
        self.0.push(Input::Action(action, edge)).is_ok()
    }

    /// A tempo fader moved to `position`: -1 slowest, 0 centre, 1 fastest
    /// (across the tempo range).
    pub fn tempo_fader(&mut self, position: f64) -> bool {
        self.0.push(Input::TempoFader(position)).is_ok()
    }

    /// A fine tempo fader moved to `position`: -1 to 1 across ±1%, on top
    /// of the tempo fader.
    pub fn fine_fader(&mut self, position: f64) -> bool {
        self.0.push(Input::FineFader(position)).is_ok()
    }
}

impl EngineHandle {
    pub fn send(&mut self, action: Action, edge: Edge) -> bool {
        self.commands.push(Command::Input(action, edge)).is_ok()
    }

    /// Needle search: jump to `fraction` (0–1) of the track. Like a CDJ with
    /// needle lock, only while stopped; a playing deck ignores it.
    pub fn needle(&mut self, fraction: f64) -> bool {
        self.commands.push(Command::Needle(fraction)).is_ok()
    }

    /// Load a track. Refused while playing: a live deck never swaps under you.
    ///
    /// # Errors
    ///
    /// Hands the track back if the deck was playing at the last snapshot or
    /// the command queue is full. If it starts playing before the load lands,
    /// the engine drops the track instead: watch `Snapshot::track_id`.
    pub fn load(&mut self, track: Arc<Track>) -> Result<(), Arc<Track>> {
        let s = self.snapshot.output_buffer();
        if s.playing || s.locked {
            return Err(track);
        }
        // Push a second reference so a full queue can hand back the original.
        self.commands.push(Command::Load(Arc::clone(&track))).map_err(|_| track)
    }

    pub fn snapshot(&mut self) -> Snapshot {
        *self.snapshot.read()
    }

    /// Drop tracks the engine has finished with. Call regularly from a non-audio thread.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

#[expect(clippy::struct_excessive_bools, reason = "the deck's independent switches: play, key lock, quantize, lock…")]
pub struct Engine {
    commands: Consumer<Command>,
    inputs: Vec<Consumer<Input>>,
    garbage: Producer<Arc<Track>>,
    snapshot: triple_buffer::Input<Snapshot>,

    track: Option<Arc<Track>>,
    grid: BeatGrid,
    playing: bool,
    preview: Option<Preview>,
    hot_cues: [Option<HotCue>; HOT_CUES],
    pos: f64,
    feed: f64,
    feed_lead: f64,
    carry: f64,
    /// Source frames fed per output frame lately, and how much of that is
    /// audible yet (key lock only).
    history: FeedHistory,
    heard_fed: f64,
    cue: f64,
    tempo_pct: f64,
    /// The fine tempo fader's, on top of `tempo_pct`.
    fine_pct: f64,
    fader: Fader,
    wide_range: bool,
    keylock: bool,
    quantize: bool,
    locked: bool,
    nudge: Nudge,
    looping: Option<Loop>,
    loop_size: usize,

    pending: Pending,

    link: LinkState,

    stretcher: Stretcher,
    scratch: Vec<f32>,
    max_block: usize,
    clock: u64,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("track", &self.track.as_ref().map(|t| t.id))
            .field("playing", &self.playing)
            .field("pos", &self.pos)
            .field("feed", &self.feed)
            .field("tempo_pct", &self.tempo_pct)
            .field("fine_pct", &self.fine_pct)
            .field("keylock", &self.keylock)
            .field("looping", &self.looping)
            .field("clock", &self.clock)
            .finish_non_exhaustive()
    }
}

impl Engine {
    #[must_use]
    pub fn new(cfg: EngineConfig) -> (Self, EngineHandle, Vec<InputSender>) {
        let (cmd_tx, cmd_rx) = RingBuffer::new(256);
        let (garbage_tx, garbage_rx) = RingBuffer::new(16);
        let grid = BeatGrid::new(120.0, 0.0, cfg.sample_rate);
        let stretcher = Stretcher::new(cfg.sample_rate);
        // Room for the fastest block or a full seek preroll.
        let scratch_frames = cfg.max_block * MAX_RATE + 4;
        let scratch = vec![0.0; scratch_frames.max(stretcher.preroll()) * 2];

        let mut engine = Self {
            commands: cmd_rx,
            inputs: Vec::new(),
            garbage: garbage_tx,
            snapshot: triple_buffer::triple_buffer(&Snapshot::empty(grid)).0,
            track: None,
            grid,
            playing: false,
            preview: None,
            hot_cues: [None; HOT_CUES],
            pos: 0.0,
            feed: 0.0,
            feed_lead: 0.0,
            carry: 0.0,
            history: FeedHistory::new(HISTORY_CHUNKS),
            heard_fed: 0.0,
            cue: 0.0,
            tempo_pct: 0.0,
            fine_pct: 0.0,
            fader: Fader::default(),
            wide_range: false,
            keylock: true,
            quantize: true,
            locked: false,
            nudge: Nudge::new(cfg.nudge),
            looping: None,
            loop_size: DEFAULT_LOOP,
            pending: Pending::default(),
            link: LinkState::default(),
            stretcher,
            scratch,
            max_block: cfg.max_block,
            clock: 0,
        };
        let (snap_in, snap_out) = triple_buffer::triple_buffer(&engine.snapshot_now());
        engine.snapshot = snap_in;

        let senders = (0..cfg.extra_inputs)
            .map(|_| {
                let (tx, rx) = RingBuffer::new(256);
                engine.inputs.push(rx);
                InputSender(tx)
            })
            .collect();
        let handle = EngineHandle { commands: cmd_tx, snapshot: snap_out, garbage: garbage_rx };
        (engine, handle, senders)
    }

    /// The shared timeline (Ableton Link) for the coming block, as heard at
    /// its first frame. The driver calls this before each `process`.
    pub const fn set_timeline(&mut self, timeline: Option<Timeline>) {
        self.link.timeline = timeline;
    }

    /// What this deck tells the timeline, if it's on MASTER.
    pub fn lead(&mut self) -> Option<Lead> {
        if self.link.mode != SyncMode::Lead || self.track.is_none() {
            return None;
        }
        let bpm = self.grid.bpm * self.base_rate();
        if !self.playing {
            return Some(Lead { bpm, beat: None, align: false });
        }
        let align = std::mem::take(&mut self.link.align_pending);
        Some(Lead { bpm, beat: Some(self.grid.beat_at(self.pos)), align })
    }

    /// Another deck took MASTER: keep following it instead, like a CDJ.
    pub const fn yield_master(&mut self) {
        if matches!(self.link.mode, SyncMode::Lead) {
            self.link.mode = SyncMode::Follow;
            self.link.align_pending = false;
            self.link.locked = false;
            self.link.offset = 0.0;
        }
    }

    /// Fill `out` (interleaved stereo) with the next block of audio.
    pub fn process(&mut self, out: &mut [f32]) {
        let was_playing = self.playing;
        let was_keylock = self.keylock;
        let was_looping = self.looping;
        self.drain_commands();
        self.follow_timeline();

        let n = out.len() / 2;
        let fade = FADE.min(n / 2);
        let pending = std::mem::take(&mut self.pending);
        let restart = pending.jump.is_some() || pending.rewrap || pending.resync || self.keylock != was_keylock;

        // Fade the old stream out over the first few frames before any change
        // lands.
        let start = if was_playing && (restart || !self.playing) {
            self.fade_out_as_before(&mut out[..fade * 2], was_keylock, was_looping);
            fade
        } else {
            0
        };
        self.apply_pending(pending);
        self.play_body(&mut out[start * 2..], fade, !was_playing || restart);
        self.advance_clock(n);
    }

    /// Act on everything queued since the last block, from every input.
    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.commands.pop() {
            match cmd {
                Command::Load(t) => self.load(t),
                Command::Input(a, e) => self.input(a, e),
                Command::Needle(f) => {
                    if !self.playing && f.is_finite() {
                        self.seek(f.clamp(0.0, 1.0) * self.track_end());
                    }
                }
            }
        }
        for i in 0..self.inputs.len() {
            while let Ok(input) = self.inputs[i].pop() {
                match input {
                    Input::Action(a, e) => self.input(a, e),
                    Input::TempoFader(p) => self.tempo_fader(p),
                    Input::FineFader(p) => self.fine_fader(p),
                }
            }
        }
    }

    /// Render `out` exactly as the old stream would have played (old key lock,
    /// old loop), fading to silence, so the change that follows never clicks.
    fn fade_out_as_before(&mut self, out: &mut [f32], keylock: bool, looping: Option<Loop>) {
        let new_keylock = std::mem::replace(&mut self.keylock, keylock);
        let new_looping = std::mem::replace(&mut self.looping, looping);
        self.render(out);
        self.keylock = new_keylock;
        self.looping = new_looping;
        ramp(out, 1.0, 0.0);
    }

    /// Move the playhead as this block's commands asked.
    fn apply_pending(&mut self, pending: Pending) {
        if let Some(jump) = pending.jump {
            let to = match jump {
                Jump::To(p) => p,
                Jump::By(d) => self.pos + d,
            };
            self.pos = to.clamp(0.0, self.track_end());
        }
        if pending.rewrap
            && let Some(lp) = self.looping
        {
            self.pos = lp.wrap(self.pos);
        }
    }

    /// The rest of the block after any fade-out: audio if playing (fading in
    /// after a start or a jump), silence if not.
    fn play_body(&mut self, body: &mut [f32], fade: usize, fade_in: bool) {
        if !self.playing {
            body.fill(0.0);
            return;
        }
        if fade_in {
            self.restart_feed();
        }
        self.render(body);
        if fade_in {
            ramp(&mut body[..fade * 2], 0.0, 1.0);
        }
    }

    /// Account for `n` rendered frames and publish the new state.
    fn advance_clock(&mut self, n: usize) {
        if self.playing {
            let secs = frames_f64(n) / f64::from(self.grid.sample_rate);
            if self.link.rate.is_some() {
                self.link.offset += self.nudge.bend() * secs * self.grid.bpm / 60.0;
            }
            self.nudge.advance(secs);
        } else {
            self.nudge.settle();
        }
        self.clock += n as u64;
        let snap = self.snapshot_now();
        self.snapshot.write(snap);
    }

    /// Playback rate: tempo (or SYNC's), plus any nudge, within limits.
    fn rate(&self) -> f64 {
        let bend = if self.playing { self.nudge.bend() } else { 0.0 };
        #[expect(clippy::cast_precision_loss, reason = "a small constant")]
        let max = MAX_RATE as f64;
        (self.base_rate() + bend).clamp(MIN_RATE, max)
    }

    fn base_rate(&self) -> f64 {
        self.link.rate.unwrap_or(1.0 + (self.tempo_pct + self.fine_pct) / 100.0)
    }

    /// On SYNC: match the timeline's tempo, line the bars up when starting
    /// (one jump if far out, gentle bends after that), then hold the beat.
    fn follow_timeline(&mut self) {
        self.link.rate = None;
        self.link.phase = self
            .link
            .timeline
            .filter(|_| self.playing && self.track.is_some())
            .map(|tl| sync::beat_phase(self.grid.beat_at(self.here()), self.grid.bpm, &tl));
        let following = self.link.mode == SyncMode::Follow && self.playing && self.track.is_some();
        let Some(tl) = self.link.timeline.filter(|_| following) else {
            self.link.error = 0.0;
            self.link.locked = false;
            return;
        };
        let quantum = self.sync_quantum();
        let beat = self.grid.beat_at(self.here()) - self.link.offset;
        let f = sync::follow(beat, self.grid.bpm, &tl, quantum.unwrap_or(QUANTUM));
        self.link.locked = true;
        // The readout shows the synced tempo, and keeps it if SYNC goes off
        // (the fine fader, ignored meanwhile, then counts again from there).
        self.tempo_pct = (f.base_rate - 1.0).mul_add(100.0, -self.fine_pct);
        self.fader.engaged = false;
        if quantum.is_none() {
            self.link.error = 0.0;
            self.link.rate = Some(f.base_rate);
            return;
        }
        self.link.error = f.error;
        if let Some(beats) = f.snap_beats {
            self.shift(beats * self.grid.period());
            self.link.rate = Some(f.base_rate);
        } else {
            self.link.rate = Some(f.rate);
        }
    }

    /// The phase SYNC holds, in beats: bars when starting, then beats — or
    /// the loop length inside a loop shorter than a beat. None inside a loop
    /// that isn't a whole number of beats (or a whole fraction of one): its
    /// every wrap moves the phase, so only the tempo is followed.
    fn sync_quantum(&self) -> Option<f64> {
        if !self.link.locked {
            return Some(QUANTUM);
        }
        let Some(lp) = self.looping else { return Some(1.0) };
        let beats = (lp.end - lp.start) / self.grid.period();
        let whole = |x: f64| x > 0.0 && (x - x.round()).abs() < 0.01;
        if beats >= 1.0 - 0.01 && whole(beats) {
            Some(1.0)
        } else if beats > 0.0 && whole(1.0 / beats) {
            Some(beats)
        } else {
            None
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        let Some(track) = self.track.as_deref() else {
            out.fill(0.0);
            return;
        };
        let rate = self.rate();
        for chunk in out.chunks_mut(self.max_block * 2) {
            let n = frames_f64(chunk.len() / 2);
            if self.keylock {
                // Whole source frames for this chunk; the fraction carries over.
                let exact = n.mul_add(rate, self.carry);
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "rate is positive and a chunk is at most max_block frames, so the floor fits a usize"
                )]
                let n_in = (exact.floor() as usize).min(self.scratch.len() / 2);
                self.carry = exact - frames_f64(n_in);
                let input = &mut self.scratch[..n_in * 2];
                source::read(track, self.looping, &mut self.feed, 1.0, input);
                self.stretcher.process(input, chunk);
                // Advance by what's become audible: fed one output latency ago.
                self.history.push(n, n * rate);
                let heard = self.history.fed_at(self.history.out() - self.stretcher.output_latency());
                let advance = heard - self.heard_fed;
                self.heard_fed = heard;
                self.pos += advance;
                self.feed_lead += n.mul_add(rate, -advance);
                if let Some(lp) = self.looping {
                    self.pos = lp.wrap(self.pos);
                }
            } else {
                source::read(track, self.looping, &mut self.feed, rate, chunk);
                self.pos = self.feed;
            }
        }
        let end = frames_f64(track.frames());
        if self.pos >= end {
            self.pos = end;
            self.playing = false;
            self.preview = None;
        }
    }

    /// Point the source reader at `pos`. With key lock, prime the stretcher
    /// with history so its output starts exactly at `pos`.
    fn restart_feed(&mut self) {
        self.carry = 0.0;
        if !self.keylock {
            self.feed = self.pos;
            self.feed_lead = 0.0;
            return;
        }
        let rate = self.rate();
        let lead = self.stretcher.lead(rate);
        let n = self.stretcher.preroll();
        let Some(track) = self.track.as_deref() else { return };
        let mut cursor = self.pos + lead - frames_f64(n);
        let input = &mut self.scratch[..n * 2];
        source::read(track, self.looping, &mut cursor, 1.0, input);
        self.stretcher.seek(input, rate);
        self.feed = cursor;
        self.feed_lead = lead;
        self.history.reset(rate);
        self.heard_fed = self.history.fed_at(-self.stretcher.output_latency());
    }

    /// After a loop change, make sure the reader is where it should be; if
    /// it already wrapped (or now must), restart it at the audible position.
    fn check_feed(&mut self) {
        if !self.playing || self.pending.jump.is_some() || self.pending.rewrap {
            return;
        }
        let mut expected = self.pos + self.feed_lead;
        if let Some(lp) = self.looping {
            expected = lp.wrap(expected);
        }
        if (self.feed - expected).abs() > 1.5 {
            self.pending.resync = true;
        }
    }

    /// Where the playhead is, counting moves requested earlier in this block.
    fn here(&self) -> f64 {
        let p = match self.pending.jump {
            None => self.pos,
            Some(Jump::To(p)) => p,
            Some(Jump::By(d)) => (self.pos + d).clamp(0.0, self.track_end()),
        };
        match self.looping {
            Some(lp) if self.pending.rewrap => lp.wrap(p),
            _ => p,
        }
    }

    fn track_end(&self) -> f64 {
        self.track.as_ref().map_or(0.0, |t| frames_f64(t.frames()))
    }

    fn seek(&mut self, to: f64) {
        self.pending.jump = Some(Jump::To(to.clamp(0.0, self.track_end())));
    }

    /// Move by `delta` frames relative to wherever playback has got to.
    fn shift(&mut self, delta: f64) {
        self.pending.jump = Some(match self.pending.jump {
            Some(Jump::To(p)) => Jump::To((p + delta).clamp(0.0, self.track_end())),
            Some(Jump::By(d)) => Jump::By(d + delta),
            None => Jump::By(delta),
        });
    }

    fn load(&mut self, track: Arc<Track>) {
        if self.playing || self.locked {
            self.dispose(track);
            return;
        }
        self.grid = track.grid;
        if let Some(old) = self.track.replace(track) {
            self.dispose(old);
        }
        self.preview = None;
        self.looping = None;
        self.link.offset = 0.0;
        self.hot_cues = self.track.as_ref().map_or([None; HOT_CUES], |t| t.hot_cues);
        // Park on the load cue, like a CDJ.
        self.cue = self.track.as_ref().and_then(|t| t.load_cue).unwrap_or(0.0).max(0.0);
        self.pending.jump = Some(Jump::To(self.cue));
    }

    #[expect(
        clippy::mem_forget,
        reason = "only when the garbage queue is full: leaking a track beats freeing it on the audio thread"
    )]
    fn dispose(&mut self, track: Arc<Track>) {
        if let Err(rtrb::PushError::Full(t)) = self.garbage.push(track) {
            // Leaking beats freeing on the audio thread. The UI drains this
            // queue every frame, so it doesn't fill in practice.
            std::mem::forget(t);
        }
    }

    fn input(&mut self, action: Action, edge: Edge) {
        if self.track.is_none() {
            return;
        }
        if let Some(i) = action.hot_cue_index() {
            if edge == Edge::Press {
                self.hot_cue_press(action, i);
            } else {
                self.preview_release(action);
            }
            return;
        }
        match (action, edge) {
            (Action::Cue, Edge::Press) => self.cue_press(),
            (Action::Cue, Edge::Release) => self.preview_release(Action::Cue),
            (Action::NudgeForward, e) => self.hold_nudge(e, 1.0),
            (Action::NudgeBack, e) => self.hold_nudge(e, -1.0),
            (_, Edge::Release) => {}
            (a, Edge::Press) => self.press(a),
        }
    }

    /// A button press that doesn't care about release.
    fn press(&mut self, action: Action) {
        match action {
            Action::PlayPause => {
                if self.preview.is_some() {
                    self.preview = None; // keep playing after cue is released
                } else {
                    self.playing = !self.playing;
                }
            }
            Action::TempoUp => self.set_tempo(self.tempo_pct + self.fine_step()),
            Action::TempoDown => self.set_tempo(self.tempo_pct - self.fine_step()),
            Action::TempoUpCoarse => self.set_tempo(self.tempo_pct + COARSE_STEP),
            Action::TempoDownCoarse => self.set_tempo(self.tempo_pct - COARSE_STEP),
            Action::TempoReset => {
                // To 0 with the fine fader where it is (it's a fader: it
                // stays put), not to the nearest step.
                self.fader.engaged = false;
                self.tempo_pct = -self.fine_pct;
            }
            Action::TempoRange => {
                self.wide_range = !self.wide_range;
                let range = self.tempo_range();
                self.set_tempo(self.tempo_pct.clamp(-range, range));
            }
            Action::JogForward => self.jog(1.0),
            Action::JogBack => self.jog(-1.0),
            Action::PhaseForward => self.phase(1.0),
            Action::PhaseBack => self.phase(-1.0),
            Action::KeyLock => self.keylock = !self.keylock,
            Action::Lock => self.locked = true,
            Action::Unlock => self.locked = false,
            Action::Quantize => self.quantize = !self.quantize,
            Action::Sync => {
                self.link.mode = if self.link.mode == SyncMode::Follow { SyncMode::Off } else { SyncMode::Follow };
                self.link.locked = false;
                self.link.offset = 0.0;
            }
            Action::Master => {
                self.link.mode = if self.link.mode == SyncMode::Lead { SyncMode::Off } else { SyncMode::Lead };
                self.link.offset = 0.0;
                self.link.align_pending = self.link.mode == SyncMode::Lead;
            }
            Action::GridAnchorHere
            | Action::GridNudgeEarlier
            | Action::GridNudgeLater
            | Action::BpmUp
            | Action::BpmDown
            | Action::BpmHalve
            | Action::BpmDouble => self.grid_press(action),
            _ => self.loop_press(action),
        }
    }

    /// Manual beat-grid edits.
    fn grid_press(&mut self, action: Action) {
        self.grid = match action {
            Action::GridAnchorHere => self.grid.anchored_at(self.here()),
            Action::GridNudgeEarlier => self.grid.shifted(-self.grid_nudge()),
            Action::GridNudgeLater => self.grid.shifted(self.grid_nudge()),
            Action::BpmUp => self.grid.with_bpm((self.grid.bpm + BPM_STEP).min(MAX_BPM), self.here()),
            Action::BpmDown => self.grid.with_bpm((self.grid.bpm - BPM_STEP).max(MIN_BPM), self.here()),
            Action::BpmHalve if self.grid.bpm / 2.0 >= MIN_BPM => self.grid.with_bpm(self.grid.bpm / 2.0, self.here()),
            Action::BpmDouble if self.grid.bpm * 2.0 <= MAX_BPM => self.grid.with_bpm(self.grid.bpm * 2.0, self.here()),
            _ => return,
        };
    }

    /// Loops and beat jumps.
    fn loop_press(&mut self, action: Action) {
        match action {
            Action::LoopToggle => {
                if self.looping.is_some() {
                    self.looping = None;
                    self.check_feed();
                } else {
                    self.start_loop();
                }
            }
            Action::LoopHalve => self.set_loop_size(self.loop_size.saturating_sub(1)),
            Action::LoopDouble => self.set_loop_size((self.loop_size + 1).min(LOOP_SIZES.len() - 1)),
            Action::BeatJumpForward => self.beat_jump(1.0),
            Action::BeatJumpBack => self.beat_jump(-1.0),
            a => {
                if let Some(i) = a.beat_loop_index() {
                    self.beat_loop(i);
                }
            }
        }
    }

    /// A beat-loop button: start a loop of that size, resize the active one,
    /// or end it if it's already that size.
    fn beat_loop(&mut self, i: usize) {
        if self.looping.is_some() && i == self.loop_size {
            self.looping = None;
            self.check_feed();
        } else if self.looping.is_some() {
            self.set_loop_size(i);
        } else {
            self.loop_size = i;
            self.start_loop();
        }
    }

    /// CDJ cue: while playing, return to cue and stop. While stopped, set the
    /// cue here (snapped to a beat with quantize on) and preview while held.
    fn cue_press(&mut self) {
        if self.playing && self.preview.is_none() {
            self.playing = false;
            self.seek(self.cue);
        } else if !self.playing {
            let here = self.here();
            if (here - self.cue).abs() > 1.0 {
                self.cue = self.snap(here);
                self.seek(self.cue);
            }
            self.start_preview(Action::Cue, self.cue);
        }
    }

    const fn start_preview(&mut self, by: Action, from: f64) {
        self.playing = true;
        self.preview = Some(Preview { by, from });
    }

    /// Releasing the button that started a preview stops and returns.
    fn preview_release(&mut self, by: Action) {
        if let Some(preview) = self.preview
            && preview.by == by
        {
            self.preview = None;
            self.playing = false;
            self.seek(preview.from);
        }
    }

    /// Hot cue: jump while playing (keeping the beat phase with quantize on, so
    /// a beatmatched mix stays locked), preview while stopped, store if empty.
    fn hot_cue_press(&mut self, by: Action, i: usize) {
        let Some(hc) = self.hot_cues[i] else {
            self.hot_cues[i] = Some(HotCue { start: self.snap(self.here()), end: None });
            return;
        };
        if let Some(end) = hc.end {
            self.looping = Some(Loop { start: hc.start, end });
            self.check_feed();
        }
        if self.playing && self.preview.is_none() {
            let here = self.here();
            let phase = if self.quantize {
                let b = self.grid.beat_at(here);
                (b - b.round()) * self.grid.period()
            } else {
                0.0
            };
            // Relative, so the fade-out before the jump doesn't cost phase.
            self.shift(hc.start + phase - here);
        } else if !self.playing {
            self.seek(hc.start);
            self.start_preview(by, hc.start);
        }
    }

    fn hold_nudge(&mut self, edge: Edge, dir: f64) {
        let held = edge == Edge::Press;
        self.nudge.hold(dir, held);
        if held && !self.playing {
            self.seek((dir * PAUSED_NUDGE_BEATS).mul_add(self.grid.period(), self.here()));
        }
    }

    /// While playing, a jog tick bends speed briefly; stopped, it steps the
    /// playhead like a phase step.
    fn jog(&mut self, dir: f64) {
        if self.playing {
            self.nudge.jog(dir);
        } else {
            self.shift(dir * self.phase_step());
        }
    }

    /// Step the phase by a few milliseconds; on SYNC, it holds the new phase.
    fn phase(&mut self, dir: f64) {
        let step = dir * self.phase_step();
        self.shift(step);
        if self.link.rate.is_some() {
            self.link.offset += step / self.grid.period();
        }
    }

    /// One phase step, in frames.
    fn phase_step(&self) -> f64 {
        self.nudge.cfg.step_secs * f64::from(self.grid.sample_rate)
    }

    fn snap(&self, frame: f64) -> f64 {
        if self.quantize { self.grid.frame_of(self.grid.nearest_beat(frame)).max(0.0) } else { frame }
    }

    const fn fine_step(&self) -> f64 {
        if self.wide_range { FINE_STEP_WIDE } else { FINE_STEP_NARROW }
    }

    /// A hardware tempo fader moved. Soft takeover: it only takes over once
    /// it reaches (or crosses) the deck's tempo, so after SYNC, a button or a
    /// range change nothing jumps. SYNC owns the tempo while it's on.
    fn tempo_fader(&mut self, pos: f64) {
        if !pos.is_finite() {
            return;
        }
        let pos = pos.clamp(-1.0, 1.0);
        let range = self.tempo_range();
        let (target, prev) = (pos * range, self.fader.pos.map(|p| p * range));
        self.fader.pos = Some(pos);
        if self.link.mode == SyncMode::Follow {
            self.fader.engaged = false;
            return;
        }
        let here = self.tempo_pct;
        let crossed = prev.is_some_and(|p| (p - here).signum() != (target - here).signum());
        if self.fader.engaged || crossed || (target - here).abs() <= TAKEOVER_PCT {
            self.fader.engaged = true;
            let step = self.fine_step();
            self.tempo_pct = (target / step).round() * step;
        }
    }

    /// The fine tempo fader moved: ±`FINE_RANGE` on top of the tempo, in
    /// hundredths of a percent. On SYNC, sync owns the tempo; it counts
    /// again from where SYNC leaves it.
    fn fine_fader(&mut self, pos: f64) {
        if pos.is_finite() {
            self.fine_pct = (pos.clamp(-1.0, 1.0) * FINE_RANGE * 100.0).round() / 100.0;
        }
    }

    fn set_tempo(&mut self, pct: f64) {
        // Buttons move the tempo away from the fader: it has to catch up.
        self.fader.engaged = false;
        let range = self.tempo_range();
        let step = self.fine_step();
        // SYNC can leave the tempo outside the range: from there, presses
        // only bring it back, without a jump.
        let (lo, hi) = (self.tempo_pct.min(-range), self.tempo_pct.max(range));
        // Snap to the step so repeated presses land on clean values.
        self.tempo_pct = ((pct / step).round() * step).clamp(lo, hi);
    }

    const fn tempo_range(&self) -> f64 {
        if self.wide_range { 16.0 } else { 8.0 }
    }

    fn grid_nudge(&self) -> f64 {
        GRID_NUDGE_SECS * f64::from(self.grid.sample_rate)
    }

    fn start_loop(&mut self) {
        let beat = self.grid.floor_beat(self.here());
        let start = self.grid.frame_of(beat);
        let end = LOOP_SIZES[self.loop_size].mul_add(self.grid.period(), start);
        self.looping = Some(Loop { start, end });
        self.check_feed();
    }

    fn set_loop_size(&mut self, i: usize) {
        self.loop_size = i;
        let here = self.here();
        if let Some(lp) = &mut self.looping {
            lp.end = LOOP_SIZES[i].mul_add(self.grid.period(), lp.start);
            self.pending.rewrap |= here >= lp.end;
            self.check_feed();
        }
    }

    fn beat_jump(&mut self, dir: f64) {
        let delta = dir * LOOP_SIZES[self.loop_size] * self.grid.period();
        let end = self.track_end();
        if let Some(lp) = &mut self.looping {
            // A loop that would leave the track stays put.
            if lp.start + delta < 0.0 || lp.end + delta > end {
                return;
            }
            lp.start += delta;
            lp.end += delta;
        }
        self.shift(delta);
    }

    /// -1, 0 or 1: which way a nudge is pushing, ignoring a fading tail.
    fn nudge_dir(&self) -> i8 {
        let bend = self.nudge.bend();
        if bend.abs() < 1e-4 {
            0
        } else if bend > 0.0 {
            1
        } else {
            -1
        }
    }

    fn snapshot_now(&self) -> Snapshot {
        let lp = self.looping.unwrap_or(Loop { start: 0.0, end: 0.0 });
        Snapshot {
            track_id: self.track.as_ref().map_or(0, |t| t.id),
            track_frames: self.track.as_ref().map_or(0, |t| t.frames() as u64),
            sample_rate: self.grid.sample_rate,
            pos: self.here(),
            playing: self.playing,
            previewing: self.preview.is_some(),
            cue: self.cue,
            hot_cues: self.hot_cues,
            tempo_pct: self.tempo_pct + self.fine_pct,
            fader: self.fader.pos.filter(|_| !self.fader.engaged).map(|p| p * self.tempo_range()),
            wide_range: self.wide_range,
            keylock: self.keylock,
            quantize: self.quantize,
            locked: self.locked,
            nudge: self.nudge_dir(),
            bend: self.nudge.bend(),
            rate: self.rate(),
            loop_active: self.looping.is_some(),
            loop_start: lp.start,
            loop_end: lp.end,
            loop_beats: LOOP_SIZES[self.loop_size],
            grid: self.grid,
            clock: self.clock,
            sync: self.link.mode,
            sync_error: self.link.error,
            link_bpm: self.link.timeline.map(|t| t.bpm),
            link_peers: self.link.timeline.and_then(|t| t.peers),
            link_phase: self.link.phase,
        }
    }
}

impl Default for Snapshot {
    /// A deck with nothing loaded.
    fn default() -> Self {
        Self::empty(BeatGrid::new(120.0, 0.0, 48_000))
    }
}

impl Snapshot {
    const fn empty(grid: BeatGrid) -> Self {
        Self {
            track_id: 0,
            track_frames: 0,
            sample_rate: grid.sample_rate,
            pos: 0.0,
            playing: false,
            previewing: false,
            cue: 0.0,
            hot_cues: [None; HOT_CUES],
            tempo_pct: 0.0,
            fader: None,
            wide_range: false,
            keylock: true,
            quantize: true,
            locked: false,
            nudge: 0,
            bend: 0.0,
            rate: 1.0,
            loop_active: false,
            loop_start: 0.0,
            loop_end: 0.0,
            loop_beats: LOOP_SIZES[DEFAULT_LOOP],
            grid,
            clock: 0,
            sync: SyncMode::Off,
            sync_error: 0.0,
            link_bpm: None,
            link_peers: None,
            link_phase: None,
        }
    }
}

/// Frame counts as `f64`, for position arithmetic.
#[expect(clippy::cast_precision_loss, reason = "frame counts stay far below 2^52, where f64 stops being exact")]
const fn frames_f64(frames: usize) -> f64 {
    frames as f64
}

#[expect(clippy::cast_precision_loss, reason = "a ramp is at most FADE (64) frames, exact in f32")]
fn ramp(buf: &mut [f32], from: f32, to: f32) {
    let (frames, _) = buf.as_chunks_mut::<2>();
    let n = frames.len().max(1) as f32;
    for (i, frame) in frames.iter_mut().enumerate() {
        let g = from + (to - from) * (i as f32 + 0.5) / n;
        frame[0] *= g;
        frame[1] *= g;
    }
}
