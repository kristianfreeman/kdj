# deck

One DJ deck for a Raspberry Pi 4 feeding a Xone:96 channel: transport, CDJ-style
cue, tempo with key lock, beat-quantized loops, Pioneer 3-band waveforms.

## Play a track

```bash
cargo run --release -p deck
```

It opens in the browser: your Rekordbox collection and any Rekordbox USB
export that's plugged in. Like a CDJ, a prepared track brings its own grid,
3-band waveform, memory cue and hot cues; anything else is analysed on load
and marked "auto grid". Loading is refused while the deck plays.

Options: `TRACK` (load a file directly), `--decks 2` (two decks stacked and
mixed, for beatmatching), `--fullscreen`, `--buffer 128` (frames per audio
callback), `--device NAME` (the audio output whose name contains NAME; the
error lists them if none matches), `--hud` (perf overlay), `--keymap
keys.toml`, `--midi k3.toml` / `--midi-learn k3.toml` / `--midi-port NAME`
(see MIDI controllers), `--no-link` (stay out of Ableton Link sessions),
`--synth` (synthetic tracks, no files needed).

On macOS this needs SDL2: `brew install sdl2`.

## Keys

| Key | Action | Key | Action |
|---|---|---|---|
| Space | play / pause | C | cue (hold to preview) |
| ↑ / ↓ | tempo ±0.02% (±0.05% on ±16) | PgUp / PgDn | tempo ±1% |
| R | tempo reset | T | range ±8 / ±16 |
| K | key lock | Q | quantize |
| → / ← | nudge, building while held (step ⅛ beat when stopped) | L | loop on / off |
| 1–6 | beat loop 1, 2, 4, 8, 16, 32 | [ / ] | halve / double loop |
| , / . | beat jump back / forward by loop size | G | bar 1 here |
| - / = | grid earlier / later 2 ms | ; / ' | BPM ∓0.01 |
| H / D | halve / double BPM | Z / X | zoom out / in |
| F1–F8 | hot cues A–H (empty: store here) | 9 / 0 | jog back / forward |
| 7 / 8 | phase step −2 / +2 ms | \` | perf overlay |
| Tab | browser | \\ | switch deck (two decks); hold Shift for the other deck |
| S | SYNC: follow the Link session | M | MASTER: lead the Link session |

In the browser: ↑/↓ and PgUp/PgDn move, ↵ opens or loads, → opens, ←
backs out, Tab closes. Play, cue and anything not listed still reach the deck.

Override any of these in a TOML file passed with `--keymap`:

```toml
[keys]
Return = "Cue"

[browser_keys]
Space = "BrowseLoad"
```

## Touch

On the touchscreen (or with a mouse) every control on screen is a button:
play, CUE (previews while held), the loop sizes and LOOP, and the MASTER /
SYNC / KEY / Q / range badges. Tap the title to open the browser. Touch or
drag along the overview to needle-search while the deck is stopped; a
playing deck ignores it, like a CDJ's needle lock. In the browser, tap a
row to highlight it and tap it again to open or load it; drag to scroll; tap
the path at the top to go back and the bottom-right corner to close.

## Ableton Link

Every deck joins the Link session on the local network (`--no-link` to
opt out), so two Pis, or a deck and Ableton Live, share tempo and phase over
Ethernet.

- MASTER (M) makes a deck's tempo the session's tempo, and lines the
  session's bars up with it when it takes over. One deck is master at a
  time: taking it hands the other deck back to SYNC. A stopped master only
  sets the tempo; nudging the master moves the session's phase, not its tempo.
- SYNC (S) follows the session: it matches the tempo (or its half or double:
  a 70 BPM grid follows 140) and, when it goes on or the deck starts, lines
  the bars up with one click-free jump if they're far out, then gentle tempo
  trims. From then on it holds the beat: a beat jump or a loop moves the bar
  alignment with it instead of being undone, and a loop that isn't whole
  beats follows the tempo only. Turning SYNC off keeps the synced tempo.

With `--no-link` (or a build without Link), SYNC and MASTER still work
between the decks of one `--decks 2` process, on a session of their own.
The header shows SYNC / MASTER and, on Link, the number of peers. Builds need
cmake and libclang for Link; build without it with `--no-default-features`.

## MIDI controllers

Any MIDI controller works; map it once with MIDI learn, which asks for each
control in turn (play, cue, jog encoder, tempo encoder, loops, hot cues,
browser…) and writes a mapping file:

```bash
cargo run --release -p deck -- --midi-learn k3.toml
cargo run --release -p deck -- --midi k3.toml
```

The mapping's `port` is part of the controller's input name (ALSA's
trailing client:port numbers, which change between boots, are ignored);
without one the first input that isn't ALSA's "Midi Through" is used.
`--midi-port NAME` picks the port for both `--midi` and `--midi-learn`,
overriding the file. Pads and buttons press and release; endless encoders
send one press-and-release per click (jog, tempo, loop size, browse). The
deck waits for the controller if it isn't there yet and reconnects on its
own if it's unplugged. Deck actions go from the MIDI
thread straight into the engine, without waiting for a screen frame. See
`config/midi-example.toml` for the format.

## Simulate

Everything except the window and the sound card runs headless in `deck-sim`:

```bash
cargo run --release -p deck-sim -- scenarios                 # scripted sessions in scenarios/
cargo run --release -p deck-sim -- perf                      # perf budgets (perf-budgets.toml)
cargo run --release -p deck-sim -- analyze ~/Music/*.mp3     # detected BPM and bar 1 for real files
cargo run --release -p deck-sim -- frame out.png --loop 4    # render the screen to PNG
cargo run --release -p deck-sim -- frame out.png --decks 2   # two decks with the phase meter
cargo run --release -p deck-sim -- browser out.png --open "Collection"   # the browser
cargo run --release -p deck-sim -- rekordbox bench           # our detector vs your Rekordbox grids
```

## Tests

`cargo test` is the fast suite (well under a second of test time): engine
behaviour, a few tempos of grid detection, and the Rekordbox readers on the
2-track demo export. The full suite adds the 16-tempo sweep and a
row-by-row comparison with rekordcrate over a real ~3,900-track export; run
it occasionally:

```bash
cargo test -- --ignored
```

## Layout

- `crates/deck-engine`: transport, loops, hot cues, nudging, key lock, fallback analysis. No I/O, no UI.
- `crates/deck-library`: Rekordbox data: analysis files (grids, cues, waveforms) and `export.pdb`.
- `crates/deck-ui`: the 800×480 screen in OpenGL (3.3 core / ES 3.0).
- `crates/deck`: the app: SDL2 window, cpal audio, keyboard.
- `crates/deck-sim`: virtual audio clock, scenarios, perf budgets, PNG frames.

## Licence

GPL-3.0-or-later (see `LICENSE`), so the deck can sync with Ableton Link,
which is GPL-2.0-or-later. Geist (in `assets/fonts`) is under the SIL Open
Font License 1.1 (`assets/fonts/OFL.txt`). The test fixtures in
`crates/deck-library/tests/fixtures` come from rekordcrate (MPL-2.0).
