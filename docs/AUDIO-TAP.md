# Trak — audio tap and visualisation (TODO 1.5 and 1.6)

Both spikes were run on 2026-09-29. `spikes/viz` covers the analysis half (1.6)
and `spikes/tap` covers the capture half (1.5).

Headline: **the audio tap works, and no permission prompt was needed at all** —
so R1 is refuted. The process-*specific* tap Trak's design requires also works,
once pids are translated into Core Audio process objects (§3b). TODO 8.3/8.5
shipped it; §4 has the measurements. `cavacore`, chosen in 1.6, turned out to be
broken and is not used (§2f).

---

## 1. Verdict so far

| Question | Answer |
| --- | --- |
| Does `cavacore` build on stable Rust for both release targets? | **Yes**, for `aarch64-apple-darwin` and `x86_64-apple-darwin` (rustc 1.98.1) |
| Does it produce a usable spectrum? | **No** — the spike's reading was wrong (§2f) |
| Chosen sample rate | **48 000 Hz** — the tap's actual rate, measured (§3) |
| Chosen bar count | **32**, and 16–96 all build |
| Does a system-audio tap work? | **Yes**, from an un-bundled CLI, with no prompt ([§3](#3-the-process-tap-r1--mostly-resolved-r6--one-real-blocker)) |
| Does a tap on **Spotify only** work? | **Yes**, with the pid→process-object translation (§3b); shipped in 8.3 (§4) |
| Is `cavacore` what ships? | **No** — it truncates every band to FFT bin 0 (§2f); `src/audio.rs` has its own FFT |

## 2. `cavacore` findings (TODO 1.6)

`cavacore` 2.0.2 is a pure-Rust port of cava's core engine. It is a good fit: it
does the FFT, the three-band weighting, the peak/ballistics maths and the
falloff, and leaves Trak to do nothing but hand it `f64` samples and read `f64`
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

### It discriminates frequency correctly — it does not; see §2f

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

### 2f. Correction (TODO 8.3): `cavacore` never discriminated frequency

The table above is wrong, and the way it is wrong is not tuning. `cavacore` 2.0.2
builds each band's FFT range with

```rust
buffer_lower_cut_off[n] = relative_cut_off[n] as u32 * (in_bass.len() / 2)
```

where `relative_cut_off[n] = cut_off_frequency[n] / (sample_rate / 2)` is a
fraction below 1, so the cast truncates it to **zero** and every band is given the
same first bin (`cavacore-2.0.2/src/lib.rs:339`). The whole 60 Hz–16 kHz range
collapses into bins 0…31 of one transform. Measured at 48 kHz, the loudest band
for a pure tone:

| Tone | Band | Tone | Band |
| --- | --- | --- | --- |
| 60 Hz | 11 | 1.6 kHz | 31 |
| 80 Hz | 9 | 4 kHz | 31 |
| 200 Hz | 17 | 8 kHz | 31 |
| 800 Hz | 30 | 15 kHz | 31 |

Everything above ~850 Hz is one band. No configuration avoids it (the cast is only
non-zero above Nyquist, which the builder rejects), and upstream still has it on
`main` as of 2026-10. So `src/audio.rs` carries the two pieces the crate was
chosen for: a radix-2 FFT with a Hann window sized from the tap's own rate (~42 ms
at any rate), and a log-spaced band map over 60 Hz–16 kHz with the top clamped
below Nyquist. Its tests put every tone in the band its frequency names.

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

### 3b. The blocker, and what actually caused it: pids are not AudioObjectIDs

Trak must tap **Spotify only**, so it neither picks up unrelated system audio nor
disturbs Sonar's own tap (SPEC §7, COMPAT rule 3). That needs a tap description
that names processes. The first round of experiments failed identically:

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
(`!hog`, 0x21686F67) — so it was never a permission problem, and there was
nothing for the owner to grant.

The pattern was sharp — **an empty pid list is accepted, any non-empty pid list
is rejected**, in both the include and the exclude form — and boxing the pids as
`NSNumber` at every width made no difference. The cause turned out to be simpler
and is now proven by the objc2 bypass (`spikes/tap/src/bin/tap-objc2.rs`):

> **`CATapDescription.h` says the array holds AudioObjectIDs, not pids:**
>
> ```
> @param processesObjectIDsToIncludeInTap
>     An NSArray of NSNumbers where each NSNumber holds an AudioObjectID of the
>     process object to include in the tap
> ```
>
> Every earlier attempt passed the **pid** directly. Translating it first, via
> `kAudioHardwarePropertyTranslatePIDToProcessObject` (`'id2p'`), makes every
> process-specific shape work. A pid has to be translated into that
> AudioObjectID first, via
> `kAudioHardwarePropertyTranslatePIDToProcessObject` ('id2p'). Passing the pid
> directly is what every earlier attempt did, and it is why `!obj` appeared for a
> non-empty list and not for an empty one.

With the translation in place (objc2-built `CATapDescription`, everything after
it still cidre):

| ObjC selector | pids (not translated) | process AudioObjectIDs | empty |
| --- | --- | --- | --- |
| `initMonoMixdownOfProcesses:` | `!obj` | **ok — 48 kHz mono** | ok |
| `initStereoMixdownOfProcesses:` | `!obj` | **ok — 48 kHz stereo** | ok |
| `initMonoGlobalTapButExcludeProcesses:` | `!obj` | **ok** | ok |
| `initStereoGlobalTapButExcludeProcesses:` | `!obj` | **ok** | ok |

The end-to-end capture on the include-list cell (`TRAK_CAPTURE=1
TRAK_TAP_MODE=mono-mixdown TRAK_LIST=objects`) delivered **real Spotify audio
from Spotify's process only**: 575 488 float samples in 12 s ≈ 47 957/s, RMS
−12 to −17 dBFS, clean teardown, no leftover tap or aggregate device in
`system_profiler SPAudioDataType`. R6 is resolved: **the fault was the missing
pid→object translation, not the OS and not cidre's class resolution** (cidre's
`TapDesc::cls()` resolves to the real `CATapDescription`; the binding simply
does not do the translation for you).

