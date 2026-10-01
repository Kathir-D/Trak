# Trak — terminal image protocols (TODO 1.4, risk R5)

**Measured 2026-09-29** on the owner's machine by running `spikes/images` inside
each terminal and screenshotting the result with `screencapture`. Not reasoned
about — the screenshots are in `docs/images/`.

`spikes/images` renders one generated test image (four colour quadrants, a 32px
grid, a white disc) through **all four** `ratatui-image` protocols side by side, so
a single screenshot answers every cell of the table below. The disc and the grid
are there on purpose: a wrong aspect ratio or a naive resampler is obvious at
20×10 cells.

## Results

| Terminal | `halfblocks` | `kitty` | `iterm2` | `sixel` | `Picker::from_query_stdio()` chose |
| --- | --- | --- | --- | --- | --- |
| **cmux 1.3.2-HEAD** (`TERM_PROGRAM=ghostty`) | ✅ works (blocky) | ✅ **works, full fidelity** | ❌ nothing rendered | ❌ nothing rendered | **`Kitty`**, cell size `(8, 17)` |
| **Terminal.app 488** (`TERM_PROGRAM=Apple_Terminal`) | ✅ works (blocky) | ❌ literal escape text | ❌ literal escape text | ❌ literal escape text | **`Halfblocks`**, font size `(10, 20)` |

Evidence:

- `docs/images/spike-1.4-cmux.png` — four columns. Columns 1 and 2 show the image;
  column 2's disc is perfectly round with thin grid lines, which is only possible
  with real pixel graphics. Columns 3 and 4 (iterm2, sixel) are empty.
- `docs/images/spike-1.4-terminal.png` — same four columns in Terminal.app. Only
  column 1 renders; the other three **print their escape sequences as visible
  text** (`/DkP;wyC;…` for kitty, `Gi=31,s=1,v=1,a=q…` for iTerm2, and sixel's
  DCS payload for sixel), which is what an unsupported protocol looks like.

### R5 is refuted

`TODO.md` risk R5 was "cmux may not pass the Kitty image protocol through". **It
does.** cmux identifies as `ghostty` and Ghostty implements the Kitty graphics
protocol, and `ratatui-image`'s query correctly negotiated it and negotiated the
cell size as well. So the best-quality art path is available in the terminal the
owner actually uses, and the half-blocks fallback is only needed for Terminal.app
and any terminal that does not speak Kitty.

The visual difference is large and worth choosing deliberately: the half-blocks
version of the same image is visibly chunky, because each cell is two stacked
square pixels. That is still perfectly presentable — it is what Terminal.app gets
and it is what Trak's `art_protocol = "halfblocks"` setting is for.

### Detection works unattended

This matters because Trak must pick a protocol with no user interaction. In both
terminals, `Picker::from_query_stdio()` returned the right answer on its own:

```
# cmux
AUTO ok protocol_type=Kitty font_size=(8, 17)
AUTO cap Kitty
AUTO cap CellSize(Some((8, 17)))

# Terminal.app
AUTO ok protocol_type=Halfblocks font_size=(10, 20)
```

Note it reports a **font size / cell size**, not just a protocol. Trak must use
that when computing how many cells an image occupies, or album art will be
mis-sized. The environment Trak should look at as a fallback is the usual
`TERM_PROGRAM` / `KITTY_WINDOW_ID` / `TERM` set, but the picker already does the
hard part and Trak's `art_protocol = "auto"` setting should just defer to it.

### A trap in the spike worth remembering

`Picker::new_protocol()` **succeeded for all four protocols in both terminals**,
including the three that do not work. Construction is not rendering. A test that
asserts `new_protocol(...).is_ok()` therefore proves nothing about whether the
terminal will display the image — the only real check is a screenshot or a human
eyeball. TODO 4.1's "done when" must not be satisfied by a unit test alone.

## Consequences

- `docs/ARCHITECTURE.md`'s "Rendering art" note ("whether cmux passes the Kitty
  graphics protocol through is unverified (TODO 1.4)") is now answered.
- `art_protocol = "auto"` maps to `Picker::from_query_stdio()`'s choice. The
  `kitty | iterm2 | sixel | halfblocks` overrides in SPEC §8 stay as manual
  escape hatches.
- README's terminal support line can now say cmux gets true-colour art, and
  Terminal.app gets half-blocks. Neither needs the user to do anything.
- The half-blocks path still has to look good on its own, because it is what
  Terminal.app users get, and it is the only path guaranteed to work anywhere.

## Reproducing

```sh
cd spikes/images
cargo build --release
./target/release/images-spike     # renders all four, logs to /tmp/spike/images.log
screencapture -x -R <x>,<y>,<w>,<h> out.png
```

To run it in a specific terminal without disturbing an existing session, open it in
a new tab rather than reusing a window — and note that on this machine a
`caffeinate` window must never be closed.
