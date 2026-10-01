# Trak — continuation prompt

Paste everything below into a fresh session. It is written to be self-contained:
you do not need this conversation, and you should not try to reconstruct it.

---

## CONTEXT: another agent was working in this repo and stopped mid-task

A previous agent was working through Phase 6 (lyrics, 6.1–6.4) and **stalled
out of context**: it left the tree uncompilable — a stray debug test outside the
test module in `src/lyrics.rs`, and a real bug where a 404 from the LRCLIB search
fallback surfaced as `Malformed` ("LRCLIB sent an answer trak could not read")
instead of `NotFound`, plus a test that served two canned replies to a code path
that makes four requests. **All of that is now fixed, tested, and committed** (see
"Phase 6" below). So: the working tree is clean, `main` builds, and the gate
passes. Nothing is half-edited. You are starting from a green tree.

Before you touch anything, form your own plan from `TODO.md` and treat the notes
under each ticked box as the real specification — several say what a task
deliberately did **not** do and why.

---

## ⚠ HOW TO WORK — the owner's standing instructions (highest priority)

These are **not** suggestions. They outrank convenience, tidiness, and your own preferences about
how to sequence a session. The owner has restated each of them repeatedly, across sessions. They are
also written into `AGENTS.md` under "⚠ Working style — the owner's standing instructions", so they
survive without this file; read them there too.

1. **Work through `TODO.md` in order, top to bottom.** Take the first unchecked task whose `Needs`
   are met. The order in that file *is* the plan; do not reorder it because something else looks more
   interesting or easier. When you reach the very end of `TODO.md`, that is the **only** moment you
   stop and talk to the owner.
2. **Use subagents constantly.** The project is big and the modules divide cleanly. Many focused
   subagents, each with a **strict file boundary** ("you own these files, touch nothing else"), the
   way Phase 6 (6.1–6.4) was done. Parallel agents share one working tree, so overlapping files
   collide: either hand each subagent disjoint files, or give each its own `git worktree` and merge
   the branch when it reports done.
3. **Never idle while a subagent is running.** Start the next piece of work in parallel, on files
   that subagent does not own. Sitting and waiting is wasted time.
4. **Commit and push constantly.** After every coherent piece of work:
   `git add -A && git commit && git push`. Saving progress is part of the task, not a finishing step
   — a context loss or a crash must never cost more than the last few minutes of work. Never leave a
   session's worth of work uncommitted.
5. **Do not stop to ask the owner anything.** You are an unattended agent with full permissions on
   their machine. Finish the task. The one exception is the end of `TODO.md`; also list the `[owner]`
   items you could not do (a browser login, a paid step, a macOS permission click, a GitHub secret)
   in your final message instead of asking for them mid-flight.
6. **Test extensively, and re-read your own work.** Run the whole suite, not just the tests you
   added. Read your diff before you commit, and read it again after. Look for the failure classes
   listed under "Things that will bite you" — they are the ones that have actually bitten this
   codebase, and every one of them got through to a "looks fine" moment first.
7. **You may be continuing someone else's mid-task work.** The previous agent stalled mid-Phase 6
   and left the repo uncompilable; the session after it found the cause by reading the diff, not by
   being told. Check `git status`, `git log --oneline -20`, and whether the suite compiles before
   assuming the tree is sane. If you find broken work, finish or revert it deliberately — never
   assume a half-edited file is intentional.
8. **Manage your context deliberately.** If you are compacted or your context fills, do **not** fish
   through history. Re-read, in this order: `git log --oneline -20`, `git status`, the ticked notes
   in `TODO.md`, `AGENTS.md`, then this file. Everything worth keeping is written into the repo on
   purpose — that is why every ticked task carries a note and why this file exists. When you stop at
   the end of the TODO, **rewrite `CONTINUATION.md`** so the next agent inherits the plan, the
   decisions and the traps instead of rediscovering them: the next task, every unmerged worktree and
   its branch, the audit findings, and the traps.

---

## Start here

1. `AGENTS.md` — the rules. Read it fully: "Hard rules" and "Definition of done".
2. `TODO.md` — the task list. **First unchecked task whose `Needs` are met is your
   next task.** Every ticked task carries a note about the *decisions*.
