//! Everything the deck can be told to do, independent of where the request
//! came from. Keyboard, touch and MIDI all map onto these; nothing downstream
//! knows which one was used.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    PlayPause,
    /// CDJ-style cue. Release matters: holding previews from the cue point.
    Cue,

    TempoUp,
    TempoDown,
    TempoUpCoarse,
    TempoDownCoarse,
    TempoReset,
    /// Cycle the tempo range between ±8% and ±16%.
    TempoRange,
    KeyLock,
    /// Lock the deck: loads are refused, playing or not, until `Unlock`
    /// (on the K3, SHIFT + LOCK: a deliberate press, so a stray one can't
    /// throw a track off).
    Lock,
    Unlock,
    /// A ~10 ms push, like flicking a jog wheel; held, it keeps pushing,
    /// building. On SYNC the deck holds where it's pushed. When paused,
    /// steps the playhead instead.
    NudgeForward,
    NudgeBack,
    /// A jog-wheel touch: a short burst of speed that fades.
    JogForward,
    JogBack,
    /// Shift the phase by a couple of milliseconds, instantly.
    PhaseForward,
    PhaseBack,

    Quantize,
    LoopToggle,
    LoopHalve,
    LoopDouble,
    BeatLoop1,
    BeatLoop2,
    BeatLoop4,
    BeatLoop8,
    BeatLoop16,
    BeatLoop32,
    /// Hot cues A–H. Playing: jump (keeping the beat phase with quantize on).
    /// Stopped: preview while held. Empty: store one here.
    HotCue1,
    HotCue2,
    HotCue3,
    HotCue4,
    HotCue5,
    HotCue6,
    HotCue7,
    HotCue8,
    /// Follow the shared timeline (Ableton Link): tempo and bar phase.
    Sync,
    /// Lead the shared timeline: this deck's tempo becomes everyone's.
    Master,
    /// Jump by the current loop size; moves the loop with you if one is active.
    BeatJumpForward,
    BeatJumpBack,

    /// Make the playhead position beat 1 of a bar.
    GridAnchorHere,
    GridNudgeEarlier,
    GridNudgeLater,
    BpmUp,
    BpmDown,
    BpmHalve,
    BpmDouble,

    // Display-only; the engine ignores these.
    ZoomIn,
    ZoomOut,
    ToggleHud,
    /// Move the keyboard to the other deck (two-deck harness).
    SwitchDeck,
    /// Open or close the settings screen.
    Settings,

    // Browser; also handled outside the engine.
    Browse,
    BrowseUp,
    BrowseDown,
    BrowsePageUp,
    BrowsePageDown,
    /// Open the highlighted folder or playlist, or load the highlighted track.
    BrowseLoad,
    /// Open the highlighted folder or playlist (never loads).
    BrowseEnter,
    BrowseBack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Press,
    Release,
}

pub const LOOP_SIZES: [f64; 6] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

impl Action {
    /// Whether holding the key should repeat it (tempo and grid trims do,
    /// transport buttons never should).
    #[must_use]
    pub const fn repeats(self) -> bool {
        matches!(
            self,
            Self::TempoUp
                | Self::JogForward
                | Self::JogBack
                | Self::PhaseForward
                | Self::PhaseBack
                | Self::TempoDown
                | Self::TempoUpCoarse
                | Self::TempoDownCoarse
                | Self::GridNudgeEarlier
                | Self::GridNudgeLater
                | Self::BpmUp
                | Self::BpmDown
                | Self::BrowseUp
                | Self::BrowseDown
                | Self::BrowsePageUp
                | Self::BrowsePageDown
        )
    }

    /// Handled by the app (screen, browser), not the engine.
    #[must_use]
    pub const fn is_ui(self) -> bool {
        matches!(
            self,
            Self::ZoomIn
                | Self::ZoomOut
                | Self::ToggleHud
                | Self::SwitchDeck
                | Self::Settings
                | Self::Browse
                | Self::BrowseUp
                | Self::BrowseDown
                | Self::BrowsePageUp
                | Self::BrowsePageDown
                | Self::BrowseLoad
                | Self::BrowseEnter
                | Self::BrowseBack
        )
    }

