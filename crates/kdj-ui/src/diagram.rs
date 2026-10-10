//! Draws a controller drawing's shapes (`kdj_engine::device`) into a frame,
//! as the SVG for docs draws the same shapes.

use std::f32::consts::TAU;

use kdj_engine::device::{Anchor, Shape, TextSize};

use crate::draw::{Frame, Rgba, count_f32, grey};
use crate::text::{Align, Atlas, Style};

/// Segments in a circle: smooth at knob sizes.
const SEGMENTS: usize = 32;

pub fn draw(f: &mut Frame, atlas: &Atlas, shapes: &[Shape]) {
    for shape in shapes {
        match *shape {
            Shape::Rect { rect, paint } => {
                if let Some(g) = paint.fill {
                    f.rect(rect[0], rect[1], rect[2], rect[3], grey(g));
                }
                if let Some((w, g)) = paint.stroke {
                    f.outline(rect, w, grey(g));
                }
            }
            Shape::Circle { centre, r, paint } => {
                if let Some(g) = paint.fill {
                    ring(f, centre, 0.0, r, grey(g));
                }
                if let Some((w, g)) = paint.stroke {
                    ring(f, centre, r - w, r, grey(g));
                }
            }
            Shape::Text { at: [x, y], anchor, size, max_w, grey: g, ref text } => {
                let style = match size {
                    TextSize::Label => Style::Label,
                    TextSize::Body => Style::Body,
                };
                let (align, left) = match anchor {
                    Anchor::Start => (Align::Left, x),
                    Anchor::Middle => (Align::Center, x - max_w / 2.0),
                    Anchor::End => (Align::Right, x - max_w),
                };
                if atlas.width_of(style, text) <= max_w {
                    atlas.draw(f, style, x, y, align, text, grey(g));
                } else {
                    atlas.draw_clipped(f, style, left, y, max_w, text, grey(g));
                }
            }
        }
    }
}

/// A ring between radii `inner` and `outer` (a disc if `inner` is 0).
fn ring(f: &mut Frame, [cx, cy]: [f32; 2], inner: f32, outer: f32, color: Rgba) {
    let at = |i: usize, r: f32| {
        let (s, c) = (count_f32(i) * TAU / count_f32(SEGMENTS)).sin_cos();
        [c.mul_add(r, cx), s.mul_add(r, cy)]
    };
    for i in 0..SEGMENTS {
        let (o0, o1) = (at(i, outer), at(i + 1, outer));
        if inner <= 0.0 {
            f.tri([cx, cy], o0, o1, color);
        } else {
            let (i0, i1) = (at(i, inner), at(i + 1, inner));
            f.tri(i0, o0, o1, color);
            f.tri(i0, o1, i1, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use kdj_engine::device::Diagram;
    use kdj_engine::midimap::MidiMap;

    use super::*;

    #[test]
    fn every_k3_label_fits_its_box() {
        let atlas = Atlas::new(1.0);
        let (_, cfg) = MidiMap::from_toml(include_str!("../../../config/xone-k3.toml")).expect("valid");
        for s in Diagram::new(&cfg).shapes() {
            if let Shape::Text { size, max_w, ref text, .. } = s {
                let style = if size == TextSize::Label { Style::Label } else { Style::Body };
                assert!(atlas.width_of(style, text) <= max_w, "{text:?} is cut short");
            }
        }
    }
}
