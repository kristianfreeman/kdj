//! deck-sim: run the deck without hardware.
//!
//!   deck-sim scenarios [FILE...] [--record DIR] [--frames DIR]
//!   deck-sim perf [--track FILE]
//!   deck-sim analyze FILE...
//!   deck-sim synth OUT.wav [--bpm N] [--bars N] [--rate HZ]
//!   deck-sim frame OUT.png [--at SECS] [--loop BEATS] [--scale N] [--decks 2] [--link BPM] [--fader PCT]
//!   deck-sim rekordbox list|bench [--count N]   (local Rekordbox collection)
//!   deck-sim browser OUT.png [--open A/B] [--cursor N]   (browser: your library + a fake USB)

mod alloc_guard;
mod perf;
mod rekordbox;
mod scenario;
mod screen;
mod sim;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use deck_engine::analysis::{self, synth::SynthSpec};
use deck_engine::{Action, Edge, Track};

#[global_allocator]
static ALLOC: alloc_guard::Guarded = alloc_guard::Guarded;

const SAMPLE_RATE: u32 = 48_000;

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = args.split_first().ok_or_else(|| anyhow!("usage: deck-sim scenarios|perf|analyze|synth …"))?;
    let opts = Opts::parse(rest)?;
    let root = workspace_root()?;
    match cmd.as_str() {
        "scenarios" => scenarios(&root, &opts),
        "perf" => perf(&root, &opts),
        "analyze" => analyze(&opts.positional),
        "synth" => synth(&opts),
        "frame" => frame(&opts),
        "browser" => browser_frame(&opts),
        "rekordbox" => match opts.positional.first().map(String::as_str) {
            Some("list") => rekordbox::list(SAMPLE_RATE),
            Some("probe") => rekordbox::probe(SAMPLE_RATE),
            Some("bench") => rekordbox::bench(SAMPLE_RATE, opts.flag("count").unwrap_or("8").parse()?),
            _ => bail!("usage: deck-sim rekordbox list|bench [--count N]"),
        },
        other => bail!("unknown command {other:?}"),
    }
}

/// Budgets for the audio callback, track loading and UI frames.
fn perf(root: &Path, opts: &Opts) -> Result<bool> {
    let budgets = perf::load_budgets(&root.join("perf-budgets.toml"))?;
    let bench: &perf::UiBench = &ui_bench;
    let ok = perf::run(&budgets, opts.flag("track").map(Path::new), Some(bench))?;
    let allocs = alloc_guard::violations();
    if allocs > 0 {
        println!("audio callback allocated {allocs} times: OVER (must be zero)");
    }
    Ok(ok && allocs == 0)
}

/// CPU ms to build and draw each two-deck UI frame over ten seconds at
/// 60 fps, from half a minute in so the waveform is busy: both decks playing
/// `track` at slightly different tempos (so the phase meters move), deck A
/// in a 4-beat loop with two hot cues in view. Also counts allocations.
fn ui_bench(track: &Arc<Track>) -> Result<perf::UiFrames> {
    const WARMUP: u32 = 60;
    let mut a = sim::Sim::new(Arc::clone(track), 128)?;
    let mut b = sim::Sim::new(Arc::clone(track), 128)?;
    for (sim, tempo) in [(&mut a, Action::TempoUp), (&mut b, Action::TempoDownCoarse)] {
        sim.send(tempo, Edge::Press);
        sim.send(Action::PlayPause, Edge::Press);
    }
    // Empty pads store "here".
    for (t, cue) in [(27.6, Action::HotCue1), (28.4, Action::HotCue2)] {
        a.run_until(t);
        a.send(cue, Edge::Press);
        a.send(cue, Edge::Release);
    }
    a.run_until(29.0);
    a.send(Action::BeatLoop4, Edge::Press);
    a.send(Action::BeatLoop4, Edge::Release);
    let frames: Vec<[deck_engine::Snapshot; 2]> = (0..600 + WARMUP)
        .map(|i| {
            let t = 30.0 + f64::from(i) / 60.0;
            a.run_until(t);
            b.run_until(t);
            [a.snapshot(), b.snapshot()]
        })
        .collect();
    // Scale 1: the Pi drives its screen at native resolution.
    let mut screen = screen::Screen::new(1.0)?;
    screen.bench_two_decks(track, &frames, WARMUP as usize)
}

