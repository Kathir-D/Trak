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
| R1 | Real-audio visualizer: the process-tap permission is attributed to the *terminal app*; a CLI child may not get a usable "System Audio Recording" prompt, or cmux may not be prompt-able | Spike first (1.5). Always ship the simulated fallback (8.3). README documents which terminals work |
| R2 | Spotify ≥ 1.3.x ignores AppleScript `set sound volume` | Read-back check + hide meter + optional system-volume fallback (4.4) |
| R3 | Keychain items created by an ad-hoc-signed binary can re-prompt after every upgrade (the code identity changes) | Test in 7.4; fall back to a `0600` token file in `~/.config/trak/` and document the trade-off |
| R4 | Spotify Web API developer-mode rules changed recently (Premium owner requirement, user cap, endpoint removals, loopback-only redirect URIs) | Verify current rules in 7.1 *before* building A; update SPEC §6 |
| R5 | cmux may not pass the Kitty image protocol through | Spike 1.4; half-blocks fallback must look good on its own |
| R6 | `cidre` API for process taps is unstable / under-documented | Spike 1.5 with its `core-audio-record` example; if unusable, write a ~150-line Objective-C-free binding or a tiny helper, decided in the spike |
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
- [ ] 0.5 Confirm CI is green on GitHub after the first push; add the badge to the README.
      Needs: 0.3. Done when: Actions tab shows a green run on `main`.

---

## Phase 1 — Spikes: kill the unknowns before building on them

Each spike ends with facts written to a doc, not just working code. Throwaway code goes in
`spikes/` (git-ignored is fine; commit only if it is useful), the **findings** are committed.

- [ ] 1.1 **AppleScript field survey.** With Spotify running and a track playing, run each getter
      from SPEC §5 via `/usr/bin/osascript` and record the exact raw output, types, and units
      (duration ms vs s, position seconds float, popularity int, `artwork url`, `spotify url`,
      `id` format). Also: behaviour when nothing is loaded, when paused, when stopped, and when
      Spotify is not running (must error, not launch it — use `application "Spotify" is running`).
      Done when: `docs/APPLESCRIPT.md` has a table of every property with a real sample, plus fixtures
      saved under `tests/fixtures/applescript/*.txt`. Verify: is a `tell` on a non-running Spotify
      launching it? (It does; the running check must come first.)
- [ ] 1.2 **Batched read script + cost.** Write one script that returns state, position, volume,
      shuffling, repeating and all track fields as one delimited string (pick a delimiter that cannot
      occur in titles, e.g. `\u{1f}` unit separator). Time it 50×.
      Needs: 1.1. Done when: `docs/APPLESCRIPT.md` records p50/p95 latency. If p95 > 80 ms, note the
      alternative (JXA, or split fast/slow reads) in `docs/ARCHITECTURE.md`.
- [ ] 1.3 **`PlaybackStateChanged` notification.** Listen for `com.spotify.client.PlaybackStateChanged`
      (distributed notification), print `userInfo` on play / pause / skip / seek. Verify the key names
      (`Player State`, `Name`, `Artist`, `Album`, `Track ID`, `Duration`, `Playback Position` — believed
      but unconfirmed) and whether seeks fire it. Try from Rust (`cidre` / objc2) or a Swift one-liner
      just to observe. Done when: `docs/APPLESCRIPT.md` lists the real keys, and there is a decision on
      how the Rust app subscribes (crate, or a poll-only fallback if subscribing from a plain CLI
      process is not viable — a non-bundled process may not receive distributed notifications; verify).
- [ ] 1.4 **Album art in cmux and other terminals.** Render one image with the Kitty graphics
      protocol, then iTerm2 inline images, then sixel, in cmux (installed at `/Applications/cmux.app`)
      and in Terminal.app; use a tiny `ratatui-image` example (`Picker::from_query_stdio`).
      Done when: `docs/TERMINALS.md` has a table terminal × protocol × works? and the auto-detect
      result. Note that agent sessions have no interactive TTY; this needs a screenshot of a real cmux
      window (`screencapture`) or an **[owner]** eyeball. Half-blocks is the acceptable fallback.
