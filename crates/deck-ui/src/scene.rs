//! The 800×480 deck screen, built from an engine snapshot.
//!
//! State reads through brightness and shape (filled vs outlined, brackets,
//! hatching), never colour alone; the only hues are the Pioneer 3-band
//! waveform's.

use std::fmt::Write;

use deck_engine::action::LOOP_SIZES;
use deck_engine::grid::BeatGrid;
use deck_engine::sync::SyncMode;
use deck_engine::{Action, Snapshot};

use crate::draw::{Frame, Rgba, Wave, alpha, count_f32, count_f64, grey, narrow, whole};
use crate::text::{Align, Atlas, Style};
use crate::touch::Target;

pub const WIDTH: f32 = 800.0;
pub const HEIGHT: f32 = 480.0;

const BG: Rgba = grey(11);
const FG: Rgba = grey(242);
const DIM: Rgba = grey(140);
const FAINT: Rgba = grey(70);

/// Detail waveform and overview strip, as x, y, w, h.
pub const DETAIL: [f32; 4] = [0.0, 44.0, WIDTH, 212.0];
pub const OVERVIEW: [f32; 4] = [16.0, 268.0, 768.0, 52.0];
const PLAYHEAD_X: f32 = WIDTH / 2.0;

/// Beats visible across the detail waveform, per zoom level.
pub const ZOOM_BEATS: [f64; 4] = [8.0, 16.0, 32.0, 64.0];
const LOW_TIME_SECS: f64 = 30.0;
/// The beat-loop buttons, in `LOOP_SIZES` order.
const BEAT_LOOPS: [Action; 6] = [
    Action::BeatLoop1,
    Action::BeatLoop2,
    Action::BeatLoop4,
    Action::BeatLoop8,
    Action::BeatLoop16,
    Action::BeatLoop32,
];
const HOT_CUE_LETTERS: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];
/// Left edge of the readouts beside the transport, and of the time column.
const READOUT_X: f32 = 96.0;
const TIME_X: f32 = 380.0;
/// Loop size buttons: left edge and spacing.
const LOOP_X: f32 = 380.0;
const LOOP_PITCH: f32 = 44.0;

/// What the screen needs to know about the loaded track.
#[derive(Clone, Copy, Debug)]
pub struct TrackInfo<'a> {
    pub title: &'a str,
    /// Frames per detail waveform column.
    pub hop: usize,
    pub columns: usize,
    /// Columns in the overview texture.
    pub overview_columns: usize,
    /// The grid came from Rekordbox rather than our detector.
    pub rekordbox_grid: bool,
}

#[derive(Debug)]
pub struct View {
    pub zoom: usize,
    /// Deck letter and whether the keyboard is on it, with more than one deck.
    pub tag: Option<(char, bool)>,
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1, tag: None }
    }
}

/// Builds frames without allocating once warmed up.
#[derive(Debug)]
pub struct Scene {
    pub frame: Frame,
    text: String,
}

impl Default for Scene {
    fn default() -> Self {
        Self { frame: Frame::default(), text: String::with_capacity(64) }
    }
}

/// Maps track frames to x on the detail waveform, centred on the playhead.
#[derive(Clone, Copy)]
struct Timeline {
    pos: f64,
    px_per_frame: f64,
}

impl Timeline {
    fn x_of(self, frame: f64) -> f32 {
        PLAYHEAD_X + narrow((frame - self.pos) * self.px_per_frame)
    }

    fn frame_at(self, px: f32) -> f64 {
        self.pos + f64::from(px - PLAYHEAD_X) / self.px_per_frame
    }
}

