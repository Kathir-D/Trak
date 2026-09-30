# Prompts for the sibling projects' agents

trak works with **Sonar** and **headless-spotify** with no changes to either, but a little
cooperation on their side gives a better experience (see `docs/COMPAT.md`). **Do not edit those
repos from a trak task.** The owner pastes these prompts into a session opened in each repo.

Neither change is a prerequisite for any trak task. trak falls back gracefully without them.
Record here when each prompt has been run (`Status:` line).

## Prompt 1 — Sonar (`projects/Sonar`)

Status: not yet run.

```
Add a small, additive integration for a new companion tool, "trak"
(github.com/Kathir-D/trak, a terminal UI for Spotify). Do not change the
auto-pause engine's decisions or the ownership rules in any way.

1. Publish engine state to ~/Library/Application Support/Sonar/state.json,
   written atomically (temp file + rename), on every FusionState/uiState
   change: {"v":1,"state":"idle|ducking|ducked|resuming","pid":<Spotify pid or
   null>,"since":<unix seconds>}. Create the directory if missing. Write
   "idle" on quit or when Auto-Pause is switched off. Failures to write must be
   logged and ignored, never surfaced or fatal.
2. Verify that Sonar still behaves correctly when another process changes
   Spotify's play state or volume (a manual pause or volume change from
   AppleScript must release ownership, as documented). Add a test for this if
   missing. Do not change the behavior; report anything that deviates.
3. README: add a short "Works with trak" note in Features/Credits and describe
   state.json (path, fields, that it is optional and versioned). Keep the
   existing README tone, and update the Contents list.
Commit in small conventional commits and push, per the global CLAUDE.md.
```

## Prompt 2 — headless-spotify (`projects/headless-spotify`)

Status: not yet run.

```
Add compatibility support for a new companion tool, "trak"
(github.com/Kathir-D/trak, a terminal UI for Spotify).

1. Add `headless-spotify launch`: start Spotify in the background without
   activating it (NSWorkspace.openApplication with activates:false, same as
   install.sh step 3), wait up to 10 s for `player state` to respond, exit 0 on
   success and 1 with a clear message on failure. It must be a no-op (exit 0)
   if Spotify is already running. It must never edit Info.plist or need sudo.
2. Lock the `status --json` output as a documented, versioned contract
   (add a "schema":1 field if absent; do not rename existing fields), because
   trak reads it to show a "headless" badge. Add a test that pins the fields.
3. README: add a "Works with trak" section and update the Sonar
   compatibility table into a "Companions" section covering Sonar and trak;
   document `launch`. Update the Contents list and the CLI usage text.
Keep it dependency-free. Commit in small conventional commits and push, per the
global CLAUDE.md.
```

## Prompt 3 — homebrew-tap (`projects/homebrew-tap`), when trak has a release

Status: not yet run. (CI in trak's release workflow, TODO 9.x, may do this automatically instead.)

```
Add trak to the tap: a formula Formula/trak.rb that downloads the release
tarball from github.com/Kathir-D/trak/releases (url + sha256 given by the
release), installs the `trak` binary, and has a `test do` block asserting
`trak --version` matches the formula version. Add a row for trak to README.md
(install: `brew install kathir-d/tap/trak`, uninstall, one-line description).
Run `brew audit --strict --online kathir-d/tap/trak` and `brew test trak`, and
fix findings. Do not add code signing steps.
```
