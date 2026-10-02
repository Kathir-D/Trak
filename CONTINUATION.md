# Trak — continuation prompt

Paste everything below into a fresh session. It is self-contained: you do not need the conversation
that produced it, and you should not try to reconstruct it.

---

## How to work

The owner's standing instructions are in `AGENTS.md` under "⚠ Working style" (work `TODO.md` in
order, subagents with strict file boundaries, commit and push constantly, never stall, never ask
mid-flight, re-read your diff). They outrank your preferences. Read that section first, then
`docs/SPEC.md` (wins over TODO), `docs/COMPAT.md` (non-negotiable) and `TODO.md`'s ticked notes.

## Current state (end of the 2026-10-02 session)

**Trak 0.1.1 is released** (`main`, tag `v0.1.1`, CI green) and installs two ways, both recorded
verbatim in `docs/RELEASING.md` §11:

- `brew install kathir-d/tap/trak` — a **formula**, not a cask: one `trak` command in Homebrew's
  `bin`, no app, no quarantine, `brew audit --strict --online` and `brew test` clean.
- `curl -fsSL https://raw.githubusercontent.com/Kathir-D/Trak/main/install.sh | sh` — checks the
  SHA-256, installs to `/usr/local/bin` if writable else `~/.local/bin`, never `sudo`.

v0.1.0 was published first and its formula failed `brew audit --strict` (a `version` line redundant
with the URL), so 0.1.1 is the first good release. The binary is identical. Never move a published
tag: fix, bump, tag again. The release workflow bumps `Formula/trak.rb` on `main` and in the tap
(`Kathir-D/homebrew-tap`, via the `TAP_DEPLOY_KEY` deploy key; secret set, key files deleted).
**`git pull` before touching `Formula/trak.rb`** — a bot commit lands there after every tag.

Phases 1–9 and 11.0, 11.4–11.7 are ticked. Done this session beyond the TODO: the selected tab is
kept centred in a too-narrow tab strip; full-screen lyrics draw over the darkened cover (cell
backgrounds, `Images::backdrop`); light terminal backgrounds are detected with OSC 11 and pale
cover colours darkened (`tui/colour.rs`); RTL titles keep the layout (`tui/bidi.rs`, U+2800 fence);
the demo GIF and stills use Jane Remover's "Dancing with your eyes closed".

### What is left (all need the owner)

| Task | What |
| --- | --- |
| 10.1–10.3, 10.6, 10.7 | The Sonar / headless-spotify rows of `docs/COMPAT.md`, run on the owner's Mac with Sonar granted its permissions. Sonar's and headless's `badge` are computed but **not drawn** (COMPAT says so) |
| 11.1 | A GitHub light/dark render check of the README; the at-a-glance table; Version A screenshot |
| 11.2 | Version A screenshots, which need a live Spotify login |
| 11.3 | Upload the social preview image (no API for it), enable private vulnerability reporting |
| 9.6 | Install on a Mac that never had Homebrew or a Spotify Automation grant, to see the first-run prompt |

**Never verified against a live account** (say "unverified" until the owner has): every Web API tab,
playlist item reads and writes in dev mode, the login itself (does the dashboard accept the no-path
redirect `http://127.0.0.1`), add-to-queue's Premium 403. The `[owner]` steps are in the 7.x notes.
Also unmeasured: battery drain over hours (CPU is under 1% idle, 3.9% with the visualizer).
Also never exercised in a real terminal: the idle card (it needs Spotify quit).

**Privacy:** the whole-desktop screenshots (`tui-6.3-tab.png`, `tui-6.4-fullscreen.png`) were purged
from history on 2026-10-02 with `git filter-repo` and a force-push of `main`, `v0.1.0` and `v0.1.1`
(every commit from `b5f32d0` on has a new SHA; trees are identical, so the releases, the formula's
checksum and both installers are unaffected). **GitHub still serves the old commits by SHA**
(`raw.githubusercontent.com/Kathir-D/Trak/<old sha>/docs/images/tui-6.4-fullscreen.png` returned 200
afterwards); only GitHub Support can drop them, via a "remove cached views / run garbage collection"
request, which needs the owner's account **[owner]**. Old SHA to quote: `b5f32d0b72d2aee7feca199d86ceaf130f6b7a1b`.
Lesson if you ever redo this: `filter-repo` drops `origin`; and **disable the Release workflow
before pushing moved tags**, because a `v*` tag push rebuilds the tarball and its checksum would
then disagree with the tap's formula.

## Tools and traps (what cost time this session)

- **No TTY, so drive cmux.** `screencapture -x -o -l <window id>` of the cmux window only (never the
  whole desktop); crop the sidebar out of anything committed. The window id changes; find it with
  `osascript`/`System Events`. Type with a raw keycode helper (`cliclick` drops keys, System Events
  keystrokes hang); mouse with `cliclick`. A cell is about 7.8 × 17 px.
- **Always the scratch config:** `XDG_CONFIG_HOME=$PWD/target/scratch/xdg`. For real Spotify use
  `TRAK_OSASCRIPT="$HOME/.local/bin/osascript"`; for a fake one point `TRAK_OSASCRIPT` at a script
  that prints a fixture from `tests/fixtures/applescript/`. The owner may be listening: read the
  state first, restore volume, shuffle, repeat and the track afterwards, and never launch Spotify.
- **The worker refuses jobs while busy.** `update()` returns commands for the loop's `pending`
  queue; a result that is dropped on the floor (`WorkerResult::Command` once ignored the commands
  `CommandDone` returned) silently kills a volume drag. Test through the loop, not just `update`.
- **ratatui rewrites cells after a Kitty image every frame** (the placeholder's `unicode-width`
  overshoots), ~15 KB/s while paused. `draw_if_changed` skips a frame identical to the last flushed.
  Anything that must repaint (resize, accent change, covering overlay) sets `last_frame = None`.
- **cmux specifics:** it runs the Unicode bidi algorithm per row (see `tui/bidi.rs`), ignores LRM,
  answers OSC 11 only when the query ends in ST (`\e\\`), not BEL. Reset with OSC 110 / 111.
- **Profiling:** `CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false cargo build
  --release --target-dir target/prof`, then `sample <pid> 5`. dtrace is blocked by SIP.
- **The demo GIF** is built with Python PIL (no ffmpeg): a `screencapture` loop, crop the terminal,
  dedupe identical frames, quantise to 128 colours; `docs/images/NOTES.md` has the recipe. Keep it
  under 3 MB.
- Gate before every commit: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test`, `cargo build --release`. `cargo` may need `export
  PATH="$HOME/.cargo/bin:$PATH"`. A `trak-local/audit` Homebrew tap exists on this Mac for running
  `brew audit --strict` against `Formula/trak.rb` before a tag.
- Sibling repos (`../Sonar`, `../headless-spotify`) are read-only. `../homebrew-tap` is read-only
  except the one-time README row, which is done; do not hand-edit `Formula/trak.rb` in it.
