---
title: Contributing
description: How to build, test and propose changes to microterm.
---

microterm is MIT licensed and developed in the open at
[github.com/0funct0ry/microterm](https://github.com/0funct0ry/microterm).

## Build and test

```sh
make setup        # toolchain, cargo-deny, cargo-about, site dependencies
make run          # debug build
make test         # all tests
make lint         # rustfmt check, clippy -D warnings, cargo-deny, architecture checks, licences
make themes       # validate the bundled themes
make site-dev     # this documentation site with live reload
```

`make doctor` reports anything that is missing. On macOS without Xcode's Metal toolchain the
Makefile falls back to GPUI's runtime shader compilation by itself.

## Ground rules

- Only `mt_term` may import `alacritty_terminal`; only `mt_ui` may call GPUI. `make lint` greps for
  both, and keeps `mt_cli` and `microterm-mux` free of GPUI.
- Do not copy code from Zed's GPL or AGPL crates. Depend on `gpui` only and follow Zed's design with
  original implementations.
- Bundled fonts, icons and themes need a permissive licence recorded in `assets/LICENSES.md` or
  `themes/LICENSES.md`; `make licenses-check` fails otherwise.
- Pages under *Reference* (actions, template functions, configuration, …) are generated:
  `make docs` regenerates them and `make docs-check` fails if they are stale. Do not edit them.
- Every change updates the docs and adds a line under `## Unreleased` in `CHANGELOG.md`.

## Reporting problems

Open an issue with the output of `microterm --version --verbose`, your OS and, for rendering
problems, your GPU driver. For terminal-behaviour bugs, a recording
(`microterm record -o bug.cast -- COMMAND`) is the best reproduction.
