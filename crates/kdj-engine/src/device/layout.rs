//! A [`Diagram`] as shapes on an 800×480 canvas, in logical pixels, for the
//! screen and the SVG to draw the same way.
//!
//! Greys only: which deck a control drives reads by fill against outline
//! (A filled, B outlined, the focused deck a dark fill), with a legend over
//! each deck's columns; unmapped controls are faint and blank.

use super::{Diagram, Kind, Part, Placed};
use crate::midimap::Target;

/// The canvas the shapes are laid out on: the deck's screen.
pub const CANVAS: [f32; 2] = [800.0, 480.0];

// The settings screen's greys.
const BG: u8 = 11;
const FG: u8 = 242;
const DIM: u8 = 140;
const FAINT: u8 = 70;
/// A focused-deck control's fill: clearly darker than deck A's.
const FOCUSED: u8 = 52;

const MARGIN: f32 = 16.0;
const TITLE_Y: f32 = 30.0;
const RULE_Y: f32 = 46.0;
const LEGEND_Y: f32 = 68.0;
const TOP: f32 = 82.0;
const BOTTOM: f32 = CANVAS[1] - 8.0;
/// Where a knob's centre sits from its cell's left edge, and its largest radius.
const KNOB_X: f32 = 17.0;
const KNOB_R: f32 = 13.0;
/// From the cell's left edge to the label box beside a knob or fader.
const BESIDE: f32 = 38.0;
const TABLE_ROW: f32 = 24.0;
const TABLE_COL: f32 = 256.0;
const TABLE_ADDR: f32 = 64.0;

/// How to fill and outline a shape; brightness 0 (black) to 255.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub fill: Option<u8>,
    /// (width, grey), drawn inside the shape's bounds.
    pub stroke: Option<(f32, u8)>,
}

/// Which point of the text `at` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// The settings screen's two text styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    /// 13 px regular.
    Label,
    /// 16 px medium.
    Body,
}

/// One thing to draw.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// x, y, w, h.
    Rect {
        rect: [f32; 4],
        paint: Paint,
    },
    Circle {
        centre: [f32; 2],
        r: f32,
        paint: Paint,
    },
    /// `at` is on the baseline; text wider than `max_w` is cut short.
    Text {
        at: [f32; 2],
        anchor: Anchor,
        size: TextSize,
        max_w: f32,
        grey: u8,
        text: String,
    },
}

const fn fill(grey: u8) -> Paint {
    Paint { fill: Some(grey), stroke: None }
}

const fn stroke(width: f32, grey: u8) -> Paint {
    Paint { fill: None, stroke: Some((width, grey)) }
}

/// A label box's paint and text grey, by the deck it drives.
const fn style(target: Option<Target>) -> (Paint, u8) {
    match target {
        None => (stroke(1.0, FAINT), DIM),
        Some(Target::Deck(0)) => (fill(FG), BG),
        Some(Target::Deck(_)) => (stroke(2.0, FG), FG),
        Some(Target::Focused) => (fill(FOCUSED), FG),
    }
}

fn deck_name(t: Target) -> String {
    match t {
        Target::Deck(d) => format!("DECK {}", deck_letter(d)),
        Target::Focused => "FOCUSED DECK".to_owned(),
    }
}

fn deck_letter(d: usize) -> char {
    u8::try_from(d).ok().and_then(|d| b'A'.checked_add(d)).map_or('?', char::from)
}

/// Roughly how wide uppercase text is, for centring the legend; the
/// renderers measure properly where it matters.
fn rough_width(s: &str, size: TextSize) -> f32 {
    let per = match size {
        TextSize::Label => 8.0,
        TextSize::Body => 10.5,
    };
    #[expect(clippy::cast_precision_loss, reason = "a label is a few characters")]
    let n = s.chars().count() as f32;
    n * per
}

#[derive(Default)]
struct Out(Vec<Shape>);

impl Out {
    fn rect(&mut self, rect: [f32; 4], paint: Paint) {
        self.0.push(Shape::Rect { rect, paint });
    }

    fn circle(&mut self, centre: [f32; 2], r: f32, paint: Paint) {
        self.0.push(Shape::Circle { centre, r, paint });
    }

    #[expect(clippy::too_many_arguments, reason = "where, which way, how big, how wide and how bright: all needed")]
    fn text(&mut self, at: [f32; 2], anchor: Anchor, size: TextSize, max_w: f32, grey: u8, text: impl Into<String>) {
        self.0.push(Shape::Text { at, anchor, size, max_w, grey, text: text.into() });
    }

