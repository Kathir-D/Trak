#!/usr/bin/env python3
"""Record the README's demo GIF from trak's own output.

Trak writes an ordinary escape-sequence stream, so the frames do not need a
window: run it in a pty, reconstruct the screen with `screen.py`'s emulator as it
plays, draw those cells with PIL, and have ffmpeg assemble the GIF. That makes the
demo reproducible (`scripts/record-demo.py` is the recipe), and it needs nothing
but a terminal -- no window id, no focus, no clicking.

The terminal is announced as Ghostty, so trak sends the cover through the Kitty
graphics protocol, as it does in cmux; `screen.py` decodes it and the frame shows
the real cover at full resolution rather than half-blocks.

Against `scripts/fake-spotify` by default, a scripted two-song queue, so the
timing is the same every run and nothing the owner is listening to is touched.
The covers, the accent taken from them and the synced lyrics are still real:
the artwork comes from Spotify's CDN and the words from LRCLIB. The frames are
taken at a steady rate while the keys are pressed, so the GIF plays at the speed
it was recorded.

    python3 scripts/record-demo.py --out docs/images/trak-demo.gif
"""

import argparse
import fcntl
import os
import pty
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from screen import PLACEHOLDER, Screen  # noqa: E402

# The frame the README shows, at the size it is shown. The cell is the 8x17 trak
# assumes when the terminal does not answer `CSI 16 t` (a pty does not), so the
# cover trak encodes is exactly the size of the cells it covers.
COLS, ROWS = 110, 34
CELL_W, CELL_H = 8, 17
# Menlo first, then fonts for what it has no glyph for (the transport symbols, the
# braille the visualizer draws with). Without the fallback those cells came out
# as empty boxes. (path, index in the collection; the bold face is index 1.)
FONTS = [
    ("/System/Library/Fonts/Menlo.ttc", 0),
    ("/System/Library/Fonts/SFNSMono.ttf", 0),
    ("/System/Library/Fonts/Apple Symbols.ttf", 0),
    ("/System/Library/Fonts/Apple Braille.ttf", 0),
]
BOLD = ("/System/Library/Fonts/Menlo.ttc", 1)
BACKGROUND = (12, 12, 14)

RIGHT, LEFT, ESC = "\x1b[C", "\x1b[D", "\x1b"


def header(text):
    return lambda screen: text in screen.text().splitlines()[0]


def anywhere(text):
    return lambda screen: text in screen.text()


# The demo: every main feature, quickly, in the order a person meets them. Each
# step is (what it shows, the keys, how long to stay on it in seconds), and for a
# change trak only learns of from Spotify, what to wait for. The keys go to the
# terminal raw, so the arrows are the escape sequences a terminal sends.
#
# The wait is off camera. Real Spotify posts `PlaybackStateChanged` and trak shows
# a skip or a pause in ~170 ms (docs/APPLESCRIPT.md §9); the fake player cannot
# post it without telling every other listener on the machine (another trak
# among them) that Spotify changed, so trak only sees it at its next 3 s poll.
# Leaving that on camera would show a delay real use does not have. Shuffle has
# no notification even on real Spotify, but the wait it would show is the poll's,
# which is not what the step is about.
SCENE = [
    ("Now Playing: the cover, its accent, the history", "", 1.4),
    ("next track: a new cover, and the colours follow it", "n", 1.8, header("Beauty Sleep")),
    ("pause", " ", 0.5, header("\u23f8")),
    ("play", " ", 0.3, header("\u25b6")),
    ("shuffle", "s", 0.4, anywhere("\u21c4")),
    ("repeat", "r", 0.5),
    ("volume up", "+", 0.25),
    ("volume up", "+", 0.35),
    ("mute", "m", 0.4),
    ("unmute", "m", 0.6),
    ("the visualizer: spectrum", "a", 0.85),
    ("mirrored", "v", 0.8),
    ("waveform", "v", 0.8),
    ("circular", "v", 0.85),
    ("the cover again", "a", 0.25),
    ("synced lyrics", "6", 1.4),
    ("full-screen lyrics", "L", 1.6),
    ("back", ESC, 0.25),
    ("history", RIGHT, 0.2),
    ("track info", RIGHT, 1.0),
    ("history", LEFT, 0.3),
    ("previous track: the colours change back", "p", 1.8, header("Dancing with your eyes")),
    ("settings", "?", 1.6),
    ("back", ESC, 0.4),
]


