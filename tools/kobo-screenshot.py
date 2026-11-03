#!/usr/bin/env python3
"""Captures the Kobo's real framebuffer over SSH and writes it as a PNG.
Usage: kobo-screenshot.py <output.png>
Reads the depth and stride from sysfs: 8 bpp (Y8), 16 bpp (RGB565) or 32 bpp (BGRA)."""
import struct, subprocess, sys, zlib

W, H = 758, 1024

def ssh(cmd):
    return subprocess.run(["sh", "tools/kobo-ssh.sh", cmd], stdin=subprocess.DEVNULL, capture_output=True, check=True).stdout

def main():
    out = sys.argv[1]
    bpp, stride = [int(x) for x in ssh("cat /sys/class/graphics/fb0/bits_per_pixel /sys/class/graphics/fb0/stride").split()]
    raw = ssh(f"head -c {stride * H} /dev/fb0")
    if len(raw) < stride * H:
        sys.exit(f"framebuffer incomplet : {len(raw)} octets")
    rows = []
    for y in range(H):
        line = raw[y * stride: (y + 1) * stride]
        if bpp == 8:
            g = line[:W]
        elif bpp == 16:
            g = bytes(((int.from_bytes(line[2*x:2*x+2], "little") >> 5) & 0x3F) << 2 for x in range(W))
        else:
            g = line[1:W * 4:4]
        rows.append(b"\x00" + bytes(g))
    def chunk(tag, body):
        c = struct.pack(">I", len(body)) + tag + body
        return c + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 0, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
    with open(out, "wb") as f:
        f.write(png)
    print(f"{out}: {W}x{H} ({bpp} bpp, stride {stride})")

if __name__ == "__main__":
    main()
