# Trak — architecture

Target design. The code currently contains only a scaffold (`src/main.rs`); this describes what to
build. If reality diverges, update this file in the same commit.

## Principles

1. **One crate, deep modules.** A small public surface per module, hidden complexity behind it.
   Split into a workspace only if compile times or a real reuse need appear.
2. **Every side effect sits behind a trait**, so the whole app runs and is tested without Spotify,
   a network, a terminal, or audio hardware:
   - `Player` — read state / send commands to Spotify (real: AppleScript; fake: in-memory).
   - `Library` — Web API calls (real: rspotify; fake: canned data). Version A only.
   - `Clock`, `Lyrics`, `AudioSource`, `Store` (config/token), `Notifier`.
3. **Pure state + pure render.** `App` holds all state; `update(App, Event) -> App` is deterministic;
   `render(&App, Frame)` only draws. Snapshot-test rendering with ratatui's `TestBackend`.
4. **Never block the UI thread.** Anything slow (osascript, HTTP, image decode, FFT) runs on its
   own thread/task and reports back as an `Event` over a channel.
5. **Fail soft.** Optional features (visualizer real audio, Sonar state, headless badge, lyrics,
   Web API) degrade silently to the simpler behaviour and say why in a status line. Never panic on
   bad external data; return `Result`, log, and continue.

## Module map (planned `src/`)

```
main.rs            argv → cli or tui
cli/               clap definitions, one-shot commands, --plain/--json output
tui/
  app.rs           App state, Event, update()
  render.rs        top-level layout + breakpoints (wide / stacked / compact / idle)
  now_playing.rs   art or visualizer, title block, progress, controls, volume
  tabs/            history.rs info.rs lyrics.rs  (A: search.rs playlists.rs queue.rs liked.rs library.rs artist.rs album.rs)
  settings.rs      the interactive config screen + guided Client ID flow
  help.rs          ? overlay
  theme.rs         accent (art/green/terminal), border styles
  input.rs         key + mouse → Action
player/
  mod.rs           trait Player, PlayerState, TrackInfo, RepeatMode
  applescript.rs   osascript runner ($TRAK_OSASCRIPT), one batched read script, write scripts
  notify.rs        PlaybackStateChanged distributed-notification listener (objc via cidre or a tiny helper)
  fake.rs          in-memory Player for tests
history.rs         session play history (ring buffer of TrackInfo with URI)
art.rs             fetch + cache artwork URL, decode, dominant colour, ratatui-image protocol picker
visualizer.rs     AudioSource trait + SimulatedSource + the four renderers (spectrum / mirrored /
                  waveform / circular) as pure fns. > **One file, not `viz/{source,dsp,render}.rs`**:
                  the tap half of the source never existed, because 1.5 could not find a way to tap
                  Spotify's process alone, so there is no dsp layer to separate out yet. Split it when
                  8.3 lands a real tap and there is a real second implementation to separate from.
lyrics.rs          LRCLIB client + LRC parser + "current line for position"
config.rs          load/save/defaults/migrate ~/.config/trak/config.toml
web/  (A)          auth.rs (PKCE, loopback server), api.rs (rspotify wrapper), token.rs (keychain/file)
integrations/
  sonar.rs         state.json watcher
  headless.rs      `headless-spotify status --json` / `launch`
notify.rs          display notification on song change
```

## Data flow

```
              ┌────────────┐  PlaybackStateChanged   ┌──────────────┐
 Spotify.app ─┤ notify.rs  ├────────────────────────►│              │
              └────────────┘                          │   event      │
              ┌────────────┐  1s/3s poll (osascript)  │   channel    │──► update() ──► App ──► render()
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
- **Rendering art**: `ratatui-image` picks Kitty / iTerm2 / sixel / half-blocks via
  `Picker::from_query_stdio()`. **Measured (TODO 1.4):** cmux speaks Kitty at full fidelity and is
  detected unattended; Terminal.app supports **only** half-blocks; iterm2 and sixel render in neither.
  The picker also returns a **cell size**, which the layout must use. Half-blocks is the always-works
  fallback and must look good on its own. Note that `new_protocol()` succeeds for unsupported
  protocols, so only a screenshot or a human eye can confirm an image actually renders.
- **Universal binary**: build `aarch64-apple-darwin` and `x86_64-apple-darwin`, `lipo` them, then
  `codesign --force -s -` (ad-hoc, free). An unsigned arm64 slice is killed by the kernel, so the
  ad-hoc step is required even though we never buy a certificate. Alternative if lipo causes
  trouble: ship two tarballs and use `on_arm` / `on_intel` in the formula.
- **Config** is plain TOML with defaults for everything, so an empty or absent file is valid.

## Testing strategy

| Layer | How |
| --- | --- |
| AppleScript parsing | Unit tests over captured real outputs (fixtures in `tests/fixtures/`) |
| `update()` | Table tests: `(state, event) → state` |
| Rendering | `ratatui::backend::TestBackend` snapshot tests at several sizes (wide, narrow, tiny, resize) |
| Visualizer renderers | Pure fns, tested with synthetic bars/samples |
| LRC parser, config, history | Plain unit tests |
| CLI | `assert_cmd` tests against the `FakePlayer` via a hidden `--fake-player` flag / env var |
| Web API | Fake `Library` + recorded JSON fixtures; no live network in CI |
| Real Spotify / real audio / cmux | Manual, listed in TODO phase 10, results recorded in `docs/COMPAT.md` |

CI must be green with **no Spotify installed, no network, no audio device**.