fn scenarios(root: &Path, opts: &Opts) -> Result<bool> {
    let files: Vec<PathBuf> = if opts.positional.is_empty() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(root.join("scenarios"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        v.sort();
        v
    } else {
        opts.positional.iter().map(PathBuf::from).collect()
    };
    let record = opts.flag("record").map(PathBuf::from);
    let frames = opts.flag("frames").map(PathBuf::from);
    let mut all_ok = true;
    for f in files {
        let sc = scenario::load(&f)?;
        let allocs_before = alloc_guard::violations();
        let track = scenario::track_for(&sc, f.parent().unwrap_or(root), SAMPLE_RATE)?;
        let mut screen = match &frames {
            Some(_) if !sc.frame.is_empty() => Some(screen::Screen::new(2.0)?),
            _ => None,
        };
        let out = scenario::run(&sc, track, record.is_some(), |frame, snap, track| {
            if let (Some(dir), Some(scr)) = (&frames, screen.as_mut()) {
                scr.save_png(&dir.join(&frame.file), snap, track)?;
            }
            Ok(())
        })?;
        let p99 = Duration::from_nanos(out.sim.timings.value_at_quantile(0.99)).as_secs_f64() * 1e6;
        let mut failures = out.failures;
        if alloc_guard::violations() > allocs_before {
            failures.push(format!("audio callback allocated {} times", alloc_guard::violations() - allocs_before));
        }
        if failures.is_empty() {
            println!("ok    {} ({:.0} s audio, callback p99 {p99:.1} µs)", sc.name, sc.duration);
        } else {
            all_ok = false;
            println!("FAIL  {}", sc.name);
            for msg in &failures {
                println!("        {msg}");
            }
        }
        if let (Some(dir), Some(rec)) = (&record, &out.sim.recording) {
            std::fs::create_dir_all(dir)?;
            let stem = f.file_stem().unwrap_or_default().to_string_lossy();
            perf::write_recording(&dir.join(format!("{stem}.wav")), rec, SAMPLE_RATE)?;
        }
    }
    Ok(all_ok)
}

/// Detected tempo and grid for real files: check the analyser on your own music.
fn analyze(files: &[String]) -> Result<bool> {
    if files.is_empty() {
        bail!("usage: deck-sim analyze FILE...");
    }
    for f in files {
        let (t, tm) = analysis::load_file(Path::new(f), SAMPLE_RATE)?;
        let first = t.grid.anchor / f64::from(t.sample_rate);
        println!(
            "{:>8.3} bpm  bar 1 at {first:>7.3} s  ({:.1} min, analysed in {:.0} ms)  {}",
            t.grid.bpm,
            t.duration_secs() / 60.0,
            tm.total().as_secs_f64() * 1000.0,
            t.title
        );
    }
    Ok(true)
}

fn synth(opts: &Opts) -> Result<bool> {
    let out = opts.positional.first().ok_or_else(|| anyhow!("usage: deck-sim synth OUT.wav"))?;
    let mut spec = SynthSpec::default();
    if let Some(v) = opts.flag("bpm") {
        spec.bpm = v.parse()?;
    }
    if let Some(v) = opts.flag("bars") {
        spec.bars = v.parse()?;
    }
    if let Some(v) = opts.flag("rate") {
        spec.sample_rate = v.parse()?;
    }
    perf::write_recording(Path::new(out), &analysis::synth::render(&spec), spec.sample_rate)?;
    println!("wrote {out}: {} bpm, first kick (bar 1) at {:.4} s", spec.bpm, spec.first_kick());
    Ok(true)
}

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().context("finding the workspace root")
}

