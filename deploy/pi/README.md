# The deck on a Raspberry Pi 4

From a blank SD card to a Pi that boots straight into the deck: full
screen, touchscreen, DAC hat, Rekordbox USB sticks, Ableton Link on
Ethernet.

## What it sets up

| | |
|---|---|
| `setup.sh` | Installs the build tools, builds the deck on the Pi, installs it to `/opt/deck`, and sets up everything below. Safe to run again after a `git pull`. |
| `deck.service` | Starts the deck at boot on the display (KMS/DRM, no desktop), as the `deck` user, restarting it if it ever exits. Lets the audio thread run at realtime priority and lock its memory, and runs the CPU at full speed. |
| `deck.env` → `/etc/deck/deck.env` | The deck's options: audio output, buffer size, MIDI mapping. |
| `99-deck-usb.rules`, `deck-usb@.service`, `deck-usb` | Mounts USB sticks read-only at `/media/deck/<name>` while they're plugged in, where the browser finds Rekordbox exports. Read-only: the deck never writes to a stick, so pulling one mid-set can't corrupt it. |
| `config.txt` | Added to `/boot/firmware/config.txt`: KMS graphics, the DAC hat's overlay, onboard and HDMI audio off, a quiet boot. |
| `build-rt-kernel.sh` | Optional: builds and installs a PREEMPT_RT kernel. |

## Steps

1. Flash **Raspberry Pi OS Lite (64-bit)** with Raspberry Pi Imager. In its
   settings, set a user, Wi-Fi or Ethernet, and enable SSH.
2. On the Pi (over SSH):

   ```bash
   sudo apt-get install -y git
   git clone <this repository> deck && cd deck
   deploy/pi/setup.sh
   ```

   For an HDMI screen, give its mode: `DECK_HDMI_MODE=800x480@60 deploy/pi/setup.sh`.
3. Check two things in `/boot/firmware/config.txt`, in the block between
   the `deck` markers:
   - **DAC hat**: `hifiberry-dacplus` suits most PCM5122 boards; some want
     `hifiberry-dacplus-std` or `iqaudio-dacplus`.
   - **Display**: nothing for HDMI; for a DSI panel, uncomment its overlay.
4. `sudo reboot`. The deck comes up full screen.

## Afterwards

- **Logs**: `journalctl -u deck -f`. At start it says which audio output it
  opened and at what rate, and whether it got realtime priority and locked
  memory.
- **Audio output**: set `--device` in `/etc/deck/deck.env` to part of the
  output's name. If nothing matches, the log lists every output.
- **Controller**: stop the deck (`sudo systemctl stop deck`), run
  `/opt/deck/bin/deck --midi-learn /etc/deck/k3.toml` (as root, or after
  `sudo chown deck /etc/deck`), then add `--midi /etc/deck/k3.toml` to
  `deck.env` and `sudo systemctl start deck`.
- **Update**: `git pull && deploy/pi/setup.sh --build-only && sudo systemctl restart deck`.
- **Console**: `sudo systemctl stop deck` gives the screen back.
- **Undo the boot changes**: `setup.sh` keeps `config.txt.deck-backup` and
  `cmdline.txt.deck-backup` next to the originals.

## Realtime kernel (optional)

The stock kernel is fine to start with. Under load from the network, USB or
the display, a PREEMPT_RT kernel keeps the audio callback's worst case
tighter. `deploy/pi/build-rt-kernel.sh` builds one natively (an hour or
two), keeps the stock kernel, and switches `config.txt` to the new one; to
go back, comment out `kernel=kernel8-rt.img`. After a reboot, `uname -v`
should say `PREEMPT_RT`.

## Tested so far

Without a Pi: the build and the test suites on Debian 13 (arm64, like
Raspberry Pi OS) with exactly `setup.sh`'s packages; the USB mount script on
loop devices (labels with spaces, two sticks with the same name, read-only,
unmounting); shellcheck on every script. Not yet on hardware: the boot
config, the service on a real display, the DAC overlay, touch, the RT kernel.
