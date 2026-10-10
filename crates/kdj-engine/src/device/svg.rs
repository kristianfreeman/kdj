//! The drawing as SVG, for docs: the same shapes the screen draws.

use std::fmt::Write as _;

use super::Diagram;
use super::layout::{Anchor, CANVAS, Paint, Shape, TextSize};

impl Diagram {
    /// A standalone SVG of [`Diagram::shapes`] on the screen's background.
    #[must_use]
    pub fn svg(&self) -> String {
        let [width, height] = CANVAS;
        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" \
             font-family=\"Geist, 'Helvetica Neue', Arial, sans-serif\">\n\
             <title>{} mapping</title>\n<rect width=\"{width}\" height=\"{height}\" fill=\"{}\"/>\n",
            esc(&self.title),
            rgb(11)
        );
        for shape in self.shapes() {
            match shape {
                Shape::Rect { rect: [x, y, w, h], paint } => {
                    // SVG strokes straddle the edge; ours sit inside it.
                    let inset = paint.stroke.map_or(0.0, |(sw, _)| sw / 2.0);
                    let _ = writeln!(
                        out,
                        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                        n(x + inset),
                        n(y + inset),
                        n(2.0f32.mul_add(-inset, w)),
                        n(2.0f32.mul_add(-inset, h)),
                        paint_attrs(paint)
                    );
                }
                Shape::Circle { centre: [x, y], r, paint } => {
                    let inset = paint.stroke.map_or(0.0, |(sw, _)| sw / 2.0);
                    let _ = writeln!(
                        out,
                        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"{}/>",
                        n(x),
                        n(y),
                        n(r - inset),
                        paint_attrs(paint)
                    );
                }
                Shape::Text { at: [x, y], anchor, size, grey, text, .. } => {
                    let anchor = match anchor {
                        Anchor::Start => "start",
                        Anchor::Middle => "middle",
                        Anchor::End => "end",
                    };
                    let (px, weight) = match size {
                        TextSize::Label => (13, 400),
                        TextSize::Body => (16, 500),
                    };
                    let _ = writeln!(
                        out,
                        "<text x=\"{}\" y=\"{}\" font-size=\"{px}\" font-weight=\"{weight}\" text-anchor=\"{anchor}\" \
                         fill=\"{}\">{}</text>",
                        n(x),
                        n(y),
                        rgb(grey),
                        esc(&text)
                    );
                }
            }
        }
        out.push_str("</svg>\n");
        out
    }
}

fn paint_attrs(p: Paint) -> String {
    let mut a = format!(" fill=\"{}\"", p.fill.map_or_else(|| "none".to_owned(), rgb));
    if let Some((w, g)) = p.stroke {
        let _ = write!(a, " stroke=\"{}\" stroke-width=\"{}\"", rgb(g), n(w));
    }
    a
}

fn rgb(g: u8) -> String {
    format!("#{g:02x}{g:02x}{g:02x}")
}

/// A coordinate to a tenth of a pixel, without a trailing ".0".
fn n(v: f32) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map_or_else(|| s.clone(), str::to_owned)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use crate::device::Diagram;
    use crate::midimap::MidiMap;

    #[test]
    fn the_svg_has_the_labels_and_closes() {
        let (_, cfg) = MidiMap::from_toml(include_str!("../../../../config/xone-k3.toml")).expect("valid");
        let svg = Diagram::new(&cfg).svg();
        assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>\n"));
        assert!(svg.contains(">CUE</text>") && svg.contains(">Allen &amp; Heath Xone:K3</text>"));
    }
}
