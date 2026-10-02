# Trak — architecture

How the shipped code is laid out and why. If reality diverges, update this file in the same
commit.

## Principles

1. **One crate, deep modules.** A small public surface per module, hidden complexity behind it.
   Split into a workspace only if compile times or a real reuse need appear.
2. **Every side effect sits behind a trait**, so the whole app runs and is tested without Spotify,
   a network, a terminal, or audio hardware:
   - `Player` — read state / send commands to Spotify (real: AppleScript; fake: in-memory).
   - `Library` — Web API calls (real: a hand-written `ureq` client; fake: canned data). Version A
     only. rspotify was rejected in TODO 7.5: it is async, and its id-list helpers call endpoints
     dev mode removed.
   - `AudioSource` — the visualizer's spectrum (real: the Core Audio tap; simulated otherwise).
   - `Store` (the token file), and the login's `TokenEndpoint` and `Browser`.
   - Lyrics and art are fetched by plain blocking functions on a worker; everything they feed is a
     pure function of bytes, so the hard part is tested without a socket. Time is passed in as an
     argument (`Instant`/`SystemTime`) rather than through a `Clock` trait.
3. **Pure state + pure render.** `App` holds all state; `update(App, Event)` is deterministic and
   returns the new `App` plus the player commands and Web jobs for the loop to run;
   `render::draw(Frame, &App, &Theme)` only draws. Snapshot-test rendering with ratatui's
   `TestBackend`.
4. **Never block the UI thread.** Anything slow (osascript, HTTP, image decode, FFT) runs on its
   own thread/task and reports back as an `Event` over a channel.
5. **Fail soft.** Optional features (visualizer real audio, Sonar state, headless badge, lyrics,
   Web API) degrade silently to the simpler behaviour and say why in a status line. Never panic on
   bad external data; return `Result`, log, and continue.

## Module map (`src/`)

```
main.rs            argv → cli or tui; the clap definitions (hidden --fake / --fake-library)
lib.rs             the library half, so examples and integration tests use the same code
cli.rs             one-shot commands, --plain/--json output, NO_COLOR, exit codes
tui/
  app.rs           App state, Event, update(); keys, mouse, the session history and the tab list
  render.rs        layout + breakpoints (wide / stacked / compact / idle), Now Playing, History,
                   Info, Lyrics, full-screen lyrics, the tab strip, toasts, the Keys panel rows
  web_tabs.rs      the five Web API tabs and the artist / album / playlist pages
  settings.rs      the settings screen (`,`, `?`, `trak config`) with the Keys panel beside it
  setup.rs         the guided Client ID setup and login
  theme.rs         accent (art/green/terminal), border styles
  colour.rs        NO_COLOR, 256/16-colour downgrade, light-background darkening (OSC 11 query)
  bidi.rs          fences right-to-left runs so a terminal's bidi pass cannot move the layout
  loop_.rs         the only part that touches the terminal or a player: event loop, worker, polls,
                   key/mouse → char mapping, draw-if-changed, the audio pipeline's owner
player/
  mod.rs           trait Player, PlayerState, TrackInfo, RepeatMode
  applescript.rs   osascript runner ($TRAK_OSASCRIPT), the fast and slow read scripts, writes
  parse.rs         batched AppleScript output → PlayerState (pure)
  actions.rs       user-initiated writes with read-back (seek, volume with a ±1 tolerance), pbcopy
  notify.rs        PlaybackStateChanged listener (objc2-foundation, NSDistributedNotificationCenter)
  fake.rs          in-memory Player for tests
art.rs             fetch + cache artwork URL off the UI thread
accent.rs          one usable colour out of a cover
visualizer.rs      AudioSource trait + SimulatedSource + the four renderers (spectrum / mirrored /
                   waveform / circular) as pure fns
audio.rs           the real source (TODO 8.3/8.5): a Core Audio process tap on Spotify's pids only,
                   its own radix-2 FFT + log band map, and `AudioPipeline`, the lifecycle (attach while
                   the visualizer is visible and Spotify runs, release otherwise, reattach after a
                   relaunch, one fallback toast). The **loop owns it**, not `App`: a thread and a Core
                   Audio device do not belong in a state that is cloned and compared, so the loop
                   sends `Event::LiveSpectrum` / `Event::TapNotice` and the app only sees bars
lyrics.rs          LRCLIB client + LRC parser + "current line for position" + on-disk cache
config.rs          load/save/defaults ~/.config/trak/config.toml (hand-written TOML subset)
web/  (A)          auth.rs (PKCE, loopback listener), api.rs (`Library`, the `ureq` client, a fake),
                   token.rs (the `0600` token file; not the Keychain, see docs/KEYCHAIN.md)
sonar.rs           Sonar's state.json, re-read every 2 s
headless.rs        `headless-spotify status --json` / `launch`
```

The song-change notification is a `PlayerCommand` (`display notification` through osascript) run
on the worker, not a module of its own.

## Data flow

```
              ┌────────────┐  PlaybackStateChanged   ┌──────────────┐
 Spotify.app ─┤ notify.rs  ├────────────────────────►│              │
              └────────────┘                          │   event      │
              ┌────────────┐  3s/5s poll (osascript)  │   channel    │──► update() ──► App ──► draw()
 Spotify.app ─┤ applescript├────────────────────────►│  (mpsc)      │        ▲
              └────────────┘                          │              │        │ Actions
 keys/mouse ─────────────────────────────────────────►│              │────────┘ (side effects run on worker
 tap/sim audio, art, lyrics, web api, sonar file ────►│              │           threads, results return as Events)
                                                      └──────────────┘
```

