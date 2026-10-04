# Trak — continuation prompt

Paste everything below into a fresh session. It is self-contained: you do not need the conversation
that produced it, and you should not try to reconstruct it.

---

## How to work

The owner's standing instructions are in `AGENTS.md` under "⚠ Working style" (work `TODO.md` in
order, subagents with strict file boundaries, commit and push constantly, never stall, never ask
mid-flight, re-read your diff). They outrank your preferences. Read that section first, then
`docs/SPEC.md` (wins over TODO), `docs/COMPAT.md` (non-negotiable) and `TODO.md`'s ticked notes.

## Current state (end of the 2026-10-03 session)

**Trak 0.2.0 is released** (`main`, tag `v0.2.0`, CI green). It exists because the owner ran 0.1.1
on a second machine and reported three things: a dashboard with a third of the screen empty, "it
feels slow", and a crash in the settings screen. `TODO.md` phase 12 is the record, one note per
item.

What changed, and what to know before touching it again:

- **The layout is responsive.** The Now Playing pane is as wide as its content needs — a square
  cover is about twice as many cells across as it is down, so the height it is given decides the
  width (`now_playing_width`, within 30 cells and 50 %). The cover takes every row the text does not
  need, up to the height at which it fills that width; the surplus is split evenly above and below
  the block. A tab pane with a short block in it centres it. Checked 30x8 to 256x90.
- **Input latency was four separate bugs**, all measured on a pty with a fake `osascript`: one
  terminal event read per frame (673 ms → 139 ms for six keys), keys dropped while a write was in
  flight (now held; volume steps add up), the meter and the bar waiting for AppleScript (now they
  move on the keypress), and a volume keypress costing three full state reads (`Player::volume`
  reads one property: ~1.5 s → 302 ms to the write).
- **A crash, found by fuzzing**: the idle card is 34 cells wide and was drawn into a 31-cell
  terminal, and ratatui panics on an index outside the buffer rather than clipping. **Fuzz the TUI
  in a pty** — see the traps below; it is how that was found and how the next one will be.
- **The redirect URI changed to `http://127.0.0.1:8888/callback`.** 0.1.1 told users to register
  `http://127.0.0.1` and the dashboard refuses that form ("This redirect URI is not secure",
  measured). A login prefers the registered port and falls back to an ephemeral one. Anyone who
  added a Client ID before 0.2.0 has to add this URI to their app.
- **Gradients are cached** by (palette, width), and the bar's ramp drifts one cell every half
  second while a track plays (still when paused). The premade ratatui gradient crates are thin
  wrappers around `colorgrad` and none of them draw a bar with a moving head.
- **The demo GIF is recorded from a pty**, not from a window: `scripts/record-demo.py` plus
  `scripts/screen.py`'s emulator (which decodes the Kitty graphics trak sends, so the cover is the
  real image) plus PIL and ffmpeg. No window id, no focus, reproducible. It runs against
  `scripts/fake-spotify`; the scene list is at the top of the script.

Still unverified, and it needs the owner: **the whole Web API half.** The Client ID works and the
authorize endpoint accepts the redirect URI, but nobody clicked "Allow" in the browser yet, so
search, playlists, queue, library and the token refresh have never met a real account. The app
also needs **Web API** ticked under "APIs used" (the first attempt was created with only Web
Playback SDK), and the account that owns it needs Premium.

Also still open: the Sonar / headless-spotify rows of `docs/COMPAT.md` (phase 10), the README's
light/dark render check and Version A screenshots (11.1, 11.2), the social preview image (11.3),
and installing on a Mac that never had Homebrew or an Automation grant (9.6).

## Tools and traps (what cost time this session)

- **Fuzz the TUI in a pty.** `target/scratch/lat/` has the harness (not committed):
  `fuzz.py` sends random keys at odd sizes and colour depths and reports anything that dies,
  panics or fails to leave the alternate screen; `drive.py` times a burst of keys and when a write
  reaches the stub; `stub-osascript` answers in the measured times; `vt.py`/`shot.py` print the
  screen as text. **Put a fake `open` first on `PATH`** (`target/scratch/lat/bin/open`) before
  anything sends `
`: the guided setup's first step opens a real browser, and a fuzz run did
  exactly that (2026-10-02, several Safari tabs).
- **`?`/`q` in the settings screen do not quit trak** — `q` saves and closes, `esc` closes, and a
  text field swallows `q` entirely. A fuzzer that ends with "still running after three q's" is
  reporting its own key sequence, not a bug; make sure the run is not sitting in a text field
  before believing it.
- **Screen size has to be set before `execv`** (ioctl on fd 0 in the child). Trak asks the
  terminal once at startup and only redraws on a change, so a size applied afterwards leaves a
  screen drawn for the pty's default 80x24 with the rest of it never painted.
- **Do not answer trak's OSC 11 background query from the harness.** The reply goes into the
  pty's input, where the line discipline may echo it into the frame and trak reads it as
  keypresses. Let the query time out (300 ms).
- **cmux cannot be driven from an agent session**: `cmux` says "only processes started inside cmux
  can connect". That is why the GIF is recorded from a pty instead.
- **The Spotify login needs a human.** trak opens Safari and waits on the browser's "Allow"; the
  owner has to click it. Everything either side of that click is automated here.
- Always the scratch config: `XDG_CONFIG_HOME=$PWD/target/scratch/xdg`. Note trak's file lives
  at `<XDG_CONFIG_HOME>/trak/config.toml`, not directly in it. For real Spotify use
  `TRAK_OSASCRIPT="$HOME/.local/bin/osascript"`. The owner may be listening: read the state first,
  restore volume, shuffle, repeat and the track afterwards, and never launch Spotify.
- `scripts/fake-spotify` is the scripted player: a two-song queue whose writes (play/pause,
  next/previous, shuffle, repeat, volume, seek) change what the next read returns, state in a JSON
  file, every call logged with a timestamp. Its track ids and artwork ids are the real ones -- an
  earlier copy pointed at another song's artwork (*Cage Girl / Camgirl*) and the demo showed it; check each with
  `curl "https://open.spotify.com/oembed?url=https://open.spotify.com/track/<id>"`. It reports a
  `spotify:track:` URI, or trak treats the track as an advert and never looks its lyrics up.
- **Profiling:** `CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false cargo build
  --release --target-dir target/prof`, then `sample <pid> 5`. dtrace is blocked by SIP.
- **Gate before every commit:** `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test`, `cargo build --release`. `cargo` may need `export
  PATH="$HOME/.cargo/bin:$PATH"`. `cargo fmt --all` (without `--check`) is how this repo is
  formatted after an edit.
- **`brew install --formula ./Formula/trak.rb` does not work on this machine** — Homebrew refuses a
  formula outside a tap. `docs/RELEASING.md` §1 step 4 therefore needs the tap route; the
  equivalent check done on 2026-10-03 was `brew audit --strict --online kathir-d/tap/trak`, plus
  extracting the tarball and running `--version` on it.
- Sibling repos (`../Sonar`, `../headless-spotify`) are read-only. `../homebrew-tap` is read-only
  except the one-time README row, which is done; do not hand-edit `Formula/trak.rb` in it.
