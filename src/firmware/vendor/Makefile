.PHONY: help bootstrap list baseline bringup device firstboot test

# The board under ../devices/ (empty: the only one), the firmware under
# ../petlibre/ that the Rust profiles link, and an optional image variant
# the firmware declares (`make list`).
DEVICE ?=
FIRMWARE ?= qa
VARIANT ?=

BUILD = ../../../target/release/petlibre-build
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

# petlibre-build runs as its own executable: `cargo run` would set
# RUSTUP_TOOLCHAIN, which a firmware build refuses as a compiler override.
$(BUILD): FORCE
	cargo build --locked --release -p petlibre-build

bootstrap: $(BUILD)
	$(BUILD) bootstrap $(DEVICE_ARG)

list: $(BUILD)
	$(BUILD) list

baseline bringup device firstboot: $(BUILD)
	$(BUILD) build --profile $@ $(DEVICE_ARG) --firmware $(FIRMWARE) $(VARIANT_ARG)

test:
	cargo test --locked -p petlibre-core -p petlibre-hal-common -p petlibre-hal -p petlibre-sdk -p petlibre-sdk-c -p petlibre-probes \
		-p petlibre-fw-qa -p petlibre-fw-optics -p petlibre-fw-buttons -p petlibre-fw-adc \
		-p petlibre-fw-speaker -p petlibre-fw-motor -p petlibre-fw-trial -p petlibre-fw-blink -p petlibre-fw-ble

.PHONY: FORCE
FORCE:
