# microterm developer entry points (SPEC §15.1).
CARGO ?= cargo

# GPUI compiles Metal shaders at build time, which needs the Xcode Metal toolchain. Without it
# (Command Line Tools only), fall back to GPUI's runtime shader compilation.
ifeq ($(shell uname -s),Darwin)
ifeq ($(shell xcrun --find metal >/dev/null 2>&1 && echo yes),)
FEATURES := --features microterm/runtime-shaders
endif
endif

.PHONY: run build release test lint fmt themes bench bundle install site-dev site-build release-dry check-facade

run:
	$(CARGO) run -p microterm $(FEATURES)

build:
	$(CARGO) build --workspace $(FEATURES)

release:
	$(CARGO) build --workspace --release $(FEATURES)

test:
	$(CARGO) test --workspace $(FEATURES)

fmt:
	$(CARGO) fmt --all

lint: check-facade
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets $(FEATURES) -- -D warnings
	$(CARGO) deny check

# Facade rule (SPEC §3.3): only mt_term may mention alacritty_terminal.
check-facade:
	@if grep -rIl 'alacritty_terminal' crates --include='*.rs' --include='Cargo.toml' | grep -v '^crates/mt_term/'; then \
		echo 'error: only crates/mt_term may use alacritty_terminal'; exit 1; fi

themes:
	$(CARGO) run -q -p xtask -- check-themes

# Headless 100 MB throughput baseline -> bench/results/<date>.json. vtebench must run inside a
# microterm tab: `bench/vtebench.sh` (see bench/README.md).
bench:
	bench/throughput.sh

bundle:
	@echo "bundle: not implemented yet (M11)"

install:
	@echo "install: not implemented yet (M11)"

release-dry:
	@echo "release-dry: not implemented yet (M11)"

site-dev:
	cd site && npm install && npm run dev

site-build:
	cd site && npm ci && npm run build
