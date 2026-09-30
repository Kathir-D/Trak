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

> **Status: pre-alpha.** The CLI works today and is shown below. The TUI, settings screen and
> visualizer are built in the order set out in [`TODO.md`](TODO.md); this README is rewritten around
> the shipped product at the end (TODO 11.1). Nothing here is aspirational — every command listed
> runs against a real Spotify today.

## What works now

```sh
trak status                 # a card of what is playing
```

```
▶
Census Designated
Jane Remover · Census Designated
1:35 / 6:00
volume 100
```

On a terminal that also gets a progress bar and a volume meter, and the artist's
name in bold. Piped or redirected, the same command prints clean text with no
escape codes — so `trak status | cat` is safe to put in a script.

### Every command

```
trak status                 # the card above
trak status --json          # machine readable
trak status artist          # just the artist
trak status album           # just the album
trak status track           # just the title
trak play                   # resume
trak play spotify:track:…   # play a URI — works on the Free tier
trak pause                  # toggles play/pause, as shpotify's did
trak stop                   # pause if playing
trak next                   # skip
trak prev                   # back
trak replay                 # restart the track
trak pos 60                 # seek to 1:00
trak vol up                 # +10%
trak vol down               # −10%
trak vol show               # print the current volume
trak toggle shuffle         # toggle shuffle
trak toggle repeat          # cycle repeat off → all → one
trak share url              # print and copy the open.spotify.com link
trak share uri              # print and copy the spotify: URI
```

`trak --help` lists them all. Exit codes are `0` on success, `1` on a runtime
failure, `2` on a usage or setup problem.

### JSON

```sh
trak status --json
```

```json
{"state":"▶","title":"Census Designated","artist":"Jane Remover","album":"Census Designated","album_artist":"Jane Remover","uri":"spotify:track:6HacgXCExkzS552ILfJTXu","duration_ms":360511,"position_secs":95.196,"volume":100,"shuffling":false,"repeating":true,"popularity":48,"track_number":8,"disc_number":1,"artwork_url":"https://i.scdn.co/image/ab67616d0000b2738a821784ac3e69e691d4945f","is_ad":false}
```

## Coming next

- The TUI: `trak` on its own, with a Now Playing pane, session history, track
  details and synced lyrics, adapting to the terminal size.
- Album art with true colour in terminals that support it.
- A live visualizer, with a simulated fallback so it works with no permission
  prompt.
- `trak config` for settings.
- Optional Spotify Client ID, which unlocks search, playlists, queue and library.

## Permissions

The first AppleScript call asks your terminal for permission to control Spotify.
If it is denied, trak says so and tells you where to fix it:

> System Settings › Privacy & Security › Automation › add your terminal, with
> Spotify checked.

That prompt belongs to your **terminal**, not to trak, so it happens once per
terminal. trak never starts Spotify as a side effect; it only talks to an app you
already have running.

## Made for a headless setup

Works alongside [Sonar](https://github.com/Kathir-D/Sonar) (menu-bar skip/prev
and auto-pause) and [headless-spotify](https://github.com/Kathir-D/headless-spotify)
(Spotify with no Dock icon). The contract trak keeps with both is written down in
[`docs/COMPAT.md`](docs/COMPAT.md) and the details that are easy to get wrong are
measured in [`docs/APPLESCRIPT.md`](docs/APPLESCRIPT.md).

## Building from source

```sh
cargo build --release
./target/release/trak status
```

Needs macOS 14.2 or newer. `cargo test` runs with no Spotify, no network and no
audio device.

## For contributors and agents

Start with [`AGENTS.md`](AGENTS.md), then [`docs/SPEC.md`](docs/SPEC.md) and
[`TODO.md`](TODO.md). To re-check the measurements behind the design,
`./spikes/verify.sh` runs every spike and compares the result against the docs.

## Credits

trak descends from [shpotify](https://github.com/hnarayanan/shpotify) by Harish
Narayanan (MIT). Visualizer maths from
[cavacore](https://github.com/TornaxO7/cavacore-rs), a port of
[cava](https://github.com/karlstav/cava). See
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

## License

MIT. See [`LICENSE`](LICENSE).
