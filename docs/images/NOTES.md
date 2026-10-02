# Images

Screenshots taken with `screencapture` on the owner's machine, 2026-09-29. They
are evidence, not decoration: each one is a claim a spike or a task made.

| File | What it shows |
| --- | --- |
| `spike-1.4-cmux.png` | TODO 1.4: cmux renders the Kitty graphics protocol at full fidelity. Column 2's disc is perfectly round with thin grid lines; columns 3 and 4 (iTerm2, sixel) are empty. |
| `spike-1.4-terminal.png` | TODO 1.4: Terminal.app renders only half-blocks and prints the other three protocols' escape sequences as visible text. |
| `trak-demo.gif` | TODO 11.1: the README hero. 21 s in cmux on 2026-10-02, Version B against real Spotify: History with the cover, the Lyrics tab, `a` and `v` through waveform / circular / spectrum, `L`, `?`. Frames are `screencapture -l` of the cmux window, cropped to the terminal (no sidebar), deduplicated and quantised to 128 colours with PIL; 1.3 MB. |
| `dashboard.png`, `visualizer-*.png`, `lyrics-fullscreen.png`, `settings.png` | TODO 11.2: stills from the same session, same crop. Version A screenshots need a live login **[owner]**. |
| `tui-3.3-wide.png` | TODO 3.3: the wide layout at 100×30 — header with status dot and clock, Now Playing with the art placeholder, interpolated progress bar, controls, volume meter, the tab strip, and the footer key hints. |

`tui-3.3-wide.png` is the honest current state: the art area is an empty framed
placeholder because album art is TODO 4.1. It is drawn rather than left blank so
the layout reads as designed and so the renderer already reserves exactly the
space the cover will need.