impl Scene {
    #[expect(
        clippy::too_many_arguments,
        reason = "everything one frame depends on, passed fresh each frame; a holder struct would only move the list"
    )]
    pub fn build(
        &mut self,
        atlas: &Atlas,
        s: &Snapshot,
        other: Option<&Snapshot>,
        track: Option<TrackInfo>,
        view: &View,
        status: &str,
        hud: &str,
    ) {
        self.frame.clear();
        let Some(t) = track.filter(|_| s.track_frames > 0) else {
            let msg = if status.is_empty() { "No track" } else { status };
            centred_message(&mut self.frame, atlas, msg);
            self.frame.touch.add([0.0, 0.0, WIDTH, HEIGHT], Target::Action(Action::Browse));
            return;
        };
        self.header(atlas, s, t, view);
        self.detail(atlas, s, t, view);
        self.phase_meter(atlas, s, other);
        self.overview(s, t);
        self.transport(atlas, s);
        self.readouts(atlas, s);
        self.loops(atlas, s);
        if !status.is_empty() {
            status_line(&mut self.frame, atlas, status);
        }
        if !hud.is_empty() {
            let f = &mut self.frame;
            let w = atlas.width_of(Style::Label, hud) + 12.0;
            f.rect(WIDTH - w - 8.0, DETAIL[1] + 4.0, w, 20.0, alpha(BG, 220));
            atlas.draw(f, Style::Label, WIDTH - 14.0, DETAIL[1] + 18.0, Align::Right, hud, FG);
        }
    }

    fn header(&mut self, atlas: &Atlas, s: &Snapshot, t: TrackInfo, view: &View) {
        let f = &mut self.frame;
        let title_x = view.tag.map_or(16.0, |(letter, focused)| {
            // Deck letter: filled when the keyboard controls this deck.
            let mut buf = [0u8; 4];
            badge_box(f, atlas, [16.0, 8.0, 30.0, 28.0], letter.encode_utf8(&mut buf), focused) + 12.0
        });
        let range = if s.wide_range { "±16" } else { "±8" };
        let badges = [
            (range, false, Action::TempoRange),
            ("Q", s.quantize, Action::Quantize),
            ("KEY", s.keylock, Action::KeyLock),
            ("SYNC", s.sync == SyncMode::Follow, Action::Sync),
            ("MASTER", s.sync == SyncMode::Lead, Action::Master),
        ];
        let mut x = WIDTH - 16.0;
        for (label, on, action) in badges {
            let left = badge(f, atlas, x, 10.0, label, on);
            f.touch.add([left, 10.0, x - left, 24.0], Target::Action(action));
            x = left - 8.0;
        }
        x -= 8.0;
        // Where the grid came from, and Link peers: plain text, not toggles.
        let text = &mut self.text;
        text.clear();
        text.push_str(if t.rekordbox_grid { "RB grid" } else { "auto grid" });
        if let Some(peers) = s.link_peers {
            let _ = write!(text, " · Link {peers}");
        }
        let src_w = atlas.draw(f, Style::Label, x, 27.0, Align::Right, text, DIM);
        let title_w = x - src_w - 16.0 - title_x;
        atlas.draw_clipped(f, Style::Body, title_x, 27.0, title_w, t.title, FG);
        // The title opens the browser, like a CDJ's BROWSE button.
        f.touch.add([title_x, 4.0, title_w, 36.0], Target::Action(Action::Browse));
    }

    fn detail(&mut self, atlas: &Atlas, snap: &Snapshot, track: TrackInfo, view: &View) {
        let [x, _, w, _] = DETAIL;
        let grid = &snap.grid;
        let beats = ZOOM_BEATS[view.zoom];
        let tl = Timeline { pos: snap.pos, px_per_frame: f64::from(w) / (beats * grid.period()) };
        let hop = count_f64(track.hop as u64);

        self.frame.waves.push(Wave {
            overview: false,
            rect: DETAIL,
            col0: narrow(tl.frame_at(x) / hop),
            cols_per_px: narrow(1.0 / (tl.px_per_frame * hop)),
            played_col: narrow(snap.pos / hop),
        });
        let f = &mut self.frame;
        if snap.loop_active {
            loop_region(f, tl, snap);
        }
        beat_grid(f, tl, grid, beats <= 32.0);
        hot_cue_flags(f, atlas, tl, snap);
        cue_marker(f, atlas, tl.x_of(snap.cue));
        playhead(f);
    }

    /// Where the other deck's beat falls relative to ours (or, with one
    /// deck, the Link session's), across ±half a beat, centred on the
    /// playhead. A marker right of centre means we're ahead. Both have to
    /// be playing for it to mean anything.
    fn phase_meter(&mut self, atlas: &Atlas, s: &Snapshot, other: Option<&Snapshot>) {
        let reading = match (other, s.link_bpm) {
            (Some(o), _) => Some(deck_phase(s, o).ok_or("other deck stopped")),
            // Alone on Link, only when there's someone to follow.
            (None, Some(bpm)) if s.link_peers.is_some_and(|n| n > 0) => {
                Some(s.link_phase.map(|p| (p, p * 60.0 / bpm * 1000.0, s.bpm() - bpm, " vs Link")).ok_or("Link"))
            }
            _ => None,
        };
        let Some(reading) = reading else { return };
        let f = &mut self.frame;
        let text = &mut self.text;
        let (cx, y, half) = (PLAYHEAD_X, DETAIL[1] + 6.0, 120.0);
        f.rect(cx - half - 4.0, y - 3.0, 2.0f32.mul_add(half, 8.0), 16.0, alpha(BG, 210));
        f.outline([cx - half, y, 2.0 * half, 10.0], 1.0, DIM);
        for q in [-0.25f32, 0.25] {
            f.rect((q * 2.0).mul_add(half, cx) - 0.5, y, 1.0, 10.0, FAINT);
        }
        f.rect(cx - 1.0, y - 2.0, 2.0, 14.0, DIM);
        let label_x = cx + half + 10.0;
        let (phase, ms, dbpm, against) = match reading {
            Ok(r) => r,
            Err(msg) => {
                f.rect(label_x - 4.0, y - 3.0, atlas.width_of(Style::Label, msg) + 8.0, 16.0, alpha(BG, 210));
                atlas.draw(f, Style::Label, label_x, y + 10.0, Align::Left, msg, DIM);
                return;
            }
        };
        let mx = (narrow(phase) * 2.0).mul_add(half, cx);
        f.rect(mx - 4.0, y - 3.0, 8.0, 16.0, BG);
        f.rect(mx - 3.0, y - 2.0, 6.0, 14.0, FG);
        text.clear();
        let _ = write!(text, "{ms:+.1} ms{against}  ·  {dbpm:+.2} BPM");
        let w = atlas.width_of(Style::Label, text);
        f.rect(label_x - 4.0, y - 3.0, w + 8.0, 16.0, alpha(BG, 210));
        atlas.draw(f, Style::Label, label_x, y + 10.0, Align::Left, text, FG);
    }

    fn overview(&mut self, snap: &Snapshot, track: TrackInfo) {
        let [x, y, w, h] = OVERVIEW;
        let total = count_f64(snap.track_frames);
        let cols = count_f32(track.overview_columns);
        let frac = |at: f64| narrow((at / total).clamp(0.0, 1.0));
        self.frame.waves.push(Wave {
            overview: true,
            rect: OVERVIEW,
            col0: 0.0,
            cols_per_px: cols / w,
            played_col: frac(snap.pos) * cols,
        });
        // x on the strip for a track position.
        let at = |frames: f64| frac(frames).mul_add(w, x);
        let frame = &mut self.frame;
        frame.touch.add(OVERVIEW, Target::Needle);
        if snap.loop_active {
            let (l0, l1) = (at(snap.loop_start), at(snap.loop_end));
            frame.hatch(l0, y, (l1 - l0).max(3.0), h, alpha(FG, 90));
            frame.outline([l0 - 1.0, y - 1.0, (l1 - l0).max(3.0) + 2.0, h + 2.0], 1.0, FG);
        }
        let cx = at(snap.cue);
        frame.tri([cx - 6.0, y + h + 7.0], [cx + 6.0, y + h + 7.0], [cx, y + h - 2.0], FG);
        // Hot cues: small downward ticks along the top edge.
        for hc in snap.hot_cues.iter().flatten() {
            let hx = at(hc.start);
            frame.tri([hx - 4.0, y - 7.0], [hx + 4.0, y - 7.0], [hx, y - 1.0], FG);
        }
        let px = at(snap.pos);
        frame.rect(px - 2.0, y - 3.0, 4.0, h + 6.0, BG);
        frame.rect(px - 1.0, y - 3.0, 2.0, h + 6.0, FG);
        frame.rect(0.0, 334.0, WIDTH, 1.0, FAINT);
    }

    fn transport(&mut self, atlas: &Atlas, s: &Snapshot) {
        let f = &mut self.frame;
        let (x, y) = (16.0, 352.0);
        // Playing: filled triangle. Paused: outline.
        f.tri([x, y], [x, y + 40.0], [x + 34.0, y + 20.0], FG);
        if !s.playing {
            f.tri([x + 3.0, y + 6.0], [x + 3.0, y + 34.0], [x + 27.0, y + 20.0], BG);
        }
        // CUE lamp: filled when parked on the cue point or previewing it.
        let at_cue = s.previewing || (!s.playing && (s.pos - s.cue).abs() < 1.0);
        badge_box(f, atlas, [x, 410.0, 54.0, 32.0], "CUE", at_cue);
        f.touch.add([x - 8.0, y - 6.0, 58.0, 52.0], Target::Action(Action::PlayPause));
        f.touch.add([x, 410.0, 54.0, 32.0], Target::Action(Action::Cue));
    }

    fn readouts(&mut self, atlas: &Atlas, s: &Snapshot) {
        self.tempo_readout(atlas, s);
        self.time_readout(atlas, s);
        self.phrase_readout(atlas, s);
    }

    /// Current BPM, the track's own, the tempo offset and the fader.
    fn tempo_readout(&mut self, atlas: &Atlas, s: &Snapshot) {
        let f = &mut self.frame;
        let text = &mut self.text;
        let x = READOUT_X;

        text.clear();
        let _ = write!(text, "{:.2}", s.bpm());
        atlas.draw(f, Style::Huge, x, 390.0, Align::Left, text, FG);
        text.clear();
        let _ = write!(text, "BPM · track {:.2}", s.grid.bpm);
        atlas.draw(f, Style::Label, x, 410.0, Align::Left, text, DIM);

        text.clear();
        if s.tempo_pct.abs() < 0.005 {
            text.push_str("0.00%");
        } else {
            let _ = write!(text, "{:+.2}%", s.tempo_pct);
        }
        let tw = atlas.draw(f, Style::Big, x, 448.0, Align::Left, text, FG);
        if s.nudge != 0 {
            nudge_arrows(f, x + tw + 10.0, f32::from(s.nudge));
        }
        tempo_fader(f, x + 120.0, s);
        atlas.draw(f, Style::Label, x, 470.0, Align::Left, "tempo", DIM);
    }

    /// Time remaining, inverted on the beat in the last 30 seconds.
    fn time_readout(&mut self, atlas: &Atlas, s: &Snapshot) {
        let f = &mut self.frame;
        let text = &mut self.text;
        let remaining = remaining_secs(s);
        text.clear();
        let _ = write!(
            text,
            "−{:02}:{:02}.{}",
            whole(remaining / 60.0),
            whole(remaining) % 60,
            whole(remaining.fract() * 10.0)
        );
        let (rx, ry) = (TIME_X, 390.0);
        let warn = s.playing && remaining < LOW_TIME_SECS && s.grid.beat_at(s.pos).rem_euclid(1.0) < 0.5;
        let rw = atlas.width_of(Style::Huge, text);
        if warn {
            f.rect(rx - 6.0, ry - 40.0, rw + 12.0, 50.0, FG);
        }
        atlas.draw(f, Style::Huge, rx, ry, Align::Left, text, if warn { BG } else { FG });
    }

    /// Bar.beat and the position in a 16-bar phrase.
    fn phrase_readout(&mut self, atlas: &Atlas, s: &Snapshot) {
        let f = &mut self.frame;
        let text = &mut self.text;
        let (bar, beat) = s.grid.bar_beat(s.pos);
        text.clear();
        let _ = write!(text, "remaining · bar {bar}.{beat}");
        atlas.draw(f, Style::Label, TIME_X, 410.0, Align::Left, text, DIM);
        let in_phrase = u8::try_from((bar - 1).rem_euclid(16)).unwrap_or(0);
        for i in 0..16u8 {
            let sx = f32::from(i).mul_add(10.5, 620.0);
            if i <= in_phrase {
                f.rect(sx, 360.0, 8.0, 8.0, FG);
            } else {
                f.outline([sx, 360.0, 8.0, 8.0], 1.0, FAINT);
            }
        }
        atlas.draw(f, Style::Label, 620.0, 384.0, Align::Left, "phrase", DIM);
    }

    fn loops(&mut self, atlas: &Atlas, s: &Snapshot) {
        let f = &mut self.frame;
        let text = &mut self.text;
        let y = 426.0;
        for ((i, size), action) in (0u8..).zip(LOOP_SIZES).zip(BEAT_LOOPS) {
            let cx = f32::from(i).mul_add(LOOP_PITCH, LOOP_X);
            f.touch.add([cx, y, 38.0, 32.0], Target::Action(action));
            #[expect(
                clippy::float_cmp,
                reason = "loop_beats is copied from LOOP_SIZES, so equality picks out the entry"
            )]
            let selected = size == s.loop_beats;
            text.clear();
            let _ = write!(text, "{size}");
            if selected && s.loop_active {
                f.rect(cx, y, 38.0, 32.0, FG);
                atlas.draw(f, Style::Body, cx + 19.0, y + 22.0, Align::Center, text, BG);
            } else {
                f.outline([cx, y, 38.0, 32.0], if selected { 2.0 } else { 1.0 }, if selected { FG } else { FAINT });
                atlas.draw(f, Style::Body, cx + 19.0, y + 22.0, Align::Center, text, if selected { FG } else { DIM });
            }
        }
        let lx = LOOP_PITCH.mul_add(6.0, LOOP_X) + 4.0;
        f.touch.add([lx, y, 112.0, 32.0], Target::Action(Action::LoopToggle));
        if s.loop_active {
            f.rect(lx, y, 112.0, 32.0, FG);
            atlas.draw(f, Style::Body, lx + 56.0, y + 22.0, Align::Center, "LOOP", BG);
        } else {
            f.outline([lx, y, 112.0, 32.0], 1.0, FAINT);
            atlas.draw(f, Style::Body, lx + 56.0, y + 22.0, Align::Center, "LOOP", DIM);
        }
    }
}

