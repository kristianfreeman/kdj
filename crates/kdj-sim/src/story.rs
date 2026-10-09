//! Storybook: every screen the deck has, in the states that matter, rendered
//! by the real UI code into one HTML page.
//!
//!   kdj-sim story [DIR]     (default target/storybook; open index.html)
//!
//! Reproducible: synthetic tracks, the bundled test exports for the
//! browser (not your own collection), fixed times.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use kdj_engine::analysis::{self, synth::SynthSpec};
use kdj_engine::session::{LocalSession, Session};
use kdj_engine::{Action, Edge, Snapshot, Track};
use kdj_library::library;
use kdj_ui::settings::{self, Prefs, Settings, SystemInfo};

use crate::screen::{Screen, write_png};
use crate::sim::Sim;
use crate::{Opts, SAMPLE_RATE, browse_to, fake_stick, silent_track, workspace_root};

struct Story {
    group: &'static str,
    name: &'static str,
    note: &'static str,
    file: String,
}

struct Book {
    out: PathBuf,
    screen: Screen,
    stories: Vec<Story>,
}

impl Book {
    fn add(&mut self, group: &'static str, name: &'static str, note: &'static str) -> PathBuf {
        let file = format!("{:02}-{}.png", self.stories.len(), slug(name));
        let path = self.out.join(&file);
        self.stories.push(Story { group, name, note, file });
        path
    }

    fn deck(&mut self, name: &'static str, note: &'static str, sim: &mut Sim, status: &str) -> Result<()> {
        let snap = sim.snapshot();
        let (w, h, px) = self.screen.render_status(&snap, None, &sim.track, None, status)?;
        let path = self.add("Deck", name, note);
        write_png(&path, w, h, &px)
    }
}

pub fn run(opts: &Opts) -> Result<bool> {
    let out = PathBuf::from(opts.positional.first().map_or("target/storybook", String::as_str));
    std::fs::create_dir_all(&out)?;
    let t = std::time::Instant::now();
    let mut book = Book { out: out.clone(), screen: Screen::new(2.0)?, stories: Vec::new() };
    deck_stories(&mut book)?;
    browser_stories(&mut book)?;
    settings_stories(&mut book)?;
    std::fs::write(out.join("index.html"), html(&book.stories))?;
    println!(
        "wrote {} stories to {} in {:.1} s",
        book.stories.len(),
        out.join("index.html").display(),
        t.elapsed().as_secs_f64()
    );
    Ok(true)
}

fn synth(bpm: f64, seed: u32) -> Arc<Track> {
    let spec = SynthSpec { bars: 120, bpm, seed, ..Default::default() };
    Arc::new(analysis::build(format!("Synthetic hardgroove {bpm} bpm"), analysis::synth::render(&spec), SAMPLE_RATE).0)
}

fn press(sim: &mut Sim, a: Action) {
    sim.send(a, Edge::Press);
    sim.send(a, Edge::Release);
    sim.step();
}

fn playing(track: &Arc<Track>, at: f64) -> Result<Sim> {
    let mut sim = Sim::new(Arc::clone(track), 128)?;
    press(&mut sim, Action::PlayPause);
    sim.run_until(at);
    Ok(sim)
}