def spawn(binary, env):
    pid, fd = pty.fork()
    if pid == 0:
        # The size has to be set *before* the program reads it: trak asks the
        # terminal once at startup and then only redraws when something changes,
        # so a size applied a moment later leaves a screen drawn for the pty's
        # default 80x24 with the rest of it never painted.
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        os.environ["TERM"] = "xterm-256color"
        os.environ["TERM_PROGRAM"] = "ghostty"
        os.environ.pop("KITTY_WINDOW_ID", None)
        # Only the system's tools. On the machine this is recorded on,
        # headless-spotify is on the PATH and trak says so in the footer, which
        # is true there and noise in a demo of everyone else's terminal.
        os.environ["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
        for k, v in env.items():
            os.environ[k] = v
        os.execv(binary, [binary])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    return pid, fd


def pump(fd, screen, seconds):
    """Feed the screen everything trak writes for `seconds`.

    Deliberately never answers trak's queries (the background colour, the cell
    size): a pty has no terminal behind it, trak carries on with its defaults, and
    a reply written here would land in trak's input as keypresses.
    """
    end = time.time() + seconds
    while True:
        left = end - time.time()
        if left <= 0:
            return True
        r, _, _ = select.select([fd], [], [], min(left, 0.02))
        if not r:
            continue
        try:
            data = os.read(fd, 1 << 20)
        except OSError:
            return False
        if not data:
            return False
        screen.feed(data)


def drain(fd, screen, quiet=0.005, limit=0.05):
    """Read until trak has stopped writing for `quiet` seconds.

    ratatui writes a frame in pieces and trak does not wrap it in synchronized
    output, so a screen taken mid-write is half one frame and half the next -- a
    cover cut off half-way down the visualizer pane, in one still that was taken
    that way.
    """
    end = time.time() + limit
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], quiet)
        if not r:
            return
        try:
            data = os.read(fd, 1 << 20)
        except OSError:
            return
        if not data:
            return
        screen.feed(data)


def snapshot(screen):
    """A cheap identity for a frame: the cells, their colours and the images."""
    return hash((
        tuple(tuple(r) for r in screen.chars),
        tuple(tuple(r) for r in screen.fg),
        tuple(tuple(r) for r in screen.bg),
        tuple(tuple(r) for r in screen.place),
        tuple((k, hash(v[2])) for k, v in sorted(screen.images.items())),
    ))


def ink(screen):
    """How much of the screen has something on it."""
    return sum(1 for row in screen.chars for ch in row if ch not in (" ", PLACEHOLDER))


def copy_screen(screen):
    """What a frame needs, copied: drawing waits until the recording is over, so
    the capture rate is the tick and not the speed of PIL."""
    other = Screen(screen.w, screen.h)
    for name in ("chars", "fg", "bg", "bold", "place"):
        setattr(other, name, [row[:] for row in getattr(screen, name)])
    other.images = dict(screen.images)
    return other


