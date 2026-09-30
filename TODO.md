# trak — TODO

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
| R3 | ~~Keychain items created by an ad-hoc-signed binary can re-prompt after every upgrade~~ **CONFIRMED AND WORSE (1.8):** a keychain item is readable only by the exact binary that created it. A *new* identity raises a dialog and blocks (first encounter); a repeat of the *same* identity errors immediately from cache. Every release is a new identity, so trak always gets the blocking case — a **hang, not a prompt** | **Resolved: use a `0600` file** (`docs/KEYCHAIN.md`). SPEC §2 and §6 updated. 7.4 drops the second backend |
| R4 | ~~Spotify Web API developer-mode rules changed recently~~ **CONFIRMED AND WORSE THAN EXPECTED (1.7):** `localhost` redirect URIs are **banned** (use `http://127.0.0.1`, no port); Premium now required of the **app owner**; user cap **5**; all batch "get several" endpoints, `/markets` and **`/artists/{id}/top-tracks` removed**; `Track.popularity` removed; search `limit` max **10**; refresh tokens expire in **6 months**; no numeric rate limits are published | All recorded with citations in `docs/WEB-API.md`; SPEC §6 and this phase rewritten to match. Still verify playlist `/items` against a real dev-mode login |
| R5 | ~~cmux may not pass the Kitty image protocol through~~ **REFUTED (1.4):** cmux speaks Kitty graphics at full fidelity, and `Picker::from_query_stdio()` detects it unattended | `docs/TERMINALS.md` + `docs/images/spike-1.4-cmux.png`. Half-blocks remains the Terminal.app path and must still look good |
| R6 | `cidre` API for process taps is unstable / under-documented | **MATERIALISED (1.5):** every *process-specific* tap description fails with `!obj` / `kAudioHardwareBadObjectError` (560947818) — both `*MixdownOfProcesses:` and `*GlobalTapButExcludeProcesses:` with a non-empty list, at every NSNumber width. An **empty** list is accepted. So a global tap works and "tap Spotify only" does not | **Decision in `docs/AUDIO-TAP.md` §3c: bypass `cidre` for the 4 ObjC initialisers with `objc2` (~60 lines) to find out whether the fault is the binding or the OS; ship simulated as the default meanwhile** |
| R7 | Spotify hiding is blocked on ≥ 1.3.1, so headless behaviour cannot be tested today | COMPAT test matrix row stays unverified until it works; do not fake it |
| R8 | Homebrew audit / policy rejects the formula | `brew audit --strict` in CI and before every release (9.5) |

---

## Phase 0 — Project init

