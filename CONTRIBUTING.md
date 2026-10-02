# Contributing to Trak

Thanks for looking. The short version: read [`AGENTS.md`](AGENTS.md) (it is written for people and
coding agents alike), pick something from [`TODO.md`](TODO.md), and keep the change small.

## Before you start

- `docs/SPEC.md` is the product; it was decided up front, so please open an issue before proposing
  a different behaviour rather than a pull request that changes it.
- `docs/COMPAT.md` is not negotiable: Trak must coexist with Sonar and headless-spotify. It never
  launches Spotify as a side effect and writes to Spotify only in answer to a key or command.
- macOS 14.2+ only. Developing on another OS works for the logic, but anything touching AppleScript
  has to be checked on a Mac.

## The checks

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test            # needs no Spotify, no network and no audio device
cargo build --release
```

All four must pass; CI runs them on `macos-15`.

## Conventions

- Match the surrounding code, including comment density. Comments say *why* (a Spotify quirk, a
  macOS constraint), not what.
- Parsers, formatters and layout are small pure functions with tests. Anything slow runs on a
  worker thread and returns as an `Event`; the UI thread never blocks.
- No `unwrap()` or `expect()` on external data (AppleScript output, HTTP, files, the terminal).
- Optional integrations fail soft: degrade, say why in one line, never panic.
- Conventional commits (`feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `ci:`, `chore:`), one task
  per commit. New dependencies must be MIT/Apache-2.0 compatible and listed in
  `THIRD-PARTY-NOTICES.md`.
- Update the docs in the same change if behaviour, keys or config change.
