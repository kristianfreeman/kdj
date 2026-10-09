//! Key name → action.
//!
//! The one place keys are mapped: the app turns window events into key names
//! (SDL's names), the simulator and scenario files use names directly, and
//! MIDI mappings will follow the same shape.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::action::Action;

const DEFAULT: &[(&str, Action)] = {
    use Action::{
        BeatJumpBack, BeatJumpForward, BeatLoop1, BeatLoop2, BeatLoop4, BeatLoop8, BeatLoop16, BeatLoop32, BpmDouble,
        BpmDown, BpmHalve, BpmUp, Browse, Cue, GridAnchorHere, GridNudgeEarlier, GridNudgeLater, HotCue1, HotCue2,
        HotCue3, HotCue4, HotCue5, HotCue6, HotCue7, HotCue8, JogBack, JogForward, KeyLock, LoopDouble, LoopHalve,
        LoopToggle, NudgeBack, NudgeForward, PhaseBack, PhaseForward, PlayPause, Quantize, SwitchDeck, TempoDown,
        TempoDownCoarse, TempoRange, TempoReset, TempoUp, TempoUpCoarse, ToggleHud, ZoomIn, ZoomOut,
    };
    &[
        ("Space", PlayPause),
        ("C", Cue),
        ("Up", TempoUp),
        ("Down", TempoDown),
        ("PageUp", TempoUpCoarse),
        ("PageDown", TempoDownCoarse),
        ("R", TempoReset),
        ("T", TempoRange),
        ("K", KeyLock),
        ("Q", Quantize),
        ("Right", NudgeForward),
        ("Left", NudgeBack),
        ("9", JogBack),
        ("0", JogForward),
        ("7", PhaseBack),
        ("8", PhaseForward),
        ("L", LoopToggle),
        ("[", LoopHalve),
        ("]", LoopDouble),
        ("1", BeatLoop1),
        ("2", BeatLoop2),
        ("3", BeatLoop4),
        ("4", BeatLoop8),
        ("5", BeatLoop16),
        ("6", BeatLoop32),
        (",", BeatJumpBack),
        (".", BeatJumpForward),
        ("G", GridAnchorHere),
        ("-", GridNudgeEarlier),
        ("=", GridNudgeLater),
        (";", BpmDown),
        ("'", BpmUp),
        ("H", BpmHalve),
        ("D", BpmDouble),
        ("Z", ZoomOut),
        ("X", ZoomIn),
        ("`", ToggleHud),
        ("S", Action::Sync),
        ("M", Action::Master),
        ("Tab", Browse),
        ("\\", SwitchDeck),
        ("O", Action::Settings),
        ("F1", HotCue1),
        ("F2", HotCue2),
        ("F3", HotCue3),
        ("F4", HotCue4),
        ("F5", HotCue5),
        ("F6", HotCue6),
        ("F7", HotCue7),
        ("F8", HotCue8),
    ]
};

/// Checked first while the browser is open; anything unbound here (play,
/// cue, tempo keys you didn't override) still reaches the deck.
const BROWSER: &[(&str, Action)] = {
    use Action::{Browse, BrowseBack, BrowseDown, BrowseEnter, BrowseLoad, BrowsePageDown, BrowsePageUp, BrowseUp};
    &[
        ("Up", BrowseUp),
        ("Down", BrowseDown),
        ("PageUp", BrowsePageUp),
        ("PageDown", BrowsePageDown),
        ("Return", BrowseLoad),
        ("Right", BrowseEnter),
        ("Left", BrowseBack),
        ("Backspace", BrowseBack),
        ("Tab", Browse),
        ("Escape", Browse),
    ]
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Deck,
    Browser,
}

#[derive(Debug)]
pub struct Keymap {
    map: HashMap<String, Action>,
    browser: HashMap<String, Action>,
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    keys: HashMap<String, Action>,
    #[serde(default)]
    browser_keys: HashMap<String, Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        let table = |t: &[(&str, Action)]| t.iter().map(|(k, a)| (normalise(k), *a)).collect();
        Self { map: table(DEFAULT), browser: table(BROWSER) }
    }
}

impl Keymap {
    /// Defaults, overridden by `[keys]` and `[browser_keys]` tables, e.g.
    /// `Space = "PlayPause"`.
    ///
    /// # Errors
    ///
    /// If `toml_src` isn't valid TOML or names an unknown action.
    pub fn with_overrides(toml_src: &str) -> Result<Self> {
        let file: File = toml::from_str(toml_src).context("parsing keymap")?;
        let mut km = Self::default();
        km.map.extend(file.keys.into_iter().map(|(k, a)| (normalise(&k), a)));
        km.browser.extend(file.browser_keys.into_iter().map(|(k, a)| (normalise(&k), a)));
        Ok(km)
    }

    /// The deck layer.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Action> {
        self.map.get(&normalise(key)).copied()
    }

    #[must_use]
    pub fn get_in(&self, key: &str, layer: Layer) -> Option<Action> {
        let browser = match layer {
            Layer::Browser => self.browser.get(&normalise(key)).copied(),
            Layer::Deck => None,
        };
        browser.or_else(|| self.get(key))
    }

    /// Bindings sorted by action, for help text.
    #[must_use]
    pub fn bindings(&self) -> Vec<(String, Action)> {
        let mut v: Vec<_> = self.map.iter().map(|(k, a)| (k.clone(), *a)).collect();
        v.sort_by_key(|(k, a)| (format!("{a:?}"), k.clone()));
        v
    }
}

/// "Page Up", `page_up` and `PageUp` are the same key.
fn normalise(key: &str) -> String {
    key.chars().filter(|c| !matches!(c, ' ' | '_')).collect::<String>().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_merge_with_defaults() {
        let km = Keymap::with_overrides("[keys]\n\"Return\" = \"Cue\"\n").unwrap();
        assert_eq!(km.get("Return"), Some(Action::Cue));
        assert_eq!(km.get("Space"), Some(Action::PlayPause));
        assert_eq!(km.get("Page Up"), Some(Action::TempoUpCoarse));
    }

    #[test]
    fn browser_layer_falls_through_to_the_deck() {
        let km = Keymap::default();
        assert_eq!(km.get_in("Up", Layer::Browser), Some(Action::BrowseUp));
        assert_eq!(km.get_in("Up", Layer::Deck), Some(Action::TempoUp));
        assert_eq!(km.get_in("Space", Layer::Browser), Some(Action::PlayPause));
    }
}
