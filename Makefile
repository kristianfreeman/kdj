# One place for the checks. `make check` before pushing; `make full` rarely.
.PHONY: check lint fmt test test-full perf scenarios full linux image

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
	cargo run -q --release -p deck-sim -- scenarios

# Perf budgets (perf-budgets.toml); exits non-zero if anything is over.
perf:
	cargo run -q --release -p deck-sim -- perf

# Lint, test and build on Linux arm64 (the Pi) in Docker.
linux:
	tools/linux-check.sh

# Raspberry Pi 4 SD card image with the deck preinstalled, in Docker
# (target/image/deck-rpi4-<date>.img.xz; see deploy/pi/README.md).
image:
	deploy/pi/image/build.sh

full: check test-full perf
