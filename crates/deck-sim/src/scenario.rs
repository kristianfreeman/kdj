//! Scripted sessions: a timeline of key presses and the state the deck must
//! be in at given moments. Keys go through the same keymap the app uses.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use deck_engine::analysis::{self, synth::SynthSpec};
use deck_engine::keymap::Keymap;
use deck_engine::{Action, Edge, Snapshot, Track};
use serde::Deserialize;

use crate::sim::Sim;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    /// "synth" (default) or a path to an audio file.
    #[serde(default = "synth")]
    pub track: String,
    #[serde(default)]
    pub synth: SynthOpts,
    #[serde(default = "default_block")]
    pub block: usize,
    pub duration: f64,
    #[serde(default)]
    pub step: Vec<Step>,
    #[serde(default)]
    pub expect: Vec<Expect>,
    #[serde(default)]
    pub frame: Vec<Frame>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SynthOpts {
    pub bpm: f64,
    pub bars: u32,
    pub intro_bars: u32,
    pub lead_in: f64,
}

impl Default for SynthOpts {
    fn default() -> Self {
        let d = SynthSpec::default();
        Self { bpm: d.bpm, bars: d.bars, intro_bars: d.intro_bars, lead_in: d.lead_in }
    }
}

impl SynthOpts {
    pub fn spec(&self, sample_rate: u32) -> SynthSpec {
        SynthSpec {
            bpm: self.bpm,
            bars: self.bars,
            intro_bars: self.intro_bars,
            lead_in: self.lead_in,
            sample_rate,
            ..Default::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Seconds on the audio clock.
    pub at: f64,
    pub key: Option<String>,
    pub action: Option<Action>,
    /// Hold for this many seconds before releasing.
    pub hold: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub at: f64,
    pub playing: Option<bool>,
    pub previewing: Option<bool>,
    pub keylock: Option<bool>,
    pub loop_active: Option<bool>,
    pub loop_beats: Option<f64>,
    pub tempo_pct: Option<f64>,
    /// Beat index of the playhead (0 = grid anchor).
    pub beat: Option<f64>,
    #[serde(default = "default_tol")]
    pub beat_tol: f64,
    /// Playhead inside the active loop.
    pub in_loop: Option<bool>,
    /// Cue point, in beats.
    pub cue_beat: Option<f64>,
}

/// A UI frame to render to PNG at a moment.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub at: f64,
    pub file: String,
}

fn synth() -> String {
    "synth".into()
}
const fn default_block() -> usize {
    128
}
const fn default_tol() -> f64 {
    0.02
}

pub fn load(path: &Path) -> Result<Scenario> {
    let src = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&src).with_context(|| format!("parsing {}", path.display()))
}

/// Build or load the scenario's track at `sample_rate`.
pub fn track_for(sc: &Scenario, base: &Path, sample_rate: u32) -> Result<Arc<Track>> {
    let track = if sc.track == "synth" {
        let spec = sc.synth.spec(sample_rate);
        analysis::build(format!("synth {} bpm", spec.bpm), analysis::synth::render(&spec), sample_rate).0
    } else {
        let p: PathBuf = base.join(&sc.track);
        analysis::load_file(&p, sample_rate)?.0
    };
    Ok(Arc::new(track))
}