3. `docs/SPEC.md` — the product. §3 layout/tabs, §4 the key table, §6 Version A,
   §8 the config schema. **SPEC wins over TODO** where they disagree.
4. `docs/COMPAT.md` — non-negotiable. Trak shares this machine with
   `../Sonar` and `../headless-spotify`; the volume/mute rules there are why
   several keys are refused while Sonar is fading.
5. `docs/WEB-API.md` — researched authority for the Web API. Four things in the
   original plan were **removed in dev mode**. Do not use an endpoint, a `limit`
   cap or a field name it does not list.

---

## Current state

Repo: **`github.com/Kathir-D/Trak`** (renamed from `trak` on 2026-10-01; GitHub
redirects the old URL). Branch `main`, **clean**, pushed, CI green.

```
fe525eb chore: rename the project to Trak, everywhere GitHub shows it
b5f32d0 feat: the lyrics scroll, the offset tag, and the full-screen lyrics page (6.2, 6.3, 6.4)
afbaa85 feat: the LRCLIB client, its disk cache, and recorded fixtures (6.1)
eb70a20 docs: the usage table carries the real output of every command (2.9)
538354c feat: the objc2 tap bypass finds the pid->AudioObjectID translation (1.5)
```

Gate on `main` as of `fe525eb`: `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, **661 lib + 27 CLI tests**,
`cargo build --release`. Run all four before every commit.

### The rename, and what deliberately did NOT change

The project is **Trak** in display: repo name, README hero, docs prose, LICENSE
lines, the in-app wordmark in the header and the idle card, the LRCLIB/other
`User-Agent` repo URLs, and the URLs in the two sibling READMEs (both pushed).

It is **still `trak` lowercase** where it is a command or a path, and that is
deliberate:

- the binary and the CLI command (`trak status`),
- `~/.config/trak/`, `~/Library/Caches/trak/`,
- the release artifacts `trak-<version>-macos.tar.gz`,
- **the Homebrew formula name `trak` and the class `Trak`** — Homebrew *requires*
  lowercase formula names and `brew audit` fails otherwise,
- diagnostic prefixes in errors (`trak: config line 7: …`) — CLI convention.

Renaming the binary would break the formula, the config path and muscle memory for
zero gain. If a future owner asks to rename the binary, that is a real change with
a migration, not a find-and-replace.

### Phase 6 — lyrics — COMPLETE this session

- **6.1** LRCLIB client, disk cache next to the art cache (`~/Library/Caches/trak/<fnv>.json`,
  64 entries, temp+rename, zero-length and non-deserialising files are misses,
  misses are never cached), real recorded replies committed under
  `tests/fixtures/lyrics/` and read at compile time. No live network in tests.
  *The bug the stall was chasing:* a 404 from the search fallback must be
  `NotFound`, not `Malformed` — the miss body is a JSON **object**
  (`TrackNotFound`), so without a status check in `search()` a double miss said
  "LRCLIB sent an answer trak could not read", blaming the service for a song that
  simply is not there.
- **6.2** LRC parser: multi-tag lines → one line per tag, a bare tag is a musical
  rest, unsynced lines carry `f64::NAN` that `index_at` can never select,
  malformed lines dropped rather than guessed. `[offset:±ms]` applies to the whole
  file including lines *before* the tag, positive shifts **earlier**, a
  shifted-before-zero line is **clamped to 0.0 not dropped**, last tag wins,
  unparseable offset shifts nothing.
- **6.3** `j`/`k` and the wheel take the scroll from the song for **4 s**, counted
  down by `Event::Tick` (never a wall clock, so the resume is tested by ticking),
  and the pane says "following paused — it resumes on its own" while it lasts. On
  the Lyrics tab `j`/`k` move the *words*, never a history selection the tab
  cannot show; a click on a lyric row plays nothing. The sung line keeps its
  gradient wherever it lands in the window.
- **6.4** Full-screen lyrics on `L` (any tab), `L`/`esc` leave, transport keys
  still answer while it is up. Centred anchor line, bold, accent colour, dim
  neighbours, renders 100×30 down to 10×1. `wrap_by_width` wraps by **display
  width** (`unicode-width`, moved from dev-deps to deps and re-recorded in
  THIRD-PARTY-NOTICES) so 40 CJK glyphs fill an 80-column row.
  `NOT_YET` is now **empty** — every key SPEC §4 promises to both versions is
  bound — and 3.8's "the excuse list should not be empty" assertion was replaced,
  because empty is the finished state.
- Real-song check (2026-10-01, "Beauty Sleep — Jane Remover): live LRCLIB lookup
  returned synced lyrics, wrote the cache, and the line-at-position resolved
  correctly at pos 48 s against the playing Spotify. Screenshots committed:
  `docs/images/tui-6.3-tab.png`, `docs/images/tui-6.4-fullscreen.png`.

### Phases finished before that

- **1–5** complete (spikes, player + CLI parity, TUI shell, art/accent/now-playing
  richness, config + settings screen).
- **7.2, 7.4, 7.5** complete: PKCE, the `0600` token file, `Library` trait +
  `ureq` client + `FakeLibrary`.
- **7.6–7.11** code landed (state machine, rendering, keys, worker calls) but the
  boxes are unticked and, per the audit below, several gaps are real.
- **8.1, 8.2, 8.4** complete: `AudioSource` trait, four pure renderers, `v` cycles,
  30 fps only while on screen.
- **9.1, 9.2, 9.3, 9.7** complete: release script, workflow, formula, runbook.

---

## WORKTREES — one has finished work that is NOT merged yet

```sh
git worktree list
git log --oneline main..wt/7.12-play-search     # 4 commits, pushed, not merged
```

### `../trak-wt-7.12` (branch `wt/7.12-play-search`) — DONE, needs merging

Four commits on top of `main`, all pushed to origin:

```
f13fdb6 docs: the usage table carries the real output of the play spellings (7.12)
6ca8465 feat: `trak play <name>` searches and plays the best match (7.12, 2.7)
cc1ba13 feat(cli): the match a play spelling chose, and the line that says it (7.12)
2cdc503 feat(web): the rule for what a play-by-name search picks (7.12)
```

It implements **7.12 / 2.7**: `trak play <name>`, `trak play album|artist|list <name>`,
best-match rule (`web::api::best_match` — first row whose *name* contains the
whole query case-insensitively, else the first row outright, mirroring shpotify's
`limit=1` behaviour), prints the choice after the play lands, and a hidden
`--fake-library` flag for tests. It reports **687 → 710 tests** and found and fixed
a real pre-existing defect (the "you need a Client ID" message hard-coded
`` `trak play "{query}"` ``, so `trak play album mezzanine` was told to run a
different command). It also found doc drift it deliberately did not fix:
**`docs/ARCHITECTURE.md:123` says the CLI test flag is `--fake-player`; the code
has always used `--fake`.**

**Your first action:** review `git diff main..wt/7.12-play-search`, run the gate in
that worktree, merge to `main` (`git merge --no-ff wt/7.12-play-search`), push,
tick **7.12** and **2.7** in `TODO.md` with notes, then `git worktree remove
../trak-wt-7.12`.

### `../trak-wt-8.3` (branch `wt/8.3-real-audio`) — EMPTY, re-run the subagent

A subagent was spawned for 8.3/8.5 and was cancelled before writing anything (the
branch is still at `fe525eb`). The brief was written and is worth reusing — see
"Next: 8.3/8.5" below.

---

## Next, in TODO order

### 1. Merge the 7.12 branch (above) and tick 2.7 + 7.12

### 2. **7.3 — guided setup in `trak config`** (the real next build)

MISSING entirely today. The `client_id` text row exists (`settings.rs:222-231`) and
is editable; nothing around it is. To build, per the task text:

1. a screen with **numbered steps**: open `https://developer.spotify.com/dashboard`
   (via `open`), create an app, add the redirect URI — **exactly
   `http://127.0.0.1`, no port, no path, never `localhost`** (Spotify bans
   `localhost`; dynamic ports are allowed only for loopback IP literals) — shown
   **copyable**,
