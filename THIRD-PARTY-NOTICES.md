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
