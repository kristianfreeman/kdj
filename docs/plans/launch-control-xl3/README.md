# Launch Control XL 3

A plan for running kdj's decks from a Novation Launch Control XL 3 instead of
the Xone:K3. Nothing here is built yet, and nothing has been tried on
hardware.

The controller does deck controls only: transport, cue, hot cues, loops, beat
jump, tempo, jog, sync and browse. The Xone:96 is the mixer, so no control
here touches volume or EQ. One Launch Control can drive four decks; a K3
split in four would be too cramped.

## Why consider it

- Its screen and RGB encoder lights are fully controlled by the host. kdj
  can show the active layer on the screen, name a control as it's touched,
  and put loop size or tempo offset on the knob's own light. The K3 lights
  only its buttons, in red, amber or green.
- In DAW mode every control, Page, Track, Shift and Mode included, is a
  plain CC, and the same CC sets its light. That's simpler than the K3's
  layers and note-offset colours.
- It runs on USB-C bus power.

What it costs:

- The encoders don't push. On the K3, one knob scrolls the browser and a
  click loads the track; here loading needs a button.
- Hot cues need a page switch.
- kdj's MIDI layer would need DAW-mode SysEx and CC lights. Today it drives
  lights through notes with `led_offset`.

For two decks the K3 stays. This is for when four decks happen, or if
layers on the K3 turn out to be confusing mid-set.

## The hardware

- 24 endless encoders in 3 rows of 8, each with an RGB light. They don't
  push.
- 8 faders of 60 mm.
- 16 buttons in 2 rows of 8.
- Left-side buttons: Page ▲▼, Track ◀▶, Record, Play, Shift, Mode,
  Solo/Arm and Mute/Select.
- A 128×64 monochrome OLED.
- USB-C, plus 5-pin MIDI in, out and out2/thru.

## Layout

Each deck gets two columns: deck A in columns 1–2, B in 3–4, C in 5–6 and D
in 7–8.

| Row | Left column | Right column |
|---|---|---|
| Encoders 1 | Jog | Tempo fine |
| Encoders 2 | Loop size | Beat jump |
| Encoders 3 | Browse | Hot cue select |
| Buttons 1 | Cue | Play |
| Buttons 2 | Loop | Sync |
| Fader | Tempo | spare |

Page ▲▼ switches between this deck page and a cues page, where the 16
buttons are hot cues A–D for each deck. Shift gives a second layer.

The screen only shows the active page or layer, plus a control's name while
it's touched. The decks themselves are on kdj's screens.

## Protocol notes

The source is Novation's programmer's guide v1.0, kept here as text:
[`programmers-guide.txt`](programmers-guide.txt).

**DAW mode.** Turn it on with `9F 0C 7F` or `F0 00 20 29 02 15 02 7F F7`,
and off with `…02 00 F7`. Controls report over USB only:

- Encoders and faders on channel 16.
- Buttons on channel 1.
- Shift on channel 7.

The same CC numbers set the lights. Mode changes come in on channel 7,
CC 1Eh: 1 is DAW Mixer, 2 is DAW Control, and 6–9 are custom modes 1–4.

**Relative encoders.** `B6 <row> 7F` puts a row in relative mode and
`B6 <row> 00` takes it back. Row 1 is 45h and row 2 is 48h.

**Screen, configure.** `F0 00 20 29 02 15 04 <target> <config> F7`.

- Targets: 05h–0Ch are the faders, 0Dh–24h the encoders, 35h the stationary
  display and 36h the overlay.
- Config bit 6 shows the display automatically on change, and bit 5 on
  touch. Bits 0–4 choose the arrangement:
  - 1: name and text value.
  - 2: title, name and value.
  - 3: title and 8 names.
  - 4: name and numeric value.

**Screen, text.** `F0 00 20 29 02 15 06 <target> <field> <text> F7`. Text
is ASCII 20h–7Eh, plus 1Bh (empty box), 1Ch (filled box), 1Dh (flat) and
1Eh (heart).

**Screen, bitmap.** `F0 00 20 29 02 15 09 <20h|21h> <1216 bytes> 7F`.

- The bitmap is 19 bytes per row for 64 rows, at 7 pixels per byte.
- The device answers `…09 7F` when it's ready for the next one, and it holds
  only one bitmap.
- The guide doesn't say how fast it updates.

## Open questions

- How fast the screen refreshes, which needs the hardware to find out.
- What Shift's second layer holds.
- Whether one controller split across Pis works for four decks the way the
  K3 split works for two.
