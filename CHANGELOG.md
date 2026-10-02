# Changelog

All notable changes to Trak are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Trak uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). How a release is cut
is in [`docs/RELEASING.md`](docs/RELEASING.md).

## [Unreleased]

## [0.1.1] - 2026-10-02

### Fixed

- The Homebrew formula no longer carries a `version` line, which
  `brew audit --strict` rejects as redundant with the URL. Nothing about the
  binary changed from 0.1.0.

## [0.1.0] - 2026-10-02

The first release. Pre-alpha: everything listed here is built and tested against
fakes and recorded replies, and the parts marked **unverified** have not yet met
a real account or a real permission prompt.

### Added

- **Every [shpotify](https://github.com/hnarayanan/shpotify) command**: `status
  [artist|album|track]`, `play`, `pause`, `stop`, `quit`, `next`, `prev`,
  `replay`, `pos`, `vol up|down|<n>|show`, `toggle shuffle|repeat`, `share
  url|uri`, `play uri <uri>`, plus `--plain` and `--json`. Plain output when
  piped, `NO_COLOR` respected, exit codes 0 / 1 / 2.
- **The TUI** (`trak` with no arguments): Now Playing with a clickable progress
  bar, volume meter, shuffle and repeat; a layout that adapts from wide to
  stacked to a compact strip; an idle card when Spotify is not running that
  launches it in the background only when you press enter; mouse support
  (click the bar to seek, drag the volume, click a tab or a button); `←`/`→` and
  `tab`/`shift-tab` move between tabs, and a strip too narrow for all of them
  keeps the selected one centred between its neighbours.
- **Album art** in kitty / Ghostty / cmux (Kitty graphics) and half-blocks
  elsewhere, cached on disk, with the accent colour taken from the cover.
- **Synced lyrics** from [LRCLIB](https://lrclib.net) in a tab and a full-screen
  page (`L`) drawn over the darkened cover, cached on disk.
- **Visualizer** with four styles (`v`), drawn from Spotify's own audio through a
  Core Audio process tap on Spotify alone (never the rest of the system). It runs
  only while the visualizer is on screen, reattaches after Spotify restarts, and
  falls back to a simulated spectrum with a one-line explanation if macOS refuses
  the tap. **The refusal path is tested only with fakes**: the Macs measured so
  far grant the tap with no prompt.
- **History and Info tabs**: tracks played this session (enter plays one again)
  and everything AppleScript exposes about the current track.
- **Settings** (`,` or `?` in the TUI, or `trak config`): a checklist that
  applies live and writes `~/.config/trak/config.toml`, with every key listed
  under it.
- **Terminals of every kind**: `NO_COLOR`, 256- and 16-colour terminals, light
  backgrounds (asked with OSC 11; pale cover colours are darkened to stay
  readable), right-to-left titles that keep the layout in place, and sizes from
  a one-line strip up. Idle CPU is under 1% and a paused screen writes nothing.
- **Version A, with a Spotify Client ID** (guided setup: `trak config`, then
  `s`): live grouped search, playlists (open, add, create, remove), queue and
  add-to-queue, liked songs and `f` to like, library, artist and album pages,
  and `trak play <song|album|artist|list>`. **Unverified against a live
  account**: it is tested against a fake library and recorded replies only, and
  no real login has been made yet.
- **Works beside [Sonar](https://github.com/Kathir-D/Sonar) and
  [headless-spotify](https://github.com/Kathir-D/headless-spotify)**: shows
  Sonar's ducking and keeps the volume keys out of its way, shows a `headless`
  badge, and never writes to Spotify except in answer to a key you pressed.
  The live compatibility matrix in `docs/COMPAT.md` is still being filled in.
- **Install**: a Homebrew formula (`brew install kathir-d/tap/trak`) and a curl
  installer (`install.sh`) that verifies the release's sha256. The binary is
  universal (arm64 + x86_64) and **ad-hoc signed, not notarized**; neither
  install path sets a quarantine flag, so there is no Gatekeeper prompt.

[Unreleased]: https://github.com/Kathir-D/Trak/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/Kathir-D/Trak/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Kathir-D/Trak/releases/tag/v0.1.0
