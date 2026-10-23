PYTHON ?= python3

.PHONY: help check check-rust check-python check-cross check-virtual-feeder check-docs check-fixtures ui check-web

help:
	@echo "check: formatting, Clippy, Rust/Python tests, cross-builds, simulator, repository checks"
	@echo "check-rust / check-python / check-cross / check-virtual-feeder / check-docs: individual checks"
	@echo "check-fixtures: optional reference-binary/SDK evidence; fails if unavailable"
	@echo "Set PYTHON=.venv/bin/python after installing src/tools/requirements-probe.txt."
	@echo "ui: build the web page (pnpm in src/web/) that petlibre-bench embeds with --features ui"
	@echo "check-web: web types, unit tests, build, then the page end to end against petlibre-bench"
	@echo "Checks do not open serial ports, change routing, or send device commands."

check: check-rust check-cross check-virtual-feeder check-python check-docs

# Library and binary code only: input never panics on `unwrap`, a narrowing
# `as` never wraps silently, and nothing is `pub` that no caller can reach.
# `expect` stays allowed for invariants, with the reason as its message.
PRODUCTION_LINTS = -D clippy::unwrap_used -D clippy::cast_possible_truncation \
	-D clippy::needless_pass_by_value -D unreachable_pub

check-rust:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo clippy --locked --workspace --lib --bins -- -D warnings $(PRODUCTION_LINTS)
	cargo test --locked --workspace --all-targets

check-cross:
	cargo build --locked -p petlibre-sdk-c --release --target thumbv8m.main-none-eabihf
	PETLIBRE_BUILD_ID=$$(cargo run --quiet --locked --release -p petlibre-build -- source-id) cargo build --locked -p petlibre-sdk-c --release --target thumbv8m.base-none-eabi --features km0

check-python:
	$(PYTHON) src/tools/check-python.py

check-virtual-feeder:
	cargo test --locked -p petlibre-sim --test virtual_feeder

check-docs:
	cargo xtask check-repository

check-fixtures:
	$(PYTHON) src/tools/check-python.py --fixtures
	cargo test --locked --lib ota_image::tests::pinned_real_images_are_immutable_after_load_and_mutated_files_are_refused -- --ignored --exact
	cargo test --locked -p petlibre-audit --lib -- --ignored
	cargo test --locked -p petlibre-build -- --ignored

# The optional web page. Node and pnpm are needed only here, never by `check`.
ui:
	pnpm -C src/web install --frozen-lockfile
	pnpm -C src/web build

# The end-to-end tests serve a synthetic private/ tree from a temporary
# directory; they never read this checkout's evidence or open a port to hardware.
check-web: ui
	pnpm -C src/web check
	pnpm -C src/web test
	cargo clippy --locked -p petlibre-bench --all-targets --features ui -- -D warnings
	cargo test --locked -p petlibre-bench --features ui
	cargo build --locked -p petlibre-bench --features ui
	pnpm -C src/web e2e
