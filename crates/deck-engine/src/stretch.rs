//! Key lock: tempo changes without pitch changes, via Signalsmith Stretch.
//!
//! Kept behind this small wrapper so another stretcher can be swapped in and
//! compared by ear. `process` and `seek` don't allocate once constructed
//! (checked in the C++ source: all buffers are sized in `configure`).

pub struct Stretcher {
    inner: signalsmith_stretch::Stretch,
    preroll: usize,
}

/// Signalsmith's presets. The deck plays with `Cheaper`; `deck-sim ab`
/// renders both for listening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// 100 ms blocks, 40 ms interval, computation split across calls.
    Cheaper,
    /// 120 ms blocks, 30 ms interval, all in one go: about 7× the worst-case
    /// callback cost.
    Default,
}

impl std::fmt::Debug for Stretcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stretcher").field("preroll", &self.preroll).finish_non_exhaustive()
    }
}

impl Stretcher {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self::with_quality(sample_rate, Quality::Cheaper)
    }

    #[must_use]
    pub fn with_quality(sample_rate: u32, quality: Quality) -> Self {
        // presetCheaper: 100 ms block, 40 ms interval, and split computation,
        // which spreads each FFT step across calls instead of landing it all
        // in one callback. Measured: callback p99 dropped from 148 µs to 22 µs
        // on an M4 (presetDefault doesn't split). Costs one interval of
        // latency, which `lead` compensates for.
        let inner = match quality {
            Quality::Cheaper => signalsmith_stretch::Stretch::preset_cheaper(2, sample_rate),
            Quality::Default => signalsmith_stretch::Stretch::preset_default(2, sample_rate),
        };
        // Seek wants at least one block plus one interval of history.
        let preroll = (sample_rate as usize * 15).div_ceil(100) + 1;
        Self { inner, preroll }
    }

    /// Source frames the stretcher reads ahead of what's audible, at a steady `rate`.
    #[must_use]
    pub fn lead(&self, rate: f64) -> f64 {
        self.output_latency().mul_add(rate, self.input_latency())
    }

    /// Output frames between feeding a frame and hearing it.
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "latencies are a few thousand frames, far below f64's 2^52")]
    pub fn output_latency(&self) -> f64 {
        self.inner.output_latency() as f64
    }

    /// Source frames held back before any output is made from them.
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "latencies are a few thousand frames, far below f64's 2^52")]
    pub fn input_latency(&self) -> f64 {
        self.inner.input_latency() as f64
    }

    /// Frames of history `seek` wants.
    #[must_use]
    pub const fn preroll(&self) -> usize {
        self.preroll
    }

    /// Restart from new material. `input` is interleaved stereo ending at the
    /// new feed position, so output starts right at the audible target.
    pub fn seek(&mut self, input: &[f32], rate: f64) {
        self.inner.reset();
        self.inner.seek(input, rate);
    }

    /// `input.len() / output.len()` sets the stretch ratio for this block.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.inner.process(input, output);
    }
}

/// Source frames fed against output frames rendered, lately.
///
/// What's audible now was fed one output latency ago, so the playhead advances at the rate fed back then: with a
/// tempo change, the display, quantize and sync all stay on the audio instead
/// of running `output_latency × Δrate` frames off it.
///
/// Fixed-size and allocation-free after construction.
#[derive(Debug)]
pub struct FeedHistory {
    /// (output frames, source frames) rendered so far, at chunk ends.
    points: Box<[(f64, f64)]>,
    len: usize,
    next: usize,
    /// The rate before the oldest point: the rate at the last restart, or
    /// of the newest segment that's been dropped.
    before: f64,
    out: f64,
    fed: f64,
}

impl FeedHistory {
    /// Room for `chunks` chunks; enough to span the output latency at the
    /// smallest expected callback size.
    #[must_use]
    pub fn new(chunks: usize) -> Self {
        let mut h = Self {
            points: vec![(0.0, 0.0); chunks.max(2)].into_boxed_slice(),
            len: 0,
            next: 0,
            before: 1.0,
            out: 0.0,
            fed: 0.0,
        };
        h.reset(1.0);
        h
    }

    /// Start over: everything before now was fed at a steady `rate`.
    pub fn reset(&mut self, rate: f64) {
        self.len = 0;
        self.next = 0;
        self.before = rate;
        self.out = 0.0;
        self.fed = 0.0;
        self.push_point();
    }

    /// `n_out` frames were rendered from `n_in` source frames.
    pub fn push(&mut self, n_out: f64, n_in: f64) {
        self.out += n_out;
        self.fed += n_in;
        self.push_point();
    }

    /// Output frames rendered since the last reset.
    #[must_use]
    pub const fn out(&self) -> f64 {
        self.out
    }

    /// Source frames fed by the time `t` output frames were rendered.
    #[must_use]
    pub fn fed_at(&self, t: f64) -> f64 {
        // Nothing is known past now.
        let t = t.min(self.out);
        let mut later: Option<(f64, f64)> = None;
        for i in (0..self.len).rev() {
            let Some(&(o, f)) = self.points.get(self.index(i)) else { break };
            if o <= t {
                return match later {
                    Some((o2, f2)) if o2 > o => (f2 - f).mul_add((t - o) / (o2 - o), f),
                    _ => f,
                };
            }
            later = Some((o, f));
        }
        // Before the oldest point.
        let (o, f) = later.unwrap_or((0.0, 0.0));
        (t - o).mul_add(self.before, f)
    }

    fn index(&self, i: usize) -> usize {
        // i counts from the oldest point.
        (self.next + self.points.len() - self.len + i) % self.points.len()
    }

    fn push_point(&mut self) {
        if self.len == self.points.len() {
            // Dropping the oldest: remember its segment's rate.
            if let (Some(&(o0, f0)), Some(&(o1, f1))) = (self.points.get(self.index(0)), self.points.get(self.index(1)))
                && o1 > o0
            {
                self.before = (f1 - f0) / (o1 - o0);
            }
            self.len -= 1;
        }
        if let Some(p) = self.points.get_mut(self.next) {
            *p = (self.out, self.fed);
        }
        self.next = (self.next + 1) % self.points.len();
        self.len += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::FeedHistory;

    #[test]
    fn interpolates_and_extrapolates() {
        let mut h = FeedHistory::new(8);
        h.reset(1.0);
        h.push(100.0, 100.0);
        h.push(100.0, 110.0);
        assert!((h.fed_at(-50.0) + 50.0).abs() < 1e-9, "before the restart: the restart rate");
        assert!((h.fed_at(50.0) - 50.0).abs() < 1e-9);
        assert!((h.fed_at(150.0) - 155.0).abs() < 1e-9);
        assert!((h.fed_at(200.0) - 210.0).abs() < 1e-9);
    }

    #[test]
    fn a_full_ring_keeps_the_dropped_rate() {
        let mut h = FeedHistory::new(4);
        h.reset(1.0);
        for _ in 0..10 {
            h.push(10.0, 12.0);
        }
        assert!((h.fed_at(0.0) - 0.0).abs() < 1e-9, "extrapolated at 1.2 from the oldest kept point");
        assert!((h.fed_at(100.0) - 120.0).abs() < 1e-9);
    }
}
