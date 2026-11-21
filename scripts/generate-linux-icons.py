#!/usr/bin/env python3
"""Generate freedesktop-compatible Pane icons with transparent corners."""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
ICONS = ROOT / "src-tauri" / "icons"
SOURCE = ICONS / "icon.png"
MASTER = ICONS / "icon-linux.png"
CANVAS_SIZE = 1024
OUTPUTS = {
    ICONS / "32x32.png": 32,
    ICONS / "64x64.png": 64,
    ICONS / "128x128.png": 128,
    ICONS / "128x128@2x.png": 256,
    ICONS / "256x256.png": 256,
    MASTER: 512,
}


def build_master() -> Image.Image:
    canvas = Image.new("RGBA", (CANVAS_SIZE, CANVAS_SIZE), (0, 0, 0, 0))

    tile_box = (72, 72, 952, 952)
    tile_radius = 196
    draw = ImageDraw.Draw(canvas)
    draw.rounded_rectangle(tile_box, radius=tile_radius, fill=(8, 9, 14, 255))
    draw.rounded_rectangle(
        (74, 74, 950, 950),
        radius=194,
        outline=(255, 255, 255, 30),
        width=4,
    )

    source = Image.open(SOURCE).convert("RGB")
    # The source mark is cyan on an opaque black square. Its blue channel is
    # therefore a high-quality antialiased mask for removing that background.
    mark_mask = source.getchannel("B")
    mark = Image.new("RGBA", source.size, (0, 228, 255, 0))
    mark.putalpha(mark_mask)
    mark = mark.resize((680, 680), Image.Resampling.LANCZOS)
    canvas.alpha_composite(mark, (172, 172))

    return canvas


def main() -> None:
    master = build_master()
    for path, size in OUTPUTS.items():
        image = master.resize((size, size), Image.Resampling.LANCZOS)
        image.save(path, optimize=True)
        print(f"Generated {path.relative_to(ROOT)} ({size}x{size})")


if __name__ == "__main__":
    main()
