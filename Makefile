.PHONY: help check check-rust check-deps check-cross check-bench-board check-virtual-feeder check-docs check-fixtures ui check-web sqlx-prepare server-dist server-image check-server-release

help:
	@echo "check: formatting, Clippy, Rust tests, cross-builds, simulator, repository checks"
	@echo "check-rust / check-deps / check-cross / check-virtual-feeder / check-docs: individual checks"
	@echo "check-bench-board: Clippy and a release build of the bench board's RP2040 firmware (part of check-cross)"
	@echo "check-fixtures: optional reference-binary/SDK evidence; fails if unavailable"
	@echo "ui: build the web page (pnpm in src/web/) that wellfed-bench serve --ui embeds (built with --features ui)"
	@echo "check-web: web types, unit tests, build, then the page end to end against wellfed-bench serve --ui on a fake relay board"
	@echo "sqlx-prepare: regenerate the server store's query metadata (.sqlx/) after changing a query or migration"
	@echo "server-dist / server-image: the server's static release binaries in dist/server/ and its container image (docker or podman)"
	@echo "check-server-release: the release check against dist/server/ and the image"
	@echo "Checks do not open serial ports, change routing, or send device commands."

check: check-rust check-deps check-cross check-virtual-feeder check-docs

# Library and binary code only: input never panics on `unwrap`, a narrowing
# `as` never wraps silently, and nothing is `pub` that no caller can reach.
# `expect` stays allowed for invariants, with the reason as its message.
PRODUCTION_LINTS = -D clippy::unwrap_used -D clippy::cast_possible_truncation \
	-D clippy::needless_pass_by_value -D unreachable_pub

# The last six lines check the bench variants' injections in wellfed-hal
# (`stop-fail`, and `feed-faults` for `feed-jam` and `sensor-fault`), which
# no workspace build enables.
check-rust:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo clippy --locked --workspace --lib --bins -- -D warnings $(PRODUCTION_LINTS)
	cargo nextest run --locked --workspace --all-targets
	cargo test --locked --workspace --doc
	cargo clippy --locked -p wellfed-hal --features stop-fail --all-targets -- -D warnings
	cargo clippy --locked -p wellfed-hal --features stop-fail --lib -- -D warnings $(PRODUCTION_LINTS)
	cargo nextest run --locked -p wellfed-hal --features stop-fail --test inject
	cargo clippy --locked -p wellfed-hal --features feed-faults --all-targets -- -D warnings
	cargo clippy --locked -p wellfed-hal --features feed-faults --lib -- -D warnings $(PRODUCTION_LINTS)
	cargo nextest run --locked -p wellfed-hal --features feed-faults --test feed_faults

# Licenses, advisories, duplicate versions and sources (deny.toml), then the
# out-of-workspace fuzz and bench board crates' own lockfiles under their own
# policies. Advisories need the RustSec database, which the first run fetches.
check-deps:
	cargo deny --locked check
	cargo deny --manifest-path src/fuzz/Cargo.toml --locked check --config src/fuzz/deny.toml
	cargo deny --manifest-path $(BENCH_BOARD)/Cargo.toml --locked check --config $(BENCH_BOARD)/deny.toml

check-cross: check-bench-board
	WELLFED_BUILD_ID=$$(cargo run --quiet --locked --release -p wellfed-build -- source-id) cargo build --locked -p wellfed-hal-common --release --target thumbv8m.base-none-eabi --features km0

# The bench board's firmware is its own workspace, built from its folder so
# its .cargo/config.toml (target, linker script, UF2 runner) applies. It only
# builds: nothing here touches a board or a feeder.
BENCH_BOARD = src/firmware/bench/wellfed-bench-board
check-bench-board:
	cd $(BENCH_BOARD) && cargo clippy --locked --all-targets -- -D warnings
	cd $(BENCH_BOARD) && cargo clippy --locked --bins -- -D warnings $(PRODUCTION_LINTS)
	cd $(BENCH_BOARD) && cargo build --locked --release

