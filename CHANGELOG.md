# Changelog

## Unreleased

### Added
- **M1 — UI shell.** Cargo workspace (SPEC §3.2), `microterm` CLI skeleton (`--version`, `--help`,
  stub subcommands), and the full GPUI window chrome running on mock data: title bar with
  workspace chip and domain indicator, Zed-style tab bar, mock terminal viewport, and every v1
  overlay (command palette, launcher, workspace switcher, theme selector with live preview, quick
  select, copy mode, search, toasts, config error banner, SSH reconnect overlay, resize HUD).
  Default keybindings from SPEC §9.3 for macOS and Linux. One Dark / One Light themes.
- **M2 — Live terminal.** The first tab runs your shell on a real PTY. `mt_term` wraps
  `alacritty_terminal` (VT parsing, grid, scrollback) behind a facade: `Terminal`, `Emulator`,
  `RenderSnapshot`, coalesced wakeups, key/paste encoding (legacy xterm and the Kitty keyboard
  protocol), shell resolution and the OSC 52 policy. `mt_ui` paints the grid on the GPU with
  cached row layouts, merged backgrounds, all SGR underline styles and colours, procedural box
  drawing / block / Braille / powerline glyphs, block/beam/underline/hollow cursors with blinking,
  a real overlay scrollbar, wheel scrolling and scroll actions, visual bell, resize with HUD,
  multi-line paste confirmation, and OSC 0/2 titles. `make bench` records a 100 MB throughput
  baseline in `bench/results/`. Linux prints the missing Vulkan package for your distro.
  Known gaps: SGR 21 and overline come from the engine, not drawn as specified.
- Component library in `mt_ui::components` (Button, IconButton, Tab, TabBar, Modal, Picker,
  ListItem, KeyBinding, Toast, Banner, Pill, Tooltip, ContextMenu, StatusDot, TextInput).
- Bundled IBM Plex Sans, JetBrains Mono and Lucide icons (`assets/LICENSES.md`).
- Astro + Starlight site scaffold in `site/`; `make site-build`.
- CI (`.github/workflows/ci.yml`): fmt, clippy `-D warnings`, cargo-deny, tests on macOS and Linux.
