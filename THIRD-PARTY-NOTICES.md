# Third-party notices

trak is MIT licensed (see `LICENSE`). It descends from, borrows from, or depends on the projects
below. Keep this file current: every crate or borrowed idea that ships in a release gets a row.
Run `cargo license` (or read `Cargo.lock`) before each release to check nothing is missing.

## Descends from

| Project | Author | License | What trak takes |
| --- | --- | --- | --- |
| [shpotify](https://github.com/hnarayanan/shpotify) | Harish Narayanan | MIT | The command set (`play`, `next`, `vol`, `status`, `share`, `toggle` ...) and the AppleScript approach. trak is a Rust rewrite, not a copy of the bash script. |

## Planned dependencies (confirm each when it is actually added, then move it to "Dependencies")

| Crate / project | License | Used for |
| --- | --- | --- |
| [ratatui](https://github.com/ratatui/ratatui) | MIT | TUI rendering |
| [ratatui-image](https://github.com/ratatui/ratatui-image) | MIT | Album art (Kitty / iTerm2 / sixel / half-blocks) |
| [cavacore](https://github.com/TornaxO7/cavacore-rs) (port of [cava](https://github.com/karlstav/cava)) | MIT | Visualizer spectrum maths |
| [cidre](https://github.com/yury/cidre) | MIT | Core Audio process tap (real-audio visualizer) |
| [rspotify](https://github.com/ramsayleung/rspotify) | MIT | Spotify Web API + PKCE (Version A) |

## Studied, not copied

| Project | License | Note |
| --- | --- | --- |
| [scope-tui](https://github.com/alemidev/scope-tui) | MIT | Approach to oscilloscope rendering in ratatui. If any code is copied, add it to "Dependencies" with its copyright line. |
| [spotify-tui](https://github.com/Rigellute/spotify-tui) | MIT | Feature reference only. trak's look and UX are deliberately different. |
| [spotify-player](https://github.com/aome510/spotify-player) | MIT | Feature reference only. |
| [LRCLIB](https://lrclib.net) | Public API | Synced lyrics source. Not a code dependency; respect their usage guidance and send a descriptive User-Agent. |

## Dependencies

_None yet._

## Verified during the Phase 1 spikes

These were checked against the actual crate metadata while the spike was written
(TODO 1.3, 1.4, 1.6, 1.8), not just listed speculatively.

| Crate | Version used | License | Used for | Confirmed |
| --- | --- | --- | --- | --- |
| [objc2](https://github.com/madsmtm/objc2) | 0.6 | MIT | `define_class!` for the notification observer | builds on stable, arm64 |
| [objc2-foundation](https://github.com/madsmtm/objc2) | 0.3.2 | MIT | `NSDistributedNotificationCenter` | ships `NSDistributedNotificationCenter`, selector-based `addObserver` only (no block variant) |
| [security-framework](https://github.com/lfittl/rust-security-framework) | 3.7.0 | MIT/Apache-2.0 | Keychain, **evaluated and rejected** — see `docs/KEYCHAIN.md` | built for the spike; not going in the release |
| [ratatui-image](https://github.com/benjajaja/ratatui-image) | 8.1.1 | MIT | Album art; `Picker::from_query_stdio` | the picker reports protocol **and** cell size; detects Kitty in cmux, half-blocks in Terminal.app |
| [cavacore](https://github.com/TornaxO7/cavacore-rs) (port of [cava](https://github.com/karlstav/cava)) | 2.0.2 | MIT | Visualizer spectrum maths | builds on stable for both `aarch64-apple-darwin` and `x86_64-apple-darwin` |
| [image](https://github.com/image-rs/image) | 0.25 | MIT/Apache-2.0 | Art decode | re-exported by ratatui-image; needed directly by the spike |
| [crossterm](https://github.com/crossterm-rs/crossterm) | 0.28 | MIT | Terminal control | |

Notes:

- `cavacore` is a port of [cava](https://github.com/karlstav/cava), also MIT. The
  ideas behind the `waveform` renderer (TODO 8.2) come from
  [scope-tui](https://github.com/alemidev/scope-tui) — credit that when the code is
  written, not before.
- `security-framework` is listed here because it was evaluated and the answer was
  "no" (TODO 1.8). It should **not** be added to `Cargo.toml`. This row is
  deliberately in the spike section rather than the dependency list.
