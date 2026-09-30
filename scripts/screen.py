#!/usr/bin/env python3
"""Render trak's TUI in a pty and print the screen as text, with colour.

An agent session cannot see a terminal it spawns -- it can only screenshot a
window, and there is no window here. But the *screen* is recoverable: trak writes
an ordinary CSI/SGR stream, so a small terminal emulator reconstructs exactly what
a user would see. Colours come through as ANSI, so this shows the accent, the
gradient and the half-block art, which a plain text dump would throw away.

    python3 scripts/screen.py                    # 110x34, the default
    python3 scripts/screen.py 80 24              # a different size
    python3 scripts/screen.py 110 34 --keys 'jj?'  # press keys, then shoot
    python3 scripts/screen.py --wait 8           # longer for a download

This is a development aid, not a test: it needs a real Spotify and it talks to the
network. The tests assert layout with ratatui's TestBackend instead.
"""

import argparse
import fcntl
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time

# xterm 256-colour cube, so a palette index can be shown too.
CUBE = [
    (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16), (36, 114, 200),
    (188, 63, 188), (17, 168, 205), (229, 229, 229), (102, 102, 102), (241, 76, 76),
    (35, 209, 139), (245, 245, 67), (59, 142, 234), (214, 112, 214), (41, 184, 219),
    (255, 255, 255),
]


def cell_hex(r, g, b):
    return f"{r:02x}{g:02x}{b:02x}"


