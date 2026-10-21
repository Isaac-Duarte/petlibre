# Virtual feeder worker: the Rust owner and ports on the fake SDK (host only).
BUILD := ../../private/firmware-tests
.PHONY: all
all:
	cargo build --locked --release -p petlibro-sim
	mkdir -p $(BUILD)
	cp ../../target/release/virtual-feeder-worker $(BUILD)/virtual-feeder-worker
