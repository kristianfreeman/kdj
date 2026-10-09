//! Reading audio out of a track: fractional positions, loop wrap, and a short
//! crossfade at the loop seam so loops never click.

use crate::track::Track;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loop {
    pub start: f64,
    pub end: f64,
}

impl Loop {
    #[must_use]
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    /// Fold a position that ran past the end back into the loop.
    #[must_use]
    pub fn wrap(&self, pos: f64) -> f64 {
        if pos >= self.end { self.start + (pos - self.start).rem_euclid(self.len()) } else { pos }
    }
}

/// Crossfade length at the loop seam (~2.7 ms at 48 kHz).
const SEAM_FRAMES: f64 = 128.0;

/// Fill `out` (interleaved stereo) from `cursor`, stepping `step` source frames
/// per output frame. Positions outside the track read as silence.
pub fn read(track: &Track, lp: Option<Loop>, cursor: &mut f64, step: f64, out: &mut [f32]) {
    let samples = &track.samples;
    for frame in out.as_chunks_mut::<2>().0 {
        let (mut left, mut right) = sample_at(samples, *cursor);
        if let Some(lp) = lp {
            // Approaching the end: blend toward the audio just before the loop
            // start, which flows seamlessly into the start when we wrap.
            let to_end = lp.end - *cursor;
            if to_end > 0.0 && to_end < SEAM_FRAMES {
                #[expect(clippy::cast_possible_truncation, reason = "a 0..1 blend weight; f32 precision is plenty")]
                let weight = (1.0 - to_end / SEAM_FRAMES) as f32;
                let (left2, right2) = sample_at(samples, *cursor - lp.len());
                left = left2.mul_add(weight, left * (1.0 - weight));
                right = right2.mul_add(weight, right * (1.0 - weight));
            }
        }
        *frame = [left, right];
        *cursor += step;
        if let Some(lp) = lp {
            *cursor = lp.wrap(*cursor);
        }
    }
}

/// Stereo sample at a fractional frame position (cubic Hermite between frames).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the fraction only needs f32 precision, and the floored frame index saturates far outside any track, reading silence"
)]
fn sample_at(samples: &[f32], pos: f64) -> (f32, f32) {
    let whole = pos.floor();
    let frac = (pos - whole) as f32;
    let base = whole as isize;
    let get = |offset: isize| -> (f32, f32) {
        match usize::try_from((base + offset) * 2).ok().and_then(|idx| samples.get(idx..idx + 2)) {
            Some(&[left, right]) => (left, right),
            _ => (0.0, 0.0),
        }
    };
    if frac == 0.0 {
        return get(0);
    }
    let (y0, y1, y2, y3) = (get(-1), get(0), get(1), get(2));
    (hermite(y0.0, y1.0, y2.0, y3.0, frac), hermite(y0.1, y1.1, y2.1, y3.1, frac))
}

fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = 0.5f32.mul_add(-y3, 2.0f32.mul_add(y2, 2.5f32.mul_add(-y1, y0)));
    let c3 = 1.5f32.mul_add(y1 - y2, 0.5 * (y3 - y0));
    c3.mul_add(t, c2).mul_add(t, c1).mul_add(t, y1)
}
