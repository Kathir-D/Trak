# Changelog

All notable changes to Trak are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Trak uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). How a release is cut
is in [`docs/RELEASING.md`](docs/RELEASING.md).

## [Unreleased]

Reports from the owner running 0.2.0 on the same second machine: a cover a third of
its pane, a slider too small to hit, lyrics that would not stay on screen, an
`enter` that did nothing, and an empty Queue tab.

### Fixed

- **The cover is sized by the cell size the terminal reports** (`CSI 16 t`), not by
  one measured once in one terminal at one font size. A square cover is about twice
  as many cells across as it is down, so the assumed cell size made the art a third
  of the size it should have been on a machine with a different font -- leaving a
  third of the screen empty under it. Ghostty, cmux, kitty, iTerm2, WezTerm and
  xterm all answer; the old default stands when nothing does.
- **The volume slider can be hit.** The clickable band is now the meter's row, the
  row above, the two rows below (the gradient rule and the pane border) and two
  cells either end. Where you click is still the volume you get.
- **Lyrics stay put.** Polling reset the lyrics state on every poll rather than on
  a track change, and a poll arrives about once a second repeating the same track:
  the pane blinked between the words and the song's title, threw away the follow
  position every second, and re-sent the same LRCLIB lookup over and over.
- **The guided setup opens the dashboard.** It called a bare `open`, which is not
  on `PATH` when trak starts from a terminal that read no profile, a launcher, or
  `sudo`; it now calls `/usr/bin/open`. The "could not open a browser" message was
  also cleared by the next keystroke, so a user who pressed enter and pressed
  something else never saw it. A notice now survives moving around the panel.

- **A 410 from the personal lists explains itself.** Measured on a real account:
  in development mode with the account not on the app's user list, Spotify answers
  410 Gone to the playlists and liked-songs reads and 403 to the queue -- the same
  cause in two costumes. It now says to add the account to the app's user list
  rather than "answered HTTP 410".

- **A 410 from the personal lists explains itself.** Measured on a real account:
  in development mode with the account not on the app's user list, Spotify answers
  410 Gone to the playlists and liked-songs reads and 403 to the queue -- the same
  cause in two costumes. It now says to add the account to the app's user list
  rather than "answered HTTP 410".

- **The seek keys are gone.** `h`/`l` no longer seek, and `[input] seek_step` has gone with them:
  the owner called a second way to move the playhead "kinda stupid" next to arrows that are
  navigation and a bar you can click. A `seek_step` left in a config file is ignored rather than
  refused, so existing files keep working.
- **Stale cover cells.** A Kitty placement covers exactly the cells it was encoded for, so a new
  cover at a new size left the old placement on the terminal -- grey placeholder boxes that ratatui
  cannot write over, because they are the image's own. A re-encoded cover now repaints the screen in
  the same frame.
- **A bar that flows instead of ticking.** The ramp slid a whole cell twice a second, so every bead
  changed colour at the same instant and the row read as a colour counting down. It now reads the
  ramp at a fractional position and blends, and the travelling white highlight is gone: the playhead
  is the edge. The beads stay.
- **The focused tab is the album's gradient** rather than one flat accent, as is the Library tab's
  section strip -- a focused tab that reads as "a grey tab that happens to be highlighted" is not a
  focus indicator.
- **`↑`/`↓` focus into a tab and out of it**, and `esc` leaves the list before the tab. An open
  album, artist or playlist page's tracks were reachable only by mouse: the arrows were moving the
  tab's hidden cursor. Fixed.
- **One cover, one colour scheme.** The accent and the gradient ramp came from two different colour
  extractors, so a track change could leave the bar on the new cover's colours and the borders on
  the old one's; and a cover with no usable colour kept the *previous* ramp while the accent fell
  back to green. Both now come from the same extraction of the same cover, so a track change moves
  every colour at once, and a cover with no hue falls back to one flat colour everywhere.
- **A selected tab is never a white box.** The tint behind the focused title is the album's own
  colour taken toward black (or lifted, for a nearly black cover), deepening along the run. Found on
  the way: `accent::mix` returned its first argument whenever either colour was a *named* one like
  `Color::Black`, so every mix in the program towards white or black had been doing nothing at all.
