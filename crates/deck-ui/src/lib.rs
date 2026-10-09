//! deck UI: the 800×480 screen. Window-system agnostic: give it a GL
//! context and it draws; the app (SDL2) and the simulator (offscreen) share it.

pub mod browser;
pub mod draw;
pub mod gpu;
pub mod scene;
pub mod text;
pub mod touch;

use std::time::{Duration, Instant};

use anyhow::Result;
use deck_engine::{Snapshot, Track};

pub use scene::{HEIGHT, OVERVIEW, View, WIDTH, ZOOM_BEATS};

#[derive(Debug)]
pub struct Ui {
    pub gpu: gpu::Gpu,
    pub atlas: text::Atlas,
    pub scene: scene::Scene,
    pub view: View,
    pub scale: f32,
    track: Option<(String, usize, usize, usize, bool)>,
}

impl Ui {
    /// `scale`: device pixels per logical pixel (2 on a Retina Mac, 1 on the Pi).
    ///
    /// # Errors
    ///
    /// If the GPU setup fails (see [`gpu::Gpu::new`]).
    pub fn new(gl: glow::Context, es: bool, scale: f32) -> Result<Self> {
        let mut gpu = gpu::Gpu::new(gl, es)?;
        let atlas = text::Atlas::new(scale);
        gpu.upload_atlas(atlas.width, atlas.height, &atlas.pixels);
        Ok(Self { gpu, atlas, scene: scene::Scene::default(), view: View::default(), scale, track: None })
    }

    pub fn set_track(&mut self, t: &Track) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the overview strip's width in whole device pixels: positive and small"
        )]
        let overview = t.waveform.overview((OVERVIEW[2] * self.scale) as usize);
        self.gpu.upload_waveforms(&t.waveform.columns, &overview);
        let rb = t.grid_source == deck_engine::track::GridSource::Rekordbox;
        self.track = Some((t.title.clone(), t.waveform.hop, t.waveform.columns.len(), overview.len(), rb));
    }

    pub fn zoom(&mut self, out: bool) {
        self.view.zoom =
            if out { (self.view.zoom + 1).min(ZOOM_BEATS.len() - 1) } else { self.view.zoom.saturating_sub(1) };
    }

    /// Build this frame's draw data: the browser if it's open, otherwise the
    /// deck. Returns the CPU time it took.
    /// `other`: the other deck, when two run side by side (for the phase meter).
    pub fn build(
        &mut self,
        snap: &Snapshot,
        other: Option<&Snapshot>,
        browser: Option<&mut browser::Browser>,
        status: &str,
        hud: &str,
    ) -> Duration {
        let t = Instant::now();
        if let Some(b) = browser.filter(|b| b.open) {
            self.scene.frame.clear();
            b.draw(&mut self.scene.frame, &self.atlas, snap, status);
            return t.elapsed();
        }
        let info = self.track.as_ref().map(|(title, hop, cols, ov, rb)| scene::TrackInfo {
            title,
            hop: *hop,
            columns: *cols,
            overview_columns: *ov,
            rekordbox_grid: *rb,
        });
        self.scene.build(&self.atlas, snap, other, info, &self.view, status, hud);
        t.elapsed()
    }

    /// Draw the last built frame, letterboxed into a `size` device-pixel surface.
    pub fn draw(&mut self, size: (u32, u32)) {
        self.draw_slot(size, 0, 1);
    }

    /// Draw into slot `slot` of `slots` deck screens stacked top to bottom.
    pub fn draw_slot(&mut self, size: (u32, u32), slot: usize, slots: usize) {
        let (fit, [w, h]) = letterbox(size, slots);
        let (sw, sh) = (to_i32(size.0), to_i32(size.1));
        let left = (sw - w) / 2;
        let top = (sh - h * to_i32(slots)) / 2;
        // GL counts y from the bottom.
        let bottom = sh - top - h * (to_i32(slot) + 1);
        self.gpu.draw(&self.scene.frame, [left, bottom, w, h], [WIDTH, HEIGHT], fit);
    }

    /// The control at a logical point of the last built frame.
    #[must_use]
    pub fn hit(&self, x: f32, y: f32) -> Option<touch::Hit> {
        self.scene.frame.touch.hit(x, y)
    }

    /// Render the last built frame offscreen as RGBA.
    ///
    /// # Errors
    ///
    /// If GL can't create the offscreen framebuffer.
    pub fn snapshot_rgba(&mut self) -> Result<(u32, u32, Vec<u8>)> {
        self.gpu.render_offscreen(&self.scene.frame, [WIDTH, HEIGHT], self.scale)
    }
}

/// Which of `slots` stacked deck screens a device-pixel point on a `size`
/// surface falls in, and where on that screen in logical pixels (the inverse
/// of `Ui::draw_slot`). None in the letterbox margins.
#[must_use]
pub fn to_logical(size: (u32, u32), slots: usize, px: [f32; 2]) -> Option<(usize, [f32; 2])> {
    let (fit, [w, h]) = letterbox(size, slots);
    let (sw, sh) = (to_i32(size.0), to_i32(size.1));
    #[expect(clippy::cast_precision_loss, reason = "window sizes in pixels are far below 2^24")]
    let (left, top) = (((sw - w) / 2) as f32, ((sh - h * to_i32(slots)) / 2) as f32);
    let (x, y) = ((px[0] - left) / fit, (px[1] - top) / fit);
    if !(0.0..WIDTH).contains(&x) || y < 0.0 {
        return None;
    }
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "non-negative and small")]
    let slot = (y / HEIGHT) as usize;
    #[expect(clippy::cast_precision_loss, reason = "a slot index is tiny")]
    let y = (slot as f32).mul_add(-HEIGHT, y);
    (slot < slots).then_some((slot, [x, y]))
}

/// The scale that fits `n` stacked screens into a `size` surface, and one
/// screen's size in whole device pixels at that scale.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "window sizes and deck counts are tiny next to f32's 2^24 exact range; whole pixels are intended"
)]
fn letterbox(size: (u32, u32), n: usize) -> (f32, [i32; 2]) {
    let fit = (size.0 as f32 / WIDTH).min(size.1 as f32 / (HEIGHT * n as f32));
    (fit, [(WIDTH * fit) as i32, (HEIGHT * fit) as i32])
}

/// A size or count as a signed pixel value; saturates instead of wrapping.
fn to_i32(n: impl TryInto<i32>) -> i32 {
    n.try_into().unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_map_back_to_the_slot_they_were_drawn_in() {
        // Two decks on a 1600x1920 surface: scale 2, no margins.
        assert_eq!(to_logical((1600, 1920), 2, [800.0, 100.0]), Some((0, [400.0, 50.0])));
        assert_eq!(to_logical((1600, 1920), 2, [20.0, 1000.0]), Some((1, [10.0, 20.0])));
        // One deck letterboxed into a wide window: 200 px margins each side.
        assert_eq!(to_logical((1200, 480), 1, [100.0, 10.0]), None);
        assert_eq!(to_logical((1200, 480), 1, [200.0, 10.0]), Some((0, [0.0, 10.0])));
    }
}
