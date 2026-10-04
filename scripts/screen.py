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
import base64
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
import unicodedata

# xterm 256-colour cube, so a palette index can be shown too.
CUBE = [
    (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16), (36, 114, 200),
    (188, 63, 188), (17, 168, 205), (229, 229, 229), (102, 102, 102), (241, 76, 76),
    (35, 209, 139), (245, 245, 67), (59, 142, 234), (214, 112, 214), (41, 184, 219),
    (255, 255, 255),
]

# The rest of the xterm 256-colour palette: 16-231 is the 6x6x6 cube and 232-255 the
# greys. Without it every 256-colour index above 15 renders as the same grey, which
# makes a cover look like a placeholder -- the half-block art is *all* 256-colour
# indices, so it is the one thing this has to get right.
STEPS = (0, 95, 135, 175, 215, 255)
PALETTE = list(CUBE)
for r in STEPS:
    for g in STEPS:
        for b in STEPS:
            PALETTE.append((r, g, b))
for i in range(24):
    v = 8 + i * 10
    PALETTE.append((v, v, v))


def palette_index(n):
    return PALETTE[n] if 0 <= n < len(PALETTE) else None


def parse_osc_colour(value):
    """`rgb:1e1e/1e1e/1e1e` or `#1e1e1e` into an (r, g, b) tuple."""
    v = value.strip()
    if v.startswith("rgb:"):
        parts = v[4:].split("/")
        try:
            out = []
            for p in parts[:3]:
                p = p.split("(")[0]
                if len(p) in (1, 2, 4):
                    out.append(int(p * 2, 16) if len(p) in (1, 3) else int(p, 16))
                else:
                    out.append(int(p[:2], 16))
            return tuple(min(255, max(0, c)) for c in out)
        except ValueError:
            return None
    if v.startswith("#") and len(v) >= 7:
        try:
            return tuple(int(v[i : i + 2], 16) for i in (1, 3, 5))
        except ValueError:
            return None
    return None


def cell_hex(r, g, b):
    return f"{r:02x}{g:02x}{b:02x}"


# The sequences `feed` knows, matched *at an offset* into the read: slicing the
# rest of the buffer at every escape copied it each time, which made a busy frame
# quadratic to parse and slow enough that the pty filled and trak stalled.
CSI = re.compile(rb"\x1b\[([0-9;?]*)([@-~])")
APC = re.compile(rb"\x1b_G.*?\x1b\\", re.S)
OSC = re.compile(rb"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)", re.S)
SHORT = re.compile(rb"\x1b[()][B0]|\x1b[=>]")


# Kitty's row/column diacritics, in order: the n-th one means row (or column) n.
# The first 128 of the table at
# https://sw.kovidgoyal.net/kitty/_downloads/1792bad15b12979994cd6ecc54c967a6/rowcolumn-diacritics.txt
# which is far more rows and columns than a cover ever has.
DIACRITICS = {
    cp: n for n, cp in enumerate([
        0x305, 0x30D, 0x30E, 0x310, 0x312, 0x33D, 0x33E, 0x33F, 0x346, 0x34A, 0x34B, 0x34C,
        0x350, 0x351, 0x352, 0x357, 0x35B, 0x363, 0x364, 0x365, 0x366, 0x367, 0x368, 0x369,
        0x36A, 0x36B, 0x36C, 0x36D, 0x36E, 0x36F, 0x483, 0x484, 0x485, 0x486, 0x487, 0x592,
        0x593, 0x594, 0x595, 0x597, 0x598, 0x599, 0x59C, 0x59D, 0x59E, 0x59F, 0x5A0, 0x5A1,
        0x5A8, 0x5A9, 0x5AB, 0x5AC, 0x5AF, 0x5C4, 0x610, 0x611, 0x612, 0x613, 0x614, 0x615,
        0x616, 0x617, 0x657, 0x658, 0x659, 0x65A, 0x65B, 0x65D, 0x65E, 0x6D6, 0x6D7, 0x6D8,
        0x6D9, 0x6DA, 0x6DB, 0x6DC, 0x6DF, 0x6E0, 0x6E1, 0x6E2, 0x6E4, 0x6E7, 0x6E8, 0x6EB,
        0x6EC, 0x730, 0x732, 0x733, 0x735, 0x736, 0x73A, 0x73D, 0x73F, 0x740, 0x741, 0x743,
        0x745, 0x747, 0x749, 0x74A, 0x7EB, 0x7EC, 0x7ED, 0x7EE, 0x7EF, 0x7F0, 0x7F1, 0x7F3,
        0x816, 0x817, 0x818, 0x819, 0x81B, 0x81C, 0x81D, 0x81E, 0x81F, 0x820, 0x821, 0x822,
        0x823, 0x825, 0x826, 0x827, 0x829, 0x82A, 0x82B, 0x82C,
    ])
}
# The Kitty "unicode placeholder": a cell that shows a slice of an image.
PLACEHOLDER = "\U0010EEEE"


