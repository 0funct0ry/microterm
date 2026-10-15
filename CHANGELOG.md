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
- **M3 — Configuration, hot reload and themes.** `mt_config` implements the full v1 TOML schema
  (SPEC §8.3) with defaults, `--config` / `$MICROTERM_CONFIG` / XDG path resolution, precise
  `line:column` errors, unknown-key warnings with "did you mean" suggestions, semantic checks,
  keybinding parsing (`cmd+shift+p`, `{ ActivateTab = 3 }`, `Disable`) and a diff that classifies
  every change (repaint / re-layout / keymap / patterns / new panes / restart). The file, its
  parent directory and the themes directory are watched with a 150 ms debounce; a valid change is
  applied atomically, an invalid one keeps the last valid config and shows the error banner
  (Open config / Dismiss). `ReloadConfig` and `OpenConfig` (`$VISUAL`/`$EDITOR` in a new tab,
  else the system opener) work. `mt_theme` derives UI tokens in OKLCH and bundles 53 themes with
  recorded licences (`themes/LICENSES.md`; Zenburn is dropped because it is GPL); user themes load
  from `<config_dir>/themes`, and `theme = { dark, light }` follows the OS appearance. The theme
  selector lists every theme with live preview and Esc reverts. New CLI: `microterm config
  path|check|edit|show-defaults` and `microterm themes list|show`; `make themes` validates the
  bundled set (licence, 16 colours, ≥ 4.5:1 contrast). Applied keys: theme, font family / size /
  line height / weights, padding, opacity and blur, cursor style / blink, scrollback, shell, env,
  `term`, `tab_bar.position` / `hide_if_single`, scrollbar mode, bell mode, paste confirmation,
  `clipboard.osc52`, scroll multiplier and `[[keys]]`. Other schema keys are accepted and
  validated now and take effect in the milestone that implements them.
- **M4 — Typography.** The full `[font]` table now drives rendering: `ligatures`
  (calt/liga/dlig), extra OpenType `features` and the ordered `fallback` list. Symbols Nerd Font
  Mono (MIT) is bundled and is the first default fallback. Rows are shaped in segments: plain
  text runs shape together so ligatures span exactly their cells, and a run is reshaped without
  ligatures when the cursor (or, later, a selection boundary) is inside one. Fallback, wide,
  emoji and combining glyphs sit on their own cells, scaled to fit and centred; regional
  indicator pairs draw as flags. `IncreaseFontSize` / `DecreaseFontSize` / `ResetFontSize` work
  per window and resize the PTYs; the line cache is keyed by font generation and size. New
  `microterm ls-fonts [--text STR]` prints the font chosen for each character and why. Known gaps:
  the terminal engine has no grapheme clustering, so ZWJ sequences and skin tones keep one cell
  pair per scalar.
- Component library in `mt_ui::components` (Button, IconButton, Tab, TabBar, Modal, Picker,
  ListItem, KeyBinding, Toast, Banner, Pill, Tooltip, ContextMenu, StatusDot, TextInput).
- Bundled IBM Plex Sans, JetBrains Mono and Lucide icons (`assets/LICENSES.md`).
- Astro + Starlight site scaffold in `site/`; `make site-build`.
- CI (`.github/workflows/ci.yml`): fmt, clippy `-D warnings`, cargo-deny, tests on macOS and Linux.
