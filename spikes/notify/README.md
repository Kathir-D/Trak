# spikes/notify — TODO 1.3

Answers the question in TODO 1.3: **can a plain Rust CLI process (no app bundle)
subscribe to Spotify's `com.spotify.client.PlaybackStateChanged` distributed
notification?**

**Yes.** Run it against a playing Spotify:

```sh
cargo run --release            # listens for 16s and prints each event's userInfo
# in another shell, or from the agent session:
ASRUN_BYPASS=1 /usr/bin/osascript -e 'tell application "Spotify" to next track'
```

## What the spike establishes

- `objc2-foundation` 0.3 exposes `NSDistributedNotificationCenter`, but **only the
  selector-based `addObserver:selector:name:object:`** — the block-taking variant
  is not generated. So trak must define a small `NSObject` subclass with
  `define_class!` and register a selector. That is fine: the TUI already owns a
  main run loop, and the callback can post into trak's `Event` channel.
- **No bundle identifier, no `NSApplication`, no run-loop app required.** A bare
  `cargo run` binary receives the notification, so the "only bundled apps get
  distributed notifications" worry does not apply here.
- Registration is cheap and the observer is delivered on the main run loop, so
  `player/notify.rs` should register once at startup and keep the object alive
  for the process's lifetime.

## Measured behaviour (also in `docs/APPLESCRIPT.md`)

| Change | Fires the notification? |
| --- | --- |
| play / pause / playpause | **yes** |
| next track / previous track | **yes** |
| seek (`set player position`) | **no** — 3 isolated seeks, 0 events |
| `set sound volume` | **no** |
| `set shuffling` / `set repeating` | **no** |

So the notification covers track and play-state changes (the expensive-to-miss
ones, and the ones Sonar, media keys and the Spotify window all produce) but
**not** trak's own volume / shuffle / repeat / seek writes. trak already applies
those locally from its own input, so it does not need an event for them — but a
volume changed by *outside* means (Sonar's fade, the Spotify UI) will only be
picked up by the poll. That is the argument for keeping the poll as a slow
safety net rather than deleting it.
