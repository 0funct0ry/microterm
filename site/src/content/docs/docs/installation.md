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

## Requirements

**macOS** needs Xcode's Metal toolchain, or just the Command Line Tools: the Makefile falls
back to runtime shader compilation automatically.

**Linux** (glibc 2.35 or newer) needs:

- a Vulkan 1.1 driver. Mesa's `llvmpipe` software renderer works as a fallback;
- `libxkbcommon`;
- the Wayland **or** X11 client libraries (microterm picks the native backend at start-up).

| Distro family | Install |
|---|---|
| Debian, Ubuntu | `sudo apt install libvulkan1 mesa-vulkan-drivers libxkbcommon0` |
| Fedora, RHEL | `sudo dnf install vulkan-loader mesa-vulkan-drivers libxkbcommon` |
| Arch | `sudo pacman -S vulkan-icd-loader vulkan-radeon libxkbcommon` (or your GPU's driver) |
| openSUSE | `sudo zypper install libvulkan1 Mesa-vulkan-drivers libxkbcommon0` |

If no Vulkan loader is found, microterm exits with a message naming the package to install for
your distro.
