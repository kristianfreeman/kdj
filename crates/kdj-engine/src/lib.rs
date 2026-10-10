//! deck engine: everything that isn't windows, devices or pixels. Pure enough
//! to run headless in the simulator and in tests.

pub mod action;
pub mod analysis;
pub mod device;
pub mod engine;
pub mod feedback;
pub mod grid;
pub mod keymap;
pub mod midimap;
pub mod nudge;
pub mod session;
pub mod source;
pub mod stretch;
pub mod sync;
pub mod track;

pub use action::{Action, Edge};
pub use engine::{Engine, EngineConfig, EngineHandle, InputSender, Lead, Snapshot};
pub use grid::BeatGrid;
pub use track::Track;