# A whole escape sequence, for the tail check above: CSI, OSC, the Kitty graphics
# APC, a character-set switch and the two one-letter sequences crossterm emits.
COMPLETE_ESCAPE = re.compile(
    rb"\x1b(?:\[[0-9;?]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|_[^\x1b]*\x1b\\|[()][B0]|[>=])",
    re.S,
)


class Screen:
    """Just enough terminal to see trak: text, SGR colour, and the usual CSI."""

    def __init__(self, w, h):
        self.w, self.h = w, h
        self.chars = [[" "] * w for _ in range(h)]
        self.fg = [[None] * w for _ in range(h)]
        self.bg = [[None] * w for _ in range(h)]
        self.bold = [[False] * w for _ in range(h)]
        self.cx = self.cy = 0
        # Set when trak asks for the terminal background, and what it answered.
        self.queried_background = False
        self.background = None
        self.cur_fg = self.cur_bg = None
        self.cur_bold = False
        self.pending = b""
        # The Kitty graphics trak sends: images by id, as (width, height, RGBA),
        # and for each cell that shows one, (id, row, column) of the slice.
        self.images = {}
        self.transmit = None
        self.place = [[None] * w for _ in range(h)]
        self.saved = (0, 0)
        self.diacritics = 0

    def clear(self):
        self.chars = [[" "] * self.w for _ in range(self.h)]
        self.fg = [[None] * self.w for _ in range(self.h)]
        self.bg = [[None] * self.w for _ in range(self.h)]
        self.bold = [[False] * self.w for _ in range(self.h)]
        self.place = [[None] * self.w for _ in range(self.h)]

    def put(self, ch):
        if self.cy >= self.h or self.cx >= self.w:
            return
        if self.diacritic(ch):
            return
        self.diacritics = 0
        self.place[self.cy][self.cx] = None
        if ch == PLACEHOLDER:
            # Kitty's rule: a placeholder with no diacritics carries on from the
            # one to its left, one column further along. The image id is the
            # foreground colour.
            fg = self.cur_fg or (0, 0, 0)
            image = (fg[0] << 16) | (fg[1] << 8) | fg[2]
            left = self.place[self.cy][self.cx - 1] if self.cx > 0 else None
            if left and left[0] == image:
                self.place[self.cy][self.cx] = (image, left[1], left[2] + 1)
            else:
                self.place[self.cy][self.cx] = (image, 0, 0)
        self.chars[self.cy][self.cx] = ch
        self.fg[self.cy][self.cx] = self.cur_fg
        self.bg[self.cy][self.cx] = self.cur_bg
        self.bold[self.cy][self.cx] = self.cur_bold
        self.cx += 1
        if self.cx >= self.w:
            self.cx = self.w - 1

    def diacritic(self, ch):
        """A combining mark belongs to the cell before it, and does not move on.

        After a placeholder, the first one is the row of the slice and the second
        the column (a third would be the high byte of the id, always 0 here).
        """
        # Combining marks only: the range Kitty draws its diacritics from also
        # holds Hebrew and Arabic letters, which are text and take a cell.
        if not unicodedata.combining(ch):
            return False
        x = self.cx - 1 if self.cx > 0 else 0
        cell = self.place[self.cy][x]
        n = DIACRITICS.get(ord(ch))
        if cell is not None and n is not None:
            if self.diacritics == 0:
                self.place[self.cy][x] = (cell[0], n, cell[2])
            elif self.diacritics == 1:
                self.place[self.cy][x] = (cell[0], cell[1], n)
        self.diacritics += 1
        return True

    def graphics(self, seq):
        """One Kitty graphics APC. Only what ratatui-image sends: a transmit of raw
        RGBA (`f=32`) in base64 chunks, `m=1` while more are coming."""
        body = seq[3:-2]
        head, _, payload = body.partition(b";")
        keys = dict(
            kv.split(b"=", 1) for kv in head.split(b",") if b"=" in kv
        )
        if b"i" in keys or self.transmit is None:
            try:
                self.transmit = (
                    int(keys.get(b"i", b"0")),
                    int(keys.get(b"s", b"0")),
                    int(keys.get(b"v", b"0")),
                    [],
                )
            except ValueError:
                self.transmit = None
                return
        self.transmit[3].append(payload)
        if keys.get(b"m", b"0") == b"0":
            image, w, h, chunks = self.transmit
            self.transmit = None
            try:
                raw = b"".join(base64.b64decode(c) for c in chunks)
            except ValueError:
                return
            if w and h and len(raw) >= w * h * 4:
                self.images[image] = (w, h, raw[: w * h * 4])

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
                    rgb = palette_index(params[i + 2]) or CUBE[7]
                    i += 2
                else:
                    i += 1
                    continue
                if target == "fg":
                    self.cur_fg = rgb
                else:
                    self.cur_bg = rgb
            i += 1

    def osc(self, seq):
        """One OSC string. Trak only asks about the background (OSC 11)."""
        body = seq[2:].rstrip(b"\x07").rstrip(b"\x1b\\")
        if body.startswith(b"11;"):
            value = body[3:].decode("utf-8", "replace")
            if value.startswith("?"):
                # A query, not a reply: there is no terminal here to answer it.
                self.queried_background = True
                return
            self.background = parse_osc_colour(value)

    def feed(self, data):
        # A multi-byte glyph can be split across two reads, and decoding half of
        # one turns the rest of the stream into mojibake -- which then desyncs the
        # escape parser and leaks SGR sequences into the text. Hold the tail until
        # the rest of it arrives.
        data = self.pending + data
        self.pending = b""
        # Two kinds of tail have to be held back, and holding back only the first
        # is what made this tool lie: a `38;5;` split across two reads parsed as
        # text, so a screen came out with colour codes printed on it and the layout
        # looked torn when it was not.
        cut = None
        for c in range(len(data), max(len(data) - 4, 0), -1):
            try:
                data[:c].decode("utf-8")
            except UnicodeDecodeError:
                continue
            cut = c
            break
        # No prefix decoded: the whole chunk is the front of a glyph, so hold it.
        cut = 0 if cut is None else cut
        # An escape sequence whose tail has not arrived yet.
        tail = data.rfind(b"\x1b")
        if tail != -1 and cut > tail and not COMPLETE_ESCAPE.match(data[tail:]):
            cut = tail
        # A read that ends between a graphics chunk's `ESC` and its `\` leaves the
        # last ESC as the terminator's, so the check above holds back only that
        # and the chunk is lost -- which tore a cover sent after a track change.
        # Hold back from the start of any APC that has not been closed.
        apc = data.rfind(b"\x1b_")
        if apc != -1 and data.find(b"\x1b\\", apc) == -1:
            cut = min(cut, apc)
        self.pending = data[cut:]
        data = data[:cut]
        i, n = 0, len(data)
        while i < n:
            b = data[i]
            if b == 0x1B:
                m = CSI.match(data, i)
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
                                self.place[self.cy][x] = None
                    elif cmd == "m":
                        self.sgr(nums)
                    elif cmd == "s":
                        self.saved = (self.cx, self.cy)
                    elif cmd == "u":
                        self.cx, self.cy = self.saved
                    elif cmd in "ABCD":
                        step = nums[0] if nums else 1
                        dx, dy = {"A": (0, -1), "B": (0, 1), "C": (1, 0), "D": (-1, 0)}[cmd]
                        self.cx = max(0, min(self.w - 1, self.cx + dx * step))
                        self.cy = max(0, min(self.h - 1, self.cy + dy * step))
                    i = m.end()
                    continue
                # The Kitty graphics APC: trak sends the cover through it. It is
                # not text, but it is the picture, so keep it.
                m = APC.match(data, i)
                if m:
                    self.graphics(m.group(0))
                    i = m.end()
                    continue
                # An OSC string, ended by BEL or by ST (`ESC \`). trak asks the
                # terminal for its background with OSC 11 at startup, and an OSC
                # that is not consumed leaks its text into the grid and desyncs
                # everything after it -- which looks exactly like a layout bug.
                m = OSC.match(data, i)
                if m:
                    self.osc(m.group(0))
                    i = m.end()
                    continue
                m = SHORT.match(data, i)
                if m:
                    i = m.end()
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
            # **With** the ESC: without it this also matched a tab label like
            # `[History]` and quietly ate its first letter.
            line = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", line)
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