class Painter:
    def __init__(self):
        from fontTools.ttLib import TTFont
        from PIL import ImageFont

        self.fonts = []
        for path, index in FONTS:
            if not os.path.exists(path):
                continue
            cmap = TTFont(path, fontNumber=index, lazy=True).getBestCmap() or {}
            self.fonts.append((ImageFont.truetype(path, 14, index=index), set(cmap)))
        self.bold = ImageFont.truetype(BOLD[0], 14, index=BOLD[1])
        self.covers = {}

    def font_for(self, ch, bold):
        if bold and ord(ch) in self.fonts[0][1]:
            return self.bold
        for font, has in self.fonts:
            if ord(ch) in has:
                return font
        return self.fonts[0][0]

    def cover(self, image, w, h, raw):
        from PIL import Image

        key = (image, hash(raw))
        if key not in self.covers:
            self.covers = {key: Image.frombytes("RGBA", (w, h), raw).convert("RGB")}
        return self.covers[key]

    def draw(self, screen):
        """The screen as a PIL image: one glyph per cell, and the cover's slices."""
        from PIL import Image, ImageDraw

        img = Image.new("RGB", (screen.w * CELL_W, screen.h * CELL_H), BACKGROUND)
        d = ImageDraw.Draw(img)
        for y in range(screen.h):
            for x in range(screen.w):
                px, py = x * CELL_W, y * CELL_H
                place = screen.place[y][x]
                if place is not None and screen.chars[y][x] == PLACEHOLDER:
                    found = screen.images.get(place[0])
                    if found:
                        cover = self.cover(place[0], *found)
                        sx, sy = place[2] * CELL_W, place[1] * CELL_H
                        if sx < cover.width and sy < cover.height:
                            img.paste(cover.crop((sx, sy, sx + CELL_W, sy + CELL_H)), (px, py))
                    continue
                fg = screen.fg[y][x] or (214, 214, 214)
                bg = screen.bg[y][x]
                if bg:
                    d.rectangle([px, py, px + CELL_W - 1, py + CELL_H - 1], fill=bg)
                ch = screen.chars[y][x]
                if ch == "\u23f8":
                    # Pause. Only the emoji font has it, and a terminal draws the
                    # text form: two bars.
                    for bx in (1, 5):
                        d.rectangle([px + bx, py + 4, px + bx + 1, py + CELL_H - 5], fill=fg)
                elif ch and ch != " ":
                    font = self.font_for(ch, screen.bold[y][x])
                    d.text((px, py + 1), ch, font=font, fill=fg)
        return img


