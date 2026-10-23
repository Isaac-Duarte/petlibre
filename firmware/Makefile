.PHONY: help bootstrap baseline bringup plaf103 plaf103-firstboot test

help:
	@echo "bootstrap: fetch and verify the pinned SDK and Linux toolchain"
	@echo "baseline: build the unchanged SDK example in a private worktree"
	@echo "bringup: build the Rust diagnostic application in a separate worktree"
	@echo "plaf103: build/audit the diagnostic with stock-derived application OTA bank addresses"
	@echo "plaf103-firstboot: build/audit UART-only SDK mains and the Rust diagnostic"
	@echo "test: run hardware-independent Rust firmware tests on the host"
	@echo "These targets only build desktop artifacts; there is no flash target."

# wellfed-build runs as its own executable: `cargo run` would set
# RUSTUP_TOOLCHAIN, which a firmware build refuses as a compiler override.
bootstrap baseline bringup plaf103 plaf103-firstboot:
	cargo build --locked --release -p wellfed-build
	../target/release/wellfed-build $@

test:
	cargo test --locked -p wellfed-core -p wellfed-hal-common -p wellfed-hal -p wellfed-fw-reference -p wellfed-sdk-c
