//! Geist, pre-rasterised into one atlas at the few sizes the screen uses.
//! Digits are laid out in fixed-width cells so readouts don't jitter.

use fontdue::{Font, FontSettings, Metrics};

use crate::draw::{Fill, Frame, Rgba, count_f32};

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Geist-Regular.otf");
const MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Geist-Medium.otf");

/// Declared in the same order as `STYLES`, which indexes faces by style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Label, // 13 px regular
    Body,  // 16 px medium
    Big,   // 26 px medium
    Huge,  // 44 px medium
}

const STYLES: [(Style, bool, f32); 4] =
    [(Style::Label, false, 13.0), (Style::Body, true, 16.0), (Style::Big, true, 26.0), (Style::Huge, true, 44.0)];

/// Every style gets printable ASCII and these symbols. List text (Label,
/// Body) also gets Latin-1 and Latin Extended-A, for artist names.
const EXTRA: [char; 12] = ['±', '·', '–', '−', 'µ', '…', '↵', '←', '→', '↑', '↓', '›'];
/// Characters below this are indexed directly; EXTRA follows.
const DIRECT_END: char = '\u{180}';
const DIRECT: usize = DIRECT_END as usize;
const ATLAS: usize = 2048;

#[derive(Clone, Copy, Default, Debug)]
struct Glyph {
    /// Atlas UV rect.
    uv: [f32; 4],
    /// Offset from the pen position to the bitmap's top-left, logical px.
    dx: f32,
    dy: f32,
    w: f32,
    h: f32,
    advance: f32,
}

#[derive(Debug)]
struct Face {
    glyphs: Vec<Glyph>, // U+0000..U+0180, then EXTRA
    digit_advance: f32,
}

pub struct Atlas {
    faces: Vec<Face>,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for Atlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the pixels: a megabyte of coverage values helps nobody.
        f.debug_struct("Atlas").field("width", &self.width).field("height", &self.height).finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Atlas {
    /// Rasterise at `scale` device pixels per logical pixel.
    ///
    /// # Panics
    ///
    /// If the built-in fonts don't parse or don't fit the atlas; both are
    /// fixed at build time, so this can't happen in a working build.
    #[must_use]
    #[expect(clippy::expect_used, reason = "the fonts are compiled in: they parse unless the build itself is broken")]
    pub fn new(scale: f32) -> Self {
        let fonts: [Font; 2] = [
            Font::from_bytes(REGULAR, FontSettings::default()).expect("Geist Regular"),
            Font::from_bytes(MEDIUM, FontSettings::default()).expect("Geist Medium"),
        ];
        let width = ATLAS;
        let mut pixels = vec![0u8; width * ATLAS];
        let (mut x, mut y, mut row_h) = (1usize, 1usize, 0usize);
        // Pixel rects first; UVs once the final atlas height is known.
        let mut placed: Vec<(usize, usize, [usize; 4])> = Vec::new();
        let mut faces = Vec::new();
        for (fi, (style, medium, px)) in STYLES.into_iter().enumerate() {
            let font = &fonts[usize::from(medium)];
            let mut glyphs = vec![Glyph::default(); DIRECT + EXTRA.len()];
            let latin = matches!(style, Style::Label | Style::Body);
            let chars = (' '..='~').chain(if latin { '\u{a0}'..DIRECT_END } else { '\0'..'\0' }).chain(EXTRA);
            for ch in chars {
                if ch as u32 >= 0x80 && !font.has_glyph(ch) {
                    continue;
                }
                let (m, bitmap) = font.rasterize(ch, px * scale);
                if x + m.width + 1 >= width {
                    x = 1;
                    y += row_h + 1;
                    row_h = 0;
                }
                assert!(y + m.height < ATLAS, "glyph atlas full");
                for r in 0..m.height {
                    let dst = (y + r) * width + x;
                    pixels[dst..dst + m.width].copy_from_slice(&bitmap[r * m.width..(r + 1) * m.width]);
                }
                placed.push((fi, slot(ch), [x, y, m.width, m.height]));
                glyphs[slot(ch)] = Glyph::from_metrics(&m, scale);
                x += m.width + 1;
                row_h = row_h.max(m.height);
            }
            let digit_advance = (b'0'..=b'9').map(|d| glyphs[d as usize].advance).fold(0.0, f32::max);
            faces.push(Face { glyphs, digit_advance });
        }
        let height = (y + row_h + 1).next_power_of_two();
        pixels.truncate(width * height);
        let (aw, ah) = (count_f32(width), count_f32(height));
        for (fi, slot, [x, y, w, h]) in placed {
            faces[fi].glyphs[slot].uv =
                [count_f32(x) / aw, count_f32(y) / ah, count_f32(x + w) / aw, count_f32(y + h) / ah];
        }
        Self { faces, width, height, pixels }
    }

    fn face(&self, style: Style) -> &Face {
        // `Style` is declared in `STYLES` order.
        &self.faces[style as usize]
    }

    #[must_use]
    pub fn width_of(&self, style: Style, s: &str) -> f32 {
        let face = self.face(style);
        s.chars().map(|c| advance(face, c)).sum()
    }

    /// Draw `s` with its baseline at `y`. Returns the width.
    #[expect(
        clippy::too_many_arguments,
        reason = "a text call names where, which face, what and in what colour; ~40 call sites read best flat"
    )]
    pub fn draw(&self, frame: &mut Frame, style: Style, x: f32, y: f32, align: Align, s: &str, color: Rgba) -> f32 {
        let face = self.face(style);
        let width = self.width_of(style, s);
        let mut pen = match align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        for ch in s.chars() {
            let g = glyph(face, ch);
            let adv = advance(face, ch);
            // Centre digits in their fixed-width cell.
            let shift = if ch.is_ascii_digit() { (adv - g.advance) / 2.0 } else { 0.0 };
            if g.w > 0.0 {
                frame.quad([pen + shift + g.dx, y + g.dy, g.w, g.h], g.uv, color, Fill::Glyph);
            }
            pen += adv;
        }
        width
    }
}

