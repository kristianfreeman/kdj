#!/bin/sh
# The parts the model uses that are too big for git: the Raspberry Pi 4's
# and Pi 5's STEP models, from step.parts (github.com/earthtojake/step.parts).
set -eu
cd "$(dirname "$0")/.."
mkdir -p parts
for name in raspberry_pi_4_model_b raspberry_pi_5; do
    [ -s "parts/$name.step" ] || curl -fL --progress-bar -o "parts/$name.step" \
        "https://media.githubusercontent.com/media/earthtojake/step.parts/main/catalog/step/$name.step"
done