    /// A label box: what `part` does (blank if unmapped), with an optional
    /// small caption at its left (LAYER, push).
    fn label_box(&mut self, [x, y, w, h]: [f32; 4], part: Option<&Part>, caption: Option<&str>) {
        let (paint, grey) = style(part.map(|p| p.target));
        self.rect([x, y, w, h], paint);
        let base = y + h / 2.0 + 4.5;
        // The label centres in what the caption leaves.
        let mut left = x + 6.0;
        if let Some(c) = caption {
            // Secondary: dimmer than the label, on either fill.
            let dim = if paint.fill == Some(FG) { 110 } else { DIM };
            self.text([x + 7.0, base], Anchor::Start, TextSize::Label, w / 3.0, dim, c);
            left += rough_width(c, TextSize::Label) + 4.0;
        }
        if let Some(p) = part {
            let room = x + w - 6.0 - left;
            self.text([left + room / 2.0, base], Anchor::Middle, TextSize::Label, room, grey, p.label.clone());
        }
    }

    /// A knob: a ring with a pointer, bright if anything's mapped to it.
    fn knob(&mut self, centre: [f32; 2], r: f32, mapped: bool) {
        let grey = if mapped { DIM } else { FAINT };
        self.circle(centre, r, stroke(2.0, grey));
        self.rect([centre[0] - 1.0, centre[1] - r + 3.0, 2.0, r - 3.0], fill(grey));
    }
}

impl Diagram {
    /// The whole drawing on a [`CANVAS`]-sized page, background excluded:
    /// title, the legend, then the device (or a table of bindings).
    #[must_use]
    pub fn shapes(&self) -> Vec<Shape> {
        let mut out = Out::default();
        let mut title = self
            .device
            .as_ref()
            .and_then(|d| d.maker.as_deref())
            .map_or_else(|| self.title.clone(), |maker| format!("{maker} {}", self.title));
        if self.device.is_some() && !self.unplaced.is_empty() {
            title = format!("{title} · {} binding(s) not on it", self.unplaced.len());
        }
        out.text([MARGIN, TITLE_Y], Anchor::Start, TextSize::Body, CANVAS[0] - 200.0, FG, title);
        let width = 2.0f32.mul_add(-MARGIN, CANVAS[0]);
        out.rect([MARGIN, RULE_Y, width, 1.0], fill(FAINT));
        let area = [MARGIN, TOP, width, BOTTOM - TOP];
        match &self.device {
            Some(d) => {
                let scale = [area[2] / d.size[0], area[3] / d.size[1]];
                let px = |[x, y, w, h]: [f32; 4]| {
                    [x.mul_add(scale[0], area[0]), y.mul_add(scale[1], area[1]), w * scale[0], h * scale[1]]
                };
                legend(&mut out, &self.controls, scale[0], area[0]);
                for p in &self.controls {
                    control(&mut out, p, px(p.control.rect));
                }
            }
            None => self.table(&mut out, area),
        }
        out.0
    }

    /// Bindings in columns: where each comes from, and a label box.
    fn table(&self, out: &mut Out, [x0, y0, width, height]: [f32; 4]) {
        let mut targets: Vec<Target> = self.unplaced.iter().map(|e| e.part.target).collect();
        sort_targets(&mut targets);
        let mut x = x0;
        for t in targets {
            x += legend_item(out, x, t) + 24.0;
        }
        if self.unplaced.is_empty() {
            out.text([x0, y0 + 20.0], Anchor::Start, TextSize::Body, width, DIM, "No bindings in this mapping");
            return;
        }
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a few rows and columns")]
        let (rows, cols) = ((height / TABLE_ROW) as usize, (width / TABLE_COL) as usize);
        let fits = rows * cols;
        let shown = if self.unplaced.len() > fits { fits - 1 } else { self.unplaced.len() };
        let cell = |i: usize| {
            #[expect(clippy::cast_precision_loss, reason = "a few rows and columns")]
            let (col, row) = ((i / rows) as f32, (i % rows) as f32);
            [col.mul_add(TABLE_COL, x0), row.mul_add(TABLE_ROW, y0)]
        };
        for (i, e) in self.unplaced.iter().take(shown).enumerate() {
            let [x, y] = cell(i);
            out.text([x, y + 16.5], Anchor::Start, TextSize::Label, TABLE_ADDR - 6.0, DIM, e.address.clone());
            out.label_box(
                [x + TABLE_ADDR, y + 2.0, TABLE_COL - TABLE_ADDR - 8.0, TABLE_ROW - 4.0],
                Some(&e.part),
                None,
            );
        }
        if shown < self.unplaced.len() {
            let [x, y] = cell(shown);
            let more = format!("+{} more", self.unplaced.len() - shown);
            out.text([x, y + 16.5], Anchor::Start, TextSize::Label, TABLE_COL, DIM, more);
        }
    }
}

/// Decks in order, the focused deck last.
fn sort_targets(ts: &mut Vec<Target>) {
    ts.sort_by_key(|t| match t {
        Target::Deck(d) => *d,
        Target::Focused => usize::MAX,
    });
    ts.dedup();
}

/// A legend entry at `x`: a sample box and the deck's name. Returns its width.
fn legend_item(out: &mut Out, x: f32, t: Target) -> f32 {
    let (paint, _) = style(Some(t));
    out.rect([x, LEGEND_Y - 13.0, 24.0, 15.0], paint);
    let name = deck_name(t);
    let w = rough_width(&name, TextSize::Body);
    out.text([x + 32.0, LEGEND_Y], Anchor::Start, TextSize::Body, w + 20.0, FG, name);
    32.0 + w
}

