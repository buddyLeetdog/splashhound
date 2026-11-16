"""Generate Splashhound app icons from tools/dog_logo.jpg.

The artwork is a 640x640 scene; we crop a square around the dog and swim ring
(leaving the reflection as dark bottom padding), round the corners and export
every size Tauri needs plus a multi-size .ico.
"""

import os

from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "tools", "dog_logo.jpg")
ICONS = os.path.join(ROOT, "src-tauri", "icons")
PREVIEW = os.path.join(ROOT, "tools", "icon_preview.png")

# Square crop centred on the dog + ring, ignoring the empty top and keeping
# some of the water/reflection as bottom padding.
CROP = (0, 26, 600, 626)
# Slightly tighter framing for the tiny sizes, where every pixel counts.
SMALL_CROP = (38, 66, 558, 586)
CORNER = 0.22


def rounded(art, corner=CORNER):
    side = art.size[0]
    mask = Image.new("L", art.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, side - 1, side - 1], radius=int(side * corner), fill=255
    )
    out = Image.new("RGBA", art.size, (0, 0, 0, 0))
    out.paste(art, (0, 0), mask)
    return out


def main():
    os.makedirs(ICONS, exist_ok=True)
    source = Image.open(SRC).convert("RGB")
    master = rounded(source.crop(CROP))
    small = rounded(source.crop(SMALL_CROP))

    targets = {
        "icon.png": 512,
        "128x128@2x.png": 256,
        "128x128.png": 128,
        "64x64.png": 64,
        "48x48.png": 48,
        "32x32.png": 32,
        "24x24.png": 24,
        "16x16.png": 16,
    }
    for name, size in targets.items():
        src = small if size <= 32 else master
        src.resize((size, size), Image.LANCZOS).save(os.path.join(ICONS, name))

    master.save(
        os.path.join(ICONS, "icon.ico"),
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    # preview strip at the sizes Windows actually shows
    strip = Image.new("RGBA", (512 + 256 + 128 + 64 + 48 + 32 + 24 + 16 + 64, 528), (12, 16, 21, 255))
    x = 8
    for size in (512, 256, 128, 64, 48, 32, 24, 16):
        icon = master.resize((size, size), Image.LANCZOS)
        strip.alpha_composite(icon, (x, 520 - size))
        x += size + 8
    strip.save(PREVIEW)

    print("icons written to", ICONS)
    print("preview:", PREVIEW)


if __name__ == "__main__":
    main()


