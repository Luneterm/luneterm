#!/usr/bin/env python3
"""macOS: print when the OS saw each left mouse press, for the tab-switch trace (plan step 3).

    python3 scripts/os-press-times.py

Polls `CGEventSourceSecondsSinceLastEventType` (HID system state, left mouse down) every 1 ms and
prints one `os_press: wall_ms=<ms modulo 1e9>` line per new press to stderr. With
`LUNETERM_TRACE_SWITCH=1` Luneterm prints `press: wall_ms=...` when the press reaches its event
loop; the difference is the time the press waited before the app saw it. It reads event times
only (no positions, no keys, no content) and needs no permission. Ctrl+C ends it.
"""

import ctypes
import sys
import time

HID_SYSTEM_STATE = 1
LEFT_MOUSE_DOWN = 1


def main():
    if sys.platform != "darwin":
        print("os-press-times.py: macOS only", file=sys.stderr)
        return 2
    cg = ctypes.CDLL(
        "/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices"
    )
    since = cg.CGEventSourceSecondsSinceLastEventType
    since.restype = ctypes.c_double
    since.argtypes = [ctypes.c_int32, ctypes.c_uint32]
    last = None
    try:
        while True:
            now = time.time()
            at = now - since(HID_SYSTEM_STATE, LEFT_MOUSE_DOWN)
            # The value moves by a few ms between reads; a new press moves it by far more.
            if last is not None and at - last > 0.05:
                print("os_press: wall_ms=%09d" % (int(at * 1000) % 1_000_000_000),
                      file=sys.stderr, flush=True)
            if last is None or at - last > 0.05:
                last = at
            time.sleep(0.001)
    except KeyboardInterrupt:
        return 0


if __name__ == "__main__":
    sys.exit(main())
