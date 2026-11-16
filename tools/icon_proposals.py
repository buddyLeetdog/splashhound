"""Render icon proposals for Splashhound.

Drawing at 4x and downsampling keeps edges clean. The dog is built from
capsules (thick lines with round caps), which reads well at small sizes.
"""

import math
import os
from PIL import Image, ImageDraw, ImageFilter, ImageChops

OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "tools", "proposals")
SIZE = 512
SS = 4
S = SIZE * SS

AQUA_TOP = (126, 231, 233)
AQUA_MID = (54, 186, 210)
DEEP = (13, 92, 138)
NAVY = (9, 22, 36)
CORAL = (255, 106, 84)
CREAM = (255, 240, 218)
YELLOW = (255, 199, 55)
YELLOW_DARK = (238, 166, 30)
WHITE = (255, 255, 255)


# ---------------------------------------------------------------- helpers
def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def gradient(size, stops):
    """Vertical gradient. stops = [(pos, (r,g,b)), ...]"""
    img = Image.new("RGB", (size, size))
    d = ImageDraw.Draw(img)
    for y in range(size):
        t = y / max(1, size - 1)
        for i in range(len(stops) - 1):
            p0, c0 = stops[i]
            p1, c1 = stops[i + 1]
            if p0 <= t <= p1:
                local = 0 if p1 == p0 else (t - p0) / (p1 - p0)
                d.line([(0, y), (size, y)], fill=lerp(c0, c1, local))
                break
        else:
            d.line([(0, y), (size, y)], fill=stops[-1][1])
    return img.convert("RGBA")


def rounded_mask(size, radius):
    mask = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(mask)
    d.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return mask


def circle_mask(size, inset=0):
    mask = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(mask)
    d.ellipse([inset, inset, size - 1 - inset, size - 1 - inset], fill=255)
    return mask


def capsule(draw, p0, p1, width, fill):
    draw.line([p0, p1], fill=fill, width=int(width))
    r = width / 2
    for p in (p0, p1):
        draw.ellipse([p[0] - r, p[1] - r, p[0] + r, p[1] + r], fill=fill)


