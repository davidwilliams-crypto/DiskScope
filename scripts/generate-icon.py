"""Generate the DiskScope application icon (packaging/icon.ico).

Renders the brand mark at high resolution and downsamples with Lanczos:
a magnifying glass inspecting a storage plate whose contents are a usage
treemap. Colors are the Meridian design system's dark theme, as in
src/theme.rs: an ink plate, the blue accent ring, and the success, warning
and danger block colors bright inside the lens and muted outside it.

Deterministic: no randomness, fixed hand-authored treemap layout.
Requires Pillow and numpy (pip install pillow numpy).

Outputs:
  packaging/icon.ico        sizes 256/128/64/48/32/24/16
  packaging/icon-master.png 1024px master PNG for docs and previews

Usage: python scripts/generate-icon.py [--out-dir packaging]
"""

from __future__ import annotations

import argparse
import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageChops, ImageDraw, ImageFilter

# Master render size; every shipped size is a Lanczos downscale of this.
MASTER = 2048

# Meridian tokens from src/theme.rs (Dark palette). The ring runs from
# `accent` in the light to a deeper step of the same hue in the shade, with
# `on-accent` as its rim. The plate runs from `surface-ink` to `surface`.
ACCENT = (125, 162, 255)
ACCENT_DEEP = (74, 118, 240)
ACCENT_EDGE = (6, 16, 43)
POSITIVE = (79, 209, 165)
WARNING = (242, 184, 75)
DANGER = (255, 138, 115)
PLATE_TOP = (28, 37, 64)
PLATE_BOTTOM = (16, 23, 42)
# Neutral blocks: steps between `tint` and `border-strong`.
BLOCK_1 = (34, 45, 78)
BLOCK_2 = (48, 62, 102)
BLOCK_3 = (58, 73, 118)
BLOCK_4 = (62, 78, 124)
BLOCK_5 = (72, 88, 134)

# Hand-authored squarified-treemap layout over the plate, in canvas
# fractions: (x, y, w, h, color). The gap between blocks is the plate
# showing through, like the application's treemap view.
TREEMAP = [
    (0.052, 0.052, 0.410, 0.530, BLOCK_2),
    (0.476, 0.052, 0.196, 0.306, POSITIVE),
    (0.686, 0.052, 0.262, 0.180, BLOCK_5),
    (0.686, 0.244, 0.262, 0.114, BLOCK_3),
    (0.476, 0.370, 0.196, 0.212, WARNING),
    (0.052, 0.594, 0.236, 0.354, BLOCK_1),
    (0.298, 0.594, 0.162, 0.354, BLOCK_4),
    (0.476, 0.594, 0.142, 0.198, POSITIVE),
    (0.476, 0.804, 0.142, 0.144, BLOCK_5),
    (0.632, 0.594, 0.316, 0.132, BLOCK_2),
    (0.632, 0.738, 0.148, 0.210, DANGER),
    (0.794, 0.738, 0.154, 0.210, BLOCK_3),
]

LENS_CENTER = (0.44, 0.475)
LENS_RADIUS = 0.300
RING_WIDTH = 0.056
HANDLE_ANGLE = 42.0  # degrees, down-right from the lens center
HANDLE_LENGTH = 0.30
HANDLE_WIDTH = 0.092

ICO_SIZES = [256, 128, 64, 48, 32, 24, 16]


def lerp_color(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def solid(size, rgb, alpha=255):
    return Image.new("RGBA", (size, size), rgb + (alpha,))


def rounded_mask(size, box, radius):
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=radius, fill=255)
    return mask


def circle_mask(size, center, radius):
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    cx, cy = center[0] * size, center[1] * size
    r = radius * size
    draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill=255)
    return mask


def inner_border(mask, thickness):
    """L mask covering a band of `thickness` just inside the mask edge."""
    eroded = mask.filter(ImageFilter.MinFilter(2 * thickness + 1))
    return ImageChops.subtract(mask, eroded)


def radial_falloff(size, center, radius, start=0.45, floor=0.0):
    """L mask that is 255 within radius*start and falls to floor at radius."""
    yy, xx = np.mgrid[0:size, 0:size]
    d = np.hypot(xx - center[0] * size, yy - center[1] * size)
    inner = max(radius * start, 1e-6) * size
    outer = max(radius, inner + 1e-6) * size
    t = np.clip(1.0 - (d - inner) / (outer - inner), floor, 1.0)
    return Image.fromarray((t * 255).astype(np.uint8), "L")


