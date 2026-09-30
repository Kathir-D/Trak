# spikes/viz — TODO 1.6

Answers: **is `cavacore` viable for trak's visualizer, and does it build on
stable Rust for both release targets?**

Both yes. Findings, with the chosen sample rate and bar count, are in
`docs/AUDIO-TAP.md`; the raw bar output is committed as a fixture at
`tests/fixtures/visualizer/cavacore-bars.txt` so TODO 8.x's renderers can be
golden-tested with no audio device, no tap and no permission prompt.

```sh
cargo build --release
./target/release/viz-spike                      # prints bars for 4 synthetic signals
cargo build --release --target x86_64-apple-darwin   # 1.6 requires both targets
```

Carried into 8.x:

- 44 100 Hz, 32 bars, mono — but the **tap's native rate wins**, so 8.3 must
  configure the builder from the tapped format rather than hard-coding a rate.
- `CavaBuilder::default()`, not `CavaBuilder::new()`; `SampleRate` is a
  `BoundedU32<1, 384_000>` so it needs `SampleRate::new(x)`, not `SampleRate::Hz(x)`.
- **One `Cava` per stream**, and a fresh one per signal when testing: it carries
  peak/autosens state, and reusing one instance made an 80 Hz sine and a 440 Hz
  sine report an identical spectrum in the first version of this spike.
- Output is **not normalised**. Autosens ramps over roughly a second, so renderers
  scale against pane height and a recent peak rather than assuming 0..1.

This spike covers the *analysis* half of TODO 1.5 only. The Core Audio process tap
is still open and needs the owner to click a "System Audio Recording" prompt — see
`docs/AUDIO-TAP.md` §3.
