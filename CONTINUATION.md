# Trak — continuation prompt

Paste everything below into a fresh session. It is self-contained: you do not need the conversation
that produced it, and you should not try to reconstruct it.

---

## How to work

The owner's standing instructions are in `AGENTS.md` under "⚠ Working style" (work `TODO.md` in
order, subagents with strict file boundaries, commit and push constantly, never stall, never ask
mid-flight, re-read your diff). They outrank your preferences. Read that section first, then
`docs/SPEC.md` (wins over TODO), `docs/COMPAT.md` (non-negotiable) and `TODO.md`'s ticked notes.

## Current state (end of the 2026-10-05 session)

**0.2.2 is released** (`main`, tag `v0.2.2`, both CI jobs green). It is a patch release: seven
user-visible fixes and no new surface. `CHANGELOG.md`'s `[0.2.2]` section is the record.

The session's theme was **focus you can see**, and every item was the same shape of bug: the state
knew where the keys were and the pane drew something else.

- **History's `›` marker tracked the wrong row once the list had scrolled.** The rows drawn start
  `history_scroll` in, so the drawn index is a *window* offset, not the cursor. It now compares
  against `history_cursor.saturating_sub(history_scroll)`, and the marker and the accent style come
  from one `selected` so they cannot disagree. **This is the trap to remember**: anything that
  compares a cursor against a row it drew has to subtract the scroll first.
- **The side pane's border only ever lit for the Web API tabs**, because only they report focus
  through `web.list_focus`. History and Lyrics answer `j`/`k` through `app.focus`, which nothing
  read. `draw_tabs` now takes the accent when `app.focus == Focus::Pane` on those two tabs, and
  `Tab::Info` stays out on purpose (nothing in it answers the arrows).
- **The Queue tab's Free-tier fallback is a list now.** The "Recently played" rows that stand in
  for a queue the Web API will not give a Free account were drawn but inert. `web_rows` counts them,
  `j`/`k` move `queue_cursor` over them, `enter`/`A`/`P` act on the selected row, and
  `App::queue_fallback_row` is the one place that resolves it. Two things had to be right: the row
  **count** lives in state (`queue_rows`, beside `RECENT_ON_QUEUE` in `app.rs`) because both
  `web_rows` and `clamp_queue_cursor` need it, and the cursor is **clamped where the rows change**,
  because a write empties the queue and the tab is then the shorter list under an old cursor.
- **One `k` now gets the arrows home** from an unfocused Web list: the pane *declines* (`false`)
  the key at the top of the list rather than swallowing it (`true`), so the level above can take it.
- **`Tab::Lyrics` needed the descent.** Found only by driving the real binary over a pty, not by
  any test: the arm that scrolls the words ran *ahead* of the descent, so on that tab alone the top
  bar kept `j`/`k` for good, against SPEC §4's unconditional "descend / come back out". Now `j`
  descends first and scrolls second, `k` off the top of the words comes back out, and the pane's
  border can light at last.
- **A focused pane no longer bolds its whole title.** `pane_block` put `BOLD` on the whole strip
  when focused, so every tab was as heavy as the selected one and the weight meant "the keys are
  down here" instead of "this is your tab" — and only while the pane was *not* focused did the
  selected tab stand out by weight. The strip test now checks both states.

Still unverified, and it needs the owner: **the whole Web API half.** No browser login has ever
been completed, so search, playlists, queue, library and the token refresh have never met a real
account (TODO 12.10). The Client ID's app also still needs **Web API** ticked under "APIs used",
and the account needs Premium. Also open: the Sonar / headless-spotify rows of `docs/COMPAT.md`
(phase 10), the README badges and Version A screenshot and social preview (11.1–11.3), and a
fresh-machine install (9.6).

The one piece of this work left undone, deliberately: **on a Web tab the marker and the border read
two different focus signals.** One `j` from the bar marks row 0 (`app.focus == Pane`) while the
border still follows `web.list_focus` — the *second* `j` — so they disagree for one keystroke. It
is in `TODO.md`'s Backlog with the reasoning: which signal is the truth for a Web tab is a
decision, and it is a one-keystroke disagreement on tabs nobody has loaded against a real account.

## Tools and traps (what cost time, across sessions)

- **Buffer-level assertions lie in two ways, and both bit this session.** A `str::find` offset is
  a **byte** offset, and a pane row starts with `│` (three bytes, one column), so reading cells at
  `pane_x + offset` lands two columns off — the cell looks like a border-painted space and the
  test fails for a reason that has nothing to do with the code. And the tab strip is **not on row
  0**: the page keeps a menu bar above the two panes, so the strip is the side pane's top border,
  which is one row down. Find the strip, do not assume where it is.