def draw_plate(size):
    """Dark rounded-square storage plate with a vertical gradient."""
    yy = np.linspace(0.0, 1.0, size)[:, None, None]
    top = np.array(PLATE_TOP, dtype=np.float32)
    bottom = np.array(PLATE_BOTTOM, dtype=np.float32)
    rgb = top + (bottom - top) * yy
    gradient = Image.fromarray(
        np.repeat(rgb[:, :], size, axis=1).astype(np.uint8), "RGB"
    ).convert("RGBA")

    margin = round(size * 0.014)
    box = (margin, margin, size - margin - 1, size - margin - 1)
    radius = round(size * 0.185)
    mask = rounded_mask(size, box, radius)

    plate = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    plate.paste(gradient, (0, 0), mask)
    # Faint inner bezel light so the plate edge reads against dark tiles.
    plate.paste(solid(size, (255, 255, 255), 22), (0, 0), inner_border(mask, max(2, size // 256)))
    return plate, mask


def draw_treemap(size):
    layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    gap = max(1, round(size * 0.010))
    for fx, fy, fw, fh, color in TREEMAP:
        x0, y0 = round(fx * size), round(fy * size)
        x1, y1 = round((fx + fw) * size) - gap, round((fy + fh) * size) - gap
        draw.rectangle((x0, y0, x1, y1), fill=color + (255,))
    return layer


def draw_scope(size):
    """The magnifier (ring + handle) as one accent-colored RGBA layer."""
    disc = circle_mask(size, LENS_CENTER, LENS_RADIUS)
    hole = circle_mask(size, LENS_CENTER, LENS_RADIUS - RING_WIDTH)
    ring_mask = ImageChops.subtract(disc, hole)

    # Handle: rounded bar drawn horizontally, rotated down-right, placed so
    # its outer end tucks under the ring and it extends toward the corner.
    length = round(HANDLE_LENGTH * size)
    width = round(HANDLE_WIDTH * size)
    bar = Image.new("L", (length, width), 0)
    ImageDraw.Draw(bar).rounded_rectangle(
        (0, 0, length - 1, width - 1), radius=width // 2, fill=255
    )
    bar = bar.rotate(-HANDLE_ANGLE, expand=True, resample=Image.BICUBIC)

    angle = math.radians(HANDLE_ANGLE)
    cx, cy = LENS_CENTER[0] * size, LENS_CENTER[1] * size
    start_r = (LENS_RADIUS - RING_WIDTH * 0.4) * size
    mid = start_r + length / 2
    mx = cx + mid * math.cos(angle)
    my = cy + mid * math.sin(angle)
    handle_mask = Image.new("L", (size, size), 0)
    handle_mask.paste(bar, (round(mx - bar.width / 2), round(my - bar.height / 2)))

    shape_mask = ImageChops.lighter(ring_mask, handle_mask)

    # Accent fill with a soft radial two-tone: lighter toward upper-left.
    base = solid(size, ACCENT_DEEP)
    bright = solid(size, ACCENT)
    light_from = radial_falloff(
        size, (LENS_CENTER[0] - 0.18, LENS_CENTER[1] - 0.18), LENS_RADIUS * 2.4, start=0.2
    )
    fill = Image.composite(bright, base, light_from)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(fill, (0, 0), shape_mask)
    # Dark rim inside the shape edge for separation and depth.
    out.paste(solid(size, ACCENT_EDGE, 190), (0, 0), inner_border(shape_mask, max(3, size // 300)))
    return out, shape_mask


def render(size):
    plate, plate_mask = draw_plate(size)
    treemap = draw_treemap(size)
    lens_mask = circle_mask(size, LENS_CENTER, LENS_RADIUS - RING_WIDTH)

    # Mute the treemap outside the lens; lift it slightly inside, so the
    # glass reads as the place the disk is actually being inspected.
    tm = np.asarray(treemap).astype(np.float32)
    lens = np.asarray(lens_mask).astype(np.float32) / 255.0
    rgb = tm[..., :3]
    gray = rgb.mean(axis=-1, keepdims=True)
    muted = (rgb * 0.62 + gray * 0.38) * 0.72 + np.array(PLATE_BOTTOM, dtype=np.float32) * 0.18
    inside = np.clip(rgb * 1.10, 0, 255)
    tm[..., :3] = muted * (1.0 - lens[..., None]) + inside * lens[..., None]
    treemap = Image.fromarray(tm.astype(np.uint8))
    # Keep the blocks inside the plate: without this their square corners
    # show past its rounded ones.
    treemap.putalpha(ImageChops.multiply(treemap.split()[3], plate_mask))

    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))

    # Plate drop shadow onto the transparent margin.
    soft_plate = plate_mask.filter(ImageFilter.GaussianBlur(size * 0.012))
    shadow = Image.composite(
        solid(size, (0, 0, 0), 110), Image.new("RGBA", (size, size), (0, 0, 0, 0)), soft_plate
    )
    canvas.alpha_composite(shadow, (round(size * 0.004), round(size * 0.010)))

    canvas.alpha_composite(Image.composite(treemap, plate, treemap.split()[3]))

    scope, shape_mask = draw_scope(size)
    # Scope shadow cast on the plate, then the scope itself.
    soft_shape = shape_mask.filter(ImageFilter.GaussianBlur(size * 0.008))
    scope_shadow = Image.composite(
        solid(size, (0, 0, 0), 80), Image.new("RGBA", (size, size), (0, 0, 0, 0)), soft_shape
    )
    canvas.alpha_composite(scope_shadow, (round(size * 0.006), round(size * 0.009)))
    canvas.alpha_composite(scope)

    # Glass sheen: soft white radial highlight in the lens upper-left.
    sheen = radial_falloff(
        size, (LENS_CENTER[0] - 0.10, LENS_CENTER[1] - 0.11), (LENS_RADIUS - RING_WIDTH) * 0.95
    ).point(lambda v: round(v * 0.28))
    canvas.paste(solid(size, (255, 255, 255)), (0, 0), ImageChops.multiply(sheen, lens_mask))
    return canvas


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out-dir", default="packaging", help="output directory (default: packaging)")
    args = parser.parse_args()
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    master = render(MASTER)
    master.resize((1024, 1024), Image.LANCZOS).save(out_dir / "icon-master.png")

    frames = [master.resize((s, s), Image.LANCZOS) for s in ICO_SIZES]
    frames[0].save(
        out_dir / "icon.ico",
        format="ICO",
        append_images=frames[1:],
        sizes=[(s, s) for s in ICO_SIZES],
    )
    print(f"Wrote {out_dir / 'icon.ico'} ({', '.join(str(s) for s in ICO_SIZES)})")
    print(f"Wrote {out_dir / 'icon-master.png'} (1024px master)")


if __name__ == "__main__":
    main()