def assemble(frames, out, width, fps):
    """Frames with durations into a GIF, through ffmpeg's two-pass palette.

    PIL's quantiser gives every frame its own coarse palette, which bands the
    cover and makes the gradients crawl; one palette built from the whole clip,
    weighted to what changes, is what keeps a GIF of album art looking like one.
    """
    tmp = tempfile.mkdtemp(prefix="trak-demo-")
    try:
        lines = []
        for i, (img, secs) in enumerate(frames):
            path = os.path.join(tmp, f"{i:05d}.png")
            img.save(path)
            lines.append(f"file '{path}'\nduration {secs:.3f}\n")
        # The concat demuxer ignores the last duration unless the file is repeated.
        lines.append(f"file '{os.path.join(tmp, f'{len(frames) - 1:05d}.png')}'\n")
        concat = os.path.join(tmp, "frames.txt")
        with open(concat, "w") as fh:
            fh.writelines(lines)
        scale = f"scale={width}:-1:flags=lanczos" if width else "null"
        subprocess.run(
            [
                "ffmpeg", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0",
                "-i", concat, "-vf",
                f"fps={fps:g},{scale},split[a][b];[a]palettegen=max_colors=256:stats_mode=diff[p];"
                "[b][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle",
                "-loop", "0", out,
            ],
            check=True,
        )
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="docs/images/trak-demo.gif")
    ap.add_argument("--binary", default="target/release/trak")
    ap.add_argument("--fps", type=float, default=20)
    ap.add_argument("--width", type=int, default=0, help="scale the GIF to this width")
    ap.add_argument("--preview", default=None, help="also write the last frame of each step here")
    ap.add_argument(
        "--real",
        action="store_true",
        help="record against the real Spotify instead of scripts/fake-spotify",
    )
    args = ap.parse_args()

    here = os.path.dirname(os.path.abspath(__file__))
    xdg = os.path.join(here, "..", "target", "scratch", "demo-xdg")
    # `XDG_CONFIG_HOME` is the *parent*: trak keeps its file in
    # `<xdg>/trak/config.toml`.
    os.makedirs(os.path.join(xdg, "trak"), exist_ok=True)
    with open(os.path.join(xdg, "trak", "config.toml"), "w") as fh:
        fh.write(
            # The simulated visualizer, so the bars are the scripted player's and
            # not whatever the real Spotify happens to be doing. A tap on a paused
            # Spotify draws a flat line.
            "[visualizer]\nsource = \"simulated\"\n"
            # Lyrics are off by default (a network call nobody asked for), and a
            # demo that shows the Lyrics tab with nothing in it is not a demo.
            "[lyrics]\nenabled = true\n"
        )
    env = {"XDG_CONFIG_HOME": xdg}
    if args.real:
        env["TRAK_OSASCRIPT"] = os.path.expanduser("~/.local/bin/osascript")
    else:
        state = os.path.join(xdg, "fake-spotify.json")
        if os.path.exists(state):
            os.remove(state)
        env.update({
            "TRAK_OSASCRIPT": os.path.join(here, "fake-spotify"),
            "STUB_LOG": "/dev/null",
            "STUB_STATE_FILE": state,
        })

    pid, fd = spawn(os.path.abspath(args.binary), env)
    screen = Screen(COLS, ROWS)
    painter = Painter()

    # The first poll, the cover download, the accent and the lyrics, off camera:
    # the GIF opens on a finished screen, not on trak starting up.
    pump(fd, screen, 5.0)

    frames = []  # [image, when it was taken]
    last = None
    # Time spent off camera, taken out of the clock so it is not on screen.
    skipped = 0.0
    tick = 1.0 / args.fps

    def record(seconds):
        # Timed by the wall clock, not by the tick: drawing a frame takes time
        # too, and a GIF timed by the tick alone plays faster than it happened.
        nonlocal last
        end = time.time() + seconds
        while time.time() < end:
            if not pump(fd, screen, max(0.0, min(tick, end - time.time()))):
                return
            drain(fd, screen)
            key = snapshot(screen)
            if key != last:
                frames.append([copy_screen(screen), time.time() - skipped])
                last = key

    previews = []
    for i, (label, keys, hold, *wait) in enumerate(SCENE):
        before = len(frames)
        if keys:
            os.write(fd, keys.encode())
        if wait:
            started = time.time()
            end = started + 6.0
            while time.time() < end and not wait[0](screen):
                pump(fd, screen, 0.05)
            skipped += time.time() - started
        record(hold)
        if args.preview and frames:
            name = label[:28].replace(" ", "-").replace(":", "").replace("/", "")
            # The fullest frame of the step, not its last: the visualizer
            # pulses, and a still taken between beats is an empty pane.
            shots = [f[0] for f in frames[before:]] or [frames[-1][0]]
            previews.append((f"{i:02d}-{name}.png", max(shots, key=ink)))
        print(f"  {i + 1}/{len(SCENE)}  {label} ({len(frames) - before} frames)", flush=True)

    os.write(fd, b"q")
    time.sleep(0.4)
    try:
        os.close(fd)
        os.waitpid(pid, 0)
    except OSError:
        pass

    ended = time.time() - skipped
    if args.preview:
        os.makedirs(args.preview, exist_ok=True)
        for name, shot in previews:
            painter.draw(shot).save(os.path.join(args.preview, name))
    for i, frame in enumerate(frames):
        frame[1] = (frames[i + 1][1] if i + 1 < len(frames) else ended) - frame[1]
        frame[0] = painter.draw(frame[0])
    # A beat on the last frame so the loop does not blink.
    frames[-1][1] += 0.8
    assemble(frames, args.out, args.width, args.fps)
    total = sum(s for _, s in frames)
    size = os.path.getsize(args.out) / 1024 / 1024
    print(f"{args.out}: {len(frames)} distinct frames, {total:.1f} s, {size:.1f} MB")


if __name__ == "__main__":
    main()
