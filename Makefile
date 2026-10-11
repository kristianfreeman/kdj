# One place for the checks. `make check` before pushing; `make full` rarely.
.PHONY: check lint fmt test test-full perf scenarios full linux image card flash screens hooks deploy

check: lint test scenarios

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --locked -- -D warnings
	cargo deny check
	cargo machete

fmt:
	cargo fmt --all

# Fast suite: well under a second of test time.
test:
	cargo test --workspace --locked

# Large fixtures and sweeps; run occasionally.
test-full:
	cargo test --workspace --locked -- --include-ignored

scenarios:
	cargo run -q --release -p kdj-sim -- scenarios

# Perf budgets (perf-budgets.toml); exits non-zero if anything is over.
perf:
	cargo run -q --release -p kdj-sim -- perf

# Every screen at the panel's own 800x480 into docs/screens, with a README
# GitHub shows as a gallery. The pre-commit hook does it when the UI changes.
screens:
	cargo run -q --release -p kdj-sim -- story docs/screens --scale 1

# Use the repository's git hooks (.githooks): screens redrawn on commit.
hooks:
	git config core.hooksPath .githooks

# Lint, test and build on Linux arm64 (the Pi) in Docker.
linux:
	tools/linux-check.sh

# SD cards: what's plugged in and on them; write the newest image to one.
card:
	cargo run -q --release -p kdj-flash

flash:
	cargo run -q --release -p kdj-flash -- flash

# This checkout's deck onto running kdj Pis over SSH (every one on the
# network, or HOSTS="kdj-oak-pike.local ..."): seconds, no reflash.
deploy:
	tools/deploy.sh $(HOSTS)

# Raspberry Pi 4 SD card image with the deck preinstalled, in Docker
# (target/image/kdjos-rpi4-<date>.img.xz; see deploy/pi/README.md).
image:
	deploy/pi/image/build.sh

full: check test-full perf
