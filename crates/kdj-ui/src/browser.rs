//! The track browser, CDJ-style.
//!
//! A full-screen list you drill into (sources, then playlists and folders,
//! then tracks) and back out of. Pure state and layout; the app decides what
//! each item opens or loads.

use std::fmt::Write;

use kdj_engine::{Action, Snapshot};

use crate::draw::{Frame, Rgba, count_f32, grey, whole};
use crate::scene::{HEIGHT, WIDTH, remaining_secs};
use crate::text::{Align, Atlas, Style};
use crate::touch::Target;

const BG: Rgba = grey(11);
const FG: Rgba = grey(242);
const DIM: Rgba = grey(140);
const FAINT: Rgba = grey(70);

pub const ROWS: usize = 12;
pub const ROW_H: f32 = 30.0;
const LIST_Y: f32 = 70.0;
/// Track list columns: artist, key, BPM (right-aligned), time (right-aligned).
const CX_ARTIST: f32 = 390.0;
const CX_KEY: f32 = 610.0;
const CX_BPM: f32 = 712.0;
const CX_TIME: f32 = WIDTH - 20.0;

#[derive(Debug)]
pub struct Row {
    pub title: String,
    pub artist: String,
    pub key: String,
    pub bpm: Option<f64>,
    pub secs: Option<f64>,
    /// Identifies the track, to mark the one on the deck.
    pub id: u64,
}

#[derive(Debug)]
pub enum Item {
    /// Something to open: a source, a folder, a playlist.
    Folder {
        name: String,
        detail: String,
    },
    Track(Row),
}

#[derive(Debug)]
pub struct Level {
    pub title: String,
    pub items: Vec<Item>,
    /// Tracks the library lists here that can't be played (audio missing).
    pub missing: usize,
    cursor: usize,
    top: usize,
}

impl Level {
    pub fn new(title: impl Into<String>, items: Vec<Item>) -> Self {
        Self { title: title.into(), items, missing: 0, cursor: 0, top: 0 }
    }
}

#[derive(Default, Debug)]
pub struct Browser {
    pub open: bool,
    levels: Vec<Level>,
    /// The track on the deck, by `Row::id`.
    pub loaded: Option<u64>,
    pub scanning: bool,
    text: String,
}

impl Browser {
    /// Empty and waiting for the first library scan.
    #[must_use]
    pub fn new(open: bool) -> Self {
        Self { open, scanning: true, ..Default::default() }
    }

    /// Start over at `root`.
    pub fn reset(&mut self, root: Level) {
        self.levels = vec![root];
    }

    /// Nothing to browse (no sources left).
    pub fn clear(&mut self) {
        self.levels.clear();
    }

    pub fn push(&mut self, level: Level) {
        self.levels.push(level);
    }

    /// Back out one level; the root stays.
    pub fn pop(&mut self) -> bool {
        if self.levels.len() > 1 {
            self.levels.pop();
            true
        } else {
            false
        }
    }

    #[must_use]
    pub const fn depth(&self) -> usize {
        self.levels.len()
    }

    /// Index of the highlighted item in the current level.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.levels.last().filter(|l| l.cursor < l.items.len()).map(|l| l.cursor)
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&Item> {
        let l = self.levels.last()?;
        l.items.get(l.cursor)
    }

    pub fn move_by(&mut self, d: isize) {
        let Some(l) = self.levels.last_mut() else { return };
        let n = l.items.len();
        if n == 0 {
            return;
        }
        l.cursor = l.cursor.saturating_add_signed(d).min(n - 1);
        if l.cursor < l.top {
            l.top = l.cursor;
        } else if l.cursor >= l.top + ROWS {
            l.top = l.cursor + 1 - ROWS;
        }
    }

    /// Highlight item `i` of the current level.
    pub fn select(&mut self, i: usize) {
        if let Some(l) = self.levels.last_mut()
            && i < l.items.len()
        {
            l.cursor = i;
        }
    }

    /// Scroll the list by `d` rows (a drag), keeping the highlight on screen.
    pub fn scroll_by(&mut self, d: isize) {
        let Some(l) = self.levels.last_mut() else { return };
        let max_top = l.items.len().saturating_sub(ROWS);
        l.top = l.top.saturating_add_signed(d).min(max_top);
        l.cursor = l.cursor.clamp(l.top, (l.top + ROWS).min(l.items.len()).saturating_sub(1).max(l.top));
    }

