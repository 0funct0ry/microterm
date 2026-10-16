---
title: Quick select
description: Copy or paste any hash, URL, path, IP address or number on screen with two keystrokes.
---

Quick select labels everything on screen that looks like something you would want to copy.
Press **Cmd+Shift+Space** (Linux: **Ctrl+Shift+Space**) and a chip appears at the start of each
match. Type a chip's letters to act on it.

| Typing | Result |
|--------|--------|
| a lowercase label, such as `a` or `sd` | copies the match and leaves quick select |
| the same label in **uppercase** | copies it **and pastes** it into the pane |
| `Esc` | cancels |

A toast confirms the copy, for example `Copied 3f2a9c1`.

Only the rows you are looking at are scanned, including scrollback when you have scrolled up.
Matches that wrap onto the next row are found as one match. Where two patterns overlap, the
longest match wins, so a URL is never split into a path and a host.

## Built-in patterns

| Name | Matches |
|------|---------|
| `git_sha` | 7 to 40 hex characters with at least one digit and one letter |
| `url` | `http://`, `https://`, `ftp://`, `file://`, `mailto:` |
| `path` | `~/src/api/main.rs`, `./build/out.log`, `../x`, `/etc/hosts` |
| `uuid` | `5f0c1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b` |
| `ipv4`, `ipv6` | `10.0.12.4:8080`, `fe80::1`, `[2001:db8::1]:443` |
| `email` | `dev@example.com` |
| `hex_color` | `#74ade8`, `#fff`, `#74ade8cc` |
| `number` | four or more digits, such as `48213` |

## Labels

Labels use the letters of `quick_select.alphabet`, and no label is the start of another, so
typing never needs a confirmation key. Matches nearest the cursor get the shortest labels. When
there are more matches than letters, labels grow to two characters; if there are more matches
than the alphabet can label, the ones farthest from the cursor are left out.

## Configuration

```toml
[quick_select]
enabled         = true                           # false turns quick select off
alphabet        = "asdfqwerzxcvjklmiuopghtybn"   # label letters, no repeats
patterns        = ['PAY-\d+', 'i-[0-9a-f]{17}']  # extra regexes; the whole match is copied
disable_builtin = ["number", "ipv6"]             # names from the table above
```

With `enabled = false` the shortcut shows a toast saying quick select is turned off, and the
"Activate quick select" entry disappears from the command palette. Changes apply the next time
you open quick select or the palette.

An invalid pattern shows a warning and quick select falls back to the built-in patterns.
