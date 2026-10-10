//! The settings screen, CDJ utility style.
//!
//! Pages down the left, the page's settings on the right. A choice is a row of segments with the current one
//! filled, so the state reads by brightness and shape, never colour.
//!
//! Data-driven: the app builds the pages from its preferences and what it
//! knows about the system, and reads back what changed by `Key`.
//!
//! Controller › Mapping opens a drawing of the controller saying what each
//! control does, over the pages until it's closed.

use std::fmt::Write;

use kdj_engine::device::{Diagram, Shape};

use crate::diagram;
use crate::draw::{Frame, Rgba, count_f32, grey};
use crate::scene::{HEIGHT, WIDTH};
use crate::text::{Align, Atlas, Style};
use crate::touch::Target;

const BG: Rgba = grey(11);
const FG: Rgba = grey(242);
const DIM: Rgba = grey(140);
const FAINT: Rgba = grey(70);
const ROW_BG: Rgba = grey(28);

const TOP: f32 = 60.0;
const PAGE_X: f32 = 16.0;
const PAGE_W: f32 = 168.0;
const PAGE_H: f32 = 40.0;
const PANEL_X: f32 = 200.0;
const PANEL_W: f32 = WIDTH - PANEL_X - 16.0;
const ROW_H: f32 = 46.0;
/// Rows that fit above the help line.
const ROWS: usize = 7;
const SEG_H: f32 = 28.0;
const SEG_GAP: f32 = 4.0;

/// Which preference a row shows, for the app to read changes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    TempoRange,
    KeyLock,
    Quantize,
    NeedleSearch,
    LoadCue,
    Nudge,
    Output,
    Buffer,
    SampleRate,
    Link,
    LinkPeers,
    SyncAlign,
    /// This kdj's deck letter, shared with the other kdjs on the network.
    Deck,
    /// The other kdjs on the network.
    Decks,
    Controller,
    LedColour,
    Mapping,
    Learn,
    Zoom,
    PhaseMeter,
    PerfOverlay,
    Version,
    Name,
    Audio,
    Rescan,
    PowerOff,
    Restart,
}

#[derive(Clone, Debug)]
pub enum Value {
    /// One of a few options; `selected` is the current one.
    Choice { options: Vec<String>, selected: usize },
    /// Read-only.
    Info(String),
    /// Something to do (opens or runs it).
    Action(String),
}

#[derive(Clone, Debug)]
pub struct Row {
    pub key: Key,
    pub label: &'static str,
    /// Shown under the list for the highlighted row.
    pub help: &'static str,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub name: &'static str,
    pub rows: Vec<Row>,
}

/// Where the arrows go: the page list or the page's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Pages,
    Rows,
}

#[derive(Debug)]
pub struct Settings {
    pub open: bool,
    pages: Vec<Page>,
    page: usize,
    row: usize,
    column: Column,
    text: String,
    mapping: Mapping,
}

/// The controller drawing.
#[derive(Debug, Default)]
struct Mapping {
    open: bool,
    /// Laid out once, when the mapping is loaded; empty without one.
    shapes: Vec<Shape>,
}

impl Settings {
    #[must_use]
    pub fn new(pages: Vec<Page>) -> Self {
        Self {
            open: false,
            pages,
            page: 0,
            row: 0,
            column: Column::Pages,
            text: String::with_capacity(64),
            mapping: Mapping::default(),
        }
    }

    /// The controller mapping that Controller › Mapping draws.
    pub fn set_mapping(&mut self, diagram: &Diagram) {
        self.mapping.shapes = diagram.shapes();
    }

    /// Whether the controller drawing is showing.
    #[must_use]
    pub const fn mapping_open(&self) -> bool {
        self.mapping.open
    }

    /// Up / down in the current column.
    pub fn move_by(&mut self, d: isize) {
        if self.mapping.open {
            return;
        }
        match self.column {
            Column::Pages => {
                self.page = clamp_step(self.page, d, self.pages.len());
                self.row = 0;
            }
            Column::Rows => self.row = clamp_step(self.row, d, self.rows().len()),
        }
    }