Two incidental traps the bypass binary hit, for whoever copies this:

- `name` on `CATapDescription` is an **instance** method; there is no class
  method, and objc2 panics on a message send to one that does not exist.
  `AnyClass::name()` is the way to read a class's name.
- The `'prs#'` (`kAudioHardwarePropertyProcessObjectList`) read returned
  `'nope'` (0x6E6F7065) on this machine even though `'id2p'` works, so do not
  make enumerating processes a dependency — translate the pids you care about.

### 3c. What this means, and the decision

**RESOLVED (1.5): process-specific taps work, and Trak ships real audio.** The
bypass proved the only missing piece was the pid→AudioObjectID translation
(§3b). The 8.3 source taps Spotify's process directly:

- translate each pid with `kAudioHardwarePropertyTranslatePIDToProcessObject`
  (`'id2p'`) — a pid that names no Core Audio process returns `kAudioObjectUnknown`
  (0), so a Spotify that is paused or exited yields an empty include list, and the
  tap is not created for one;
- hand the translated ids to cidre's own `TapDesc::with_mono_mixdown_of_processes`.
  The spike built `CATapDescription` through `objc2` to rule cidre out
  (`spikes/tap/src/bin/tap-objc2.rs`), but cidre's binding is fine once it is given
  object ids rather than pids, so 8.3 needs no objc2 bypass — only the one
  `AudioObjectGetPropertyData` call for `'id2p'`, which cidre does not bind;
- the global tap stays unused: it captures all system audio, including Sonar's,
  which is what COMPAT rule 3 is about.

### 3d. Two more things for 8.3 and 8.5

- **The tap is 48 000 Hz, 1 channel, 32-bit float** (flags 9 = float + packed).
  8.3 must size its transform from `asbd.sample_rate`, not a constant.
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

`spikes/tap/build.rs` has it. **Trak proper does not need it**: the Swift
reference comes from cidre's default `foundation` → `swift` features, and
`Cargo.toml` turns the defaults off and lists only `core_audio`, `cf`, `ns`, `ca`,
`cat`, `app`, `objc`, `blocks`, `dispatch` and `macos_14_2`. With that list
`otool -L target/release/trak` shows no `@rpath` entry and no Swift dylib, and the
binary starts with no `build.rs`. Whoever adds a cidre feature must re-check
`otool -L` before a release.

---

## 4. What shipped, and how it was checked (TODO 8.3 and 8.5)

All on 2026-10-01, this machine, Spotify 48 kHz, `examples/tap-probe.rs`
(`cargo build --release --example tap-probe`). None of it can be a unit test:
CI has no Spotify, no audio device and no grant. The lifecycle *logic* is unit
tested against a fake backend in `src/audio.rs` instead.

### 4a. The shape

