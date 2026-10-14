# Changelog

## Unreleased

### Added
- **M1 — UI shell.** Cargo workspace (SPEC §3.2), `microterm` CLI skeleton (`--version`, `--help`,
  stub subcommands), and the full GPUI window chrome running on mock data: title bar with
  workspace chip and domain indicator, Zed-style tab bar, mock terminal viewport, and every v1
  overlay (command palette, launcher, workspace switcher, theme selector with live preview, quick
  select, copy mode, search, toasts, config error banner, SSH reconnect overlay, resize HUD).
  Default keybindings from SPEC §9.3 for macOS and Linux. One Dark / One Light themes.
- Component library in `mt_ui::components` (Button, IconButton, Tab, TabBar, Modal, Picker,
  ListItem, KeyBinding, Toast, Banner, Pill, Tooltip, ContextMenu, StatusDot, TextInput).
- Bundled IBM Plex Sans, JetBrains Mono and Lucide icons (`assets/LICENSES.md`).
- Astro + Starlight site scaffold in `site/`; `make site-build`.
- CI (`.github/workflows/ci.yml`): fmt, clippy `-D warnings`, cargo-deny, tests on macOS and Linux.
