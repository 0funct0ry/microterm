<p align="center">
  <img src="packaging/icon/microterm.svg" width="96" alt="microterm logo: a μ in a terminal window">
</p>

# microterm

A minimal GPU-accelerated terminal emulator for macOS and Linux, written in Rust on
[GPUI](https://www.gpui.rs/). Tabs, splits, workspaces, SSH sessions that survive a dropped
connection, quick select, a vi-style copy mode, hot-reloaded TOML configuration and a scriptable
command line (`microterm ctl`).

Website and docs: <https://0funct0ry.github.io/microterm/>

## Install

```sh
brew install --cask 0funct0ry/tap/microterm      # macOS
brew install 0funct0ry/tap/microterm             # Linux
curl -fsSL https://0funct0ry.github.io/microterm/install.sh | sh
```

Or download an archive from [Releases](https://github.com/0funct0ry/microterm/releases) and check it
against `SHA256SUMS`. macOS builds that are only ad-hoc signed need the quarantine flag removed once:

```sh
xattr -dr com.apple.quarantine /Applications/microterm.app
```

Linux needs a Vulkan 1.1 driver (Mesa works, including llvmpipe), `libxkbcommon`, and Wayland or X11.

## Build from source

```sh
make setup      # toolchain, cargo-deny, cargo-about, site dependencies
make run        # debug build
make test
make lint
make bundle     # macOS .app/.dmg or Linux tarball into dist/
make install    # ~/.local (and ~/Applications on macOS)
```

`make help` lists every target. The design lives in `internal-docs/SPEC.md` (not published); the
user documentation is in `site/src/content/docs/docs/`.

## License

microterm is released under the [MIT License](LICENSE). It links code under other permissive
licences and bundles fonts, icons and themes; their licence texts are in
`THIRD-PARTY-LICENSES.html` (regenerate with `make licenses`) and ship with every download. Run
`microterm --version --verbose` to see the licence from the command line.