check-virtual-feeder:
	cargo test --locked -p wellfed-sim --test virtual_feeder

check-docs:
	cargo xtask check-repository

check-fixtures:
	cargo test --locked -p wellfed-emu -p wellfed-flash -- --ignored
	cargo test --locked -p wellfed-audit --lib -- --ignored
	cargo test --locked -p wellfed-build -- --ignored
	cargo test --locked -p wellfed-rig --lib -- --ignored

# The optional web page. Node and pnpm are needed only here, never by `check`.
ui:
	pnpm -C src/web install --frozen-lockfile
	pnpm -C src/web build

# The end-to-end tests run `wellfed-bench serve --ui` on a synthetic private/
# tree in a temporary directory, with a scripted relay board (`fake-board`)
# and a stand-in flash client; they never read this checkout's evidence or
# open a port to hardware. The page's crate and the bench, which serves it
# with `--ui`, are linted and tested with it built in, the production lints
# included.
PAGE_CRATES = -p wellfed-dashboard -p wellfed-bench
PAGE_FEATURES = ui,wellfed-bench/fake-board
check-web: ui
	pnpm -C src/web check
	pnpm -C src/web test
	cargo clippy --locked $(PAGE_CRATES) --all-targets --features $(PAGE_FEATURES) -- -D warnings
	cargo clippy --locked $(PAGE_CRATES) --lib --bins --features $(PAGE_FEATURES) -- -D warnings $(PRODUCTION_LINTS)
	cargo test --locked $(PAGE_CRATES) --features $(PAGE_FEATURES)
	cargo build --locked -p wellfed-bench --features $(PAGE_FEATURES)
	pnpm -C src/web e2e

# The server store's queries are checked at build time against committed
# metadata, so no build needs a database. After changing a query or a
# migration, this rebuilds the metadata from a scratch database that the
# sqlite3 command line applies the migrations to.
STORE = src/server/wellfed-store
STORE_DB = $(CURDIR)/target/sqlx-prepare.db
sqlx-prepare:
	rm -rf $(STORE)/.sqlx $(STORE_DB)
	mkdir -p $(STORE)/.sqlx $(dir $(STORE_DB))
	cat $(STORE)/migrations/*.sql | sqlite3 $(STORE_DB)
	touch $(STORE)/src/lib.rs
	DATABASE_URL=sqlite://$(STORE_DB) SQLX_OFFLINE_DIR=$(CURDIR)/$(STORE)/.sqlx cargo check --locked -p wellfed-store
	rm -f $(STORE_DB)

# The server's release: static musl binaries for x86_64 and aarch64 in
# dist/server/, and the container image, both built in a container so the
# host needs only docker or podman (CONTAINER=podman). Never part of
# `check`. check-server-release runs the release check against this host's
# binary from dist/server/ and against the image; `check` already runs it
# against the debug build. docs/server/running.md#packaging.
CONTAINER ?= docker
SERVER_CONTAINERFILE = src/server/wellfed-server/packaging/Containerfile
SERVER_DIST = dist/server
SERVER_IMAGE ?= wellfed-server
server-dist:
	$(CONTAINER) build -f $(SERVER_CONTAINERFILE) --target dist --output type=local,dest=$(SERVER_DIST) .

server-image:
	$(CONTAINER) build -f $(SERVER_CONTAINERFILE) -t $(SERVER_IMAGE) .

check-server-release:
	WELLFED_RELEASE_SERVER=$(CURDIR)/$(SERVER_DIST)/wellfed-server-$$(uname -m)-unknown-linux-musl cargo nextest run --locked -p wellfed-server --test release
	WELLFED_RELEASE_IMAGE=$(SERVER_IMAGE) WELLFED_RELEASE_RUNTIME=$(CONTAINER) cargo nextest run --locked -p wellfed-server --test release --run-ignored only
