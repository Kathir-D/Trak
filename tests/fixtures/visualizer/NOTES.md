# Visualizer fixtures (TODO 1.6)

`cavacore-bars.txt` is the raw stdout of `spikes/viz` — `cavacore` run over
synthetic signals at 44.1 kHz / 32 bars / mono, one fresh `Cava` per signal.

It is the deterministic reference TODO 8.x needs: it fixes what the real DSP
produces for a known input, so the renderers in `viz/render.rs` (spectrum,
mirrored, waveform, circular) can be golden-tested **with no audio device, no tap
and no permission prompt** — which is what CI has to satisfy.

Read it together with `docs/AUDIO-TAP.md`. Two properties of `cavacore` that the
file makes visible and that the renderers must respect:

1. **A fresh `Cava` per signal is required for a correct spectrum.** `cavacore`
   carries peak and autosens state across `execute()` calls, so reusing one
   instance makes the previous signal's spectrum leak into the next. This was a
   real bug in the first version of the spike — 440 Hz and 80 Hz produced
   identical output until each got its own instance.
2. **The output is not normalised.** With `enable_autosens`, values start near
   zero and ramp up over roughly a second of audio. Bar heights must be scaled
   against the pane height and against recent peak, never treated as 0..1 already.
