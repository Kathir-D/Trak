# trak — AppleScript field survey (TODO 1.1 and 1.2)

Everything here was measured live on the owner's machine, not read from docs.

| | |
| --- | --- |
| Date | 2026-09-29 |
| macOS | 27.0 (26A428), arm64 |
| Spotify | **1.3.1.234** (`/Applications/Spotify.app`, bundle `com.spotify.client`) |
| Interpreter | `/usr/bin/osascript` (macOS built-in) |
| Fixtures | `tests/fixtures/applescript/` — see that folder's `NOTES.md` |

> The `osascript` in `~/.local/bin` is a **shim** that routes calls through
> `asrun` so an agent session holds the Automation grant. It adds **~250 ms of
> pure overhead** to every call (see [Cost](#cost)). All numbers below come from
> `/usr/bin/osascript` invoked as the shim with `ASRUN_BYPASS=1`, which is what
> trak itself will do (COMPAT rule 7). Never benchmark trak through the shim.

---

## 1. The dictionary (authoritative list)

Dumped from the app itself, not from the internet:

```sh
sdef /Applications/Spotify.app
```

`tests/fixtures/applescript/../spotify.sdef` is not committed (it is Apple's
output, and it can change with the app), but the surface it declares is:

**application** — `current track` (r), `sound volume` (rw), `player state` (r,
enum `stopped|playing|paused`), `player position` (rw, real), `repeating` (rw),
`repeating enabled` (r), `shuffling` (rw), `shuffling enabled` (r).

**track** — `artist`, `album`, `album artist`, `disc number`, `duration`,
`played count`, `track number`, `starred`, `popularity`, `id`, `name`,
`artwork url` (all r), `artwork` (r, **deprecated** — "will never be set"),
`spotify url`.

**commands** — `next track`, `previous track`, `playpause`, `pause`, `play`,
`play track "uri" [in context "uri"]`.

There is **no** `stop` command and **no** `quit` in Spotify's own suite. See
[`stop` / `quit`](#stop-and-quit).

---

## 2. Every property, with a real sample

Measured with `tell application "Spotify" to return <expr>`, while
`Census Designated` by Jane Remover was playing.

| Property | Raw output | Type | Notes |
| --- | --- | --- | --- |
| `player state` | `playing` | enum, as text | `stopped` / `playing` / `paused`. Read with `as string`; asking for a bare enumeration returns the name, which is what we want. |
| `player position` | `3.359999895096` | real (f32 widened to f64) | **Seconds**, float. The `f32` widening shows up as noise digits — parse as `f64` and **never** compare for equality. shpotify's `pos` must accept floats. |
| `sound volume` | `100` | integer | 0–100. See [Volume](#volume-the-r2-claim-is-false). |
| `shuffling` | `false` | boolean | The **mode**. |
| `shuffling enabled` | `true` | boolean (r) | The **live state**. These can differ: while a shuffled context is playing, `shuffling` may be `false` and `shuffling enabled` `true`. trak should show `shuffling` (what the user toggles) and may use `enabled` for the "context is shuffled" hint. |
| `repeating` | `true` | boolean | Mode, as above. |
| `repeating enabled` | `true` | boolean (r) | Live state. |
| `name` | `Census Designated` | text | Empty string when nothing is loaded. |
| `artist` | `Jane Remover` | text | Empty string for an ad. |
| `album` | `Census Designated` | text | Empty string for an ad. |
| `album artist` | `Jane Remover` | text | |
| `duration` | `360511` | integer | **Milliseconds**, despite the sdef description saying "in seconds". 360511 ms = 6:00.5, which matches. trak must divide by 1000. |
| `disc number` | `1` | integer | `0` for an ad. |
| `track number` | `8` | integer | `0` for an ad. |
| `popularity` | `48` | integer | 0–100. `0` for an ad. |
| `played count` | `0` | integer | Local play count, not scrobbles. |
| `artwork url` | `https://i.scdn.co/image/ab67616d0000b2738a821784ac3e69e691d4945f` | text | The `640`-size variant. No size parameter is exposed; fetch the larger `ab67616d00001e02…` form by string surgery if a bigger image is wanted (TODO 4.1). For an ad this is the **literal string** `missing value`. |
| `spotify url` | `spotify:track:6HacgXCExkzS552ILfJTXu` | text | A **URI**, not an `https://` URL. |
| `id` | `spotify:track:6HacgXCExkzS552ILfJTXu` | text | **Identical to `spotify url`** on every track observed. The sdef calls it "The ID of the item" but the value is the full URI. trak needs only one of the two; `id` is the cheaper, more honest name. |
| `starred` | *error* `-10000 AppleEvent handler failed` | — | **Broken.** Not in the standard suite, and the handler fails even with a valid track. Liking needs the Web API (Version A, `f` key). Do not surface `starred`. |
| `artwork` (image data) | *error* `-1700` | — | Deprecated by the app itself. Ignore. |
| `version` | `1.3.1.234` | text | From the Standard Suite. Useful in the Info tab and in bug reports. |
| `genre`, `comment`, `kind`, `type` | *error* `-1700` / `-1728` | — | **Do not exist.** `-1728` is `errAEEventNotHandled` (the property is not in the dictionary); `-1700` is a type-specifier error. |

### Types, the reliable way

`class of x` **does not work** for these properties — it always fails with
`-1700` because the results are bridged values, not AppleScript objects:

```
$ osascript -e 'using terms from application "Spotify"
return (class of (player position)) as string'
execution error: Can’t make class of «class pPos» of «script» into type string. (-1700)
```

`properties of current track` also fails (`-10000`), so there is no shortcut to
a whole-record fetch. The type column above comes from the `sdef` plus the value
shapes observed; it is authoritative enough to write a parser against.

---

## 3. States

| State | Observed |
| --- | --- |
| **Playing a track** | Everything populated, `id` = `spotify:track:…`. |
| **Paused** | **Indistinguishable from playing except `player state`.** All track fields, position, volume, shuffle, repeat still read fine. `player position` stays put. So the parser needs no special paused branch — only the status dot changes. |
| **Playing an ad** | The nasty one. `id`/`spotify url` = `spotify:ad:…`; `name` is the ad's name; `artist`, `album`, `album artist` are **empty strings**; `artwork url` is the **literal string** `missing value`; `duration`, `disc number`, `track number`, `popularity`, `played count` are all **`0`**. See `tests/fixtures/applescript/playing_ad.txt`. |
| **Stopped** | Not reachable while an app is loaded with a track; the enumeration has `stopped` but there is no `stop` command to reach it (see below). Treat `player state == stopped` as "nothing to show" and show the idle card. |
| **Nothing loaded** | Same shape as the ad: empty strings, `0`s, `missing value` artwork. A fresh install with no history behaves this way. |
| **Spotify not running** | Must never be reached — see below. |

### A `tell` **launches** a non-running app — the guard is not optional

Verified with a proxy app so the owner's Spotify was never disturbed:

```
$ osascript -e 'return (application "TextEdit" is running) as string'
false
$ osascript -e 'tell application "TextEdit" to return name of front window'
Untitled
$ osascript -e 'return (application "TextEdit" is running) as string'
true          # ← the tell LAUNCHED it
$ pgrep -x TextEdit   # a real process appeared
```

So `tell application "Spotify"` on a quit Spotify would **start Spotify**,
violating COMPAT rule 2. Every single osascript invocation must therefore be
preceded by the running check, and the check must be in the **same process**:

```applescript
if application "Spotify" is running then return "1" else return "0" end if
```

A caveat: that **multi-line** form must be a real script file. As a single `-e`
argument it is a syntax error (`-2740`), because `osascript` joins `-e` chunks
and the `else` lands inside a string:

```
$ osascript -e 'if application "Spotify" is running then return "1" else return "0"'
syntax error: A "else" can't go after this "". (-2740)
```

The safe one-liner (works as a single `-e`, no script file needed, and it is
what `AppleScriptPlayer` should use):

```
$ osascript -e 'return (application "Spotify" is running) as string'
true
```

Two more traps found while testing this:

- `osascript -e 'tell application "NoSuchAppXYZ" to return 1'` **succeeds and
  prints `1`, exit 0** — for a typo'd app name, because a bare `return 1` never
  reaches the app. It is not evidence the app exists. A real property access on
  the same name is a syntax error. Never treat a successful bare return as proof
  of liveness; always use `is running`.
- Nothing is created for a nonexistent bundle, so the guard cannot itself launch
  anything: `application "NoSuchAppXYZ" is running` → `false`, no process.

---

## 4. Cost (TODO 1.2)

The brief predicted "≈1 process per poll, not 10", and assumed a poll would be
comfortably under 80 ms. **On this machine it is not, and by a lot.**

### Method

`subprocess`-spawned `/usr/bin/osascript`, 5 warm-up runs discarded, 50 runs for
the headline number, p50/p95 over the sorted list. Called directly (not through
the `asrun` shim).

### Results

| Script | p50 | p95 | min | max |
| --- | --- | --- | --- | --- |
| `-e 'return 1'` (no Apple Event) | **50.1 ms** | 54.6 | 48.5 | — |
| `is running` guard | **51.2 ms** | 54.1 | — | — |
| `tell` + 1 property (`player state`) | 133.7 ms | — | — | — |
| `tell` + 5 app-level properties | 200.8 ms | — | — | — |
| **Batched 17-field read (the real poll)** | **431 ms** | **437 ms** | 417 | — |
| Batched 17-field read, via `~/.local/bin/osascript` shim | 725 ms | 1064 | 501 | 1303 |
| Same, as JXA (`osascript -l JavaScript`) | 415 ms | — | — | — |
| Same, from a **warm** process, 10 iterations, total | 3139 ms → **~308 ms/iter** | — | — | — |
| Same, warm, 20 iterations, total | 6163 ms → **~308 ms/iter** | — | — | — |
| Compiled with `osacompile` (`.scpt`) instead of interpreted | 421 ms | — | — | — |

**p95 of the full read is 437 ms, 5.5× the 80 ms budget in TODO 1.2.**

### Where the time actually goes

It is not the process spawn, and it is not the script size. Adding *N* extra
track-property reads to a script and measuring the slope:

| Extra properties | p50 | Marginal cost per property |
| --- | --- | --- |
| 0 | 130.8 ms | — |
| 1 | 150.2 ms | 19.4 ms |
| 2 | 166.8 ms | 18.0 ms |
| 4 | 200.3 ms | 17.4 ms |
| 8 | 282.0 ms | 18.9 ms |
| 12 | 348.5 ms | 18.1 ms |

**A flat ~18 ms per Apple Event, paid by Spotify.** It is not proportional to the
value's size (a 22-byte int costs the same as a 90-byte URL), and it is not
AppleScript interpretation:

| Same round-trip count, different app | p50 |
| --- | --- |
| Finder, 1 Apple Event | 59.7 ms |
| Finder, 10 Apple Events | 59.9 ms |
| Finder, 30 Apple Events | 62.3 ms → **0.1 ms marginal** |
| Spotify, 30 Apple Events | 618 ms → **~19 ms marginal** |

Finder answers 30 events in 2 ms of Apple Event time. Spotify takes 600 ms. So
the cost is **inside Spotify's Apple Event handler**, and trak cannot get it back
by restructuring AppleScript. Confirmed dead ends:

- **JXA** (`osascript -l JavaScript`, same bridge) — 415 ms, no better.
- **`osacompile` to `.scpt`** — 421 ms, no better.
- **Coalescing into a `{a, b, c}` list** (which AppleScript *can* turn into one
  event for some apps) — 435 ms, no better. Spotify's handler does not coalesce.
- **A warm process** (repeat in one `osascript` run) — saves only the ~50 ms
  spawn and the ~50 ms first-event warm-up, i.e. 434 → 308 ms. The per-event
  cost is unchanged.
- **Paused vs playing** — 3139 ms vs 3135 ms for 10 warm iterations. Identical,
  so this is *not* contention with audio decoding or the UI.

### Consequences for the design

1. **A 1 s poll cannot be a 17-field read.** At 431 ms of mostly-idle waiting
   per second, trak would sit at ~43 % of a core just reading state, and the
   progress bar would visibly stutter. Split the read:
   - **Fast read** (the 1 s / 3 s poll): `player state`, `player position`,
     `sound volume`, `shuffling`, `repeating`, `id` — 6 events, **~300 ms**.
   - **Slow read** (only when `id` differs from the last one): the 11 track
     fields — name, artist, album, album artist, duration, disc/track number,
     popularity, played count, artwork url, spotify url. Track metadata changes
     once per song, so this can afford ~300 ms.
   This still leaves a 300 ms floor per poll. It is the best available, and it
   is acceptable because the poll runs on a worker thread and the progress bar is
   interpolated locally between reads (ARCHITECTURE "Key decisions").
2. **Do not chain reads.** Every extra property is another ~18 ms, so the fast
   read is exactly the 6 fields above and no more. Add a "now playing in the
   background" panel sparingly or not at all.
3. **Prefer the notification to the poll** (TODO 1.3). If
   `PlaybackStateChanged` works, a poll only has to be a safety net, and can drop
   to 3 s even while playing.
4. **The `is running` guard is a whole extra 51 ms process** if run separately. It
   must be the first statement of the *same* script as the read — which is
   exactly what the batched script should do, so a non-running Spotify costs one
   51 ms process that returns "not running" instead of launching it.
5. **The 5 s timeout in TODO 2.3 stays.** Worst observed was 1.3 s through the
   shim; 5 s is a comfortable multiple, not a guess about the typical case.

Batched read script (the one trak should ship) is in
[`trak-read.applescript`](../../spikes/applescript/trak-read.applescript) and its
output is `tests/fixtures/applescript/*.txt`.

---

## 5. Volume — the R2 claim is false

`docs/COMPAT.md` rule 5 and risk **R2** both assert, on headless-spotify's
authority, that "Spotify ≥ 1.3.x ignores AppleScript volume sets". **On
1.3.1.234 that is not true. Sets work.** 8/8 randomised targets applied:

```
target=59 -> 58    target=81 -> 80    target=47 -> 46    target=8  -> 7
target=41 -> 40    target=54 -> 53    target=67 -> 66    target=29 -> 28
```

But there is a **consistent off-by-one** in the read-back, which is a different
and more dangerous bug. Sweeping the range:

| set | read | Δ | | set | read | Δ |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | 0 | 0 | | 50 | 49 | −1 |
| 1 | 0 | −1 | | 60 | 60 | 0 |
| 2 | 1 | −1 | | 70 | 69 | −1 |
| 5 | 4 | −1 | | 80 | 80 | 0 |
| 10 | 9 | −1 | | 90 | 89 | −1 |
| 20 | 20 | 0 | | 95 | 94 | −1 |
| 30 | 29 | −1 | | 98 | 97 | −1 |
| 40 | 40 | 0 | | 99 | 98 | −1 |
| | | | | 100 | 100 | 0 |

Spotify quantises to some internal scale (most likely `round(v/100 * N)/N * 100`
for an `N`-step internal volume) and the round-trip lands one step low about half
the time. The value is **stable** afterwards — sampling every 250 ms for 3 s
after setting 70 read `69` every time, not a ramp — and it survives a
pause/play cycle. So this is a quantisation artefact, not a delayed apply.

**What this means for trak (TODO 2.4, 3.10, 4.4):**

- The read-back check in COMPAT rule 5 must use a **tolerance of ±1**, or it will
  declare every write a failure and hide the volume meter permanently. This is
  the single most important output of this spike for the volume feature.
- The failure mode trak should still detect is *Spotify ignoring the write
  entirely* — the read staying at the pre-write value (e.g. set 70, still 100).
  Distinguish "unchanged" from "off by one", not "changed at all".
- trak must show the **value trak last successfully set**, not the raw read,
  otherwise the meter visibly jitters by 1 % after every keypress.
- The system-volume fallback (`set volume output volume N`, verified working:
  sets 50, reads back 50) is still worth keeping as a config option, but it
  should now be offered as a *preference*, not as an automatic consequence of a
  suspected Spotify bug.

`docs/COMPAT.md` rule 5 and `TODO.md` R2 are updated by this task.

---

## 6. `stop` and `quit`

Spotify's suite has **no `stop` and no `quit` command**. shpotify's `stop` is
implemented as "if playing then `playpause`", and its `quit` uses the Standard
Suite's `quit`.

- `osascript -e 'tell application "Spotify" to stop'` returns `stop` and does
  **not** pause playback — an unhandled command that silently no-ops. trak's
  `stop` must use shpotify's semantics (`playpause` when playing), and must not
  pretend the command exists.
- `quit` works via the Standard Suite (`NSApplication`), but quitting Spotify is
  a write and must be user-initiated (COMPAT rule 3).

---

## 7. Batched read script

Fields are separated by **U+001F (ASCII unit separator)**, chosen in TODO 1.2.
Rationale, now evidence-based: the fixtures contain album `♡`, an ad name, empty
strings and the literal `missing value`, and `U+001F` is a C0 control character
that cannot appear in a Spotify title, album or artist (the strings are UTF-8
text from the catalogue; C0 controls are not emitted in any observed value).
Newline `U+000A` was rejected as a delimiter on the same grounds.

```
0 player state     1 player position    2 sound volume      3 shuffling
4 repeating        5 name               6 artist            7 album
8 album artist     9 duration (ms)     10 disc number      11 track number
12 popularity     13 played count      14 artwork url      15 spotify url
16 id
```

The `id` is field 16 precisely so a cheap "has the track changed?" check can be
done against the fast read without parsing the slow fields.

When nothing is loaded, the script must emit the 17-field record with empty /
`missing value` / `0` placeholders and a `stopped` state, **not** a shorter
record — a fixed field count keeps the parser a pure `split` plus per-index
conversion, with no length branching.

## 8. Raw commands for re-verification

```sh
# run with the shim removed so the numbers match this document
export ASRUN_BYPASS=1
sdef /Applications/Spotify.app
osascript -e 'return (application "Spotify" is running) as string'
osascript -e 'tell application "Spotify" to return player state as string'
osascript -e 'tell application "Spotify" to return player position'
osascript -e 'tell application "Spotify" to return duration of current track'
osascript -e 'tell application "Spotify" to return starred of current track'   # -10000
osascript spikes/applescript/trak-read.applescript | tr '\037' '\n'
```

---

## 9. `PlaybackStateChanged` (TODO 1.3)

### It works, from a plain CLI process

`com.spotify.client.PlaybackStateChanged` is a real distributed notification, and
**a bare, un-bundled Rust binary receives it.** That was the open question in
TODO 1.3 ("a non-bundled process may not receive distributed notifications") and
the answer is that it does. Verified with `spikes/notify` (Rust, `objc2` +
`objc2-foundation`) and, independently, with a 20-line Swift program.

`objc2-foundation` 0.3 exposes `NSDistributedNotificationCenter` but **only the
selector-based registration** — the block variant is not generated:

```rust
// spikes/notify/src/main.rs, the shape player/notify.rs should take
define_class!(
    #[unsafe(super(NSObject))]
    pub struct Observer;
    unsafe impl NSObjectProtocol for Observer {}
    impl Observer {
        #[unsafe(method(handleNotification:))]
        fn handle(&self, note: &NSNotification) { /* -> trak Event channel */ }
    }
);

let observer: Retained<Observer> = msg_send![Observer::alloc(), init];
NSDistributedNotificationCenter::defaultCenter().addObserver_selector_name_object(
    &observer,
    sel!(handleNotification:),
    Some(&NSNotificationName::from_str("com.spotify.client.PlaybackStateChanged")),
    None,
);
NSRunLoop::currentRunLoop().runUntilDate(&until);
```

Note the cost of this choice: trak's TUI already runs a main run loop, so
registering a selector is workable, but it means `player/notify.rs` must own an
`NSObject` subclass and keep it alive. It also means the callback is delivered on
the **main thread**, so it must do nothing but hand the data to a channel.

### The real `userInfo` keys — the guessed names were wrong

TODO 1.3 listed `Player State`, `Name`, `Artist`, `Album`, `Track ID`,
`Duration`, `Playback Position` as "believed but unconfirmed". Confirmed, with
**13** keys, not 7:

| Key | Value | Type | Notes |
| --- | --- | --- | --- |
| `Player State` | `Playing` | `NSTaggedPointerString` | **Capitalised**, unlike AppleScript's lowercase `playing`/`paused`/`stopped`. Normalise on parse. |
| `Name` | `misplace` | `NSTaggedPointerString` | track title |
| `Artist` | `Jane Remover` | `__NSCFString` | |
| `Album` | `Frailty` | `NSTaggedPointerString` | |
| `Album Artist` | `Jane Remover` | `__NSCFString` | |
| `Track ID` | `spotify:track:0ALXVfQFaNZ1GmqvlG8X7V` | `__NSCFString` | **Full URI**, same as AppleScript's `id`. Not a bare 22-char id. |
| `Duration` | `233783` | `__NSCFNumber` | **Milliseconds**, same unit as AppleScript. |
| `Playback Position` | `108.714` | `__NSCFNumber` | **Seconds**, float, same as AppleScript. |
| `Track Number` | `3` | `__NSCFNumber` | |
| `Disc Number` | `1` | `__NSCFNumber` | |
| `Popularity` | `46` | `__NSCFNumber` | 0–100 |
| `Play Count` | `0` | `__NSCFNumber` | note the space in the name; AppleScript calls it `played count` |
| `Has Artwork` | `1` | `__NSCFBoolean` | **new, and not in SPEC §5.** It is a *boolean*, not a URL. |

Two gaps worth noting:

- **There is no `artwork url` in the notification.** Only the boolean
  `Has Artwork`. Album art still requires an AppleScript read (TODO 4.1) — the
  notification can tell trak *whether* to bother, not *what* to show. This is
  the single best reason to keep the slow AppleScript read in the loop.
- `Play Count` (notification) vs `played count` (AppleScript) — different
  spellings for what appears to be the same value. Treat the notification as
  authoritative only for the fields it has, and normalise both into one
  `TrackInfo` in the parser.

`object` is always `com.spotify.client`.

### Which changes actually fire it

This is the finding that shapes the architecture. Each row was an isolated
6-second listening window with a single write, so there is no ambiguity:

| Change | Fires? |
| --- | --- |
| `play` / `pause` / `playpause` | **yes** |
| `next track` / `previous track` | **yes** |
| `set player position` (seek) | **no** — 3 separate seeks, 0 events, 0 events again on a repeat run |
| `set sound volume` | **no** |
| `set shuffling` | **no** |
| `set repeating` | **no** |

The notification therefore covers *playback state and track changes* — precisely
the events trak must not miss, and precisely the ones Sonar's buttons, the media
keys and the Spotify window all produce. It does **not** cover the four things
trak's own keys write. trak applies its own writes locally, so it does not need
an event for them, but a volume or shuffle changed *outside* trak (Sonar's fade,
the Spotify window) will only be seen by a poll.

### Latency

Measured from a second process issuing the write to the callback firing, using
`Date().timeIntervalSince1970` on both sides:

| Action | Write issued → notification |
| --- | --- |
| `pause` → event | ~170 ms |
| `next track` → event | ~170 ms |

**Well inside TODO 3.9's < 300 ms budget** (and that 170 ms includes the ~50 ms
`osascript` spawn of the *writer*, so the real notification latency is lower).

### Decision

Subscribe via `objc2-foundation` + a `define_class!` observer, and keep the
AppleScript poll as a **slow safety net**, not as the primary path. Concretely,
this revises the poll cost problem in §4: the 300 ms fast read no longer has to
happen every second, because the notification tells trak immediately when
something changed. The poll becomes 3–5 s (or slower) and only has to cover
volume, shuffle, repeat and artwork — the things the notification is silent
about. Recorded in `docs/ARCHITECTURE.md`.