- **The right pane's border is lit from one side** instead of being flat grey, using the album's own
  colours -- top and left in one, bottom and right in another. The lyrics text is unchanged: lines
  not yet sung stay dim, the one being sung stays in the accent.
- **Focus is visible.** The side pane's border is `DIM` when unfocused and the plain accent when the
  arrows are driving its list. With the default green palette the two were previously the same
  colour to the byte, so focusing a list did nothing you could see.
- **`↑`/`↓` and the picture.** Swept seven terminal sizes at three cell sizes: the cover is always
  inside its pane, fills it to within a cell, and keeps its aspect; the text under it is the same
  number of rows whatever the window is, because a terminal cannot scale text.

- **`scripts/preview`** builds the working tree and runs it, so UI changes can be seen without a
  release.

### Changed

- **The Queue tab is never just empty.** What Spotify will play next is not
  knowable by trak -- only Spotify knows, and it only tells the Web API on Premium
  -- so an empty queue read now says that in one line and then shows the songs trak
  has seen play this session, newest first, capped, under a heading that does not
  pretend to be a queue. Premium accounts still get the real queue.

## [0.2.0] - 2026-10-03

Reports from the owner running 0.1.1 on a second machine, at a different terminal
size: a dashboard with a third of the screen empty, keypresses that felt slow, and
a crash in the settings screen.

### Fixed

- **A crash.** The idle card (`Spotify isn't running`) is 34 cells wide and was
  drawn into a narrower terminal without a clamp, so ratatui panicked on an index
  outside the buffer. Found by fuzzing the whole TUI in a pty across terminal
  sizes and key streams.
- **Keys are acted on in the frame they arrive in.** Every queued terminal event
  is read per frame and the frame wait is skipped after input; a command pressed
  while a write is in flight is held rather than dropped, with volume steps
  adding up. Six keys and a quit: **673 ms -> 139 ms**. A held key used to repeat
  three times a second and half of every burst of keypresses was silently
  discarded.
- **The volume meter and the progress bar move on the keypress** instead of after
  two AppleScript round trips, and a volume keypress reads one property rather
  than the 17-field batch: the write now reaches Spotify **302 ms** after the key
  (was about 1.5 s).
- **Lyrics wrap in the side tab.** A long line was cut off at the pane's edge with
  the rest of the words unreachable.
- A config value longer than the comment column gets a space before its comment.

### Changed

- **The dashboard has no dead space at any size.** The Now Playing pane is as wide
  as its content needs (a square cover is about twice as many cells across as it
  is down, so the height it is given decides the width), the cover takes every row
  the text does not need up to the height at which it fills that width, and the
  surplus is split evenly above and below the block. A tab pane with a short
  block in it centres it. Checked from 30x8 to 256x90.
- **The visualizer loses its gradient spine** -- a second gradient beside a
  gradient, at the cost of two columns of the pane -- and takes the cover's whole
  rectangle.
- **Gradients are cached** by palette and width, so a few hundred HSL
  interpolations a frame became a memcpy. The bar's ramp now drifts one cell
  every half second while a track plays, and is completely still when paused.
- **The guided Spotify setup is written for a first-timer**: what to click, what to
  paste and what "it worked" looks like, in the dashboard's own words. The panel
  scrolls to the step you are on. Every Web API notice names the **keys**
  (`press , then s`) rather than a command.
- **Register `http://127.0.0.1:8888/callback` as the redirect URI.** 0.1.1 said to
  register `http://127.0.0.1`, on the strength of Spotify's guide prose about
  registering a loopback literal "without any port number" -- but the dashboard
  refuses that form ("This redirect URI is not secure", measured 2026-10-02) while
  the guide's own examples all carry a port. **If you added a Client ID before
  0.2.0, add this URI to your app in the Spotify dashboard.** A login prefers that
  exact port and falls back to an ephemeral one, which Spotify allows for loopback
  literals. `docs/WEB-API.md` §1 is corrected.

### Still unverified

