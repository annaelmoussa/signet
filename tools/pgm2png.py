#!/usr/bin/env python3
"""Converts a PGM capture (P5, 8-bit) produced on the Kobo into a PNG.
No dependencies: zlib + struct from the standard library."""
import struct, sys, zlib

def read_pgm(path):
    with open(path, "rb") as f:
        data = f.read()
    # Header: "P5\n<w> <h>\n255\n" (possibly with comments)
    parts, pos = [], 0
    while len(parts) < 4:
        while data[pos:pos+1].isspace():
            pos += 1
        if data[pos:pos+1] == b"#":
            pos = data.index(b"\n", pos) + 1
            continue
        start = pos
        while not data[pos:pos+1].isspace():
            pos += 1
        parts.append(data[start:pos])
    pos += 1
    assert parts[0] == b"P5", "pas un PGM binaire"
    w, h = int(parts[1]), int(parts[2])
    return w, h, data[pos:pos + w * h]

def write_png(path, w, h, pixels):
    def chunk(tag, body):
        c = struct.pack(">I", len(body)) + tag + body
        return c + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + pixels[y*w:(y+1)*w] for y in range(h))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 0, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)

if __name__ == "__main__":
    src, dst = sys.argv[1], sys.argv[2]
    w, h, px = read_pgm(src)
    write_png(dst, w, h, px)
    print(f"{dst}: {w}x{h}")