    /// Right: into the rows, then change the value; left: the other way
    /// (and out of the controller drawing).
    pub fn sideways(&mut self, d: isize) {
        if self.mapping.open {
            self.mapping.open = d >= 0;
            return;
        }
        match (self.column, d.signum()) {
            (Column::Pages, 1) => self.column = Column::Rows,
            (Column::Rows, -1) if !matches!(self.current().map(|r| &r.value), Some(Value::Choice { .. })) => {
                self.column = Column::Pages;
            }
            (Column::Rows, _) => self.change(d),
            _ => {}
        }
    }

    /// Close the controller drawing, or back out to the page list; false if
    /// already there.
    pub fn back(&mut self) -> bool {
        if std::mem::take(&mut self.mapping.open) {
            return true;
        }
        std::mem::replace(&mut self.column, Column::Pages) == Column::Rows
    }

    /// Show page `i` (a tap on it).
    pub const fn select_page(&mut self, i: usize) {
        if i < self.pages.len() {
            (self.page, self.row, self.column) = (i, 0, Column::Pages);
        }
    }

    /// Set row `row`'s choice to `option` (a tap on a segment); returns its key.
    pub fn choose(&mut self, row: usize, option: usize) -> Option<Key> {
        let r = self.pages.get_mut(self.page)?.rows.get_mut(row)?;
        if let Value::Choice { options, selected } = &mut r.value
            && option < options.len()
        {
            *selected = option;
        }
        (self.row, self.column) = (row, Column::Rows);
        Some(r.key)
    }

    /// Return: run the highlighted row's action (its key), or step into the
    /// rows / to the next option. Settings opens the controller drawing
    /// itself.
    pub fn enter(&mut self) -> Option<Key> {
        if self.mapping.open {
            return None;
        }
        if let Some(&Row { key, value: Value::Action(_), .. }) = self.current() {
            return self.run(key);
        }
        self.sideways(1);
        None
    }

    /// A tap on row `row`'s action button: its key, for the app to run (None
    /// for the controller drawing, which settings opens itself).
    pub fn press(&mut self, row: usize) -> Option<Key> {
        let key = self.rows().get(row)?.key;
        (self.row, self.column) = (row, Column::Rows);
        self.run(key)
    }

    /// An action row's key for the app to run, unless it's one of ours.
    const fn run(&mut self, key: Key) -> Option<Key> {
        if matches!(key, Key::Mapping) {
            self.mapping.open = true;
            return None;
        }
        Some(key)
    }

    /// The highlighted row, if the rows have focus.
    #[must_use]
    pub fn current(&self) -> Option<&Row> {
        self.rows().get(self.row).filter(|_| self.column == Column::Rows)
    }

    /// The selected option of `key`'s row, wherever it is.
    #[must_use]
    pub fn choice(&self, key: Key) -> Option<usize> {
        self.pages.iter().flat_map(|p| &p.rows).find(|r| r.key == key).and_then(|r| match r.value {
            Value::Choice { selected, .. } => Some(selected),
            _ => None,
        })
    }

    /// Show `option` as `key`'s choice (the app changed it, not a tap).
    pub fn set_choice(&mut self, key: Key, option: usize) {
        if let Some(Value::Choice { options, selected }) = self.value_mut(key)
            && option < options.len()
        {
            *selected = option;
        }
    }

    /// Change what `key`'s read-only row says.
    pub fn set_info(&mut self, key: Key, text: String) {
        if let Some(Value::Info(s)) = self.value_mut(key) {
            *s = text;
        }
    }

    fn value_mut(&mut self, key: Key) -> Option<&mut Value> {
        self.pages.iter_mut().flat_map(|p| &mut p.rows).find(|r| r.key == key).map(|r| &mut r.value)
    }

    fn rows(&self) -> &[Row] {
        self.pages.get(self.page).map_or(&[], |p| p.rows.as_slice())
    }

    /// Step the highlighted choice by `d`, stopping at the ends.
    fn change(&mut self, d: isize) {
        let Some(Value::Choice { options, selected }) =
            self.pages.get_mut(self.page).and_then(|p| p.rows.get_mut(self.row)).map(|r| &mut r.value)
        else {
            return;
        };
        *selected = clamp_step(*selected, d, options.len());
    }