/// Our phase against another deck's, in beats within ±½, with the gap in ms
/// and BPM; None unless both are playing.
fn deck_phase(s: &Snapshot, other: &Snapshot) -> Option<(f64, f64, f64, &'static str)> {
    if !(s.playing && other.playing && s.track_frames > 0 && other.track_frames > 0) {
        return None;
    }
    // Bring the other deck's position to our audio clock before comparing.
    let other_pos = (count_f64(s.clock) - count_f64(other.clock)).mul_add(other.rate, other.pos);
    let phase = (s.grid.beat_at(s.pos) - other.grid.beat_at(other_pos) + 0.5).rem_euclid(1.0) - 0.5;
    Some((phase, phase * 60.0 / s.bpm() * 1000.0, s.bpm() - other.bpm(), ""))
}

/// The status line, bottom right under the loop buttons; cut short rather
/// than running into the time column.
fn status_line(frame: &mut Frame, atlas: &Atlas, status: &str) {
    let right = WIDTH - 16.0;
    let max_w = right - TIME_X;
    let w = atlas.width_of(Style::Label, status).min(max_w);
    atlas.draw_clipped(frame, Style::Label, right - w, HEIGHT - 8.0, max_w, status, DIM);
}

/// A message alone in the middle of the screen (no track loaded): large if it
/// fits, else smaller and cut short, so a long error never runs off screen.
fn centred_message(frame: &mut Frame, atlas: &Atlas, msg: &str) {
    let max_w = 2.0f32.mul_add(-16.0, WIDTH);
    let (x, y) = (WIDTH / 2.0, HEIGHT / 2.0);
    if atlas.width_of(Style::Big, msg) <= max_w {
        atlas.draw(frame, Style::Big, x, y, Align::Center, msg, DIM);
        return;
    }
    let w = atlas.width_of(Style::Body, msg).min(max_w);
    atlas.draw_clipped(frame, Style::Body, x - w / 2.0, y, max_w, msg, DIM);
}

