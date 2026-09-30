<div align="center">
  <h1>trak</h1>
  <p><strong>A fast, good-looking terminal UI for Spotify on macOS.</strong></p>
  <p>Now playing, album art or a live visualizer, synced lyrics, and every <a href="https://github.com/hnarayanan/shpotify">shpotify</a> command. It controls the official Spotify app, so the Free tier works with no setup.</p>
</div>

<p align="center">
  <a href="https://github.com/Kathir-D/trak/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/Kathir-D/trak/actions/workflows/ci.yml/badge.svg"></a>
  <img alt="macOS 14.2+" src="https://img.shields.io/badge/macOS-14.2%2B-000000?logo=apple&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-DEA584?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="status: pre-alpha" src="https://img.shields.io/badge/status-pre--alpha-orange">
</p>

> **Status: pre-alpha.** Nothing is usable yet. The design is finished and the build is planned in
> [`TODO.md`](TODO.md). This README describes the goal; it will be rewritten around the shipped
> product (see TODO 11.1).

## What it will be

- **Two modes.** With no setup, trak drives Spotify through AppleScript: now playing, controls,
  play history you can jump back into, track details, synced lyrics, and the visualizer. Add a free
  Spotify Client ID and it also gets search, playlists, queue, liked songs, library, and artist and
  album pages.
- **A look of its own.** Rounded panes that adapt to your terminal size, an accent colour taken
  from the album cover, and a real-audio visualizer (spectrum, mirrored, waveform, circular) you can
  swap in for the art.
- **All of shpotify.** `trak play`, `next`, `prev`, `vol`, `status`, `share`, `toggle` and the rest
  keep working. Run `trak` on its own to open the TUI, and `trak config` to change what it shows.
- **Made for a headless setup.** Works alongside
  [Sonar](https://github.com/Kathir-D/Sonar) (menu-bar skip/prev and auto-pause) and
  [headless-spotify](https://github.com/Kathir-D/headless-spotify) (Spotify with no Dock icon).
  See [`docs/COMPAT.md`](docs/COMPAT.md).
- **Homebrew, no signing.** `brew install kathir-d/tap/trak` (once released).

## For contributors and agents

Start with [`AGENTS.md`](AGENTS.md), then [`docs/SPEC.md`](docs/SPEC.md) and [`TODO.md`](TODO.md).

## Credits

trak descends from [shpotify](https://github.com/hnarayanan/shpotify) by Harish Narayanan (MIT). See
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

## License

MIT. See [`LICENSE`](LICENSE).
