//! Touchscreen input. SDL turns touches into mouse events, on the Pi's
//! touchscreen as with a mouse on a Mac, so mouse events are all we handle.
//!
//! A button presses on touch and releases on lift (CUE previews while held).
//! The overview needle-searches while the deck is stopped. In the browser, a
//! row highlights on the first tap and opens or loads on the second; dragging
//! scrolls. A double tap on the deck screen, anywhere but a control, opens
//! the sync view, and one on the sync view closes it.

use std::time::{Duration, Instant};

use kdj_engine::{Action, Edge};
use kdj_ui::browser::ROW_H;
use kdj_ui::touch::{Hit, Target};

use crate::App;

/// Movement (logical px) before a touch on a row counts as a drag.
const DRAG_PX: f32 = 10.0;
/// Two taps this close in time and place make a double tap.
const DOUBLE_TAP: Duration = Duration::from_millis(350);
const DOUBLE_TAP_PX: f32 = 60.0;

#[derive(Debug, Default)]
pub struct Touch {
    down: Option<Down>,
    /// The last tap on no control, and where: half a double tap.
    last_tap: Option<(Instant, [f32; 2])>,
}

#[derive(Debug)]
struct Down {
    slot: usize,
    hit: Hit,
    start_y: f32,
    /// Rows scrolled so far by this drag; None until it's a drag.
    scrolled: Option<isize>,
}

impl Touch {
    pub fn down(&mut self, app: &mut App, slot: usize, [x, y]: [f32; 2]) {
        if slot != app.focus && !app.browser.open {
            app.set_focus(slot);
        }
        let Some(hit) = app.slots.get(slot).and_then(|s| s.ui.hit(x, y)) else {
            self.tap_nothing(app, [x, y]);
            return;
        };
        match hit.target {
            Target::Action(a) => app.act(a, Edge::Press, slot),
            Target::Needle => {
                app.slots[slot].deck.needle(hit.fraction(x));
            }
            Target::Row(_) | Target::Back | Target::Page(_) | Target::Choice(..) | Target::Setting(_) => {}
        }
        self.down = Some(Down { slot, hit, start_y: y, scrolled: None });
    }

    /// A tap on no control: the second of a double tap on the deck screen
    /// or the sync view opens or closes the sync view.
    fn tap_nothing(&mut self, app: &mut App, at: [f32; 2]) {
        if app.browser.open || app.settings.open {
            return;
        }
        let near = |p: [f32; 2]| (p[0] - at[0]).hypot(p[1] - at[1]) < DOUBLE_TAP_PX;
        if self.last_tap.take().is_some_and(|(t, p)| t.elapsed() < DOUBLE_TAP && near(p)) {
            app.act(Action::SyncView, Edge::Press, app.focus);
        } else {
            self.last_tap = Some((Instant::now(), at));
        }
    }

    /// The finger moved (while down).
    pub fn motion(&mut self, app: &mut App, [x, y]: [f32; 2]) {
        let Some(d) = &mut self.down else { return };
        match d.hit.target {
            Target::Needle => {
                app.slots[d.slot].deck.needle(d.hit.fraction(x));
            }
            Target::Row(_) => {
                let dy = d.start_y - y;
                if d.scrolled.is_none() && dy.abs() < DRAG_PX {
                    return;
                }
                #[expect(clippy::cast_possible_truncation, reason = "a drag is a few hundred pixels")]
                let rows = (dy / ROW_H).round() as isize;
                let done = d.scrolled.unwrap_or(0);
                app.browser.scroll_by(rows - done);
                d.scrolled = Some(rows);
            }
            Target::Action(_) | Target::Back | Target::Page(_) | Target::Choice(..) | Target::Setting(_) => {}
        }
    }

    pub fn up(&mut self, app: &mut App) {
        let Some(d) = self.down.take() else { return };
        match d.hit.target {
            Target::Action(a) => app.act(a, Edge::Release, d.slot),
            Target::Row(i) if d.scrolled.is_none() => {
                if app.browser.selected() == Some(i) {
                    app.open_selected(Some(app.focus));
                } else {
                    app.browser.select(i);
                }
            }
            Target::Back => app.act(Action::BrowseBack, Edge::Press, d.slot),
            Target::Page(i) => app.settings.select_page(i),
            Target::Choice(row, option) => {
                app.settings.choose(row, option);
            }
            Target::Setting(row) => {
                if let Some(key) = app.settings.press(row) {
                    app.setting_action(key);
                }
            }
            Target::Row(_) | Target::Needle => {}
        }
    }
}
