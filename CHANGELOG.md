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
- **M5 — Tabs and workspaces.** Every tab is a real shell. New `mt_mux` crate (workspace →
  window → tab → pane tree, domains, monotonic ids, `MuxEvent`s). Tab actions: `SpawnTab`,
  `CloseTab` (with `confirm_close`), `ActivateTab`, `ActivateTabRelative`, `MoveTabRelative`,
  `RenameTab` (inline), `SpawnWindow`, `CloseWindow`, drag-to-reorder and middle-click close.
  Tab titles follow `tab_bar.title_format`; a dot marks tabs running a non-shell program; new
  tabs inherit the working directory (`new_tab_cwd`) from the OS process table. tmux-style
  workspaces: one active per process, switcher with window/tab counts, `CreateWorkspace`,
  `RenameWorkspace`, `SwitchWorkspace`, declared `[[workspaces]]`, `startup_workspace` and
  `microterm --workspace NAME`. Launching `microterm` again opens a window in the running
  instance over a private Unix socket. `window.remember_size` is honoured.
  Known gap: OSC 7 cwd reports are not read (the engine exposes no hook); the cwd comes from
  process inspection instead.
- **M6 — Command palette and launcher.** The action registry now covers every SPEC §9.2 action
  (`PastePrimary`, `ScrollToPrompt`, `SendString`, `SpawnCommand`, `ConnectSsh` added) and
  feeds the keymap, the palette and the docs. Palette providers with prefixes `>` actions,
  `@` workspaces, `#` tabs, `~` themes (live preview, `Esc` reverts); nucleo ranking plus an MRU of
  the last 50 items in `$XDG_STATE_HOME/microterm/mru.json`; `Cmd/Ctrl+Enter` opens in a new
  window. The launcher lists `[[launch]]` profiles, "New tab in <domain>" and workspaces and
  refreshes on hot reload. `cargo run -p xtask -- gen-docs` generates Reference → Actions; CI
  fails when it is stale. Known gap: SSH domains join the launcher in M10.
- **M7 — Mouse, selection and hyperlinks.** Mouse reporting for X10-style (1000), drag (1002)
  and motion (1003) tracking with legacy, SGR (1006) and SGR-pixels (1016) encoding; Shift always
  bypasses reporting; wheel events are reported or, in the alternate screen, become arrow keys.
  Selection by drag, double-click (word, honouring `selection.word_chars`), triple-click (line),
  Alt/Option-drag (block) and Shift+click (extend), with auto-scroll past the edges,
  `trim_trailing_whitespace`, `copy_on_select` and the Linux primary selection (middle-click
  pastes it). Right-click follows `mouse.right_click` and opens the context menu (Copy, Paste,
  Select all, Open link, Copy link, Search for selection, Clear scrollback). The pointer is an
  I-beam, a hand over a link while the activation modifier is held, and hides while typing. New
  `mt_patterns` crate: OSC 8 links plus built-in URL and `localhost:PORT` rules and user
  `[[hyperlink_rules]]` with `$0`-`$9`, a scheme allowlist (`hyperlinks.schemes`) and the
  `hyperlinks.modifier` (Cmd on macOS, Ctrl on Linux); links open through the system opener,
  never a shell. Hovered links get a dotted accent underline and a tooltip with the real target.
  New actions `SelectAll` and `SearchSelection`; `Copy` and `PastePrimary` are implemented.
  Invalid `hyperlink_rules[].regex` values are config errors. Known gaps: implicit links do not
  span wrapped rows; the pointer shape updates on the next mouse move after the modifier changes;
  `ftp://` links are recognised but blocked until `ftp` is added to `hyperlinks.schemes`.
- **M8 — Quick select, copy mode and search.** Quick select (Cmd/Ctrl+Shift+Space) scans the
  visible rows with the built-in git SHA, URL, path, UUID, IPv4/IPv6, email, hex colour and
  number patterns plus `quick_select.patterns`, minus `quick_select.disable_builtin`; overlapping
  matches resolve to the longest, matches that wrap rows are found whole, labels come from
  `quick_select.alphabet` (prefix-free, shortest nearest the cursor), a lowercase label copies and
  an uppercase one copies and pastes. Copy mode (Cmd/Ctrl+Shift+X) is a vi-style cursor over the
  whole scrollback with every SPEC §10.5 motion, numeric prefixes, character/line/block
  selection, `o`, `y`/`Y`, `/ ? n N` search, `[`/`]` prompt jumps from OSC 133 marks (recorded in
  `mt_term`), and bindings overridable in `[[copy_mode.keys]]`. The find bar searches scrollback
  and screen with case and regex toggles, highlights visible matches, caps at 10,000 matches and
  shows `n of m`. `mt_term` gains absolute-line access, selection by absolute line and prompt
  marks; `mt_patterns` gains `QuickSet`. New docs: Quick select, Copy mode, Search.
- **`text.wide_regional_indicators`** (default `false`). Counts each regional indicator as two
  cells, so a flag takes four like zsh's line editor assumes. Fixes the cursor jumping to the
  wrong column after zsh-autosuggestions draws a flag emoji. Implemented as a PTY output filter in
  `mt_term` plus a snapshot step, without touching the engine. Hot-reloadable; documented under
  Fonts → Wide characters and emoji. Known gap: a flag at the very last column wraps as two
  narrow indicators.