/// Seconds left in the track at the current rate.
pub(crate) fn remaining_secs(s: &Snapshot) -> f64 {
    (count_f64(s.track_frames) - s.pos).max(0.0) / f64::from(s.sample_rate) / s.rate.max(0.01)
}

/// The active loop on the detail waveform: hatched, with brackets at each end.
fn loop_region(frame: &mut Frame, tl: Timeline, snap: &Snapshot) {
    let [x, y, w, h] = DETAIL;
    let (lx0, lx1) = (tl.x_of(snap.loop_start).max(x - 4.0), tl.x_of(snap.loop_end).min(x + w + 4.0));
    if lx1 > lx0 {
        frame.hatch(lx0, y, lx1 - lx0, h, alpha(FG, 60));
        bracket(frame, tl.x_of(snap.loop_start), y, h, 1.0);
        bracket(frame, tl.x_of(snap.loop_end), y, h, -1.0);
    }
}

/// Grid: bars get tall white ticks and a faint line; beats short grey ticks
/// (when `beat_ticks`).
fn beat_grid(frame: &mut Frame, tl: Timeline, grid: &BeatGrid, beat_ticks: bool) {
    let [x, y, w, h] = DETAIL;
    let (first, last) = beat_span(grid.beat_at(tl.frame_at(x)), grid.beat_at(tl.frame_at(x + w)));
    for beat in first..=last {
        let bx = tl.x_of(grid.frame_of(f64::from(beat)));
        if beat.rem_euclid(4) == 0 {
            frame.rect(bx - 0.5, y, 1.0, h, alpha(FG, 70));
            let th = if beat.rem_euclid(64) == 0 { 22.0 } else { 14.0 };
            frame.rect(bx - 1.5, y, 3.0, th, FG);
            frame.rect(bx - 1.5, y + h - th, 3.0, th, FG);
        } else if beat_ticks {
            frame.rect(bx - 0.75, y, 1.5, 7.0, DIM);
            frame.rect(bx - 0.75, y + h - 7.0, 1.5, 7.0, DIM);
        }
    }
}

