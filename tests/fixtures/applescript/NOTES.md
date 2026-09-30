# AppleScript fixtures (TODO 1.1 / 1.2)

Captured live on the owner's machine on **2026-09-29**, macOS 27.0, arm64,
Spotify **1.3.1.234**, using the batched read script documented in
`docs/APPLESCRIPT.md`.

Every file is the **raw stdout of one `osascript` run**, field-separated by
`U+001F` (ASCII 31, unit separator), with the trailing newline that `osascript`
adds stripped by the reader.

| File | What it shows |
| --- | --- |
| `playing_track.txt` | Normal playing track, all 17 fields populated |
| `paused_track.txt` | Same track while paused — all track fields still readable |
| `playing_ad.txt` | **An advertisement is playing.** `id`/`spotify url` are `spotify:ad:…`, `name` is the ad's name, `album`/`artist`/`album artist` are **empty strings**, `artwork url` is the literal string `missing value`, and every numeric field is `0`. A parser that assumes `spotify:track:` or a non-empty album will break here. |

Field order (stable, owned by `docs/APPLESCRIPT.md`):

```
0 player state     1 player position    2 sound volume      3 shuffling
4 repeating        5 name               6 artist            7 album
8 album artist     9 duration (ms)     10 disc number      11 track number
12 popularity     13 played count      14 artwork url      15 spotify url
16 id
```

Ad fixtures are the reason `AppleScriptPlayer`'s parser must be a pure function
tested against all three files (TODO 2.3).
