# trak — continuation prompt

Paste everything below into a fresh session. It is written to be self-contained:
you do not need this conversation, and you should not try to reconstruct it.

---

## Who you are and what you are doing

You are continuing work on `trak`, a Rust + ratatui terminal UI and CLI for macOS
that controls the Spotify desktop app through AppleScript, with an optional
Spotify Web API layer. You are an unattended agent on the owner's machine: nobody
is at the keyboard, you have full permissions, and **you do not stop to ask
questions**. The only reason to stop is the end of `TODO.md`. When the TODO is
finished, report and stop.

Working style the owner has asked for repeatedly:

- **Work through `TODO.md` in order.** It is the task list, and the order is the
  plan. Do not reorder it for convenience.
- **Use subagents generously.** The project is large and the modules divide
  cleanly. Give each one a strict file boundary and tell it not to touch anything
  else — that has been the single biggest source of wasted work.
- **Commit and push constantly.** After every coherent piece of work:
  `git add -A && git commit && git push`. The repo is `github.com/Kathir-D/trak`,
  branch `main`, and every commit so far has gone to `main` directly.
- **Wrap every long command in `scripts/with-timeout`.** See below; it matters.

## Start here

1. `AGENTS.md` — the rules. Read it fully. Especially the "Hard rules" and
   "Definition of done" sections.
2. `TODO.md` — find the first unchecked task whose `Needs` are met. That is your
   next task. Every ticked task carries a note explaining the *decisions*, not
   just the change; read the notes, they are the point.
3. `docs/SPEC.md` — the product. `SPEC §3` is the tab layout, `§4` the key table,
   `§8` the config schema. **SPEC wins over TODO** where they disagree, and several
   TODO entries already say so explicitly.
4. `docs/COMPAT.md` — non-negotiable. trak shares a machine with two sibling
   projects (`../Sonar`, `../headless-spotify`) and those are read-only to you.
5. `docs/WEB-API.md` — researched authority for the Web API. **Four things in the
   original plan were removed in dev mode and this document is the corrected
   version.** Do not use an endpoint, a `limit` cap, or a field name it does not
   list.

## Current state

`main` is clean and pushed. Last commit:

```
06e9c08 feat: the Web tabs take keys, and the loop makes the calls
```

