//! What the deck makes of an audio file: its rate and length, and how far
//! the engine plays through it in two seconds of output, key lock on and
//! off (it should be two seconds). For checking a file that plays wrongly.
//!
//!   cargo run --release -p kdj-engine --example decode -- FILE

use std::sync::Arc;

use anyhow::{Context, Result};
use kdj_engine::analysis::{self, decode};
use kdj_engine::{Action, Edge, Engine, EngineConfig};

fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("usage: decode FILE")?;
    let path = std::path::Path::new(&path);
    let d = decode::decode(path, None)?;
    let frames = d.samples.len() / 2;
    #[expect(clippy::cast_precision_loss, reason = "a frame count, shown in seconds")]
    let secs = frames as f64 / f64::from(d.sample_rate);
    println!("decoded: {frames} frames at {} Hz = {secs:.2} s", d.sample_rate);
    let (track, _) = analysis::load_file(path, 48_000)?;
    let track = Arc::new(track);
    println!("grid {:.2} BPM", track.grid.bpm);

    for keylock in [true, false] {
        let (mut engine, mut handle, _) = Engine::new(EngineConfig::default());
        handle.load(Arc::clone(&track)).map_err(|_| anyhow::anyhow!("load refused"))?;
        let mut out = vec![0.0f32; 128 * 2];
        engine.process(&mut out);
        if handle.snapshot().keylock != keylock {
            handle.send(Action::KeyLock, Edge::Press);
            handle.send(Action::KeyLock, Edge::Release);
            engine.process(&mut out);
        }
        handle.send(Action::PlayPause, Edge::Press);
        handle.send(Action::PlayPause, Edge::Release);
        engine.process(&mut out);
        let start = handle.snapshot().pos;
        // Two seconds of output, in the Pi's 128-frame blocks.
        for _ in 0..(2 * 48_000 / 128) {
            engine.process(&mut out);
        }
        let s = handle.snapshot();
        println!(
            "key lock {}: played {:.3} s of track in 2 s (playing {}, keylock {})",
            if keylock { "on " } else { "off" },
            (s.pos - start) / 48_000.0,
            s.playing,
            s.keylock
        );
    }
    Ok(())
}
