#!/usr/bin/env python3
"""Injects a touch press on the Kobo over SSH (type A multitouch protocol
of the Elan sensor). Usage: kobo-tap.py <x> <y> [x2 y2 ...]  (screen coordinates).
The raw coordinates are the inverse of Signet's calibration:
raw_x = screen_y, raw_y = 758 - screen_x (axes swapped, X inverted)."""
import struct, subprocess, sys, time

EV_SYN, EV_ABS = 0, 3
ABS_MT_TOUCH_MAJOR, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TRACKING_ID = 0x30, 0x35, 0x36, 0x39
SYN_REPORT, SYN_MT_REPORT = 0, 2

def ev(t, c, v):
    return struct.pack("<llHHi", 0, 0, t, c, v)

def frame(raw_x, raw_y, major):
    return (ev(EV_ABS, ABS_MT_TRACKING_ID, 7) + ev(EV_ABS, ABS_MT_POSITION_X, raw_x)
            + ev(EV_ABS, ABS_MT_POSITION_Y, raw_y) + ev(EV_ABS, ABS_MT_TOUCH_MAJOR, major)
            + ev(EV_SYN, SYN_MT_REPORT, 0) + ev(EV_SYN, SYN_REPORT, 0))

def tap(x, y):
    raw_x, raw_y = int(y), 758 - int(x)
    return frame(raw_x, raw_y, 1) * 2 + frame(raw_x, raw_y, 0)

def main():
    """Arguments: x y [x y …]. "long:x y" does a long press (0.9 s); "drag:x0,y0 x1,y1" a held press followed by a drag."""
    args = sys.argv[1:]
    if len(args) < 2 or len(args) % 2:
        sys.exit(__doc__)
    ssh = ["sh", "tools/kobo-ssh.sh", "cat > /dev/input/event1"]
    for i in range(0, len(args), 2):
        x, y = args[i], args[i + 1]
        if x.startswith("drag:"):
            # drag:x0,y0 x1,y1: press held 0.7 s then drag in 6 steps
            x0, y0 = [int(v) for v in x[5:].split(",")]
            x1, y1 = [int(v) for v in y.split(",")]
            frames = frame(int(y0), 758 - x0, 1) * 3
            steps = b""
            for k in range(1, 7):
                xx = x0 + (x1 - x0) * k // 6
                yy = y0 + (y1 - y0) * k // 6
                steps += frame(int(yy), 758 - xx, 1)
            # one frame = 6 events of 16 bytes; steps are spaced 0.15 s apart on the reader
            script = f"head -c {len(frames)} > /dev/input/event1; sleep 0.7; for i in 1 2 3 4 5 6; do head -c 96 > /dev/input/event1; sleep 0.15; done"
            subprocess.run(["sh", "tools/kobo-ssh.sh", script], input=frames + steps, check=True)
            subprocess.run(ssh, input=frame(int(y1), 758 - x1, 0), check=True)
            print(f"glissement ({x0},{y0}) → ({x1},{y1})")
            time.sleep(1.2)
            continue
        if x.startswith("long:"):
            x = x[5:]
            raw_x, raw_y = int(y), 758 - int(x)
            # hold: repeated "pressed" frames for 0.9 s, then release
            subprocess.run(["sh", "tools/kobo-ssh.sh", "cat > /dev/input/event1; sleep 0.9; printf '%s' > /dev/null"], input=frame(raw_x, raw_y, 1) * 3, check=True)
            subprocess.run(ssh, input=frame(raw_x, raw_y, 0), check=True)
            print(f"appui long ({x},{y})")
        else:
            subprocess.run(ssh, input=tap(x, y), check=True)
            print(f"tap ({x},{y})")
        time.sleep(1.2)

if __name__ == "__main__":
    main()