2. paste the Client ID (**validate the shape**; 5.2 deliberately left validation
   out because 7.3 owns it),
3. press enter → browser login → success screen,
4. `Log out` clears the token (`Store::clear` exists and is tested; unused),
5. errors handled with retry: bad ID, denied consent, port busy.

The machinery is all built and tested: `web::auth::Login::run` (`auth.rs:547`) and
`Session::refresh` (`auth.rs:614`). **`docs/WEB-API.md` §1's open question — does
the dashboard accept the bare no-path form? — is confirmed at the owner's first
real login** (`[owner]`); fall back to a fixed `http://127.0.0.1:<port>/callback`
if not.

### 3. The three systemic Phase 7 gaps (the audit's G1–G3)

These are why 7.6–7.11 are unticked despite existing code. Fix them and much of
7.6–7.11 becomes verifiable:

- **G1 — `Event::Connection` is never emitted.** The handler exists
  (`app.rs:1334-1340`); **no producer exists anywhere**. So
  `WebState.connection` stays `NoClientId` forever ⇒ (a) the per-tab auto-fetch
  gated on `web.connection.connected()` (`loop_.rs:452-453`) **never fires** — the
  Playlists/Liked/Queue/Library lists never load on tab entry — and (b) the "add a
  Client ID" notice shows even when a token exists. `web::token::Connection`
  (`LoggedOut`/`Connected`/`ExpiringSoon`/`NeedsReconnect`) is never mapped to
  `app::Connection`. `hint_shown` is dead for the same reason.
