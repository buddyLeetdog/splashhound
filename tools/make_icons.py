"""Generate the Splashhound app icon (PNG sizes + Windows .ico).

The design comes from icon_proposals.propose_b ("night pool"). Render at
1024px and downsample for each target size.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import icon_proposals as ic
from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICONS = os.path.join(ROOT, "src-tauri", "icons")
PROPOSALS = os.path.join(ROOT, "tools", "proposals")


def main():
    os.makedirs(ICONS, exist_ok=True)
    os.makedirs(PROPOSALS, exist_ok=True)

    # Same 2048px supersampled canvas as the proposals, larger output.
    ic.SIZE = 1024
    ic.SS = 2
    ic.S = ic.SIZE * ic.SS

    master = ic.propose_b()
    master.save(os.path.join(PROPOSALS, "b_night_master.png"))

    targets = {
        "icon.png": 512,
        "128x128@2x.png": 256,
        "128x128.png": 128,
        "64x64.png": 64,
        "48x48.png": 48,
        "32x32.png": 32,
        "24x24.png": 24,
        "16x16.png": 16,
        "Square150x150Logo.png": 150,
        "Square44x44Logo.png": 44,
        "StoreLogo.png": 50,
    }
    for name, size in targets.items():
        master.resize((size, size), Image.LANCZOS).save(os.path.join(ICONS, name))

    master.save(
        os.path.join(ICONS, "icon.ico"),
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    # small-size sanity strip
    strip = Image.new("RGBA", (16 + 256 + 128 + 64 + 48 + 32 + 24 + 16 + 8 * 8, 128), (12, 16, 21, 255))
    x = 8
    for s in (16, 24, 32, 48, 64, 128, 256):
        strip.alpha_composite(master.resize((s, s), Image.LANCZOS), (x, 64 - s // 2))
        x += s + 8
    strip.save(os.path.join(PROPOSALS, "icon_sizes.png"))

    print("icons written to", ICONS)


if __name__ == "__main__":
    main()