/// Whole beats covering `from..to`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "beat numbers within a track are far inside i32; `as` saturates on anything absurd"
)]
const fn beat_span(from: f64, to: f64) -> (i32, i32) {
    (from.floor() as i32, to.ceil() as i32)
}

/// Hot cues: a lettered flag below the bar ticks and a faint line.
fn hot_cue_flags(frame: &mut Frame, atlas: &Atlas, tl: Timeline, snap: &Snapshot) {
    let [x, y, w, h] = DETAIL;
    for (hc, letter) in snap.hot_cues.iter().zip(HOT_CUE_LETTERS) {
        let Some(hc) = hc else { continue };
        let hx = tl.x_of(hc.start);
        if !(x - 16.0..x + w).contains(&hx) {
            continue;
        }
        frame.rect(hx - 0.5, y + 24.0, 1.0, h - 24.0, alpha(FG, 110));
        frame.rect(hx - 1.0, y + 23.0, 18.0, 18.0, BG);
        frame.rect(hx, y + 24.0, 16.0, 16.0, FG);
        atlas.draw(frame, Style::Label, hx + 8.0, y + 37.0, Align::Center, letter, BG);
        if let Some(end) = hc.end {
            // Hot loop: a bar out to the loop end along the flag's base.
            let ex = tl.x_of(end).min(x + w);
            frame.rect(hx, y + 40.0, (ex - hx).max(0.0), 2.0, FG);
        }
    }
}

