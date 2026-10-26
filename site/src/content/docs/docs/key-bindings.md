---
title: Key bindings
description: Default shortcuts, how to rebind them, and how to remove one.
---

Every default shortcut is listed next to its action in the [Actions reference](/docs/reference/actions/).
The command palette shows the shortcut beside each action too.

## Rebinding

Bindings are `[[keys]]` entries in the [config file](/docs/config-file/):

```toml
[[keys]]
key = "cmd+shift+p"
action = "CommandPalette"

[[keys]]
key = "alt+1"
action = { ActivateTab = 1 }

[[keys]]
key = "ctrl+shift+t"
action = "Disable"        # remove a default binding
```

`key` is modifiers joined by `+` and then a key: `cmd+shift+p`, `ctrl+alt+k`, `alt+1`, `f5`,
`ctrl+pageup`. Modifiers are `cmd` (alias `super`), `ctrl`, `alt` (alias `opt`) and `shift`. User
bindings win over defaults on the same keystroke. An unknown action name is skipped and shown in
the error banner, and the rest of the file still applies.

On macOS the defaults use `cmd`; on Linux the same actions use `ctrl+shift`, so muscle memory carries
over without clashing with terminal programs.

## Pane-scoped bindings

A binding can be limited to panes that match a selector, or to one pane at a time; see
[Selectors & targeting](/docs/selectors-and-targeting/) and [Private OSC](/docs/private-osc/).