- [ ] 1.5 **Process-tap visualizer spike (R1, R6).** Using `cidre`'s `core-audio-record` example as
      the base, tap **Spotify's process only** (by bundle ID → pid → tap description) and print RMS
      levels while a track plays. Test from the terminal app the owner uses. Record: does macOS show a
      "System Audio Recording" prompt for the terminal? does it work after granting? what happens on
      denial (error code, silence, or hang)? Does it work while Sonar's own tap is active?
      Done when: `docs/AUDIO-TAP.md` answers those four questions with evidence and states the
      go / no-go for real audio. **[owner]** may need to click a permission prompt.
- [ ] 1.6 **cavacore feasibility.** Feed 1 s of synthetic sine + the tapped samples through
      `cavacore` and print bar heights; confirm the crate builds on stable Rust for both
      `aarch64-apple-darwin` and `x86_64-apple-darwin`. Done when: findings + chosen bar count and
      sample rate in `docs/AUDIO-TAP.md`.
- [ ] 1.7 **Spotify Web API reality check (R4).** Read Spotify's current developer docs (redirect URI
      rules, developer-mode restrictions, rate limits, which endpoints still exist: search, playlists,
      queue, saved tracks/albums, followed artists, recently played, artist top tracks, album tracks,
      playlist edit). Done when: `docs/WEB-API.md` lists each endpoint trak needs, whether it is
      available in developer mode, scopes required, and any Premium-only behaviour; SPEC §6 updated.
      Cite the doc URL and date for each claim.
- [ ] 1.8 **Keychain vs ad-hoc signing (R3).** Store and read a secret with the `security-framework`
      crate from an ad-hoc-signed binary; rebuild (new signature) and read again; observe prompts.
      Done when: `docs/WEB-API.md` (token storage section) states the finding and picks Keychain or
      the `0600` file for real.

---

## Phase 2 — Core: Player layer + CLI parity with shpotify

Reference: upstream `spotify` bash script at https://github.com/hnarayanan/shpotify (or the local
clone at `../shpotify-tui/spotify` on the owner's machine). Behaviour reference only; write idiomatic Rust.

- [ ] 2.1 **Crate skeleton.** Add deps (`clap` derive, `anyhow`/`thiserror`, `serde`, `serde_json`,
      `toml`, `dirs` or manual XDG). Create the module tree from ARCHITECTURE (empty modules OK).
      Record each new dependency in `THIRD-PARTY-NOTICES.md`. Done when: `cargo build`, `clippy -D
      warnings`, `fmt --check` pass.
- [ ] 2.2 **`Player` trait + `PlayerState`/`TrackInfo` types + `FakePlayer`.**
      Needs: 1.1. Done when: unit tests drive the fake through play/pause/next/seek/volume.
- [ ] 2.3 **`AppleScriptPlayer`.** Uses `$TRAK_OSASCRIPT` (default `/usr/bin/osascript`), the batched
      read script (1.2), a 5 s timeout, `is running` guard (never launches Spotify), typed errors
      (`NotRunning`, `PermissionDenied` [-1743], `Timeout`, `Script(String)`). Parsing is a pure
      function tested against the fixtures from 1.1. Done when: parsing tests pass and a manual run
      prints real state; permission-denied produces a friendly message telling the user to allow their
      terminal in System Settings › Privacy & Security › Automation.
- [ ] 2.4 **Write actions with read-back**: play, pause, toggle, next, prev, replay, seek, set volume
      (read-back per COMPAT rule 5), shuffle, repeat, `play track "<uri>"`. Every write is user-initiated
      only. Done when: unit tests on generated script text; manual check against Spotify.
- [ ] 2.5 **CLI: `status`, `share`, `vol`, `pos`, `toggle`, playback verbs** with the decided
      output style (SPEC §9): tidy, coloured only on a TTY, `--plain`, `--json`. Exit codes 0/1/2.
      Done when: `assert_cmd` tests against `FakePlayer` cover each subcommand incl. errors; running
      `trak status` prints a small card with a progress bar; piping it prints plain text.
