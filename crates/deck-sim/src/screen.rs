//! The real UI renderer in a hidden window, for PNG frames and GPU timing.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use deck_engine::{Snapshot, Track};
use deck_ui::Ui;

use crate::alloc_guard;
use crate::perf::UiFrames;

pub struct Screen {
    pub ui: Ui,
    _ctx: sdl2::video::GLContext,
    _window: sdl2::video::Window,
    video: sdl2::VideoSubsystem,
    _sdl: sdl2::Sdl,
    track_id: u64,
    scale: f32,
}

impl Screen {
    pub fn new(scale: f32) -> Result<Self> {
        let sdl = sdl2::init().map_err(|e| anyhow!(e))?;
        let video = sdl.video().map_err(|e| anyhow!(e))?;
        let attr = video.gl_attr();
        attr.set_context_profile(sdl2::video::GLProfile::Core);
        attr.set_context_version(3, 3);
        attr.set_context_flags().forward_compatible().set();
        let window = video.window("deck-sim", 64, 64).opengl().hidden().build()?;
        let ctx = window.gl_create_context().map_err(|e| anyhow!(e))?;
        // SAFETY: the GL context created just above is current on this thread,
        // and SDL's loader returns entry points for exactly that context.
        let gl = unsafe { glow::Context::from_loader_function(|s| video.gl_get_proc_address(s).cast()) };
        let ui = Ui::new(gl, false, scale)?;
        Ok(Self { ui, _ctx: ctx, _window: window, video, _sdl: sdl, track_id: 0, scale })
    }

    /// Another deck's UI on the same GL context, as the app makes one per deck.
    fn another_ui(&self) -> Result<Ui> {
        // SAFETY: our GL context is current on this thread (it was made
        // current in `new` and nothing else is), and SDL's loader returns
        // entry points for it.
        let gl = unsafe { glow::Context::from_loader_function(|s| self.video.gl_get_proc_address(s).cast()) };
        Ui::new(gl, false, self.scale)
    }

    fn sync_track(&mut self, track: &Arc<Track>) {
        if self.track_id != track.id {
            self.ui.set_track(track);
            self.track_id = track.id;
        }
    }

    pub fn save_png(&mut self, path: &Path, snap: &Snapshot, track: &Arc<Track>) -> Result<()> {
        let (w, h, rgba) = self.render(snap, None, track, None)?;
        write_png(path, w, h, &rgba)
    }

    /// Render one deck screen; `other` drives the phase meter, `tag` the deck letter.
    pub fn render(
        &mut self,
        snap: &Snapshot,
        other: Option<&Snapshot>,
        track: &Arc<Track>,
        tag: Option<(char, bool)>,
    ) -> Result<(u32, u32, Vec<u8>)> {
        self.render_status(snap, other, track, tag, "")
    }

    /// As `render`, with a status line.
    pub fn render_status(
        &mut self,
        snap: &Snapshot,
        other: Option<&Snapshot>,
        track: &Arc<Track>,
        tag: Option<(char, bool)>,
        status: &str,
    ) -> Result<(u32, u32, Vec<u8>)> {
        self.sync_track(track);
        self.ui.view.tag = tag;
        self.ui.build(snap, other, deck_ui::Overlay::None, status, "");
        self.ui.snapshot_rgba()
    }

    /// Render the browser over `snap` (the deck playing underneath).
    pub fn save_browser_png(
        &mut self,
        path: &Path,
        browser: &mut deck_ui::browser::Browser,
        snap: &Snapshot,
    ) -> Result<()> {
        self.save_browser_status_png(path, browser, snap, "")
    }

    /// As `save_browser_png`, with a status line.
    pub fn save_browser_status_png(
        &mut self,
        path: &Path,
        browser: &mut deck_ui::browser::Browser,
        snap: &Snapshot,
        status: &str,
    ) -> Result<()> {
        self.ui.build(snap, None, deck_ui::Overlay::Browser(browser), status, "");
        let (w, h, rgba) = self.ui.snapshot_rgba()?;
        write_png(path, w, h, &rgba)
    }

    /// Render the settings screen.
    pub fn save_settings_png(
        &mut self,
        path: &Path,
        settings: &mut deck_ui::settings::Settings,
        status: &str,
    ) -> Result<()> {
        settings.open = true;
        self.ui.build(&Snapshot::default(), None, deck_ui::Overlay::Settings(settings), status, "");
        let (w, h, rgba) = self.ui.snapshot_rgba()?;
        write_png(path, w, h, &rgba)
    }

    /// A two-deck frame as the app makes it: clear, then build and draw each
    /// deck into its half (each deck's phase meter reading the other). Times
    /// the CPU side of `frames` after `warmup` unmeasured ones, and counts
    /// heap allocations in the measured part. Waits for the GPU between
    /// frames, outside the timing, so one frame's queue doesn't bill the next.
    pub fn bench_two_decks(&mut self, track: &Arc<Track>, frames: &[[Snapshot; 2]], warmup: usize) -> Result<UiFrames> {
        self.sync_track(track);
        let mut b = self.another_ui()?;
        b.set_track(track);
        self.ui.view.tag = Some(('A', true));
        b.view.tag = Some(('B', false));
        let size = (800, 960);
        let mut out = UiFrames { ms: Vec::with_capacity(frames.len()), allocs: 0 };
        for (i, [sa, sb]) in frames.iter().enumerate() {
            let (elapsed, allocs) = alloc_guard::count(|| {
                let t = Instant::now();
                self.ui.gpu.clear_all(size);
                self.ui.build(sa, Some(sb), deck_ui::Overlay::None, "", "");
                self.ui.draw_slot(size, 0, 2);
                b.build(sb, Some(sa), deck_ui::Overlay::None, "", "");
                b.draw_slot(size, 1, 2);
                t.elapsed()
            });
            self.ui.gpu.finish();
            if i >= warmup {
                out.ms.push(elapsed.as_secs_f64() * 1000.0);
                out.allocs += allocs;
            }
        }
        Ok(out)
    }
}

pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}
