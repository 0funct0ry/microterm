---
title: Hot reload
description: Edit the config file and see the change immediately.
---

microterm watches its config file, the directory that contains it (editors save by renaming a
temporary file over the original) and the `themes/` directory next to it. Changes are debounced
for 150 ms, then the file is parsed, validated, compared with the active config and applied.

The swap is **atomic**: either the whole new config applies or none of it does.

## What each change does

| Change | Effect |
|---|---|
| `theme`, `cursor`, `text`, `window.opacity`, `scrollbar`, `bell`, `paste`, … | repaint |
| `font.*`, `window.padding`, `tab_bar.*` | cell size is recomputed and every PTY is resized |
| `ctl.*` | read when the next request arrives; nothing to rebuild |
| `hyperlink_rules`, `hyperlinks`, `quick_select` | link patterns are recompiled |
| `keys` | the keymap is rebuilt; palette key chips update |
| `shell`, `env`, `term`, `scrollback_lines`, `launch`, `ssh_domains` | new panes only |
| `window.decorations`, `gpu.*` | restart required; a banner says so |

## When the file is invalid

microterm keeps running on the last valid config and shows a banner in every window:

```text
microterm.toml:12:5 — expected a boolean. Using the last valid config.   [Open config] [Dismiss]
```

**Open config** opens the file in `$VISUAL` / `$EDITOR` in a new tab (or the system opener).
Fixing the file clears the banner. Warnings, such as an unknown key, show the same banner in
amber and the rest of the file still applies.

## Manual reload

`ReloadConfig` (`⌘⇧R` / `Ctrl+Shift+R`, or "Reload config" in the command palette) runs the same pipeline as the
file watcher and reports what changed. `microterm config check` runs the validation without
starting the app.
