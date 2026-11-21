#!/usr/bin/env python3
"""Generate Pane's padded macOS icon and icon.icns from the existing logo."""

from pathlib import Path
import shutil
import subprocess
import tempfile

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[1]
ICONS = ROOT / "src-tauri" / "icons"
SOURCE = ICONS / "icon.png"
MASTER = ICONS / "icon-macos.png"
ICNS = ICONS / "icon.icns"
SIZE = 1024


def build_master() -> Image.Image:
    canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))

    tile_box = (92, 68, 932, 908)
    radius = 190

    shadow = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    shadow_draw = ImageDraw.Draw(shadow)
    shadow_draw.rounded_rectangle(
        (92, 88, 932, 928), radius=radius, fill=(0, 0, 0, 150)
    )
    shadow = shadow.filter(ImageFilter.GaussianBlur(28))
    canvas.alpha_composite(shadow)

    tile = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    tile_draw = ImageDraw.Draw(tile)
    tile_draw.rounded_rectangle(tile_box, radius=radius, fill=(8, 9, 14, 255))
    tile_draw.rounded_rectangle(
        (94, 70, 930, 906),
        radius=188,
        outline=(255, 255, 255, 22),
        width=3,
    )
    canvas.alpha_composite(tile)

    logo = Image.open(SOURCE).convert("RGBA").resize((660, 660), Image.Resampling.LANCZOS)
    canvas.alpha_composite(logo, (182, 158))
    return canvas


def generate_icns(master: Image.Image) -> None:
    with tempfile.TemporaryDirectory() as temp_dir:
        iconset = Path(temp_dir) / "Pane.iconset"
        iconset.mkdir()
        variants = {
            "icon_16x16.png": 16,
            "icon_16x16@2x.png": 32,
            "icon_32x32.png": 32,
            "icon_32x32@2x.png": 64,
            "icon_128x128.png": 128,
            "icon_128x128@2x.png": 256,
            "icon_256x256.png": 256,
            "icon_256x256@2x.png": 512,
            "icon_512x512.png": 512,
            "icon_512x512@2x.png": 1024,
        }
        for name, size in variants.items():
            master.resize((size, size), Image.Resampling.LANCZOS).save(iconset / name)
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(ICNS)], check=True)


def main() -> None:
    master = build_master()
    master.save(MASTER)
    generate_icns(master)
    print(f"Generated {MASTER.relative_to(ROOT)}")
    print(f"Generated {ICNS.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