- [ ] 2.6 **`share url|uri`** copies to clipboard via `pbcopy` (fake in tests). Done when tested.
- [ ] 2.7 **`play <name>` / `play album|artist|list <name>` / `play uri`.** `uri` works without an API
      (AppleScript). Name searches need Version A: without a Client ID print how to get one and exit 2.
      Needs: 7.x for the search half. Done when: `play uri` works now; the search half is stubbed with
      the friendly error and a `TODO(7.x)` — do not leave dead code.
- [ ] 2.8 **`trak` (no args) → TUI entry; `trak config` → settings entry** (placeholders until 3.x / 5.x).
      Done when: both exist and print a clear "not built yet" message with exit 2 until replaced.
- [ ] 2.9 Update README "Usage" with real, copy-pasted output of each command. Done when: matches
      the binary.

---

## Phase 3 — TUI shell (Version B): layout, history, info

- [ ] 3.1 **Terminal plumbing**: ratatui + crossterm, alternate screen, raw mode, panic hook that
      restores the terminal, clean exit on `q`/ctrl-c/SIGTERM, resize events. Done when: the app opens
      and quits without leaving the terminal broken, including after a forced panic (test).
- [ ] 3.2 **`App` state, `Event`, `update()`** per ARCHITECTURE; event loop with a tick and a worker
      thread that polls `Player` (1 s playing / 3 s otherwise) and sends `Event::PlayerState`.
      Done when: table tests for `update()`; running against `FakePlayer` advances state.
- [ ] 3.3 **Wide layout**: header (name, status dot, clock), Now Playing pane (text-only for now:
      title, artist, album, shuffle/repeat, progress bar interpolated locally, ⏮ ⏯ ⏭, volume meter),
      right pane with tab strip, footer key hints. Rounded borders. Done when: `TestBackend` snapshots
      at 100×30 look like SPEC §3 with no wrapped or torn borders.
- [ ] 3.4 **Responsive breakpoints.** Tune numbers empirically and record them in ARCHITECTURE:
      wide (side by side) → stacked (right pane under Now Playing) → compact strip → "terminal too
      small" message below a hard minimum. Done when: snapshot tests at ≥ 6 sizes and a manual live
      resize test show no panic, no garbled borders, and layouts switching at the recorded thresholds.
- [ ] 3.5 **Idle card** when Spotify is not running: `enter` launches it (COMPAT rule 2) and the UI
      recovers by itself when the poll sees it. Done when: quitting Spotify while trak runs shows the
      card within 3 s and pressing enter brings it back without focusing Spotify's window.
- [ ] 3.6 **Session history + History tab.** Record each new track (by id/URI) as it starts playing
      while trak is open (dedupe consecutive identical, cap 500). `↑↓`/`jk` move, `enter` plays that
      URI via `play track`, current track marked `▶`. Empty-state text. Done when: unit tests on the
      recorder; manual: play 3 songs, arrow to the first, enter → it plays.
- [ ] 3.7 **Info tab** (AppleScript facts: duration, disc/track number, popularity, URI, artwork URL,
      played count, album artist, and "times heard this session"). Done when: shows real values and
      handles missing ones with `—`.
- [ ] 3.8 **Input**: all keys in SPEC §4 that apply to B, arrows + vim, `Tab`/`Shift-Tab`, `1`–`3`,
      `?` help overlay listing every key, `esc`. Mouse (click tabs and rows, click/drag progress bar to
      seek, wheel scroll, click ⏮⏯⏭) gated by the config flag (default on). Done when: each key has
      an `update()` test; help overlay matches the SPEC table (add a test that fails if a key in the
      table is missing from the help text).
- [ ] 3.9 **Subscribe to `PlaybackStateChanged`** (per the 1.3 decision) so external changes (Sonar,
      media keys) appear instantly; keep the poll as backup. Done when: skipping with a media key
      updates trak in < 300 ms (measure and record).
