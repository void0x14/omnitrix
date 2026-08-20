#!/usr/bin/env python3
"""TUI screenshot rig: run a TUI binary in a PTY, drive keys, render PNGs.

Usage:
  python tui_capture.py --bin <binary> --scenario <file.json> --out <dir> \
      [--cols 120] [--rows 32]

Scenario steps: {"sleep": seconds} | {"send": "bytes"} | {"resize": {"cols": n, "rows": n}} | {"snap": "name"}
"""
import argparse
import fcntl
import json
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time

import pyte
from PIL import Image, ImageDraw, ImageFont

FONT = "/usr/share/fonts/TTF/DejaVuSansMono.ttf"
FONT_BOLD = "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf"

DEFAULT_FG = (226, 226, 226)
DEFAULT_BG = (8, 8, 10)

NAMED = {
    "black": (0, 0, 0), "red": (205, 49, 49), "green": (13, 188, 121),
    "brown": (229, 229, 16), "yellow": (229, 229, 16), "blue": (36, 114, 200),
    "magenta": (188, 63, 188), "cyan": (17, 168, 205), "white": (229, 229, 229),
    "brightblack": (102, 102, 102), "brightred": (241, 76, 76),
    "brightgreen": (35, 209, 139), "brightyellow": (245, 245, 67),
    "brightblue": (59, 142, 234), "brightmagenta": (217, 87, 217),
    "brightcyan": (41, 184, 219), "brightwhite": (255, 255, 255),
    "gray": (102, 102, 102),
}


def color_rgb(value, default):
    if value is None or value == "default":
        return default
    if isinstance(value, str):
        if len(value) == 6:
            try:
                return (int(value[0:2], 16), int(value[2:4], 16), int(value[4:6], 16))
            except ValueError:
                pass
        return NAMED.get(value, default)
    return default


def render(screen, path, font, font_bold, cell_w, cell_h):
    cols, rows = screen.columns, screen.lines
    pad = 6
    width = int(cols * cell_w) + pad * 2
    height = rows * cell_h + pad * 2
    img = Image.new("RGB", (width, height), DEFAULT_BG)
    d = ImageDraw.Draw(img)
    for row in range(rows):
        line = screen.buffer[row]
        for col in range(cols):
            ch = line[col]
            fg = color_rgb(ch.fg, DEFAULT_FG)
            bg = color_rgb(ch.bg, DEFAULT_BG)
            if ch.reverse:
                fg, bg = bg, fg
            x0 = pad + int(col * cell_w)
            x1 = pad + int((col + 1) * cell_w)
            y0 = pad + row * cell_h
            y1 = y0 + cell_h
            if bg != DEFAULT_BG or ch.reverse:
                d.rectangle([x0, y0, x1 - 1, y1 - 1], fill=bg)
            if ch.data and ch.data != " ":
                f = font_bold if ch.bold else font
                if ch.bold and fg == DEFAULT_FG:
                    fg = (255, 255, 255)
                d.text((x0, y0), ch.data, font=f, fill=fg)
    img.save(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--scenario", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cols", type=int, default=120)
    ap.add_argument("--rows", type=int, default=32)
    ap.add_argument("--font-size", type=int, default=15)
    ap.add_argument("--cwd", default=None)
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    steps = json.load(open(args.scenario))

    font = ImageFont.truetype(FONT, args.font_size)
    font_bold = ImageFont.truetype(FONT_BOLD, args.font_size)
    cell_w = font.getlength("M")
    ascent, descent = font.getmetrics()
    cell_h = ascent + descent

    master, slave = pty.openpty()
    fcntl.ioctl(master, termios.TIOCSWINSZ,
                struct.pack("HHHH", args.rows, args.cols, 0, 0))
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"
    env["COLORTERM"] = "truecolor"
    env["LANG"] = env.get("LANG", "C.UTF-8")
    # Force the Unicode logo fallback: pyte renders Kitty payloads as garbage.
    env["OMNITRIX_NO_KITTY"] = "1"
    proc = subprocess.Popen([args.bin], stdin=slave, stdout=slave, stderr=slave,
                            env=env, cwd=args.cwd, close_fds=True)
    os.close(slave)

    screen = pyte.Screen(args.cols, args.rows)
    stream = pyte.ByteStream(screen)
    fl = fcntl.fcntl(master, fcntl.F_GETFL)
    fcntl.fcntl(master, fcntl.F_SETFL, fl | os.O_NONBLOCK)

    def resize_terminal(cols, rows):
        fcntl.ioctl(master, termios.TIOCSWINSZ,
                    struct.pack("HHHH", rows, cols, 0, 0))
        try:
            screen.resize(rows, cols)
        except AttributeError:
            pass


    def pump(duration):
        end = time.time() + duration
        while time.time() < end and proc.poll() is None:
            r, _, _ = select.select([master], [], [], 0.05)
            if master in r:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if data:
                    stream.feed(data)

    def drain(settle=0.25, timeout=2.0):
        end = time.time() + timeout
        quiet = time.time()
        while time.time() < end:
            r, _, _ = select.select([master], [], [], 0.05)
            got = False
            if master in r:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if data:
                    stream.feed(data)
                    got = True
            if got:
                quiet = time.time()
            elif time.time() - quiet > settle:
                break

    try:
        for step in steps:
            if proc.poll() is not None:
                print(f"[warn] process exited early (rc={proc.returncode})", file=sys.stderr)
                break
            if "sleep" in step:
                pump(step["sleep"])
            elif "send" in step:
                os.write(master, step["send"].encode("utf-8"))
                if "sleep" not in step:
                    drain()
            elif "resize" in step:
                dims = step["resize"]
                resize_terminal(int(dims["cols"]), int(dims["rows"]))
                drain()
            elif "snap" in step:
                drain()
                path = os.path.join(args.out, step["snap"] + ".png")
                render(screen, path, font, font_bold, cell_w, cell_h)
                print(f"saved {path}")
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=3)
        except Exception:
            proc.kill()
        os.close(master)


if __name__ == "__main__":
    main()
