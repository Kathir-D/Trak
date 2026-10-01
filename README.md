<div align="center">
  <h1>Trak</h1>
  <p><strong>A fast, good-looking terminal UI for Spotify on macOS.</strong></p>
  <p>Now playing, album art or a live visualizer, synced lyrics, and every <a href="https://github.com/hnarayanan/shpotify">shpotify</a> command. It controls the official Spotify app, so the Free tier works with no setup.</p>
</div>

<p align="center">
  <a href="https://github.com/Kathir-D/Trak/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/Kathir-D/Trak/actions/workflows/ci.yml/badge.svg"></a>
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

| Command | What it does | Output |
| --- | --- | --- |
| `trak status` | the card above | |
| `trak status --json` | machine readable | one JSON object, below |
| `trak status artist` | just the artist | `Jane Remover` |
| `trak status album` | just the album | `Census Designated` |
| `trak status track` | just the title | `Census Designated` |
| `trak play` | resume | *(silent)* |
| `trak play spotify:track:…` | play a URI — works on the Free tier | *(silent)* |
| `trak play <song>` | search and play the best match (needs the optional Client ID) | `Playing Teardrop — Massive Attack (Mezzanine)` |
| `trak play album <name>` | search albums, play the best match | `Playing Mezzanine — Massive Attack (1998)` |
| `trak play artist <name>` | search artists, play the best match | `Playing Massive Attack` |
| `trak play list <name>` | search playlists, play the best match | `Playing Massive Attack on Repeat · 3 tracks` |
| `trak play uri <uri>` | play a URI, spelt the way shpotify did | *(silent)* |
| `trak pause` | toggles play/pause, as shpotify's did | `Pausing Spotify.` / silent when already paused |
| `trak stop` | pause if playing; Spotify has no `stop` command | `Pausing Spotify.` / silent |
| `trak next` | skip | *(silent)* |
| `trak prev` | back | *(silent)* |
| `trak replay` | restart the track | *(silent)* |
| `trak pos 60` | seek to 1:00 | `1:00` |
| `trak vol up` | +10% | `90` — the volume after the step |
| `trak vol down` | −10% | `70` |
| `trak vol show` | print the current volume | `80` |
| `trak toggle shuffle` | toggle shuffle | *(silent)* |
| `trak toggle repeat` | cycle repeat off → all → one | *(silent)* |
| `trak share url` | print and copy the open.spotify.com link | `https://open.spotify.com/track/…` |
| `trak share uri` | print and copy the spotify: URI | `spotify:track:…` |
| `trak quit` | quit the Spotify app | *(silent)* |

`trak --help` lists them all. Exit codes are `0` on success, `1` on a runtime
failure, `2` on a usage or setup problem. Commands that write to Spotify print
nothing on success — the *next* `trak status` is the confirmation. The one
exception is a play by name, which prints the match it chose: you named a song
rather than a URI, and "the best match" is a decision worth seeing. It never
prints a miss as a success — nothing found is `No results when searching for
"…"`, exit 1.

Search is the optional Client ID's too: `trak play <song>` and friends need the
one-time setup (`trak config`, the steps it prints), which no machine has walked
yet — until then those commands print the setup steps and exit 2, exactly as
shpotify did before its user configured an app.

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
If it is denied, Trak says so and tells you where to fix it:

> System Settings › Privacy & Security › Automation › add your terminal, with
> Spotify checked.

That prompt belongs to your **terminal**, not to Trak, so it happens once per
terminal. Trak never starts Spotify as a side effect; it only talks to an app you
already have running.

## Made for a headless setup

Works alongside [Sonar](https://github.com/Kathir-D/Sonar) (menu-bar skip/prev
and auto-pause) and [headless-spotify](https://github.com/Kathir-D/headless-spotify)
(Spotify with no Dock icon). The contract Trak keeps with both is written down in
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

Trak descends from [shpotify](https://github.com/hnarayanan/shpotify) by Harish
Narayanan (MIT). Visualizer maths from
[cavacore](https://github.com/TornaxO7/cavacore-rs), a port of
[cava](https://github.com/karlstav/cava). See
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

## License

MIT. See [`LICENSE`](LICENSE).
