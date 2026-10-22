# microterm developer entry points (SPEC §15.1).
#
# Everything needed to build, test, lint, document and package the project goes through this
# file: `make setup` installs what is missing, so cargo and npm never need to be run by hand.
# Run `make` or `make help` for the list of targets.

.DEFAULT_GOAL := help
SHELL := /bin/sh

CARGO ?= cargo
NPM   ?= npm
SITE  := site

# Overridable inputs:
#   ARGS        extra arguments for `make run`          make run ARGS="ssh prod"
#   T           test name filter                        make test T=replay
#   C           one crate                               make test-crate C=mt_term
#   PREFIX      reserved for `make install` (M13)
#   MUX_TARGETS targets built by `make mux-musl`
ARGS ?=
T ?=
C ?=
PREFIX ?= /usr/local
MUX_TARGETS ?= x86_64-unknown-linux-musl aarch64-unknown-linux-musl

# Minimum Node for the Starlight site (site/package.json "engines").
NODE_MIN_MAJOR := 22

# GPUI compiles Metal shaders at build time, which needs the Xcode Metal toolchain. Without it
# (Command Line Tools only), fall back to GPUI's runtime shader compilation.
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
ifeq ($(shell xcrun --find metal >/dev/null 2>&1 && echo yes),)
FEATURES := --features microterm/runtime-shaders
XTASK_FEATURES := --features mt_ui/runtime-shaders
endif
endif

# Per-crate runs (`-p CRATE`) can only enable features the crate itself declares.
CRATE_FEATURES = $(if $(filter microterm,$(C)),$(FEATURES),$(if $(filter mt_ui xtask,$(C)),$(XTASK_FEATURES),))

.PHONY: help setup doctor tools tool-cargo-deny \
	run run-release run-headless build release mux mux-release mux-musl \
	test test-crate test-ssh test-ssh-local ssh-demo ssh-demo-drop check clippy fmt fmt-check lint deny audit \
	check-facade check-mux-gpui check-snippets-pure themes docs docs-check doc \
	bench bundle install release-dry \
	site-install site-dev site-build site-preview \
	ci tree clean clean-site distclean

# ------------------------------------------------------------------------------ help

help: ## Show this list
	@echo "microterm — make targets"; echo
	@awk 'BEGIN{FS=":.*## "} /^[a-zA-Z0-9_%-]+:.*## /{printf "  \033[1m%-16s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
	@echo; echo "Variables: ARGS=…  T=<test filter>  C=<crate>  MUX_TARGETS=…  CARGO=…  NPM=…"

# ------------------------------------------------------------------------------ setup

setup: tools site-install ## Install everything the build needs (toolchain, cargo-deny, site deps)
	@echo "setup done. Try: make doctor"

# `rustup show` installs the toolchain pinned in rust-toolchain.toml (with rustfmt + clippy).
tools: tool-cargo-deny
	@command -v rustup >/dev/null 2>&1 || { echo "error: rustup is required: https://rustup.rs"; exit 1; }
	@rustup show active-toolchain >/dev/null 2>&1 || rustup show >/dev/null

tool-cargo-deny:
	@command -v cargo-deny >/dev/null 2>&1 || { echo "installing cargo-deny…"; $(CARGO) install cargo-deny --locked; }

doctor: ## Report tool versions and anything missing
	@echo "os:         $(UNAME_S) $$(uname -m)"
	@printf "rustup:     "; (command -v rustup >/dev/null 2>&1 && rustup --version 2>/dev/null | head -1) || echo "MISSING (https://rustup.rs)"
	@printf "cargo:      "; $(CARGO) --version 2>/dev/null || echo "MISSING"
	@printf "toolchain:  "; (rustc --version 2>/dev/null) || echo "MISSING (run: make tools)"
	@printf "cargo-deny: "; (cargo-deny --version 2>/dev/null) || echo "missing (make tools)"
	@printf "node:       "; (node --version 2>/dev/null) || echo "missing (needed for the site, >= $(NODE_MIN_MAJOR))"
	@printf "npm:        "; ($(NPM) --version 2>/dev/null) || echo "missing"
	@printf "docker:     "; (docker --version 2>/dev/null) || echo "missing (only for make test-ssh)"
	@printf "zig:        "; (zig version 2>/dev/null) || echo "missing (only for cross-built mux targets)"
