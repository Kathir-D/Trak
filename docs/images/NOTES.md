# Images

Screenshots taken with `screencapture` on the owner's machine, 2026-09-29. They
are evidence, not decoration: each one is a claim a spike or a task made.

| File | What it shows |
| --- | --- |
| `spike-1.4-cmux.png` | TODO 1.4: cmux renders the Kitty graphics protocol at full fidelity. Column 2's disc is perfectly round with thin grid lines; columns 3 and 4 (iTerm2, sixel) are empty. |
| `spike-1.4-terminal.png` | TODO 1.4: Terminal.app renders only half-blocks and prints the other three protocols' escape sequences as visible text. |
| `trak-demo.gif` | TODO 11.1: the README hero, re-recorded 2026-10-04 by `python3 scripts/record-demo.py`. It runs trak in a **pty** announced as Ghostty, so the cover goes out through the **Kitty graphics protocol** as it does in cmux; `screen.py` decodes the image and its placeholder cells and the frame shows the real cover at full resolution. Frames are taken at 20 fps while the keys are pressed and assembled by ffmpeg with one palette for the whole clip. Against `scripts/fake-spotify`, a two-song queue (Jane Remover, *Dancing with your eyes closed* and *Beauty Sleep*) with the real track ids: the covers come from Spotify's CDN (checked against `open.spotify.com/oembed` for each id), the accent is taken from them and the lyrics are the real synced ones from LRCLIB. Scenes, in 20 s: the cover and history, a skip that changes the cover and every colour, pause/play, shuffle, repeat, volume and mute, the visualizer through all four styles, the Lyrics tab, full-screen lyrics, Info, a skip back, settings. A track change or a pause is waited for **off camera**: real Spotify posts `PlaybackStateChanged` and trak shows it in ~170 ms, but the fake cannot post it without telling every other listener on the machine, so trak only sees it at the next 3 s poll. 0.7 MB. |
| `dashboard.png`, `visualizer-*.png`, `lyrics-fullscreen.png`, `settings.png` | TODO 11.2: stills from the same session (`--preview DIR` writes the fullest frame of every step). Version A screenshots need a live login **[owner]**. |
| `tui-3.3-wide.png` | TODO 3.3: the wide layout at 100×30 — header with status dot and clock, Now Playing with the art placeholder, interpolated progress bar, controls, volume meter, the tab strip, and the footer key hints. |

`tui-3.3-wide.png` is the honest current state: the art area is an empty framed
placeholder because album art is TODO 4.1. It is drawn rather than left blank so
the layout reads as designed and so the renderer already reserves exactly the
space the cover will need.