- **`Buffer` cells are not `Copy`.** `let cell = buf[(x, y)]` moves; borrow it and read the fields.
- **A focused pane's border has square corners and no `DIM`; an unfocused one has rounded corners,
  `DIM` and a lit-from-one-side edge.** That is the cheapest way to assert pane focus from raw
  escape sequences when driving the real binary.
- **Fuzz and probe the TUI in a pty.** `target/scratch/lat/` has the harness (not committed):
  `fuzz.py` sends random keys at odd sizes and colour depths and reports anything that dies,
  panics or fails to leave the alternate screen; `drive.py` times a burst of keys, can slow one key
  down (`--slow n:3.4`, so each poll catches one track change) and dumps the screen as text or as a
  per-step style probe; `stub-osascript` answers in the measured times; `vt.py`/`shot.py` print the
  screen. **Put a fake `open` first on `PATH`** (`target/scratch/lat/bin/open`) before anything
  sends `s`: the guided setup's first step opens a real browser, and a fuzz run did exactly that
  (2026-10-02, several Safari tabs).
- **`scripts/fake-spotify` only has two songs**, so a scrolled History row is indistinguishable
  from the newest one. Copy it to scratch and change the `TRACKS` list to 40 distinct titles when a
  test needs a long history — do not edit the repo's copy.
- Screen size has to be set before `execv` (ioctl on fd 0 in the child). Trak asks the terminal
  once at startup and only redraws on a change, so a size applied afterwards leaves a screen drawn
  for the pty's default 80x24 with the rest of it never painted.
- **Do not answer trak's OSC 11 background query from the harness.** The reply goes into the pty's
  input, where the line discipline may echo it into the frame and trak reads it as keypresses. Let
  the query time out (300 ms).
- **`?`/`q` in the settings screen do not quit trak** — `q` saves and closes, `esc` closes, and a
  text field swallows `q` entirely. A fuzzer that ends with "still running after three q's" is
  reporting its own key sequence, not a bug.
- **cmux cannot be driven from an agent session**: `cmux` says "only processes started inside cmux
  can connect". That is why the GIF is recorded from a pty instead.
- **The Spotify login needs a human.** trak opens Safari and waits on the browser's "Allow"; the
  owner has to click it. Everything either side of that click is automated here.
- Always the scratch config: `XDG_CONFIG_HOME=$PWD/target/scratch/xdg` (trak's file is at
  `<XDG_CONFIG_HOME>/trak/config.toml`, not directly in it). For real Spotify use
  `TRAK_OSASCRIPT="$HOME/.local/bin/osascript"`. The owner may be listening: read the state first,
  restore volume, shuffle, repeat and the track afterwards, and never launch Spotify.
- `scripts/fake-spotify`'s track ids and artwork ids are the real ones -- an earlier copy pointed
  at another song's artwork (*Cage Girl / Camgirl*) and the demo showed it; check each with
  `curl "https://open.spotify.com/oembed?url=https://open.spotify.com/track/<id>"`. It reports a
  `spotify:track:` URI, or trak treats the track as an advert and never looks its lyrics up.
- **Profiling:** `CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false cargo build
  --release --target-dir target/prof`, then `sample <pid> 5`. dtrace is blocked by SIP.
- **Gate before every commit:** `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test`, `cargo build --release`. `cargo` may need `export
  PATH="$HOME/.cargo/bin:$PATH"`. `cargo fmt --all` (without `--check`) is how this repo is
  formatted after an edit. `cargo` takes a lock on `target/`, so two agents building at once
  serialise: give a subagent its own worktree if it needs to build while you do.
- **A subagent that only reviews earns its keep**: the review of this session's diff found two real
  defects the tests did not (the open-page guard and the missing cursor clamp), and both fixes got
  a test that was checked to *fail* with the fix reverted. Reverting a fix and watching its test
  go red is the only honest way to know a test is evidence.
- **`scripts/package-release.sh` does not refuse a version that is already released.** Measured
  2026-10-05: it re-packaged 0.2.1 in place, without a word, and a local rebuild's sha256 is not
  the published one. `docs/RELEASING.md` §1 now says so; the published checksum is verified in §4
  against the release asset.
- **`brew install --formula ./Formula/trak.rb` does not work on this machine** — Homebrew refuses a
  formula outside a tap. `docs/RELEASING.md` §1 step 4 therefore needs the tap route; the
  equivalent check is `brew audit --strict --online kathir-d/tap/trak` plus `brew style`, then
  extracting the tarball and running `--version` on it.
- Sibling repos (`../Sonar`, `../headless-spotify`) are read-only. `../homebrew-tap` is read-only
  except the one-time README row, which is done; do not hand-edit `Formula/trak.rb` in it.