- [x] 0.1 Create repo `Kathir-D/trak` (public), MIT `LICENSE` (with shpotify's notice), `.gitignore`,
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
- [ ] 1.5 **Process-tap visualizer spike (R1, R6).** Spike is `spikes/tap`; write-up is
      `docs/AUDIO-TAP.md` §3. **Left unticked because "tap Spotify's process only" does not work yet**,
      but most of the task is now answered. > **R1 REFUTED — no permission prompt at all.** A global
      tap from an un-bundled `cargo run` binary delivered real audio (48 kHz, 1 ch, 32-bit float,
      958 976 samples in 20 s ≈ 47 950/s, -17.57 dBFS RMS, peak 0.383) with **no System Audio
      Recording click**, on a machine whose TCC had no audio grant at all. Clean teardown, no
      leftover device. > **The real blocker (R6): every process-specific tap description fails** with
      `!obj` / `kAudioHardwareBadObjectError` (560947818, 0x216F626A) — `initMonoMixdownOfProcesses:`,
      `initStereoMixdownOfProcesses:`, and `initMonoGlobalTapButExcludeProcesses:` with a non-empty
      pid list, **at every NSNumber width (f64/i32/i64/u32)**. An *empty* list is accepted. It is
      **not** `kAudioDevicePermissionsError` (`!hog`), so there is nothing for the owner to grant.
      > **Decision (`docs/AUDIO-TAP.md` §3c): bypass `cidre` for the four ObjC initialisers using
      `objc2` (~60 lines) to find out whether the fault is cidre's binding or macOS; keep simulated
      as the default until then. Do NOT fall back to a global tap — it captures all system audio,
      including Sonar's, which is what COMPAT rule 3 is about.**
      > **Two traps carried into 8.3/8.5: (a) the tap is 48 000 Hz, so 1.6's sample rate is corrected
      and 8.3 must read `asbd.sample_rate` rather than hard-code; (b) `ca::device_start` returns a
      `StartedDevice` that must be kept alive — dropping it early stops the device and looks exactly
      like a hang with 0 samples.**
      > **No [owner] step is needed for the permission question** — there is no prompt to click. The
      remaining question for a human is only whether the `objc2` bypass fixes the allow-list; if it
      does not, 8.3 ships simulated and this task can be closed as a documented no-go.
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
      what makes trak hide the volume meter after every keypress. > A seek is compared against the
      **clamped** target, not the raw request — the first version reported every over-seek as a
      failure, which two tests caught. > The `Worker` drops a submission while one is in flight, so
      a held key cannot build a backlog. > `check_playable_uri` is an **allow-list in
      `player/mod.rs`, not in the AppleScript transport** — it was in the transport first, and the
      CLI tests caught that the fake bypassed it. User input enters trak at exactly one place.
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
- [ ] 2.7 **`play <name>` / `play album|artist|list <name>` / `play uri`.** > The URI half is
      done and tested: `trak play <spotify:uri>` works, and anything that is not a Spotify URI or a
      search term is rejected before it reaches AppleScript. > The search half still needs 7.12; it
      prints the Client ID steps and exits 2, which shpotify also did. `album|artist|list` subcommands
      are still to add — they only make sense with search.
- [ ] 2.8 **`trak` (no args) → TUI entry; `trak config` → settings entry.** > Bare `trak` exists
      and says what does work, exit 2. Replaced by the TUI in 3.1. `trak config` not added yet.
- [ ] 2.9 Update README "Usage" with real, copy-pasted output of each command. Done when: matches
      the binary.

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
      the background" and "trak never starts Spotify on its own". > `enter` and space are the only
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
      the mid-fade value, and a meter that follows it down says trak turned the music down. > An
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
      the Kitty query exactly as cmux does, trak sent **25 chunks totalling 73 984 bytes = 136×136×4**
      — a raw RGBA image at `f=32`, sized 17×8 cells from cmux's 8×17 px cell — and **zero**
      half-block glyphs. Without an answer it fell back to half-blocks and drew 64 `▀` cells. The
      half-block path is snapshot-tested: a red/blue fixture must put both colours in the buffer.
      > **[owner]** still worth one look: real cover art in a real cmux window, since a pty can only
      prove the bytes trak sends, not how they land on screen.
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
- [ ] 4.3 **Art / visualizer toggle plumbing** (`a`): `display.mode`. With `visualizer`, art is not
      drawn but is still fetched for colour. Visualizer content arrives in phase 8; until then show a
      placeholder pane. Done when: toggling swaps the pane and persists in config (after 5.x).
- [ ] 4.4 **System-volume fallback** (R2): setting `volume.control = "spotify" | "system"`, auto-
      suggested when read-back fails; `osascript -e 'set volume output volume N'`. Done when: works and
      the notice explains how to switch.
- [ ] 4.5 **Song-change notification** (`display notification` via osascript; setting off by default;
      only while the TUI runs; title = track, body = artist – album). Done when: toggling in settings
      works and it never fires on the first state read.
- [ ] 4.6 **Sonar state integration** exactly per COMPAT (file watch, freshness rules, header badge,
      mute disabled during a duck, never write a mid-fade volume). Done when: unit tests with fixture
      files (fresh / stale / malformed / wrong pid / unknown version) and a manual test with a
      hand-written state.json (Sonar itself may not have the writer yet).
- [ ] 4.7 **headless-spotify integration**: if on `PATH`, run `status --json` once at start and on
      demand (`headless` badge, extra hint to open Spotify's window), and prefer `launch` on the idle
      card. Unknown/absent fields ignored. Done when: tests with a fake binary on `PATH`.
- [ ] 4.8 **Status line / toasts**: one bottom-right line for transient messages (permission denied,
      volume ignored, API 403, lyrics not found). Done when: messages expire and never overlap the
      footer keys.

---

## Phase 5 — Config and the settings screen

- [ ] 5.1 **`config.rs`** per SPEC §8: load/save/defaults/unknown keys ignored/corrupt file backed up,
      atomic write, `XDG_CONFIG_HOME` respected, file mode `0600`. Done when: unit tests for each case.
- [ ] 5.2 **Settings screen** (`trak config` and `,` overlay): grouped checklist (Display, Theme,
      Visualizer, Input, Notifications, Spotify API) with `space` toggle, `←/→` change enum values,
      `enter` for text/guided flows, `q`/`esc` save-and-close; live preview: changes apply instantly to
      the running TUI. `trak config` standalone shows the same screen full-size. Done when: every key in
      SPEC §8 is editable, invalid states are impossible from the UI, snapshot tests of the screen.
- [ ] 5.3 **Wire every setting** to real behaviour (art/mode, progress, volume, popularity, hints,
      clock, side pane, default tab, border, accent, art protocol, viz style/source, mouse, steps,
      notifications, lyrics). Done when: a test per setting flips it and asserts the render changes.
- [ ] 5.4 **First-run experience**: no config → defaults, a one-time dismissible hint about
      `trak config` and the optional Client ID. Done when: first launch in a clean `HOME` shows it once.

---

## Phase 6 — Lyrics (both versions)

- [ ] 6.1 **LRCLIB client** (`https://lrclib.net/api/get?...` by track/artist/album/duration; fall back
      to search). Read their docs first and set a descriptive `User-Agent: trak/<version>
      (https://github.com/Kathir-D/trak)`. Timeouts, cache results on disk, never block the UI, no
      lyrics = clean empty state. Done when: tests with recorded JSON fixtures (no live network in CI).
- [ ] 6.2 **LRC parser + "current line for position"** (handles `[mm:ss.xx]`, multiple stamps per line,
      offset tag, plain-text lyrics without stamps). Done when: unit tests incl. malformed input.
- [ ] 6.3 **Lyrics tab**: current line highlighted and auto-scrolling, manual scroll pauses
      auto-follow for a few seconds. Done when: snapshot tests; manual sync check on a real song.
- [ ] 6.4 **Full-screen lyrics (`L`)**: large centered current line, dim neighbours, accent colour,
      `esc` returns. Done when: works at several sizes, no wrapping glitches (wrap long lines by
      display width, mind wide/CJK characters).

---

## Phase 7 — Version A: Web API

- [x] 7.1 **Confirm API reality (R4)** — done in 1.7; SPEC §6 and every task in this phase were
      rewritten against `docs/WEB-API.md` in the same commit. > The 7.2/7.3/7.5/7.7/7.8/7.10/7.11
      notes below carry the specific changes.
- [ ] 7.2 **PKCE auth**: loopback listener on an **ephemeral** port; the registered redirect URI is
      `http://127.0.0.1` with **no port**, and the port actually used is sent in the request. Opens
      the browser with `open`, exchanges the code, refreshes tokens automatically. > **Refresh
      tokens expire after 6 months** (`docs/WEB-API.md` §6) — record the authorisation time locally,
      warn before expiry, and treat an invalid refresh token as "discard and re-login", not as an
      error state. Needs: 7.1. Done when: unit tests with a mock token endpoint; **[owner]** completes
      a real login once.
- [ ] 7.3 **Guided setup in `trak config`**: screen with numbered steps — open
      `https://developer.spotify.com/dashboard` (via `open`), create an app, add the exact redirect URI
      shown (copyable), paste the Client ID (validate shape), press enter → browser login → success
      screen. `Log out` clears the token. > **The redirect URI to display and copy is exactly
      `http://127.0.0.1` — no port, no path, and never `localhost`** (Spotify bans `localhost`, and
      dynamic ports are explicitly allowed only for loopback IP literals). Confirm during the first
      real login whether the dashboard accepts the no-path form; fall back to a fixed
      `http://127.0.0.1:<port>/callback` if not. Done when: every step has an on-screen explanation
      and errors (bad ID, denied consent, port busy) are handled with retry. **[owner]** walks
      through it once.
- [ ] 7.4 **Token storage**: the `0600` file decided in 1.8 (`~/.config/trak/token.json`, `XDG_CONFIG_HOME`
      respected, directory `0700`, atomic temp-then-rename write). **Refuse to read a token whose mode
      is looser than `0600`** rather than proceeding, and assert that in a test. Never logged, never in
      `config.toml`. Keep the `Store` trait so tests can fake it, but there is only **one** real
      backend — do not build a Keychain one (it hangs, see 1.8). Documented in the README.
- [ ] 7.5 **`Library` trait + rspotify wrapper + `FakeLibrary`.** > **Do not use `rspotify`'s
      id-list helpers** (`tracks(ids)`, `artists(ids)`, `albums(ids)`) — the batch endpoints they
      call were removed in dev mode. Loop one id per request and cache hard; the quota is per
      developer account and shared across Client IDs. 429: **only `Retry-After` is documented and
      there is no `X-RateLimit-*` header or quota endpoint**, so back off with a cap and show a
      one-liner; distinguish `"reason": "QUOTA_EXCEEDED"` in the body. 403: two real causes — Premium
      (queue) and an account not on the app's 5-user allowlist. Map both to friendly typed errors.
      Needs: 7.1. Done when: fixture-driven tests, no live network.
- [ ] 7.6 **Search tab**: `/` focuses the input, live results debounced (~250 ms), grouped Tracks /
      Albums / Artists / Playlists, `Tab` jumps groups, `enter` plays (AppleScript `play track "<uri>"`,
      so it works on Free), `A` queues, `o` opens the artist/album page. Done when: update() tests
      with `FakeLibrary`; snapshot tests; stale responses never overwrite newer ones.
- [ ] 7.7 **Playlists tab** (list, open, play, tracklist) and **Liked tab** (list, play from here,
      `f` toggles like on the current track). > Field rename: playlist `tracks` → **`items`**
      (`items.items.item`), and `items` is **only present for playlists the user owns or collaborates
      on** — no feature may promise to show any playlist's tracks. > `f` is `PUT`/`DELETE
      `/me/library` and the liked check is `GET /me/library/contains`, not `/me/tracks`. > **Playlist
      item read/write is unverified in dev mode** (docs contradict themselves) — confirm here with a
      real login and degrade cleanly if it 403s. Done when: tests + manual.
- [ ] 7.8 **Queue tab** (now playing + up next) and add-to-queue (`A`). > `POST /me/player/queue`
      is **Premium-only by Spotify's own documentation**; `GET /me/player/queue` is not. A 403 on add
      is the expected Free-tier path, not an error. `User.product` no longer exists, so Premium
      cannot be detected up front — rely on the 403. Done when: works or shows the clear Premium
      message; tests.
- [ ] 7.9 **Library tab**: saved albums, followed artists, recently played. Done when: paginated
      lists load lazily; tests.
- [ ] 7.10 **Artist page** and **album page** (tracklist, play from track). Back with `esc`. >
      **Albums only — `GET /artists/{id}/top-tracks` was removed in dev mode with no replacement**, so
      the top-tracks half of this task is dead. `GET /artists/{id}/albums` still works. SPEC §6 is
      updated. Done when: navigation stack tests; manual.
- [ ] 7.11 **Playlist editing**: add current/selected track to a playlist (picker), remove from a
      playlist, create playlist. > Use `POST`/`DELETE /me/playlists/{id}/items` (the `/tracks`
      variants are removed), and `POST /me/playlists` to create. Same unverified-in-dev-mode caveat as
      7.7. Done when: confirmation on destructive actions; tests with fakes.
- [ ] 7.12 **`trak play <song|album|artist|list>`** (finish 2.7) using search; pick the best match
      like shpotify; print what it chose. Done when: `assert_cmd` tests with `FakeLibrary`.
- [ ] 7.13 **Tab order and default tab for A**, plus the B-mode hint that a Client ID unlocks these.
      Done when: SPEC §3 matches the built UI (update the doc if the order changed).

---

## Phase 8 — Visualizer

- [ ] 8.1 **`AudioSource` trait + simulated source**: smooth pseudo-random bars that respond to
      play / pause / track change (decay to zero when paused). Done when: deterministic seeded tests.
- [ ] 8.2 **Renderers** (pure fns): `spectrum`, `mirrored`, `waveform` (braille), `circular`; scale to
      any `Rect` (including tiny), accent colour gradient, no flicker. Study scope-tui for the braille
      approach (credit in THIRD-PARTY-NOTICES if code is borrowed). Done when: golden-buffer tests per
      style at 3 sizes.
- [ ] 8.3 **Real audio source** from the spike: tap Spotify's process, mono-mix, ring buffer, cavacore
      → bars, on its own thread; auto-fallback to simulated on denial/error with a one-time toast that
      says how to grant permission. `source = simulated` forces the fallback. Needs: 1.5, 1.6.
      Done when: bars visibly track the music on the owner's machine; denial path tested manually;
      CPU stays low (record % in the PR/commit).
- [ ] 8.4 **`v` cycles styles**, `a` toggles art/visualizer, both persist to config; visualizer FPS
      capped (~30) and paused when the terminal is hidden/too small. Done when: works live.
- [ ] 8.5 **Tap lifecycle**: start on demand, stop when the visualizer is hidden or Spotify quits,
      reattach when Spotify restarts, never leave a tap/aggregate device behind after exit or crash
      (RAII + signal handling; verify with `system_profiler SPAudioDataType` before/after).
      Done when: 10 start/stop cycles leave no leftover devices.

---

## Phase 9 — Release and Homebrew (no paid signing)

- [ ] 9.1 **`scripts/package-release.sh`**: build both targets (`rustup target add x86_64-apple-darwin`),
      `lipo` into a universal binary, `codesign --force -s -` (ad-hoc), tarball
      `trak-<version>-macos.tar.gz` containing `trak`, `LICENSE`, `README.md`, `THIRD-PARTY-NOTICES.md`,
      plus `SHA256SUMS.txt`. VERSION is the source of truth; the tag must be `v$(cat VERSION)`.
      Done when: script runs locally; `file` shows both archs; `codesign -dv` shows ad-hoc; the binary
      runs on arm64 with `--version`; `xattr` shows no quarantine after `curl` download.
      Alternative if lipo/sign misbehaves: two tarballs + `on_arm`/`on_intel` (see ARCHITECTURE).
- [ ] 9.2 **`.github/workflows/release.yml`** on `v*` tags: verify tag == VERSION, build, package,
      create the GitHub release with tarball + checksums, then update `Formula/trak.rb` in
      `Kathir-D/homebrew-tap` (needs a `HOMEBREW_TAP_TOKEN` secret). Model it on
      `../headless-spotify/.github/workflows/release.yml`. Done when: a dry-run on a pre-release tag
      produces a release and a tap commit. **[owner]** adds the secret (fine-grained PAT, contents:write
      on the tap repo only).
- [ ] 9.3 **`Formula/trak.rb`**: `desc`, `homepage`, `url`, `sha256`, `license "MIT"`,
      `depends_on macos: :sonoma` (14.x; the audio tap needs 14.2 — note this in `caveats` since Homebrew
      cannot express minor versions), `def install; bin.install "trak"; end`, caveats (optional Client
      ID via `trak config`, terminal permissions, optional Sonar / headless-spotify), `test do`
      asserting `trak --version` matches. No `system "codesign"`, no quarantine hacks: Homebrew formula
      downloads are not quarantined. Done when: `brew install --formula ./Formula/trak.rb` works from a
      local tap and `brew test trak` passes.
- [ ] 9.4 **Add the row to the tap README** (`brew install kathir-d/tap/trak`, uninstall, description)
      — or run prompt 3 in `docs/AGENT-PROMPTS.md`. Done when: the tap README lists trak.
- [ ] 9.5 **`brew audit --strict --online kathir-d/tap/trak`** clean; `brew style`. Done when: both pass.
- [ ] 9.6 **Fresh-machine test**: `brew install kathir-d/tap/trak` on a clean user/VM (or after
      `brew uninstall`), run `trak --version`, `trak status`, open the TUI. Done when: no Gatekeeper
      prompt, no manual steps. Record in `docs/RELEASING.md`.
- [ ] 9.7 **`docs/RELEASING.md`**: bump VERSION and Cargo.toml, changelog entry, tag, what CI does,
      how to yank a bad release. Add `CHANGELOG.md` (Keep a Changelog format).
- [ ] 9.8 Tag **v0.1.0** when phases 2–8 are done and phase 10 rows pass. **[owner]** approves the
      release before tagging.

---

## Phase 10 — Compatibility verification with Sonar and headless-spotify

Run each row of the table in `docs/COMPAT.md`, tick it there with the date and what was observed.
Requires Sonar installed and running (`brew install --cask kathir-d/tap/sonar`) and the real Spotify.

- [ ] 10.1 trak ↔ Sonar skip/prev both directions (< ~1 s propagation).
- [ ] 10.2 Ducking: trak shows the Sonar badge (needs Sonar prompt 1) and stays consistent; the volume
      meter is not corrupted; `m` is disabled during a duck.
- [ ] 10.3 Ownership: pausing in trak during/after a duck never gets undone by Sonar.
- [ ] 10.4 Spotify quit / relaunch while both are running.
- [ ] 10.5 Without Sonar and without headless-spotify installed: zero errors, zero warnings.
- [ ] 10.6 Headless Spotify: badge + full control, **when Spotify honours LSUIElement again (R7)**;
      until then leave unchecked and say so.
- [ ] 10.7 Visualizer taps concurrently with Sonar's tap without either failing.

---

## Phase 11 — README, docs, polish

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
         keep it < ~3 MB), a "Why trak" three-line pitch, feature list with icons, install (Homebrew
         first), a quick usage table, a clear **Version A vs Version B** comparison table, the
         `trak config` settings table, keybindings, "Works with Sonar & headless-spotify" (link to
         COMPAT), troubleshooting (permissions!), credits (shpotify, spotify-tui inspiration,
         cava/cavacore, ratatui, LRCLIB), license.
      3. Keep it scannable: contents list, collapsible `<details>` for long tables, alt text on every
         image, every claim true for the released version (no features that are not built).
      Needs: phases 3–8 built enough to screenshot. Done when: the README renders well on GitHub in
      light and dark mode (check both), all links work (`lychee` or manual), and an outsider can
      install and use trak from it alone.
- [ ] 11.2 Record the demo GIF/screenshots: Version B, Version A, visualizer styles, settings screen,
      full-screen lyrics. Done when: images committed and referenced.
- [ ] 11.3 GitHub repo polish: description, topics (`spotify`, `tui`, `rust`, `ratatui`, `macos`,
      `homebrew`, `terminal`), social preview image, `CONTRIBUTING.md`, issue templates,
      `SECURITY.md` (token handling note), `CODE_OF_CONDUCT.md` optional.
- [ ] 11.4 Add a "Works with trak" mention in Sonar's and headless-spotify's READMEs (their agents do
      this via prompts 1 and 2). Confirm the three READMEs cross-link.
- [ ] 11.5 Performance and battery pass: idle CPU < 1 % with the visualizer off and < ~5 % on;
      wake-ups minimised when paused; measure and record.
- [ ] 11.6 Accessibility / robustness pass: works with `NO_COLOR`, 16-colour terminals, light
      themes, very small and very large terminals, non-ASCII titles, right-to-left text does not break
      layout.
- [ ] 11.7 Final read-through of `AGENTS.md`, `CLAUDE.md`, and the docs so they match the shipped
      product; remove stale TODOs.

---

## Backlog (not committed to; do not start without the owner)

- Rebindable keys.
