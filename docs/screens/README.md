# Screens

Every screen of the deck at the panel's own 800×480, drawn by the real UI code (`kdj-sim story`). `make screens` redraws them; the pre-commit hook does when the UI changes, so each commit shows what it did to them.

## Deck

**No track**: First start: tapping anywhere opens the browser.

![No track](deck-no-track.png)

**Stopped at the cue**: Paused, parked on the cue: CUE lit, play outlined.

![Stopped at the cue](deck-stopped-at-the-cue.png)

**Playing**: Auto grid (no Rekordbox analysis), key lock and quantize on.

![Playing](deck-playing.png)

**Loop, 4 beats**: Hatched on both waveforms, bracketed; 4 and LOOP filled.

![Loop, 4 beats](deck-loop-4-beats.png)

**Hot cues A and B**: Flags on the detail waveform, ticks on the overview.

![Hot cues A and B](deck-hot-cues-a-and-b.png)

**+6% on ±16, key lock off**: KEY outlined; the range badge reads ±16.

![+6% on ±16, key lock off](deck-6-on-16-key-lock-off.png)

**SYNC to Link**: Following a 136 BPM session with one peer: locked, the meter at zero.

![SYNC to Link](deck-sync-to-link.png)

**MASTER on Link**: This deck sets the session's tempo for two peers.

![MASTER on Link](deck-master-on-link.png)

**Alone on Link, not synced**: The phase meter reads against the session.

![Alone on Link, not synced](deck-alone-on-link-not-synced.png)

**A track from deck B's stick**: Loaded across the network; it plays from memory.

![A track from deck B's stick](deck-a-track-from-deck-b-s-stick.png)

**Lending this stick**: Deck B has a track from the stick in this kdj: bright, not dim.

![Lending this stick](deck-lending-this-stick.png)

**Locked**: No loading until SHIFT + LOCK: the badge shows only while it's locked.

![Locked](deck-locked.png)

**Tempo fader catching up**: Hollow: where the hardware fader is; solid: the tempo.

![Tempo fader catching up](deck-tempo-fader-catching-up.png)

**Last 30 seconds**: Time remaining flashes inverted on the beat.

![Last 30 seconds](deck-last-30-seconds.png)

**Loading**: Status bottom right while the next track loads.

![Loading](deck-loading.png)

**Long title, long error**: Both cut short, never off screen.

![Long title, long error](deck-long-title-long-error.png)

**Two decks**: The software beatmatching harness: A has the keyboard; phase meters read each other.

![Two decks](deck-two-decks.png)

## Browser

**Sources**: Two sticks plugged in.

![Sources](browser-sources.png)

**A stick's top level**: All tracks, then its playlist folders.

![A stick's top level](browser-a-stick-s-top-level.png)

**Playlist folder**: Tracks found on the stick, and how many are missing.

![Playlist folder](browser-playlist-folder.png)

**Track list**: All tracks: the loaded one is marked, the highlight filled.

![Track list](browser-track-list.png)

**Mixing in key**: Keys on the Camelot wheel; ones that go with the playing track's (beside KEY) marked: the same key on a block, a neighbour boxed.

![Mixing in key](browser-mixing-in-key.png)

**Empty playlist**: Says so instead of a blank screen.

![Empty playlist](browser-empty-playlist.png)

**Deep path**: The path is trimmed from the left.

![Deep path](browser-deep-path.png)

**Reading the library**: Right after start, or a stick going in.

![Reading the library](browser-reading-the-library.png)

**Nothing to browse**: No collection, no stick.

![Nothing to browse](browser-nothing-to-browse.png)

## Settings

**Choosing a page**: Arrows move through the pages; the current one is filled.

![Choosing a page](settings-choosing-a-page.png)

**Playback: changing a setting**: In the rows: the page is outlined, the row highlighted, help below.

![Playback: changing a setting](settings-playback-changing-a-setting.png)

**Audio**: Buffer sizes with their latency at the output's rate.

![Audio](settings-audio.png)

**Sync**: Ableton Link, how SYNC lines up, and this deck's letter.

![Sync](settings-sync.png)

**Network**: Wi-Fi on or off, its network and address, and Ethernet's.

![Network](settings-network.png)

**Controller**: LED colour for the K3, the mapping drawn, and MIDI learn.

![Controller](settings-controller.png)

**Display**: Zoom, phase meter, perf overlay.

![Display](settings-display.png)

**System**: Version and library rescan.

![System](settings-system.png)

## Mapping

**Xone:K3**: config/xone-k3.toml: deck A's columns filled, deck B's outlined; unused pots and faders faint.

![Xone:K3](mapping-xone-k3.png)

**Focused deck**: config/midi-example.toml on the K3: no deck given, so they follow the focused deck (dark fill).

![Focused deck](mapping-focused-deck.png)

**Unknown controller**: No drawing for its port: the bindings as a table.

![Unknown controller](mapping-unknown-controller.png)

**No mapping**: Started without --midi.

![No mapping](mapping-no-mapping.png)
