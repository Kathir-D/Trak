# trak — audio tap and visualisation (TODO 1.5 and 1.6)

Both spikes were run on 2026-09-29. `spikes/viz` covers the analysis half (1.6)
and `spikes/tap` covers the capture half (1.5).

Headline: **the audio tap works, and no permission prompt was needed at all** —
so R1 is refuted. But the process-*specific* tap that trak's design requires does
not, and that is the open problem. Details in §3.

---

## 1. Verdict so far

| Question | Answer |
| --- | --- |
| Does `cavacore` build on stable Rust for both release targets? | **Yes**, for `aarch64-apple-darwin` and `x86_64-apple-darwin` (rustc 1.98.1) |
| Does it produce a usable spectrum? | **Yes**, and it discriminates frequency correctly |
| Chosen sample rate | **48 000 Hz** — the tap's actual rate, measured (§3) |
| Chosen bar count | **32**, and 16–96 all build |
| Does a system-audio tap work? | **Yes**, from an un-bundled CLI, with no prompt ([§3](#3-the-process-tap-r1--mostly-resolved-r6--one-real-blocker)) |
| Does a tap on **Spotify only** work? | **No** — every process-specific tap description fails ([§3](#3-the-process-tap-r1--mostly-resolved-r6--one-real-blocker)) |

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

**Chosen: 48 000 Hz, 32 bars, mono.**

- **48 000 is measured, not chosen** — the tap on this machine reports
  `48000 Hz, 1 ch, 32 bit` (§3), and the default output device is also running at
  48 kHz. An earlier revision of this doc said 44 100; that was wrong.
- **mono** because the tap mixes anyway and a stereo tap would double the
  bandwidth for a display that is 40 cells wide.
- **32 bars** because the Now Playing pane is roughly 40 cells wide (SPEC §3) and
  more bars than cells just aliases.

**The tap's native rate decides this, and it must be read, not hard-coded.**
`cavacore` trusts whatever sample rate it is given, so TODO 8.3 must take
`asbd.sample_rate` off the tap and configure the builder from it. The committed
fixture is 44.1 kHz and is still a valid reference for the *renderers*; the rate
8.3 passes in will be 48 000.

## 3. The process tap (TODO 1.5)

Run by `spikes/tap`, on 2026-09-29, with Spotify playing (pid 60095).

### 3a. A tap works, and R1 is refuted

A global tap created from an **un-bundled CLI** (`cargo run`, no app bundle, no
`NSApplication`, no `Info.plist`) produced real audio:

```
  tap description mode: global-mono
  tap created
    format: 48000 Hz, 1 ch, 32 bit, flags=FormatFlags(9)
  aggregate device created
  starting IO proc — 20s window, play something in Spotify
  rms=0.132270   -17.57 dBFS  peak=0.3830  |############################################|
  ...
--- 958976 float samples in 20s (47948.8 samples/s) ---
stopped and dropped the device, tap and aggregate device
```

**No "System Audio Recording" prompt appeared, and nothing had to be clicked.**
For context, the machine had *no* audio TCC grants at all beforehand — the
`kTCCServiceMicrophone` table was empty — so this is not a case of a pre-existing
grant being reused.

R1 was "a CLI child may not get a usable *System Audio Recording* prompt, or cmux
may not be prompt-able". The premise is wrong: no prompt is needed. **R1 is
refuted**, and with it the biggest scheduling risk in the project. TODO 3.x and
8.3 no longer have to plan around a modal permission step.

Teardown is clean: after the process exited, `system_profiler SPAudioDataType`
showed no tap or aggregate device.

### 3b. The one real blocker: a process-specific tap does not work

trak must tap **Spotify only**, so it neither picks up unrelated system audio nor
disturbs Sonar's own tap (SPEC §7, COMPAT rule 3). That needs a tap description
that names pids. Every such shape fails identically:

| `TRAK_TAP_MODE` | ObjC selector | Result |
| --- | --- | --- |
| `mono-mixdown` | `initMonoMixdownOfProcesses:` (allow-list) | **`!obj` 560947818** |
| `stereo-mixdown` | `initStereoMixdownOfProcesses:` (allow-list) | **`!obj` 560947818** |
| `global-exclude-spotify` | `initMonoGlobalTapButExcludeProcesses:` (1 pid) | **`!obj` 560947818** |
| `global-mono` | `initMonoGlobalTapButExcludeProcesses:` (empty) | **ok** |
| `global-stereo` | `initStereoGlobalTapButExcludeProcesses:` (empty) | **ok** |

`560947818` is `0x216F626A` = fourcc **`!obj`** = `kAudioHardwareBadObjectError`
("The AudioObjectID passed to the function doesn't map to a valid AudioObject"),
from `AudioHardwareBase.h`. It is **not** `kAudioDevicePermissionsError`
(`!hog`, 0x21686F67) — so this is not a permission problem, and there is nothing
for the owner to grant.

The pattern is sharp: **an empty pid list is accepted, any non-empty pid list is
rejected**, in both the include and the exclude form. The obvious explanation is
a marshalling problem rather than a macOS policy, so the obvious thing was tried
first — the pids were boxed as `NSNumber` at every width:

| `TRAK_PID_TYPE` | `global-exclude-spotify` | `mono-mixdown` |
| --- | --- | --- |
| `f64` | `!obj` | `!obj` |
| `i32` | `!obj` | `!obj` |
| `i64` | `!obj` | — |
| `u32` | `!obj` | — |

The number width makes no difference, so the obvious cause is ruled out. This is
exactly risk **R6** ("`cidre` API for process taps is unstable / under-documented")
materialising, and it is the thing TODO 1.5 exists to find.

### 3c. What this means, and the decision

**Go / no-go for real audio: conditional go.** Real audio works; process isolation
does not. Three options, in the order they should be tried:

1. **Bypass `cidre` for the tap description only.** `TapDesc` is four thin ObjC
   initialisers. Calling them directly through `objc2` is maybe 60 lines and would
   test whether the fault is in cidre's binding or in the OS. This is the cheapest
   way to find out, and TODO 1.5's own escape hatch ("if unusable, write a ~150-line
   binding") already anticipates it.
2. **Fall back to the global tap with a filter.** A global tap sees all system
   audio, including whatever Sonar is playing. trak could still restrict *what it
   renders* to the Spotify track it currently knows about — but that is a visual
   approximation, it would pick up other apps' audio during Spotify's silence, and
   it sits badly with COMPAT rule 3's intent. Only acceptable as a labelled
   opt-in.
3. **Ship simulated.** TODO 8.1/8.2 are already specified and need nothing from
   this spike, so this is always available and must remain the default until
   option 1 is settled.

**Recommendation: do option 1, and keep simulated as the default meanwhile.** The
audio path is proven to work end to end — 48 kHz, mono, float32, clean teardown —
so the only thing missing is a working way to name one process, and that is a
small amount of code rather than a platform limitation.

### 3d. Two more things for 8.3 and 8.5

- **The tap is 48 000 Hz, 1 channel, 32-bit float** (flags 9 = float + packed).
  8.3 must build `cavacore` from `asbd.sample_rate`, not a constant.
- **`ca::device_start` returns a `StartedDevice` that must be kept alive.** An
  early version of the spike dropped it immediately, which stopped the device and
  looked exactly like a hang with zero samples. 8.5 must hold it, drop it
  explicitly, and then assert via `system_profiler SPAudioDataType` that no tap or
  aggregate device is left behind after 10 start/stop cycles.

### 3e. A packaging trap worth recording

`cidre` links a Swift-concurrency static library that references
`@rpath/libswift_Concurrency.dylib`. On this machine that dylib is **not a file on
disk** — it is in the dyld shared cache — so the binary dies at launch with
`Library not loaded … Reason: no LC_RPATH's found`. The fix is one line in
`build.rs`:

```rust
println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
```

`spikes/tap/build.rs` has it. Whoever adds `cidre` to trak proper must carry that
line across, or the Homebrew binary will not start on this OS. It also needs
`default-features = false` with an explicit feature list (`core_audio`, `ca`,
`cat`, `av`, `app`, `objc`, `blocks`, `dispatch`, `cf`, `ns`, `macos_14_2`); the
default feature set pulled in a `crate::blocks` the code does not use.