- **G2 — nothing ever writes a token file.** Only doc comments outside `src/web/`
  reference `web::auth`. Version A is unreachable in a live session without a
  hand-made `token.json`. *This is exactly what 7.3 fixes.*
- **G3 — no token refresh in the loop.** `web_client()` (`loop_.rs:686-700`) reads
  the token file fresh and never refreshes; after the 1-hour access-token life,
  jobs 401, and `submit_web` (`loop_.rs:804-806`) **silently drops jobs** when
  there is no client — a stale token means keys do nothing with no message.

### 4. The per-task gaps the audit found

| Task | State | What is missing |
| --- | --- | --- |
| 7.6 Search | partly | `SEARCH_DEBOUNCE_SECS` (`app.rs:666`) is referenced **nowhere**: nothing counts it down, nothing fires — search runs **only on Enter**, so "live results" do not exist. Stale-answer guard exists and is tested (`app.rs:1300-1302`). `[`/`]` jump groups instead of `Tab` — deliberate, with a written rationale at `app.rs:1550-1565`; **TODO's text was never amended**. |
| 7.7 Playlists/Liked | partly | `WebJob::IsLiked` is **never pushed**, so `liked_here` starts `None` and the first `f` always assumes *not liked* — `f` on an already-liked track re-likes it. The Liked tab's rows have **no cursor marker** although `j`/`k` move `liked_cursor` and `enter` plays it. |
| 7.8 Queue | partly | Renders; Premium 403 maps to a clear message. Never loads live (G1); no periodic refresh. |
| 7.9 Library | partly | Three lazy sections. **No load-more path**: every `run_web` call passes `after: None` (`loop_.rs:717-761`), nothing follows a continuation, and the `more ↓` row (`web_tabs.rs:511-513`) is **inert** — nothing can ever make it true. |
| 7.10 Artist/album | partly | Pages + `esc` stack work. `row_id` returns `None` whenever a page is open (`app.rs:1677-1679`), so **enter/`o` on an album row inside an artist page does nothing — an artist page is a dead end**. The 2-level test builds its stack by calling `open_album` directly, not through `update`. |
| 7.11 Playlist editing | partly | Backend **complete** (`POST /me/playlists`, `POST`/`DELETE /playlists/{id}/items`, all three tested). **Zero UI**: no key pushes those jobs, no picker, no create flow, and **no confirmation on destructive actions** — a stated done-when criterion. |
| 7.12 | **done, unmerged** | see above |
| 7.13 | partly | `Tab::ALL` order matches SPEC §3's A-mode list, but **SPEC §3 was never updated** (its own done-when). **`default_tab` is parsed, saved, round-tripped — and never applied**: `App::new` hardcodes History and the loop never assigns `app.tab` (`loop_.rs:323-324`). |

