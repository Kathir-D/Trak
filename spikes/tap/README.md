# spikes/tap — TODO 1.5

Answers the three questions 1.5 asks, as far as an unattended session can. The
full write-up, with the evidence, is `docs/AUDIO-TAP.md`.

```sh
cargo build --release

# defaults to the Spotify-only allow-list (the shape SPEC §7 / COMPAT require)
./target/release/tap-spike

# TRAK_TAP_MODE picks the tap-description shape:
#   mono-mixdown            initMonoMixdownOfProcesses:            <-- allow-list
#   stereo-mixdown          initStereoMixdownOfProcesses:
#   global-mono             initMonoGlobalTapButExcludeProcesses:  (empty list)
#   global-stereo           initStereoGlobalTapButExcludeProcesses:(empty list)
#   global-exclude-spotify  initMonoGlobalTapButExcludeProcesses:  (1 pid)
# TRAK_PID_TYPE: f64 | i32 | i64 | u32   (how the pids are boxed as NSNumber)
```

Play something in Spotify first; the program refuses to launch it (COMPAT rule 2).

## What it found

- **A global tap works from an un-bundled CLI, with no permission prompt.** 48 kHz
  mono float32, ~47 950 samples/s, real audio at about -17.6 dBFS RMS while
  Spotify played, and a clean teardown leaving no device behind. **R1 is
  refuted** — no "System Audio Recording" click was needed.
- **Every process-specific tap fails** with `kAudioHardwareBadObjectError`
  (`!obj`, OSStatus 560947818): both the include-list and the exclude-list forms,
  at every NSNumber width. Only the empty-exclude-list global tap is accepted.
  This is the blocker — see `docs/AUDIO-TAP.md` §3.

## Two things to carry into 8.3 / 8.5

- The tap's format is **48 000 Hz**, not 44 100. `cavacore` must be configured from
  the tapped format, not a constant (TODO 1.6 recorded this; 8.3 must do it).
- `ca::device_start` returns a `StartedDevice` that must be kept alive for the
  duration of the tap. Dropping it early stops the device and looks exactly like a
  hang. TODO 8.5's teardown must drop it explicitly, then check
  `system_profiler SPAudioDataType` for leftovers.
