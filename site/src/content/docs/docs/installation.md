---
title: Installation
description: Install microterm on macOS or Linux with Homebrew, the install script, or a manual download.
---

Every method installs the same build and puts a `microterm` command on your `PATH`.

## Homebrew

```sh
# macOS 12 or later, Apple Silicon or Intel
brew install --cask 0funct0ry/tap/microterm
```

```sh
# Linux (x86_64 or aarch64); needs a Vulkan driver, see Requirements
brew install 0funct0ry/tap/microterm
```

## Install script

The script detects your OS and CPU, downloads the matching archive and `SHA256SUMS`, verifies the
checksum, and installs without needing root.

```sh
curl -fsSL https://0funct0ry.github.io/microterm/install.sh | sh
```

| Flag | Effect |
|---|---|
| `--version vX.Y.Z` | Install a specific release instead of the latest |
| `--prefix DIR` | Install under `DIR` instead of `~/.local` |
| `--no-modify-path` | Do not add the bin directory to your shell profile |

On Linux it installs `~/.local/bin/microterm`, a desktop entry, the icons and the licence files. On
macOS it installs `~/Applications/microterm.app` and links `~/.local/bin/microterm` to the launcher
inside the app (never to `microterm-app`).

## Manual download

Every release on [GitHub Releases](https://github.com/0funct0ry/microterm/releases) has a `.dmg` and
a `.tar.gz` for macOS, a `.tar.gz` for Linux, the static `microterm-mux` daemons and `SHA256SUMS`.

```sh
sha256sum -c SHA256SUMS --ignore-missing      # shasum -a 256 -c on macOS
tar -xzf microterm-1.0.0-x86_64-unknown-linux-gnu.tar.gz
./microterm-1.0.0-x86_64-unknown-linux-gnu/bin/microterm --version
```

### macOS: unsigned builds

microterm is ad-hoc signed but not yet notarized by Apple. Homebrew and the install script handle
this for you. A `.dmg` or `.tar.gz` downloaded in a browser is blocked by Gatekeeper the first time;
do one of these once:

```sh
xattr -dr com.apple.quarantine /Applications/microterm.app
```

or right-click the app and choose **Open**, or open System Settings → Privacy & Security and choose
**Open Anyway**.

## Check the install

```sh
microterm --version
microterm --version --verbose     # also prints the licence
```

## `microterm` on your PATH (macOS)

The `microterm` command is a small launcher that lives inside the app bundle. It starts the app
through LaunchServices (so the Dock shows the microterm icon) or hands a new window to the running
instance, and forwards every other command (`ctl`, `sh`, `config`, …) to the app binary. `--help`,
`--version` and exit codes are identical either way.

## Build from source

```sh
git clone https://github.com/0funct0ry/microterm
cd microterm
make run            # debug build, run it
make install        # release build, install under ~/.local (and ~/Applications on macOS)
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

## Licences

microterm is MIT licensed. The app, the `.dmg`, every archive and every `microterm-mux` download
ship `LICENSE` and `THIRD-PARTY-LICENSES.html`. See [Licences](/docs/licenses/).