    pub fn draw(&mut self, f: &mut Frame, atlas: &Atlas, status: &str) {
        f.rect(0.0, 0.0, WIDTH, HEIGHT, BG);
        if self.mapping.open {
            self.draw_mapping(f, atlas);
            return;
        }
        atlas.draw(f, Style::Body, 16.0, 30.0, Align::Left, "Settings", FG);
        atlas.draw(f, Style::Label, WIDTH - 16.0, 30.0, Align::Right, concat!("kdj ", env!("CARGO_PKG_VERSION")), DIM);
        f.rect(16.0, 46.0, WIDTH - 32.0, 1.0, FAINT);
        self.page_list(f, atlas);
        self.rows_panel(f, atlas);
        self.footer(f, atlas, status);
    }

    /// The controller drawing, full screen; back (esc, ←, or a tap top
    /// right) returns to the Controller page.
    fn draw_mapping(&self, f: &mut Frame, atlas: &Atlas) {
        if self.mapping.shapes.is_empty() {
            atlas.draw(f, Style::Body, 16.0, 30.0, Align::Left, "Controller mapping", FG);
            f.rect(16.0, 46.0, WIDTH - 32.0, 1.0, FAINT);
            atlas.draw(f, Style::Body, 16.0, 90.0, Align::Left, "No mapping: start kdj with --midi FILE", DIM);
        } else {
            diagram::draw(f, atlas, &self.mapping.shapes);
        }
        atlas.draw(f, Style::Label, WIDTH - 16.0, 30.0, Align::Right, "esc back ×", DIM);
        f.touch.add([WIDTH - 150.0, 0.0, 150.0, 46.0], Target::Action(kdj_engine::Action::Browse));
    }

    fn page_list(&self, f: &mut Frame, atlas: &Atlas) {
        for (i, page) in self.pages.iter().enumerate() {
            let y = count_f32(i).mul_add(PAGE_H, TOP);
            let rect = [PAGE_X, y, PAGE_W, PAGE_H - 4.0];
            f.touch.add(rect, Target::Page(i));
            let base = y + 24.0;
            if i != self.page {
                atlas.draw(f, Style::Body, PAGE_X + 12.0, base, Align::Left, page.name, DIM);
            } else if self.column == Column::Pages {
                // Filled: the arrows move through pages.
                f.rect(rect[0], rect[1], rect[2], rect[3], FG);
                atlas.draw(f, Style::Body, PAGE_X + 12.0, base, Align::Left, page.name, BG);
            } else {
                // Outlined: this page, but the arrows are in its rows.
                f.outline(rect, 2.0, FG);
                atlas.draw(f, Style::Body, PAGE_X + 12.0, base, Align::Left, page.name, FG);
            }
        }
    }

    fn rows_panel(&mut self, f: &mut Frame, atlas: &Atlas) {
        let Some(page) = self.pages.get(self.page) else { return };
        let focused = (self.column == Column::Rows).then_some(self.row);
        for (i, row) in page.rows.iter().enumerate().take(ROWS) {
            let y = count_f32(i).mul_add(ROW_H, TOP);
            if focused == Some(i) {
                f.rect(PANEL_X, y, PANEL_W, ROW_H - 4.0, ROW_BG);
                f.rect(PANEL_X, y, 3.0, ROW_H - 4.0, FG);
            }
            atlas.draw(f, Style::Body, PANEL_X + 16.0, y + 27.0, Align::Left, row.label, FG);
            value(f, atlas, &mut self.text, i, y, &row.value);
        }
        if let Some(row) = focused.and_then(|i| page.rows.get(i)) {
            atlas.draw_clipped(f, Style::Label, PANEL_X + 16.0, HEIGHT - 60.0, PANEL_W - 32.0, row.help, DIM);
        }
    }

    fn footer(&self, f: &mut Frame, atlas: &Atlas, status: &str) {
        let fy = HEIGHT - 44.0;
        f.rect(0.0, fy, WIDTH, 1.0, FAINT);
        if !status.is_empty() {
            atlas.draw_clipped(f, Style::Label, 16.0, fy + 28.0, 300.0, status, FG);
        }
        let hint = match self.column {
            Column::Pages => "↑↓ page   → settings   esc close ×",
            Column::Rows => "↑↓ setting   ←→ change   ↵ open   esc back",
        };
        atlas.draw(f, Style::Label, WIDTH - 16.0, fy + 28.0, Align::Right, hint, DIM);
        f.touch.add([WIDTH - 220.0, fy + 4.0, 220.0, 40.0], Target::Action(kdj_engine::Action::Settings));
    }
}