    pub fn draw(&mut self, f: &mut Frame, atlas: &Atlas, s: &Snapshot, status: &str) {
        self.breadcrumbs(f, atlas);
        if let Some(l) = self.levels.last() {
            let tracks = l.items.iter().any(|i| matches!(i, Item::Track(_)));
            if tracks {
                column_heads(f, atlas);
            }
        }
        f.rect(16.0, LIST_Y - 2.0, WIDTH - 32.0, 1.0, FAINT);
        self.list(f, atlas);
        self.footer(f, atlas, s, status);
    }

    /// Breadcrumbs, whole levels dropped from the left if they don't fit,
    /// and the item count.
    fn breadcrumbs(&mut self, f: &mut Frame, atlas: &Atlas) {
        let text = &mut self.text;
        if self.levels.is_empty() {
            text.clear();
            text.push_str(if self.scanning { "Reading library…" } else { "Plug in a Rekordbox USB stick" });
        } else {
            let titles: Vec<&str> = self.levels.iter().map(|l| l.title.trim()).collect();
            fit_path(text, &titles, atlas, Style::Body, WIDTH - 200.0);
        }
        if self.levels.len() > 1 {
            f.touch.add([0.0, 0.0, WIDTH - 180.0, 46.0], Target::Back);
        }
        atlas.draw(f, Style::Body, 16.0, 32.0, Align::Left, text, FG);
        if let Some(l) = self.levels.last() {
            text.clear();
            let _ = write!(text, "{} items", l.items.len());
            if l.missing > 0 {
                let _ = write!(text, " · {} missing", l.missing);
            }
            atlas.draw(f, Style::Label, WIDTH - 16.0, 32.0, Align::Right, text, DIM);
        }
    }

    /// The visible rows of the current level, and where they sit in it.
    fn list(&mut self, f: &mut Frame, atlas: &Atlas) {
        let Some(level) = self.levels.last() else {
            if !self.scanning {
                let hint = "Or put one in another kdj on the network: it shows up here too";
                atlas.draw(f, Style::Body, WIDTH / 2.0, HEIGHT / 2.0, Align::Center, hint, DIM);
            }
            return;
        };
        if level.items.is_empty() {
            let text = &mut self.text;
            text.clear();
            if level.missing > 0 {
                let _ = write!(text, "{} tracks here, none of them on this source", level.missing);
            } else {
                text.push_str("Nothing here");
            }
            atlas.draw(f, Style::Body, WIDTH / 2.0, HEIGHT / 2.0, Align::Center, text, DIM);
            return;
        }
        for (i, item) in level.items.iter().enumerate().skip(level.top).take(ROWS) {
            let y = count_f32(i - level.top).mul_add(ROW_H, LIST_Y);
            let selected = i == level.cursor;
            f.touch.add([16.0, y, WIDTH - 32.0, ROW_H], Target::Row(i));
            if selected {
                f.rect(16.0, y, WIDTH - 32.0, ROW_H - 2.0, FG);
            }
            match item {
                Item::Folder { name, detail } => folder_row(f, atlas, y, (name, detail), selected),
                Item::Track(row) => {
                    let loaded = self.loaded == Some(row.id);
                    track_row(f, atlas, &mut self.text, y, row, (selected, loaded));
                }
            }
        }
        // Scroll position.
        let len = level.items.len();
        if len > ROWS {
            let track_h = count_f32(ROWS) * ROW_H;
            let h = (track_h * count_f32(ROWS) / count_f32(len)).max(12.0);
            let y = LIST_Y + (track_h - h) * count_f32(level.top) / count_f32(len - ROWS);
            f.rect(WIDTH - 9.0, y, 3.0, h, DIM);
        }
    }