impl Atlas {
    /// Draw left-aligned, cut short with "…" if wider than `max_w`.
    #[expect(
        clippy::too_many_arguments,
        reason = "same shape as `Atlas::draw`, with the width limit in place of alignment"
    )]
    pub fn draw_clipped(&self, frame: &mut Frame, style: Style, x: f32, y: f32, max_w: f32, s: &str, color: Rgba) {
        if self.width_of(style, s) <= max_w {
            self.draw(frame, style, x, y, Align::Left, s, color);
            return;
        }
        let face = self.face(style);
        let budget = max_w - advance(face, '…');
        let mut width = 0.0;
        let end = s
            .char_indices()
            .find(|&(_, c)| {
                width += advance(face, c);
                width > budget
            })
            .map_or(s.len(), |(i, _)| i);
        let pen = self.draw(frame, style, x, y, Align::Left, s[..end].trim_end(), color);
        self.draw(frame, style, x + pen, y, Align::Left, "…", color);
    }
}

impl Glyph {
    /// Placement for a rasterised glyph; UVs are filled in once the atlas is packed.
    #[expect(clippy::cast_precision_loss, reason = "glyph metrics are at most a few hundred pixels, exact in f32")]
    fn from_metrics(m: &Metrics, scale: f32) -> Self {
        Self {
            uv: [0.0; 4],
            dx: m.xmin as f32 / scale,
            dy: -(m.ymin as f32 + m.height as f32) / scale,
            w: m.width as f32 / scale,
            h: m.height as f32 / scale,
            advance: m.advance_width / scale,
        }
    }
}

fn advance(f: &Face, c: char) -> f32 {
    if c.is_ascii_digit() { f.digit_advance } else { glyph(f, c).advance }
}

/// The glyph for `c`, or "?" if this style doesn't have one.
fn glyph(f: &Face, c: char) -> Glyph {
    let g = f.glyphs[slot(c)];
    if g.advance > 0.0 || c == ' ' { g } else { f.glyphs[b'?' as usize] }
}

/// Where a character's glyph lives; anything not baked in shows as "?".
fn slot(c: char) -> usize {
    match c as usize {
        i if i < DIRECT => i,
        _ => EXTRA.iter().position(|&e| e == c).map_or(b'?' as usize, |i| DIRECT + i),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_are_listed_in_declaration_order() {
        for (i, (style, _, _)) in STYLES.into_iter().enumerate() {
            assert_eq!(style as usize, i);
        }
    }
}
