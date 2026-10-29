//! The output device. The callback only runs the engines, mixes and converts
//! samples, and reports how long it took; it never allocates, locks or blocks.
//! With more than one deck (the two-deck harness) they're summed at −6 dB
//! each, like two mixer channels with the faders up.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use kdj_engine::Engine;

/// Runs around every block in the audio callback (the Link driver). Must be
/// realtime-safe.
///
/// A large callback is rendered in several blocks. Every block of one callback
/// gets the same `latency_us` (the callback's first frame to the speakers);
/// the hook places later blocks itself from the frames `after` has counted.
pub trait BlockHook: Send {
    /// Before rendering; the callback's first frame is heard `latency_us` from now.
    fn before(&mut self, engines: &mut [Engine], latency_us: i64);
    /// After rendering `frames`.
    fn after(&mut self, frames: u64);
}

/// SYNC and MASTER between the decks in this process, without a network.
pub struct LocalHook(pub kdj_engine::session::LocalSession);

impl BlockHook for LocalHook {
    fn before(&mut self, engines: &mut [Engine], _latency_us: i64) {
        kdj_engine::session::drive(engines, &mut self.0, None);
    }

    fn after(&mut self, frames: u64) {
        self.0.advance(frames);
    }
}

/// One callback's cost, sent to the UI for the perf overlay.
#[derive(Clone, Copy)]
pub struct CallbackStat {
    pub ns: u32,
    pub frames: u32,
}

pub struct Output {
    device: cpal::Device,
    config: cpal::StreamConfig,
    format: SampleFormat,
    pub name: String,
}

pub struct Audio {
    /// None when silent (no sound card): see `silent`.
    _stream: Option<cpal::Stream>,
    pub stats: rtrb::Consumer<CallbackStat>,
    /// Stream errors (xruns and the like), counted for the UI thread to report.
    pub errors: Arc<AtomicU64>,
    /// The device went away (unplugged); the stream is silent from here on.
    pub lost: Arc<AtomicBool>,
    /// How the audio thread's realtime promotion went (`rt::PROMOTED`, or `-errno`).
    pub rt: Arc<AtomicI32>,
}

impl Output {
    /// The output device whose name contains `wanted` (any case), or the
    /// default one; asking for `buffer` frames per callback. If nothing
    /// matches `wanted`, it says so, lists the outputs, and plays on the
    /// default one: a deck started at boot keeps running (and the log says
    /// what to put in `--device`) rather than failing over and over.
    pub fn open(buffer: u32, wanted: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let picked = match wanted {
            None => None,
            Some(w) => {
                let devices: Vec<_> = host.output_devices().context("listing output devices")?.collect();
                let names: Vec<String> = devices.iter().map(label).collect();
                let pick = names.iter().position(|n| n.to_lowercase().contains(&w.to_lowercase()));
                let device = pick.and_then(|i| devices.into_iter().nth(i));
                if device.is_none() {
                    eprintln!(
                        "audio: no output device matching {w:?}, using the default; there are:\n  {}",
                        names.join("\n  ")
                    );
                }
                device
            }
        };
        let device = match picked {
            Some(d) => d,
            None => host.default_output_device().ok_or_else(|| anyhow!("no audio output device"))?,
        };
        let supported = pick_config(&device)?;
        let format = supported.sample_format();
        let mut config = supported.config();
        if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size() {
            config.buffer_size = cpal::BufferSize::Fixed(buffer.clamp(*min, *max));
        }
        let name = label(&device);
        Ok(Self { device, config, format, name })
    }

    pub const fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn start(self, engines: Vec<Engine>, hook: Option<Box<dyn BlockHook>>) -> Result<Audio> {
        let (tx, rx) = rtrb::RingBuffer::new(4096);
        let errors = Arc::new(AtomicU64::new(0));
        let lost = Arc::new(AtomicBool::new(false));
        let rt = Arc::new(AtomicI32::new(crate::rt::UNKNOWN));
        let report = Report { errors: Arc::clone(&errors), lost: Arc::clone(&lost), rt: Arc::clone(&rt) };
        let stream = match self.format {
            SampleFormat::F32 => self.build::<f32>(engines, hook, tx, report),
            SampleFormat::I16 => self.build::<i16>(engines, hook, tx, report),
            SampleFormat::I32 => self.build::<i32>(engines, hook, tx, report),
            f => return Err(anyhow!("unsupported sample format {f}")),
        }?;
        stream.play()?;
        Ok(Audio { _stream: Some(stream), stats: rx, errors, lost, rt })
    }

