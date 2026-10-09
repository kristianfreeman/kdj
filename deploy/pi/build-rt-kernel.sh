#!/bin/bash
# Optional: build a PREEMPT_RT kernel for the Pi 4, natively on the Pi
# (an hour or two), from the Raspberry Pi kernel branch it's running now.
# The stock kernel stays; this adds /boot/firmware/kernel8-rt.img and
# switches config.txt to it. To go back, comment out `kernel=kernel8-rt.img`
# in config.txt (on the Pi, or with the SD card in another computer).
#
# The deck runs fine on the stock kernel; RT tightens worst-case latency
# when other things (network, USB, the display) are busy.
set -euo pipefail

src=${RT_KERNEL_SRC:-$HOME/rpi-linux}
branch="rpi-$(uname -r | cut -d. -f1-2).y"
fw=/boot/firmware
[ -d "$fw" ] || fw=/boot

sudo apt-get install -y --no-install-recommends git bc bison flex libssl-dev make libc6-dev libncurses-dev
if [ ! -d "$src" ]; then
    git clone --depth=1 --branch "$branch" https://github.com/raspberrypi/linux "$src"
fi
cd "$src"

# The Pi 4's config, made fully preemptible.
make bcm2711_defconfig
./scripts/config --enable EXPERT --enable PREEMPT_RT --set-str LOCALVERSION "-v8-rt"
make olddefconfig
if ! grep -q '^CONFIG_PREEMPT_RT=y' .config; then
    echo "This kernel ($branch) doesn't offer PREEMPT_RT on arm64 (needs 6.12 or later)." >&2
    exit 1
fi

make -j"$(nproc)" Image.gz modules dtbs
sudo make -j"$(nproc)" modules_install
sudo cp arch/arm64/boot/Image.gz "$fw/kernel8-rt.img"
sudo sed -i 's/^#kernel=kernel8-rt.img$/kernel=kernel8-rt.img/' "$fw/config.txt"
grep -q '^kernel=kernel8-rt.img$' "$fw/config.txt" || echo 'kernel=kernel8-rt.img' | sudo tee -a "$fw/config.txt" >/dev/null

echo "Installed $fw/kernel8-rt.img. Reboot, then: uname -v  (should say PREEMPT_RT)"