    /// A short uppercase name that fits a button on a controller drawing.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PlayPause => "PLAY",
            Self::Cue => "CUE",
            Self::TempoUp => "TEMPO +",
            Self::TempoDown => "TEMPO −",
            Self::TempoUpCoarse => "TEMPO ++",
            Self::TempoDownCoarse => "TEMPO −−",
            Self::TempoReset => "TEMPO 0",
            Self::TempoRange => "RANGE",
            Self::KeyLock => "KEY",
            Self::Lock => "LOCK",
            Self::Unlock => "UNLOCK",
            Self::NudgeForward => "NUDGE →",
            Self::NudgeBack => "← NUDGE",
            Self::JogForward => "JOG →",
            Self::JogBack => "← JOG",
            Self::PhaseForward => "PHASE +",
            Self::PhaseBack => "PHASE −",
            Self::Quantize => "QUANTIZE",
            Self::LoopToggle => "LOOP",
            Self::LoopHalve => "LOOP ÷2",
            Self::LoopDouble => "LOOP ×2",
            Self::BeatLoop1 => "LOOP 1",
            Self::BeatLoop2 => "LOOP 2",
            Self::BeatLoop4 => "LOOP 4",
            Self::BeatLoop8 => "LOOP 8",
            Self::BeatLoop16 => "LOOP 16",
            Self::BeatLoop32 => "LOOP 32",
            Self::HotCue1 => "HOT A",
            Self::HotCue2 => "HOT B",
            Self::HotCue3 => "HOT C",
            Self::HotCue4 => "HOT D",
            Self::HotCue5 => "HOT E",
            Self::HotCue6 => "HOT F",
            Self::HotCue7 => "HOT G",
            Self::HotCue8 => "HOT H",
            Self::Sync => "SYNC",
            Self::Master => "MASTER",
            Self::BeatJumpForward => "JUMP →",
            Self::BeatJumpBack => "← JUMP",
            Self::GridAnchorHere => "GRID 1",
            Self::GridNudgeEarlier => "← GRID",
            Self::GridNudgeLater => "GRID →",
            Self::BpmUp => "BPM +",
            Self::BpmDown => "BPM −",
            Self::BpmHalve => "BPM ÷2",
            Self::BpmDouble => "BPM ×2",
            Self::ZoomIn => "ZOOM +",
            Self::ZoomOut => "ZOOM −",
            Self::ToggleHud => "HUD",
            Self::SwitchDeck => "DECK",
            Self::Settings => "SETTINGS",
            Self::Browse => "BROWSE",
            Self::BrowseUp => "BROWSE ↑",
            Self::BrowseDown => "BROWSE ↓",
            Self::BrowsePageUp => "PAGE ↑",
            Self::BrowsePageDown => "PAGE ↓",
            Self::BrowseLoad => "LOAD",
            Self::BrowseEnter => "OPEN",
            // Opens the browser, backs out a level, and at the top closes it.
            Self::BrowseBack => "BROWSE/BACK",
        }
    }

    /// The name for an endless encoder turning `up` one way and `down` the
    /// other: one word for a known pair (JOG, TEMPO), else both labels.
    #[must_use]
    pub fn pair_label(up: Self, down: Self) -> String {
        let pairs = [
            (Self::JogForward, Self::JogBack, "JOG"),
            (Self::TempoUp, Self::TempoDown, "TEMPO"),
            (Self::TempoUpCoarse, Self::TempoDownCoarse, "TEMPO ±"),
            (Self::NudgeForward, Self::NudgeBack, "NUDGE"),
            (Self::PhaseForward, Self::PhaseBack, "PHASE"),
            (Self::LoopDouble, Self::LoopHalve, "LOOP ×2/÷2"),
            (Self::BeatJumpForward, Self::BeatJumpBack, "JUMP"),
            (Self::GridNudgeLater, Self::GridNudgeEarlier, "GRID"),
            (Self::BpmUp, Self::BpmDown, "BPM"),
            (Self::BpmDouble, Self::BpmHalve, "BPM ×2/÷2"),
            (Self::ZoomIn, Self::ZoomOut, "ZOOM"),
            (Self::BrowseDown, Self::BrowseUp, "BROWSE"),
            (Self::BrowsePageDown, Self::BrowsePageUp, "PAGE"),
        ];
        // Either way round: which way is "up" is the mapping's business.
        pairs
            .iter()
            .find(|&&(a, b, _)| (a, b) == (up, down) || (b, a) == (up, down))
            .map_or_else(|| format!("{} / {}", up.label(), down.label()), |&(_, _, name)| name.to_owned())
    }

    /// Slot (0–7) for the `HotCueN` actions.
    #[must_use]
    pub fn hot_cue_index(self) -> Option<usize> {
        [
            Self::HotCue1,
            Self::HotCue2,
            Self::HotCue3,
            Self::HotCue4,
            Self::HotCue5,
            Self::HotCue6,
            Self::HotCue7,
            Self::HotCue8,
        ]
        .iter()
        .position(|&a| a == self)
    }

    /// Loop-size index for the `BeatLoopN` actions.
    #[must_use]
    pub const fn beat_loop_index(self) -> Option<usize> {
        Some(match self {
            Self::BeatLoop1 => 0,
            Self::BeatLoop2 => 1,
            Self::BeatLoop4 => 2,
            Self::BeatLoop8 => 3,
            Self::BeatLoop16 => 4,
            Self::BeatLoop32 => 5,
            _ => return None,
        })
    }
}