- [ ] 3.10 **Volume behaviour** per COMPAT rules 3 and 5: user-set volume tracking, read-back after
      writes, meter hidden + notice when Spotify ignores sets. Done when: tests with a fake that
      ignores writes; manual on the real Spotify.
- [ ] 3.11 **Copy share URL (`c`)** with a transient "copied" toast. Done when: works, tested with
      a fake clipboard.

---

## Phase 4 — Now Playing richness and integrations

- [ ] 4.1 **Album art**: fetch `artwork url` (cache on disk under `~/Library/Caches/trak/`, bounded),
      decode, render with `ratatui-image` (auto protocol; half-blocks fallback), correct aspect ratio
      in cells, re-render on resize, never block the UI. Needs: 1.4. Done when: art shows for the
      current track in the owner's terminal and in half-blocks mode; snapshot test of the half-block
      path; art disappears cleanly when the terminal shrinks below the breakpoint.
- [ ] 4.2 **Accent colour from art**: dominant colour (skip near-black/near-white, bias saturation),
      applied to borders, progress, highlights; smooth-ish change on track change; settings `accent =
      art|green|terminal`. Ensure contrast on dark *and* light terminals. Done when: three modes
      visibly work; a unit test on the extractor with fixture images.
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

- [ ] 7.1 **Confirm API reality (R4)** — this is task 1.7's output; read `docs/WEB-API.md` and adjust
      the tasks below if endpoints or rules changed. Done when: SPEC §6 and this phase match the doc.
- [ ] 7.2 **PKCE auth**: loopback listener on a free port (or the fixed port the redirect URI needs),
      opens the browser with `open`, exchanges the code, refreshes tokens automatically, handles
      revocation / expiry by prompting re-login. Needs: 7.1. Done when: unit tests with a mock token
      endpoint; **[owner]** completes a real login once.
- [ ] 7.3 **Guided setup in `trak config`**: screen with numbered steps — open
      `https://developer.spotify.com/dashboard` (via `open`), create an app, add the exact redirect URI
      shown (copyable), paste the Client ID (validate shape), press enter → browser login → success
      screen. `Log out` clears the token. Done when: every step has an on-screen explanation and errors
      (bad ID, denied consent, port busy) are handled with retry. **[owner]** walks through it once.
- [ ] 7.4 **Token storage** per the 1.8 decision (Keychain or `0600` file), never logged, never in the
      config file. Done when: tests for both backends behind a `Store` trait; documented in README.
- [ ] 7.5 **`Library` trait + rspotify wrapper + `FakeLibrary`.** Rate-limit (429 + Retry-After) and
      403 (Premium-only) map to friendly typed errors. Done when: fixture-driven tests, no live network.
- [ ] 7.6 **Search tab**: `/` focuses the input, live results debounced (~250 ms), grouped Tracks /
      Albums / Artists / Playlists, `Tab` jumps groups, `enter` plays (AppleScript `play track "<uri>"`,
      so it works on Free), `A` queues, `o` opens the artist/album page. Done when: update() tests
      with `FakeLibrary`; snapshot tests; stale responses never overwrite newer ones.
- [ ] 7.7 **Playlists tab** (list, open, play, tracklist) and **Liked tab** (list, play from here,
      `f` toggles like on the current track). Done when: tests + manual.
- [ ] 7.8 **Queue tab** (now playing + up next) and add-to-queue (`A`). Note Premium-only limits from
      1.7. Done when: works or shows the clear Premium message; tests.
- [ ] 7.9 **Library tab**: saved albums, followed artists, recently played. Done when: paginated
      lists load lazily; tests.
- [ ] 7.10 **Artist page** (top tracks, albums) and **album page** (tracklist, play from track).
      Back with `esc`. Done when: navigation stack tests; manual.
- [ ] 7.11 **Playlist editing**: add current/selected track to a playlist (picker), remove from a
      playlist, create playlist. Done when: confirmation on destructive actions; tests with fakes.
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

- Homebrew-core submission (needs stars/notability).
- Device switching (Spotify Connect) — dropped from v1.
- Rebindable keys.
- Extras explicitly rejected for v1: `status --format`, `trak mini`, shell completions, man page.