/// Cue: a triangle on the bottom edge at `cx`, outlined against the waveform.
fn cue_marker(frame: &mut Frame, atlas: &Atlas, cx: f32) {
    let [x, y, w, h] = DETAIL;
    if (x..x + w).contains(&cx) {
        let base = y + h;
        frame.tri([cx - 11.0, base], [cx + 11.0, base], [cx, base - 17.0], BG);
        frame.tri([cx - 8.0, base], [cx + 8.0, base], [cx, base - 13.0], FG);
        atlas.draw(frame, Style::Label, cx, base - 20.0, Align::Center, "C", FG);
    }
}

/// Playhead: white with a dark edge, carets top and bottom; then the rule
/// under the detail waveform.
fn playhead(f: &mut Frame) {
    let [_, y, _, h] = DETAIL;
    f.rect(PLAYHEAD_X - 2.5, y, 5.0, h, BG);
    f.rect(PLAYHEAD_X - 1.5, y, 3.0, h, FG);
    f.tri([PLAYHEAD_X - 8.0, y], [PLAYHEAD_X + 8.0, y], [PLAYHEAD_X, y + 9.0], FG);
    f.tri([PLAYHEAD_X - 8.0, y + h], [PLAYHEAD_X + 8.0, y + h], [PLAYHEAD_X, y + h - 9.0], FG);
    f.rect(0.0, y + h + 4.0, WIDTH, 1.0, FAINT);
}