    /// Footer: the deck keeps playing while you browse, so show it.
    fn footer(&mut self, f: &mut Frame, atlas: &Atlas, s: &Snapshot, status: &str) {
        let text = &mut self.text;
        let fy = HEIGHT - 44.0;
        f.rect(0.0, fy, WIDTH, 1.0, FAINT);
        let (px, py) = (16.0, fy + 12.0);
        f.touch.add([px - 8.0, fy + 4.0, 36.0, 38.0], Target::Action(Action::PlayPause));
        // The hint corner closes the browser.
        f.touch.add([WIDTH - 220.0, fy + 4.0, 220.0, 40.0], Target::Action(Action::Browse));
        // Settings, a tap away: the title opens the browser, this the rest.
        let settings = [WIDTH / 2.0 - 70.0, fy + 8.0, 120.0, 28.0];
        f.outline(settings, 1.5, FG);
        atlas.draw(f, Style::Label, settings[0] + settings[2] / 2.0, fy + 27.0, Align::Center, "Settings  ›", FG);
        f.touch.add([settings[0], fy + 2.0, settings[2], 42.0], Target::Action(Action::Settings));
        f.tri([px, py], [px, py + 22.0], [px + 19.0, py + 11.0], FG);
        if !s.playing {
            f.tri([px + 2.5, py + 5.0], [px + 2.5, py + 17.0], [px + 13.0, py + 11.0], BG);
        }
        if s.track_frames > 0 {
            let remaining = remaining_secs(s);
            text.clear();
            let _ = write!(text, "−{:02}:{:02}   {:.2} BPM", whole(remaining / 60.0), whole(remaining) % 60, s.bpm());
            atlas.draw(f, Style::Body, 48.0, fy + 29.0, Align::Left, text, FG);
        }
        let hint = if !status.is_empty() {
            status
        } else if self.depth() > 1 {
            "↵ open / load   ← back   tab close ×"
        } else {
            "↵ open / load   tab close ×"
        };
        atlas.draw(
            f,
            Style::Label,
            WIDTH - 16.0,
            fy + 28.0,
            Align::Right,
            hint,
            if status.is_empty() { DIM } else { FG },
        );
    }
}

/// Column heads, for track lists.
fn column_heads(f: &mut Frame, atlas: &Atlas) {
    atlas.draw(f, Style::Label, 24.0, 62.0, Align::Left, "TITLE", DIM);
    atlas.draw(f, Style::Label, CX_ARTIST, 62.0, Align::Left, "ARTIST", DIM);
    atlas.draw(f, Style::Label, CX_KEY, 62.0, Align::Left, "KEY", DIM);
    atlas.draw(f, Style::Label, CX_BPM, 62.0, Align::Right, "BPM", DIM);
    atlas.draw(f, Style::Label, CX_TIME, 62.0, Align::Right, "TIME", DIM);
}

/// Text colours for a row: dark on the highlight, otherwise bright and grey.
const fn row_colors(selected: bool) -> (Rgba, Rgba) {
    if selected { (BG, BG) } else { (FG, DIM) }
}

/// A folder row at `y`: name, detail, and a chevron on the right that says "opens".
fn folder_row(f: &mut Frame, atlas: &Atlas, y: f32, (name, detail): (&str, &str), selected: bool) {
    let (fg, dim) = row_colors(selected);
    let base = y + 20.0;
    atlas.draw_clipped(f, Style::Body, 24.0, base, CX_BPM - 60.0, name, fg);
    atlas.draw(f, Style::Body, CX_TIME - 22.0, base, Align::Right, detail, dim);
    let (ax, ay) = (CX_TIME - 10.0, y + ROW_H / 2.0 - 1.0);
    f.tri([ax - 4.0, ay - 6.0], [ax - 4.0, ay + 6.0], [ax + 3.0, ay], fg);
}

/// A track row at `y`; `loaded` marks the one on the deck.
fn track_row(f: &mut Frame, atlas: &Atlas, text: &mut String, y: f32, r: &Row, (selected, loaded): (bool, bool)) {
    let (fg, dim) = row_colors(selected);
    let base = y + 20.0;
    if loaded {
        // On the deck: a solid bar in the margin.
        f.rect(6.0, y + 4.0, 5.0, ROW_H - 10.0, FG);
    }
    atlas.draw_clipped(f, Style::Body, 24.0, base, CX_ARTIST - 36.0, &r.title, fg);
    atlas.draw_clipped(f, Style::Body, CX_ARTIST, base, CX_KEY - CX_ARTIST - 16.0, &r.artist, dim);
    atlas.draw_clipped(f, Style::Body, CX_KEY, base, 44.0, &r.key, dim);
    text.clear();
    if let Some(b) = r.bpm {
        let _ = write!(text, "{b:.2}");
    }
    atlas.draw(f, Style::Body, CX_BPM, base, Align::Right, text, fg);
    text.clear();
    if let Some(secs) = r.secs {
        let _ = write!(text, "{}:{:02}", whole(secs) / 60, whole(secs) % 60);
    }
    atlas.draw(f, Style::Body, CX_TIME, base, Align::Right, text, dim);
}

