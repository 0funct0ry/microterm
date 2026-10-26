# Terminal conformance notes

The machine-checked list lives in `crates/mt_term/tests/vt_conformance.toml`: one row per sequence in
Ghostty's VT reference, each with an emulator test and a status of `supported`, `partial` or
`unsupported` with a reason. `cargo test -p mt_term vt_conformance` runs it and `make docs` renders
it into the site's *Escape sequences* page (`reference/terminal-api.md`, generated).

## Known gaps

- **DECSLRM (left/right margins)** is not supported; see `docs/adr/0002-decslrm-margins.md`.
- **Inline graphics** (Kitty, iTerm2, Sixel) arrive in v1.2.
- **OSC 9 / ConEmu**: only `9;4` progress and the plain notification form are handled; other ConEmu
  subcommands are parsed and ignored.

## How to extend

Add a `[[row]]` with `[[row.case]]` input and assertions, run the test, then `make docs`. A sequence
that is deliberately not implemented gets `status = "unsupported"` and a `notes` line saying why.

## Per-platform notes

Fill in during each release's manual pass (`docs/release-checklist.md`):

| Target | Terminal-visible differences |
|---|---|
| macOS arm64 / x64 | none known |
| Linux Wayland (GNOME, KDE) | none known |
| Linux X11 | none known |
