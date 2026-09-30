# trak — product spec

Every decision here was made by the owner (Kathir-D) in the design interview. **Do not re-ask
these questions.** If something is missing or contradicts reality, add it to the "Open questions"
list at the bottom and ask only about that.

Legend: **A** = Version A (a Spotify Client ID is configured). **B** = Version B (no Client ID).
"Verify" marks a claim that is believed but not yet confirmed on this machine; the matching task in
`TODO.md` says how to verify it.

## 1. What trak is

An interactive, good-looking terminal UI for the **official Spotify desktop app on macOS**, plus
every one-shot command from [shpotify](https://github.com/hnarayanan/shpotify). It controls Spotify
through AppleScript (so it works on the **Free tier**, with no setup) and optionally through the
Spotify Web API (unlocks search, playlists, queue, library).

It is one part of the owner's music setup:

1. **headless-spotify** hides Spotify from the Dock and Cmd-Tab.
2. **Sonar** is a menu-bar item: skip / previous, plus auto-pause and resume when other audio plays.
3. **trak** is the terminal display and controller.

See `docs/COMPAT.md` for how the three coexist. trak's look and feel is intentionally different from
[spotify-tui](https://github.com/Rigellute/spotify-tui) even though the feature set is similar.

## 2. Decisions

| Topic | Decision |
| --- | --- |
| Name / binary | `trak` (free on Homebrew and no notable GitHub repo of that name at the time of checking) |
| Language / UI | Rust + [ratatui](https://github.com/ratatui/ratatui) |
| License | MIT. `LICENSE` carries both the trak and the shpotify copyright lines |
| Platform | **macOS 14.2+ only** (Core Audio process taps need 14.2). arm64 + x86_64 |
| Repo | Public `Kathir-D/trak`, from day one |
| Distribution | Homebrew **formula** in `Kathir-D/homebrew-tap`: `brew install kathir-d/tap/trak`. Prebuilt binary from a GitHub release tarball. **No paid code signing / notarization** (ad-hoc `codesign -s -` is free and fine) |
| CLI compat | Every shpotify subcommand keeps working. Bare `trak` opens the TUI |
| CLI output | Redesigned but not extravagant. Plain by default when piped. `--plain` and `--json` flags |
| Extras | **Out of scope:** `status --format`, `trak mini`, shell completions, man page |
| Version B history | Session-only (in memory, cleared when the TUI exits) |
| Keys | Arrows **and** vim keys. Not user-rebindable in v1 |
| Mouse | A setting, on by default |
| Volume keys | Spotify's volume only (10 % step, a setting). `m` mutes |
| No Spotify running | Idle card "Spotify isn't running — press enter to launch". Never auto-launch |
| Small terminals | Adapt: side-by-side → stacked → compact strip. Art shrinks/hides first |
| Accent colour | Setting: `art` (default, dominant cover colour), `green` (#1DB954), `terminal` (ANSI palette) |
| Borders | Setting: `rounded` (default), `sharp`, `double`, `none` |
| Header | Status dot (green playing / yellow paused / grey not running) and a clock |
| Notifications | Song-change notification is a setting, **off** by default (`osascript display notification`) |
| Lyrics | Synced lyrics from LRCLIB, both versions. A side-pane tab and a full-screen mode (`L`) |
| Client ID setup | Guided flow inside `trak config`, PKCE (no client secret) |
| Token storage | macOS Keychain first; see risk R3 in `TODO.md` for the fallback |

## 3. Layout

Full dashboard: rounded panes, header, two columns, footer. Left = Now Playing, right = tabs.

```
╭─ trak ─────────────────────────────────────────────────────────────────── ● playing   19:42 ─╮
│╭─ Now Playing ─────────────────────────────────╮╭─ [1]Search [2]Playlists [3]Queue [4]Liked ─╮│
││  <album art OR visualizer, ~20x10 cells>      ││ Up next                                    ││
││                        Nights                 ││  ▶ Nights               Frank Ocean   5:07 ││
││                        Frank Ocean            ││    Solo                 Frank Ocean   4:17 ││
││                        Blonde · 2016          ││    Self Control         Frank Ocean   4:09 ││
││                        ⇄ Shuffle  ↻ Repeat    ││    ...                                     ││
││  1:42 ━━━━━━━━━━●──────────────────── 5:07    ││                                            ││
││       ⏮     ⏸     ⏭        🔊 ▰▰▰▰▰▰▰▱▱▱ 70%  ││                                            ││
│╰───────────────────────────────────────────────╯╰────────────────────────────────────────────╯│
│ space play/pause  n/p next/prev  ←/→ seek  +/- vol  s shuffle  r repeat  / search  ? help     │
╰────────────────────────────────────────────────────────────────────────────────────────────────╯
```

Rules:

- The layout is computed from the terminal size on every resize. Borders must never wrap or tear.
  Below the breakpoints (tuned in TODO 3.x) the right pane moves under Now Playing, then collapses
  to the compact strip (title, artist, progress, controls).
- **Art vs visualizer** is a toggle (`a`). When the visualizer is shown there is **no art anywhere**,
  but the accent colour still comes from the cover (fetch it invisibly).
- **Version B** right-pane tabs: `[1] History`, `[2] Info`, `[3] Lyrics`.
  - History: tracks played this session. `↑/↓`/`j/k` to move, `enter` plays it again (by URI).
  - Info: everything AppleScript exposes (see section 5).
- **Version A** right-pane tabs: `[1] Search`, `[2] Playlists`, `[3] Queue`, `[4] Liked`,
  `[5] Library`, `[6] Lyrics`, plus `History` and `Info` reachable via `Tab`. Exact tab order is a
  tab-order task (TODO 7.13), but all of these must exist.
- Version B shows a one-line hint that a Client ID unlocks more (`trak config`).
- Spotify not running: centered idle card, `enter` launches it in the background.

## 4. Keys

| Key | Action | Version |
| --- | --- | --- |
| `space` | Play / pause | both |
| `n` / `p` | Next / previous track | both |
| `←` `→` / `h` `l` | Seek −/+ 5 s (a setting) | both |
| `+` `-` | Spotify volume ±10 (a setting) | both |
| `m` | Mute / unmute (disabled while Sonar is fading, see COMPAT) | both |
| `s` / `r` | Toggle shuffle / cycle repeat | both |
| `a` | Toggle art ↔ visualizer | both |
| `v` | Cycle visualizer style | both |
| `↑` `↓` / `j` `k` | Move in a list | both |
| `enter` | Play the selected item | both |
| `Tab` / `Shift-Tab` | Cycle focus between panes / tabs | both |
| `1`–`6` | Jump to tab | both |
| `c` | Copy the current track's share URL | both |
| `L` | Full-screen lyrics | both |
| `,` | Open settings (`trak config`) | both |
| `?` | Help overlay | both |
| `q` / `ctrl-c` | Quit | both |
| `/` | Search (live, grouped) | A |
| `f` | Like / unlike current track ("favourite"; `l` is seek-right) | A |
| `A` (shift-a) | Add selected to queue | A |
| `o` | Open artist or album page of the selection | A |
| `esc` | Back / close overlay | both |

Keep this table and the `?` help overlay in sync.

## 5. What AppleScript can give us (Version B feature ceiling)

The Spotify scripting dictionary exposes, for `current track`: `name`, `artist`, `album`,
`album artist`, `duration` (ms), `disc number`, `track number`, `popularity`, `played count`,
`artwork url`, `spotify url`, `id`. App-level: `player state`, `player position`, `sound volume`,
`shuffling`, `repeating`. Actions: `play`, `pause`, `playpause`, `next track`, `previous track`,
`play track "<uri>"` (tracks **and** album / playlist / artist context URIs), `set player position`,
`set sound volume`, `set shuffling`, `set repeating`.

It **cannot** list a queue, playlists, the library, or search. That is exactly the A/B split.
(Verify each field on this machine in TODO 1.1 and write the real output into `docs/APPLESCRIPT.md`.)

## 6. Version A (Web API)

- Auth: Authorization Code + PKCE, loopback redirect. Verify Spotify's current rules for redirect
  URIs (loopback `http://127.0.0.1:<port>` vs `localhost`) and **developer-mode restrictions**
  (Premium requirement, user cap, endpoint deprecations) before building. TODO 1.7 and 7.1.
- Features: live grouped search (Tracks / Albums / Artists / Playlists), playlists, queue view and
  add-to-queue, liked songs, library (saved albums, followed artists, recently played), artist page
  (top tracks, albums), album page (tracklist), playlist add / remove / create.
- **Playback of results still goes through AppleScript** (`play track "<uri>"`), so it works on Free.
  Queue mutation and device transfer may need Premium on Spotify's side; when the API returns 403,
  show a clear one-line message, never a stack trace.
- Client ID lives in config; token in Keychain (or fallback, R3). Never log tokens.

## 7. Visualizer

- Styles: `spectrum`, `mirrored`, `waveform` (braille), `circular`. `v` cycles.
- Source `auto`: real audio via a Core Audio process tap on Spotify's process (macOS 14.2+), falling
  back to a simulated visualizer if permission is denied / the tap fails. Source `simulated` forces it.
- Math: [cavacore](https://github.com/TornaxO7/cavacore-rs). Tap: [cidre](https://github.com/yury/cidre).
  Waveform drawing: study [scope-tui](https://github.com/alemidev/scope-tui). Renderers are small
  pure functions `(samples/bars, Rect) -> Buffer` so they are unit-testable.
- The tap targets **Spotify's process only**, never system audio, so it does not disturb Sonar's tap.
- Permission is attributed to the **terminal app** ("System Audio Recording"). This is the top risk
  (R1 in `TODO.md`). Prototype it first.

## 8. Config

`~/.config/trak/config.toml` (respect `XDG_CONFIG_HOME` if set). Edited by `trak config` (an
interactive checklist) and by `,` inside the TUI; changes apply live. Unknown keys are ignored,
missing keys take defaults, a corrupt file is renamed to `config.toml.bak` and defaults are used.

```toml
[display]
art = true                 # show album art (ignored while the visualizer is showing)
mode = "art"               # "art" | "visualizer"
progress = true
volume = true
popularity = true
key_hints = true
clock = true
side_pane = true
default_tab = "history"    # B: history|info|lyrics ; A: search|playlists|queue|liked|library|lyrics
border = "rounded"         # rounded | sharp | double | none
accent = "art"             # art | green | terminal
art_protocol = "auto"      # auto | kitty | iterm2 | sixel | halfblocks

[visualizer]
style = "spectrum"         # spectrum | mirrored | waveform | circular
source = "auto"            # auto | simulated

[input]
mouse = true
volume_step = 10
seek_step = 5

[notifications]
song_change = false

[lyrics]
enabled = true

[spotify]
client_id = ""             # empty = Version B
```

Toggles in the settings screen: show art, visualizer on/off + style, progress bar, volume meter,
popularity, key hints, clock, side pane, default tab, art protocol, accent, border style, mouse,
step sizes, notifications, lyrics, and the guided Client ID setup.

## 9. CLI (shpotify parity)

```
trak                       open the TUI
trak config                open settings
trak play                  resume
trak play <song>           find a song and play it        (needs a Client ID, like shpotify)
trak play album|artist|list <name>
trak play uri <uri>
trak next | prev | replay
trak pos <seconds>
trak pause | stop | quit
trak vol up | down | <0-100> | show
trak status [artist|album|track]
trak share [url|uri]       print (and copy for url/uri)
trak toggle shuffle|repeat
        --plain   no colour / no decoration
        --json    machine readable (status)
```

`play <song>` etc. need search, so they need a Client ID (shpotify needed one too). Without it,
print a friendly explanation and how to get one; exit code 2. Exit codes: 0 ok, 1 runtime failure,
2 usage / missing setup. Never launch Spotify from a one-shot command (see COMPAT).

## 10. Open questions

_None._ Add items here instead of guessing.