- **M9 — CLI, IPC and recording.** A flat command set (reorganised as `microterm ctl` in M10.1)
  to list, spawn, send text, read text, activate and rename tabs, kill panes, manage workspaces,
  record, replay and reload the config drives a running instance over a
  versioned JSON-lines protocol (`mt_proto`) on a per-user `0700` Unix socket with peer-uid
  checks and stale-socket cleanup; `--json` everywhere; exit codes 0–4. `send-text` honours
  bracketed paste (`--no-paste` types raw bytes), `get-text` reads line ranges into scrollback
  and re-encodes SGR with `--escapes`. `subscribe` streams mux events. Panes get
  `MICROTERM_PANE` and `MICROTERM_UNIX_SOCKET`. asciicast v2 writer/reader (`mt_record`),
  `microterm record -o FILE -- CMD`, `ctl record` (`--input` is opt-in) and `ctl replay`.
  `microterm completions bash|zsh|fish`. A hidden `--headless` flag runs the mux and IPC server
  without windows for the end-to-end tests. `mt_term` gains output/input taps and styled
  scrollback access; `mt_mux` gains `kill_pane`. New docs: CLI overview, microterm ctl,
  Recording & replay, Environment variables, Exit codes.
- **M10 — SSH domains and reconnection.** Tabs can live on remote hosts and survive dropped
  connections. `microterm-mux` (new `mt_mux_server` crate, no GPUI) is a per-user daemon
  (`serve`, `proxy`, `list`, `kill`) that owns the PTYs and an authoritative terminal per pane,
  behind a `0700` socket with peer-credential checks. On attach it replays the screen,
  scrollback, modes, cursor, colours and title as a synthesized VT stream (`mt_term`
  round-trip tested; `Terminal::remote` runs a pane with no local PTY). `mt_proto::mux` adds the
  postcard frame codec and message set with a `Hello` version check. `mt_ssh` (russh on a
  background Tokio runtime) reads `~/.ssh/config` (`Host` patterns, `ProxyJump`, `ProxyCommand`,
  identities, keepalive), authenticates via agent, identity files, keyboard-interactive and
  password, verifies `known_hosts` (trust dialog; a changed key is a hard failure), bootstraps
  the daemon over SFTP with a `SHA256SUMS`-verified cache, and falls back to a plain session
  with a notice when that fails. Reconnection backs off 1, 2, 4, 8, 16, 30 s with jitter, with a
  real overlay (attempt counter, *Reconnect now*, *Close tab*), blocked input and replay on
  re-attach. `[[ssh_domains]]` entries appear in the launcher with status dots, *Connect to
  host…* opens an ad-hoc domain, `microterm ssh [--domain NAME]` and `ctl tab new --domain` work,
  and the title bar shows the active domain's state. `mt_mux` gains `SshDomain`,
  `DomainState` and `DomainStateChanged`. The M1 debug action `DebugShowSshOverlay` is gone.
  New docs: SSH domains, microterm-mux. Not preserved on replay: scroll region, saved cursor,
  character sets, tab stops, hyperlinks and the normal screen behind a full-screen app.
- **M10.1 — ctl: resource-oriented commands, targeting and selectors.** **`microterm cli` is
  renamed `microterm ctl` and reorganised by resource** (nothing was released, so there is no
  alias and the old flat verbs are gone): `ctl workspace list|create|switch|rename|delete`,
  `ctl window list|new|focus|close`, `ctl tab list|new|focus|rename|close`,
  `ctl pane list|get|send-text|get-text|close`, plus `ctl list|record|replay|reload-config`.
  Old → new: `spawn` → `tab new` / `window new` (now prints `tab= pane= window=`, safe to
  `eval`), `activate-tab` → `tab focus`, `set-tab-title` → `tab rename`, `kill-pane` →
  `pane close`. Items are addressed by `--pane|--tab|--window ID`, by `--match EXPR` (selectors:
  `title:build*,!exited:true`, globs, `re:` regexes, AND, evaluated by `mt_mux::ctl` against
  plain snapshot rows), or from the caller's own pane through the new `MICROTERM_TAB` and
  `MICROTERM_WINDOW` (exported next to `MICROTERM_PANE` and `MICROTERM_UNIX_SOCKET` into local
  panes only, never SSH panes). Read-only verbs fall back to the focused item; input and
  destructive verbs never do. Zero matches exit 3, several exit **5** with the candidates listed
  (`--all` opts in on `close` and `send-text`). Closes honour `confirm_close` without ever
  opening a dialog: if it would ask, exit **8** and nothing is closed, unless `--force`.
  `pane list --json` returns id, tab, window, workspace, domain, title, cwd, process, size,
  focused, exited and exit_code; `--json` errors are one JSON object on stderr. IPC methods are
  renamed (`tab.new|focus|rename|close`, `window.list|focus|close`, `pane.list|get|close`,
  `workspace.delete`, …) at protocol `v:1`, and a test pins the list. New docs: microterm ctl,
  Selectors & targeting; updated CLI overview, Environment variables, Exit codes.
- Component library in `mt_ui::components` (Button, IconButton, Tab, TabBar, Modal, Picker,
  ListItem, KeyBinding, Toast, Banner, Pill, Tooltip, ContextMenu, StatusDot, TextInput).
- Bundled IBM Plex Sans, JetBrains Mono and Lucide icons (`assets/LICENSES.md`).
- Astro + Starlight site scaffold in `site/`; `make site-build`.
- CI (`.github/workflows/ci.yml`): fmt, clippy `-D warnings`, cargo-deny, tests on macOS and Linux.
