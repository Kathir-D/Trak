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

All MIT or MIT/Apache-2.0, checked against the crate metadata in `Cargo.lock`.

| Crate | Version | License | Used for | Note |
| --- | --- | --- | --- | --- |
| [clap](https://github.com/clap-rs/clap) | 4 | MIT/Apache-2.0 | CLI parsing | the derive API, so the shpotify commands are one table |
| [thiserror](https://github.com/dtolnay/thiserror) | 2 | MIT/Apache-2.0 | typed errors | the binary edge is the only place `anyhow` would be allowed |
| [ratatui](https://github.com/ratatui/ratatui) | 0.29 | MIT | TUI rendering | |
| [crossterm](https://github.com/crossterm-rs/crossterm) | 0.28 | MIT | terminal and input events | mouse capture, bracketed paste, the run-loop-safe read |
| [objc2](https://github.com/madsmtm/objc2) | 0.6 | MIT | Objective-C runtime | the `PlaybackStateChanged` observer; no app bundle needed |
| [objc2-foundation](https://github.com/madsmtm/objc2) | 0.3 | MIT | Foundation bindings | only the notification classes, nothing else |
| [ratatui-image](https://github.com/ratatui/ratatui-image) | 8.0.1 | MIT | Album art | Kitty / iTerm2 / sixel / half-blocks; `Picker::from_query_stdio()` also reports the cell size (TODO 1.4) |
| [image](https://github.com/image-rs/image) | 0.25 | MIT/Apache-2.0 | decoding cover art | `load_from_memory`, not `open`: the cache is called `.img` and the format is read from the content |
| [ureq](https://github.com/algesten/ureq) | 3 | MIT/Apache-2.0 | fetching cover art | blocking, on the worker thread; default features are rustls + webpki-roots, so there is no OpenSSL and no system trust store to go stale |
| [assert_cmd](https://github.com/assert-rs/assert_cmd) | 2 | MIT/Apache-2.0 | CLI tests (dev only) | |
| [predicates](https://github.com/assert-rs/predicates) | 3 | MIT/Apache-2.0 | CLI test assertions (dev only) | |
| [unicode-width](https://github.com/unicode-rs/unicode-width) | 0.2 | MIT/Apache-2.0 | asserting bars are one cell per character (dev only) | |
| [filetime](https://github.com/alsdy/filetime) | 0.2 | MIT/Apache-2.0 | setting file times in the cache-pruning test (dev only) | |

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
| [cidre](https://github.com/yury/cidre) | 0.29 | MIT | Core Audio process tap (TODO 1.5/8.3) | **needs `default-features = false`** plus an rpath to `/usr/lib/swift`, or the binary will not launch on macOS 27 — see `docs/AUDIO-TAP.md` §3e. Its process-specific tap descriptions currently fail; see §3b |

Notes:

- `cavacore` is a port of [cava](https://github.com/karlstav/cava), also MIT. The
  ideas behind the `waveform` renderer (TODO 8.2) come from
  [scope-tui](https://github.com/alemidev/scope-tui) — credit that when the code is
  written, not before.
- `security-framework` is listed here because it was evaluated and the answer was
  "no" (TODO 1.8). It should **not** be added to `Cargo.toml`. This row is
  deliberately in the spike section rather than the dependency list.