fn deck_stories(book: &mut Book) -> Result<()> {
    let track = synth(134.0, 1);

    let mut sim = Sim::new(Arc::clone(&track), 128)?;
    let snap = Snapshot { track_frames: 0, ..sim.snapshot() };
    let (w, h, px) = book.screen.render_status(&snap, None, &track, None, "Tap here or press Tab to browse")?;
    write_png(&book.add("Deck", "No track", "First start: tapping anywhere opens the browser."), w, h, &px)?;

    sim.run_until(0.2);
    book.deck("Stopped at the cue", "Paused, parked on the cue: CUE lit, play outlined.", &mut sim, "")?;

    let mut sim = playing(&track, 42.0)?;
    book.deck("Playing", "Auto grid (no Rekordbox analysis), key lock and quantize on.", &mut sim, "")?;

    let mut sim = playing(&track, 40.5)?;
    press(&mut sim, Action::BeatLoop4);
    sim.run_until(42.0);
    book.deck("Loop, 4 beats", "Hatched on both waveforms, bracketed; 4 and LOOP filled.", &mut sim, "")?;

    let mut sim = playing(&track, 39.0)?;
    press(&mut sim, Action::HotCue1);
    sim.run_until(40.8);
    press(&mut sim, Action::HotCue2);
    sim.run_until(42.0);
    book.deck("Hot cues A and B", "Flags on the detail waveform, ticks on the overview.", &mut sim, "")?;

    let mut sim = playing(&track, 1.0)?;
    for a in [Action::TempoRange, Action::KeyLock] {
        press(&mut sim, a);
    }
    for _ in 0..6 {
        press(&mut sim, Action::TempoUpCoarse);
    }
    sim.run_until(42.0);
    book.deck("+6% on ±16, key lock off", "KEY outlined; the range badge reads ±16.", &mut sim, "")?;

    let mut sim = playing(&track, 1.0)?;
    let mut session = LocalSession::new(136.0, SAMPLE_RATE);
    session.force_beat(0.3);
    sim.join(session, Some(1));
    press(&mut sim, Action::Sync);
    sim.run_until(42.0);
    book.deck("SYNC to Link", "Following a 136 BPM session with one peer: locked, the meter at zero.", &mut sim, "")?;

    let mut sim = playing(&track, 1.0)?;
    sim.join(LocalSession::new(120.0, SAMPLE_RATE), Some(2));
    press(&mut sim, Action::Master);
    sim.run_until(42.0);
    book.deck("MASTER on Link", "This deck sets the session's tempo for two peers.", &mut sim, "")?;

    let mut sim = playing(&track, 1.0)?;
    let mut session = LocalSession::new(135.0, SAMPLE_RATE);
    session.force_beat(0.08);
    sim.join(session, Some(1));
    sim.run_until(42.0);
    book.deck("Alone on Link, not synced", "The phase meter reads against the session.", &mut sim, "")?;

    let mut sim = playing(&track, 41.0)?;
    if let Some(c) = sim.controller.as_mut() {
        c.tempo_fader(-5.0 / 8.0);
    }
    sim.run_until(42.0);
    book.deck("Tempo fader catching up", "Hollow: where the hardware fader is; solid: the tempo.", &mut sim, "")?;

    #[expect(clippy::cast_precision_loss, reason = "a few minutes of frames")]
    let end = sim.track.frames() as f64 / f64::from(SAMPLE_RATE);
    let mut sim = playing(&track, end - 20.0)?;
    // Just after a beat, when the time flashes.
    let snap = sim.snapshot();
    let next = snap.grid.frame_of(snap.grid.beat_at(snap.pos).ceil() + 0.2);
    sim.run_until((next - snap.pos).mul_add(1.0 / f64::from(SAMPLE_RATE), sim.secs()));
    book.deck("Last 30 seconds", "Time remaining flashes inverted on the beat.", &mut sim, "")?;

    let mut sim = playing(&track, 42.0)?;
    book.deck("Loading", "Status bottom right while the next track loads.", &mut sim, "Loading Skid Row…")?;

    let mut sim = Sim::new(
        Arc::new(Track {
            title: "An Extremely Long Track Title (Extended Club Mix) [Remastered 2026]".into(),
            ..silent_track(240)
        }),
        128,
    )?;
    sim.run_until(0.2);
    let long =
        "Couldn't load /Volumes/USB/Contents/Some Artist/Some Album/01 Some Track.mp3: unrecognised audio format";
    book.deck("Long title, long error", "Both cut short, never off screen.", &mut sim, long)?;

    two_decks(book, &track)
}

fn two_decks(book: &mut Book, a: &Arc<Track>) -> Result<()> {
    let b = synth(135.0, 2);
    let mut sa = playing(a, 1.0)?;
    press(&mut sa, Action::TempoUpCoarse);
    press(&mut sa, Action::TempoUp);
    let mut sb = playing(&b, 1.0)?;
    press(&mut sb, Action::TempoDownCoarse);
    sa.run_until(42.0);
    sb.run_until(42.0);
    let (snap_a, snap_b) = (sa.snapshot(), sb.snapshot());
    let (w, h1, mut px) = book.screen.render(&snap_a, Some(&snap_b), &sa.track, Some(('A', true)))?;
    let (_, h2, px2) = book.screen.render(&snap_b, Some(&snap_a), &sb.track, Some(('B', false)))?;
    px.extend_from_slice(&px2);
    let path = book.add(
        "Deck",
        "Two decks",
        "The software beatmatching harness: A has the keyboard; phase meters read each other.",
    );
    write_png(&path, w, h1 + h2, &px)
}