/// A row's value, right-aligned in the panel.
fn value(f: &mut Frame, atlas: &Atlas, text: &mut String, row: usize, y: f32, value: &Value) {
    let right = PANEL_X + PANEL_W - 16.0;
    let sy = y + (ROW_H - 4.0 - SEG_H) / 2.0;
    match value {
        Value::Choice { options, selected } => {
            let widths: Vec<f32> = options.iter().map(|o| atlas.width_of(Style::Label, o) + 22.0).collect();
            let total: f32 = SEG_GAP.mul_add(count_f32(widths.len().saturating_sub(1)), widths.iter().sum::<f32>());
            let mut x = right - total;
            for (k, (o, w)) in options.iter().zip(widths).enumerate() {
                let rect = [x, sy, w, SEG_H];
                f.touch.add(rect, Target::Choice(row, k));
                if k == *selected {
                    f.rect(x, sy, w, SEG_H, FG);
                    atlas.draw(f, Style::Label, x + w / 2.0, sy + 19.0, Align::Center, o, BG);
                } else {
                    f.outline(rect, 1.0, FAINT);
                    atlas.draw(f, Style::Label, x + w / 2.0, sy + 19.0, Align::Center, o, DIM);
                }
                x += w + SEG_GAP;
            }
        }
        Value::Info(s) => {
            atlas.draw(f, Style::Label, right, sy + 19.0, Align::Right, s, DIM);
        }
        Value::Action(s) => {
            text.clear();
            let _ = write!(text, "{s}  ›");
            let w = atlas.width_of(Style::Label, text) + 24.0;
            let rect = [right - w, sy, w, SEG_H];
            f.outline(rect, 1.5, FG);
            atlas.draw(f, Style::Label, right - w / 2.0, sy + 19.0, Align::Center, text, FG);
            f.touch.add(rect, Target::Setting(row));
        }
    }
}

fn clamp_step(i: usize, d: isize, len: usize) -> usize {
    i.saturating_add_signed(d).min(len.saturating_sub(1))
}

/// Toggle options, off first.
fn on_off(on: bool) -> Value {
    Value::Choice { options: vec!["Off".into(), "On".into()], selected: usize::from(on) }
}

fn choice(options: &[&str], selected: usize) -> Value {
    Value::Choice { options: options.iter().map(|&o| o.to_owned()).collect(), selected }
}

/// The deck's preferences: what the settings screen changes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "independent on/off preferences, one per settings row")]
pub struct Prefs {
    pub wide_range: bool,
    pub keylock: bool,
    pub quantize: bool,
    /// Needle search while playing too (a CDJ locks it).
    pub needle_while_playing: bool,
    /// Park on Rekordbox's first memory cue on load (else the track start).
    pub memory_cue_on_load: bool,
    /// 0 gentle, 1 normal, 2 strong.
    pub nudge: usize,
    pub buffer_frames: u32,
    pub link: bool,
    /// Line the bars up when SYNC goes on (else only the beat).
    pub bar_sync: bool,
    /// Xone:K2/K3 LED colour: 0 red, 1 amber, 2 green.
    pub led_colour: usize,
    /// Index into the zoom levels (8, 16, 32, 64 beats).
    pub zoom: usize,
    pub phase_meter: bool,
    pub perf_overlay: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            wide_range: false,
            keylock: true,
            quantize: true,
            needle_while_playing: false,
            memory_cue_on_load: true,
            nudge: 1,
            buffer_frames: 128,
            link: true,
            bar_sync: true,
            led_colour: 0,
            zoom: 1,
            phase_meter: true,
            perf_overlay: false,
        }
    }
}

/// What the settings screen shows but doesn't set.
#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    /// This Pi's network name (kdj-word-word).
    pub hostname: String,
    pub output: String,
    pub sample_rate: u32,
    /// Link peers, if Link is on.
    pub link_peers: Option<u32>,
    /// The controller's name, or how the search for it is going.
    pub controller: String,
    /// This kdj's deck letter (0 = A).
    pub deck: usize,
    /// The other kdjs on the network.
    pub decks: String,
    pub audio_errors: u64,
}

pub const BUFFERS: [u32; 4] = [64, 128, 256, 512];

