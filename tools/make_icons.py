"""Generate app icons (PNG + ICO) without external dependencies."""
import os
import struct
import zlib
import math


def png_bytes(width, height, rgba_rows):
    raw = b"".join(b"\x00" + row for row in rgba_rows)

    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def render(size):
    rows = []
    cx = cy = size / 2.0
    for y in range(size):
        row = bytearray()
        for x in range(size):
            # normalised coords in [-1, 1]
            nx = (x + 0.5 - cx) / (size / 2.0)
            ny = (y + 0.5 - cy) / (size / 2.0)
            r = math.sqrt(nx * nx + ny * ny)
            # rounded-square mask
            m = max(abs(nx), abs(ny))
            alpha = 0
            if m <= 1.0:
                # smooth rounded corners
                if m > 0.86 and r > 0.98:
                    alpha = int(255 * max(0.0, min(1.0, (1.04 - r) / 0.08)))
                else:
                    alpha = 255
            # background gradient
            t = (ny + 1) / 2
            br = int(18 + 22 * t)
            bg = int(22 + 40 * t)
            bb = int(30 + 60 * t)
            # play triangle
            tri = False
            if 0.12 < ny < 0.88 and -0.55 < nx < 0.62:
                left = -0.42 + 0.20 * (1 - (ny + 1) / 2) * 0
                width_at = (0.88 - abs(ny)) / 0.88
                if nx > -0.30 and nx < -0.30 + 0.92 * width_at:
                    tri = True
            # ring accent
            ring = 0.30 < r < 0.38
            if tri:
                cr, cg, cb = 235, 245, 240
            elif ring:
                cr, cg, cb = 66, 200, 160
            else:
                cr, cg, cb = br, bg, bb
            row += bytes((cr, cg, cb, alpha))
        rows.append(bytes(row))
    return png_bytes(size, size, rows)


def ico(png256):
    header = struct.pack("<HHH", 0, 1, 1)
    entry = struct.pack("<BBBBHHII", 0, 0, 0, 0, 1, 32, len(png256), 22)
    return header + entry + png256


def main():
    out = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src-tauri", "icons")
    os.makedirs(out, exist_ok=True)
    pngs = {}
    for size in (32, 128, 256, 512):
        pngs[size] = render(size)
    with open(os.path.join(out, "32x32.png"), "wb") as f:
        f.write(pngs[32])
    with open(os.path.join(out, "128x128.png"), "wb") as f:
        f.write(pngs[128])
    with open(os.path.join(out, "128x128@2x.png"), "wb") as f:
        f.write(pngs[256])
    with open(os.path.join(out, "icon.png"), "wb") as f:
        f.write(pngs[512])
    with open(os.path.join(out, "icon.ico"), "wb") as f:
        f.write(ico(pngs[256]))
    print("icons written to", out)


if __name__ == "__main__":
    main()
