#!/usr/bin/env python3
"""Record the README's demo GIF from trak's own output.

Trak writes an ordinary escape-sequence stream, so the frames do not need a
window: run it in a pty, reconstruct the screen after every frame with
`screen.py`'s emulator, draw those cells with PIL, and assemble a GIF. That makes
the demo reproducible (`scripts/record-demo.py` is the recipe), and it needs
nothing but a terminal -- no window id, no focus, no clicking.

Against real Spotify, so the cover art, the accent and the lyrics are the real
thing. `TERM` is deliberately **not** set to anything cmux-like: this records the
half-block path, which every terminal can draw.

    python3 scripts/record-demo.py --out docs/images/trak-demo.gif
"""

import argparse
import fcntl
import os
import pty
import select
import struct
import sys
import termios
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from screen import Screen  # noqa: E402

# The frame the README shows, at the size it is shown.
COLS, ROWS = 110, 34
CELL_W, CELL_H = 8, 17
FONT = "/System/Library/Fonts/Menlo.ttc"

# The demo: the same song as before, and every main feature in the order a person
# meets them. `keys` is written to the terminal raw, so the arrow keys go as the
# escape sequences a terminal sends.
SCENE = [
    ("Now Playing: cover, bar, transport, history", "", 2.5),
    ("the visualizer", "a", 2.5),
    ("mirrored", "v", 2.0),
    ("waveform", "v", 2.0),
    ("the cover again", "a", 2.0),
    ("the volume meter", "+", 1.2),
    ("the lyrics tab", "\x1b[D", 2.5),
    ("full-screen lyrics", "L", 3.0),
    ("and back", "\x1b", 1.2),
    ("the settings screen", "?", 3.0),
]

def spawn(binary, env=None):
    pid, fd = pty.fork()
    if pid == 0:
        # The size has to be set *before* the program reads it: trak asks the
        # terminal once at startup and then only redraws when something changes,
        # so a size applied a moment later leaves a screen drawn for the pty's
        # default 80x24 with the rest of it never painted.
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        os.environ["TERM"] = "xterm-256color"
        os.environ.pop("KITTY_WINDOW_ID", None)
        os.environ.pop("TERM_PROGRAM", None)
        os.environ.setdefault("TRAK_OSASCRIPT", os.path.expanduser("~/.local/bin/osascript"))
        for k, v in (env or {}).items():
            os.environ[k] = v
        os.execv(binary, [binary])
    # Also from the parent, and a SIGHUP so trak is told the size changed and
    # repaints. A program that asks the terminal once at startup and then only
    # redraws on a change otherwise keeps a screen drawn for the pty's default.
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    # No SIGHUP: a terminal hangup is how a program is told to die, and trak has
    # no reason to survive one.
    return pid, fd


def read_frames(fd, screen, want, deadline_seconds):
    """Read until `want` distinct frames have been seen, or time is up."""
    frames = []
    seen = set()
    last_change = time.time()
    end = time.time() + deadline_seconds
    while time.time() < end and len(frames) < want:
        r, _, _ = select.select([fd], [], [], 0.05)
        if not r:
            continue
        try:
            data = os.read(fd, 65536)
        except OSError:
            break
        if not data:
            break
        screen.feed(data)
        # Deliberately **not** answered. trak asks the terminal for its background
        # once at startup and waits 300 ms for a reply; a pty has no terminal behind
        # it, so the answer never comes and trak carries on with the default. Writing
        # a reply here looks helpful and is not: the bytes go into the pty's input,
        # where the line discipline may echo them into the frame (they really did) and
        # where trak reads them as keypresses.
        key = snapshot(screen)
        if key != seen:
            seen.add(key)
            frames.append(copy_screen(screen))
            last_change = time.time()
    return frames


def settle(fd, screen, quiet=0.45, limit=8.0):
    """Read until the stream has been quiet for `quiet` seconds.

    ratatui paints a screen incrementally, so a frame captured mid-paint is a
    half-drawn screen -- and a screen that never changes again is never finished.
    A demo frame has to be taken after the last write, not after the first.
    """
    end = time.time() + limit
    last = time.time()
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if not r:
            if time.time() - last > quiet:
                return
            continue
        try:
            data = os.read(fd, 65536)
        except OSError:
            return
        if not data:
            return
        screen.feed(data)
        # Deliberately **not** answered. trak asks the terminal for its background
        # once at startup and waits 300 ms for a reply; a pty has no terminal behind
        # it, so the answer never comes and trak carries on with the default. Writing
        # a reply here looks helpful and is not: the bytes go into the pty's input,
        # where the line discipline may echo them into the frame (they really did) and
        # where trak reads them as keypresses.
        last = time.time()


def copy_screen(screen):
    """A copy, because the frames are compared after the fact."""
    other = Screen(screen.w, screen.h)
    other.chars = [row[:] for row in screen.chars]
    other.fg = [row[:] for row in screen.fg]
    other.bg = [row[:] for row in screen.bg]
    other.bold = [row[:] for row in screen.bold]
    return other


def ink(screen):
    """How much of the screen has something on it."""
    return sum(
        1 for y in range(screen.h) for x in range(screen.w) if screen.chars[y][x] != " "
    )


