//! One-off sample-rate conversion at load time, so playback never resamples
//! for the device.

use anyhow::Result;
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

/// Interleaved stereo `input` at `from` Hz → interleaved stereo at `to` Hz.
///
/// # Errors
///
/// If the resampler can't be built for these rates or fails while running.
pub fn resample(input: Vec<f32>, from: u32, to: u32) -> Result<Vec<f32>> {
    if from == to {
        return Ok(input);
    }
    let frames = input.len() / 2;
    let mut rs = Fft::<f32>::new(from as usize, to as usize, 1024, 2, FixedSync::Both)?;
    let out_frames = rs.process_all_needed_output_len(frames);
    let mut output = vec![0.0f32; out_frames * 2];
    let src = InterleavedSlice::new(&input, 2, frames)?;
    let mut dst = InterleavedSlice::new_mut(&mut output, 2, out_frames)?;
    let (_, produced) = rs.process_all_into_buffer(&src, &mut dst, frames, None)?;
    output.truncate(produced * 2);
    Ok(output)
}
