---
title: Upgrading
description: Upgrade microterm with the same method you used to install it.
---

Upgrade with the method you installed with.

```sh
# Homebrew on macOS
brew upgrade --cask microterm
```

```sh
# Homebrew on Linux
brew upgrade microterm
```

```sh
# Install script (either OS): re-run it, or pin a version
curl -fsSL https://0funct0ry.github.io/microterm/install.sh | sh
curl -fsSL https://0funct0ry.github.io/microterm/install.sh | sh -s -- --version v1.0.1
```

Manual installs: download the new archive from
[GitHub Releases](https://github.com/0funct0ry/microterm/releases) and replace the old one.

## What happens to running sessions

- Open windows keep running the old version until you quit them. Quit and start microterm again to
  use the new one.
- Open SSH sessions keep running on the remote host. On the next connection microterm uploads the
  `microterm-mux` daemon that matches the new version and leaves the old one serving your existing
  sessions until they end.
- Your configuration is never rewritten by an upgrade. New keys have defaults; see the
  [Changelog](/docs/changelog/) for anything that changed meaning.
