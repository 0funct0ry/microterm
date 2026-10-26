---
title: Licences
description: microterm's licence and where the third-party notices are.
---

microterm is released under the [MIT licence](https://github.com/0funct0ry/microterm/blob/main/LICENSE).

It links code under other permissive licences (Apache-2.0, MIT, BSD, ISC, MPL-2.0, Zlib and others)
and bundles fonts (SIL OFL 1.1, MIT), Lucide icons (ISC) and colour themes. Their licence texts
travel with every download:

| Where | Files |
|---|---|
| macOS `.app` | `Contents/Resources/LICENSE`, `Contents/Resources/THIRD-PARTY-LICENSES.html` |
| `.dmg`, macOS and Linux `.tar.gz` | `LICENSE` and `THIRD-PARTY-LICENSES.html` at the archive root; Linux also `share/doc/microterm/` |
| `microterm-mux-<version>-<target>` | `microterm-mux-<version>-<target>.LICENSES.txt` beside the binary |
| Homebrew, install script | installed with the app (`share/doc/microterm/` on Linux, inside the app on macOS) |
| This site | [Third-party notices](/microterm/THIRD-PARTY-LICENSES.html) |

`microterm --version --verbose` prints `License: MIT`.