Every Web API tab, the login itself and the token refresh: the Client ID and the
redirect URI are accepted by Spotify, but nobody has completed a browser approval
yet, so search, playlists, queue and library have still never met a real account.

## [0.1.1] - 2026-10-02

### Fixed

- The Homebrew formula no longer carries a `version` line, which
  `brew audit --strict` rejects as redundant with the URL. Nothing about the
  binary changed from 0.1.0.

## [0.1.0] - 2026-10-02

The first release. Pre-alpha: everything listed here is built and tested against
fakes and recorded replies, and the parts marked **unverified** have not yet met
a real account or a real permission prompt.

### Added

- **Every [shpotify](https://github.com/hnarayanan/shpotify) command**: `status
  [artist|album|track]`, `play`, `pause`, `stop`, `quit`, `next`, `prev`,
  `replay`, `pos`, `vol up|down|<n>|show`, `toggle shuffle|repeat`, `share
  url|uri`, `play uri <uri>`, plus `--plain` and `--json`. Plain output when
  piped, `NO_COLOR` respected, exit codes 0 / 1 / 2.
- **The TUI** (`trak` with no arguments): Now Playing with a clickable progress
  bar, volume meter, shuffle and repeat; a layout that adapts from wide to
  stacked to a compact strip; an idle card when Spotify is not running that
  launches it in the background only when you press enter; mouse support
  (click the bar to seek, drag the volume, click a tab or a button); `←`/`→` and
  `tab`/`shift-tab` move between tabs, and a strip too narrow for all of them
  keeps the selected one centred between its neighbours.
- **Album art** in kitty / Ghostty / cmux (Kitty graphics) and half-blocks
  elsewhere, cached on disk, with the accent colour taken from the cover.
- **Synced lyrics** from [LRCLIB](https://lrclib.net) in a tab and a full-screen
  page (`L`) drawn over the darkened cover, cached on disk.
- **Visualizer** with four styles (`v`), drawn from Spotify's own audio through a
  Core Audio process tap on Spotify alone (never the rest of the system). It runs
  only while the visualizer is on screen, reattaches after Spotify restarts, and
  falls back to a simulated spectrum with a one-line explanation if macOS refuses
  the tap. **The refusal path is tested only with fakes**: the Macs measured so
  far grant the tap with no prompt.
- **History and Info tabs**: tracks played this session (enter plays one again)
  and everything AppleScript exposes about the current track.
- **Settings** (`,` or `?` in the TUI, or `trak config`): a checklist that
  applies live and writes `~/.config/trak/config.toml`, with every key listed
  under it.
- **Terminals of every kind**: `NO_COLOR`, 256- and 16-colour terminals, light
  backgrounds (asked with OSC 11; pale cover colours are darkened to stay
  readable), right-to-left titles that keep the layout in place, and sizes from
  a one-line strip up. Idle CPU is under 1% and a paused screen writes nothing.
- **Version A, with a Spotify Client ID** (guided setup: `trak config`, then
  `s`): live grouped search, playlists (open, add, create, remove), queue and
  add-to-queue, liked songs and `f` to like, library, artist and album pages,
  and `trak play <song|album|artist|list>`. **Unverified against a live
  account**: it is tested against a fake library and recorded replies only, and
  no real login has been made yet.
- **Works beside [Sonar](https://github.com/Kathir-D/Sonar) and
  [headless-spotify](https://github.com/Kathir-D/headless-spotify)**: shows
  Sonar's ducking and keeps the volume keys out of its way, shows a `headless`
  badge, and never writes to Spotify except in answer to a key you pressed.
  The live compatibility matrix in `docs/COMPAT.md` is still being filled in.
- **Install**: a Homebrew formula (`brew install kathir-d/tap/trak`) and a curl
  installer (`install.sh`) that verifies the release's sha256. The binary is
  universal (arm64 + x86_64) and **ad-hoc signed, not notarized**; neither
  install path sets a quarantine flag, so there is no Gatekeeper prompt.

[Unreleased]: https://github.com/Kathir-D/Trak/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Kathir-D/Trak/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Kathir-D/Trak/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Kathir-D/Trak/releases/tag/v0.1.0
