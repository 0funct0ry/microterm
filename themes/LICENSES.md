# Bundled theme licences

Every bundled theme records its `author` and `license` in its own file; `make themes` fails if
either is missing or the licence is not permissive (MIT, ISC, Apache-2.0, BSD-2/3-Clause, CC0-1.0).
The colour values are palettes published by the upstream projects below. Only the colour
values are used; no upstream source files are copied.

| Upstream | Licence | Themes |
|---|---|---|
| Ayu theme (Ike Ku) | MIT | Ayu Dark, Ayu Light, Ayu Mirage |
| EdenEast (nightfox.nvim) | MIT | Carbonfox, Dawnfox, Dayfox, Duskfox, Nightfox, Nordfox, Terafox |
| Catppuccin (Catppuccin Org) | MIT | Catppuccin Frappé, Catppuccin Latte, Catppuccin Macchiato, Catppuccin Mocha |
| Zeno Rocha (Dracula Theme) | MIT | Dracula |
| sainnhe (Everforest) | MIT | Everforest Dark, Everforest Light |
| Steph Ango (Flexoki) | MIT | Flexoki Dark, Flexoki Light |
| GitHub Primer (github-vscode-theme) | MIT | GitHub Dark Dimmed, GitHub Dark, GitHub Light |
| Pavel Pertsev (gruvbox) | MIT | Gruvbox Dark Hard, Gruvbox Dark Soft, Gruvbox Dark, Gruvbox Light Hard, Gruvbox Light Soft, Gruvbox Light |
| cocopon (iceberg.vim) | MIT | Iceberg Dark, Iceberg Light |
| rebelot (kanagawa.nvim) | MIT | Kanagawa Dragon, Kanagawa Lotus, Kanagawa Wave |
| Sarah Drasner (Night Owl) | MIT | Light Owl, Night Owl |
| Kyle Rober Smith (melange-nvim) | MIT | Melange Dark, Melange Light |
| Arctic Ice Studio (Nord) | MIT | Nord |
| Atom One Dark/Light syntax themes (GitHub, Inc.) | MIT | One Dark, One Light |
| nyoom-engineering (Oxocarbon) | MIT | Oxocarbon |
| drcmda (poimandres-theme) | MIT | Poimandres |
| Rosé Pine (Rosé Pine contributors) | MIT | Rosé Pine Dawn, Rosé Pine Moon, Rosé Pine |
| Ethan Schoonover (Solarized) | MIT | Solarized Dark, Solarized Light |
| enkia (Tokyo Night) | MIT | Tokyo Night Day, Tokyo Night Moon, Tokyo Night Storm, Tokyo Night |
| Chris Kempson (Tomorrow Theme) | MIT | Tomorrow Night, Tomorrow |

## Dropped themes

- **Zenburn** — the original colour scheme is distributed under the GPL, which is not
  permissive, so it is not bundled (SPEC §8.4: failing themes are dropped, not replaced). The
  bundled set is therefore 53 themes.

## Adjusted themes

- **Solarized Dark / Light** use the "emphasised content" foregrounds (`base1` / `base01`) so
  the foreground/background contrast reaches the 4.5:1 minimum enforced by `make themes`.

## Verification note

Licences were recorded from each upstream repository's published licence at the time of
writing. Re-verify them before every release.
