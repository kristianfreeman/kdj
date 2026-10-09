//! File → interleaved stereo f32 at the file's own sample rate.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

pub struct Decoded {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Encoder delay trimmed from the start (MP3 LAME tag), in frames.
    pub trimmed_delay: Option<u32>,
}

impl std::fmt::Debug for Decoded {
    // Not derived: the sample buffer is millions of floats.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoded")
            .field("frames", &(self.samples.len() / 2))
            .field("sample_rate", &self.sample_rate)
            .field("trimmed_delay", &self.trimmed_delay)
            .finish_non_exhaustive()
    }
}

/// Decode the default audio track of the file at `path`.
///
/// # Errors
///
/// If the file can't be opened, isn't a format Symphonia knows, has no audio
/// track, or yields no audio at all.
pub fn decode(path: &Path) -> Result<Decoded> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .context("unrecognised audio format")?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| anyhow!("no audio track"))?;
    let track_id = track.id;
    let trimmed_delay = track.delay;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| anyhow!("not audio"))?;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;

    let mut samples = Vec::new();
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
    }
    if samples.is_empty() {
        return Err(anyhow!("no audio decoded from {}", path.display()));
    }
    Ok(Decoded { samples, sample_rate, trimmed_delay })
}