- `AudioPipeline` is created idle by the TUI loop. `set_wanted(visible)` is called
  every frame with the same test that drives the frame rate (visualizer on screen
  **and** Spotify running), so the tap attaches when the pane appears and is
  released within one 20 ms worker tick when it is hidden or Spotify goes idle.
  The settings screen can switch `[visualizer] source` live: `simulated` drops the
  worker, and the tap, before the call returns.
- A Spotify that quits under a live tap is not reported by Core Audio: the device
  keeps running and the callback stops, exactly as when paused. The worker checks
  `kill(pid, 0)` on the tapped pids once a second, drops the tap when they are
  gone, and retries every 2 s — which is what reattaches when Spotify is relaunched.
- **A paused Spotify stops the IO proc being called at all** — there is no silence
  to hand over. After three empty ticks (60 ms) the analyser is fed an empty window
  and the bars drop to zero, instead of freezing on the last spectrum.
- A failure the owner can act on (a denial, a Core Audio error) is one toast per
  run; "Spotify not running" is silent. The bars fall back to the simulated source.
- Teardown order is the `TapSession` field order: stop the device and destroy the
  aggregate device, then the IO buffer, then the tap.

### 4b. It tracks the music

`tap-probe run 12` with Spotify playing — one line per second of real frames
(mean bar height, loudest bar, mean of the low/mid/high thirds):

```
     t  state                      mean   peak    low    mid   high
   1.2s  Tapping { sample_rate: 48000.0 } 0.186  1.000  0.210  0.172  0.175
   2.4s  Tapping { sample_rate: 48000.0 } 0.216  1.000  0.253  0.193  0.199
   4.8s  Tapping { sample_rate: 48000.0 } 0.199  1.000  0.253  0.166  0.176
   6.0s  Tapping { sample_rate: 48000.0 } 0.169  1.000  0.179  0.146  0.183
   9.6s  Tapping { sample_rate: 48000.0 } 0.240  1.000  0.239  0.250  0.230
  verdict         : bars are moving
```

The same with Spotify paused: every line `0.000  0.000  0.000  0.000  0.000`,
state still `Tapping { sample_rate: 48000.0 }`. The loudest bar is always 1.0
while playing because the bars are scaled against a decaying recent peak.

No permission prompt appeared and none was needed, from a CLI or from the TUI in
a pty, matching §3a.

### 4c. Nothing is left behind

`system_profiler SPAudioDataType` lists 3 devices (Dell S2716DG, MacBook Pro
Microphone, MacBook Pro Speakers) before and after `tap-probe cycles 10`, and the
two dumps are byte-identical. That check is weaker than it looks: Trak's aggregate
device is **private**, so `system_profiler` cannot see it even while it is live.
The probe therefore also counts Core Audio's own lists from inside the process
(`'dev#'` devices, `'tps#'` taps):

```
  before any tap:            devices : 3   taps : 0
  with the first tap up:     devices : 4   taps : 1
  cycle 1..10: up as Tapping { sample_rate: 48000.0 }, down as Idle
  after 10 cycles, every Drop run:   devices : 3   taps : 0
```

A second process does *not* see another process's tap or aggregate device (it
counted 3 / 0 while a tap was live), so a leftover from a dead process cannot be
observed from outside at all.

- **SIGTERM / SIGHUP** with a tap up (`tap-probe signal`): the handler only sets an
  atomic; the worker drops the tap, restores the default disposition and re-raises.
  The process died of SIGTERM (exit 143) 0.10 s after the signal.
- **SIGKILL, or a panic** (release is `panic = "abort"`, so no `Drop` runs): nothing
  in-process can clean up. The tap and aggregate device are private to the process
  that made them and coreaudiod owns their lifetime; after a `kill -9` with a tap
  live, Spotify kept playing, `system_profiler` was identical, and a fresh tap came
  up normally. That is reasoned from Core Audio's ownership model plus those
  observations, not observed directly — see above for why it cannot be.

### 4d. Cost

The release TUI in a 110×34 pty (`XDG_CONFIG_HOME` pointing at a config with
`[display] mode = "visualizer"`), Spotify playing, 30 s after a 5 s warm-up:

| Source | cpu time / wall | `ps -o %cpu` mean (min–max) |
| --- | --- | --- |
| real tap (`source = "auto"`) | 1.17 s / 30.8 s = **3.8 %** | 3.5 (1.9–4.7) |
| simulated (`source = "simulated"`) | 0.99 s / 30.8 s = **3.2 %** | 3.1 (2.1–4.4) |

So the tap, the ring and the FFT add about 0.6 % of one core over the simulated
bars; nearly all of the cost is drawing 30 frames a second. `q` exited in 0.14 s
with the tap up.