    fn build<T: SizedSample + FromSample<f32>>(
        &self,
        mut engines: Vec<Engine>,
        mut hook: Option<Box<dyn BlockHook>>,
        mut stats: rtrb::Producer<CallbackStat>,
        report: Report,
    ) -> Result<cpal::Stream> {
        const CHUNK: usize = 4096;
        let ch = self.config.channels as usize;
        let mut stereo = vec![0.0f32; CHUNK * 2];
        let mut deck_buf = vec![0.0f32; CHUNK * 2];
        let gain = if engines.len() > 1 { 0.5 } else { 1.0 };
        let rt = Arc::clone(&report.rt);
        let mut promoted = false;
        let callback = move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            if !promoted {
                // Once, on the audio thread itself: a single syscall.
                promoted = true;
                rt.store(crate::rt::promote_current_thread(), Ordering::Relaxed);
            }
            let t = Instant::now();
            // How long until this callback's first frame is heard.
            let stamp = info.timestamp();
            let latency_us = stamp.playback.duration_since(stamp.callback).as_micros();
            let latency_us = i64::try_from(latency_us).unwrap_or(0);
            guarded(|| {
                for out in data.chunks_mut(CHUNK * ch) {
                    let n = out.len() / ch;
                    // The same latency for every block: the hook's clock
                    // already moves later blocks on by the frames before them.
                    if let Some(h) = hook.as_mut() {
                        h.before(&mut engines, latency_us);
                    }
                    let buf = &mut stereo[..n * 2];
                    buf.fill(0.0);
                    for engine in &mut engines {
                        let one = &mut deck_buf[..n * 2];
                        engine.process(one);
                        for (m, v) in buf.iter_mut().zip(one.iter()) {
                            *m += gain * v;
                        }
                    }
                    if let Some(h) = hook.as_mut() {
                        h.after(n as u64);
                    }
                    for (o, &[l, r]) in out.chunks_exact_mut(ch).zip(buf.as_chunks::<2>().0) {
                        if ch == 1 {
                            o[0] = T::from_sample(f32::midpoint(l, r));
                        } else {
                            o[0] = T::from_sample(l);
                            o[1] = T::from_sample(r);
                            o[2..].fill(T::EQUILIBRIUM);
                        }
                    }
                }
            });
            // Saturate rather than wrap: neither is anywhere near 2^32.
            let frames = u32::try_from(data.len() / ch).unwrap_or(u32::MAX);
            let ns = u32::try_from(t.elapsed().as_nanos()).unwrap_or(u32::MAX);
            let _ = stats.push(CallbackStat { ns, frames });
        };
        // No printing here: this can run on the audio thread. The UI thread
        // reports what it counts.
        let on_error = move |e: cpal::Error| {
            report.errors.fetch_add(1, Ordering::Relaxed);
            if matches!(e.kind(), cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::HostUnavailable) {
                report.lost.store(true, Ordering::Relaxed);
            }
        };
        Ok(self.device.build_output_stream(self.config, callback, on_error, None)?)
    }
}

/// Where the stream's error callback reports to.
struct Report {
    errors: Arc<AtomicU64>,
    lost: Arc<AtomicBool>,
    rt: Arc<AtomicI32>,
}

/// Sample formats the deck can write, best first.
const FORMATS: [SampleFormat; 3] = [SampleFormat::F32, SampleFormat::I32, SampleFormat::I16];
/// Rates the engine is happy at, best first. Anything far above these just
/// costs CPU (an ALSA `hw:` device may offer up to 384 kHz).
const RATES: [u32; 2] = [48_000, 44_100];

/// The stream config to ask for: a preferred rate and format, stereo if
/// offered; the device's default if nothing fits.
fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let ranges: Vec<_> = device.supported_output_configs().context("listing output configs")?.collect();
    let rank = |r: &cpal::SupportedStreamConfigRange| {
        let format = FORMATS.iter().position(|&f| f == r.sample_format())?;
        let rate = RATES.iter().position(|&s| (r.min_sample_rate()..=r.max_sample_rate()).contains(&s))?;
        Some((rate, format, usize::from(r.channels() != 2), RATES.get(rate).copied()?))
    };
    let best = ranges.iter().filter_map(|r| Some((rank(r)?, r))).min_by_key(|(k, _)| (k.0, k.1, k.2));
    match best {
        Some(((.., rate), r)) => Ok(r.with_sample_rate(rate)),
        None => device.default_output_config().context("querying output config"),
    }
}

/// A device's name as shown to people (and matched by `--device`).
fn label(device: &cpal::Device) -> String {
    device.description().map_or_else(|_| "output".into(), |d| d.to_string())
}

/// Debug builds abort if the audio callback allocates.
#[inline]
fn guarded(f: impl FnOnce()) {
    #[cfg(debug_assertions)]
    assert_no_alloc::assert_no_alloc(f);
    #[cfg(not(debug_assertions))]
    f();
}

/// No sound card: run the decks anyway, in real time on a thread of their
/// own, and throw the sound away. The screen, the playhead and the controls
/// all work, and the deck can say what's missing instead of going dark.
pub fn silent(mut engines: Vec<Engine>, mut hook: Option<Box<dyn BlockHook>>, sample_rate: u32) -> Audio {
    const BLOCK: usize = 256;
    const BLOCK_FRAMES: u32 = 256;
    let (mut tx, rx) = rtrb::RingBuffer::new(4096);
    let (errors, lost, rt) =
        (Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicI32::new(0)));
    let _ = std::thread::Builder::new().name("audio-silent".into()).spawn(move || {
        let mut buf = vec![0.0f32; BLOCK * 2];
        #[expect(clippy::cast_precision_loss, reason = "a block of 256 frames")]
        let period = std::time::Duration::from_secs_f64(BLOCK as f64 / f64::from(sample_rate.max(1)));
        let mut next = Instant::now();
        loop {
            if let Some(h) = hook.as_mut() {
                h.before(&mut engines, 0);
            }
            for engine in &mut engines {
                engine.process(&mut buf);
            }
            if let Some(h) = hook.as_mut() {
                h.after(BLOCK as u64);
            }
            let _ = tx.push(CallbackStat { ns: 0, frames: BLOCK_FRAMES });
            next += period;
            match next.checked_duration_since(Instant::now()) {
                Some(rest) => std::thread::sleep(rest),
                // Fell behind (a stall): carry on from now, don't race to catch up.
                None => next = Instant::now(),
            }
        }
    });
    Audio { _stream: None, stats: rx, errors, lost, rt }
}
