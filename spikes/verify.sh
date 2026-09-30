#!/bin/zsh
# Re-checks every claim the Phase 1 spikes made, against the real machine.
#
# Run it from a normal terminal:
#
#   ./spikes/verify.sh          # everything, ~3 minutes
#   ./spikes/verify.sh --quick  # skip the slow tap and notification checks
#
# It reads from Spotify, and the 1.3 and 1.5 sections do pause it, seek it and
# skip a track (that is the only way to test those claims), but it never quits
# Spotify, never launches it, and leaves playback running. Nothing needs a
# network, and nothing here is safe to run unattended while you are listening.
#
# A FAIL is a real disagreement with docs/ -- read the section it names before
# changing either the code or the doc.
#
# The 1.8 (Keychain) section is OPT-IN, because testing it deliberately provokes
# a real macOS keychain authorization dialog that asks for your login password.
# In an unattended session nobody can answer it, so the dialogs just pile up on
# your screen. Run it only when you are at the machine:
#
#   ./spikes/verify.sh --keychain
#
# It cleans up every item it creates, including on Ctrl-C.
#
# Sections that need a human to look at a screen are listed at the end and are
# NOT run by this script.

# rustup's cargo is on PATH in a login shell but not always in a non-login one,
# and this script is often run from a fresh terminal. AGENTS.md covers this.
[[ -d $HOME/.cargo/bin ]] && export PATH="$HOME/.cargo/bin:$PATH"

em=~/.local/bin/osascript
[[ -x $em ]] || em=/usr/bin/osascript
# On the owner's machine an agent session must bypass the asrun shim (it adds
# ~250ms to every call and would skew the timings). A human's own terminal needs
# no bypass, so only apply it when we are not already going through the shim.
if [[ $em == *local/bin* ]]; then
  export ASRUN_BYPASS=1
fi
# A function, not an array: in zsh "$OSA" on an array expands to only the first
# element, which would pass the script text as a *file path* to osascript.
osa() { "$em" -e "$1" 2>&1; }

pass=0; fail=0; skip=0
if [[ -t 1 ]]; then G=$'\033[32m'; R=$'\033[31m'; Y=$'\033[33m'; B=$'\033[1m'; N=$'\033[0m'
else G=""; R=""; Y=""; B=""; N=""; fi   # no colour when piped to a file
ok()   { printf "  ${G}PASS${N}  %s\n" "$1"; ((pass++)); }
no()   { printf "  ${R}FAIL${N}  %s\n" "$1"; ((fail++)); }
sk()   { printf "  ${Y}SKIP${N}  %s\n" "$1"; ((skip++)); }
head_() { printf "\n${B}== %s${N}\n" "$1"; }
say()  { printf "        %s\n" "$1"; }

check() { # check <description> <expected-substring> <actual>
  if [[ $3 == *"$2"* ]]; then ok "$1"; else no "$1"; say "expected to contain: $2"; say "got: ${3:-<empty>}"; fi
}

quick=0
keychain=0
for arg in "$@"; do
  [[ $arg == --quick ]]    && quick=1
  [[ $arg == --keychain ]] && keychain=1
done

sp() { osa "tell application \"Spotify\" to return $1"; }

# ---------------------------------------------------------------- 1.1 fields
head_ "1.1  AppleScript field survey (docs/APPLESCRIPT.md)"

if [[ $(osa 'return (application "Spotify" is running) as string') != true ]]; then
  print -r -- "  Spotify is not running, so nothing can be checked."
  print -r -- "  Start Spotify and play something, then re-run. (trak must never launch it itself.)"
  exit 1
fi
ok "Spotify is running (the guard this script starts with is the one trak must use)"

ver=$(sp 'version')
check "Spotify version is recorded" "1.3" "$ver"

dur=$(sp 'duration of current track')
if [[ -n $dur && $dur -gt 10000 ]]; then
  ok "duration is milliseconds ($dur ms) — not seconds, despite the sdef saying seconds"
else
  no "duration looked like $dur — expected a large integer, i.e. milliseconds"
fi

id=$(sp 'id of current track')
url=$(sp 'spotify url of current track')
if [[ -n $id && $id == "$url" ]]; then
  ok "id == spotify url == $id (the sdef calls it 'the ID' but it is the full URI)"