/// The settings pages for `prefs` and `info`.
#[must_use]
pub fn pages(prefs: &Prefs, info: &SystemInfo) -> Vec<Page> {
    vec![
        Page { name: "Playback", rows: playback(prefs) },
        Page { name: "Audio", rows: audio(prefs, info) },
        Page { name: "Sync", rows: sync(prefs, info) },
        Page { name: "Controller", rows: controller(prefs, info) },
        Page { name: "Display", rows: display(prefs) },
        Page { name: "System", rows: system(info) },
    ]
}

const fn row(key: Key, label: &'static str, help: &'static str, value: Value) -> Row {
    Row { key, label, help, value }
}

fn playback(p: &Prefs) -> Vec<Row> {
    vec![
        row(
            Key::TempoRange,
            "Tempo range",
            "The range a new track starts in; T switches it while playing.",
            choice(&["±8%", "±16%"], usize::from(p.wide_range)),
        ),
        row(
            Key::KeyLock,
            "Key lock",
            "Change tempo without changing pitch. The deck starts with this.",
            on_off(p.keylock),
        ),
        row(Key::Quantize, "Quantize", "Cues, loops and hot cues land on the beat.", on_off(p.quantize)),
        row(
            Key::NeedleSearch,
            "Needle search",
            "Touching the overview jumps there. A CDJ only allows it while stopped.",
            choice(&["Stopped only", "Always"], usize::from(p.needle_while_playing)),
        ),
        row(
            Key::LoadCue,
            "On load, cue at",
            "Where a new track parks: Rekordbox's first memory cue, or the start.",
            choice(&["Track start", "Memory cue"], usize::from(p.memory_cue_on_load)),
        ),
        row(
            Key::Nudge,
            "Nudge strength",
            "How hard holding nudge bends the tempo (it builds the longer you hold).",
            choice(&["Gentle", "Normal", "Strong"], p.nudge),
        ),
    ]
}

fn audio(p: &Prefs, info: &SystemInfo) -> Vec<Row> {
    let rate = info.sample_rate.max(1);
    let buffers = BUFFERS.iter().map(|&b| format!("{b} · {:.1} ms", f64::from(b) * 1000.0 / f64::from(rate))).collect();
    let selected = BUFFERS.iter().position(|&b| b == p.buffer_frames).unwrap_or(1);
    vec![
        row(Key::Output, "Output", "Set with --device; the DAC hat on the Pi.", Value::Info(info.output.clone())),
        row(
            Key::Buffer,
            "Buffer",
            "Smaller is tighter but less forgiving. 128 suits the Pi.",
            Value::Choice { options: buffers, selected },
        ),
        row(
            Key::SampleRate,
            "Sample rate",
            "Chosen at start: 48 kHz where the output offers it.",
            Value::Info(format!("{:.1} kHz", f64::from(rate) / 1000.0)),
        ),
        row(
            Key::Audio,
            "Dropouts",
            "Audio the deck couldn't deliver in time since it started.",
            Value::Info(info.audio_errors.to_string()),
        ),
    ]
}

fn sync(p: &Prefs, info: &SystemInfo) -> Vec<Row> {
    let peers = info.link_peers.map_or_else(|| "–".into(), |n| n.to_string());
    vec![
        row(
            Key::Link,
            "Ableton Link",
            "Share tempo and phase with other decks and Live on the network.",
            on_off(p.link),
        ),
        row(Key::LinkPeers, "Peers", "Other Link apps in the session.", Value::Info(peers)),
        row(
            Key::SyncAlign,
            "SYNC lines up",
            "On engaging SYNC: the bars (4 beats), or just the nearest beat.",
            choice(&["Beats", "Bars"], usize::from(p.bar_sync)),
        ),
        row(
            Key::Deck,
            "This deck",
            "Its letter: a controller's deck A side plays deck A, wherever it's plugged in.",
            choice(&["A", "B", "C", "D"], info.deck),
        ),
        row(Key::Decks, "Other decks", "Other kdjs on the network, by letter.", Value::Info(info.decks.clone())),
    ]
}

fn controller(p: &Prefs, info: &SystemInfo) -> Vec<Row> {
    vec![
        row(
            Key::Controller,
            "Controller",
            "The MIDI mapping's port; see --midi.",
            Value::Info(info.controller.clone()),
        ),
        row(
            Key::LedColour,
            "LED colour",
            "Xone:K2/K3 button colour. Lit or not is what matters.",
            choice(&["Red", "Amber", "Green"], p.led_colour),
        ),
        row(Key::Mapping, "Mapping", "What each control does, drawn on the controller.", Value::Action("Show".into())),
        row(
            Key::Learn,
            "MIDI learn",
            "Map a controller by touching each control when asked.",
            Value::Action("Learn".into()),
        ),
    ]
}