ifeq ($(UNAME_S),Darwin)
	@printf "metal:      "; (xcrun --find metal >/dev/null 2>&1 && echo "Xcode Metal toolchain") || echo "not found: using GPUI runtime shaders ($(FEATURES))"
endif
	@printf "site deps:  "; ([ -d $(SITE)/node_modules ] && echo installed) || echo "missing (make site-install)"

# ------------------------------------------------------------------------------ run

# SSH tabs install the daemon from a local build until releases exist (M13): build it and point
# MICROTERM_MUX_BINARY at it unless you set your own. Without it, SSH tabs fall back to plain
# sessions that cannot survive a disconnect.
run: export MICROTERM_MUX_BINARY ?= $(CURDIR)/target/debug/microterm-mux
run: mux ## Run the GUI (debug). Pass arguments with ARGS="…"
	$(CARGO) run -p microterm $(FEATURES) -- $(ARGS)

run-release: ## Run the GUI (release build)
	$(CARGO) run --release -p microterm $(FEATURES) -- $(ARGS)

run-headless: ## Run the windowless mux + IPC server (used by CLI tests)
	$(CARGO) run -p microterm $(FEATURES) -- --headless

# ------------------------------------------------------------------------------ build

build: ## Build the whole workspace (debug)
	$(CARGO) build --workspace $(FEATURES)

release: ## Build the whole workspace (release)
	$(CARGO) build --workspace --release $(FEATURES)

mux: ## Build the microterm-mux daemon for this machine (debug)
	$(CARGO) build -p mt_mux_server
	@echo "-> target/debug/microterm-mux   (MICROTERM_MUX_BINARY=$$PWD/target/debug/microterm-mux)"

mux-release: ## Build the microterm-mux daemon for this machine (release)
	$(CARGO) build --release -p mt_mux_server
	@echo "-> target/release/microterm-mux"

# Static Linux daemons for remote hosts. The native build needs only the musl target; any other
# combination (e.g. aarch64 from x86_64, or any Linux target from macOS) is cross-built with
# cargo-zigbuild, which needs `zig` on PATH (brew install zig / your package manager).
mux-musl: $(addprefix mux-target-,$(MUX_TARGETS)) ## Static Linux microterm-mux binaries (MUX_TARGETS)

mux-target-%:
	@rustup target add $* >/dev/null
	@host=$$(rustc -vV | sed -n 's/^host: //p'); \
	native_musl=$$(echo $* | sed 's/-musl$$/-gnu/'); \
	if [ "$$host" = "$$native_musl" ] && [ "$$(uname -s)" = Linux ]; then \
		$(CARGO) build --release -p mt_mux_server --target $*; \
	else \
		command -v zig >/dev/null 2>&1 || { echo "error: building $* here needs zig on PATH"; exit 1; }; \
		command -v cargo-zigbuild >/dev/null 2>&1 || $(CARGO) install cargo-zigbuild --locked; \
		$(CARGO) zigbuild --release -p mt_mux_server --target $*; \
	fi
	@echo "-> target/$*/release/microterm-mux"

# ------------------------------------------------------------------------------ test

test: ## Run the whole test suite (T=<filter> to narrow by test name)
	$(CARGO) test --workspace $(FEATURES) $(if $(T),-- $(T),)

test-crate: ## Run one crate's tests: make test-crate C=mt_term [T=filter]
	@test -n "$(C)" || { echo "usage: make test-crate C=<crate> [T=<filter>]"; exit 1; }
	$(CARGO) test -p $(C) $(CRATE_FEATURES) $(if $(T),-- $(T),)

test-ssh: ## SSH integration tests against Docker openssh-server (needs MICROTERM_MUX_BINARY for Linux)
	scripts/test-ssh.sh

ssh-demo: mux ## Start a throw-away local sshd and print the microterm.toml block for it (PORT=, NAME=)
	scripts/ssh-demo.sh $(or $(PORT),2299) $(or $(NAME),demo)

ssh-demo-drop: ## Drop the demo sshd's live connections (tests reconnection; the listener stays up)
	scripts/ssh-demo.sh --drop

test-ssh-local: mux ## SSH integration tests against a throw-away local sshd (no Docker, no root)
	scripts/test-ssh.sh --local

bench: ## Headless 100 MB throughput baseline -> bench/results/ (vtebench: see bench/README.md)
	bench/throughput.sh

# ------------------------------------------------------------------------------ quality