def snapshot(screen):
    """A cheap identity for a frame: the cells and their colours."""
    return "".join(
        "".join(
            f"{screen.chars[y][x]}|{screen.fg[y][x]}|{screen.bg[y][x]}"
            for x in range(screen.w)
        )
        for y in range(screen.h)
    )


def draw(screen, font):
    """The screen as a PIL image: one glyph per cell, colours and all."""
    from PIL import Image, ImageDraw

    img = Image.new(
        "RGB", (screen.w * CELL_W, screen.h * CELL_H), (12, 12, 14)
    )
    d = ImageDraw.Draw(img)
    for y in range(screen.h):
        for x in range(screen.w):
            fg = screen.fg[y][x] or (214, 214, 214)
            bg = screen.bg[y][x]
            px, py = x * CELL_W, y * CELL_H
            if bg:
                d.rectangle([px, py, px + CELL_W, py + CELL_H], fill=bg)
            ch = screen.chars[y][x]
            if ch and ch != " ":
                colour = fg
                if screen.bold[y][x]:
                    colour = tuple(min(255, int(c * 1.25 + 24)) for c in fg)
                d.text((px, py + 1), ch, font=font, fill=colour)
    return img


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="docs/images/trak-demo.gif")
    ap.add_argument("--binary", default="target/release/trak")
    ap.add_argument("--fps", type=int, default=12)
    ap.add_argument("--preview", default=None, help="also write one PNG per scene here")
    ap.add_argument(
        "--real",
        action="store_true",
        help="record against the real Spotify instead of scripts/fake-spotify",
    )
    args = ap.parse_args()

    from PIL import Image, ImageFont

    font = ImageFont.truetype(FONT, 15)
    here = os.path.dirname(os.path.abspath(__file__))
    env = {}
    if not args.real:
        # The scripted player, so the frame timing is repeatable and the demo does
        # not need the owner's music to be doing anything. The cover art, the accent
        # and the lyrics are still real: the artwork URL is fetched from Spotify's
        # CDN and the words come from LRCLIB.
        env["TRAK_OSASCRIPT"] = os.path.join(here, "fake-spotify")
        env["STUB_LOG"] = "/dev/null"
        env["XDG_CONFIG_HOME"] = os.path.join(here, "..", "target", "scratch", "demo-xdg")
        # `XDG_CONFIG_HOME` is the *parent*: trak keeps its file in
        # `<xdg>/trak/config.toml`, so the demo's own directory goes inside it.
        env["XDG_CONFIG_HOME"] = os.path.join(env["XDG_CONFIG_HOME"], "trak")
        os.makedirs(env["XDG_CONFIG_HOME"], exist_ok=True)
        # The simulated visualizer, so the bars are the scripted player's and not
        # whatever the real Spotify happens to be playing (or not playing) while the
        # demo is recorded. A tap on a paused Spotify draws a flat line.
        with open(os.path.join(env["XDG_CONFIG_HOME"], "config.toml"), "w") as fh:
            fh.write(
                "[visualizer]\nsource = \"simulated\"\n"
                # Lyrics are off by default (a network call nobody asked for), and
                # a demo that shows the Lyrics tab with nothing in it is not a demo.
                "[lyrics]\nenabled = true\n"
            )
    pid, fd = spawn(os.path.abspath(args.binary), env)
    screen = Screen(COLS, ROWS)

    # Let the first poll, the cover download and the accent extraction land, and
    # then wait for the paint to finish.
    read_frames(fd, screen, 3, 10.0)
    settle(fd, screen, quiet=0.8)
    all_frames = [draw(screen, font)]

    for i, (label, keys, hold) in enumerate(SCENE):
        if keys:
            os.write(fd, keys.encode())
            settle(fd, screen)
        # The simulated visualizer pulses, so a frame taken at the bottom of a
        # beat is an empty pane. Take a burst and keep the fullest one: the most
        # non-blank cells wins, which is the frame with the most bars on it.
        burst = read_frames(fd, screen, 12, hold)
        settle(fd, screen, quiet=0.35)
        if burst:
            all_frames.append(draw(max(burst, key=ink), font))
        else:
            all_frames.append(draw(screen, font))
        if args.preview:
            os.makedirs(args.preview, exist_ok=True)
            all_frames[-1].save(f"{args.preview}/{i:02d}-{label[:24].replace(' ', '-')}.png")
        print(f"  {i+1}/{len(SCENE)}  {label}", flush=True)

    os.write(fd, b"q")
    time.sleep(0.4)
    try:
        os.close(fd)
        os.waitpid(pid, 0)
    except OSError:
        pass

    # Longer hold on the last frame so the loop does not blink.
    all_frames.extend([all_frames[-1]] * args.fps)
    ms = 1000 / args.fps
    all_frames[0].save(
        args.out,
        save_all=True,
        append_images=all_frames[1:],
        duration=ms,
        loop=0,
        optimize=True,
        disposal=2,
    )
    size = os.path.getsize(args.out) / 1024 / 1024
    print(f"{args.out}: {len(all_frames)} frames, {size:.1f} MB")


if __name__ == "__main__":
    main()