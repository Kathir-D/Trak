# spikes/images — TODO 1.4

Answers: **which image protocols actually work in cmux and Terminal.app?**
(Risk R5 in TODO.md was "cmux may not pass the Kitty image protocol through".)

It renders one generated test image — four colour quadrants, a 32px grid, a white
disc — through all four `ratatui-image` protocols side by side, so a single
`screencapture` of the window answers every cell of the table in
`docs/TERMINALS.md`. The disc and grid exist so a wrong aspect ratio or a naive
resampler is visible even at 20x10 cells.

```sh
cargo build --release
./target/release/images-spike          # holds ~20s, logs to /tmp/spike/images.log
screencapture -x -R <x>,<y>,<w>,<h> out.png
```

Results: cmux renders **kitty** at full fidelity and falls back to halfblocks
correctly; Terminal.app renders **only** halfblocks and prints the other three
protocols' escape sequences as visible text. R5 is refuted.

Two things to carry into TODO 4.1:

- `Picker::from_query_stdio()` also reports a **cell size**, which the image layout
  must use. trak's `art_protocol = "auto"` should just defer to it.
- `new_protocol()` **succeeds for protocols the terminal does not support** — it
  constructed all four in both terminals. Only a screenshot or a human eye proves
  an image renders, so 4.1's "done when" cannot be satisfied by a unit test.
