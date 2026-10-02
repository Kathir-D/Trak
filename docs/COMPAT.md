# Working with Sonar and headless-spotify

Trak is the third piece of the owner's music setup. This file is the **contract** between the
three. Sibling repos live next to this one in `projects/` (`../Sonar`, `../headless-spotify`); read
their READMEs before touching anything here. **Never edit a sibling repo from a Trak task** — write a
prompt for its own agent in `docs/AGENT-PROMPTS.md` instead.

| Project | Role | Repo |
| --- | --- | --- |
| headless-spotify | Hides Spotify from Dock + Cmd-Tab, keeps AppleScript working | https://github.com/Kathir-D/headless-spotify |
| Sonar | Menu-bar item: track, skip / previous, and auto-pause when other audio plays | https://github.com/Kathir-D/Sonar |
| Trak | Terminal UI + CLI for Spotify | https://github.com/Kathir-D/Trak |

All three talk to the **same Spotify process** (`com.spotify.client`) through the **same AppleScript
dictionary**. None of them needs the others installed.

## Rules Trak must follow

1. **Use the Spotify contract only**: bundle ID `com.spotify.client`, `tell application "Spotify"`.
   Never hardcode `/Applications/Spotify.app`, never rename or wrap the process.
2. **Never launch Spotify as a side effect.** One-shot commands and the TUI's polling only talk to
   an already-running Spotify. The single exception is the explicit "press enter to launch" on the
   idle card, which runs `headless-spotify launch` if that exists, else `open -g -j -a Spotify`
   (background, no focus steal, no Dock bounce).
3. **Do not break Sonar's ownership rules.** Sonar resumes music only if *it* paused it, and it
   gives up ownership on any manual pause, volume change, player restart, or quit. Therefore:
   - Trak writes to Spotify (play state, volume, position, shuffle, repeat) **only in direct
     response to a user key / command**. No startup writes, no background "sync" writes, no retries
     that write.
   - Reading is free. Poll `sound volume` and `player state` as often as needed.
   - Sonar fades volume while ducking. Trak must show the *user's* volume (the last value Trak
     knows the user chose), not a mid-fade reading, and must never write a mid-fade value back.
   - `m` (mute) saves the pre-mute volume and restores it. It is disabled while Sonar reports
     `ducking` / `ducked` / `resuming`, so the two cannot overwrite each other.
   - A pause or play issued from Trak is a manual action; Sonar correctly releases ownership. That
     is the desired behaviour.
4. **Live updates**: subscribe to the distributed notification
   `com.spotify.client.PlaybackStateChanged` (verify its userInfo keys in TODO 1.3). It fires for
   changes made by Sonar's buttons, the Spotify window, media keys, and headless Spotify. A slow
   poll (1 s while playing, 3 s otherwise) is the backup and also drives the progress bar.
5. **Read back after every volume write, but compare with a ±1 tolerance.** headless-spotify
   reported on 2026-09-28 that "Spotify ≥ 1.3.x ignores AppleScript volume sets". **That is not true
   on 1.3.1.234** — sets apply, verified 8/8 (see `docs/APPLESCRIPT.md` §5). What *is* true is
   that Spotify quantises the volume, so **the read-back is often exactly 1 lower than the value
   set** and is stable there. Trak must therefore distinguish "the write was ignored" (read unchanged
   at the pre-write value) from "the write landed, quantised" (read differs by ≤ 1), and must show
   the value Trak last *set* rather than the raw read, or the meter jitters by 1 % on every keypress.
   If a write is genuinely ignored, hide the meter and show a one-line notice; the system-volume
   fallback is then a config choice (`volume.control`), not an automatic repair (TODO 4.4).
6. **Permissions belong to the terminal app**, not to Sonar. The first AppleScript call from
   e.g. cmux triggers a one-time "control Spotify" prompt for that terminal. The visualizer's tap
   needs "System Audio Recording" for the terminal, once. Denied → simulated visualizer.
