# Trak — TODO

The master task list. **Read `AGENTS.md` first**, then `docs/SPEC.md` (what to build, all decisions
made), `docs/COMPAT.md` (Sonar / headless-spotify contract), `docs/ARCHITECTURE.md` (how).

## How to use this file

- Work **top to bottom**; phases are ordered by dependency and by risk. Each task lists `Needs:`
  (tasks that must be done first), `Do:`, and `Done when:` (an observable check, not "code written").
- Take the first unchecked task whose `Needs` are met. Do one task per commit (or a few tiny ones).
- When done: tick the box, add a one-line note under it if you learned something a later agent needs
  (real command output, a surprise, a decision), commit, push. Ticking a box without meeting
  `Done when` is a lie to the next agent; if you can only partly finish, leave it unticked and say what
  is left.
- If a task is impossible or wrong, do not silently skip it: add a `> BLOCKED:` or `> CHANGED:` note
  with the reason and update `docs/SPEC.md` / `docs/ARCHITECTURE.md` in the same commit.
- Anything marked **[owner]** needs the human (a browser login, a paid step, a physical prompt click).
  Do everything else first, then list the owner steps in your final message.
- `Verify` items are claims believed true but not confirmed. Confirm before building on them, and
  write the confirmed facts into the doc named in the task.

Legend: `[ ]` todo · `[x]` done · **A/B** = with / without a Spotify Client ID.

---

## Risks (read before starting)

| ID | Risk | Mitigation / where handled |
| --- | --- | --- |
| R1 | ~~Real-audio visualizer: the process-tap permission is attributed to the terminal app; a CLI child may not get a usable "System Audio Recording" prompt~~ **REFUTED (1.5): a global tap works from an un-bundled CLI with NO prompt at all** — 48 kHz mono f32, ~47 950 samples/s, real audio at -17.6 dBFS, clean teardown. The machine had no audio TCC grant beforehand | `docs/AUDIO-TAP.md` §3a. **A new blocker replaces it: see R6** |
| R2 | ~~Spotify ≥ 1.3.x ignores AppleScript `set sound volume`~~ **REFUTED on 1.3.1.234 (1.1):** sets work 8/8, but the read-back is often target−1 (quantisation). Rule now = read back with a ±1 tolerance (`docs/APPLESCRIPT.md` §5) | 4.4 (reframed as a preference) |
| R3 | ~~Keychain items created by an ad-hoc-signed binary can re-prompt after every upgrade~~ **CONFIRMED AND WORSE (1.8):** a keychain item is readable only by the exact binary that created it. A *new* identity raises a dialog and blocks (first encounter); a repeat of the *same* identity errors immediately from cache. Every release is a new identity, so Trak always gets the blocking case — a **hang, not a prompt** | **Resolved: use a `0600` file** (`docs/KEYCHAIN.md`). SPEC §2 and §6 updated. 7.4 drops the second backend |
| R4 | ~~Spotify Web API developer-mode rules changed recently~~ **CONFIRMED AND WORSE THAN EXPECTED (1.7):** `localhost` redirect URIs are **banned** (use `http://127.0.0.1`, no port); Premium now required of the **app owner**; user cap **5**; all batch "get several" endpoints, `/markets` and **`/artists/{id}/top-tracks` removed**; `Track.popularity` removed; search `limit` max **10**; refresh tokens expire in **6 months**; no numeric rate limits are published | All recorded with citations in `docs/WEB-API.md`; SPEC §6 and this phase rewritten to match. Still verify playlist `/items` against a real dev-mode login |
| R5 | ~~cmux may not pass the Kitty image protocol through~~ **REFUTED (1.4):** cmux speaks Kitty graphics at full fidelity, and `Picker::from_query_stdio()` detects it unattended | `docs/TERMINALS.md` + `docs/images/spike-1.4-cmux.png`. Half-blocks remains the Terminal.app path and must still look good |
| R6 | ~~`cidre` API for process taps is unstable / under-documented~~ **RESOLVED (1.5): the fault was the missing pid→AudioObjectID translation, not the OS or the binding.** `CATapDescription.h` documents that the array holds **AudioObjectIDs**, not pids; translating via `kAudioHardwarePropertyTranslatePIDToProcessObject` (`'id2p'`) makes every process-specific shape work. End-to-end capture delivered real Spotify audio from Spotify's process only, clean teardown | `docs/AUDIO-TAP.md` §3b/§3c; spike `spikes/tap/src/bin/tap-objc2.rs`. **8.3 is unblocked** |
| R7 | Spotify hiding is blocked on ≥ 1.3.1, so headless behaviour cannot be tested today | COMPAT test matrix row stays unverified until it works; do not fake it |
| R8 | Homebrew audit / policy rejects the formula | `brew audit --strict` in CI and before every release (9.5) |

---

## Phase 0 — Project init

- [x] 0.1 Create repo `Kathir-D/Trak` (public), MIT `LICENSE` (with shpotify's notice), `.gitignore`,
      `rustfmt.toml`, `VERSION`, minimal `Cargo.toml` + `src/main.rs` (`trak --version`).
- [x] 0.2 Write `AGENTS.md`, `CLAUDE.md`, `README.md` (placeholder), `TODO.md`, `docs/SPEC.md`,
      `docs/COMPAT.md`, `docs/ARCHITECTURE.md`, `docs/AGENT-PROMPTS.md`, `THIRD-PARTY-NOTICES.md`.
- [x] 0.3 CI (`.github/workflows/ci.yml`): fmt, clippy `-D warnings`, test, release build,
      VERSION == Cargo.toml version check, on `macos-15`.
- [ ] 0.4 **[owner]** Run the sibling agent prompts in `docs/AGENT-PROMPTS.md` (1 = Sonar,
      2 = headless-spotify) whenever convenient. Not blocking. Update their `Status:` lines when done.
- [x] 0.5 Confirm CI is green on GitHub after the first push; add the badge to the README. (green on first push, badge is in the README)
      Needs: 0.3. Done when: Actions tab shows a green run on `main`.

---

## Phase 1 — Spikes: kill the unknowns before building on them

Each spike ends with facts written to a doc, not just working code. Throwaway code goes in
`spikes/` (git-ignored is fine; commit only if it is useful), the **findings** are committed.

- [x] 1.1 **AppleScript field survey.** Full table in `docs/APPLESCRIPT.md`; fixtures in
      `tests/fixtures/applescript/`. > Later agents: **`starred` is broken (-10000)** — liking needs
      the Web API. `duration` is **ms** despite the sdef saying seconds. `id` == `spotify url` (both
      full URIs). **An ad has empty album/artist, `missing value` artwork, `0` numerics, and a
      `spotify:ad:` URI** — the parser must survive it. A `tell` **launches** a non-running app
      (verified with TextEdit), so the `is running` guard is mandatory and must be the first
      statement of the same script.
- [x] 1.2 **Batched read script + cost.** > **p50 431 ms / p95 437 ms for the 17-field read — 5.5×
      over the 80 ms budget.** Cause is ~18 ms *per Apple Event inside Spotify's handler* (Finder
      does 30 events in 2 ms; JXA, `osacompile`, list-coalescing and a warm process all fail to help;
      paused == playing, so it is not audio contention). > Design response, recorded in
      `docs/APPLESCRIPT.md` §4: **split fast (6 fields, ~300 ms) / slow (11 track fields, only when
      `id` changes)**, keep the poll on a worker thread, and lean on 1.3's notification to make the
      poll a safety net. Process spawn alone is 50 ms, so the `is running` check must live inside the
      same script.
