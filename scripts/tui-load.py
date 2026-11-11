#!/usr/bin/env python3
"""Synthetic terminal load: imitates a busy TUI (such as Claude Code) without doing any real work.

    python3 scripts/tui-load.py [--mode ink|tui|flood] [--fps N] [--seconds S] [--transcript N]

It only writes escape sequences and generated text to its own stdout: no network, no files, no
child processes. Used by the tab-switch measurements (plan step 3; `tests/switch_load.rs`, and the
real-window demo with `LUNETERM_TRACE_SWITCH=1`).

Modes:
  ink    main screen, like Ink-based CLIs: every frame is one synchronized update (DECSET ?2026)
         that erases the live region line by line (EL + CUU) and redraws it with SGR colours and
         a spinner; every `--full-every` frames it clears the screen and the scrollback
         (ED 2, ED 3) and reprints the whole transcript (`--transcript` lines), as Claude Code
         does when its output no longer fits. Every 6th frame it also sets the window title
         (OSC 0, a spinner) and an indeterminate progress report (OSC 9;4;3) unless --no-chrome.
  tui    alternate screen (?1049): every frame is one synchronized update that addresses every
         row (CUP), paints it with 256-colour SGR and ends it with EL.
  flood  `yes`-like lines as fast as the terminal takes them.
  idle   a prompt once, then nothing (a tab at rest; a shell prompt without the shell).

`--fps 0` writes frames back to back. Every frame shows a `t=<ms>` stamp, the wall clock in ms
modulo 1e9, so a reader of the screen can tell how old the shown frame is. The size follows
SIGWINCH. Ctrl+C (or the PTY closing) ends it.
"""

import argparse
import os
import signal
import sys
import time

SPIN = "|/-\\"
WORDS = (
    "reading", "parsing", "tokens", "context", "planning", "editing", "tests", "diff",
    "review", "build", "cargo", "render", "glyph", "atlas", "session", "engine",
)


def stamp():
    return "t=%09d" % (int(time.time() * 1000) % 1_000_000_000)


def size():
    try:
        s = os.get_terminal_size(sys.stdout.fileno())
        return max(s.columns, 10), max(s.lines, 3)
    except OSError:
        return 80, 24


def line_text(i, cols, salt):
    out = []
    n = 0
    k = i * 7 + salt
    while n < cols:
        w = WORDS[k % len(WORDS)]
        k = k * 1103515245 + 12345 & 0x7FFFFFFF
        out.append(w)
        n += len(w) + 1
    return " ".join(out)[:cols]


def sgr(i):
    return "\x1b[38;5;%dm" % (16 + i % 216)


class Out:
    def __init__(self):
        self.fd = sys.stdout.fileno()

    def write(self, s):
        data = s.encode()
        view = memoryview(data)
        while view:
            n = os.write(self.fd, view)
            view = view[n:]


def ink(out, a):
    region = 0  # lines of the live region drawn last frame
    transcript = 0  # lines printed above the live region so far
    f = 0
    while True:
        cols, rows = size()
        live = max(rows - 2, 1)
        parts = ["\x1b[?2026h"]
        if a.full_every and f % a.full_every == 0:
            parts.append("\x1b[2J\x1b[3J\x1b[H")
            for i in range(a.transcript):
                parts.append(sgr(i) + line_text(i, cols - 1, f) + "\x1b[0m\r\n")
            transcript = a.transcript
            region = 0
        else:
            parts.append("\x1b[2K\x1b[1A" * region + "\x1b[2K\x1b[G")
        head = "%s %s f=%d %s" % (SPIN[f % 4], stamp(), f, WORDS[f % len(WORDS)])
        parts.append("\x1b[1;35m" + head[: cols - 1] + "\x1b[0m\r\n")
        for i in range(live - 1):
            parts.append(sgr(i + f) + line_text(i + f, cols - 1, f) + "\x1b[0m")
            parts.append("\r\n" if i < live - 2 else "")
        region = live - 1
        parts.append("\x1b[?2026l")
        if a.chrome and f % 6 == 0:
            # A spinner in the window title, and an indeterminate OSC 9;4 progress report, as
            # Ink-based CLIs send while they work.
            parts.append("\x1b]0;%s working\x07\x1b]9;4;3;\x07" % SPIN[f // 6 % 4])
        out.write("".join(parts))
        f += 1
        yield


def tui(out, a):
    out.write("\x1b[?1049h\x1b[?25l")
    f = 0
    while True:
        cols, rows = size()
        parts = ["\x1b[?2026h\x1b[H"]
        head = "%s %s f=%d" % (SPIN[f % 4], stamp(), f)
        parts.append("\x1b[1;1H\x1b[1;33;44m" + head[:cols] + "\x1b[K\x1b[0m")
        for r in range(2, rows + 1):
            parts.append("\x1b[%d;1H" % r + sgr(r + f) + line_text(r + f, cols, f) + "\x1b[K")
        parts.append("\x1b[0m\x1b[?2026l")
        out.write("".join(parts))
        f += 1
        yield


def flood(out, a):
    f = 0
    while True:
        cols, _ = size()
        body = ("y " + stamp() + " ")
        lines = [(body * (cols // len(body) + 1))[: cols - 1] for _ in range(64)]
        out.write("\r\n".join(lines) + "\r\n")
        f += 1
        yield


def idle(out, a):
    out.write("idle tab " + stamp() + "\r\n$ ")
    while True:
        time.sleep(3600)
        yield


def main():
    p = argparse.ArgumentParser(description="synthetic TUI load (no real work)")
    p.add_argument("--mode", choices=("ink", "tui", "flood", "idle"), default="ink")
    p.add_argument("--fps", type=float, default=60.0, help="frames per second, 0 = unthrottled")
    p.add_argument("--seconds", type=float, default=0.0, help="stop after this long, 0 = never")
    p.add_argument("--transcript", type=int, default=2000, help="ink: transcript lines")
    p.add_argument("--full-every", type=int, default=30, help="ink: full reprint every N frames")
    p.add_argument("--no-chrome", dest="chrome", action="store_false",
                   help="ink: no title spinner and no OSC 9;4 progress")
    a = p.parse_args()
    signal.signal(signal.SIGWINCH, lambda *_: None)
    out = Out()
    gen = {"ink": ink, "tui": tui, "flood": flood, "idle": idle}[a.mode](out, a)
    period = 1.0 / a.fps if a.fps > 0 else 0.0
    start = time.monotonic()
    nxt = start
    try:
        while True:
            next(gen)
            now = time.monotonic()
            if a.seconds and now - start >= a.seconds:
                break
            if period:
                nxt += period
                if nxt > now:
                    time.sleep(nxt - now)
                else:
                    nxt = now
    except (BrokenPipeError, OSError, KeyboardInterrupt):
        return 0
    finally:
        if a.mode == "tui":
            try:
                out.write("\x1b[?25h\x1b[?1049l")
            except OSError:
                pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