def quad(p0, p1, p2, t):
    x = (1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t * t * p2[0]
    y = (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t * t * p2[1]
    return x, y


DOG_PALETTE = {
    "body": (255, 199, 55),
    "shade": (230, 150, 18),
    "light": (255, 228, 138),
    "dark": (41, 28, 11),
    "collar": (255, 106, 84),
    "tag": (255, 214, 96),
}


def draw_dog(px=1400, palette=None, details=True):
    """A leaping dog, drawn in a px*px transparent tile.

    Flat silhouette shapes first, then two-tone fur shading, a cream belly,
    face details and a collar, all clipped to the silhouette.
    """
    pal = dict(DOG_PALETTE)
    if palette:
        pal.update(palette)
    body = pal["body"]
    img = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    k = px / 1000.0

    def P(x, y):
        return (x * k, y * k)

    def W(w):
        return w * k

    # tail (tapered curve) behind everything
    for i in range(28):
        t = i / 27
        x, y = quad(P(310, 585), P(200, 445), P(105, 330), t)
        r = W(58 - 40 * t)
        d.ellipse([x - r, y - r, x + r, y + r], fill=body)

    # hind legs trailing: thigh, then lower leg with a bend
    capsule(d, P(345, 635), P(268, 728), W(118), body)
    capsule(d, P(268, 728), P(238, 812), W(66), body)
    capsule(d, P(408, 660), P(336, 786), W(86), body)

    # body
    capsule(d, P(322, 620), P(648, 468), W(238), body)
    hx0, hy0 = P(330, 620)
    r0 = W(119)
    d.ellipse([hx0 - r0, hy0 - r0, hx0 + r0, hy0 + r0], fill=body)

    # neck rising out of the shoulders
    capsule(d, P(636, 472), P(736, 338), W(196), body)

    # head, snout, ears
    hx, hy = P(772, 292)
    hr = W(116)
    d.ellipse([hx - hr, hy - hr, hx + hr, hy + hr], fill=body)
    capsule(d, P(812, 306), P(918, 332), W(88), body)
    nx, ny = P(922, 334)
    nr = W(46)
    d.ellipse([nx - nr, ny - nr, nx + nr, ny + nr], fill=body)
    capsule(d, P(742, 226), P(682, 138), W(88), body)
    ex, ey = P(682, 138)
    er = W(44)
    d.ellipse([ex - er, ey - er, ex + er, ey + er], fill=body)

    # front legs reaching forward, one bent at the elbow
    capsule(d, P(690, 432), P(820, 470), W(82), body)
    capsule(d, P(820, 470), P(886, 520), W(66), body)
    px_, py_ = P(892, 528)
    pr_ = W(34)
    d.ellipse([px_ - pr_, py_ - pr_, px_ + pr_, py_ + pr_], fill=body)
    capsule(d, P(712, 492), P(840, 540), W(70), body)
    capsule(d, P(840, 540), P(898, 582), W(56), body)
    qx_, qy_ = P(904, 588)
    qr_ = W(28)
    d.ellipse([qx_ - qr_, qy_ - qr_, qx_ + qr_, qy_ + qr_], fill=body)

    if not details:
        return img

    alpha = img.split()[3]

    # shade the lower / trailing side of the fur
    up = ImageChops.offset(alpha, int(-20 * k), int(-24 * k))
    shade_mask = ImageChops.subtract(alpha, up).filter(ImageFilter.GaussianBlur(7 * k))
    shade = Image.new("RGBA", img.size, pal["shade"] + (255,))
    shade.putalpha(shade_mask)
    img = Image.alpha_composite(img, shade)

    # rim light on the upper / leading side
    down = ImageChops.offset(alpha, int(18 * k), int(22 * k))
    light_mask = ImageChops.subtract(alpha, down).filter(ImageFilter.GaussianBlur(6 * k))
    light = Image.new("RGBA", img.size, pal["light"] + (255,))
    light.putalpha(light_mask.point(lambda v: int(v * 0.75)))
    img = Image.alpha_composite(img, light)

    # cream chest / belly
    belly = Image.new("L", img.size, 0)
    ImageDraw.Draw(belly).ellipse(
        [P(400, 585)[0], P(400, 585)[1], P(640, 705)[0], P(640, 705)[1]], fill=255
    )
    belly = belly.filter(ImageFilter.GaussianBlur(26 * k))
    belly = ImageChops.multiply(belly, alpha).point(lambda v: int(v * 0.55))
    belly_layer = Image.new("RGBA", img.size, pal["light"] + (255,))
    belly_layer.putalpha(belly)
    img = Image.alpha_composite(img, belly_layer)

    d = ImageDraw.Draw(img)

    # inner-ear shadow
    d.ellipse([P(700, 176)[0], P(700, 176)[1], P(716, 226)[0], P(716, 226)[1]],
              fill=pal["shade"] + (200,))

    # collar across the neck + tag
    collar = pal["collar"]
    capsule(d, P(596, 344), P(772, 470), W(34), collar + (255,))
    capsule(d, P(596, 344), P(772, 470), W(34), collar + (255,))
    tx, ty = P(756, 456)
    tr = W(23)
    d.ellipse([tx - tr, ty - tr, tx + tr, ty + tr], fill=pal["tag"] + (255,))

    # muzzle shading, nose, eye, mouth
    d.ellipse([P(880, 330)[0], P(880, 330)[1], P(944, 358)[0], P(944, 358)[1]],
              fill=pal["light"] + (90,))
    d.ellipse([P(912, 322)[0], P(912, 322)[1], P(956, 360)[0], P(956, 360)[1]],
              fill=pal["dark"] + (255,))
    d.ellipse([P(920, 328)[0], P(920, 328)[1], P(933, 340)[0], P(933, 340)[1]],
              fill=(255, 255, 255, 140))
    d.ellipse([P(786, 256)[0], P(786, 256)[1], P(822, 292)[0], P(822, 292)[1]],
              fill=pal["dark"] + (255,))
    d.ellipse([P(795, 263)[0], P(795, 263)[1], P(807, 275)[0], P(807, 275)[1]],
              fill=(255, 255, 255, 235))
    d.arc([P(886, 344)[0], P(886, 344)[1], P(952, 392)[0], P(952, 392)[1]], 20, 120,
          fill=pal["dark"] + (210,), width=max(2, int(6 * k)))

    return img


def tint_dog(dog, color, shade):
    """Re-colour the dog silhouette and add a subtle underside shade."""
    alpha = dog.split()[3]
    base = Image.new("RGBA", dog.size, color + (0,))
    base.putalpha(alpha)
    return base


def paste_dog(canvas, dog, center, scale, angle=0, shadow=False, outline=None, outline_px=0):
    w = int(dog.width * scale)
    d = dog.resize((w, w), Image.LANCZOS)
    if angle:
        d = d.rotate(angle, resample=Image.BICUBIC, expand=True)
    if outline and outline_px > 0:
        # grow the silhouette to make a soft keyline that separates the dog
        # from the striped ring
        alpha = d.split()[3]
        grown = alpha.filter(ImageFilter.MaxFilter(outline_px * 2 + 1))
        rim = Image.new("RGBA", d.size, outline + (255,))
        rim.putalpha(grown)
        merged = Image.new("RGBA", d.size, (0, 0, 0, 0))
        merged.alpha_composite(rim)
        merged.alpha_composite(d)
        d = merged
    x = int(center[0] - d.width / 2)
    y = int(center[1] - d.height / 2)
    if shadow:
        sh = Image.new("RGBA", d.size, (0, 0, 0, 0))
        sh.paste((6, 40, 60, 90), (0, 0), d.split()[3])
        sh = sh.filter(ImageFilter.GaussianBlur(12))
        canvas.alpha_composite(sh, (x + 14, y + 18))
    canvas.alpha_composite(d, (x, y))


def composite_masked(canvas, layer, mask):
    """Alpha-composite `layer` onto `canvas` through `mask` (keeps canvas where
    the layer is transparent)."""
    masked = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    masked.paste(layer, (0, 0), mask)
    canvas.alpha_composite(masked)


def make_ring(canvas, center, rx, ry, tube, colors, angle_shift=0, lift=0.32):
    """Tilted torus: band mask, stripe layer, outer/inner ellipses.

    `lift` shifts the opening upward so the near (bottom) tube reads thicker
    than the far side, which is what makes it look inflated instead of flat.
    """
    cx, cy = center
    tube_y = tube * (ry / rx)
    outer = [cx - rx, cy - ry, cx + rx, cy + ry]
    icy = cy - tube_y * lift
    inner = [cx - (rx - tube), icy - (ry - tube_y), cx + (rx - tube), icy + (ry - tube_y)]

    band = Image.new("L", canvas.size, 0)
    bd = ImageDraw.Draw(band)
    bd.ellipse(outer, fill=255)
    bd.ellipse(inner, fill=0)

    stripes = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    sd = ImageDraw.Draw(stripes)
    n = max(2, len(colors) * 2)
    for i in range(n):
        sd.pieslice(
            outer,
            360 * i / n + angle_shift,
            360 * (i + 1) / n + angle_shift,
            fill=colors[i % len(colors)] + (255,),
        )
    return band, stripes, outer, inner, tube_y


def ring_volume(canvas, outer, inner, tube, tube_y, clip):
    """Round the tube: dark rims, broad crest light, tight specular, inner wall."""
    edges = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    ed = ImageDraw.Draw(edges)
    ed.ellipse(outer, outline=(3, 22, 34, 170), width=max(2, int(tube * 0.15)))
    ed.ellipse(inner, outline=(3, 22, 34, 150), width=max(2, int(tube * 0.13)))
    composite_masked(canvas, edges.filter(ImageFilter.GaussianBlur(tube * 0.09)), clip)

    crest = [
        outer[0] + tube * 0.5,
        outer[1] + tube_y * 0.5,
        outer[2] - tube * 0.5,
        outer[3] - tube_y * 0.5,
    ]
    broad = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    bd = ImageDraw.Draw(broad)
    bd.arc(crest, 145, 350, fill=(255, 255, 255, 110), width=max(2, int(tube * 0.34)))
    bd.arc(crest, 12, 150, fill=(120, 238, 230, 55), width=max(2, int(tube * 0.28)))
    composite_masked(canvas, broad.filter(ImageFilter.GaussianBlur(tube * 0.10)), clip)

    spec = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    ImageDraw.Draw(spec).arc(
        crest, 198, 284, fill=(255, 255, 255, 220), width=max(2, int(tube * 0.11))
    )
    composite_masked(canvas, spec.filter(ImageFilter.GaussianBlur(max(1, tube * 0.02))), clip)

    hole = Image.new("L", canvas.size, 0)
    ImageDraw.Draw(hole).ellipse(inner, fill=255)
    wall = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    ImageDraw.Draw(wall).arc(
        inner, 188, 352, fill=(2, 18, 30, 115), width=max(2, int(tube * 0.34))
    )
    composite_masked(canvas, wall.filter(ImageFilter.GaussianBlur(tube * 0.16)), hole)


def front_half(canvas, band, cy):
    half = Image.new("L", canvas.size, 0)
    ImageDraw.Draw(half).rectangle([0, cy, canvas.size[0], canvas.size[1]], fill=255)
    return ImageChops.multiply(band, half)


def water(canvas, horizon, top, bottom, waves=3):
    """Translucent water band with soft wave crests."""
    w, h = canvas.size
    band = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    bd = ImageDraw.Draw(band)
    for i in range(h - horizon):
        y = horizon + i
        t = i / max(1, h - horizon)
        bd.line([(0, y), (w, y)], fill=lerp(top, bottom, t) + (110,))
    for k in range(waves):
        y = horizon + 26 + k * 46
        if y > h:
            continue
        bd.arc([-w * 0.2, y - 40, w * 0.55, y + 40], 190, 350, fill=(255, 255, 255, 40), width=8)
        bd.arc([w * 0.45, y - 30, w * 1.2, y + 50], 190, 350, fill=(255, 255, 255, 34), width=8)
    canvas.alpha_composite(band)


def droplets(canvas, points, color=(255, 255, 255, 235)):
    d = ImageDraw.Draw(canvas)
    for x, y, r in points:
        d.ellipse([x - r, y - r, x + r, y + r], fill=color)


def splash_arcs(canvas, center, spread, color=(255, 255, 255, 210)):
    d = ImageDraw.Draw(canvas)
    cx, cy = center
    for i, (sx, sy) in enumerate([(-0.9, 0.25), (-0.55, 0.42), (0.55, 0.42), (0.9, 0.25)]):
        x = cx + sx * spread
        y = cy + sy * spread * 0.5
        d.arc([x - spread * 0.09, y - spread * 0.09, x + spread * 0.09, y + spread * 0.09],
              200 if sx < 0 else 20, 340 if sx < 0 else 160, fill=color, width=9)










# ---------------------------------------------------------------- designs
DOG_OUTLINE = (9, 62, 86)
SPLASH = (255, 255, 255, 235)


def water_shadow(canvas, cx, cy, rx, ry):
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    ImageDraw.Draw(layer).ellipse(
        [cx - rx * 1.05, cy + ry * 0.55, cx + rx * 1.05, cy + ry * 1.62],
        fill=(3, 28, 46, 105),
    )
    canvas.alpha_composite(layer.filter(ImageFilter.GaussianBlur(S * 0.02)))


def splash(canvas, cx, cy, rx, color=SPLASH, big=1.0):
    d = ImageDraw.Draw(canvas)
    for sx, sy, r in [
        (-1.10, 0.62, 0.019), (-1.22, 0.42, 0.010), (-1.00, 0.26, 0.007),
        (1.10, 0.62, 0.019), (1.24, 0.40, 0.010), (1.00, 0.24, 0.007),
    ]:
        x = cx + rx * sx
        y = cy + rx * sy
        rr = S * r * big
        d.ellipse([x - rr, y - rr, x + rr, y + rr], fill=color)
    for sx in (-1, 1):
        x = cx + rx * 0.94 * sx
        y = cy + rx * 0.58
        d.arc([x - rx * 0.18, y - rx * 0.22, x + rx * 0.18, y + rx * 0.22],
              200 if sx < 0 else 20, 340 if sx < 0 else 160,
              fill=color, width=max(2, int(S * 0.009)))


def ring_pass(img, center, rx, ry, tube, colors, angle_shift, dog_center, dog_scale, dog_angle,
              dog_outline=None, outline_px=0, dog_palette=None):
    band, stripes, outer, inner, tube_y = make_ring(img, center, rx, ry, tube, colors, angle_shift)
    composite_masked(img, stripes, band)
    ring_volume(img, outer, inner, tube, tube_y, band)

    dog = draw_dog(1400, palette=dog_palette)
    paste_dog(img, dog, dog_center, dog_scale, angle=dog_angle, shadow=True,
              outline=dog_outline, outline_px=outline_px)

    front = front_half(img, band, center[1])
    composite_masked(img, stripes, front)
    ring_volume(img, outer, inner, tube, tube_y, front)
    return outer


def pool_scene(img, ring_cy, rx_ratio, ry_ratio, colors, angle_shift,
               dog_scale, dog_angle, dog_dx, dog_dy, tube_ratio=0.37,
               shadow=True, splash_color=SPLASH, dog_outline=DOG_OUTLINE, outline_px=9,
               dog_palette=None):
    cx, cy = S * 0.5, S * ring_cy
    rx, ry = S * rx_ratio, S * rx_ratio * ry_ratio
    tube = rx * tube_ratio
    if shadow:
        water_shadow(img, cx, cy, rx, ry)
    ring_pass(img, (cx, cy), rx, ry, tube, colors, angle_shift,
              (cx + rx * dog_dx, cy + ry * dog_dy), dog_scale, dog_angle,
              dog_outline=dog_outline, outline_px=outline_px, dog_palette=dog_palette)
    splash(img, cx, cy, rx, color=splash_color)


def propose_a():
    img = gradient(S, [(0.0, AQUA_TOP), (0.40, AQUA_MID), (1.0, DEEP)])
    water(img, int(S * 0.76), (132, 238, 230), (10, 66, 106))
    pool_scene(img, 0.705, 0.328, 0.62, [CORAL, CREAM], 12, 0.86, 20, 0.05, -1.32)
    return finish(img, rounded_mask(S, int(S * 0.22)))


def propose_b():
    img = gradient(S, [(0.0, (28, 50, 72)), (0.52, (14, 28, 45)), (1.0, (5, 12, 22))])
    glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse([S * 0.14, S * 0.20, S * 0.86, S * 0.92], fill=(40, 150, 160, 55))
    img.alpha_composite(glow.filter(ImageFilter.GaussianBlur(S * 0.05)))
    water(img, int(S * 0.77), (46, 140, 158), (3, 12, 24))
    pool_scene(img, 0.705, 0.322, 0.62, [(255, 118, 92), (255, 234, 208)], 20, 0.84, 20, 0.05, -1.30,
               splash_color=(126, 234, 222, 220), dog_outline=(5, 22, 34))
    return finish(img, rounded_mask(S, int(S * 0.22)))


def propose_c():
    img = gradient(S, [(0.0, (255, 206, 174)), (0.26, (255, 160, 142)), (0.52, (150, 120, 170)),
                       (0.60, (36, 68, 100)), (1.0, (8, 28, 48))])
    sun = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(sun).ellipse([S * 0.10, S * 0.11, S * 0.48, S * 0.49], fill=(255, 242, 208, 225))
    img.alpha_composite(sun.filter(ImageFilter.GaussianBlur(S * 0.006)))
    sun_glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(sun_glow).ellipse([S * 0.04, S * 0.05, S * 0.54, S * 0.55], fill=(255, 226, 180, 70))
    img.alpha_composite(sun_glow.filter(ImageFilter.GaussianBlur(S * 0.03)))
    water(img, int(S * 0.66), (90, 120, 150), (8, 30, 50))
    pool_scene(img, 0.715, 0.322, 0.62, [(20, 150, 160), (248, 250, 242)], 22, 0.84, 20, 0.05, -1.30,
               shadow=True, splash_color=(255, 244, 232, 235), dog_outline=(32, 48, 78))
    g = ImageDraw.Draw(img)
    for k in range(4):
        y = S * (0.79 + k * 0.05)
        g.arc([S * (0.04 + 0.20 * k), y - 24, S * (0.48 + 0.18 * k), y + 24], 190, 350,
              fill=(255, 248, 240, 40), width=6)
    return finish(img, rounded_mask(S, int(S * 0.22)))


def propose_d():
    img = gradient(S, [(0.0, (172, 246, 240)), (0.50, (62, 196, 214)), (1.0, (14, 100, 148))])
    water(img, int(S * 0.755), (144, 242, 232), (12, 78, 120))
    pool_scene(img, 0.70, 0.302, 0.62, [WHITE, (240, 86, 68)], 0, 0.79, 18, 0.05, -1.28,
               dog_outline=(12, 66, 92))
    d = ImageDraw.Draw(img)
    d.ellipse([S * 0.014, S * 0.014, S * 0.986, S * 0.986], outline=(255, 255, 255, 150),
              width=int(S * 0.014))
    return finish(img, circle_mask(S, int(S * 0.014)))


def propose_e():
    img = Image.new("RGBA", (S, S), (255, 122, 99, 255))
    cx, cy = S * 0.5, S * 0.705
    rx, ry = S * 0.328, S * 0.328 * 0.62
    tube = rx * 0.37
    band, stripes, outer, inner, tube_y = make_ring(img, (cx, cy), rx, ry, tube, [CREAM, CREAM], 0)
    composite_masked(img, stripes, band)
    ring_volume(img, outer, inner, tube, tube_y, band)
    dog = draw_dog(1400, palette={"body": (16, 44, 62), "shade": (8, 30, 44),
                                  "light": (44, 82, 102), "dark": (255, 236, 210),
                                  "collar": (240, 86, 68), "tag": (255, 214, 96)})
    paste_dog(img, dog, (cx + rx * 0.05, cy - ry * 1.30), 0.86, angle=20)
    front = front_half(img, band, cy)
    composite_masked(img, stripes, front)
    ring_volume(img, outer, inner, tube, tube_y, front)
    d = ImageDraw.Draw(img)
    for sx, sy, r in [(-1.10, 0.62, 0.017), (1.12, 0.56, 0.014), (-0.98, 0.30, 0.008)]:
        x, y = cx + rx * sx, cy + rx * sy
        d.ellipse([x - S * r, y - S * r, x + S * r, y + S * r], fill=CREAM)
    return finish(img, rounded_mask(S, int(S * 0.22)))


def finish(img, mask):
    img = img.convert("RGBA")
    out = Image.new("RGBA", img.size, (0, 0, 0, 0))
    out.paste(img, (0, 0), mask)
    return out.resize((SIZE, SIZE), Image.LANCZOS)


def preview_sheet(paths):
    from PIL import ImageFont
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/segoeui.ttf", 22)
    except Exception:
        font = ImageFont.load_default()
    cell = 300
    sheet = Image.new("RGBA", (cell * len(paths), 480), (12, 16, 21, 255))
    d = ImageDraw.Draw(sheet)
    for i, (name, path) in enumerate(paths):
        img = Image.open(path).convert("RGBA")
        sheet.alpha_composite(img.resize((256, 256), Image.LANCZOS), (i * cell + 22, 18))
        sheet.alpha_composite(img.resize((64, 64), Image.LANCZOS), (i * cell + 22, 292))
        sheet.alpha_composite(img.resize((32, 32), Image.LANCZOS), (i * cell + 100, 308))
        d.text((i * cell + 22, 392), name, fill=(220, 230, 240, 255), font=font)
    return sheet


def main():
    os.makedirs(OUT, exist_ok=True)
    designs = {
        "a_pool": propose_a,
        "b_night": propose_b,
        "c_sunset": propose_c,
        "d_badge": propose_d,
        "e_flat": propose_e,
    }
    paths = []
    for name, fn in designs.items():
        img = fn()
        path = os.path.join(OUT, f"{name}.png")
        img.save(path)
        paths.append((name, path))
        print("wrote", path)
    preview_sheet(paths).save(os.path.join(OUT, "sheet.png"))
    print("wrote sheet")


if __name__ == "__main__":
    main()


