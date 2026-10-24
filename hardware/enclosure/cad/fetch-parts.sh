#!/bin/sh
# The parts the model uses that are too big for git: the Raspberry Pi 4's
# STEP model, MIT, from step.parts (github.com/earthtojake/step.parts).
set -eu
cd "$(dirname "$0")/.."
mkdir -p parts
curl -fL --progress-bar -o parts/raspberry_pi_4_model_b.step \
    https://media.githubusercontent.com/media/earthtojake/step.parts/main/catalog/step/raspberry_pi_4_model_b.step
