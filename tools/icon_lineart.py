"""Line-art variants of the Splashhound icon.

Silhouettes are outlined from their alpha masks (blur + subtract), interiors
are left open, details are drawn as strokes: eye, nose, mouth, collar, panel
seams on the ring, waterline and splash.
"""

import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import icon_proposals as ic
from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "tools", "proposals")
SIZE = 512
SS = 4
S = SIZE * SS


def outline_mask(alpha, width):
    blurred = alpha.filter(ImageFilter.GaussianBlur(width))
    diff = ImageChops.subtract(alpha, blurred)
    return diff.point(lambda v: 255 if v > 40 else int(v * 255 / 40))


def stroke(draw, color, width):
    draw._splash_stroke = (color, width)


def dog_line_tile(px, color, width, detail_width, tint=None):
    """Outlined dog tile, details in the same stroke colour."""
    sil = ic.draw_dog(px, details=False)
    alpha = sil.split()[3]
    tile = Image.new("RGBA", (px, px), (0, 0, 0, 0))

    if tint:
        fill = Image.new("RGBA", (px, px), tint + (255,))
        fill.putalpha(alpha.point(lambda v: int(v * 0.16)))
        tile.alpha_composite(fill)

    edge = outline_mask(alpha, width)
    layer = Image.new("RGBA", (px, px), color + (255,))
    layer.putalpha(edge)
    tile.alpha_composite(layer)

    d = ImageDraw.Draw(tile)
    k = px / 1000.0

    def P(x, y):
        return (x * k, y * k)

    w = max(2, int(detail_width))

    # eye + brow
    ex, ey = P(804, 274)
    r = 17 * k
    d.ellipse([ex - r, ey - r, ex + r, ey + r], fill=color + (255,))
    # nose
    nx, ny = P(934, 340)
    nr = 25 * k
    d.ellipse([nx - nr, ny - nr, nx + nr, ny + nr], fill=color + (255,))
    # mouth
    d.arc([P(886, 344)[0], P(886, 344)[1], P(952, 392)[0], P(952, 392)[1]], 20, 115,
          fill=color + (235,), width=w)
    # collar + tag
    d.line([P(602, 352), P(768, 470)], fill=color + (255,), width=int(w * 1.5))
    tx, ty = P(742, 448)
    tr = 24 * k
    d.ellipse([tx - tr, ty - tr, tx + tr, ty + tr], outline=color + (255,), width=w)
    # inner ear
    d.arc([P(700, 168)[0], P(700, 168)[1], P(752, 232)[0], P(752, 232)[1]], 210, 20,
          fill=color + (220,), width=w)
    # paw separations
    d.line([P(860, 508), P(878, 540)], fill=color + (200,), width=w)
    d.line([P(846, 556), P(864, 590)], fill=color + (170,), width=w)
    return tile


def water_line(canvas, cx, rx, y, color, width):
    """Waterline only outside the ring so the opening stays clean."""
    d = ImageDraw.Draw(canvas)
    w = canvas.size[0]
    left_end = cx - rx * 1.02
    right_start = cx + rx * 1.02
    for x0, x1, flip in ((-w * 0.06, left_end, False), (right_start, w * 1.06, True)):
        seg = x1 - x0
        humps = max(2, int(seg / (w * 0.16)))
        step = seg / humps
        for i in range(humps):
            a = x0 + i * step
            d.arc([a, y - w * 0.022, a + step, y + w * 0.030], 200, 340,
                  fill=color + (200,), width=width)