struct Opts {
    positional: Vec<String>,
    flags: Vec<(String, String)>,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Self> {
        let mut o = Self { positional: Vec::new(), flags: Vec::new() };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                let v = it.next().ok_or_else(|| anyhow!("--{name} needs a value"))?;
                o.flags.push((name.into(), v.clone()));
            } else {
                o.positional.push(a.clone());
            }
        }
        Ok(o)
    }

    fn flag(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// Render one moment of a synthetic set to PNG: playing, optionally in a
/// loop. With `--decks 2`, a second deck at 135 BPM pulled down to roughly
/// match, slightly out of phase, both stacked as in the two-deck harness.
fn frame(opts: &Opts) -> Result<bool> {
    let out = opts.positional.first().ok_or_else(|| anyhow!("usage: deck-sim frame OUT.png"))?;
    let at: f64 = opts.flag("at").unwrap_or("42.0").parse()?;
    let scale: f32 = opts.flag("scale").unwrap_or("2").parse()?;
    let decks: usize = opts.flag("decks").unwrap_or("1").parse()?;
    if let Some(path) = opts.flag("track") {
        return compare_waveforms(Path::new(path), Path::new(out), at, scale);
    }
    let mut sims = Vec::new();
    for (seed, bpm) in (1u32..).zip([134.0, 135.0]).take(decks.clamp(1, 2)) {
        let spec = SynthSpec { bars: 120, bpm, seed, ..Default::default() };
        let (track, _) =
            analysis::build(format!("Synthetic hardgroove {bpm} bpm"), analysis::synth::render(&spec), SAMPLE_RATE);
        let mut sim = sim::Sim::new(Arc::new(track), 128)?;
        let steps =
            if seed == 1 { [Action::TempoUpCoarse, Action::TempoUp].as_slice() } else { &[Action::TempoDownCoarse] };
        for &a in steps {
            sim.send(a, Edge::Press);
        }
        sim.send(Action::PlayPause, Edge::Press);
        sims.push(sim);
    }
    if let Some(bpm) = opts.flag("link") {
        // One deck on a Link session with a peer, a little behind its beat.
        let mut session = deck_engine::session::LocalSession::new(bpm.parse()?, SAMPLE_RATE);
        deck_engine::session::Session::force_beat(&mut session, 0.08);
        sims.truncate(1);
        sims[0].join(session, Some(1));
    }
    if let Some(pct) = opts.flag("fader") {
        // A hardware tempo fader somewhere other than the tempo: not taken over.
        let pos = pct.parse::<f64>()? / 8.0;
        sims[0].controller.as_mut().map(|c| c.tempo_fader(pos));
    }
    if let Some(beats) = opts.flag("loop") {
        let a = match beats {
            "1" => Action::BeatLoop1,
            "2" => Action::BeatLoop2,
            "4" => Action::BeatLoop4,
            "8" => Action::BeatLoop8,
            "16" => Action::BeatLoop16,
            _ => Action::BeatLoop32,
        };
        sims[0].run_until(at - 1.5);
        sims[0].send(a, Edge::Press);
    }
    if opts.flag("hotcues").is_some() {
        // Store A and B just behind where we'll look (empty pads store "here").
        for (t, a) in [(at - 3.0, Action::HotCue1), (at - 1.2, Action::HotCue2)] {
            sims[0].run_until(t);
            sims[0].send(a, Edge::Press);
            sims[0].send(a, Edge::Release);
        }
    }
    for sim in &mut sims {
        sim.run_until(at);
    }
    let snaps: Vec<_> = sims.iter_mut().map(sim::Sim::snapshot).collect();
    let mut screen = screen::Screen::new(scale)?;
    let (mut w, mut h, mut rgba) = (0, 0, Vec::new());
    for (i, (sim, letter)) in sims.iter().zip(['A', 'B']).enumerate() {
        let other = (snaps.len() > 1).then(|| &snaps[(i + 1) % snaps.len()]);
        let tag = (snaps.len() > 1).then_some((letter, i == 0));
        let (fw, fh, px) = screen.render(&snaps[i], other, &sim.track, tag)?;
        (w, h) = (fw, h + fh);
        rgba.extend_from_slice(&px);
    }
    screen::write_png(Path::new(out), w, h, &rgba)?;
    println!("wrote {out}");
    Ok(true)
}

/// Render the browser: the real local collection plus a small fake USB stick
/// (a real export, with placeholder audio for 150 of its tracks). `--open`
/// drills in by item names ("A/B/C"); several paths separated by ";" render
/// one PNG each (OUT-0.png, OUT-1.png, …) in one go.
fn browser_frame(opts: &Opts) -> Result<bool> {
    use deck_library::{library, nav};
    use deck_ui::browser::{self, Browser};
    let out =
        Path::new(opts.positional.first().ok_or_else(|| anyhow!("usage: deck-sim browser OUT.png [--open A/B;C]"))?);
    let t = std::time::Instant::now();
    let mut sources = library::sources();
    sources.push(fake_stick(150)?);
    let libs: Vec<_> = sources.iter().map(library::scan).collect();

    // The footer only needs a deck that's playing: a silent, unanalysed track.
    let mut sim = sim::Sim::new(Arc::new(silent_track(240)), 128)?;
    sim.send(Action::PlayPause, Edge::Press);
    sim.run_until(0.5);
    let snap = sim.snapshot();
    let mut screen = screen::Screen::new(2.0)?;
    let cursor: isize = opts.flag("cursor").unwrap_or("2").parse()?;

    let paths: Vec<&str> = opts.flag("open").unwrap_or("").split(';').collect();
    for (k, path) in paths.iter().enumerate() {
        let mut b = Browser::new(true);
        b.scanning = false;
        let (level, nodes) = browser::level_for(&libs, nav::root(&libs));
        b.reset(level);
        let mut current = nodes;
        for name in path.split('/').filter(|n| !n.is_empty()) {
            let i = current
                .iter()
                .position(|&n| nav::title(&libs, n).trim() == name.trim())
                .ok_or_else(|| anyhow!("no {name:?} here"))?;
            b.move_by(isize::try_from(i)?);
            let (level, nodes) = browser::level_for(&libs, current[i]);
            b.push(level);
            current = nodes;
        }
        b.move_by(cursor);
        let file = if paths.len() == 1 {
            out.to_path_buf()
        } else {
            out.with_file_name(format!("{}-{k}.png", out.file_stem().unwrap_or_default().to_string_lossy()))
        };
        screen.save_browser_png(&file, &mut b, &snap)?;
        println!("wrote {}", file.display());
    }
    println!("{:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    Ok(true)
}

/// Silence with a plain 134 BPM grid: enough to run a deck without analysis.
fn silent_track(secs: usize) -> Track {
    use deck_engine::track::{GridSource, HOT_CUES, Waveform};
    Track {
        id: u64::MAX,
        title: "silence".into(),
        sample_rate: SAMPLE_RATE,
        samples: vec![0.0; secs * SAMPLE_RATE as usize * 2],
        grid: deck_engine::BeatGrid::new(134.0, 0.0, SAMPLE_RATE),
        grid_source: GridSource::Detected,
        load_cue: None,
        hot_cues: [None; HOT_CUES],
        waveform: Waveform { hop: 320, columns: Vec::new() },
    }
}

/// A USB stick in a temp folder: the large test export, with placeholder
/// audio for its first `tracks` tracks (the rest read as missing).
fn fake_stick(tracks: usize) -> Result<deck_library::library::Source> {
    let root = std::env::temp_dir().join(format!("deck-fake-stick-{tracks}"));
    let pdb = root.join("PIONEER/rekordbox/export.pdb");
    if !pdb.exists() {
        let fixture = workspace_root()?.join("crates/deck-library/tests/fixtures/large_export.pdb");
        std::fs::create_dir_all(pdb.parent().unwrap_or(&root))?;
        std::fs::copy(fixture, &pdb)?;
        let mut db = deck_library::pdb::read(&pdb)?.tracks;
        db.sort_by_key(|t| t.id);
        for t in db.into_iter().take(tracks) {
            let audio = root.join(t.file_path.trim_start_matches('/'));
            std::fs::create_dir_all(audio.parent().unwrap_or(&root))?;
            std::fs::write(audio, b"")?;
        }
    }
    Ok(deck_library::library::Source {
        name: "USB · REKORDBOX".into(),
        usbanlz: root.join("PIONEER/USBANLZ"),
        root: Some(root),
        pdb: Some(pdb),
    })
}

/// A real track at `at` seconds, drawn with Rekordbox's waveform (top) and our
/// own analysis (bottom), to compare them.
fn compare_waveforms(audio: &Path, out: &Path, at: f64, scale: f32) -> Result<bool> {
    let dat = deck_library::anlz::find(audio).ok_or_else(|| anyhow!("not in the Rekordbox collection"))?;
    let p = deck_library::anlz::prepared(&dat, SAMPLE_RATE).ok_or_else(|| anyhow!("no Rekordbox analysis"))?;
    let grid = p.grid.ok_or_else(|| anyhow!("no steady Rekordbox grid"))?;
    let rb = analysis::Known { grid: Some(grid), waveform: p.waveform, load_cue: p.load_cue, hot_cues: p.hot_cues };
    let ours = analysis::Known { grid: Some(grid), ..Default::default() };
    let mut screen = screen::Screen::new(scale)?;
    let (mut w, mut h, mut rgba) = (0, 0, Vec::new());
    for known in [rb, ours] {
        let (track, _) = analysis::load_file_known(audio, SAMPLE_RATE, known)?;
        let mut sim = sim::Sim::new(Arc::new(track), 128)?;
        sim.send(Action::PlayPause, Edge::Press);
        sim.run_until(at);
        let snap = sim.snapshot();
        let (fw, fh, px) = screen.render(&snap, None, &sim.track, None)?;
        (w, h) = (fw, h + fh);
        rgba.extend_from_slice(&px);
    }
    screen::write_png(out, w, h, &rgba)?;
    println!("wrote {} (top: Rekordbox waveform, bottom: ours)", out.display());
    Ok(true)
}
