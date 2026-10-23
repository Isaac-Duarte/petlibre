.PHONY: help bootstrap baseline bringup plaf103 plaf103-firstboot test

# The firmware under ../firmwares/ that the Rust profiles link.
FIRMWARE ?= reference

help:
	@echo "bootstrap: fetch and verify the pinned SDK and Linux toolchain"
	@echo "baseline: build the unchanged SDK example in a private worktree"
	@echo "bringup: build the Rust diagnostic application in a separate worktree"
	@echo "plaf103: build/audit the diagnostic with stock-derived application OTA bank addresses"
	@echo "plaf103-firstboot: build/audit UART-only SDK mains and the Rust diagnostic"
	@echo "test: run hardware-independent Rust firmware tests on the host"
	@echo "FIRMWARE=<name> picks the firmware the Rust profiles link (default: reference)."
	@echo "These targets only build desktop artifacts; there is no flash target."

# petlibre-build runs as its own executable: `cargo run` would set
# RUSTUP_TOOLCHAIN, which a firmware build refuses as a compiler override.
bootstrap baseline:
	cargo build --locked --release -p petlibre-build
	../target/release/petlibre-build $@

bringup plaf103 plaf103-firstboot:
	cargo build --locked --release -p petlibre-build
	../target/release/petlibre-build $@ --firmware $(FIRMWARE)

test:
	cargo test --locked -p petlibre-core -p petlibre-hal-common -p petlibre-hal -p petlibre-sdk -p petlibre-sdk-c -p petlibre-fw-reference -p petlibre-fw-blink