fn display(p: &Prefs) -> Vec<Row> {
    vec![
        row(
            Key::Zoom,
            "Waveform zoom",
            "Beats across the waveform for a new track; Z / X zoom while playing.",
            choice(&["8", "16", "32", "64"], p.zoom),
        ),
        row(
            Key::PhaseMeter,
            "Phase meter",
            "Against the other deck, or the Link session when alone.",
            on_off(p.phase_meter),
        ),
        row(Key::PerfOverlay, "Perf overlay", "Audio and drawing times, top right.", on_off(p.perf_overlay)),
    ]
}

fn system(info: &SystemInfo) -> Vec<Row> {
    vec![
        row(
            Key::Name,
            "Name",
            "Its name on the network: ssh in with it, and tell several kdjs apart.",
            Value::Info(format!("{}.local", info.hostname)),
        ),
        row(Key::Version, "Version", "GPL-3.0-or-later.", Value::Info(env!("CARGO_PKG_VERSION").into())),
        row(Key::Rescan, "Library", "Read the collection and USB sticks again.", Value::Action("Rescan".into())),
        row(
            Key::Restart,
            "Restart",
            "Restart the Pi. Tap twice: a stray touch does nothing.",
            Value::Action("Restart".into()),
        ),
        row(
            Key::PowerOff,
            "Power off",
            "Shut down safely before pulling the power. Tap twice.",
            Value::Action("Power off".into()),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_move_through_pages_then_rows_then_values() {
        let mut s = Settings::new(pages(&Prefs::default(), &SystemInfo::default()));
        s.move_by(1); // Audio
        s.move_by(-1); // Playback
        s.sideways(1); // into the rows: Tempo range
        assert_eq!(s.current().map(|r| r.key), Some(Key::TempoRange));
        s.sideways(1); // ±16
        assert_eq!(s.choice(Key::TempoRange), Some(1));
        s.sideways(1);
        assert_eq!(s.choice(Key::TempoRange), Some(1), "stops at the end");
        s.move_by(1);
        s.sideways(-1); // key lock off
        assert_eq!(s.choice(Key::KeyLock), Some(0));
        assert!(s.back() && !s.back());
    }

    #[test]
    fn every_segment_and_page_can_be_tapped() {
        let atlas = Atlas::new(1.0);
        let mut s = Settings::new(pages(&Prefs::default(), &SystemInfo::default()));
        let mut f = Frame::default();
        s.draw(&mut f, &atlas, "");
        let segments = f.touch.iter().filter(|t| matches!(t, Target::Choice(..))).count();
        assert_eq!(segments, 2 + 2 + 2 + 2 + 2 + 3, "every option on the Playback page");
        assert_eq!(f.touch.iter().filter(|t| matches!(t, Target::Page(_))).count(), 6);
        assert_eq!(s.choose(2, 0), Some(Key::Quantize));
        assert_eq!(s.choice(Key::Quantize), Some(0));
    }

    #[test]
    fn the_mapping_opens_from_the_controller_page_and_backs_out() {
        let mut s = Settings::new(pages(&Prefs::default(), &SystemInfo::default()));
        s.move_by(3); // Controller
        s.sideways(1);
        s.move_by(2);
        assert_eq!(s.current().map(|r| r.key), Some(Key::Mapping));
        assert_eq!(s.enter(), None, "settings opens it itself");
        assert!(s.mapping_open());
        s.move_by(1);
        let mut f = Frame::default();
        s.draw(&mut f, &Atlas::new(1.0), "");
        assert_eq!(f.touch.iter().collect::<Vec<_>>(), [Target::Action(kdj_engine::Action::Browse)], "only back");
        assert!(s.back() && !s.mapping_open());
        assert_eq!(s.current().map(|r| r.key), Some(Key::Mapping), "where it was");
        assert_eq!(s.press(3), Some(Key::Learn));
        assert_eq!(s.press(2), None);
        s.sideways(-1);
        assert!(!s.mapping_open(), "← closes it too");
    }
}