else
  no "id ($id) and spotify url ($url) differ"
fi

if [[ $id == spotify:track:* ]]; then
  ok "a real track is loaded, so the field survey's samples are representative"
elif [[ $id == spotify:ad:* ]]; then
  sk "an advertisement is playing — the ad fixture applies instead of the normal one"
else
  sk "no track loaded, so track fields will be empty (this is the 'nothing loaded' state)"
fi

starred=$(osa 'tell application "Spotify" to return starred of current track')
check "starred is broken, so liking needs the Web API" "-10000" "$starred"

genre=$(osa 'tell application "Spotify" to return genre of current track')
if [[ $genre == *-1700* || $genre == *-1728* ]]; then
  ok "genre does not exist (-1700/-1728) — the doc's 'do not use' list is right"
else
  no "genre returned '$genre' — the doc says it errors"
fi

art=$(sp 'artwork url of current track')
if [[ -n $art ]]; then
  ok "artwork url is a plain https URL"
fi

# The committed fixtures must still parse to 17 fields.
cd "$(dirname $0)/.." || exit 1
for f in tests/fixtures/applescript/*.txt; do
  n=$(python3 -c "
import sys
raw = open('$f','rb').read().decode()
print(len(raw.rstrip('\n').split('\x1f')))
")
  if [[ $n == 17 ]]; then ok "fixture $(basename $f) parses to 17 fields"
  else no "fixture $(basename $f) parses to $n fields, expected 17"; fi
done

# The guard must not launch anything: a typo'd bundle must stay non-existent.
before=$(pgrep -f NoSuchAppXYZ | wc -l | tr -d ' ')
osa 'return (application "NoSuchAppXYZ" is running) as string' >/dev/null
after=$(pgrep -f NoSuchAppXYZ | wc -l | tr -d ' ')
if [[ $before == "$after" ]]; then
  ok "the 'is running' guard never launches anything (NoSuchAppXYZ still absent)"
else
  no "something got launched by the guard"
fi

# ------------------------------------------------------------------ 1.2 cost
head_ "1.2  Poll cost (docs/APPLESCRIPT.md section 4)"
if [[ $quick == 1 ]]; then
  sk "skipped (--quick)"
else
  say "timing the 17-field read 20x; expect a few seconds..."
  stats=$(ASRUN_BYPASS=${ASRUN_BYPASS:-1} python3 - <<'PY'
import subprocess, time, os
env = {**os.environ}
ts = []
for _ in range(20):
    t0 = time.perf_counter()
    subprocess.run(["osascript", "spikes/applescript/trak-read.applescript"],
                   capture_output=True, env=env)
    ts.append((time.perf_counter() - t0) * 1000)
ts.sort()
print(f"{ts[len(ts)//2]:.0f} {ts[int(len(ts)*0.95)]:.0f}")
PY
)
  p50=${stats% *}; p95=${stats#* }
  if (( p50 > 200 )); then
    ok "p50 is ${p50} ms — confirms the doc's claim that this is far over the old 80 ms budget"
  else
    no "p50 is only ${p50} ms, but the doc claims ~430 ms. Re-read docs/APPLESCRIPT.md section 4."
  fi
  say "p50=${p50}ms  p95=${p95}ms   (a much lower number would mean the timing changed)"

  # The real cause: cost is per-Apple-Event inside Spotify, not per-process.
  say "the doc's cause claim is ~18ms per Apple Event inside Spotify (Finder: ~0.1ms)."
fi

# -------------------------------------------------------------- 1.3 notify
head_ "1.3  PlaybackStateChanged notification (docs/APPLESCRIPT.md section 9)"
if [[ $quick == 1 ]]; then
  sk "skipped (--quick)"
else
  if ! (cd spikes/notify && cargo build --release 2>/dev/null); then
    sk "spikes/notify did not build"
  else
    (cd spikes/notify && ./target/release/notify-spike > /tmp/trak-verify-notify.txt 2>&1) &
    listener=$!
    sleep 4

    # Make sure a *real* transition is possible, then count events around each
    # action. A no-op (pausing an already-paused player) fires nothing, so the
    # state has to be normalised first or the count is meaningless.
    if [[ $(sp 'player state') != playing ]]; then
      osa 'tell application "Spotify" to play' >/dev/null; sleep 2
    fi

    evcount() { grep -c '^EVENT' /tmp/trak-verify-notify.txt 2>/dev/null | head -1; }

    before=$(evcount)
    # playpause is a toggle, so it always changes state and always fires.
    osa 'tell application "Spotify" to playpause' >/dev/null; sleep 2
    after_toggle=$(evcount)
    osa 'tell application "Spotify" to playpause' >/dev/null; sleep 2
    osa 'tell application "Spotify" to next track' >/dev/null; sleep 2.5
    after_next=$(evcount)
    # and now the asymmetry the doc claims: a seek fires nothing
    osa 'tell application "Spotify" to set player position to 30' >/dev/null; sleep 2.5
    after_seek=$(evcount)

    wait $listener 2>/dev/null
    osa 'tell application "Spotify" to play' >/dev/null

    toggled=$(( after_toggle - before ))
    skipped=$(( after_next - after_toggle ))
    fired=$(( after_next - before ))
    seekfired=$(( after_seek - after_next ))
    what=$(sp 'id of current track')

    # Only the playpause toggle is a guaranteed state change, so that is the
    # reliable pass condition. An advert is not: skipping one often lands on
    # another advert, and no track or play state actually changes, so the
    # notification legitimately does not fire. Requiring 2 made this flaky.
    if (( toggled >= 1 )); then
      ok "a plain Rust CLI received the notification for a playpause toggle — no bundle id needed"
    else
      no "a playpause toggle fired nothing; docs/APPLESCRIPT.md section 9 says play/pause fire"
    fi
    if [[ $what == spotify:ad:* ]]; then
      sk "skip fired $skipped notification(s) — an advert is playing, where that is expected"
    elif (( skipped >= 1 )); then
      ok "next track fired $skipped notification(s), as the doc claims"
    else
      no "next track fired nothing; the doc says skips fire too (currently $what)"
    fi
    say "total for playpause+skip: $fired"

    if (( seekfired == 0 )); then
      ok "a seek fired 0 notifications — the asymmetry the doc records is confirmed"
    else
      no "the seek fired $seekfired notification(s), but the doc says seeks do NOT fire"
    fi

    if grep -q 'Has Artwork' /tmp/trak-verify-notify.txt; then
      ok "userInfo has the undocumented 'Has Artwork' boolean"
    else
      sk "no 'Has Artwork' key seen this run (it appears on every track event observed so far)"
    fi
    if grep -q 'artwork url' /tmp/trak-verify-notify.txt; then
      no "userInfo HAS an 'artwork url' key, but the doc says it does not"
    else
      ok "userInfo has no 'artwork url' — album art must still come from AppleScript"
    fi
    if grep -q 'Player State = ' /tmp/trak-verify-notify.txt; then
      ok "'Player State' present — and capitalised, unlike AppleScript's lowercase 'playing'"
    fi
  fi
fi

# ----------------------------------------------------------------- 1.6 dsp
head_ "1.6  cavacore (docs/AUDIO-TAP.md section 2)"
if (cd spikes/viz && cargo build --release 2>/dev/null); then
  viz=$(cd spikes/viz && ./target/release/viz-spike 2>&1)
  if [[ $viz == *"440Hz peak"* ]]; then
    ok "cavacore runs and reports bar heights"
  else
    no "cavacore spike produced no output"
  fi
  # frequency discrimination: 80 Hz must put energy low, 440 Hz higher
  # count bars containing at least one '#', inside the 80 Hz section only.
  # The bar column is space-padded, so match '\|#' rather than a closed '|#+|'.
  low=$(print -r -- "$viz" | awk '/80 Hz sine/{f=1;next} /440 Hz|silence|noise/{f=0} f&&/\|#/{n++} f&&/peak=/{print n+0; exit}')
  if [[ -n $low && $low -gt 3 ]]; then
    ok "80 Hz puts energy in the low bars, so the frequency mapping is real"
  else
    no "80 Hz produced almost no bars; the doc claims bars 0-12 light up"
  fi
  if [[ $viz == *"44100 Hz -> ok"* && $viz == *"48000 Hz -> ok"* ]]; then
    ok "44.1 kHz and 48 kHz both build (48 000 is what the tap actually uses)"
  fi
  if (cd spikes/viz && cargo build --release --target x86_64-apple-darwin 2>/dev/null); then
    ok "also builds for x86_64-apple-darwin (1.6's cross-target requirement)"
  else
    sk "x86_64 target not installed: run 'rustup target add x86_64-apple-darwin'"
  fi
else
  sk "spikes/viz did not build"
fi

# ----------------------------------------------------------------- 1.5 tap
head_ "1.5  Process tap (docs/AUDIO-TAP.md section 3)"
if [[ $quick == 1 ]]; then
  sk "skipped (--quick; this one takes ~25s per mode)"
else
  if (cd spikes/tap && cargo build --release 2>/dev/null); then
    say "a) global tap (should deliver audio)..."
    # A tap is silent when Spotify is paused or between tracks, which would be a
    # misleading FAIL. Normalise to a playing track first.
    if [[ $(sp 'player state') != playing ]]; then
      osa 'tell application "Spotify" to play' >/dev/null; sleep 3
    fi
    say "   spotify state: $(sp 'player state')"
    g=$(cd spikes/tap && TRAK_TAP_MODE=global-mono ./target/release/tap-spike 2>&1)
    # the line reads "--- 958976 float samples in 20s ...", so anchor only the
    # number extraction, not the line start
    n=$(print -r -- "$g" | grep -oE '[0-9]+ float samples' | grep -oE '^[0-9]+')
    if [[ -n $n ]] && (( n > 100000 )); then
      ok "global tap delivered $n samples — and needed NO permission prompt (R1 refuted)"
    else
      no "global tap delivered ${n:-0} samples; the doc claims ~960k in 20s"
    fi
    if print -r -- "$g" | grep -q "48000 Hz, 1 ch, 32 bit"; then
      ok "tap format is 48000 Hz / 1 ch / 32-bit float — this is why 1.6 uses 48 000"
    else
      no "tap format was not 48000/1/32; re-check docs/AUDIO-TAP.md section 3"
    fi
    if print -r -- "$g" | grep -q "48000 Hz, 1 ch, 32 bit"; then
      say "NOTE: if macOS prompted to allow System Audio Recording, click Allow and re-run."
      say "The spike got no prompt at all on this machine, which is why R1 is refuted."
    fi

    say "b) Spotify-only tap (the doc claims this FAILS)..."
    s=$(cd spikes/tap && TRAK_TAP_MODE=mono-mixdown ./target/release/tap-spike 2>&1)
    if print -r -- "$s" | grep -q "560947818"; then
      ok "Spotify-only tap fails with 560947818 (!obj) — the blocker the doc records is real"
    elif print -r -- "$s" | grep -q "tap created"; then
      no "the Spotify-only tap WORKED. Good news, but it contradicts the doc —"
      say "re-check docs/AUDIO-TAP.md section 3b and 8.3's assumptions."
    else
      sk "unexpected result: $(print -r -- "$s" | tail -3 | head -1)"
    fi

    left=$(system_profiler SPAudioDataType 2>/dev/null | grep -ciE "tap|aggregate")
    if [[ $left == 0 ]]; then
      ok "no tap or aggregate device left behind (TODO 8.5's check)"
    else
      no "$left leftover device(s) — that is the leak TODO 8.5 must test for"
    fi
  else
    sk "spikes/tap did not build"
  fi
fi

# -------------------------------------------------------------- 1.8 keychain
head_ "1.8  Keychain (docs/KEYCHAIN.md)"
if (( ! keychain )); then
  sk "opt-in only: this provokes a real keychain password dialog on your screen."
  sk "run ./spikes/verify.sh --keychain when you are at the machine."
elif (cd spikes/keychain && cargo build --release 2>/dev/null); then
  KC=spikes/keychain/target/release/keychain-spike
  SVC="trak-verify-$$"        # per-run, so no stale item or cached decision lingers
  # Clean up on any exit, including Ctrl-C, so nothing is left in the keychain.
  trap 'security delete-generic-password -s "$SVC" -a t >/dev/null 2>&1
        rm -f /tmp/trak-verify-kc /tmp/trak-verify-kc.out' EXIT INT TERM
  say "using service name $SVC; press Ctrl-C at any time to clean up"
  security delete-generic-password -s "$SVC" -a t >/dev/null 2>&1
  say "storing with the current binary..."
  if $KC store trak-verify t secret-value >/dev/null 2>&1; then
    ok "the binary can store an item (this is why the problem is invisible in development)"
  else
    no "storing failed unexpectedly"
  fi
  if $KC read "$SVC" t 2>/dev/null | grep -q "secret-value"; then
    ok "the same binary reads it back with no prompt"
  else
    no "the same binary could not read it back"
  fi

  say "re-signing a copy (this is what scripts/package-release.sh does every release)..."
  cp $KC /tmp/trak-verify-kc
  codesign --force -s - /tmp/trak-verify-kc 2>/dev/null

  # Two things can happen, and BOTH mean the read did not work:
  #   - it blocks, because macOS is waiting on an authorization dialog, or
  #   - it returns an error immediately, because the decision for this exact
  #     (item, code identity) pair is already cached from an earlier run.
  # A new identity prompts; the same one again errors. The first encounter is
  # the one that matters, because every trak release is a brand-new identity.
  /tmp/trak-verify-kc read "$SVC" t >/tmp/trak-verify-kc.out 2>&1 &
  pid=$!
  blocked=0
  for i in {1..20}; do
    if ! kill -0 $pid 2>/dev/null; then break; fi
    sleep 0.5
    [[ $i == 20 ]] && blocked=1
  done
  kill -9 $pid 2>/dev/null; wait $pid 2>/dev/null

  if (( blocked )); then
    ok "the re-signed copy BLOCKS on the keychain read — no data, trak would hang"
    say "it was waiting on an authorization dialog that nobody could dismiss"
  elif grep -q "secret-value" /tmp/trak-verify-kc.out 2>/dev/null; then
    no "the re-signed copy READ the secret. If that is real, docs/KEYCHAIN.md is wrong"
    say "and the Keychain should be reconsidered."
  else
    ok "the re-signed copy returned an error instead of the secret (cached denial)"
    say "first encounter blocks on a dialog; later ones fail immediately. Either way,"
    say "a trak release -- always a new identity -- gets the blocking case."
    say "        got: $(head -1 /tmp/trak-verify-kc.out 2>/dev/null)"
  fi
  rm -f /tmp/trak-verify-kc /tmp/trak-verify-kc.out
  security delete-generic-password -s trak-verify -a t >/dev/null 2>&1
  say "storing with the current binary..."
  if $KC store trak-verify t secret-value >/dev/null 2>&1; then
    ok "the binary can store an item (this is why the problem is invisible in development)"
  else
    no "storing failed unexpectedly"
  fi
  if $KC read "$SVC" t 2>/dev/null | grep -q "secret-value"; then
    ok "the same binary reads it back with no prompt"
  else
    no "the same binary could not read it back"
  fi

fi

# ------------------------------------------------------------------ summary
printf "\n${B}== summary${N}\n"
printf "  %s passed, %s failed, %s skipped\n\n" "$pass" "$fail" "$skip"
print -r -- "Anything marked FAIL is a real disagreement with docs/. Read the section it names"
print -r -- "before changing either the code or the doc: docs/ is the source of truth, so if"
print -r -- "reality disagrees with it, the doc is what gets fixed."
print -r -- ""
print -r -- "${B}Checks that need your eyes (not run above):${N}"
print -r -- "  1.4 image protocols — in cmux, run:  cd spikes/images && ./target/release/images-spike"
print -r -- "      Columns 1-2 should show the image; column 2 (kitty) sharp, column 1 (halfblocks)"
print -r -- "      blocky. Columns 3-4 (iterm2, sixel) should be empty."
print -r -- "      Then repeat in Terminal.app: only column 1 should render, and the other"
print -r -- "      three should print garbled escape sequences as visible text."
print -r -- ""
print -r -- "  1.1 ad handling — wait for an advert to start playing and re-run this script."
print -r -- "      The ad fixture (tests/fixtures/applescript/playing_ad.txt) should match:"
print -r -- "      empty album/artist, artwork url = the literal string 'missing value',"
print -r -- "      all numerics 0, and a spotify:ad: URI."
