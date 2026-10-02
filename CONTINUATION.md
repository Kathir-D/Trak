# Trak — continuation prompt

Paste everything below into a fresh session. It is self-contained: you do not need the conversation
that produced it, and you should not try to reconstruct it.

---

## How to work

The owner's standing instructions are in `AGENTS.md` under "⚠ Working style" (work `TODO.md` in
order, subagents with strict file boundaries, commit and push constantly, never stall, never ask
mid-flight, re-read your diff). They outrank your preferences. Read that section first.

## Start here

1. `AGENTS.md` — rules and Definition of done.
2. `TODO.md` — first unchecked task whose `Needs` are met. Every ticked task has a note about the
   decisions; read the notes, not just the boxes.
3. `docs/SPEC.md` (wins over TODO), `docs/COMPAT.md` (non-negotiable), `docs/WEB-API.md`.

---

## Current state (end of the 2026-10-02 session)

Branch `ccr-529c7134-80scjb` (cloud session; `main` is the owner's merge target), pushed. **Every
task that can be done without the owner's Mac, a real Spotify login or a paid step is done.**
Phases 1–7 and 9.1–9.3/9.7 are ticked; this session did 7.3, 7.6–7.11, 7.13 and part of 11.x, and
found that most of "7.6–7.11 code landed" was not actually wired:

- 7.3 guided setup (`src/tui/setup.rs`, `s` on the settings screen); startup now derives the Web
  connection (`connection_at_start`) — nothing set it before, so no Web tab could ever load.
- A stale Web **access** token is renewed on the worker (`web_client`); the UI thread asks the
  no-network `web_ready`. Before this a login would have died after an hour.
- 7.6 the debounced search never fired (nothing advanced `search_debounce`); 7.7 nothing asked
  whether the playing track is liked; 7.9 every list stopped at its first page (`more ↓` was
  unreachable); 7.11 no key reached the playlist writes. All wired and tested.
- 11.6 `NO_COLOR` + 256/16-colour downgrade (`src/tui/colour.rs`); 11.3 CONTRIBUTING / SECURITY /
  issue templates; 11.1 README rewritten (partial).
- Fixed a real bug: a broken pipe writing the script to osascript hid osascript's own error.

### What is left, and why you cannot do it here

| Task | State | Needs |
| --- | --- | --- |
| 0.4 | open | **[owner]** run the sibling prompts in `docs/AGENT-PROMPTS.md` |
| 8.3, 8.5 | blocked | real audio tap; 1.5 found no process-only tap (see 8.1's note). Needs the owner's Mac |
| 9.4–9.6, 9.8 | open | sibling tap repo, `brew audit`, fresh-machine install, release approval — **[owner]** |
| 10.1–10.7 | open | run `docs/COMPAT.md`'s matrix with Sonar and real Spotify — **[owner]** |
| 11.1 | partial | demo GIF, release badges, `docs/README-NOTES.md`, light/dark render check |
| 11.2 | open | screenshots/GIF from a real terminal |
| 11.3 | partial | **[owner]** repo description/topics/social preview, enable private vulnerability reporting |
| 11.4 | open | the sibling READMEs, via prompts |
| 11.5 | open | idle CPU / battery measurement on a Mac |
| 11.6 | code done | **[owner]** check a light-background theme, RTL order, `NO_COLOR=1 trak` live |
| 11.7 | open | final read-through; do it last, when the above are settled |

**Never verified against a live account (so say "unverified" until the owner has):** every Web API
tab, playlist item reads/writes in dev mode (docs contradict themselves), the login itself
(whether the dashboard accepts the no-path redirect `http://127.0.0.1`), add-to-queue's Premium 403.
`[owner]` steps for each are in the 7.x notes in `TODO.md`.

**Privacy:** `docs/images/tui-6.4-fullscreen.png` is a whole-desktop screenshot (other windows, an
API-key page). The README does not reference it; the owner should remove it and re-shoot.

---

## Working in the Linux cloud container (this is not a Mac)

The project is macOS-only (`objc2`, `osascript`, `pbcopy`), so `cargo test` does not build there. What
worked, and what to expect:

- `export CARGO_HTTP_CAINFO=/root/.ccr/ca-bundle.crt CARGO_HTTP_MULTIPLEXING=false` or crate
  downloads die with HTTP/2 broken-pipe errors through the proxy.
- **Type-check and clippy for the real target:** `rustup target add aarch64-apple-darwin`, then
  `pip install ziglang`, a wrapper that runs `python3 -m ziglang cc -target aarch64-macos` (strip
  cc-rs's `--target=`, `-arch`, `-mmacosx-version-min`, `-gfull` args) set as
  `CC_aarch64_apple_darwin`, then `cargo clippy --all-targets --target aarch64-apple-darwin`.
  Check/clippy work; nothing can be *run*.
- **Run the tests on Linux** from a scratch copy of the repo with `objc2` removed from `Cargo.toml`
  and `src/player/notify.rs` stubbed (delete `define_class!`, `parse`, `subscribe`, `pump_run_loop`;
  add stubs). 718 lib + 37 CLI tests pass there except `the_clipboard_helper_actually_copies`
  (needs `pbcopy`) — that one failing on Linux is expected, not a regression. The script that
  builds the copy lived in the session scratchpad and is not in the repo; rewriting it takes
  minutes.
- A newer clippy than the owner's flags nothing in the gate now; two `manual_range_contains` lints
  in `accent.rs` were fixed for it.

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