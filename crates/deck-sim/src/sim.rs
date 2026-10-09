//! Drives the real engine on a virtual audio clock: same code as the audio
//! callback, no device, as fast as the CPU allows. Every block is timed and
//! runs with allocation forbidden.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use deck_engine::session::{self, LocalSession};
use deck_engine::{Action, Edge, Engine, EngineConfig, EngineHandle, Snapshot, Track};
use hdrhistogram::Histogram;

pub struct Sim {
    pub engine: Engine,
    pub handle: EngineHandle,
    pub track: Arc<Track>,
    pub block: usize,
    out: Vec<f32>,
    /// Rendered audio, if recording.
    pub recording: Option<Vec<f32>>,
    /// Callback CPU time (this thread only, so preemption doesn't count), ns.
    pub timings: Histogram<u64>,
    /// Callback wall time, ns: includes the OS scheduling something else.
    pub wall: Histogram<u64>,
    frames: u64,
    /// A shared timeline standing in for Ableton Link, and its peer count.
    session: Option<(LocalSession, Option<u32>)>,
}

impl Sim {
    pub fn new(track: Arc<Track>, block: usize) -> Result<Self> {
        let cfg = EngineConfig {
            sample_rate: track.sample_rate,
            max_block: block.max(64),
            extra_inputs: 0,
            ..Default::default()
        };
        let (engine, mut handle, _) = Engine::new(cfg);
        handle.load(Arc::clone(&track)).map_err(|_| anyhow!("a fresh engine refused the load"))?;
        let mut sim = Self {
            engine,
            handle,
            track,
            block,
            out: vec![0.0; block * 2],
            recording: None,
            timings: Histogram::new_with_bounds(100, 1_000_000_000, 3)?,
            wall: Histogram::new_with_bounds(100, 1_000_000_000, 3)?,
            frames: 0,
            session: None,
        };
        sim.step(); // apply the load
        sim.timings.reset();
        sim.wall.reset();
        Ok(sim)
    }

    pub fn sample_rate(&self) -> u32 {
        self.track.sample_rate
    }

    #[expect(clippy::cast_precision_loss, reason = "frame counts stay far below 2^52 (millennia of audio)")]
    pub fn secs(&self) -> f64 {
        self.frames as f64 / f64::from(self.sample_rate())
    }

    /// Frames rendered so far.
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// The first frame count at which the audio clock reads `secs` or later,
    /// so loops can run on whole frames instead of comparing floats.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "ceil of a time on the clock; negative or NaN saturates to 0, as `secs() < secs` would stop"
    )]
    pub fn frame_at(&self, secs: f64) -> u64 {
        (secs * f64::from(self.sample_rate())).ceil() as u64
    }

    /// Follow (or lead) `session` from now on, as if `peers` were on Link.
    pub const fn join(&mut self, session: LocalSession, peers: Option<u32>) {
        self.session = Some((session, peers));
    }

    pub fn step(&mut self) {
        let (engine, out) = (&mut self.engine, &mut self.out);
        let (t, cpu) = (Instant::now(), thread_cpu_ns());
        let session = &mut self.session;
        crate::alloc_guard::forbid(|| {
            if let Some((s, peers)) = session {
                session::drive(std::slice::from_mut(engine), s, *peers);
            }
            engine.process(out);
        });
        if let Some((s, _)) = &mut self.session {
            s.advance(self.block as u64);
        }
        let wall = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let cpu = thread_cpu_ns() - cpu;
        self.timings.saturating_record(cpu.max(100));
        self.wall.saturating_record(wall.max(100));
        if let Some(rec) = &mut self.recording {
            rec.extend_from_slice(&self.out);
        }
        self.frames += self.block as u64;
        self.handle.collect_garbage();
    }

    /// Render until the audio clock reaches `secs`.
    pub fn run_until(&mut self, secs: f64) {
        let end = self.frame_at(secs);
        while self.frames < end {
            self.step();
        }
    }

    pub fn send(&mut self, action: Action, edge: Edge) {
        self.handle.send(action, edge);
    }

    pub fn snapshot(&mut self) -> Snapshot {
        self.handle.snapshot()
    }
}

/// CPU time consumed by the calling thread, in nanoseconds.
fn thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut ts) };
    // A CPU-time clock never reads negative.
    let secs = u64::try_from(ts.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(ts.tv_nsec).unwrap_or(0);
    secs * 1_000_000_000 + nanos
}
