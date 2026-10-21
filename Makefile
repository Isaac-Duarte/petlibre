PYTHON ?= python3

.PHONY: help check check-rust check-python check-cross check-virtual-feeder check-docs check-fixtures

help:
	@echo "check: formatting, Clippy, Rust/Python tests, cross-builds, simulator, repository checks"
	@echo "check-rust / check-python / check-cross / check-virtual-feeder / check-docs: individual checks"
	@echo "check-fixtures: optional reference-binary/SDK evidence; fails if unavailable"
	@echo "Set PYTHON=.venv/bin/python after installing tools/requirements-probe.txt."
	@echo "Checks do not open serial ports, change routing, or send device commands."

check: check-rust check-cross check-virtual-feeder check-python check-docs

check-rust:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo test --locked --workspace --all-targets

check-cross:
	cargo build --locked -p wellfed-ffi --release --target thumbv8m.main-none-eabihf
	WELLFED_BUILD_ID=$$($(PYTHON) firmware/tools/build.py source-id) cargo build --locked -p wellfed-ffi --release --target thumbv8m.base-none-eabi --features km0
	cargo build --locked -p wellfed-probe --profile firmware --target thumbv7em-none-eabi

check-python:
	$(PYTHON) tools/check-python.py

check-virtual-feeder:
	$(MAKE) -C firmware/sim
	$(PYTHON) -m unittest discover -s tools/tests -p test_virtual_feeder.py -v

check-docs:
	$(PYTHON) tools/check-repository.py

check-fixtures:
	$(PYTHON) tools/check-python.py --fixtures
	cargo test --locked --lib ota_image::tests::pinned_real_images_are_immutable_after_load_and_mutated_files_are_refused -- --ignored --exact