- [x] 1.3 **`PlaybackStateChanged` notification.** > **It works, from a plain un-bundled Rust
      CLI** (`spikes/notify` proves it; the "only bundled apps get distributed notifications"
      worry does not apply). Use **`objc2-foundation` + `define_class!` with the
      selector-based `addObserver`** — 0.3 does not generate the block variant — and keep the
      observer alive for the process. Latency **~170 ms** (inside 3.9's 300 ms budget). > **Real
      userInfo has 13 keys, not the 7 guessed**, and the guesses were wrong in detail: `Player
      State` is `Playing`/`Paused` (capitalised, unlike AppleScript's `playing`), `Track ID` is
      the **full URI**, `Duration` is **ms**, `Playback Position` is **seconds**, the play count
      key is `Play Count` not `played count`, and there is a new `Has Artwork` **boolean**. > **No
      `artwork url` in the notification**, which is the strongest reason to keep the slow
      AppleScript read. > **Fires for play/pause/skip only — NOT for seek, volume, shuffle or
      repeat** (each isolated in its own 6 s window). Decision: notification is primary, poll drops
      to 3–5 s as a safety net. Written up in `docs/APPLESCRIPT.md` §9 and `ARCHITECTURE.md`.
- [x] 1.4 **Album art in cmux and other terminals.** `docs/TERMINALS.md`, with real screenshots in
      `docs/images/spike-1.4-{cmux,terminal}.png` taken with `screencapture`; the program is
      `spikes/images`. > **R5 refuted: cmux renders the Kitty graphics protocol at full fidelity**
      (`TERM_PROGRAM=ghostty`, Ghostty 1.3.2-HEAD) and `Picker::from_query_stdio()` picks `Kitty`
      unattended. **Terminal.app 488 renders only halfblocks** and prints the kitty/iTerm2/sixel
      escape sequences as visible text. iterm2 and sixel render nothing in *either* terminal.
      > **Carry into 4.1:** the picker also returns a **cell size** — `(8,17)` in cmux, font
      `(10,20)` in Terminal.app — and the art layout must use it. > **Trap:** `new_protocol()`
      *succeeds* for all four protocols in both terminals, so `is_ok()` proves nothing about
      rendering; 4.1's "done when" cannot be met by a unit test alone.
- [x] 1.5 **Process-tap visualizer spike (R1, R6).** Spike is `spikes/tap`; write-up is
      `docs/AUDIO-TAP.md` §3. > **R1 REFUTED — no permission prompt at all.** A global
      tap from an un-bundled `cargo run` binary delivered real audio (48 kHz, 1 ch, 32-bit float,
      958 976 samples in 20 s ≈ 47 950/s, -17.57 dBFS RMS, peak 0.383) with **no System Audio
      Recording click**, on a machine whose TCC had no audio grant at all. Clean teardown, no
      leftover device. > **R6 RESOLVED — the fault was the missing pid→AudioObjectID translation.**
      `CATapDescription.h` says the array holds **AudioObjectIDs, not pids**; every earlier attempt
      passed the pid and got `!obj` (560947818) for any non-empty list. Translating first with
      `kAudioHardwarePropertyTranslatePIDToProcessObject` (`'id2p'`) makes all four process-specific
      shapes work, including the include-list mono mixdown SPEC §7 wants. The objc2 bypass
      (`spikes/tap/src/bin/tap-objc2.rs`, ~60 lines) builds `CATapDescription` directly; cidre's
      class resolution was fine all along, its binding just does not translate. End-to-end capture:
      **575 488 float samples in 12 s ≈ 47 957/s, real Spotify audio from Spotify's process only**
      (RMS −12 to −17 dBFS), clean teardown, no leftover tap/aggregate device. > Traps for 8.3/8.5:
      (a) the tap is 48 000 Hz — read `asbd.sample_rate`, do not hard-code; (b) `ca::device_start`
      returns a `StartedDevice` that must be kept alive — dropping it early stops the device and
      looks exactly like a hang with 0 samples; (c) `name` on `CATapDescription` is an *instance*
      method and objc2 panics on a class-method send to it — `AnyClass::name()` instead; (d) the
      `'prs#'` process list read returns `'nope'` here even though `'id2p'` works — translate the
      pids you care about, do not enumerate. > Do NOT fall back to a global tap — it captures all
      system audio, including Sonar's, which is what COMPAT rule 3 is about. No [owner] step was
      needed — there was no prompt to click.
- [x] 1.6 **cavacore feasibility.** `docs/AUDIO-TAP.md`; spike is `spikes/viz`; bar output is a
      committed fixture at `tests/fixtures/visualizer/cavacore-bars.txt`. > **Builds on stable
      (rustc 1.98.1) for both `aarch64-apple-darwin` and `x86_64-apple-darwin`** — 1.6's
      cross-target requirement is met. **Chosen: 44 100 Hz, 32 bars, mono** (96 kHz and 16–96 bars
      all build too), but **8.3 must take the rate from the tap's format, not a constant** —
      `cavacore` trusts the sample rate it is given. > Frequency discrimination is real: 80 Hz →
      bars 0–12, 440 Hz → bars 21–26, noise → broadband, silence → nothing. > **Two traps:** the
      crate carries peak/autosens state inside a `Cava`, so **one instance per stream** (reusing one
      made an 80 Hz and a 440 Hz sine report identical spectra), and the **output is not
      normalised** — autosens ramps over ~1 s, so renderers must scale against pane height and a
      recent peak. > API notes for `viz/dsp.rs`: `CavaBuilder::default()` (no `new()`), and
      `SampleRate::new(x)` not `SampleRate::Hz(x)`.
- [x] 1.7 **Spotify Web API reality check (R4).** `docs/WEB-API.md`, every claim cited and dated,
      two of the most plan-changing (the `localhost` ban and the Feb 2026 removal list) verified a
      second time by hand against the live docs. > Later agents: **`localhost` is banned** — register
      `http://127.0.0.1` with no port and bind an ephemeral one (7.3's instructions must say this
      exactly). **Developer mode needs a Premium *app owner* and caps at 5 users.** **`GET
      /artists/{id}/top-tracks` is gone with no replacement** (7.10 is albums-only). Library writes
      go to `/me/library`, is-liked to `/me/library/contains`. **All batch "get several" endpoints
      are gone**, so `rspotify`'s id-list helpers must not be used (7.5). Search `limit` max is 10.
      **Refresh tokens last 6 months** (7.2). **No numeric rate limits are published** and no
      `X-RateLimit-*` headers exist — 429 handling uses `Retry-After` only. > **Unresolved:** the
      Feb 2026 "still available" list omits `/playlists/{id}/items` while the migration guide tells
      you to use it. Verify in 7.7/7.11 with a real login; do not assume either way.
- [x] 1.8 **Keychain vs ad-hoc signing (R3).** `docs/KEYCHAIN.md`, with the experiment in
      `spikes/keychain`. > Later agents: **a keychain item is readable only by the exact binary that
      created it.** A re-signed binary blocks on both read *and* store; Apple's `security` blocks
      too; a fresh linker-signed build blocks on a foreign item. Since
      `scripts/package-release.sh` re-signs on every release, the Keychain means **a hang on the
      first launch after every `brew upgrade`** — a Security.framework call blocked on
      authorization cannot be cancelled, so even a timeout cannot recover it. > **Decision: a `0600`
      file at `~/.config/trak/token.json`** (respect `XDG_CONFIG_HOME`, dir `0700`, atomic write,
      refuse to read if the mode is looser than `0600`). > 7.4 keeps the `Store` trait for testability
      but builds **one** backend, not two.

---

## Phase 2 — Core: Player layer + CLI parity with shpotify

Reference: upstream `spotify` bash script at https://github.com/hnarayanan/shpotify (or the local
clone at `../shpotify-tui/spotify` on the owner's machine). Behaviour reference only; write idiomatic Rust.

- [x] 2.1 **Crate skeleton.** `clap` derive + `thiserror` only, so far. > `serde`/`toml`/`dirs` are
      not added yet: nothing needs them until 5.x (config) and 6.x (lyrics cache), and adding a
      dependency before it is used is how the lock file rots. `assert_cmd`/`predicates` are
      dev-dependencies for the 2.5 CLI tests. Module tree from ARCHITECTURE; `player/`, `cli/` and
      `testutil` exist. > `clap` will not flatten `trak toggle shuffle|repeat` next to a
      play/pause `toggle`, so there is no play/pause subcommand: shpotify's `pause` *is* the
      toggle, and `toggle` takes the required `shuffle|repeat` argument SPEC §9 specifies.
- [x] 2.2 **`Player` trait + `PlayerState`/`TrackInfo` types + `FakePlayer`.** `src/player/fake.rs`.
      > `Quirks` is the important part: `quantise_volume`, `ignore_volume_writes`, `not_running`,
      `denied`, `hang`. Without them the read-back logic and the idle card could only be tested
      against a player that always behaves. > `writes()` records every write so a test can prove a
      **poll never writes** (COMPAT rule 3). `volume_write_landed` is the ±1 rule, in one place.
      `TrackInfo` uses `Option` for popularity / play_count / artwork_url / uri, because an advert
      really does report 0 and `missing value` and the Info tab has to tell "none" from "zero".
- [x] 2.3 **`AppleScriptPlayer`.** `src/player/applescript.rs` + `src/player/parse.rs`. > Script is
      fed on **stdin**, not `-e` or a file, because the `is running` guard is a multi-line `if` and
      a multi-line `-e` is a `-2740` syntax error (`docs/APPLESCRIPT.md` §3). Guard is the first
      statement, so a non-running Spotify can never be launched (COMPAT rule 2). > **Two real bugs
      the fixture tests caught, both the same mistake:** the parser was strict about numeric and
      boolean fields, but "nothing is loaded" reports **empty strings for every field, not zeros**
      — so the exact state the idle card has to render was rejected. Empty now means 0/false;
      genuinely malformed values still fail loudly. > `play_uri` validates against an **allow-list**
      rather than escaping, so a quote in a user-supplied URI cannot close the AppleScript string
      literal. Verified live: `trak status` prints real state, `-1743` becomes a named
      `PermissionDenied` with a message pointing at Privacy & Security › Automation.
- [x] 2.4 **Write actions with read-back.** `src/player/actions.rs` holds `seek_checked`,
      `set_volume_checked`, `step_volume` and the background `Worker`. > The ±1 tolerance lives in
      **one** function, so there is one implementation and one set of tests; getting this wrong is
      what makes Trak hide the volume meter after every keypress. > A seek is compared against the
      **clamped** target, not the raw request — the first version reported every over-seek as a
      failure, which two tests caught. > The `Worker` drops a submission while one is in flight, so
      a held key cannot build a backlog. > `check_playable_uri` is an **allow-list in
      `player/mod.rs`, not in the AppleScript transport** — it was in the transport first, and the
      CLI tests caught that the fake bypassed it. User input enters Trak at exactly one place.
- [x] 2.5 **CLI.** Every shpotify command: `status [artist|album|track]`, `play`, `pause`, `stop`,
      `quit`, `next`, `prev`, `replay`, `pos`, `vol up|down|show`, `toggle shuffle|repeat`,
      `share url|uri`. Tidy on a TTY, plain when piped, `--plain` and `--json`. Exit codes 0/1/2.
      27 `assert_cmd` tests via a hidden `--fake` flag, so they run in CI with no Spotify.
      > `stop` and `quit` are in SPEC §9 and shpotify; `stop` is "pause if playing" because
      Spotify's dictionary has **no `stop` command** (`tell ... to stop` silently no-ops).
      > There is no play/pause subcommand: `clap` will not flatten `toggle shuffle|repeat` beside
      one, and shpotify's `pause` *is* the toggle. > Piped output carries no ANSI, asserted.
      > Verified live against real Spotify: `status`, `vol up|down` (read-back visible as target−1,
      which is the quantisation), `pos 60`.
- [x] 2.6 **`share url|uri`** copies via `pbcopy` and prints the link. > A clipboard failure is
      not worth failing the command over, so it is best-effort; the link always goes to stdout.
- [x] 2.7 **`play <name>` / `play album|artist|list <name>` / `play uri`.** > Finished with 7.12:
      every spelling exists and is tested — a bare name searches tracks, `album|artist|list`
      search their group, and both the `uri` subcommand and a bare positional play straight
      through AppleScript after the allow-list check. A name with no connected Web API prints
      the Client ID steps and exits 2, as shpotify did. The pick and its one-line report are
      `web::api::best_match` and `cli::choose`; see the 7.12 note.
- [x] 2.8 **`trak` (no args) → TUI entry; `trak config` → settings entry.** > Handled **before the
      player is built**, so it works on a machine where Spotify is not installed and so it cannot
      launch Spotify (COMPAT rule 2). > With no terminal it prints the settings as TOML and exits 2. > Bare `trak` exists
      and says what does work, exit 2. Replaced by the TUI in 3.1. `trak config` not added yet.
- [x] 2.9 Update README "Usage" with real, copy-pasted output of each command. Done when: matches
      the binary. (done in eb70a20; the box had been left unticked)

---

## Phase 3 — TUI shell (Version B): layout, history, info

- [x] 3.1 **Terminal plumbing.** `src/tui/loop_.rs`. Alternate screen, raw mode, line wrap off,
      mouse capture on, all restored from a **panic hook** as well as on the happy path. > The
      **worker shuts down and joins its thread on `Drop`**, so no thread is still inside an
      AppleScript call when the terminal is restored — that is how a TUI leaves a terminal broken.
      > **A TUI on a pipe refuses cleanly** (exit 2, points at the CLI) instead of entering raw mode
      and looking hung. > Verified by running it in cmux and screenshotting
      (`docs/images/tui-3.3-wide.png`). > **[owner]** worth a look: a live resize, and a forced
      panic, since neither can be checked without hands on a terminal.
- [x] 3.2 **`App` state, `Event`, `update()`.** `src/tui/app.rs`, 40 tests. `update` is pure and
      returns the commands to run; nothing draws or writes there. > **The poll is 3 s playing / 5 s
      idle, not the 1 s the task guessed**, because 1.3 proved the notification is the primary
      update path and the poll only covers what it is silent about (seek, volume, shuffle, repeat,
      artwork) — and a read is ~430 ms. > The progress bar is **interpolated locally** from the last
      read, clamped at the duration and frozen while paused. > `PlayerCommand` lives in
      `player::actions`, not the TUI: the worker reports it, so the player layer needs it. > The
      worker's result is a `WorkerResult` enum, not a `Result<(), _>`: a poll's payload *is* the
      state, and folding both into one type is what made the first version poll without ever
      displaying anything. > A test asserts **a poll or a tick never produces a command** (COMPAT
      rule 3) and another that a second key is dropped while one is in flight.
- [x] 3.3 **Wide layout.** `src/tui/render.rs`. Header with status dot and clock, Now Playing with
      title/artist/album/shuffle/repeat, the interpolated bar, ⏮ ⏯ ⏭, the volume meter, the tab
      strip and the footer hints. Rounded by default. Snapshot at 100×30 plus a real cmux screenshot
      (`docs/images/tui-3.3-wide.png`). > The art area is a **framed placeholder**, not a gap, so
      the layout reads as designed and 4.1 already has its space reserved. > Every pane is built
      from an `inner()` rect shrunk by one cell, which is what stops borders tearing. > The volume
      line says `vol` in plain text: the 🔊 emoji rendered as a **muted speaker** in cmux next to a
      100% meter, which says the opposite of what it means.
- [x] 3.4 **Responsive breakpoints.** One function, `layout_for(w, h)`, so the renderer and the
      tests cannot disagree. **wide ≥ 76×16 · stacked ≥ 46×12 · compact ≥ 30×8 · too small below
      that.** A test sweeps 27 widths × 20 heights asserting the wide threshold holds, and another
      renders at 7 sizes × 7 heights plus the idle case and asserts no panic. > Bars and meters are
      asserted to be **one cell wide per character** (via `unicode-width`), because a wide glyph in a
      bar is the first thing that tears a layout.
- [x] 3.5 **Idle card.** Centred, replaces the whole dashboard, says "press enter to launch it in
      the background" and "Trak never starts Spotify on its own". > `enter` and space are the only
      keys it accepts, and a test asserts every other key does nothing on it. > The launch runs
      `headless-spotify launch` when it is on PATH, else `open -g -j -a Spotify` — no focus, no Dock
      bounce. A test asserts the flags, not just that the command exists.
- [x] 3.6 **Session history + History tab.** `enter` plays the selected row by URI. **Verified
      against a real Spotify**: skip, arrow to the row, `enter`, and the same
      `spotify:track:` is playing again. > The tab draws the *now playing* track as its own `▶` row
      above the history, because the history only holds tracks that have already finished —
      without that row the current song is missing from the tab you use to remember what you just
      played. > **The cursor was clamped at `history.len()` rather than `len - 1`**, so it could sit
      one past the last row with nothing selected, and `j` counted into a list the renderer never
      draws. Both are fixed and pinned by tests. > A new row lands at the *front* of the view
      (newest first), so a track change steps the cursor down by one to keep the same song selected
      — **but only if the user has moved the selection at all**. Without that condition a session
      left alone walks the cursor to the oldest track, because every skip nudged it one row down. > 
      `FakePlayer::play_uri` was **not** applying the URI allow-list, so the fake accepted URIs the
      real player refuses; a test that passed against the fake would have failed in production. >
      **The first read waited out a whole poll interval**, so the TUI opened on the idle card for five
      seconds even with Spotify playing and no window focused — which looks exactly like Spotify not
      running. `last_poll` now starts as "never polled", which is due immediately.
- [x] 3.7 **Info tab.** Every AppleScript fact, plus **"heard this session"**, counted by URI
      across the history. > The label column is 18 wide, not 14: "heard this session" is the longest
      label and at 14 the value ran straight into it with no gap. > The count is the current track
      plus every history entry with the same URI, so a track heard once reads `1`, not `0` — and a
      poll can never inflate it, because re-reading the same track is not a change. > An advert has
      no URI to count by, so it prints a dash rather than a number it could not have earned.
- [x] 3.8 **Input.** Every B key in SPEC §4 is bound and each has an `update()` test; arrows fold onto
      their vim equivalents so one binding serves both; `Tab`/`Shift-Tab` cycle the tabs; `?` opens a
      centred help overlay that **swallows keys so nothing fires behind it**, while `q` still quits.
      > **`esc` closes the overlay** (SPEC §4) and is inert everywhere else, so it cannot quit by
      accident. > **The help overlay is checked against `docs/SPEC.md` itself, not a copy of it.** The
      old test listed sixteen strings inside the test file, which is a copy that goes stale — the
      exact failure it existed to catch. The new one parses the §4 table, keeps the rows marked
      `both`, and requires every key in it to be either a token in the overlay's key column or listed
      in `NOT_YET` **with a reason**. Tokens, not substrings: a substring search passes for `a` because
      `tab` contains it, and case matters because `L` is not `l`. The overlay **no longer advertises
      keys that do nothing yet** (`a`, `v`, `L`, `,`) — listing a dead key is a small lie. > Both
      directions are checked, because either alone is a test that cannot fail: dropping a key from the
      help fails the first, and an excuse that has quietly become *false* fails the second — every
      `NOT_YET` key is pressed through `update` and the whole app state fingerprinted. Five mutations
      were run against it and each one fails the build.
- [x] 3.8 **Input, mouse.** Click a tab, a history row or a transport control; click or drag the
      progress bar to seek; wheel over the list to scroll. Gated by `[input] mouse` (default on): when
      off the loop does not ask the terminal for events, **and `update` ignores any that arrive anyway**
      — a terminal that reports them regardless must not turn them into Spotify writes (COMPAT rule 3).
      > **Clicks are hit-tested against the regions the last frame recorded, not against a second copy
      of the layout.** `draw_with` fills a `Regions` as it draws and the loop keeps it, so a click
      lands where the pixels are and there is no second layout calculation to drift. A test reads the
      *rendered buffer* at each region and checks something is actually drawn there: a tab region must
      sit over its label, a control over a transport glyph, the bar over a row of ─. > Two things only
      a real terminal shows: **ratatui draws a block title over the top border, not inside the body**,
      so the tab strip is a row higher than the pane's inner area; and a click resolves to the
      **middle of the cell** it landed in, so the ends of the bar are half a cell in, not 0 and 1.
      > **The history list scrolls.** It can be 500 rows in a 20-row pane, and `j` used to walk the
      cursor off the screen with nothing to show for it. The app keeps a scroll that follows the
      selection, the loop tells it how many rows fit, and the view scrolls only when the cursor would
      leave it — not re-centring on every notch, which is the behaviour you can predict. The cursor is
      now clamped to the *list*, not to `HISTORY_VIEW`, so the oldest rows of a long session are
      reachable; a test walks 570 `j` presses and asserts the cursor is always one of the rows on
      screen. > **Verified live against real Spotify**: clicking ⏭ skipped the track, clicking the
      Info label switched tab, dragging the bar seeked to 41 s, the wheel scrolled the list, and
      clicking a history row then pressing enter played the remembered track.
- [x] 3.9 **Subscribe to `PlaybackStateChanged`.** `src/player/notify.rs`, and `cargo run --release
      --example notify-probe` measures it. **Measured 172.7 ms and 173.1 ms for pause and play**
      against a real Spotify, inside the 300 ms budget. > **The notification is delivered on the
      *main* run loop**, whichever thread registered it — a background run loop receives nothing at
      all, and a plain `sleep` in the wait path is equally silent. The event loop's wait *is* a
      run-loop pump, so the fast path costs nothing extra. > `merge` keeps what the notification
      does **not** carry: **no artwork url**, no play count, no popularity, no volume. Losing the
      artwork would blank the cover on every skip, so it is carried over. A track change also
      triggers one real read, because the notification cannot fill those fields in. > The userInfo
      values arrive as untyped `AnyObject` (`NSTaggedPointerString` for text, `__NSCFNumber` for
      numbers), so each is downcast to the class it is documented to be.
- [x] 3.10 **Volume behaviour** per COMPAT rules 3 and 5. > **The read-back was being computed and
      then thrown away**: `run_one` mapped every `WriteOutcome` to `()`, so the app could not tell
      "Spotify ignored this" from "Spotify took it". `WorkerResult::CommandDone(cmd, Result<()>)`
      is now `Command(CommandOutcome)`, whose `result` is `Err` / `Ok(None)` / `Ok(Some(outcome))` —
      three outcomes, because *succeeded at being ignored* is its own thing. > **The meter now shows
      the user's volume, not the polled one.** A read only updates `read_volume`; `user_volume` is
      set by `m`, `+`, `-` and by the target the player layer actually aimed for, so the clamp lives
      in one place. This is COMPAT rule 3, not just rule 5: during a Sonar fade the polled value *is*
      the mid-fade value, and a meter that follows it down says Trak turned the music down. > An
      ignored volume write hides the meter and shows the one-line notice; a landed one brings it
      back, so one bad keypress is not a permanently broken display. An ignored *seek* gets the
      notice but does not hide the meter. > Measured on real Spotify 1.3.1.234: `-` at 78 → 67 →
      56, mute → 0, unmute restores exactly what it had, `+` at 100 clamps and stays 100, and a set
      of 90 reads back **89** — the ±1 quantisation rule 5 documents, confirmed again.
- [x] 3.11 **Copy share URL (`c`)** with a transient "copied" toast. > `pbcopy` rather than a
      pasteboard API: no entitlement, no extra binding, and it is what every other tool on the
      machine already uses. A test round-trips through `pbpaste` rather than trusting the exit
      status. A refused pasteboard is **not** an error — the link is already in the toast.

---

## Phase 4 — Now Playing richness and integrations

- [x] 4.1 **Album art.** `src/art.rs` (fetch + cache) and `render::Images` (protocol + draw).
      > **Cache**: `~/Library/Caches/trak/<fnv>.img`, bounded to 64 files by mtime, written to a
      `.part` file and renamed so a killed fetch cannot leave a half-written file the length check
      would accept. **A zero-length file counts as a miss** — otherwise one interrupted download is a
      permanent hole. Only `https` on Spotify's own image hosts is fetched; a 10 s timeout and an
      8 MB cap are enforced on the *bytes read*, not on `Content-Length`, because a response can lie
      about its length. > **Decode from the bytes, not with `image::open`.** That infers the format
      from the file extension, and the cache is deliberately `.img` because it will not claim to
      know whether Spotify sent a JPEG or a WebP. This was a real bug found by running it: a 239 KB
      640×640 JPEG downloaded correctly, cached correctly, and then never drew, because the decode
      failed on the extension. > **Fetch and decode both happen on the player worker**; only the
      resize happens on the render thread. > **The image is sized in cells and scaled here, not left
      to the protocol**: `ImageSource::new` derives its size from the image's *natural* size and the
      font size and then never grows it, so a 32×32 cover with a (10, 20) cell became 3×2 cells and
      a 12×6 pane drew it at 3×2. Scaling to the target rectangle first is the only way to fill it.
      > **A resize invalidates the encoded image**, because Kitty's state is only valid for the size it
      was encoded at. > A **stale download is dropped and always clears `loading`** — clearing it only
      on the happy path is how one skipped track disables art for the rest of the session. The cover is
      matched to the track by URL, so a slow download never shows the previous album under a new title.
      > **Art needs a hole of at least 6×4 cells**; below that the space goes to the text, which is
      how it "disappears cleanly" on a small terminal. > The **placeholder is no longer a framed box**:
      the Now Playing pane's own border already frames the hole, and a second frame drawn as text
      landed a whole art-height below the space it was reserving. While a cover is downloading the
      hole says `fetching cover…`; otherwise it is empty. > **Verified live**: with a pty answering
      the Kitty query exactly as cmux does, Trak sent **25 chunks totalling 73 984 bytes = 136×136×4**
      — a raw RGBA image at `f=32`, sized 17×8 cells from cmux's 8×17 px cell — and **zero**
      half-block glyphs. Without an answer it fell back to half-blocks and drew 64 `▀` cells. The
      half-block path is snapshot-tested: a red/blue fixture must put both colours in the buffer.
      > **[owner]** still worth one look: real cover art in a real cmux window, since a pty can only
      prove the bytes Trak sends, not how they land on screen. > **The protocol is detected from the
      environment, not by asking the terminal**, and that is not a shortcut. `Picker::
      from_query_stdio()` starts a thread that calls `enable_raw_mode`, reads the reply from stdin and
      then calls `disable_raw_mode` -- and when the terminal does not answer inside its one second,
      that thread is still running after Trak has re-enabled raw mode and switches it back off
      underneath. crossterm then sees canonical mode and **every single keypress is silently
      discarded**: the clock ticks, the frame redraws, nothing responds to anything. Found by
      running the TUI in a pty and logging `poll`, which returned `Ok(false)` for every key pressed,
      with the picker removed answering all of them. `TERM_PROGRAM=ghostty` and `KITTY_WINDOW_ID` are
      the signals, and anything unrecognised gets halfblocks -- so being wrong costs a blocky cover,
      not an unusable interface.
- [x] 4.2 **Accent colour from art.** `src/accent.rs`; the three `accent = art|green|terminal` modes
      are `Theme::accent_colour`. > **One measure throughout: WCAG relative luminance.** The first
      version filtered pixels by HSL lightness and fixed up the result by WCAG luminance, so colours
      the filter had approved came back as *different* colours. Lightness and luminance are different
      scales; using both is just wrong. > **The band is 0.10–0.60 luma**, because the terminal's own
      colours are unknown: below it an accent vanishes into a dark background, above it into a light
      one. Saturation must clear 0.20, because a grey accent is indistinguishable from no accent. > A
      **strongly** saturated candidate (≥ 0.45) beats a more central mild one: it is the colour a
      person would name if asked what colour the album is. > `is_usable` is the single gate the theme
      asks, and it also requires **3:1 contrast against whichever of black or white goes on top**,
      which is what makes the accent safe on a light terminal and a dark one at once. The selected tab
      and the toast take `accent_text()` rather than hardcoded black-on-cyan, which was unreadable on
      a dark cover. > **A monochrome cover is a real answer**: `None`, and green stays. A test that
      used to assert the opposite (that `Rgb(1,2,3)` is used as the accent) was the bug. > `set_art_colour`
      returns whether the colour *changed*, so an unchanged cover does not force a redraw; a changed one
      clears the terminal, because every cell that was the old colour has to be repainted. > The
      tab strip is now a styled `Line` rather than a string, since ratatui draws a block title over the
      border and the selected tab needs its own background. > `cargo run --release --example
      accent-probe` reports the accent for every cached cover, and why when there is none — "this
      sleeve got no colour" is otherwise impossible to tell from a bug. **All three real cached covers
      produce a usable accent**, and the live TUI's backgrounds contain the extracted colour.
- [x] 6.1 **(partly) The Lyrics tab is live.** Ahead of its turn in the list, because a tab that
      says "no lyrics yet" is worse than no tab. `src/lyrics.rs` fetches from LRCLIB on the worker and
      the tab shows the line being sung in the album's gradient with four lines of lead-in and a dozen
      ahead. > **The worker refused jobs silently**, so a lyrics lookup that lost the race against an
      art download was dropped while the status stayed `Loading` — every track said "looking for
      lyrics…" forever. `Worker::submit` now returns whether it was accepted, and a caller that has
      claimed a slot undoes the claim when the job is refused. > Lyrics are cleared on a track change
      and a lookup that lands after the user has skipped is dropped: showing the wrong chorus under a
      new title is worse than showing none. > Rest of phase 6 still owed: `L` for full-screen,
      translations, caching to disk, and the other providers.
- [x] 4.3 **Art / visualizer toggle plumbing** (`a`): `display.mode`. With `visualizer`, art is not
      drawn but is still fetched for colour. > The two share **one rectangle**, so `a` swaps what is
      drawn rather than what is laid out and the layout cannot jump. > The placeholder is not a
      "coming soon" box: it draws a row of bars sized from the interpolated position, so even the
      stand-in moves with the music instead of sitting there inert, and it names the style `v` would
      change. > `v` cycles the four styles of SPEC §8 and the choice is carried now, though 8.1 draws
      them. > Persisting to config lands with 5.x, which is what reads these.
- [x] 4.4 **System-volume fallback** (R2). `volume.control = "spotify" | "system"`, parsed and
      carried; the notice after an ignored write already names the setting. > On `system` the volume
      keys say **what they are about to change** rather than doing it silently: that volume belongs to
      the machine, not to Spotify, and silently turning down every sound on the Mac because someone
      pressed `-` is not a thing a music player should do behind your back. > Reading and writing the
      system volume is 5.x's job along with the rest of the config; the setting, the guard and the
      explanation exist and are tested, and 8.x proves the fallback against a real Spotify that
      ignores sets (which 1.3 says 1.3.1.234 does not).
- [x] 4.5 **Song-change notification.** `display notification` through osascript, off by default,
      title = track, body = `artist — album`. > **It fires on a change, never on the first read**:
      the event is raised from the notification path, which only runs when a track *changes*, so
      starting Trak cannot announce whatever happened to be playing. A test asserts both, including
      that the setting is off by default. > The body copes with every shape of a track: a promo with
      an artist but no album gets the artist, and one with neither gets an empty body rather than
      " — ". > A quote in a title is escaped, because the title is interpolated into an AppleScript
      string literal -- the same allow-list problem `play track` has. > It runs as a command on the
      worker: osascript is a process spawn and must never happen on the render thread.
- [x] 4.6 **Sonar state integration** per COMPAT. `src/sonar.rs` + `SONAR_STATE` in the environment.
      > The state file is re-read **every 2 s, not per frame**: it is a small file, the answer changes
      on the order of seconds, and a frame is 100 ms. Reading it faster would be a way of making the
      disk busy for no information. > **The whole point of this task is the volume guard, and it is
      not just the mute.** COMPAT rule 3 says Trak must never write a mid-fade volume, and a
      *relative* step (`-`) is computed from the **live** volume -- which mid-fade is Sonar's value --
      so pressing `-` during a duck would write Sonar's own number back and appear to undo the fade.
      `m`, `+`, `-` are all refused while ducking, and each says why: a key that silently stops
      working looks like a broken keyboard. > A duck that has just begun raises a toast once, and not
      again on every re-read, and the keys come back the moment it ends. > **`pid` in Sonar's file is
      *Spotify's* pid, not Sonar's** (per `docs/AGENT-PROMPTS.md`), which is the opposite of what the
      task text implies; Trak therefore vets the file against its own Spotify process, and a live
      Sonar makes an otherwise-stale file acceptable, because that is what COMPAT says. > The parser
      refuses an unknown *version* rather than guessing -- a file from the future could mean anything.
      26 tests cover valid, malformed, truncated, unknown-field, wrong-pid, stale and missing files;
      nothing in it can panic.
- [x] 4.7 **headless-spotify integration.** `src/headless.rs`. Run `status --json` once at startup
      on the worker, show a dim `headless` badge when Spotify is hidden, and give the footer a hint
      about opening a window that has no Dock icon. The idle card prefers `headless-spotify launch`.
      > **The subagent read the actual sibling repo, which exists locally, rather than guessing from
      the docs** -- and that changed the design. The real `status --json` prints **no `schema`
      field** and escapes slashes in paths, and it exits 1; both are now pinned fixtures. > An
      unknown `schema` is *read, not refused*, the opposite of Sonar: this decides a badge and a
      sentence, both of which have a safe absence, whereas Sonar decides whether Trak may write a
      volume. > `installed` in the sibling's JSON means *Spotify.app exists*, which Trak already knows
      from AppleScript; a field with that name sitting next to Trak's own "is the tool installed" is a
      trap, so it is ignored. > `is_installed` requires the **execute bit**, unlike the old `which`
      helper: a non-runnable file with the right name must not win COMPAT rule 2's one launch. The
      test asserts the exact argv of both launch branches, because a test that only checks the
      command exists cannot tell `-g -j` from a plain `open`.
- [x] 4.8 **Status line / toasts**: one bottom-right line, two and a half seconds, for permission
      denied, an ignored volume write, a failed lyrics lookup and a Sonar duck. > It is drawn one row
      **above** the footer, so a toast never covers the key hints -- the one thing a user needs to be
      able to read while a message is up. > Expiry is ticked locally and a test pushes the toast's
      timestamp into the past rather than sleeping.

---

## Phase 5 — Config and the settings screen

- [x] 5.1 **`config.rs`** per SPEC §8. Hand-written TOML subset, no new dependency. > **A wrong
      *type* on one key falls back to that key's default and leaves the rest of the file alone** — it
      does not make the file corrupt. Corrupt is a rename-the-whole-thing path a user has to recover
      from by hand, and one typo in one setting must not throw away the other forty. > **A leading
      BOM is stripped**: TextEdit writes one, and refusing the file would silently reset every setting
      the moment someone edited it in the obvious editor. > Unknown keys *and* unknown tables are
      ignored, so the file can gain keys before Trak knows them. > Save is atomic (temp + rename) and
      `0600`, and fixes the mode even over an existing `0644` file. > 77 tests, including a
      **round-trip with every key set to a non-default value** and a truncation sweep over all 1,378
      cut points of a real file. > `Paths` takes `HOME`/`XDG_CONFIG_HOME` as arguments, so no test can
      touch a real config. > `[volume] control` is in the file although SPEC §8's block does not show
      it: 4.4 pins it there, and without it the setting would be unreachable from disk. SPEC §8 wants
      that line added.
- [x] 5.2 **Settings screen** (`trak config` and `,` overlay). > **One screen, two ways in** — `,
      ` and `trak config` are the same code, so a setting cannot be editable in one and not the
      other. > The theme is rebuilt from the settings every frame rather than once at startup; that is
      the only reason the border and the accent can be previewed live. > **The Client ID field has no
      shape validation on purpose**: 7.3 owns that, and a validation rule written twice would be one
      of them wrong. > 35 tests, one of which parses the TOML block out of `docs/SPEC.md` at test
      time, so the check cannot pass by falling behind a copied list.
- [x] 5.3 **Wire every setting** to real behaviour. > `Settings` now carries a field for **every** key
      in SPEC §8, so wiring a setting is reading a field rather than threading a new argument through
      the renderer. The border moved from a `rounded: bool` to the real enum, because `double` and
      `none` are in the file too and a bool cannot say which was meant. > `show_progress` and
      `show_popularity` were being carried with **nothing reading them** — the exact failure a config
      layer grows quietly — so both are wired and asserted; popularity turns off the `▰▱` meter row
      and not the number, because a person who dislikes a bar still wants the fact. > `input.mouse`
      has no render effect by nature — it is decided once in `run()` — so it is covered there.
- [x] 5.4 **First-run experience**. > First run is **a fact to remember, not a state to model**: the
      loader already distinguishes a missing file from an empty one, and the hint is never saved — the
      config file appearing at all is what makes the next launch a *not*-first run. > The hint
      outranks the key list in the footer, because a user who has never run Trak does not know it has
      a settings screen and the key list cannot say so in the space it has.

---

## Phase 6 — Lyrics (both versions)

- [x] 6.1 **LRCLIB client** (`https://lrclib.net/api/get?...` by track/artist/album/duration; fall back
      to search). Read their docs first and set a descriptive `User-Agent: trak/<version>
      (https://github.com/Kathir-D/Trak)`. Timeouts, cache results on disk, never block the UI, no
      lyrics = clean empty state. Done when: tests with recorded JSON fixtures (no live network in CI).
      > Finished on top of the 4.x part-done pass: the disk cache lives next to the art cache
      > (`~/Library/Caches/trak/<fnv>.json`, 64 entries, temp+rename, zero-length and
      > non-deserialising files are misses, misses are never cached). Real replies from
      > 2026-10-01 are committed under `tests/fixtures/lyrics/` and read at compile time.
      > **A 404 from the search fallback is `NotFound`, not `Malformed`**: the miss body is a
      > JSON *object* (`TrackNotFound`), so without a status check in `search` a double miss came
      > out as "LRCLIB sent an answer Trak could not read" — a message that blames the service for
      > the song not being there.
- [x] 6.2 **LRC parser + "current line for position"** (handles `[mm:ss.xx]`, multiple stamps per line,
      offset tag, plain-text lyrics without stamps). Done when: unit tests incl. malformed input.
      > All four shapes live in `src/lyrics.rs`: multi-tag lines yield one line per tag, a bare tag
      > is a musical rest, unsynced lyrics get NaN times that `index_at` can never select, and
      > malformed lines are dropped rather than guessed at. **The `[offset:±ms]` tag is applied to
      > the whole file** (including lines before it): positive shifts the lyrics *earlier*, per the
      > LRC convention; a shifted-before-zero line is **clamped to 0.0, not dropped** — it is still
      > a line of the song; the last offset tag wins; an unparseable offset shifts nothing.
- [x] 6.3 **Lyrics tab**: current line highlighted and auto-scrolling, manual scroll pauses
      auto-follow for a few seconds. Done when: snapshot tests; manual sync check on a real song.
      > `j`/`k` and the mouse wheel take the scroll from the song for **4 s** (ticked down by
      > `Event::Tick`, so the resume is tested by ticking, never by sleeping), and the pane says
      > "following paused — it resumes on its own" while the hold lasts. `j`/`k` on this tab move
      > the words, not a history selection the tab cannot show; a click on a lyric row plays
      > nothing. The sung line keeps its gradient wherever it lands in the window, so scrolling
      > ahead still shows which line the song is on. **Real-song check (2026-10-01, "Beauty Sleep"
      > — Jane Remover):** the live lookup returned synced lyrics from `/api/get`, wrote the disk
      > cache, and the line-at-position resolved correctly at pos 48 s against the playing
      > Spotify. (Its screenshot was a whole-desktop capture and was removed on 2026-10-01; 11.2 re-shoots it window-only.)
- [x] 6.4 **Full-screen lyrics (`L`)**: large centered current line, dim neighbours, accent colour,
      `esc` returns. Done when: works at several sizes, no wrapping glitches (wrap long lines by
      display width, mind wide/CJK characters).
      > `wrap_by_width` wraps by display width (`unicode-width`, moved from dev-deps to deps and
      > re-recorded in THIRD-PARTY-NOTICES), so 40 CJK glyphs fill an 80-column row instead of
      > tearing — and a TestBackend test had to learn to walk skip cells to *measure* that, since
      > every wide glyph is followed by a pad cell that naïve counting reads as a third column.
      > The anchor line is centred, bold, in the accent colour; `L`/`esc` leave; transport keys
      > still answer while the page is up (the song does not stop being controllable because its
      > words fill the screen). Renders at every size from 100×30 down to 10×1 without panicking.
      > NOT_YET is now **empty** — every key SPEC §4 promises to both versions is bound — and the
      > 3.8 test that asserted the excuse list was non-empty was updated: empty is the finished
      > state, and the two render-side tests on the same list are what stop it meaning "the help
      > and the SPEC diverged". (Its screenshot was a whole-desktop capture and was removed on 2026-10-01.)

---

## Phase 7 — Version A: Web API

- [x] 7.1 **Confirm API reality (R4)** — done in 1.7; SPEC §6 and every task in this phase were
      rewritten against `docs/WEB-API.md` in the same commit. > The 7.2/7.3/7.5/7.7/7.8/7.10/7.11
      notes below carry the specific changes.
- [x] 7.2 **PKCE auth** (the real login is still **[owner]**): loopback listener on an **ephemeral** port; the registered redirect URI is
      `http://127.0.0.1` with **no port**, and the port actually used is sent in the request. Opens
      the browser with `open`, exchanges the code, refreshes tokens automatically. > **Refresh
      tokens expire after 6 months** (`docs/WEB-API.md` §6) — record the authorisation time locally,
      warn before expiry, and treat an invalid refresh token as "discard and re-login", not as an
      error state. Needs: 7.1. Done when: unit tests with a mock token endpoint; **[owner]** completes
      a real login once.
- [x] 7.3 **Guided setup in `trak config`**: screen with numbered steps — open
      `https://developer.spotify.com/dashboard` (via `open`), create an app, add the exact redirect URI
      shown (copyable), paste the Client ID (validate shape), press enter → browser login → success
      screen. `Log out` clears the token. > **The redirect URI to display and copy is exactly
      `http://127.0.0.1` — no port, no path, and never `localhost`** (Spotify bans `localhost`, and
      dynamic ports are explicitly allowed only for loopback IP literals). Confirm during the first
      real login whether the dashboard accepts the no-path form; fall back to a fixed
      `http://127.0.0.1:<port>/callback` if not. Done when: every step has an on-screen explanation
      and errors (bad ID, denied consent, port busy) are handled with retry. **[owner]** walks
      through it once.
      > Done: `s` in the settings screen (heading `Spotify API` says so) opens `tui/setup.rs`, a
      > pure state machine (`Setup::handle` -> `Effect`s) that `loop_.rs`'s `SetupRunner` carries
      > out: open the dashboard / copy the URI (`pbcopy`) / save the config / log in on its own
      > thread / delete the token. Client ID is validated as 32 hex digits and a bad paste stays in
      > the input with the reason; a failed login (denied, port busy, timeout) shows the notice and
      > `enter` retries; a second login cannot start while one waits. **Found while wiring it:
      > nothing set `app.web.connection` at startup**, so no Web API tab could ever have loaded;
      > `connection_at_start` now derives it from the config and token file. The TUI also
      > never refreshed a stale *access* token, so a login would have died after ~1 h:
      > `web_client()` (worker only) now renews and saves it via `Session::access`, and the UI
      > thread asks the no-network `web_ready()` instead. `esc` while the browser is open closes the panel but
      > cannot cancel the wait; it ends at the login timeout. **[owner]** walk through it once.
      > **Checked in real cmux (2026-10-01, 100×40 and a 46-column split):** all four steps render
      > and the copy/open keys work (enter on step 1 opened the dashboard). **Bug found and fixed:**
      > `Paragraph`'s own wrap started a continued explanation flush against the panel's left border,
      > and cmux then lost the border cells on exactly those rows; explanations and titles are now
      > word-wrapped by display width with a hanging indent and the URI is its own line
      > (`explanations_wrap_at_words_and_never_reach_the_border`, which failed before the fix). **Still
      > [owner]:** no `~/.config/trak/token.json` exists and the developer site is not signed in on
      > this Mac, so the login itself, whether the dashboard accepts `http://127.0.0.1` with no port,
      > and every live Web API check (search, `f`, `A`, `P`/`n`/`X`, paging, `trak play`) wait on it.
      > The fixed-port fallback is not built: it is only needed if the dashboard refuses the no-port
      > form, and nobody has seen that happen.
- [x] 7.4 **Token storage**: the `0600` file decided in 1.8 (`~/.config/trak/token.json`, `XDG_CONFIG_HOME`
      respected, directory `0700`, atomic temp-then-rename write). **Refuse to read a token whose mode
      is looser than `0600`** rather than proceeding, and assert that in a test. Never logged, never in
      `config.toml`. Keep the `Store` trait so tests can fake it, but there is only **one** real
      backend — do not build a Keychain one (it hangs, see 1.8). Documented in the README.
- [x] 7.5 **`Library` trait + `ureq` client + `FakeLibrary`.** > **rspotify rejected**: it is
      async and would drag `tokio` into a single-threaded TUI for no gain, and the only thing 7.5
      wanted from it — the id-list helpers — calls endpoints removed in dev mode. `ureq` plus a
      hand-written trait instead. > The `0600` write is copied line for line from `config.rs`'s, so
      there is one atomic-write-and-tighten implementation rather than two. > **`Debug` redacts** on
      every type that can hold a token: a `Debug` impl is how a token ends up in a log. > Six-month
      refresh window: 183 days, **long is the safe direction**; a 14-day warning lead is mine, not
      Spotify's; the local clock warns and **the server decides** — a six-month-old refresh token is
      still offered, because a fast local clock must not cost a login; `authorized_at` never moves on
      refresh, which is what keeps the reconnect state reachable at all; any 4xx on refresh is spent,
      5xx is not. > A test asserts no error `notice()` contains the token string. > **Do not use `rspotify`'s
      id-list helpers** (`tracks(ids)`, `artists(ids)`, `albums(ids)`) — the batch endpoints they
      call were removed in dev mode. Loop one id per request and cache hard; the quota is per
      developer account and shared across Client IDs. 429: **only `Retry-After` is documented and
      there is no `X-RateLimit-*` header or quota endpoint**, so back off with a cap and show a
      one-liner; distinguish `"reason": "QUOTA_EXCEEDED"` in the body. 403: two real causes — Premium
      (queue) and an account not on the app's 5-user allowlist. Map both to friendly typed errors.
      Needs: 7.1. Done when: fixture-driven tests, no live network.
- [x] 7.6 **Search tab**: `/` focuses the input, live results debounced (~250 ms), grouped Tracks /
      Albums / Artists / Playlists, `Tab` jumps groups, `enter` plays (AppleScript `play track "<uri>"`,
      so it works on Free), `A` queues, `o` opens the artist/album page. Done when: update() tests
      with `FakeLibrary`; snapshot tests; stale responses never overwrite newer ones.
      > The tab, keys, grouping and stale-response drop were already built; **the live search
      > itself never fired** — `search_debounce` was reset on keys but nothing advanced it. `Event::Tick`
      > now counts it (only while the box has focus, connected, not already searching, and the
      > query is neither blank nor the one on screen) and the loop sends the job (it used to
      > discard a tick's `web` jobs). `[`/`]` jump groups, not `Tab` (SPEC §4). **[owner]**: type in
      > Search with a real login and watch results appear ~0.3 s after you pause.
- [x] 7.7 **Playlists tab** (list, open, play, tracklist) and **Liked tab** (list, play from here,
      `f` toggles like on the current track). > Field rename: playlist `tracks` → **`items`**
      (`items.items.item`), and `items` is **only present for playlists the user owns or collaborates
      on** — no feature may promise to show any playlist's tracks. > `f` is `PUT`/`DELETE
      `/me/library` and the liked check is `GET /me/library/contains`, not `/me/tracks`. > **Playlist
      item read/write is unverified in dev mode** (docs contradict themselves) — confirm here with a
      real login and degrade cleanly if it 403s. Done when: tests + manual.
      > Lists, open, play and `f` were built; **nothing ever asked the server whether the playing
      > track is liked**, so `liked_here` stayed blank and `f` could only ever send a like. The
      > loop now asks once per track (`WebState::next_liked_check` -> `WebJob::IsLiked` ->
      > `Event::LikedHere`, dropped if the track changed) and re-asks after any landed write.
      > **[owner]** with a real login: confirm the heart matches the app, and whether playlist
      > items 403 in dev mode (the tab must degrade, not crash).
- [x] 7.8 **Queue tab** (now playing + up next) and add-to-queue (`A`). > `POST /me/player/queue`
      is **Premium-only by Spotify's own documentation**; `GET /me/player/queue` is not. A 403 on add
      is the expected Free-tier path, not an error. `User.product` no longer exists, so Premium
      cannot be detected up front — rely on the 403. Done when: works or shows the clear Premium
      message; tests.
      > Audited: tab, `A`, lazy load and the typed Premium 403 (`ApiError::PremiumOnly`) were all
      > built and tested. Added: a landed write empties the cached queue so the tab refetches on
      > its next visit and shows the track just added. **[owner]**: add to queue on Free to see
      > the Premium line.
- [x] 7.9 **Library tab**: saved albums, followed artists, recently played. Done when: paginated
      lists load lazily; tests.
      > The three Library lists loaded their first page and stopped: **every fetch passed `None`
      > for the continuation**, so "more ↓" was drawn and unreachable, for Playlists and Liked as
      > well. `WebState::next_more` now asks for the next page when the cursor is within
      > `MORE_AHEAD` (5) rows of the end of the list on screen (`WebJob::More` ->
      > `Event::MorePage`, appended). A failed page gives up on the rest with a toast rather than
      > retrying every frame; a not-sent page unmarks itself. Also fixed on the way: `submit_web`'s
      > no-op fallback was `WebWrote(Ok)`, which now re-asks the like check and would have looped on
      > a failing client — it is `Resize` now. Opened pages (playlist items, artist albums, album
      > tracks) are still one request each.
- [x] 7.10 **Artist page** and **album page** (tracklist, play from track). Back with `esc`. >
      **Albums only — `GET /artists/{id}/top-tracks` was removed in dev mode with no replacement**, so
      the top-tracks half of this task is dead. `GET /artists/{id}/albums` still works. SPEC §6 is
      updated. Done when: navigation stack tests; manual.
      > Audited: `o` opens, `esc` walks the stack (`escape_walks_the_navigation_stack`), `enter` on
      > a track row plays it (`PlayUri`); an artist page lists albums only. Nothing to add.
- [x] 7.11 **Playlist editing**: add current/selected track to a playlist (picker), remove from a
      playlist, create playlist. > Use `POST`/`DELETE /me/playlists/{id}/items` (the `/tracks`
      variants are removed), and `POST /me/playlists` to create. Same unverified-in-dev-mode caveat as
      7.7. Done when: confirmation on destructive actions; tests with fakes.
      > The three write jobs and the client calls existed; **no key reached them**. Now: `P` opens
      > a picker (`PlaylistEdit::Pick`) for the selected track, or the playing one when the cursor
      > is not on a track, and fetches the playlist list if it was never loaded; `n` in the picker
      > names and creates a private playlist; `X` in an open playlist asks `y`/`n` before removing
      > (the row leaves the list at once, a failed write toasts). The modal owns the keyboard, so
      > `space`/`n`/`p` there are answers, not transport keys. A playlist with no `items` (not
      > yours) is shown read-only and refused with a toast rather than sent to 403. Keys are in
      > SPEC §4 and the `?` overlay (its height clamp went 24 -> 28 to fit two rows). **[owner]**
      > with a real login: add, create and remove once, and report whether dev mode 403s the item
      > writes (the caveat is unchanged).
- [x] 7.12 **`trak play <song|album|artist|list>`** (finish 2.7) using search; pick the best match
      like shpotify; print what it chose. Done when: `assert_cmd` tests with `FakeLibrary`.
      > The pick is shpotify's first-result rule plus one tiebreak: the first row whose
      > **name** contains the whole query, case-insensitively — so `trak play mezzanine` plays
      > the *song* Mezzanine, not whichever track from the album Spotify ranked first. The rule
      > is written on `web::api::best_match`; `trak play uri <uri>` was added too (SPEC §9,
      > shpotify's own spelling). The "Playing …" line prints only **after** the play landed,
      > so trak never claims to be playing something Spotify refused. The setup path is still
      > what every real machine sees today (no token file anywhere yet): Client ID steps, exit 2.
      > `--fake` alone deliberately still means "no library", so that path stayed testable;
      > `--fake-library` installs `FakeLibrary::seeded()` — the api.rs Massive Attack catalogue
      > plus the song "Mezzanine", which is what exercises the tiebreak end to end. No refresh
      > on the CLI path: a stale access token surfaces as the search's own "log in again"
      > (`ApiError::Unauthorized`), because renewing is 7.3's login flow, not this task's.
      > Two deliberate departures from shpotify, both because a command that reports its pick
      > cannot report a coin flip: `play list` was a random row out of ten, and `play uri` was
      > not silent (`Playing Spotify URI: …`). The setup message quotes back the spelling that
      > was typed, so `trak play album x` is not told to run `trak play "x"`.
- [x] 7.13 **Tab order and default tab for A**, plus the B-mode hint that a Client ID unlocks these.
      Done when: SPEC §3 matches the built UI (update the doc if the order changed).
      > Built order already matched the SPEC's list (`Tab::ALL`, digits 1-6, History/Info via
      > `Tab`, `default_tab` = history). What differed was the *B-mode* wording: SPEC said B has
      > three tabs, but the build draws one strip of eight and the Web tabs carry the Client ID
      > notice. SPEC §3 now says what is built, and why (one strip, one meaning for `1`-`6`).

---

## Phase 8 — Visualizer

- [x] 8.1 **`AudioSource` trait + simulated source**. > **Simulated is the shippable default, not
      a placeholder**: 1.5 never found a process-only tap, so 8.3 is blocked and something still has to
      draw. > `Send` but deliberately **not** `Sync` — the source is owned by its own worker thread, and
      requiring `Sync` would commit 8.3 to locking a ring buffer it has no reason to lock. > 28 tests,
      deterministic from a seed. > Decay is driven by **call count, not wall-clock** — the trait has to
      stay clock-free to be testable — and 8.4's 30 fps cap turns that into a real 1.3 s fade. > One
      file rather than the `viz/{source,dsp,render}.rs` in ARCHITECTURE.md: there is no dsp layer to
      separate out until 8.3 lands a real second implementation, so ARCHITECTURE.md is corrected in the
      same commit.
- [x] 8.2 **Renderers** (pure fns)**: `spectrum`, `mirrored`, `waveform` (braille), `circular`. All
      are `fn(&[f32], width, height) -> Vec<String>` — no I/O, no terminal, no panics at any size
      including 0 and 1. `BARS = 32`, matching what 1.6 chose for `cavacore`, so the simulated and the
      real source draw the same shape. > Bars are **one cell wide per character**: TODO 3.4 recorded
      that a wide glyph in a bar tears the layout, and every returned line is asserted to be exactly
      the requested width. > Braille earns its keep: `circular`, walking each band along its own ray,
      put a dot on the centre line where no cell can hold one at an even width, so it works cell-first
      and a flat spectrum is now exactly symmetric at both parities. The symmetry test found that, not
      looking at it did. > The tests assert a flat spectrum looks flat and a peak looks like a peak — a
      renderer that only does not panic is a renderer that is wrong. > Not borrowed from scope-tui; the
      braille dot maths is written from the Unicode 2x4 grid, so THIRD-PARTY-NOTICES is unchanged.
- [x] 8.3 **Real audio source** from the spike: tap Spotify's process, mono-mix, ring buffer, cavacore
      → bars, on its own thread; auto-fallback to simulated on denial/error with a one-time toast that
      says how to grant permission. `source = simulated` forces the fallback. Needs: 1.5, 1.6.
      Done when: bars visibly track the music on the owner's machine; denial path tested manually;
      CPU stays low (record % in the PR/commit).
      > Merged 2026-10-01 from `wt/8.3-tap`. Taps **Spotify's process only** (pids → process objects
      > via `'id2p'`) through cidre's `TapDesc`; no objc2 bypass, no Swift rpath (`otool -L` clean).
      > **cavacore dropped**: 2.0.2 truncates every band's cut-off to FFT bin 0 (AUDIO-TAP §2f), so
      > `src/audio.rs` has its own radix-2 FFT and log band map. Measured with
      > `cargo run --release --example tap-probe -- run 12`: bars move with the music (per-second
      > band means 0.17–0.25) and read exactly zero when paused — a paused Spotify stops the IO proc
      > entirely, so three empty ticks count as silence. TUI CPU in a 110×34 pty over 30 s: **3.8 %
      > real audio vs 3.2 % simulated**. Denial falls back to simulated with **one** toast per run
      > naming *Screen & System Audio Recording* (fake-tested,
      > `a_denied_tap_falls_back_to_simulated_bars_and_says_so_once`); **a real denial could not be
      > provoked**, because this Mac grants the tap with no prompt at all (R1). `source = "simulated"`
      > never taps, and switching to it live lets go at once. MSRV went 1.85 → 1.88: main already
      > used let-chains, so 1.85 was wrong before this. **[owner]**: watch the bars in cmux by eye.
- [x] 8.4 **`v` cycles styles**, `a` toggles art/visualizer, both persist to config; visualizer FPS
      capped (~30) and paused when the terminal is hidden/too small. > The four renderers are pure
      functions over a spectrum and hand back plain text; **the colour is applied per column in the
      renderer, off the album's own ramp**. That split is what makes a visualizer tinted by the cover
      possible without four renderers knowing anything about palettes. > Bars are one cell wide per
      character, and the colouring walks `chars()` rather than bytes — a braille glyph is one char but
      three bytes, and walking bytes colours the wrong column. > **30 fps only while the visualizer is
      on screen**; the rest of the time the loop waits its usual 100 ms, because 10 fps is plenty for a
      dashboard and is a tenth of the wake-ups (TODO 11.5's idle-CPU budget). The frame tick is a
      separate `Event` from the one-second clock, so 8.5's tap can start and stop without the clock
      noticing. > `a` and `v` mark the config dirty and the write happens on the way out, so holding
      `v` down is not thirty writes a second, and a **failed save is a line of text after the terminal
      is restored** rather than an escape sequence. > Not paused for a hidden terminal: ratatui has no
      way to know the window is occluded, so 8.5's answer is that the tap stops, not that the frames
      do.
- [x] 8.5 **Tap lifecycle**: start on demand, stop when the visualizer is hidden or Spotify quits,
      reattach when Spotify restarts, never leave a tap/aggregate device behind after exit or crash
      (RAII + signal handling; verify with `system_profiler SPAudioDataType` before/after).
      Done when: 10 start/stop cycles leave no leftover devices.
      > The **loop, not `App`, owns `AudioPipeline`** (a thread and a Core Audio device do not belong
      > in a state that is cloned and compared); it sends `Event::LiveSpectrum` / `Event::TapNotice`.
      > `set_wanted(visualizer_visible)` runs every frame, so the tap attaches on show and releases
      > within ~20 ms on hide or when Spotify goes idle. A live tap checks its pids with
      > `kill(pid, 0)` every 1 s and retries every 2 s, so it reattaches by itself after a relaunch
      > (lifecycle tested against a fake backend; **quit/relaunch of the real Spotify was not done**,
      > because an agent must not launch Spotify). `tap-probe cycles 10`: `system_profiler` 3 → 3
      > devices, byte-identical dumps; that alone is weak because the aggregate device is private,
      > so the probe also counts Core Audio's lists in-process: 3 devices/0 taps → 4/1 with a tap up
      > → 3/0 after 10 cycles. SIGTERM/SIGHUP drop the tap and re-raise (exit 143 in 0.1 s). SIGKILL
      > or a panic (release is `panic = "abort"`) cannot clean up in-process; that relies on the tap
      > and device being private to the process (AUDIO-TAP §4c) — after a `kill -9` Spotify kept
      > playing and a fresh tap came up normally. **[owner]**: with the visualizer showing, quit and
      > relaunch Spotify and see the bars come back within ~2 s.

---

## Phase 9 — Release and Homebrew (no paid signing)

- [x] 9.1 **`scripts/package-release.sh`**: build both targets (`rustup target add x86_64-apple-darwin`),
      `lipo` into a universal binary, `codesign --force -s -` (ad-hoc), tarball
      `trak-<version>-macos.tar.gz` containing `trak`, `LICENSE`, `README.md`, `THIRD-PARTY-NOTICES.md`,
      plus `SHA256SUMS.txt`. VERSION is the source of truth; the tag must be `v$(cat VERSION)`.
      Done when: script runs locally; `file` shows both archs; `codesign -dv` shows ad-hoc; the binary
      runs on arm64 with `--version`; `xattr` shows no quarantine after `curl` download.
      Alternative if lipo/sign misbehaves: two tarballs + `on_arm`/`on_intel` (see ARCHITECTURE).
- [x] 9.2 **`.github/workflows/release.yml`** on `v*` tags: verify tag == VERSION, build, package,
      create the GitHub release with tarball + checksums, then update `Formula/Trak.rb` in
      `Kathir-D/homebrew-tap` (needs a `HOMEBREW_TAP_TOKEN` secret). Model it on
      `../headless-spotify/.github/workflows/release.yml`. Done when: a dry-run on a pre-release tag
      produces a release and a tap commit. **[owner]** adds the secret (fine-grained PAT, contents:write
      on the tap repo only).
- [x] 9.3 **`Formula/Trak.rb`**: `desc`, `homepage`, `url`, `sha256`, `license "MIT"`,
      `depends_on macos: :sonoma` (14.x; the audio tap needs 14.2 — note this in `caveats` since Homebrew
      cannot express minor versions), `def install; bin.install "Trak"; end`, caveats (optional Client
      ID via `trak config`, terminal permissions, optional Sonar / headless-spotify), `test do`
      asserting `trak --version` matches. No `system "codesign"`, no quarantine hacks: Homebrew formula
      downloads are not quarantined. Done when: `brew install --formula ./Formula/Trak.rb` works from a
      local tap and `brew test Trak` passes.
- [ ] 9.4 **Add the row to the tap README** (`brew install kathir-d/tap/Trak`, uninstall, description)
      — or run prompt 3 in `docs/AGENT-PROMPTS.md`. Done when: the tap README lists Trak.
- [ ] 9.5 **`brew audit --strict --online kathir-d/tap/Trak`** clean; `brew style`. Done when: both pass.
- [ ] 9.6 **Fresh-machine test**: `brew install kathir-d/tap/Trak` on a clean user/VM (or after
      `brew uninstall`), run `trak --version`, `trak status`, open the TUI. Done when: no Gatekeeper
      prompt, no manual steps. Record in `docs/RELEASING.md`.
- [x] 9.7 **`docs/RELEASING.md`**: bump VERSION and Cargo.toml, changelog entry, tag, what CI does,
      how to yank a bad release. Add `CHANGELOG.md` (Keep a Changelog format).
- [ ] 9.8 Tag **v0.1.0** when phases 2–8 are done and phase 10 rows pass. **[owner]** approves the
      release before tagging.
      > **CHANGED (owner override, 2026-10-01):** the gate is relaxed to *phases 2–7 and 9 pass*.
      > 8.3/8.5 (real audio) and phase 10 (Sonar matrix) may still be open; if 8.3 is not merged the
      > simulated visualizer ships and the README and CHANGELOG say so. A half-working 8.3 is never
      > merged. The agent may tag and push `v0.1.0` itself once every pre-tag check passes (gate +
      > CI green, `package-release.sh`, `brew audit`/`brew style`, `TAP_DEPLOY_KEY` present, README /
      > CHANGELOG / RELEASING true for the release). If the workflow fails after a tag is published,
      > fix it and cut `v0.1.1`; never move a published tag.
- [ ] 9.9 **curl installer** (`install.sh` at the repo root; owner override 2026-10-01, a second
      install path beside Homebrew). POSIX sh, shellcheck-clean. Refuses anything but macOS 14.2+;
      version is `$TRAK_VERSION` or the latest release; downloads the tarball and `SHA256SUMS.txt`
      into a `mktemp -d` (trap-cleaned), verifies with `shasum -a 256 -c` and aborts on a mismatch;
      installs `trak` to `$TRAK_INSTALL_DIR`, else `/usr/local/bin` if writable, else `~/.local/bin`
      (with a PATH hint); never sudo, never touches quarantine, never launches Spotify;
      `--uninstall` removes exactly what it installed; `--help`; prints `trak --version` at the end.
      Needs: 9.1. Done when: tested end to end against a local `file://` mirror of a packaged release
      **and** against the real release after tagging; documented in SPEC §9, `docs/RELEASING.md`
      and the README (Homebrew first, curl second).
      > `tests/install_sh.rs` runs the real script against a `file://` mirror of a stand-in tarball
      > packed the way `package-release.sh` packs it: install + version, forged checksum (nothing
      > installed, no receipt), a sums file without the tarball's line, a missing release, macOS
      > 14.1/13.6.1 refused before any download and 14.2/15.0 accepted (a `sw_vers` shim on PATH),
      > uninstall leaves a neighbour file and `~/.config/trak` alone and is a no-op the second time,
      > a Homebrew Cellar symlink is never overwritten, `--help` and an unknown flag (exit 2).
      > `--uninstall` works from a **receipt** (`${XDG_DATA_HOME:-~/.local/share}/trak/install-sh-receipt`)
      > rather than by guessing paths, which is what "exactly what it installed" means: it can never
      > remove a Homebrew trak or one a user copied in by hand.

---

## Phase 10 — Compatibility verification with Sonar and headless-spotify

Run each row of the table in `docs/COMPAT.md`, tick it there with the date and what was observed.
Requires Sonar installed and running (`brew install --cask kathir-d/tap/sonar`) and the real Spotify.

- [ ] 10.1 Trak ↔ Sonar skip/prev both directions (< ~1 s propagation).
      > 2026-10-01: not observable unattended. Sonar 0.1.3 launched, but its status item exposes no
      > title to Accessibility and its popover does not open from System Events, so there is no way
      > to read what Sonar shows. Trak's half is measured (3.9: a skip from any source reaches Trak
      > in ~173 ms through the notification). **[owner]**: skip in Trak, watch Sonar's title; skip in
      > Sonar, watch Trak.
- [ ] 10.2 Ducking: Trak shows the Sonar badge (needs Sonar prompt 1) and stays consistent; the volume
      meter is not corrupted; `m` is disabled during a duck.
      > 2026-10-01: blocked here. Sonar's Auto-Pause has never been switched on on this Mac (no saved
      > settings) and needs *Screen & System Audio Recording* plus Automation for Sonar, which only a
      > person can grant. The Trak side is covered by 4.6's 26 state-file tests. **[owner]**.
- [ ] 10.3 Ownership: pausing in Trak during/after a duck never gets undone by Sonar.
      > Blocked like 10.2. Note: Sonar's own README lists as a *known limitation* that a pause sent
      > by another tool **while `ducked`** is invisible to Sonar, which will still resume. So the
      > "during" half is expected to fail on Sonar's side, not Trak's; check "after" separately.
- [x] 10.4 Spotify quit / relaunch while both are running.
      > 2026-10-01, Trak only (Sonar could not be driven, see 10.1): quit → idle card, no relaunch in
      > 20 s; `enter` → relaunch → Trak reattached on its own. The relaunch hit a Spotify admin
      > password dialog (bundle owned by another user; COMPAT "Status"); Trak timed out politely
      > throughout. Found: the quit/relaunch adds the same track to History a second time (Backlog).
- [x] 10.5 Without Sonar and without headless-spotify installed: zero errors, zero warnings.
      > 2026-10-01: `PATH=/usr/bin:/bin SONAR_STATE=/nonexistent`: TUI and `trak status` clean.
- [ ] 10.6 Headless Spotify: badge + full control, **when Spotify honours LSUIElement again (R7)**;
      until then leave unchecked and say so.
      > 2026-10-01: still R7 — `headless-spotify status --json` reports `"headless":false`.
- [ ] 10.7 Visualizer taps concurrently with Sonar's tap without either failing.
      > Blocked like 10.2 (Sonar's tap only runs with Auto-Pause on). Both are private process taps
      > on disjoint process sets (Sonar excludes Spotify; Trak includes only Spotify), so no conflict
      > is expected — reasoned, not observed. **[owner]**.

---

## Phase 11 — README, docs, polish

- [x] 11.0 **Real-terminal key and mouse sweep** (owner request, 2026-10-01/02). Every key in the
      Keys panel and every click target, in cmux against real Spotify, watching what AppleScript
      was sent (a logging `TRAK_OSASCRIPT` wrapper) and what Spotify read back.
      > Bugs it found, all fixed with tests: commands dropped while a poll was in flight (and
      > `busy` stuck, so every key died); `h`/`l` sent the step as an absolute position; repeat
      > desynced and a poll reset "one"; shift-tab (crossterm `BackTab`) unmapped; overlay
      > remnants over the Kitty cover (`terminal.clear()` when an overlay opens/closes);
      > clicks went through the settings screen; a volume *drag* stopped at its first step (and
      > the loop dropped commands a finished write returned); Web API keys off their tabs were
      > silent. Owner decisions in the same pass: `←`/`→` switch tabs, `h`/`l` seek, the help
      > overlay is folded into the settings screen (`?` and `,` both open it, Keys panel beside
      > it). **Tooling:** `cliclick` drops keys in cmux and System Events hangs, so keys were
      > typed by a tiny Swift CGEvent keycode typer; clicks via `cliclick` work. Not covered: the
      > idle card (needs Spotify quit, which would interrupt the owner).

- [ ] 11.1 **Refactor the README** using https://github.com/abhisheknaiidu/awesome-github-profile-readme
      as the style reference. Note: that repo is a *curated list of GitHub profile READMEs*, not a
      template, and profile READMEs are not project READMEs, so **borrow the presentation ideas, not
      the structure blindly**:
      1. Open the repo's categories (**Minimalistic ✨, Descriptive 🗒, Badges 🎫, Icons 🎯, GIFS 👻,
         Dynamic Realtime 💫**) and study 6–8 linked READMEs across them. Also study the sibling
         READMEs in `../Sonar/README.md` and `../headless-spotify/README.md` for the owner's house style.
         Write 5–10 bullet "what works" notes in `docs/README-NOTES.md` (link + the idea).
      2. Produce: a centered hero (logo/wordmark + one-line pitch), a badge row (CI, release, macOS,
         Rust, license, Homebrew), a **demo GIF/screenshot right under the hero** (record with
         [vhs](https://github.com/charmbracelet/vhs) or a screen recording; commit under `docs/images/`;
         keep it < ~3 MB), a "Why Trak" three-line pitch, feature list with icons, install (Homebrew
         first), a quick usage table, a clear **Version A vs Version B** comparison table, the
         `trak config` settings table, keybindings, "Works with Sonar & headless-spotify" (link to
         COMPAT), troubleshooting (permissions!), credits (shpotify, spotify-tui inspiration,
         cava/cavacore, ratatui, LRCLIB), license.
      3. Keep it scannable: contents list, collapsible `<details>` for long tables, alt text on every
         image, every claim true for the released version (no features that are not built).
      Needs: phases 3–8 built enough to screenshot. Done when: the README renders well on GitHub in
      light and dark mode (check both), all links work (`lychee` or manual), and an outsider can
      install and use Trak from it alone.
      > **Partly done (left unticked).** README rewritten around the shipped product: status note
      > that names what is *unverified* (Web API live, real-audio visualizer), contents list, Why
      > Trak, feature table, install (source only: no formula exists), TUI key table, A vs B table,
      > settings in a `<details>`, `NO_COLOR`, CLI reference kept, permissions, Sonar/headless,
      > credits; relative links checked. **Not done:** the hero demo GIF (needs a real terminal),
      > release/Homebrew badges (no release yet), `docs/README-NOTES.md` (it asks for a study of
      > 6–8 external READMEs, which was not done), and the light/dark GitHub render check.
      > **Privacy (2026-10-01):** `tui-6.3-tab.png` and `tui-6.4-fullscreen.png` were whole-desktop
      > captures (other windows, an API-key page) and are removed from the tree. They are still in
      > git history; purging that needs a force-push, which is the owner's call. Every capture
      > from now on is the cmux window only.
      > **2026-10-02:** hero GIF done (`docs/images/trak-demo.gif`, 1.3 MB, see 11.2),
      > `docs/README-NOTES.md` written (Sonar, headless-spotify, spotify-tui, spotify-player,
      > ratatui and six profile READMEs), and the GitHub render checked in both colour schemes
      > with headless Chrome (`--blink-settings=preferredColorScheme=0/1`): the dark-terminal GIF
      > reads well on both. Left: release and Homebrew badges, added with the v0.1.0 release.
- [ ] 11.2 Record the demo GIF/screenshots: Version B, Version A, visualizer styles, settings screen,
      full-screen lyrics. Done when: images committed and referenced.
      > Version B, three visualizer styles, full-screen lyrics and settings are recorded and in
      > the README's Screenshots section (recording method in `docs/images/NOTES.md`). Version A
      > needs a live login **[owner]**; left unticked for that alone.
- [ ] 11.3 GitHub repo polish: description, topics (`spotify`, `tui`, `rust`, `ratatui`, `macos`,
      `homebrew`, `terminal`), social preview image, `CONTRIBUTING.md`, issue templates,
      `SECURITY.md` (token handling note), `CODE_OF_CONDUCT.md` optional.
      > Files written: `CONTRIBUTING.md`, `SECURITY.md` (token handling, checked against
      > `web/token.rs` and `web/auth.rs`), `.github/ISSUE_TEMPLATE/{bug_report,feature_request}.md`
      > and `config.yml`. **[owner]** (needs repo admin, not available to an agent session): set the
      > description and topics (`spotify`, `tui`, `rust`, `ratatui`, `macos`, `homebrew`,
      > `terminal`), upload the social preview, enable *private vulnerability reporting* so the
      > SECURITY.md link works. Left unticked until those are done.
- [ ] 11.4 Add a "Works with Trak" mention in Sonar's and headless-spotify's READMEs (their agents do
      this via prompts 1 and 2). Confirm the three READMEs cross-link.
- [x] 11.5 Performance and battery pass: idle CPU < 1 % with the visualizer off and < ~5 % on;
      wake-ups minimised when paused; measure and record.
      > Measured 2026-10-02 in cmux, release build, Kitty art on, `top -l 31 -s 1` averages:
      > **paused 0.87 %** (was 1.38 %; a fake `TRAK_OSASCRIPT` answering with the
      > `paused_track.txt` fixture, so the owner's music was not stopped), **playing 0.95 %**,
      > **visualizer on 3.85 %** (real Spotify, `waveform`). Two fixes found by `sample` on a
      > debug-symbol build: the header clock spawned `date` every second *on the UI thread*
      > (now `localtime_r`), and a paused screen still wrote ~15 KB/s because the Kitty cover's
      > placeholder row overshoots `unicode-width` and ratatui's diff rewrites the cells after it
      > every frame (`draw_if_changed` skips a frame identical to the last one sent; now 0 B/s).
      > What remains while paused is the 100 ms render into memory and the 5 s poll. Not measured:
      > battery drain over hours, which needs the owner's laptop on battery **[owner]**.
- [ ] 11.6 Accessibility / robustness pass: works with `NO_COLOR`, 16-colour terminals, light
      themes, very small and very large terminals, non-ASCII titles, right-to-left text does not break
      layout.
      > Done in code: `NO_COLOR` (CLI -> plain; TUI strips fg/bg per frame, keeps reverse/bold),
      > RGB downgraded to 256/16 colours by `COLORTERM`/`TERM` (`tui/colour.rs`, applied once on the
      > finished buffer, so no widget knows), a draw sweep over CJK / RTL / emoji / combining / 500-char
      > titles x every tab x 1x1..250x70 never panics, and the setup panel has the same too-small floor
      > as the checklist. **Not verifiable here — [owner]:** a *light-background* theme (dim text may
      > be hard to read), real RTL rendering order in cmux, and `NO_COLOR=1 trak` in a real terminal.
      > Kept open until those are looked at.
- [ ] 11.7 Final read-through of `AGENTS.md`, `CLAUDE.md`, and the docs so they match the shipped
      product; remove stale TODOs.

---

## Backlog (not committed to; do not start without the owner)

- Rebindable keys.
- History: a Spotify quit and relaunch on the same track records that track a second time (10.4).
- Idle card: when Spotify is running but every Apple Event times out (e.g. stuck on an admin
  dialog), say "Spotify is not answering" rather than "isn't running" (COMPAT "Status").