/// Two arrows after the tempo readout at `ax`, pointing the way `dir` nudges.
fn nudge_arrows(f: &mut Frame, ax: f32, dir: f32) {
    for i in [0.0f32, 1.0] {
        let cx = (i * 9.0).mul_add(dir, ax + 8.0);
        let back = 5.0f32.mul_add(-dir, cx);
        f.tri([back, 430.0], [back, 446.0], [4.0f32.mul_add(dir, cx), 438.0], FG);
    }
}

/// Tempo fader position starting at `bx`: centre tick, filled marker.
/// The tempo across its range: a solid marker. A hardware fader that hasn't
/// caught up with the tempo yet (soft takeover) shows as a hollow one.
fn tempo_fader(f: &mut Frame, bx: f32, s: &Snapshot) {
    let range = if s.wide_range { 16.0 } else { 8.0 };
    let (by, bw) = (458.0, 150.0);
    f.outline([bx, by, bw, 12.0], 1.0, FAINT);
    f.rect(bx + bw / 2.0 - 0.5, by - 3.0, 1.0, 18.0, DIM);
    let x_of = |pct: f64| narrow((pct / range).clamp(-1.0, 1.0)).mul_add(bw / 2.0 - 3.0, bx + bw / 2.0);
    if let Some(hw) = s.fader {
        f.outline([x_of(hw) - 4.0, by - 2.0, 8.0, 16.0], 1.5, FG);
    }
    let mx = x_of(s.tempo_pct);
    f.rect(mx - 3.0, by + 2.0, 6.0, 8.0, FG);
}

/// A loop bracket: a thick vertical bar with short arms pointing inward.
fn bracket(f: &mut Frame, x: f32, y: f32, h: f32, dir: f32) {
    f.rect(x - 1.5, y, 3.0, h, FG);
    let arm_x = if dir > 0.0 { x } else { x - 10.0 };
    f.rect(arm_x, y, 10.0, 3.0, FG);
    f.rect(arm_x, y + h - 3.0, 10.0, 3.0, FG);
}

/// A right-aligned badge ending at `right`; returns its left edge.
fn badge(f: &mut Frame, atlas: &Atlas, right: f32, y: f32, label: &str, on: bool) -> f32 {
    let w = atlas.width_of(Style::Label, label) + 18.0;
    badge_box(f, atlas, [right - w, y, w, 24.0], label, on);
    right - w
}