/// Something to look at once the audio clock reaches its time.
enum Probe<'a> {
    Expect(&'a Expect),
    Frame(&'a Frame),
}

pub struct Outcome {
    pub failures: Vec<String>,
    pub sim: Sim,
}

/// Run a scenario. `on_frame` is called at each requested UI frame.
pub fn run(
    sc: &Scenario,
    track: Arc<Track>,
    record: bool,
    mut on_frame: impl FnMut(&Frame, &Snapshot, &Arc<Track>) -> Result<()>,
) -> Result<Outcome> {
    validate(sc)?;
    let keymap = Keymap::default();
    let mut events: Vec<(f64, u8, Action, Edge)> = Vec::new();
    for s in &sc.step {
        let action = match (&s.key, s.action) {
            (Some(k), None) => keymap.get(k).ok_or_else(|| anyhow!("{}: unmapped key {k:?}", sc.name))?,
            (None, Some(a)) => a,
            _ => return Err(anyhow!("{}: a step needs exactly one of key or action", sc.name)),
        };
        // At equal times, presses go before releases.
        events.push((s.at, 0, action, Edge::Press));
        events.push((s.at + s.hold.unwrap_or(0.0), 1, action, Edge::Release));
    }
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut probes: Vec<(f64, Probe)> = sc.expect.iter().map(|e| (e.at, Probe::Expect(e))).collect();
    probes.extend(sc.frame.iter().map(|f| (f.at, Probe::Frame(f))));
    probes.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut sim = Sim::new(track, sc.block)?;
    if record {
        sim.recording = Some(Vec::new());
    }
    let mut failures = Vec::new();
    let (mut ei, mut pi) = (0, 0);
    let end = sim.frame_at(sc.duration);
    while sim.frames() < end {
        while ei < events.len() && events[ei].0 <= sim.secs() {
            let (_, _, a, e) = events[ei];
            sim.send(a, e);
            ei += 1;
        }
        sim.step();
        while pi < probes.len() && probes[pi].0 <= sim.secs() {
            let snap = sim.snapshot();
            match probes[pi].1 {
                Probe::Expect(e) => failures.extend(check(e, &snap)),
                Probe::Frame(f) => on_frame(f, &snap, &sim.track)?,
            }
            pi += 1;
        }
    }
    // `validate` keeps every time inside the run, so this is only a backstop.
    if let Some((at, _)) = probes.get(pi) {
        return Err(anyhow!("{}: the run ended at {:.2}s before reaching a check at {at}s", sc.name, sim.secs()));
    }
    Ok(Outcome { failures, sim })
}

/// Refuse a scenario that would quietly test less than it says: a step,
/// check or frame after the end, or a check that checks nothing.
fn validate(sc: &Scenario) -> Result<()> {
    let d = sc.duration;
    let within = |at: f64| (0.0..=d).contains(&at);
    for (i, s) in sc.step.iter().enumerate() {
        let release = s.at + s.hold.unwrap_or(0.0);
        if !(within(s.at) && s.at < d && within(release)) {
            return Err(anyhow!(
                "{}: step #{} (at {}s, released at {release}s) is outside the {d}s duration",
                sc.name,
                i + 1,
                s.at
            ));
        }
    }
    for (i, e) in sc.expect.iter().enumerate() {
        if !within(e.at) {
            return Err(anyhow!("{}: expect #{} at {}s is past the {d}s duration", sc.name, i + 1, e.at));
        }
        if !e.checks_something() {
            return Err(anyhow!("{}: expect #{} at {}s checks nothing", sc.name, i + 1, e.at));
        }
    }
    for f in &sc.frame {
        if !within(f.at) {
            return Err(anyhow!("{}: frame {:?} at {}s is past the {d}s duration", sc.name, f.file, f.at));
        }
    }
    Ok(())
}

impl Expect {
    /// At least one field to compare (`beat_tol` alone only tunes others).
    const fn checks_something(&self) -> bool {
        self.playing.is_some()
            || self.previewing.is_some()
            || self.keylock.is_some()
            || self.loop_active.is_some()
            || self.loop_beats.is_some()
            || self.tempo_pct.is_some()
            || self.beat.is_some()
            || self.in_loop.is_some()
            || self.cue_beat.is_some()
    }
}

fn check(e: &Expect, s: &Snapshot) -> Vec<String> {
    let mut f = Vec::new();
    let mut flag = |name: &str, ok: bool, got: String| {
        if !ok {
            f.push(format!("at {:.2}s: {name} was {got}", e.at));
        }
    };
    if let Some(v) = e.playing {
        flag("playing", s.playing == v, s.playing.to_string());
    }
    if let Some(v) = e.previewing {
        flag("previewing", s.previewing == v, s.previewing.to_string());
    }
    if let Some(v) = e.keylock {
        flag("keylock", s.keylock == v, s.keylock.to_string());
    }
    if let Some(v) = e.loop_active {
        flag("loop_active", s.loop_active == v, s.loop_active.to_string());
    }
    if let Some(v) = e.loop_beats {
        flag("loop_beats", (s.loop_beats - v).abs() < 1e-9, s.loop_beats.to_string());
    }
    if let Some(v) = e.tempo_pct {
        flag("tempo_pct", (s.tempo_pct - v).abs() < 1e-9, format!("{:.2}", s.tempo_pct));
    }
    if let Some(v) = e.beat {
        let b = s.grid.beat_at(s.pos);
        flag(&format!("beat (want {v}±{})", e.beat_tol), (b - v).abs() <= e.beat_tol, format!("{b:.4}"));
    }
    if let Some(v) = e.in_loop {
        let inside = s.loop_active && s.pos >= s.loop_start && s.pos < s.loop_end;
        let g = &s.grid;
        let where_ = format!(
            "{inside} (beat {:.3}, loop {:.3}–{:.3})",
            g.beat_at(s.pos),
            g.beat_at(s.loop_start),
            g.beat_at(s.loop_end)
        );
        flag("in_loop", inside == v, where_);
    }
    if let Some(v) = e.cue_beat {
        let b = s.grid.beat_at(s.cue);
        flag("cue_beat", (b - v).abs() <= e.beat_tol, format!("{b:.4}"));
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(body: &str) -> Scenario {
        let src = format!("name = \"t\"\nduration = 2.0\n{body}");
        match toml::from_str(&src) {
            Ok(sc) => sc,
            Err(e) => unreachable!("test scenario doesn't parse: {e}"),
        }
    }

    fn error(body: &str) -> String {
        validate(&scenario(body)).err().map(|e| e.to_string()).unwrap_or_default()
    }

    #[test]
    fn times_inside_the_run_are_fine() {
        let ok = "[[step]]\nat = 1.0\nkey = \"space\"\nhold = 1.0\n[[expect]]\nat = 2.0\nplaying = true\n";
        assert!(validate(&scenario(ok)).is_ok());
    }

    #[test]
    fn late_items_and_empty_checks_fail_by_name() {
        assert!(error("[[step]]\nat = 2.5\nkey = \"space\"\n").contains("t: step #1"));
        assert!(error("[[step]]\nat = 1.5\nkey = \"space\"\nhold = 1.0\n").contains("t: step #1"));
        assert!(error("[[expect]]\nat = 3.0\nplaying = true\n").contains("t: expect #1 at 3s is past"));
        assert!(error("[[expect]]\nat = 1.0\n").contains("t: expect #1 at 1s checks nothing"));
        assert!(error("[[frame]]\nat = 9.0\nfile = \"x.png\"\n").contains("t: frame \"x.png\""));
    }
}