Also worth fixing while you are in there: **mouse is not wired to web lists** — no
`Hit` variant for a web row, so a click or wheel on a web tab resolves to
`HistoryPane` and *silently moves the history cursor*.

### 5. **8.3 + 8.5 — the real audio tap**

**The old continuation note claiming 8.3 is blocked is WRONG — it was written
before 1.5 landed. `docs/AUDIO-TAP.md` §3b/§3c records it working:** the fault was
the missing pid→AudioObjectID translation, not the OS. Translating with
`kAudioHardwarePropertyTranslatePIDToProcessObject` (`'id2p'`) makes every
process-specific shape work, and `spikes/tap/src/bin/tap-objc2.rs` (~60 lines)
builds `CATapDescription` directly. 575 488 float samples in 12 s ≈ 47 957/s of
**real Spotify audio from Spotify's process only**, **no permission prompt at all**,
clean teardown.

Traps recorded for 8.3/8.5 (take them literally):

- (a) the tap is 48 000 Hz — **read `asbd.sample_rate`**, never hard-code;
- (b) `ca::device_start` returns a `StartedDevice` that **must be kept alive** —
  dropping it early stops the device and looks exactly like a hang with 0 samples;
- (c) `name` on `CATapDescription` is an *instance* method and **objc2 panics** on
  a class-method send — use `AnyClass::name()`;
- (d) the `'prs#'` process-list read returns `'nope'` here even though `'id2p'`
  works — translate the pids you care about, **do not enumerate**;
- **never fall back to a global tap** — it captures all system audio including
  Sonar's, which is what COMPAT rule 3 is about;
- cavacore: `CavaBuilder::default()` (no `new()`), `SampleRate::new(x)` not
  `SampleRate::Hz(x)`, **one `Cava` per stream** (peak/autosens state lives inside
  it), output is **not normalised**.

Re-run the 8.3 subagent with the worktree, owning `src/audio.rs` (new),
`src/visualizer.rs`, `Cargo.toml`, `THIRD-PARTY-NOTICES.md`, `docs/AUDIO-TAP.md`,
`examples/` — **not** `src/tui/**`; wire it into `loop_.rs`/`app.rs` yourself.
8.5's done-when is 10 start/stop cycles leaving no leftover devices — verify with
`system_profiler SPAudioDataType` before/after.

### 6. The rest

`9.4` tap README row · `9.5` `brew audit --strict --online` · `9.6` fresh-machine
`brew install` test (needs a release; partly `[owner]`) · `9.8` tag **v0.1.0**
(`[owner]` approves) · **10.1–10.7** the COMPAT matrix (needs Sonar running —
check `brew list --cask`; if Sonar is not installed that whole phase is
`[owner]`, say so rather than faking it) · **11.1–11.7** the README refactor,
demo GIFs, repo polish, performance/battery pass, accessibility pass, final
read-through. `0.4` (sibling agent prompts) is `[owner]`, not blocking.

---

## Things that will bite you

**Wrap long commands in `scripts/with-timeout`.** An unattended agent will
otherwise sit on a hung command forever with no output and no exit. Neither
`timeout` nor `gtimeout` is on this machine, so the script's own fallback runs,
and that fallback is the interesting part: **macOS has no `setsid`**, so it walks
the process *tree* with `pgrep -P` instead of signalling a process group. The
first version returned the right exit code and left orphans holding a terminal.
Keep that property if you extend it.

```sh
scripts/with-timeout 300 cargo test --lib
scripts/with-timeout 600 cargo build --release
```

**`src/tui/mod.rs` must declare every module.** A file that exists but is not
declared does not compile in *and its tests silently do not run*. This once hid
119 tests in `src/web/*`. After creating a module file, declare it and check the
test count actually went up.

**Never `SIGKILL` a live raw-mode TUI** — it leaves the terminal broken. `SIGINT`,
or close only the tab you created.

