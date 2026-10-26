# Release checklist

## One-time setup

- [ ] **Without an Apple Developer membership (current path): add nothing.** `release.yml` skips the
      certificate import and notarization when the `APPLE_*` secrets are missing, and
      `scripts/bundle-macos.sh` ad-hoc signs (`codesign -s -`), which is enough for Apple Silicon to run
      the binary. See "Unsigned macOS releases" below for what users do.
- [ ] *Later, if you join the Apple Developer Program ($99/yr):* add secrets `APPLE_CERTIFICATE`
      (base64 Developer ID Application `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`,
      `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` (app-specific password), delete the `postflight` block in
      `packaging/homebrew/cask.rb.in`, and drop the quarantine notes from the docs. No other change.
- [ ] Repository secret `TAP_TOKEN`: a token with push access to `0funct0ry/homebrew-tap`.
- [ ] `0funct0ry/homebrew-tap` exists with `Casks/` and `Formula/` directories.
- [ ] GitHub Pages: Settings → Pages → Source: *GitHub Actions*.

## Before tagging

- [ ] `make lint && make test && make docs-check && make themes` pass on macOS and Linux CI.
- [ ] `make licenses` is committed (`make licenses-check` clean).
- [ ] `[workspace.package] version` in `Cargo.toml` is the release version; `Cargo.lock` is committed.
- [ ] `CHANGELOG.md`: rename `## Unreleased` to `## vX.Y.Z` and add a fresh `## Unreleased` above it
      (`scripts/release-notes.sh vX.Y.Z` prints the notes); `make docs` regenerates the Changelog page.
- [ ] `make release-dry` on a macOS and a Linux machine; install with `scripts/install.sh` from `dist/`.
- [ ] Manual test pass (below) recorded in the release PR.

## Tag and publish

- [ ] GitHub → Actions → **release** → Run workflow on `main` with the version (`1.0.0`, no `v`). It
      refuses if `Cargo.toml` disagrees, builds four platforms and drafts the release. No tag exists yet.
- [ ] Draft has: 2 × `.dmg`, 2 × macOS `.tar.gz`, 2 × Linux `.tar.gz`, 4 × `microterm-mux-*` with
      `.LICENSES.txt`, `SHA256SUMS`. The `install-test` job is green.
- [ ] Publish the draft (this creates the `vX.Y.Z` tag on the built commit) → `homebrew.yml` commits the cask and formula; `pages.yml` redeploys the site.
- [ ] On a clean macOS machine: `brew install --cask 0funct0ry/tap/microterm` (the cask clears the
      quarantine flag), run `microterm` from a terminal: it opens with no Gatekeeper prompt and the
      Dock shows the μ icon. Also try the `.dmg` download in a browser and confirm the documented
      workaround below opens it.
- [ ] On a clean Linux machine: `curl -fsSL https://0funct0ry.github.io/microterm/install.sh | sh`;
      `microterm --version --verbose` prints `License: MIT`; the launcher entry shows the icon.
- [ ] https://0funct0ry.github.io/microterm/ serves the marketing page and `/docs/`.

## Unsigned macOS releases

Without notarization, an app downloaded in a browser carries the quarantine flag and Gatekeeper says
it "cannot be opened". Which install paths are affected:

| Path | Result | Why |
|---|---|---|
| `brew install --cask` | works | the cask `postflight` removes the quarantine flag |
| `curl … install.sh \| sh` | works | `curl` sets no quarantine flag; the script also clears it |
| `.dmg` / `.tar.gz` from the browser | blocked once | tell users one of the fixes below |

Fixes for a browser download (also in the README and Installation page): `xattr -dr com.apple.quarantine
/Applications/microterm.app`, or right-click the app → Open → Open, or System Settings → Privacy &
Security → *Open Anyway*. Apple Silicon needs *a* signature, and ad-hoc counts. Put the install
script or Homebrew first on the site and in release notes, and the `.dmg` last.

## Manual test matrix

Run every demo in `docs/demos/` (M1 through M13) on each target and note failures.

| Target | Notes |
|---|---|
| macOS arm64 | primary |
| macOS x64 | Rosetta is not enough: use an Intel Mac or macos-13 |
| Ubuntu, GNOME on Wayland | native Wayland backend, Vulkan via Mesa or the vendor driver |
| Fedora, KDE | Wayland session and X11 session of the same install |
| Any distro, X11 session | `XDG_SESSION_TYPE=x11`; icon via `StartupWMClass=microterm` |

Also check on each target: cold start, `cat` of 100 MB, `microterm ssh` with a dropped connection,
`microterm ctl pane list` from inside a pane, light/dark switching, hot reload of a broken config,
and that the window/Dock/taskbar shows the μ icon for an installed build.