Gate passes as of that commit: `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, **635 lib tests + 27 CLI tests**,
`cargo build --release`. Re-run all four before you commit anything.

### Phases finished

- **1–4** complete, including 4.4 (volume control enum + Sonar ducking guard),
  4.5 (song-change notification), 4.8 (toasts).
- **5** complete: 5.1 `config.rs` (hand-written TOML, no new dependency), 5.2
  settings screen, 5.3 every setting wired, 5.4 first-run hint.
- **8.1, 8.2, 8.4** complete: `AudioSource` trait, four renderers, `v` cycles
  styles, 30 fps only while the visualizer is on screen.
- **9.1, 9.2, 9.3, 9.7** complete: release script, workflow, formula, runbook.
- **2.8** complete: `trak config` as a standalone screen.
- **7.2, 7.4, 7.5** complete: PKCE, the `0600` token file, the `Library` trait +
  `ureq` client + `FakeLibrary`.
- **6.1, 6.2, 6.3** complete (lyrics tab, synced, works).
- **7.6–7.11** *just landed* in the last two commits. The state machine, the
  rendering, the keys and the worker calls are all in.

### Where the work actually is now

The Web API tabs are written but **nothing has been verified against a real
Spotify login**, because that needs a Client ID only the owner has. The next
substantive piece of work is:

- **7.3** — guided setup in `trak config`: numbered steps, open the dashboard, log
  in, verify. The Client ID field already exists and is editable (5.2); what is
  missing is the guided flow around it and wiring it to `web::auth::Login`.
- **7.12 / 2.7** — `trak play <song|album|artist|list>` using search, picking the
  best match.
- **7.13** — tab order is *built* (`Tab::ALL`, `Tab::digit`, `Tab::from_digit`)
  and the strip works, but **SPEC §3 has not been updated to match what was
  built**, and that is explicitly part of 7.13's done-when.
- **6.4** — full-screen lyrics on `L`. The only key left in `NOT_YET`.
- **8.3, 8.5** — real audio source and tap lifecycle. **8.3 is blocked**, see
  below.
- **9.4–9.6, 9.8, 10.x, 11.x** — the tap README row, `brew audit`, the fresh
  machine test, tagging, the COMPAT test matrix, and the polish pass.

## Things that will bite you

**Use `scripts/with-timeout` for anything long.** It exists because an unattended
agent will otherwise sit on a hung command forever with no output and no exit.
Neither `timeout` nor `gtimeout` is on this machine, so the script's own
fallback runs, and that fallback is the interesting part: **macOS has no `setsid`**,
so it walks the process *tree* with `pgrep -P` instead of signalling a process
group. The first version reported the right exit code and left orphaned processes
holding a terminal. If you extend it, keep that property and its test.

```sh
scripts/with-timeout 300 cargo test --lib
scripts/with-timeout 600 cargo build --release
```

**`src/tui/mod.rs` must list every module.** A module file that exists but is not
declared simply does not compile in — and its tests silently do not run. This cost
real time: 119 tests in `src/web/*` were not executing because `mod.rs` had doc
comments and no `pub mod` lines. After creating a module file, declare it and
check the test count actually went up.

**Do not use `git checkout <file>` to undo a mistake.** It threw away an hour of
uncommitted tab-strip work. Fix forward with a targeted edit.

**Bugs the tests keep finding, so you know where to look first:**

- *Tables that must agree.* Two lists describing the same fact drift. The tab
  digits, the strip order, the click index, the config key names, the help table
  versus SPEC §4 — each is one table now, and each has a test that compares them
  to each other. When you add a tab, a key, or a setting, extend the table and
  let the test tell you what else mentions it.
- *Off-by-one between a person-facing number and a table index.* `from_digit` and
  `digit` disagreed by one for a while. The digits are 1-based because people
  count from one; `ALL` is 0-based. One conversion, in one place.
- *`x as usize` on a fieldless enum is the discriminant, not the position.* It
  compiles and it is wrong. Use `iter().position()`.
- *Doc lines starting with `NNN.`* are Markdown ordered-list items, so rustdoc
  renders the comment as a numbered list with hanging continuations. Spell the
  status out: "HTTP 429".
- *A read limit is not a status.* Hitting one fails the read, and a failed read is
  indistinguishable from a network that went away. Check the declared length
  first.

**Existing conventions you must match.** Typed errors with `thiserror` and a
one-line `notice()`; no `unwrap()` on anything external; no panics in the render
path; `update(App, Event) -> App` is pure and *returns* commands rather than
running them; slow work goes on the worker; the renderer only draws. Comments
explain **why** — a Spotify quirk, a measurement, a dev-mode removal — never
what. No emojis. `rustfmt.toml` sets width 100.

**Four JSON readers exist:** `src/lyrics.rs`, `src/sonar.rs`, `src/headless.rs` are
hand-written (they predate serde), and `src/web/api.rs` uses serde. Do not
refactor the first three; that is churn, not a task. They are noted in the TODO
backlog.

## Blocked, and only the owner can unblock these

- **1.5 / 8.3, the real audio source.** Every process-specific Core Audio tap
  description fails with `kAudioHardwareBadObjectError` / `!obj` / OSStatus
  `560947818`. A *global* tap works. So the visualizer ships on the simulated
  source, which is why 8.1's note says simulated is the shippable default and not
  a placeholder. The next attempt is bypassing `cidre` with `objc2`, or shipping
  simulated.
- **A real Web API login.** Needs a Spotify Client ID (developer mode requires the
  owner to hold Premium and caps the app at 5 allowlisted users), and confirming
  that the dashboard accepts `http://127.0.0.1` with **no port and no path**.
  `AUTHORIZE_ENDPOINT` and `TOKEN_ENDPOINT` in `src/web/auth.rs` are constants
  whose doc comments say outright that they are *not* from `docs/WEB-API.md` —
  they are confirmed at the first real login.
- **`brew` release steps** (9.4–9.6, 9.8) and the **COMPAT matrix** (10.x), which
  needs both sibling projects running.

**Owner items are marked `[owner]` in `TODO.md`.** List them in your final message;
do not attempt them.

## Verifying against a real terminal

There is no interactive TTY, so:

- `scripts/screen.py W H --keys 'jj?'` runs trak in a pty and prints the screen.
  It is an approximation — it leaks some SGR, so colours are unreliable — but it
  catches layout, and the keys work now that `Picker` no longer breaks input.
- `trak config < /dev/null` prints the settings as TOML and exits 2, which is a
  useful non-TTY check.
- The real visual authority is a cmux window. **Never `SIGKILL` a live raw-mode
  TUI** — it leaves the terminal in a broken state. Use `SIGINT`, or close only the
  tab you created.
- AppleScript from an agent session must go through the shim:
  `ASRUN_BYPASS=1 $HOME/.local/bin/osascript`. trak itself uses `$TRAK_OSASCRIPT`,
  default `/usr/bin/osascript`.
- `./spikes/verify.sh --quick` re-checks the Phase 1 spike claims in about three
  minutes. Read it before re-deriving anything in `docs/APPLESCRIPT.md`,
  `TERMINALS.md`, `AUDIO-TAP.md` or `KEYCHAIN.md` — it compares live output against
  what those docs claim, and a FAIL means the doc is wrong.

## Definition of done, every task

1. The task's `Done when:` is observably true. Run it; do not assume.
2. `fmt`, `clippy -D warnings`, `test`, `build --release` all pass.
3. New behaviour has tests. Anything you could only check by hand is listed with
   exact steps.
4. Docs updated in the same commit if behaviour, keys, config keys or architecture
   changed. `docs/SPEC.md` is the source of truth for product behaviour.
5. The `TODO.md` box ticked **with a note about the decisions**, not just the
   change. Those notes are what the next agent reads; several of the most useful
   ones in the file right now are about what a task deliberately did *not* do and
   why.
6. Committed and pushed.
