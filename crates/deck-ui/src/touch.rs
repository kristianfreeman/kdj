//! What's under a finger. Controls register their rectangles as they're
//! drawn, so hit-testing always matches the layout on screen.

use deck_engine::Action;

/// Extra reach around each control, in logical pixels: badges are 24 px
/// tall, about 3 mm on the 5" screen, so fingers get some slack.
const MARGIN: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A button: pressed on touch, released on lift.
    Action(Action),
    /// The overview strip: needle search (while stopped).
    Needle,
    /// A browser row, by index in the current level: tap to highlight, tap
    /// again to open or load; drag to scroll.
    Row(usize),
    /// Back out one browser level.
    Back,
    /// A settings page, in the page list.
    Page(usize),
    /// A settings row's option: (row, option).
    Choice(usize, usize),
    /// A settings row's action button.
    Setting(usize),
}

/// A target and the rectangle (x, y, w, h) it was drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub target: Target,
    pub rect: [f32; 4],
}

impl Hit {
    /// Where `x` falls across the target, 0 at its left edge to 1 at its right.
    #[must_use]
    pub fn fraction(&self, x: f32) -> f64 {
        let [rx, _, w, _] = self.rect;
        f64::from(((x - rx) / w.max(1.0)).clamp(0.0, 1.0))
    }
}

#[derive(Debug, Default)]
pub struct Targets {
    items: Vec<Hit>,
}

impl Targets {
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Every target, in drawing order.
    pub fn iter(&self) -> impl Iterator<Item = Target> + '_ {
        self.items.iter().map(|h| h.target)
    }

    pub fn add(&mut self, rect: [f32; 4], target: Target) {
        self.items.push(Hit { target, rect });
    }

    /// The control at (`x`, `y`): one it's inside, else the nearest within
    /// reach.
    #[must_use]
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let gap = |h: &Hit| {
            let [rx, ry, w, hh] = h.rect;
            let dx = (rx - x).max(x - (rx + w)).max(0.0);
            let dy = (ry - y).max(y - (ry + hh)).max(0.0);
            dx.hypot(dy)
        };
        self.items.iter().filter(|h| gap(h) <= MARGIN).min_by(|a, b| gap(a).total_cmp(&gap(b))).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_wins_then_the_nearest_within_reach() {
        let mut t = Targets::default();
        t.add([0.0, 0.0, 40.0, 24.0], Target::Action(Action::Sync));
        t.add([48.0, 0.0, 40.0, 24.0], Target::Action(Action::Master));
        assert_eq!(t.hit(10.0, 10.0).map(|h| h.target), Some(Target::Action(Action::Sync)));
        assert_eq!(t.hit(46.0, 30.0).map(|h| h.target), Some(Target::Action(Action::Master)), "2 px vs 6 px away");
        assert_eq!(t.hit(10.0, 40.0), None, "out of reach");
    }

    #[test]
    fn fraction_is_clamped_to_the_target() {
        let h = Hit { target: Target::Needle, rect: [100.0, 0.0, 200.0, 10.0] };
        assert!((h.fraction(200.0) - 0.5).abs() < 1e-9);
        assert!((h.fraction(50.0)).abs() < 1e-9);
        assert!((h.fraction(400.0) - 1.0).abs() < 1e-9);
    }
}
