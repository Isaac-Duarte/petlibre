.PHONY: help check check-rust check-deps check-cross check-virtual-feeder check-docs check-fixtures ui check-web sqlx-prepare

help:
	@echo "check: formatting, Clippy, Rust tests, cross-builds, simulator, repository checks"
	@echo "check-rust / check-deps / check-cross / check-virtual-feeder / check-docs: individual checks"
	@echo "check-fixtures: optional reference-binary/SDK evidence; fails if unavailable"
	@echo "ui: build the web page (pnpm in src/web/) that wellfed-dashboard embeds with --features ui"
	@echo "check-web: web types, unit tests, build, then the page end to end against wellfed-bench --ui"
	@echo "sqlx-prepare: regenerate the server store's query metadata (.sqlx/) after changing a query or migration"
	@echo "Checks do not open serial ports, change routing, or send device commands."

check: check-rust check-deps check-cross check-virtual-feeder check-docs

# Library and binary code only: input never panics on `unwrap`, a narrowing
# `as` never wraps silently, and nothing is `pub` that no caller can reach.
# `expect` stays allowed for invariants, with the reason as its message.
PRODUCTION_LINTS = -D clippy::unwrap_used -D clippy::cast_possible_truncation \
	-D clippy::needless_pass_by_value -D unreachable_pub

check-rust:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo clippy --locked --workspace --lib --bins -- -D warnings $(PRODUCTION_LINTS)
	cargo nextest run --locked --workspace --all-targets
	cargo test --locked --workspace --doc

# Licenses, advisories, duplicate versions and sources (deny.toml), then the
# out-of-workspace fuzz crate's own lockfile under its own policy. Advisories
# need the RustSec database, which the first run fetches.
check-deps:
	cargo deny --locked check
	cargo deny --manifest-path src/fuzz/Cargo.toml --locked check --config src/fuzz/deny.toml

check-cross:
	WELLFED_BUILD_ID=$$(cargo run --quiet --locked --release -p wellfed-build -- source-id) cargo build --locked -p wellfed-hal-common --release --target thumbv8m.base-none-eabi --features km0

check-virtual-feeder:
	cargo test --locked -p wellfed-sim --test virtual_feeder

check-docs:
	cargo xtask check-repository

check-fixtures:
	cargo test --locked -p wellfed-emu -p wellfed-flash -- --ignored
	cargo test --locked -p wellfed-audit --lib -- --ignored
	cargo test --locked -p wellfed-build -- --ignored
	cargo test --locked -p wellfed-bench --lib -- --ignored

# The optional web page. Node and pnpm are needed only here, never by `check`.
ui:
	pnpm -C src/web install --frozen-lockfile
	pnpm -C src/web build

# The end-to-end tests serve a synthetic private/ tree from a temporary
# directory; they never read this checkout's evidence or open a port to hardware.
# The page's crate and the two binaries that serve it with `--ui` are linted
# and tested with it built in, the production lints included.
PAGE_CRATES = -p wellfed-dashboard -p wellfed-bench -p wellfed-itest
check-web: ui
	pnpm -C src/web check
	pnpm -C src/web test
	cargo clippy --locked $(PAGE_CRATES) --all-targets --features ui -- -D warnings
	cargo clippy --locked $(PAGE_CRATES) --lib --bins --features ui -- -D warnings $(PRODUCTION_LINTS)
	cargo test --locked $(PAGE_CRATES) --features ui
	cargo build --locked $(PAGE_CRATES) --features ui
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