fn browser_stories(book: &mut Book) -> Result<()> {
    let demo = workspace_root()?.join("crates/kdj-library/tests/fixtures/demo_export");
    let demo = library::Source {
        name: "USB · DEMO".into(),
        usbanlz: demo.join("PIONEER/USBANLZ"),
        pdb: Some(demo.join("PIONEER/rekordbox/export.pdb")),
        root: Some(demo),
    };
    let libs: Vec<_> = [fake_stick(150)?, demo].iter().map(library::scan).collect();
    let mut sim = playing(&Arc::new(silent_track(240)), 0.5)?;
    let snap = sim.snapshot();
    let playlists = "USB · REKORDBOX/PBAR CURRENT/HOUSE NYC/HOUSE PLAYLISTS";
    let stories: [(&str, &str, &str, isize); 6] = [
        ("Sources", "Two sticks plugged in.", "", 0),
        ("A stick's top level", "All tracks, then its playlist folders.", "USB · REKORDBOX", 1),
        ("Playlist folder", "Tracks found on the stick, and how many are missing.", playlists, 1),
        ("Track list", "All tracks: the loaded one is marked, the highlight filled.", "USB · REKORDBOX/All tracks", 3),
        ("Empty playlist", "Says so instead of a blank screen.", &format!("{playlists}/Pbar Nov 23"), 0),
        ("Deep path", "The path is trimmed from the left.", &format!("{playlists}/Horoom April 24"), 0),
    ];
    for (name, note, path, cursor) in stories {
        let mut b = browse_to(&libs, path)?;
        b.move_by(cursor);
        if name == "Track list" {
            b.loaded = libs.first().and_then(|l| l.entries.get(1)).map(kdj_library::nav::track_id);
        }
        let file = book.add("Browser", name, note);
        book.screen.save_browser_png(&file, &mut b, &snap)?;
    }
    let mut b = kdj_ui::browser::Browser::new(true);
    let file = book.add("Browser", "Reading the library", "Right after start, or a stick going in.");
    book.screen.save_browser_png(&file, &mut b, &snap)?;
    b.scanning = false;
    let file = book.add("Browser", "Nothing to browse", "No collection, no stick.");
    book.screen.save_browser_status_png(&file, &mut b, &snap, "")?;
    Ok(())
}

fn settings_stories(book: &mut Book) -> Result<()> {
    let info = SystemInfo {
        hostname: "kdj-warm-lark".into(),
        output: "snd_rpi_hifiberry_dacplus".into(),
        sample_rate: 48_000,
        link_peers: Some(1),
        controller: "XONE:K3 · connected".into(),
        audio_errors: 0,
    };
    let fresh = || Settings::new(settings::pages(&Prefs::default(), &info));
    let mut s = fresh();
    let file = book.add("Settings", "Choosing a page", "Arrows move through the pages; the current one is filled.");
    book.screen.save_settings_png(&file, &mut s, "")?;
    let mut s = fresh();
    s.sideways(1);
    s.sideways(1);
    let file = book.add(
        "Settings",
        "Playback: changing a setting",
        "In the rows: the page is outlined, the row highlighted, help below.",
    );
    book.screen.save_settings_png(&file, &mut s, "")?;
    for (page, name, note) in [
        (1, "Audio", "Buffer sizes with their latency at the output's rate."),
        (2, "Sync", "Ableton Link and how SYNC lines up."),
        (3, "Controller", "LED colour for the K3, and MIDI learn."),
        (4, "Display", "Zoom, phase meter, perf overlay."),
        (5, "System", "Version and library rescan."),
    ] {
        let mut s = fresh();
        s.move_by(page);
        s.sideways(1);
        let file = book.add("Settings", name, note);
        book.screen.save_settings_png(&file, &mut s, "")?;
    }
    Ok(())
}

fn slug(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn html(stories: &[Story]) -> String {
    let mut h = String::from(concat!(
        "<!doctype html><html lang=en><head><meta charset=utf-8>",
        "<meta name=viewport content='width=device-width,initial-scale=1'><title>deck storybook</title><style>",
        ":root{color-scheme:dark}body{margin:0;background:#050505;color:#eee;font:15px/1.45 system-ui,sans-serif}",
        "header{padding:20px 24px 4px}h1{font-size:20px;margin:0}header p{color:#999;margin:4px 0 0}",
        "nav{position:sticky;top:0;background:#050505ee;padding:10px 24px;display:flex;gap:16px;z-index:1;border-bottom:1px solid #222}",
        "nav a{color:#ccc;text-decoration:none}nav a:hover{color:#fff}",
        "h2{font-size:16px;margin:28px 24px 12px;color:#ccc;font-weight:600}",
        ".grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,420px),1fr));gap:20px;padding:0 24px 24px}",
        "figure{margin:0}figure img{width:100%;display:block;border-radius:6px;border:1px solid #2a2a2a}",
        "figcaption{margin-top:8px}figcaption b{display:block;font-weight:600}figcaption span{color:#999;font-size:14px}",
        "</style></head><body><header><h1>deck storybook</h1>",
        "<p>800×480 screens rendered by the real UI code. Regenerate: <code>cargo run --release -p kdj-sim -- story</code></p></header><nav>"
    ));
    let mut groups: Vec<&str> = Vec::new();
    for s in stories {
        if !groups.contains(&s.group) {
            groups.push(s.group);
        }
    }
    for g in &groups {
        let _ = write!(h, "<a href='#{}'>{g}</a>", slug(g));
    }
    h.push_str("</nav>");
    for g in &groups {
        let _ = write!(h, "<h2 id='{}'>{g}</h2><div class=grid>", slug(g));
        for s in stories.iter().filter(|s| s.group == *g) {
            let _ = write!(
                h,
                "<figure><a href='{f}'><img src='{f}' alt='{n}' loading=lazy></a><figcaption><b>{n}</b><span>{d}</span></figcaption></figure>",
                f = s.file,
                n = esc(s.name),
                d = esc(s.note)
            );
        }
        h.push_str("</div>");
    }
    h.push_str("</body></html>\n");
    h
}
