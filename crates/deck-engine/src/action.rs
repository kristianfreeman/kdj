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
    /// Tempo bend while held, building the longer it's held. When paused,
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
