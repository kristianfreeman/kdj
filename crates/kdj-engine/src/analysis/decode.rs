//! File → interleaved stereo f32 at the file's own sample rate.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::audio::well_known::CODEC_ID_MP3;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

pub struct Decoded {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Silence put in front of the audio (see `lead_in`), in frames.
    pub lead_in: usize,
}

impl std::fmt::Debug for Decoded {
    // Not derived: the sample buffer is millions of floats.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoded")
            .field("frames", &(self.samples.len() / 2))
            .field("sample_rate", &self.sample_rate)
            .field("lead_in", &self.lead_in)
            .finish_non_exhaustive()
    }
}

/// Frames of silence an MP3 with an encoder's tag needs in front of it to sit
/// where Rekordbox's grid, cues and waveform expect it. Symphonia cuts the
/// encoder delay the tag declares and skips the frame holding the tag, while
/// Rekordbox's timeline keeps both: measured on a library, kicks on these
/// tracks came that much ahead of the grid (47.0 ms at 48 kHz) and all others
/// were on it. The delay is 0 if the tag is from an encoder Symphonia doesn't
/// know, and those need nothing.
fn lead_in(is_mp3: bool, delay: Option<u32>, rate: u32) -> usize {
    let delay = delay.filter(|&d| is_mp3 && d > 0).map_or(0, |d| d as usize);
    if delay == 0 {
        return 0;
    }
    // MPEG-1 frames are twice as long as those of the lower MPEG-2 rates.
    delay + if rate < 32_000 { 576 } else { 1152 }
}

/// Decode the default audio track of the file at `path`, refusing it if it
/// runs past `max_frames` (what fits in memory; see `analysis::frame_budget`).
///
/// # Errors
///
/// If the file can't be opened, isn't a format Symphonia knows, has no audio
/// track, yields no audio at all, or is too long.
pub fn decode(path: &Path, max_frames: Option<usize>) -> Result<Decoded> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    decode_from(Box::new(file), path.extension().and_then(|e| e.to_str()), max_frames)
}

/// As `decode`, from a whole file already in memory (a track on another
/// kdj); `ext` is its file type, a hint for the format.
///
/// # Errors
///
/// As `decode`.
pub fn decode_bytes(bytes: Vec<u8>, ext: Option<&str>, max_frames: Option<usize>) -> Result<Decoded> {
    decode_from(Box::new(std::io::Cursor::new(bytes)), ext, max_frames)
}

fn decode_from(source: Box<dyn MediaSource>, ext: Option<&str>, max_frames: Option<usize>) -> Result<Decoded> {
    let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .context("unrecognised audio format")?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| anyhow!("no audio track"))?;
    let track_id = track.id;
    let max = max_frames.unwrap_or(usize::MAX);
    let rate = track.codec_params.as_ref().and_then(|p| p.audio()).and_then(|a| a.sample_rate).unwrap_or(44_100);
    let too_long = move || {
        #[expect(clippy::cast_precision_loss, reason = "a frame count, shown in whole minutes")]
        let minutes = max as f64 / f64::from(rate) / 60.0;
        anyhow!("too long: about {minutes:.0} minutes of audio fit in free memory")
    };
    // Size the buffer once when the length is known, instead of doubling.
    let expected = track.num_frames.and_then(|n| usize::try_from(n).ok());
    if expected.is_some_and(|n| n > max) {
        return Err(too_long());
    }
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| anyhow!("not audio"))?;
    let lead_in = lead_in(params.codec == CODEC_ID_MP3, track.delay, rate);
    let mut samples = Vec::with_capacity(expected.unwrap_or(0).saturating_add(lead_in).saturating_mul(2));
    samples.resize(lead_in * 2, 0.0);
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;

    let mut packet_buf = Vec::new();
    let mut sample_rate = 0;
    while let Some(packet) = format.next_packet()? {
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(Error::DecodeError(_)) => continue, // skip a corrupt frame, keep going
            Err(e) => return Err(e.into()),
        };
        sample_rate = buf.spec().rate();
        let channels = buf.spec().channels().count();
        packet_buf.resize(buf.samples_interleaved(), 0.0f32);
        buf.copy_to_slice_interleaved(&mut packet_buf);
        // Always stereo: duplicate mono, keep the first two of anything wider.
        for frame in packet_buf.chunks_exact(channels) {
            let l = frame[0];
            let r = if channels > 1 { frame[1] } else { l };
            samples.extend_from_slice(&[l, r]);
        }
        if samples.len() / 2 > max {
            return Err(too_long());
        }
    }
    if samples.len() == lead_in * 2 {
        return Err(anyhow!("no audio in it"));
    }
    Ok(Decoded { samples, sample_rate, lead_in })
}