**Do not use `git checkout <file>` to undo a mistake.** It threw away an hour of
uncommitted work once. Fix forward with a targeted edit.

**Do not try to view screenshots with the model.** This session's model could not
process image input, so `screencapture` output had to be verified
programmatically instead (TestBackend snapshots, cache contents, live AppleScript
reads). If your model can see images, look; otherwise do not waste turns on it.

**Bugs the tests keep finding, so you know where to look first:**

- *Tables that must agree.* Two lists describing one fact drift. Tab digits, strip
  order, click index, config key names, help table vs SPEC §4 — each is one table
  with a test comparing them. Add a tab/key/setting and let the test tell you what
  else mentions it.
- *Off-by-one between a person-facing number and a table index.* Digits are 1-based,
  `ALL` is 0-based, one conversion in one place.
- *A test that cannot fail.* Both directions get checked (the help must list every
  promised key **and** every excuse must genuinely be inert). Half a check is a
  test that passes forever.
- *Measuring a rendered wide glyph.* ratatui pads a two-cell character with a skip
  cell; naïve width counting reads every CJK glyph as three columns. Walk the
  buffer the way the terminal does.
- *`x as usize` on a fieldless enum is the discriminant, not the position.*
- *Doc lines starting `NNN.`* become Markdown ordered lists in rustdoc.
- *A read limit is not a status* — check the declared length before reading.

**Conventions to match:** typed errors with `thiserror` plus a one-line
`notice()`; no `unwrap()`/`expect()` on anything external; no panics in the render
path; `update(App, Event) -> App` is pure and *returns* commands rather than
running them; slow work goes on the worker, the renderer only draws; `Debug`
redacts anything holding a token; comments explain **why** (a Spotify quirk, a
measurement, a dev-mode removal) never *what*; no emojis; `rustfmt.toml` width 100.
Four JSON readers exist (`lyrics.rs`, `sonar.rs`, `headless.rs` hand-written;
`web/api.rs` uses serde) — do not refactor the first three, that is churn.

---

## Verifying in this environment

There is no interactive TTY.

- `scripts/screen.py W H --keys 'jj?'` runs Trak in a pty and prints the screen. An
  approximation — it leaks some SGR, so colours are unreliable — but it catches
  layout, and the keys work now that `Picker` no longer breaks input.
- `trak config < /dev/null` prints the settings as TOML and exits 2.
- A real cmux window is the visual authority: `open -a cmux`, then System Events
  keystrokes to type the command, `screencapture -x` for the pixels, `q` to quit.
- AppleScript from an agent session **must** go through the shim:
  `TRAK_OSASCRIPT="$HOME/.local/bin/osascript" ./target/release/trak`, or
  `ASRUN_BYPASS=1 $HOME/.local/bin/osascript`. Plain `/usr/bin/osascript` gets
  silently denied (-1743). If a `-1743` appears, its one-time prompt is sitting
  unanswered on screen — do the job another way and mention it.
- `./spikes/verify.sh --quick` re-checks the Phase 1 spike claims in ~3 minutes.
  Read it **before** re-deriving anything in `docs/APPLESCRIPT.md`,
  `TERMINALS.md`, `AUDIO-TAP.md` or `KEYCHAIN.md` — it compares live output to what
  those docs claim, and a FAIL means the doc is wrong.
- Spotify was left **paused on "Beauty Sleep — Jane Remover"** where it was found.

---

## Definition of done, every task

1. The task's `Done when:` is **observably true**. Run it; do not assume.
2. `fmt`, `clippy -D warnings`, `test`, `build --release` all pass.
3. New behaviour has tests. Anything you could only check by hand is listed with
   exact steps.
4. Docs updated in the same commit if behaviour, keys, config keys or architecture
   changed. `docs/SPEC.md` is the source of truth for product behaviour; the
   README describes only what is shipped.
5. `TODO.md` box ticked **with a note about the decisions** — what it does *not*
   do and why is the most valuable part. If a task is impossible or the docs are
   wrong, add a `> BLOCKED:` or `> CHANGED:` note and fix the doc in the same
   commit. A ticked box without a met done-when is a lie to the next agent.
6. Committed and pushed.