class Screen:
    """Just enough terminal to see trak: text, SGR colour, and the usual CSI."""

    def __init__(self, w, h):
        self.w, self.h = w, h
        self.chars = [[" "] * w for _ in range(h)]
        self.fg = [[None] * w for _ in range(h)]
        self.bg = [[None] * w for _ in range(h)]
        self.bold = [[False] * w for _ in range(h)]
        self.cx = self.cy = 0
        self.cur_fg = self.cur_bg = None
        self.cur_bold = False
        self.pending = b""

    def clear(self):
        self.chars = [[" "] * self.w for _ in range(self.h)]
        self.fg = [[None] * self.w for _ in range(self.h)]
        self.bg = [[None] * self.w for _ in range(self.h)]
        self.bold = [[False] * self.w for _ in range(self.h)]

    def put(self, ch):
        if self.cy >= self.h or self.cx >= self.w:
            return
        self.chars[self.cy][self.cx] = ch
        self.fg[self.cy][self.cx] = self.cur_fg
        self.bg[self.cy][self.cx] = self.cur_bg
        self.bold[self.cy][self.cx] = self.cur_bold
        self.cx += 1
        if self.cx >= self.w:
            self.cx = self.w - 1

    def sgr(self, params):
        i = 0
        if not params:
            params = [0]
        while i < len(params):
            p = params[i]
            if p == 0:
                self.cur_fg = self.cur_bg = None
                self.cur_bold = False
            elif p == 1:
                self.cur_bold = True
            elif p == 22:
                self.cur_bold = False
            elif 30 <= p <= 37:
                self.cur_fg = CUBE[p - 30]
            elif p == 39:
                self.cur_fg = None
            elif 40 <= p <= 47:
                self.cur_bg = CUBE[p - 40]
            elif p == 49:
                self.cur_bg = None
            elif 90 <= p <= 97:
                self.cur_fg = CUBE[p - 90 + 8]
            elif 100 <= p <= 107:
                self.cur_bg = CUBE[p - 100 + 8]
            elif p in (38, 48) and i + 1 < len(params):
                target = "fg" if p == 38 else "bg"
                if params[i + 1] == 2 and i + 4 < len(params):
                    rgb = (params[i + 2], params[i + 3], params[i + 4])
                    i += 4
                elif params[i + 1] == 5 and i + 2 < len(params):
                    n = params[i + 2]
                    rgb = CUBE[n % 16] if n < 16 else (128, 128, 128)
                    i += 2
                else:
                    i += 1
                    continue
                if target == "fg":
                    self.cur_fg = rgb
                else:
                    self.cur_bg = rgb
            i += 1

    def feed(self, data):
        # A multi-byte glyph can be split across two reads, and decoding half of
        # one turns the rest of the stream into mojibake -- which then desyncs the
        # escape parser and leaks SGR sequences into the text. Hold the tail until
        # the rest of it arrives.
        data = self.pending + data
        self.pending = b""
        for cut in range(len(data), max(len(data) - 4, 0), -1):
            try:
                data[:cut].decode("utf-8")
            except UnicodeDecodeError:
                continue
            self.pending = data[cut:]
            data = data[:cut]
            break
        i, n = 0, len(data)
        while i < n:
            b = data[i]
            if b == 0x1B:
                m = re.match(rb"\x1b\[([0-9;?]*)([@-~])", data[i:])
                if m:
                    raw, cmd = m.group(1).decode(), m.group(2).decode()
                    nums = [int(x) for x in raw.split(";") if x.isdigit()]
                    if cmd == "H":
                        self.cy = (nums[0] if nums else 1) - 1
                        self.cx = (nums[1] if len(nums) > 1 else 1) - 1
                    elif cmd == "J":
                        self.clear()
                    elif cmd == "K":
                        if 0 <= self.cy < self.h:
                            for x in range(self.cx, self.w):
                                self.chars[self.cy][x] = " "
                    elif cmd == "m":
                        self.sgr(nums)
                    i += m.end()
                    continue
                # The Kitty graphics APC: trak sends the cover through it and it is
                # not text, so count it and move on.
                m = re.match(rb"\x1b_G.*?\x1b\\", data[i:], re.S)
                if m:
                    i += m.end()
                    continue
                m = re.match(rb"\x1b[()][B0]|\x1b[=>]|\x1b\][^\x07]*\x07", data[i:], re.S)
                if m:
                    i += m.end()
                    continue
                i += 1
                continue
            if b == 0x0A:
                self.cy += 1
                i += 1
                continue
            if b == 0x0D:
                self.cx = 0
                i += 1
                continue
            width = 1
            if b >= 0xF0:
                width = 4
            elif b >= 0xE0:
                width = 3
            elif b >= 0xC0:
                width = 2
            try:
                ch = data[i:i + width].decode("utf-8")
            except UnicodeDecodeError:
                i += 1
                continue
            i += width
            # A wide glyph takes two cells; the second is a continuation.
            self.put(ch)
            if ord(ch) >= 0x1100 and (
                0x1100 <= ord(ch) <= 0x115F
                or 0x2E80 <= ord(ch) <= 0xA4CF
                or 0xAC00 <= ord(ch) <= 0xD7A3
                or 0xF900 <= ord(ch) <= 0xFAFF
                or 0xFE30 <= ord(ch) <= 0xFE6F
                or 0xFF00 <= ord(ch) <= 0xFF60
                or 0xFFE0 <= ord(ch) <= 0xFFE6
            ):
                self.cx += 1

    def text(self):
        # An SGR sequence can end up in the cell grid when the stream desyncs; it
        # is noise in a layout review, so it is dropped rather than debugged.
        out = []
        for row in self.chars:
            line = "".join(row).rstrip()
            line = re.sub(r"\x1b?\[[0-9;?]*[A-Za-z]", "", line)
            out.append(line)
        return "\n".join(out)

    def colors(self):
        """Every distinct colour pair on screen, and how many cells use it."""
        seen = {}
        for y in range(self.h):
            for x in range(self.w):
                key = (self.fg[y][x], self.bg[y][x], self.bold[y][x])
                if self.chars[y][x] == " " and key == (None, None, False):
                    continue
                seen[key] = seen.get(key, 0) + 1
        return sorted(seen.items(), key=lambda kv: -kv[1])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cols", nargs="?", type=int, default=110)
    ap.add_argument("rows", nargs="?", type=int, default=34)
    ap.add_argument("--wait", type=float, default=6.0)
    ap.add_argument("--keys", default="")
    ap.add_argument("--colors", action="store_true", help="list the colours used")
    ap.add_argument("--binary", default="./target/release/trak")
    args = ap.parse_args()

    env = dict(os.environ)
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(args.binary, [args.binary], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", args.rows, args.cols, 0, 0))

    screen = Screen(args.cols, args.rows)

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    return
                if not chunk:
                    return
                screen.feed(chunk)

    pump(args.wait)
    for key in args.keys:
        os.write(fd, key.encode())
        pump(0.7)
    pump(1.0)

    print(screen.text())
    if args.colors:
        print("\ncolours (fg, bg, bold) -> cells:")
        for (fg, bg, bold), count in screen.colors()[:14]:
            f = cell_hex(*fg) if fg else "------"
            b = cell_hex(*bg) if bg else "------"
            print(f"  {f} on {b}{' bold' if bold else ''}: {count}")

    try:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    except (ProcessLookupError, ChildProcessError):
        pass


if __name__ == "__main__":
    sys.exit(main())