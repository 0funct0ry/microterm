---
title: Search
description: Find text in the screen and scrollback with case and regex toggles.
---

Press **Cmd+F** (Linux: **Ctrl+Shift+F**) to open the find bar at the top right of the pane.
Matches highlight as you type, in the visible area and in scrollback. The current match is
brighter and is scrolled into view when it is off screen.

| Control | Does |
|---------|------|
| typing | searches incrementally |
| `Enter` / `Shift+Enter` | next / previous match, wrapping at the ends |
| **Match case** button or `Alt+C` | case-sensitive (default is case-insensitive) |
| **Regular expression** button or `Alt+R` | treat the query as a regex |
| `Esc` | closes the bar and returns to the terminal |

The counter reads `n of m`. At most the first 10,000 matches are kept; if there are more, the
count shows a `+` (`1 of 10000+`). An invalid regex shows `invalid regex`.

A match that wraps across rows is found as one match, and the match list stays current while
output arrives. *Search for selection* in the right-click menu opens the bar with the selected
text.

Regular expressions use Rust's `regex` syntax: no look-around or backreferences.
