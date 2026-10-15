---
title: Hyperlinks
description: Clickable OSC 8 links, built-in URL detection, custom rules and the scheme allowlist.
---

microterm makes three kinds of text clickable:

1. **Explicit links.** Programs can mark text as a link with the OSC 8 escape sequence, for
   example `ls --hyperlink=auto` or `gcc` diagnostics. The visible text can differ from the
   target, so hovering shows the real target in a tooltip.
2. **Built-in rules.** `http://`, `https://`, `ftp://`, `file://`, `mailto:` and
   `localhost:PORT` (opened as `http://localhost:PORT`).
3. **Your rules.** Add `[[hyperlink_rules]]` to turn any pattern into a link.

```toml
[[hyperlink_rules]]
regex  = '\b([A-Z]{2,}-\d+)\b'
format = "https://linear.app/acme/issue/$1"
```

`format` may use `$0` (the whole match) and `$1`..`$9` (capture groups). An invalid `regex` is
reported as a config error and the previous config stays active. Built-in rules win where
matches overlap.

## Opening a link

Hold the activation modifier and click: **Cmd** on macOS, **Ctrl** on Linux. Hovering with the
modifier held shows a hand pointer; the link gets a dotted accent underline. Links open with the
system opener (`open` / `xdg-open`), never through a shell.

```toml
[hyperlinks]
schemes  = ["http", "https", "mailto", "file"]   # anything else is blocked
modifier = "cmd"                                 # "cmd" | "ctrl" | "none" (plain click)
```

A link whose scheme is not in `schemes` is not opened; a notification names the scheme. For
example, add `"ftp"` to open `ftp://` links. The allowlist applies to every link, including
those produced by your rules. Links containing control characters are refused.

Right-click a link for *Open link* and *Copy link*.

## Performance

Only visible rows are scanned, and only the row under the pointer is matched, using a cache keyed
by the row's contents, so scrolling and idle output stay cheap.
