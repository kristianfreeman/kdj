# The deck on a Raspberry Pi 4

From a blank SD card to a Pi that boots straight into the deck: full
screen, touchscreen, DAC hat, Rekordbox USB sticks, Ableton Link on
Ethernet.

## What it sets up

| | |
|---|---|
| `setup.sh` | Installs the build tools, builds the deck on the Pi, and runs `install.sh`. Safe to run again after a `git pull`. |
| `install.sh` | Installs a built deck to `/opt/deck` and sets up everything below (shared by `setup.sh` and the image build), including a systemd preset so that a fresh image's first boot doesn't turn the background services it disables back on. |
| `image/build.sh` | Builds a ready-to-flash SD card image with all of this done (see below). |
| `deck.service` | Starts the deck at boot on the display (KMS/DRM, no desktop), as the `deck` user, without waiting for the network, and restarts it whenever it exits, forever. Lets the audio thread run at realtime priority and lock its memory, and runs the CPU at full speed. |
| `deck.env` → `/etc/deck/deck.env` | The deck's options: audio output, buffer size, MIDI mapping. |
| `99-deck-usb.rules`, `deck-usb@.service`, `deck-usb` | Mounts USB sticks read-only at `/media/deck/<name>` while they're plugged in, where the browser finds Rekordbox exports. Read-only: the deck never writes to a stick, so pulling one mid-set can't corrupt it. |
| `config.txt` | Added to `/boot/firmware/config.txt`: KMS graphics, the DAC hat's overlay, onboard and HDMI audio off, Bluetooth off, no splash. |
| `/etc/sudoers.d/deck` | Lets the deck power off and restart the Pi (`systemctl poweroff` / `reboot`, nothing else). |
| `build-rt-kernel.sh` | Optional: builds and installs a PREEMPT_RT kernel. |

## What it's like to use

Power on: a black screen, then the deck. Boot messages go to tty3, out of
sight; there's no login prompt on the screen and no splash. If the deck
ever exits, it's back a second later. Power off or restart from the deck's
Settings → System. For a console, SSH in, or `sudo systemctl stop deck` and
switch to another VT with a keyboard (Ctrl+Alt+F2).

In the background nothing competes with it mid-set: the apt, man-db,
e2scrub and dpkg backup timers are off, as are Bluetooth and waiting for
the network at boot. Wi-Fi and SSH work as Imager set them.

## Flash a ready-made image

On a Mac or Linux machine with Docker (Apple Silicon runs the arm64
containers natively):

```bash
make image          # or deploy/pi/image/build.sh
```

It downloads the latest Raspberry Pi OS Lite (64-bit) (cached in
`target/image/cache`, checked against Raspberry Pi's published SHA-256),
builds the deck in a Debian container of the same release, installs it in
the image with `install.sh`, and writes
`target/image/deck-rpi4-<date>.img.xz` and its `.sha256`, after checking
the result, including booting it in a container. About 2 minutes on an
Apple Silicon Mac once the caches are warm. `RASPIOS_URL=…` pins a
particular release.

Flash it with Raspberry Pi Imager: **Raspberry Pi 4**, **Use custom** → the
`.img.xz`, then edit the OS customisation: a user and password, SSH (a key
or password), Wi-Fi if you use it. The hostname is `deck` unless you set
another there. The image has no user, password or keys of its own; without
a user from Imager, first boot shows Raspberry Pi OS's own "create a user"
dialog on the screen (it needs a keyboard) before the deck. First boot grows
the filesystem to fill the card and applies the customisation (it may
restart once), then the deck starts.

Already set up in the image: the deck at `/opt/deck/bin/deck`, starting at
boot, `/etc/deck/deck.env` with `--device hifiberry --buffer 128`, the
example MIDI mapping in `/etc/deck/`, USB stick mounting, power off and
restart for the deck user, the `config.txt` block and quiet `cmdline.txt`,
and the hostname `deck`. Check, with the card in your computer (the
boot partition) or over SSH:

- **DAC hat** and **display** in `config.txt`, as in step 3 below.
- **HDMI mode**, if the screen needs one: add `video=HDMI-A-1:800x480@60`
  to the single line in `cmdline.txt`.
- After booting: `journalctl -u deck` says which audio output it opened (or,
  if the DAC isn't found, that it fell back to the default and which outputs
  there are) and whether it got realtime priority.

## Set up by hand

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
  output's name. If nothing matches, the deck plays on the default output
  and the log lists every output.
- **Controller**: stop the deck (`sudo systemctl stop deck`), run
  `/opt/deck/bin/deck --midi-learn /etc/deck/k3.toml` (as root, or after
  `sudo chown deck /etc/deck`), then add `--midi /etc/deck/k3.toml` to
  `deck.env` and `sudo systemctl start deck`.
- **Update**: `git pull && deploy/pi/setup.sh --build-only && sudo systemctl restart deck`.
  On a Pi flashed from the image, copy over a new `target/image/build/deck`
  and `sudo install -m 755 deck /opt/deck/bin/deck && sudo systemctl restart deck`
  (or flash a new image).
- **Undo the boot changes**: `install.sh` keeps `config.txt.deck-backup` and
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
unmounting); shellcheck on every script. The image build checks the image
itself: every library the deck links resolves and every symbol binds; it
starts, finds no DAC, falls back to the default output and gets as far as
the display; the units pass `systemd-analyze verify`; and the finished
image boots in a container (systemd-nspawn, a first boot) with the deck
enabled and restarting, the background services still off and the sudoers
rule in place. Not yet on hardware: the boot config, the service on a real
display, the DAC overlay, touch, Imager's customisation, the RT kernel.