def lineart(bg_stops, color, tint=None, glow=None):
    img = ic.gradient(S, bg_stops)
    if glow:
        layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
        ImageDraw.Draw(layer).ellipse([S * 0.16, S * 0.16, S * 0.84, S * 0.88], fill=glow)
        img.alpha_composite(layer.filter(ImageFilter.GaussianBlur(S * 0.06)))

    cx, cy = S * 0.5, S * 0.715
    rx, ry = S * 0.315, S * 0.315 * 0.62
    tube = rx * 0.36
    tube_y = tube * (ry / rx)
    lift = 0.32
    icy = cy - tube_y * lift
    outer = [cx - rx, cy - ry, cx + rx, cy + ry]
    inner = [cx - (rx - tube), icy - (ry - tube_y), cx + (rx - tube), icy + (ry - tube_y)]
    lw = max(3, int(S * 0.013))

    water_line(img, cx, rx, int(cy + ry * 0.92), color, max(2, int(S * 0.010)))

    # splash spray at the waterline, outside the ring
    d = ImageDraw.Draw(img)
    for sx in (-1, 1):
        x = cx + rx * 1.22 * sx
        y = cy + ry * 0.35
        d.arc([x - rx * 0.16, y - ry * 0.22, x + rx * 0.16, y + ry * 0.22],
              200 if sx < 0 else 20, 340 if sx < 0 else 160,
              fill=color + (215,), width=lw)
        for dx, dy, rr in ((0.30, -0.60, 0.011), (0.48, -0.92, 0.007), (0.16, -1.02, 0.005)):
            px = x + rx * dx * sx
            py = cy + ry * dy
            d.ellipse([px - S * rr, py - S * rr, px + S * rr, py + S * rr], fill=color + (225,))

    # ring: two bold contours, panel seams, near-side crest line
    d.ellipse(outer, outline=color + (255,), width=lw)
    d.ellipse(inner, outline=color + (255,), width=lw)
    seam = max(2, int(S * 0.011))
    for i in range(4):
        a = math.radians(90 * i + 20)
        ox, oy = cx + rx * math.cos(a), cy + ry * math.sin(a)
        ix, iy = cx + (rx - tube) * math.cos(a), icy + (ry - tube_y) * math.sin(a)
        d.line([(ox, oy), (ix, iy)], fill=color + (245,), width=seam)
    crest = [outer[0] + tube * 0.5, outer[1] + tube_y * 0.5,
             outer[2] - tube * 0.5, outer[3] - tube_y * 0.5]
    d.arc(crest, 18, 162, fill=color + (150,), width=max(2, int(S * 0.009)))

    # dog above the ring, outlined
    tile = dog_line_tile(1400, color, width=30, detail_width=int(7 * (S / 512)), tint=tint)
    ic.paste_dog(img, tile, (cx + rx * 0.05, cy + ry * -1.34), 0.84, angle=20)

    return ic.finish(img, ic.rounded_mask(S, int(S * 0.22)))


def main():
    os.makedirs(OUT, exist_ok=True)
    navy = [(0.0, (28, 50, 72)), (0.52, (14, 28, 45)), (1.0, (5, 12, 22))]
    paper = [(0.0, (250, 246, 236)), (1.0, (238, 230, 214))]
    charcoal = [(0.0, (20, 30, 30)), (1.0, (8, 14, 14))]

    variants = {
        "la_night": lineart(navy, (250, 244, 228), glow=(40, 150, 160, 45)),
        "la_amber": lineart(navy, (255, 199, 55), glow=(40, 150, 160, 40)),
        "la_paper": lineart(paper, (24, 42, 58), tint=(255, 199, 55)),
        "la_teal": lineart(charcoal, (120, 240, 210)),
    }
    for name, img in variants.items():
        path = os.path.join(OUT, f"{name}.png")
        img.save(path)
        print(path)

    # sheet with small sizes
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/segoeui.ttf", 22)
    except Exception:
        font = ImageFont.load_default()
    names = list(variants.keys())
    cell = 330
    sheet = Image.new("RGBA", (cell * len(names), 500), (12, 16, 21, 255))
    d = ImageDraw.Draw(sheet)
    for i, n in enumerate(names):
        img = variants[n]
        sheet.alpha_composite(img.resize((256, 256), Image.LANCZOS), (i * cell + 30, 18))
        sheet.alpha_composite(img.resize((64, 64), Image.LANCZOS), (i * cell + 30, 300))
        sheet.alpha_composite(img.resize((32, 32), Image.LANCZOS), (i * cell + 108, 316))
        sheet.alpha_composite(img.resize((16, 16), Image.LANCZOS), (i * cell + 152, 324))
        d.text((i * cell + 30, 400), n, fill=(220, 230, 240, 255), font=font)
    sheet.save(os.path.join(OUT, "lineart_sheet.png"))
    print("sheet")


if __name__ == "__main__":
    main()