## Key decisions and why

- **Shell out to `/usr/bin/osascript`** (like headless-spotify) instead of in-process
  NSAppleScript: no Objective-C surface for the common path, trivial to fake, easy to time out.
  Read everything in **one** batched script that returns a delimited string (≈1 process per poll,
  not 10). **Measured: 431 ms p50 / 437 ms p95 for a 17-field read — 5.5× the 80 ms the plan
  assumed** (`docs/APPLESCRIPT.md` §4). The cost is ~18 ms *per Apple Event inside Spotify's own
  handler*; Finder answers 30 events in 2 ms, and JXA, `osacompile`d scripts, list coalescing and
  a warm process all fail to recover it. So the read is **split**:
  - **Fast read** (state, position, volume, shuffling, repeating, id — 6 events, ~300 ms) on the
    poll tick.
  - **Slow read** (the 11 track fields) only when `id` differs from the last read, so it costs
    ~300 ms once per song rather than once per tick.
  Every script's first statement must be the `application "Spotify" is running` guard, because a
  bare `tell` **launches** a non-running Spotify (verified) and COMPAT rule 2 forbids that. The
  guard cannot be a multi-line `-e` (syntax error `-2740`); use
  `-e 'return (application "Spotify" is running) as string'`.
- **The notification is the primary path; the poll is a slow safety net.** Spotify's
  `com.spotify.client.PlaybackStateChanged` distributed notification **is received by a plain
  un-bundled Rust process** (`spikes/notify`), so `player/notify.rs` subscribes with
  `objc2-foundation`'s `NSDistributedNotificationCenter` (selector-based registration only — the
  block variant is not generated) and delivers ~170 ms after a play/pause/skip, inside TODO 3.9's
  300 ms budget. **It does not fire for seeks, volume, shuffle or repeat changes** — exactly the
  four things the notification's `userInfo` (13 keys) omits artwork for. So the poll must still
  read volume / shuffle / repeat / artwork, but at **3–5 s**, not 1 s, since nothing it covers
  changes quickly. This is what makes the expensive AppleScript read affordable at all.
- **Progress bar is interpolated locally** between polls and notifications (position + elapsed
  since last read while `playing`), so it is smooth without polling at 60 Hz.
- **Session history** is recorded by observing track changes while the TUI runs; it stores the
  `spotify url`/URI, so `enter` replays with `play track "<uri>"`.
- **Rendering art**: `ratatui-image` draws Kitty / iTerm2 / sixel / half-blocks. The protocol and
  cell size are chosen once at startup **from the environment** (`TERM_PROGRAM`, `TERM`,
  `KITTY_WINDOW_ID`; `render::detect_protocol`). **Not**
  `Picker::from_query_stdio()`: when a terminal does not answer within its second, its reader thread
  turns raw mode off under the running TUI and every keypress is silently dropped.
  **Measured (TODO 1.4):** cmux speaks Kitty at full fidelity; Terminal.app supports **only**
  half-blocks; iterm2 and sixel render in neither. Half-blocks is the always-works fallback and must
  look good on its own. Note that `new_protocol()` succeeds for unsupported protocols, so only a
  screenshot or a human eye can confirm an image actually renders.
- **Universal binary**: build `aarch64-apple-darwin` and `x86_64-apple-darwin`, `lipo` them, then
  `codesign --force -s -` (ad-hoc, free). An unsigned arm64 slice is killed by the kernel, so the
  ad-hoc step is required even though we never buy a certificate. Alternative if lipo causes
  trouble: ship two tarballs and use `on_arm` / `on_intel` in the formula.
- **Config** is plain TOML with defaults for everything, so an empty or absent file is valid.
- **Terminal colour is fixed up on the finished frame**, not in the widgets (`tui/colour.rs`):
  `NO_COLOR` strips fg/bg, RGB is downgraded to 256/16 colours by `COLORTERM`/`TERM`, and on a light
  background (asked once at startup with OSC 11) pale foregrounds are darkened. One pass over the
  buffer means no widget has to know which terminal it is in.
- **A frame identical to the last one is not sent** (`draw_if_changed` in `loop_.rs`, TODO 11.5):
  the Kitty cover's placeholder row overshoots `unicode-width`, so ratatui's diff would otherwise
  rewrite part of a paused screen every 100 ms.

## Testing strategy

| Layer | How |
| --- | --- |
| AppleScript parsing | Unit tests over captured real outputs (fixtures in `tests/fixtures/`) |
| `update()` | Table tests: `(state, event) → state` |
| Rendering | `ratatui::backend::TestBackend` snapshot tests at several sizes (wide, narrow, tiny, resize) |
| Visualizer renderers | Pure fns, tested with synthetic bars/samples |
| LRC parser, config, history | Plain unit tests |
| CLI | `assert_cmd` tests (`tests/cli.rs`) against the `FakePlayer` / `FakeLibrary` via the hidden `--fake` / `--fake-library` flags |
| `install.sh` | `tests/install_sh.rs` runs the real script against a `file://` mirror of a packed release |
| Web API | Fake `Library` + recorded JSON fixtures; no live network in CI |
| Real Spotify / real audio / cmux | Manual: `spikes/verify.sh`, the `examples/*-probe` programs, `scripts/screen.py`; the Sonar/headless rows are TODO phase 10, recorded in `docs/COMPAT.md` |

CI must be green with **no Spotify installed, no network, no audio device**.
