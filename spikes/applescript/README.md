# spikes/applescript

Throwaway scripts from TODO 1.1 / 1.2, kept because they are how the numbers in
`docs/APPLESCRIPT.md` were produced and because the parser tests (TODO 2.3) will
want to re-run them.

- `trak-read.applescript` — the batched 17-field read. This is the shape trak
  should ship. `U+001F` separated; see `docs/APPLESCRIPT.md` §7.
- `fast-read.applescript` — the 6 fields the 1 s / 3 s poll needs
  (state, position, volume, shuffling, repeating, id).
- `probe-missing-value.applescript` — distinguishes empty string from
  `missing value` from an error, which is how the ad fixture was understood.

Run them against the real app with:

```sh
ASRUN_BYPASS=1 /usr/bin/osascript spikes/applescript/trak-read.applescript | tr '\037' '\n'
```

Timing harness (the one that produced §4 of the doc):

```sh
ASRUN_BYPASS=1 python3 - <<'PY'
import subprocess, time, os
env = {**os.environ, "ASRUN_BYPASS": "1"}
ts = []
for _ in range(50):
    t0 = time.perf_counter()
    subprocess.run(["/usr/bin/osascript", "spikes/applescript/trak-read.applescript"],
                   capture_output=True, env=env)
    ts.append((time.perf_counter() - t0) * 1000)
ts.sort()
print(f"p50={ts[25]:.1f}ms p95={ts[47]:.1f}ms")
PY
```