7. **AppleScript path**: call `/usr/bin/osascript` by absolute path, overridable with the env var
   `TRAK_OSASCRIPT`. (On the owner's dev machine, plain `osascript` from an agent session is
   routed through a shim; see `AGENTS.md` "Dev machine".)

## Optional integrations (fail soft — Trak must work perfectly without them)

### Sonar state file

Sonar (once its agent implements `docs/AGENT-PROMPTS.md` prompt 1) writes
`~/Library/Application Support/Sonar/state.json` atomically:

```json
{"v":1,"state":"idle|ducking|ducked|resuming","pid":12345,"since":1790000000}
```

Trak watches it (FSEvents or a 500 ms stat poll) and, when `state` is `ducking|ducked|resuming`
**and** `pid` matches Spotify's pid **and** the file is fresh (mtime within 10 minutes, or Sonar's
process is alive), shows `⏸ auto-paused by Sonar` in the header and treats the pause as Sonar's.
Missing file, unknown `v`, bad JSON, or stale data ⇒ ignore silently and show plain "paused".

### headless-spotify

If `headless-spotify` is on `PATH`:

- `headless-spotify status --json` → show a `headless` badge (schema is pinned by prompt 2; read
  only fields you know; ignore extras).
- `headless-spotify launch` → used by the idle card.

If Spotify has no window / Dock icon, Trak is the only display, so surface more (full Info tab, a key
to open the Spotify window via `open -a Spotify`).

## Status of the shared platform

- Spotify ≥ 1.3.1 quits on launch when `LSUIElement` is set, so **hiding does not currently work**
  (headless-spotify README, verified 2026-09-28). Trak therefore runs against normal Spotify today.
  When Spotify honours the key again, Trak needs no change.
- Sonar requires macOS 15+; headless-spotify macOS 15+; Trak macOS 14.2+. All in one machine ⇒ 15+.
- **Observed 2026-10-01:** on the owner's Mac `/Applications/Spotify.app` is owned by another
  local user (`dev`), so a *fresh launch* of Spotify blocks its main thread in
  `AuthorizationCopyRights` (`system.privilege.admin`, its updater) until someone answers a password
  dialog. Every Apple Event — Trak's, Sonar's, anyone's — times out meanwhile. Trak handled it as
  designed (timeout toast, idle card, no writes, reattached once the dialog was cancelled), but
  the idle card says "isn't running" for a Spotify that is running and stuck; a sharper line for
  that case is in TODO's Backlog. Not a Trak bug; the fix is `chown` of the bundle or a reinstall.

## Test matrix (fill in as verified; each row is a TODO 10.x task)

| Scenario | Expect | Verified |
| --- | --- | --- |
| Trak skip while Sonar running | Sonar's title updates within ~1 s | |
| Sonar skip button while Trak open | Trak updates within ~1 s | |
| Other app makes noise → Sonar ducks | Trak shows `auto-paused by Sonar`, volume meter not corrupted | |
| Noise stops → Sonar resumes | Trak returns to `playing` | |
| User pauses in Trak, then noise starts/stops | Sonar does **not** resume it | |
| User presses `m` during a duck | ignored with a hint | |
| Spotify quits while Trak open | idle card, no crash, no relaunch | ✅ 2026-10-01, Spotify 1.3.1.234, Trak `main`: `quit` via AppleScript → idle card at once, Spotify still not running 20 s later. `enter` on the card relaunched it (`headless-spotify launch`), and once it answered Trak reattached by itself: cover, paused state and position back, no restart of Trak. See the note below on what blocked the relaunch |
| Trak open with no Sonar / no headless-spotify | everything works, no errors | ✅ 2026-10-01: `PATH=/usr/bin:/bin`, `SONAR_STATE=/nonexistent` — the TUI draws with no badge, no toast, no hint about headless-spotify, and `trak status` exits 0 |
| Headless Spotify (when possible again) | badge shown, all controls work | |