check: ## Fast type-check of every target (no codegen)
	$(CARGO) check --workspace --all-targets $(FEATURES)

fmt: ## Format all Rust code
	$(CARGO) fmt --all

fmt-check: ## Fail if formatting differs
	$(CARGO) fmt --all -- --check

clippy: ## Clippy with warnings as errors
	$(CARGO) clippy --workspace --all-targets $(FEATURES) -- -D warnings

deny: tool-cargo-deny ## Licence, advisory and source checks (cargo-deny)
	$(CARGO) deny check

audit: deny ## Alias of deny

lint: check-facade check-mux-gpui check-snippets-pure fmt-check clippy deny ## fmt check + clippy -D warnings + cargo-deny + architecture checks

# Facade rule (SPEC §3.3): only mt_term may mention alacritty_terminal.
check-facade:
	@if grep -rIl 'alacritty_terminal' crates --include='*.rs' --include='Cargo.toml' | grep -v '^crates/mt_term/'; then \
		echo 'error: only crates/mt_term may use alacritty_terminal'; exit 1; fi

# microterm-mux is copied to remote hosts: it must not pull in GPUI or Tokio (SPEC §3.1).
check-mux-gpui:
	@if $(CARGO) tree -p mt_mux_server 2>/dev/null | grep -Ei 'gpui|tokio'; then \
		echo 'error: mt_mux_server must not depend on gpui or tokio'; exit 1; fi

check-snippets-pure: # mt_snippets stays free of GPUI and the terminal engine (SPEC §10.8, D12)
	@if $(CARGO) tree -p mt_snippets 2>/dev/null | grep -Ei 'gpui|alacritty'; then \
		echo 'error: mt_snippets must not depend on gpui or alacritty_terminal'; exit 1; fi

themes: ## Validate the bundled themes (licence, 16 colours, contrast)
	$(CARGO) run -q -p xtask $(XTASK_FEATURES) -- check-themes

docs: ## Regenerate generated docs (reference/actions.md, agent-skill.md, terminal-api.md, context-menus.md, template-functions.md) from the registries
	$(CARGO) run -q -p xtask $(XTASK_FEATURES) -- gen-docs

docs-check: ## Fail if generated docs are stale
	$(CARGO) run -q -p xtask $(XTASK_FEATURES) -- gen-docs --check

doc: ## Build the Rust API docs
	$(CARGO) doc --workspace --no-deps $(FEATURES)

tree: ## Show the workspace dependency tree
	$(CARGO) tree --workspace

# ------------------------------------------------------------------------------ site

$(SITE)/node_modules: $(SITE)/package-lock.json
	cd $(SITE) && $(NPM) ci
	@touch $(SITE)/node_modules

site-install: ## Install the documentation site's dependencies
	@command -v node >/dev/null 2>&1 || { echo "error: node >= $(NODE_MIN_MAJOR) is required"; exit 1; }
	@[ "$$(node -p 'process.versions.node.split(".")[0]')" -ge $(NODE_MIN_MAJOR) ] || { echo "error: node >= $(NODE_MIN_MAJOR) is required (found $$(node --version))"; exit 1; }
	@$(MAKE) --no-print-directory $(SITE)/node_modules

site-dev: site-install ## Run the site with live reload
	cd $(SITE) && $(NPM) run dev

site-build: site-install ## Build the static site into site/dist
	cd $(SITE) && $(NPM) run build

site-preview: site-build ## Serve the built site locally
	cd $(SITE) && $(NPM) run preview

# ------------------------------------------------------------------------------ CI

ci: check-facade check-mux-gpui check-snippets-pure fmt-check clippy deny test themes docs-check site-build ## Everything CI runs

# ------------------------------------------------------------------------------ packaging (M13)

bundle: ## (M13) macOS .app/.dmg and Linux tarball
	@echo "bundle: not implemented yet (M13)"

install: ## (M13) install microterm and microterm-mux under PREFIX
	@echo "install: not implemented yet (M13)"

release-dry: ## (M13) dry-run of the release pipeline
	@echo "release-dry: not implemented yet (M13)"

# ------------------------------------------------------------------------------ clean

clean: ## Remove Rust build output
	$(CARGO) clean

clean-site: ## Remove the site's build output and dependencies
	rm -rf $(SITE)/dist $(SITE)/.astro $(SITE)/node_modules

distclean: clean clean-site ## Remove all build output and installed dependencies
