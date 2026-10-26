.PHONY: help bootstrap list baseline bringup device firstboot test

# The board under ../devices/ (empty: the only one), the firmware under
# ../wellfed/ that the Rust profiles link, and an optional image variant
# the firmware declares (`make list`).
DEVICE ?=
FIRMWARE ?= qa
VARIANT ?=

BUILD = ../../../target/release/wellfed-build
DEVICE_ARG = $(if $(DEVICE),--device $(DEVICE))
VARIANT_ARG = $(if $(VARIANT),--variant $(VARIANT))

help:
	@echo "bootstrap: fetch and verify the device platform's pinned SDK and toolchain"
	@echo "list: the devices and firmwares (with their variants) this checkout builds"
	@echo "baseline: build the unchanged SDK example in a private worktree"
	@echo "bringup: build the Rust application in a separate worktree"
	@echo "device: build/audit the application with the device's stock-derived OTA bank addresses"
	@echo "firstboot: build/audit UART-only SDK mains and the Rust application"
	@echo "test: run hardware-independent Rust firmware tests on the host"
	@echo "DEVICE=<name> FIRMWARE=<name> VARIANT=<name> pick what to build (default: the only device, qa, none)."
	@echo "These targets only build desktop artifacts; there is no flash target."

# wellfed-build runs as its own executable: `cargo run` would set
# RUSTUP_TOOLCHAIN, which a firmware build refuses as a compiler override.
$(BUILD): FORCE
	cargo build --locked --release -p wellfed-build

bootstrap: $(BUILD)
	$(BUILD) bootstrap $(DEVICE_ARG)

list: $(BUILD)
	$(BUILD) list

baseline bringup device firstboot: $(BUILD)
	$(BUILD) build --profile $@ $(DEVICE_ARG) --firmware $(FIRMWARE) $(VARIANT_ARG)

test:
	cargo test --locked -p wellfed-session -p wellfed-core -p wellfed-hal-common -p wellfed-hal -p wellfed-sdk -p wellfed-sdk-c -p wellfed-probes \
		-p wellfed-fw-qa -p wellfed-fw-optics -p wellfed-fw-buttons -p wellfed-fw-adc \
		-p wellfed-fw-speaker -p wellfed-fw-motor -p wellfed-fw-trial -p wellfed-fw-blink -p wellfed-fw-feeder -p wellfed-feeder -p wellfed-board-plaf103

.PHONY: FORCE
FORCE:
