---
title: Installation (coming soon)
description: How to install microterm.
---

microterm has not shipped its first release yet. Installation instructions (Homebrew, the
`curl | sh` installer and manual downloads) will appear here with v1.0.0.

Until then you can run it from source:

```sh
git clone https://github.com/0funct0ry/microterm
cd microterm
make run
```

On Linux you need a Vulkan 1.1 driver, `libxkbcommon`, and either the Wayland or X11 client
libraries. On macOS you need Xcode's Metal toolchain, or just the Command Line Tools: the
Makefile falls back to runtime shader compilation automatically.