/// Drop characters from the left of `text`, behind one ellipsis, until it fits `max_w`.
/// `titles` joined with `›` into `text`, dropping whole levels from the left
/// (shown as `…`) until it fits in `max_w`: "… › HOUSE NYC › Pbar Nov 23",
/// never half a name. A last level too long on its own is cut from its left.
fn fit_path(text: &mut String, titles: &[&str], atlas: &Atlas, style: Style, max_w: f32) {
    for skip in 0..titles.len() {
        text.clear();
        if skip > 0 {
            text.push_str("…  ›  ");
        }
        for (i, t) in titles[skip..].iter().enumerate() {
            if i > 0 {
                text.push_str("  ›  ");
            }
            text.push_str(t);
        }
        if atlas.width_of(style, text) <= max_w {
            return;
        }
    }
    trim_left(text, atlas, style, max_w);
}

fn trim_left(text: &mut String, atlas: &Atlas, style: Style, max_w: f32) {
    while atlas.width_of(style, text) > max_w && text.chars().count() > 2 {
        let skip = usize::from(text.starts_with('…')) + 1;
        let cut = text.char_indices().nth(skip).map_or(text.len(), |(i, _)| i);
        text.replace_range(..cut, "…");
    }
}

/// The browser level for a library node, and the node behind each item.
#[must_use]
pub fn level_for(
    libs: &[kdj_library::library::Library],
    node: kdj_library::nav::Node,
) -> (Level, Vec<kdj_library::nav::Node>) {
    use kdj_library::nav::{self, Listing};
    let (nodes, items) = nav::children(libs, node)
        .into_iter()
        .map(|(n, listing)| {
            let item = match listing {
                Listing::Folder { name, detail } => Item::Folder { name, detail },
                Listing::Track(e) => Item::Track(Row {
                    title: e.title.clone(),
                    artist: e.artist.clone(),
                    key: e.key.clone(),
                    bpm: e.bpm,
                    secs: e.secs,
                    id: nav::track_id(e),
                }),
            };
            (n, item)
        })
        .unzip();
    let mut level = Level::new(nav::title(libs, node), items);
    level.missing = nav::missing(libs, node);
    (level, nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The touchscreen's way to the settings: open the browser (tap the
    /// title), then this.
    #[test]
    fn settings_are_a_tap_away_in_the_browser() {
        let mut b = Browser::new(true);
        let mut f = Frame::default();
        b.draw(&mut f, &Atlas::new(1.0), &Snapshot::default(), "");
        assert!(f.touch.iter().any(|t| t == Target::Action(Action::Settings)), "a Settings button");
    }

    #[test]
    fn long_paths_drop_whole_levels_from_the_left() {
        let atlas = Atlas::new(1.0);
        let mut text = String::new();
        let titles = ["Sources", "USB · REKORDBOX", "PBAR CURRENT", "HOUSE NYC", "HOUSE PLAYLISTS", "Horoom April 24"];
        fit_path(&mut text, &titles, &atlas, Style::Body, 600.0);
        assert!(text.starts_with("…  ›  ") && text.ends_with("  ›  Horoom April 24"), "{text}");
        let shown: Vec<_> = text.split("  ›  ").skip(1).collect();
        assert!(shown.iter().all(|t| titles.contains(t)), "only whole names: {text}");
        assert!(atlas.width_of(Style::Body, &text) <= 600.0);
        fit_path(&mut text, &titles[..2], &atlas, Style::Body, 600.0);
        assert_eq!(text, "Sources  ›  USB · REKORDBOX", "fits: all of it");
    }

    #[test]
    fn long_breadcrumbs_are_trimmed_from_the_left() {
        let atlas = Atlas::new(1.0);
        let mut text = format!("USB · REKORDBOX  ›  {}", "x".repeat(400));
        trim_left(&mut text, &atlas, Style::Body, 600.0);
        assert!(text.starts_with("…x") && text.ends_with('x'), "{text}");
        assert!(atlas.width_of(Style::Body, &text) <= 600.0);
    }
}
