# trak — audio tap and visualisation (TODO 1.5 and 1.6)

TODO 1.6 is done and its findings are here. TODO 1.5 (the process tap) is the
part of this spike that could not be completed from an unattended session; read
[§3](#3-the-process-tap-15--not-completed) for exactly what is missing and what
the owner has to do.

---

## 1. Verdict so far

| Question | Answer |
| --- | --- |
| Does `cavacore` build on stable Rust for both release targets? | **Yes**, for `aarch64-apple-darwin` and `x86_64-apple-darwin` (rustc 1.98.1) |
| Does it produce a usable spectrum? | **Yes**, and it discriminates frequency correctly |
| Chosen sample rate | **44 100 Hz** (48 000 also builds; see below) |
| Chosen bar count | **32**, and 16–96 all build |
| Can a process tap on Spotify be read? | **Unresolved — needs the owner** ([§3](#3-the-process-tap-15--not-completed)) |

## 2. `cavacore` findings (TODO 1.6)

`cavacore` 2.0.2 is a pure-Rust port of cava's core engine. It is a good fit: it
does the FFT, the three-band weighting, the peak/ballistics maths and the
falloff, and leaves trak to do nothing but hand it `f64` samples and read `f64`
bars back. The API is small:

```rust
let cava = cavacore::CavaBuilder::default()
    .bars_per_channel(NonZeroUsize::new(32).unwrap())
    .sample_rate(SampleRate::new(44_100).unwrap())   // BoundedU32<1, 384_000>
    .audio_channels(Channels::Mono)
    .enable_autosens(true)
    .noise_reduction(0.2)
    .build()?;
cava.execute(&samples, &mut bars);
```

`CavaBuilder` has no `new()`; it is `CavaBuilder::default()` and then
builder methods. `SampleRate` is a `bounded_integer::BoundedU32<1, 384_000>`, so
it needs `SampleRate::new(x)`, not `SampleRate::Hz(x)`. Both cost a compile cycle
to discover and are worth a comment in `viz/dsp.rs`.

### It discriminates frequency correctly

`spikes/viz` feeds synthetic signals through a **fresh** `Cava` and prints the
resulting bars. The output is in `tests/fixtures/visualizer/cavacore-bars.txt`:

| Signal | Where the energy lands (32 bars, 60 Hz–16 kHz range) |
| --- | --- |
| 80 Hz sine | bars 0–12 — the low end |
| 440 Hz sine | bars 21–26 — the middle |
| white noise | spread across all 32 bars |
| silence | nothing |

So the low/mid/treble split cava is built around is real, and the bar order is
low-to-high. That is what the `spectrum`, `mirrored` and `circular` renderers in
TODO 8.2 will draw.

### Two things the spike got wrong, that 8.x must not repeat

1. **Reusing one `Cava` across signals gives a wrong answer.** `cavacore` carries
   peak and autosens state inside the instance, so the first version of this spike
   reported an *identical* spectrum for an 80 Hz sine and a 440 Hz sine. Each
   signal needs its own instance to get a clean reading. In the real app there is
   only one continuous stream, so this is not a correctness problem there — but it
   makes the fixture worthless if the test reuses an instance.
2. **The output is not normalised.** With `enable_autosens`, values start near
   zero and ramp over roughly a second. Renderers must scale against the pane
   height and a recent peak; they must not assume the bars are already 0..1.

### Sample rate and bar count

| Setting | Result |
| --- | --- |
| 44 100 Hz | builds, used for the fixture |
| 48 000 Hz | builds |
| 96 000 Hz | builds |
| 16 / 24 / 32 / 48 / 64 / 96 bars | all build |

**Chosen: 44 100 Hz, 32 bars, mono.** 44.1 kHz because the tap is easiest to get
consistently and it matches the fixture; **mono** because the tap mixes anyway and
a stereo tap would double the bandwidth for a display that is 40 cells wide.
32 bars because the Now Playing pane is roughly 40 cells wide (SPEC §3) and more
bars than cells just aliases.

**The tap's native rate decides this, not preference.** If the tap on this machine
comes up at 48 kHz, trak must tell `cavacore` 48 000 and resample or accept the
mismatch — `cavacore` trusts the value it is given. TODO 8.3 must read the format
off the tap and configure the builder from it, rather than hard-coding 44 100.

## 3. The process tap (TODO 1.5) — not completed

R1 and R6 are still open. What is known:

- `spikes/viz` proves the **analysis** half works and is testable with no audio
  device at all, which is what CI needs.
- The **capture** half — a Core Audio process tap on `com.spotify.client` via
  `cidre` — was not exercised. It needs a live "System Audio Recording" decision
  for the terminal, which cannot be granted from an unattended session: the
  prompt is a modal system panel and nobody is at the keyboard to click it.

What still has to be answered, and cannot be answered by reasoning:

1. **Does the prompt appear at all** when an *un-bundled CLI* (not a `.app`) asks
   for a process tap? This is R1. The permission is normally attributed to the
   parent application, so a plain `cargo run` binary may either inherit cmux's
   existing grant, prompt again, or fail outright with a specific `OSStatus`.
2. **What does denial look like** — a specific error code, silence, or a hang?
   trak must have a bounded timeout either way.
3. **Does it work while Sonar's own tap is active?** TODO 10.7 depends on this.

### [owner] steps to finish 1.5

Run these in **cmux** (not Terminal.app) and expect a system prompt:

```sh
cd spikes/tap                      # to be created from cidre's core-audio-record example
cargo run --release
# 1. play something in Spotify
# 2. when macOS asks to allow "System Audio Recording", click Allow
# 3. report: did a prompt appear? what happened on the second run?
```

Useful to run first, so the state is clean:

```sh
system_profiler SPAudioDataType    # note any existing tap/aggregate devices
```

Afterwards, the leftover-device check TODO 8.5 needs:

```sh
system_profiler SPAudioDataType    # compare with the above
```

If it turns out that an un-bundled CLI cannot obtain the grant, the fallback
decided in this spike is: **ship the simulated visualizer (TODO 8.1/8.2) as the
default, and gate real audio behind an explicit opt-in that instructs the user to
run trak from a terminal which has been granted System Audio Recording.** The
analysis path is already proven, so nothing else in 8.x is blocked by this.
