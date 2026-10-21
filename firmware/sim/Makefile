# Virtual feeder worker: the Rust owner and ports on the fake SDK (host only).
BUILD := ../../private/firmware-tests
# Honour an overridden Cargo target directory (relative paths are cwd-relative).
CARGO_TARGET_DIR ?= ../../target
.PHONY: all
all:
	cargo build --locked --release -p petlibro-sim
	mkdir -p $(BUILD)
	cp $(CARGO_TARGET_DIR)/release/virtual-feeder-worker $(BUILD)/virtual-feeder-worker