/// One legend entry per deck. Over its own columns, with a rule under it
/// and a divider between neighbours, when the decks split the device into
/// sides; in a row otherwise.
fn legend(out: &mut Out, controls: &[Placed], unit: f32, x0: f32) {
    let mut spans: Vec<(Target, f32, f32)> = Vec::new();
    for p in controls {
        let [x, _, cw, _] = p.control.rect;
        for part in [&p.main, &p.push].into_iter().flatten() {
            match spans.iter_mut().find(|s| s.0 == part.target) {
                Some(s) => (s.1, s.2) = (s.1.min(x), s.2.max(x + cw)),
                None => spans.push((part.target, x, x + cw)),
            }
        }
    }
    spans.sort_by(|a, b| a.1.total_cmp(&b.1));
    let sides = spans.windows(2).all(|p| p[0].2 <= p[1].1 + 1e-3);
    if !sides {
        let mut targets: Vec<Target> = spans.iter().map(|s| s.0).collect();
        sort_targets(&mut targets);
        let mut x = x0;
        for t in targets {
            x += legend_item(out, x, t) + 24.0;
        }
        return;
    }
    for (i, &(t, from, to)) in spans.iter().enumerate() {
        let (left, right) = (from.mul_add(unit, x0), to.mul_add(unit, x0));
        let item = 32.0 + rough_width(&deck_name(t), TextSize::Body);
        legend_item(out, f32::midpoint(left, right) - item / 2.0, t);
        out.rect([left + 4.0, TOP - 7.0, right - left - 8.0, 1.0], fill(DIM));
        if spans.get(i + 1).is_some_and(|n| (n.1 - to).abs() < 1e-3) {
            out.rect([right - 0.5, LEGEND_Y - 16.0, 1.0, BOTTOM - LEGEND_Y + 16.0], fill(FAINT));
        }
    }
}

/// One control in its cell (logical pixels).
fn control(out: &mut Out, placed: &Placed, [x, y, w, h]: [f32; 4]) {
    let cy = y + h / 2.0;
    let beside = |top: f32, height: f32| [x + BESIDE, top, w - BESIDE - 4.0, height];
    match placed.control.kind {
        Kind::Button | Kind::Pad => {
            out.label_box([x + 4.0, y + 2.0, w - 8.0, h - 4.0], placed.main.as_ref(), placed.control.name.as_deref());
        }
        Kind::Encoder | Kind::Pot => {
            let radius = (h / 2.0 - 2.0).min(KNOB_R);
            out.knob([x + KNOB_X, cy], radius, placed.main.is_some() || placed.push.is_some());
            if placed.control.push.is_some() && (placed.main.is_some() || placed.push.is_some()) {
                // Turn above, push below.
                let bh = (h - 7.0) / 2.0;
                let turn = placed.main.as_ref().map(|_| "turn");
                out.label_box(beside(y + 2.0, bh), placed.main.as_ref(), turn);
                out.label_box(beside(y + 5.0 + bh, bh), placed.push.as_ref(), placed.push.as_ref().map(|_| "push"));
            } else if let Some(part) = &placed.main {
                let bh = (h - 4.0).min(24.0);
                out.label_box(beside(cy - bh / 2.0, bh), Some(part), None);
            }
        }
        Kind::Fader => {
            let grey = if placed.main.is_some() { DIM } else { FAINT };
            out.rect([x + KNOB_X - 2.0, y + 4.0, 4.0, h - 8.0], fill(grey));
            out.rect([x + KNOB_X - 11.0, cy - 6.0, 22.0, 12.0], fill(grey));
            out.rect([x + KNOB_X - 11.0, cy - 0.5, 22.0, 1.0], fill(BG));
            if let Some(part) = &placed.main {
                out.label_box(beside(cy - 12.0, 24.0), Some(part), None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midimap::MidiMap;

    #[test]
    fn the_k3_fits_the_canvas_with_a_legend_over_each_side() {
        let (_, cfg) = MidiMap::from_toml(include_str!("../../../../config/xone-k3.toml")).expect("valid");
        let shapes = Diagram::new(&cfg).shapes();
        let texts: Vec<&str> = shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"DECK A") && texts.contains(&"DECK B") && !texts.contains(&"FOCUSED DECK"));
        assert_eq!(texts.iter().filter(|&&t| t == "CUE").count(), 2);
        for s in &shapes {
            let [x, y, w, h] = match *s {
                Shape::Rect { rect, .. } => rect,
                Shape::Circle { centre: [x, y], r, .. } => [x - r, y - r, 2.0 * r, 2.0 * r],
                Shape::Text { at: [x, y], .. } => [x, y, 0.0, 0.0],
            };
            assert!(x >= 0.0 && y >= 0.0 && x + w <= CANVAS[0] && y + h <= CANVAS[1], "off the canvas: {s:?}");
        }
    }
}
