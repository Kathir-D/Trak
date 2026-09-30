# AGENTS.md — trak

Instructions for any coding agent (Claude Code, opencode, Codex, ...) working in this repo. You are
assumed to have **no prior context**. Read this file fully, then follow "Start here".

## What this project is

`trak` is a Rust + ratatui terminal UI and CLI that controls the **official Spotify desktop app on
macOS** (through AppleScript, so the Free tier works with zero setup) and, optionally, the Spotify
Web API (with a user-supplied Client ID) for search, playlists, queue and library. It is a Rust
rewrite and expansion of [shpotify](https://github.com/hnarayanan/shpotify) (MIT), keeps all of its
commands, and adds a rich TUI, a music visualizer, and synced lyrics. It ships as a **Homebrew
formula** with **no paid code signing**.

It is one piece of the owner's music setup, alongside two sibling projects in `../`:
**headless-spotify** (hides Spotify from the Dock) and **Sonar** (menu-bar skip/prev + auto-pause).
trak must coexist with both. That contract is `docs/COMPAT.md` and is non-negotiable.

**Status:** pre-alpha. Only a scaffold exists (`src/main.rs` prints `--version`). Everything else is
planned in `TODO.md`.

## Start here (read in this order)

1. `docs/SPEC.md` — every product decision, the layout, keys, config schema, CLI. Already decided
   by the owner in a long design interview. **Do not re-ask; do not redesign.**
2. `docs/COMPAT.md` — how trak must behave next to Sonar and headless-spotify.
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
- Sibling repos (`../Sonar`, `../headless-spotify`, `../homebrew-tap`) are **read-only for trak
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
  never need it for trak.
- **AppleScript from an agent session**: plain `osascript` in `~/.local/bin` is a shim routed
  through `asrun` (which holds the Automation/Accessibility grants). Plain calls to
  `/usr/bin/osascript` from the agent process are silently denied (error -1743). trak calls
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
osascript -e 'tell application "Spotify" to return player state as string'   # via the shim in agent sessions
```
Use only when a task says so, and never in automated tests. Record real outputs as fixtures under
`tests/fixtures/` so future tests are hermetic.

## When you are stuck or something contradicts the docs

- The docs are wrong or a claim marked "verify" is false → fix the doc in the same commit and note it
  in `TODO.md`. Do not build on an unverified claim.
- A decision is genuinely missing → add it to `docs/SPEC.md` §10 "Open questions" and ask the
  owner only that question, with a recommended default.
- Spotify, Sonar, or headless-spotify behave differently than `docs/COMPAT.md` says → stop, record
  the observation with real output in `docs/COMPAT.md`, and adapt trak (not the siblings).

## Repo map

```
AGENTS.md  CLAUDE.md  README.md  TODO.md  LICENSE  THIRD-PARTY-NOTICES.md  VERSION
Cargo.toml  rustfmt.toml  src/main.rs
docs/  SPEC.md  COMPAT.md  ARCHITECTURE.md  AGENT-PROMPTS.md
       TERMINALS.md + KEYCHAIN.md + WEB-API.md + AUDIO-TAP.md exist (1.3–1.7)
       (created by tasks: APPLESCRIPT.md KEYCHAIN.md TERMINALS.md AUDIO-TAP.md README-NOTES.md RELEASING.md)
.github/workflows/ci.yml
tests/fixtures/   (created by task 1.1)
spikes/          (throwaway spike crates: applescript/ notify/ keychain/ images/ viz/)
scripts/          (created by task 9.1: package-release.sh)
```
