# AGENTS.md — Trak

Instructions for any coding agent (Claude Code, opencode, Codex, ...) working in this repo. You are
assumed to have **no prior context**. Read this file fully, then follow "Start here".

---

## ⚠ Working style — the owner's standing instructions (highest priority)

These are not suggestions and they outrank convenience, tidiness, and your own preferences about
how to sequence a session. The owner has restated each of them repeatedly, across sessions.

1. **Work through `TODO.md` in order, top to bottom.** Take the first unchecked task whose `Needs`
   are met. The order in that file *is* the plan; do not reorder it because something else looks
   more interesting or easier. When you reach the very end of `TODO.md`, that is the only moment
   you stop and talk to the owner.
2. **Use subagents constantly.** This is a big project and the modules divide cleanly. The way to
   get through it is many focused subagents, each with a **strict file boundary** ("you own these
   files, touch nothing else"), the same way Phase 6 (6.1–6.4) was done. Parallel agents share one
   working tree, so overlapping files collide: either hand each subagent disjoint files, or give
   each its own `git worktree` and merge the branch when it reports done.
3. **Never idle while a subagent is running.** Start the next piece of work in parallel, on files
   that subagent does not own. Sitting and waiting is wasted time.
4. **Commit and push constantly.** After every coherent piece of work:
   `git add -A && git commit && git push`. Treat saving progress as part of the task, not as a
   finishing step — a context loss or a crash must never cost more than the last few minutes of
   work. Never leave a session's worth of work uncommitted.
5. **Do not stop to ask the owner anything.** You are an unattended agent with full permissions on
   their machine. Finish the task. The one exception is the end of `TODO.md`; also list the `[owner]`
   items you could not do (a browser login, a paid step, a macOS permission click, a GitHub secret)
   in your final message instead of asking for them mid-flight.
6. **Test extensively, and re-read your own work.** Run the whole suite, not just the tests you
   added. Read your diff before you commit, and read it again after. Look for the failure classes
   listed under "Conventions" — they are the ones that have actually bitten this codebase, and
   every one of them got through to a "looks fine" moment first.
7. **Be aware you may be continuing someone else's mid-task work.** Check `git status`,
   `git log --oneline -20`, and whether a test suite compiles before assuming the tree is sane. A
   previous agent once stalled mid-task and left the repo uncompilable; the next session found the
   cause by reading the diff, not by being told. If you find broken work, finish or revert it
   deliberately — never assume a half-edited file is intentional.
8. **Manage your own context deliberately.** If you are compacted or your context fills, do not fish
   through history: re-read `git log --oneline -20`, `git status`, the ticked notes in `TODO.md`,
   and this file. Everything worth keeping is written into the repo on purpose — that is why every
   ticked task carries a note and why `CONTINUATION.md` exists. When you stop at the end of the
   TODO, rewrite `CONTINUATION.md` so the next agent inherits the plan, the decisions and the traps
   rather than rediscovering them.
9. **Never stall.** The previous agent stalled mid-task and the repo sat broken and idle. So:
   wrap anything that could hang in `scripts/with-timeout` and move on rather than waiting on it;
   if a subagent fails, is cancelled or returns nothing useful, **do the work yourself or re-spawn
   it — never wait on it**; never leave the tree not compiling, because a red suite is how a
   session dies quietly. If you notice you have stopped making progress — the same command twice,
   an empty turn, a subagent you are polling — **stop, re-anchor from `git log`/`git status`/
   `TODO.md`, and start the next unchecked task.** Forward progress is the whole job.

---

## What this project is

Trak is a Rust + ratatui terminal UI and CLI that controls the **official Spotify desktop app on
macOS** (through AppleScript, so the Free tier works with zero setup) and, optionally, the Spotify
Web API (with a user-supplied Client ID) for search, playlists, queue and library. It is a Rust
rewrite and expansion of [shpotify](https://github.com/hnarayanan/shpotify) (MIT), keeps all of its
commands, and adds a rich TUI, a music visualizer, and synced lyrics. It ships as a **Homebrew
formula** with **no paid code signing**.

It is one piece of the owner's music setup, alongside two sibling projects in `../`:
**headless-spotify** (hides Spotify from the Dock) and **Sonar** (menu-bar skip/prev + auto-pause).
Trak must coexist with both. That contract is `docs/COMPAT.md` and is non-negotiable.

**Status:** pre-alpha, and further along than it looks. Phases 1–6 are done: the player layer and
the full shpotify CLI, the TUI, album art and the accent extracted from it, config and the settings
screen, and synced lyrics with a full-screen page. Phase 7 (the Web API, "Version A") has its client,
token store and tabs built but is not finished. `TODO.md` is the truth, and every ticked box carries
a note about the decisions behind it — read those notes, not just the boxes.

## Start here (read in this order)

1. `docs/SPEC.md` — every product decision, the layout, keys, config schema, CLI. Already decided
   by the owner in a long design interview. **Do not re-ask; do not redesign.**
2. `docs/COMPAT.md` — how Trak must behave next to Sonar and headless-spotify.
3. `docs/ARCHITECTURE.md` — module map, principles, testing strategy.
4. `TODO.md` — the ordered task list. Pick the first unchecked task whose `Needs` are met.
5. `THIRD-PARTY-NOTICES.md` — update whenever you add a dependency or borrow an idea.

Then read the code for the task you picked. Do not start coding from this file alone.

## Hard rules

**Where things live (owner's workspace rule, from `../AGENTS.md`)**
- All repos and code live in `/Users/kathirdev/Documents/projects/`. This repo is
  `projects/trak`. Never create repos or write code outside `projects/`.
- Clone with `gh repo clone <owner/repo>` or `git clone <url>` with **no target dir** (shell
  wrappers redirect it into `projects/`). Never pass a target outside `projects/`.
- Sibling repos (`../Sonar`, `../headless-spotify`, `../homebrew-tap`) are **read-only for Trak
  tasks**. Do not edit them. If they need a change, add or update a prompt in
  `docs/AGENT-PROMPTS.md` and tell the owner. (`../shpotify-tui` is a local clone of the upstream
  shpotify for reference; also read-only.)

**Git (owner's global rules)**
- After every commit, push right away (`git push -u origin <branch>` if no upstream). Do not stop to ask.
- **Never force-push** without asking first. Prefer `main` for small tasks; use a branch + PR only
  if the owner asks.
- Conventional commits (`feat:`, `fix:`, `docs:`, `chore:`, `test:`, `ci:`, `refactor:`), small,
  one task per commit. Run `git status` before committing; never commit `target/`, `dist/`,
  secrets, tokens, or `.env` files.
- End commit messages with the `Co-Authored-By` trailer your harness gives you.

**Product**
- macOS 14.2+ only. MIT license. Keep the shpotify copyright line in `LICENSE`.
- Distribution is a Homebrew **formula** (prebuilt binary from a GitHub release). **No Apple
  Developer account, no notarization, no paid signing, no quarantine tricks.** Ad-hoc
  `codesign -s -` (free) is allowed and required for the universal arm64 binary.
- Talk to Spotify **only** through `tell application "Spotify"` / bundle ID `com.spotify.client`
  and, in Version A, the Web API. Never hardcode `/Applications/Spotify.app`.
- **Never launch Spotify as a side effect.** The only launch is the explicit "press enter to
  launch" on the idle card (`docs/COMPAT.md` rule 2).
- **Write to Spotify only in direct response to a user key or command** (`docs/COMPAT.md` rule 3),
  or Sonar's auto-pause ownership breaks.
- No secrets in the repo, in logs, or in the config file. Tokens go to Keychain or a `0600` file.
- Optional integrations (Sonar state file, headless-spotify, real-audio visualizer, lyrics, Web
  API) must **fail soft**: degrade to simpler behaviour, explain in one status line, never panic.
- Do not add the rejected extras: `status --format`, `trak mini`, shell completions, man page.

## Toolchain and commands

Rust is installed via rustup (`~/.cargo/bin`). It is on `PATH` in login shells; if `cargo` is "not
found" in a non-login shell run `export PATH="$HOME/.cargo/bin:$PATH"` (or `. "$HOME/.cargo/env"`).
Stable Rust, edition 2024, MSRV in `Cargo.toml`.

```sh
cargo build                      # debug build
cargo run -- --version           # run the binary
cargo test                       # all tests (must pass with no Spotify, no network, no audio device)
cargo fmt --all -- --check       # formatting (rustfmt.toml: width 100)
cargo clippy --all-targets -- -D warnings
cargo build --release
```

All four of fmt / clippy / test / release-build must pass before you commit. CI
(`.github/workflows/ci.yml`, `macos-15`) runs them plus a check that `VERSION` equals the
`Cargo.toml` version. To install the second target for universal builds:
`rustup target add x86_64-apple-darwin`.

## Dev machine (unattended home server)

- Nobody is at the keyboard. Do not stop to ask permission for routine work; finish the task.
  Ask only for genuine owner decisions or steps marked **[owner]** in `TODO.md` (browser logins,
  clicking a macOS permission prompt, adding a GitHub secret, approving a release). List those in
  your final message.
- `sudo -n` works without a password (use `-n` so it fails instead of hanging). You should almost
  never need it for Trak.
- **AppleScript from an agent session**: plain `osascript` in `~/.local/bin` is a shim routed
  through `asrun` (which holds the Automation/Accessibility grants). Plain calls to
  `/usr/bin/osascript` from the agent process are silently denied (error -1743). Trak calls
  `$TRAK_OSASCRIPT` (default `/usr/bin/osascript`), so when testing against a real Spotify from an
  agent session run with `TRAK_OSASCRIPT="$HOME/.local/bin/osascript"`, or `ASRUN_BYPASS=1` to call
  the real binary. If an app returns -1743, its one-time Automation prompt is pending on screen and
  nobody can click it: do the job another way (System Events UI scripting, the app's CLI, `open`),
  and mention the pending prompt in your final message.
- Agent sessions have **no interactive TTY**. You cannot see the TUI directly. Verify rendering with
  ratatui `TestBackend` snapshot tests, and for real-terminal checks either use `screencapture` on a
  cmux window or list the check as **[owner]**.
- The terminal used day to day is **cmux** (`/Applications/cmux.app`, libghostty based).
- Spotify may or may not be running or playing. Never assume it; tests must not depend on it.

## Architecture in one screen

`main.rs` → `cli/` (one-shot commands) or `tui/` (bare `trak`). All side effects sit behind traits
(`Player`, `Library`, `AudioSource`, `Lyrics`, `Store`, `Clock`, `Notifier`) with fakes, so the whole
app runs in tests. State is pure: `update(App, Event) -> App`; `render(&App, Frame)` only draws.
Slow work runs on worker threads and returns as `Event`s. Details: `docs/ARCHITECTURE.md`.

## Conventions

- **Match the surrounding code** — comment density, naming, idiom. Comments explain *why*
  (a Spotify quirk, a macOS constraint), not what. The sibling projects' comments are a good tone
  reference.
- Errors: typed errors in libraries (`thiserror`), `anyhow` only at the binary edge. No `unwrap()`
  or `expect()` on external data (AppleScript output, HTTP, files, terminal). Never panic in the
  render or event loop; the panic hook must still restore the terminal.
- Never block the UI thread (osascript, HTTP, image decode, FFT all go on workers).
- Prefer small pure functions for anything that parses, formats, or lays out — they are the
  testable core. Snapshot-test layouts at several terminal sizes.
- Keep dependencies few and mainstream; every new crate must be MIT/Apache-2.0 compatible and
  recorded in `THIRD-PARTY-NOTICES.md`.
- No unrequested features, refactors, or abstractions beyond the task. If you see something worth
  doing, add it to `TODO.md` (Backlog) instead.

## Definition of done (every task)

1. The task's `Done when:` is observably true (run it; do not assume).
2. `fmt`, `clippy -D warnings`, `test`, and `build --release` pass.
3. New behaviour has tests (see the strategy table in `docs/ARCHITECTURE.md`). Anything you could
   only verify by hand is listed with exact steps.
4. Docs updated in the same commit if behaviour, keys, config keys, or architecture changed
   (`docs/SPEC.md` is the source of truth for product behaviour; the README only describes what is
   shipped).
5. `TODO.md` box ticked with a one-line note if a later agent needs to know something.
6. Committed and pushed.

## Verifying against real Spotify (manual, keep it short)

```sh
./spikes/verify.sh          # re-checks every Phase 1 spike claim; ~3 min, prints PASS/FAIL
./spikes/verify.sh --quick  # same, minus the slow tap and notification sections
```

**Read `spikes/verify.sh` before re-deriving anything in `docs/APPLESCRIPT.md`,
`docs/TERMINALS.md`, `docs/AUDIO-TAP.md` or `docs/KEYCHAIN.md`** — it runs the
spikes and compares their output against what those docs claim, and a FAIL means
the doc is wrong. It is the fastest way for a later agent to check its own work
against a claim it did not make. It reads Spotify and, in the 1.3 and 1.5
sections, pauses / seeks / skips a track, so it is not something to run while
someone is listening.

To check one thing by hand:

```sh
osascript -e 'tell application "Spotify" to return player state as string'   # via the shim in agent sessions
```

Use only when a task says so, and never in automated tests. Record real outputs as
fixtures under `tests/fixtures/` so future tests are hermetic.

## When you are stuck or something contradicts the docs

- The docs are wrong or a claim marked "verify" is false → fix the doc in the same commit and note it
  in `TODO.md`. Do not build on an unverified claim.
- A decision is genuinely missing → add it to `docs/SPEC.md` §10 "Open questions" and ask the
  owner only that question, with a recommended default.
- Spotify, Sonar, or headless-spotify behave differently than `docs/COMPAT.md` says → stop, record
  the observation with real output in `docs/COMPAT.md`, and adapt Trak (not the siblings).

## Repo map

```
AGENTS.md  CLAUDE.md  README.md  TODO.md  LICENSE  THIRD-PARTY-NOTICES.md  VERSION
Cargo.toml  rustfmt.toml  src/main.rs
scripts/          (created by task 9.1: package-release.sh)
Formula/trak.rb   (created by task 9.3)
.github/workflows/release.yml  (created by task 9.2)
docs/  SPEC.md  COMPAT.md  ARCHITECTURE.md  AGENT-PROMPTS.md
       TERMINALS.md + KEYCHAIN.md + WEB-API.md + AUDIO-TAP.md exist (1.3–1.7)
       (created by tasks: APPLESCRIPT.md KEYCHAIN.md TERMINALS.md AUDIO-TAP.md README-NOTES.md RELEASING.md)
.github/workflows/ci.yml
tests/fixtures/   (created by task 1.1)
spikes/          (throwaway spike crates: applescript/ notify/ keychain/ images/ viz/ tap/)
scripts/          (created by task 9.1: package-release.sh)
```