/// A badge in `rect` (x, y, w, h). On: filled, dark text. Off: outlined, grey
/// text. Returns the right edge.
fn badge_box(frame: &mut Frame, atlas: &Atlas, rect: [f32; 4], label: &str, on: bool) -> f32 {
    let [x, y, w, h] = rect;
    let style = if h >= 30.0 { Style::Body } else { Style::Label };
    let baseline = y + h / 2.0 + if h >= 30.0 { 6.0 } else { 4.5 };
    if on {
        frame.rect(x, y, w, h, FG);
        atlas.draw(frame, style, x + w / 2.0, baseline, Align::Center, label, BG);
    } else {
        frame.outline(rect, 1.5, DIM);
        atlas.draw(frame, style, x + w / 2.0, baseline, Align::Center, label, DIM);
    }
    x + w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x_range(f: &Frame) -> (f32, f32) {
        f.verts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v.pos[0]), hi.max(v.pos[0])))
    }

    #[test]
    fn long_messages_stay_on_screen() {
        let atlas = Atlas::new(1.0);
        let long = format!("Couldn't load: {}", "/Volumes/USB/Contents/Some Artist/Some Album/".repeat(6));
        let mut f = Frame::default();
        centred_message(&mut f, &atlas, &long);
        let (lo, hi) = x_range(&f);
        assert!(lo >= 16.0 && hi <= WIDTH - 16.0, "centred message spans {lo}..{hi}");

        f.clear();
        status_line(&mut f, &atlas, &long);
        let (lo, hi) = x_range(&f);
        assert!(lo >= TIME_X && hi <= WIDTH - 16.0, "status line spans {lo}..{hi}");
    }

    /// A snapshot of a deck with a second of silence loaded.
    fn loaded_snapshot() -> Snapshot {
        use std::sync::Arc;

        use deck_engine::track::{GridSource, HOT_CUES, Waveform};
        use deck_engine::{Engine, EngineConfig, Track};
        let track = Track {
            id: 1,
            title: "t".into(),
            sample_rate: 48_000,
            samples: vec![0.0; 96_000],
            grid: BeatGrid::new(128.0, 0.0, 48_000),
            grid_source: GridSource::Detected,
            load_cue: None,
            hot_cues: [None; HOT_CUES],
            waveform: Waveform { hop: 320, columns: Vec::new() },
        };
        let (mut engine, mut handle, _) = Engine::new(EngineConfig::default());
        assert!(handle.load(Arc::new(track)).is_ok());
        engine.process(&mut [0.0; 256]);
        handle.snapshot()
    }

    #[test]
    fn every_control_can_be_tapped_where_it_is_drawn() {
        let atlas = Atlas::new(1.0);
        let mut scene = Scene::default();
        let info = TrackInfo { title: "t", hop: 320, columns: 0, overview_columns: 0, rekordbox_grid: false };
        scene.build(&atlas, &loaded_snapshot(), None, Some(info), &View::default(), "", "");
        let at = |x: f32, y: f32| scene.frame.touch.hit(x, y).map(|h| h.target);
        assert_eq!(at(30.0, 370.0), Some(Target::Action(Action::PlayPause)));
        assert_eq!(at(40.0, 425.0), Some(Target::Action(Action::Cue)));
        assert_eq!(at(LOOP_X + 19.0, 440.0), Some(Target::Action(Action::BeatLoop1)));
        assert_eq!(at(LOOP_PITCH.mul_add(5.0, LOOP_X) + 19.0, 440.0), Some(Target::Action(Action::BeatLoop32)));
        assert_eq!(at(LOOP_PITCH.mul_add(6.0, LOOP_X) + 60.0, 440.0), Some(Target::Action(Action::LoopToggle)));
        assert_eq!(at(OVERVIEW[0] + 100.0, OVERVIEW[1] + 20.0), Some(Target::Needle));
        assert_eq!(at(WIDTH - 30.0, 22.0), Some(Target::Action(Action::TempoRange)));
        assert_eq!(at(60.0, 22.0), Some(Target::Action(Action::Browse)), "the title");
        assert_eq!(at(PLAYHEAD_X, 150.0), None, "the waveform isn't a control");
    }
}
