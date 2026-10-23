PYTHON ?= python3

.PHONY: help check check-rust check-python check-cross check-virtual-feeder check-docs check-fixtures

help:
	@echo "check: formatting, Clippy, Rust/Python tests, cross-builds, simulator, repository checks"
	@echo "check-rust / check-python / check-cross / check-virtual-feeder / check-docs: individual checks"
	@echo "check-fixtures: optional reference-binary/SDK evidence; fails if unavailable"
	@echo "Set PYTHON=.venv/bin/python after installing tools/requirements-probe.txt."
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
	cargo clippy --locked --workspace --lib --bins --exclude wellfed-bench -- -D warnings $(PRODUCTION_LINTS)
	cargo test --locked --workspace --all-targets

check-cross:
	cargo build --locked -p wellfed-ffi --release --target thumbv8m.main-none-eabihf
	WELLFED_BUILD_ID=$$(cargo run --quiet --locked --release -p wellfed-build -- source-id) cargo build --locked -p wellfed-ffi --release --target thumbv8m.base-none-eabi --features km0

check-python:
	$(PYTHON) tools/check-python.py

check-virtual-feeder:
	cargo test --locked -p wellfed-sim --test virtual_feeder

check-docs:
	cargo xtask check-repository

check-fixtures:
	$(PYTHON) tools/check-python.py --fixtures
	cargo test --locked --lib ota_image::tests::pinned_real_images_are_immutable_after_load_and_mutated_files_are_refused -- --ignored --exact
	cargo test --locked -p wellfed-audit --lib -- --ignored
	cargo test --locked -p wellfed-build -- --ignored
