# Plan: browsing and loading tracks from other kdjs

Status: proposed, not started (2026-10-11).

## Goal

Two or more kdjs on one wired network work like CDJs on Pro DJ Link: a USB
stick in any of them can be browsed from every one, and a track loads onto
any deck no matter which kdj the stick is in. One kdj (or a laptop) can act
as the library host for the others.

## What we have to build on

- `net.rs` already finds the other kdjs (a multicast hello each second with
  name, deck letter and a UDP port) and passes controller messages between
  them. Discovery is done; this adds a second, bigger channel next to it.
- A loaded track lives entirely in RAM (decoded, resampled). Once a track
  is on a deck, playback never touches the network again, so a stick pulled
  or a network glitch can't stop a playing track. That's the property that
  makes sharing safe; the plan keeps it.
- `kdj-library` reads a source (a stick's `export.pdb`, or the local
  collection's analysis files) into a `Library` of `Entry`s and `Playlist`s.
  `nav` lists sources, then playlists, then tracks. The browser only sees
  `nav`. Remote sources can slot in as more `Library`s.
- Loading reads the audio file and the track's `ANLZ` files from disk paths
  (`load_in_background`, `anlz::prepared`, `decode::decode`).

## Who's the host: every kdj with a stick

Recommendation: **every kdj shares its own sticks** (as CDJs do), and every
kdj browses everyone's. "The host" is then simply whichever kdj has the
stick. If it reboots, only its stick's tracks go missing from the others'
browsers; nothing else depends on it.

The alternative, one designated host that serves a library to the others,
falls out of the same design for free: a kdj with no stick serves nothing,
and a **headless kdj** (`kdj --serve`, no screen or audio) can run on a
laptop or a spare Pi to share a Rekordbox collection or a big drive. That
is the "laptop as library host" setup, and it needs no extra protocol.

## Design

### Library service (each kdj)

A small read-only HTTP/1.1 server on a TCP port, announced in the hello
(add `library_port: u16` and a `library_stamp: u32` that changes whenever
sticks come and go or are rescanned). HTTP because it has range requests,
it's easy to test with `curl`, and small crates exist for both ends
(`tiny_http` for the server, `ureq` for the client; or hand-rolled, as there
are four routes).

| Route | Answer |
| --- | --- |
| `GET /sources` | The shared sources: id, name, stamp, track count (JSON) |
| `GET /sources/{id}/library` | The scanned `Library`: entries and playlists, paths replaced by entry numbers (JSON, cached per stamp; ~1 MB for 2000 tracks) |
| `GET /sources/{id}/tracks/{n}/anlz/{dat,ext}` | The track's analysis files (beat grid, waveform, cues; ~100 KB) |
| `GET /sources/{id}/tracks/{n}/audio` | The audio file, with `Range` support |

Rules:
- Only files a scanned entry points to are served, by number, never by a
  path from the request (no path traversal).
- Read-only, and only to kdjs that have said hello (as the UDP side already
  does). It's still plain HTTP on the LAN; fine for a DJ booth network, not
  for a hostile one.
- Served from a normal-priority thread. The audio thread is realtime and its
  memory is locked, so serving can't make it drop out, but serving while
  loading a track ourselves competes for the stick's bandwidth. Measure.

### Remote sources (each kdj's browser)

- `library::Source` gets an origin: local, or `Remote { peer, source id }`.
  A remote source is named after the stick and the kdj it's in:
  "KRISTIAN on B (kdj-oak-pike)", like a CDJ showing the player number.
- When a peer's `library_stamp` changes, fetch its `/sources`, then each
  changed source's `/library`, in the background. The `Scan` and
  `SourceStamp` machinery in `main.rs` already rescans on change; extend it
  with remote stamps.
- A peer that goes silent (the hello times out) takes its sources out of
  the browser. A deck with one of its tracks keeps playing it.

### Loading a remote track

1. Fetch the ANLZ files into memory; `anlz::prepared` gets a bytes-based
   twin of its path-based API.
2. Fetch the audio into memory, then decode from a `Cursor` (symphonia takes
   any `Read + Seek`). The memory guard (`frame_budget`) counts the file's
   bytes too. On gigabit Ethernet a 10 MB MP3 takes about 0.1 s and a 60 MB
   WAV about 0.5 s.
3. Later, if memory is tight: decode straight from the network with a
   range-request reader (`Read + Seek` over HTTP), so the file is never
   held whole.
4. Errors say where: "Couldn't load: the stick in kdj-oak-pike went away".

### The network itself

- **Through a switch or router:** works as is (DHCP, mDNS, multicast).
- **A cable straight between two Pis:** there's no DHCP server, so neither
  gets an address. NetworkManager must fall back to link-local
  (`ipv4.link-local=fallback` on the wired connection, NetworkManager 1.40
  and later; trixie has 1.52). Bake that into the image and test it. mDNS
  and multicast work on link-local.
- Wi-Fi works but is slow for loading and adds latency to shared
  controllers. The settings could say which one is in use.

## Phases

1. **Library service and discovery.** The HTTP server, the hello fields,
   `kdj --serve` (headless). Tests: serve `tests/fixtures/demo_export`,
   fetch each route; refuse paths and unknown numbers; range requests.
2. **Remote sources in the browser.** Fetch and cache libraries, list them
   by kdj, refresh on stamp changes, drop them when a peer goes. Storybook
   story with a remote source. Test with two kdj processes on one machine
   (as `net.rs` does).
3. **Loading remote tracks.** Bytes-based ANLZ and decode, memory guard,
   errors. Measure load times on the Pis: local stick vs. over Ethernet vs.
   Wi-Fi, and the serving Pi's audio while it serves (no dropouts).
4. **Polish.** Direct-cable link-local in the image. Show which track each
   deck has loaded, as CDJs do ("on B"). Optionally stream-decode.

Phase 1 and 2 are independent of the audio path and safe to build first;
phase 3 is where the Pi's 1 GB matters.

## Open questions

- Should every kdj share its sticks by default (recommended), or only when
  turned on in the settings?
- Will a laptop with Rekordbox be a host (`kdj --serve` on the Mac, sharing
  the local collection)? If so, the local collection's playlists matter, and
  its database is encrypted (we only see tracks, not playlists, today).
- Two Pis on a direct cable, or through a switch/router? That decides
  whether link-local setup is phase 1 or phase 4.
