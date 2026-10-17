#!/usr/bin/env python3
"""Regenerate the Download Craft app mark.

Single source of truth for the artwork is `src/dm.rs::dm_mark`: an outlined
accent circle (r9, 2-unit stroke, #3b82f6) plus a download arrow, drawn in a
24x24 viewBox. This script renders that geometry once at 1024x1024 and
LANCZOS-downscales it, which gives clean AA at every icon size.

Writes:
  assets/icon.png                    256x256 RGBA (eframe window icon)
  assets/icon.ico                    multi-size; 256 as PNG, the rest 32bpp DIB
  browser-extension/icons/icon.png   same bytes (toolbar / chrome://extensions)

K is the glyph scale. The glyph spans 20 of the 24 units, so the content
fraction is 20*K/24 and the margin per side is (1 - 20*K/24)/2:

  K = 1.00 -> 83.3% content, reads small
  K = 1.05 -> 87.5% content, ~1px clear at 16px   <- used
  K = 1.15 -> 95.8% content, antialiasing clips the stroke at 16/24px

The mark is drawn *centred* on the canvas: an off-centre mark looks clipped on
one side even when its scale leaves a margin (that was the old bug: content
started 25px in and ran to the last pixel).

Usage:  python make_icon.py            (or `python -c "import make_icon"`)
"""

from pathlib import Path
import struct

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent
ACC = (0x3B, 0x82, 0xF6, 255)
SS = 1024  # master canvas, downscaled for every target size
K = 1.05
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def render(n=SS):
    """The `dm_mark` geometry on an n x n transparent canvas."""
    im = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    u = n / 24.0 * K  # pixels per viewBox unit
    c = n / 2.0
    w = max(1, round(2.0 * u))  # 2-unit stroke

    def at(x, y):
        return (c + (x - 12.0) * u, c + (y - 12.0) * u)

    # Circle: the stroke is centred on r=9, so the outer edge is r=10.
    r = 10.0 * u
    d.ellipse([c - r, c - r, c + r, c + r], outline=ACC, width=w)

    # Arrow: stem + chevron, round caps and joins.
    stem = [at(12.0, 8.0), at(12.0, 15.0)]
    chev = [at(8.5, 11.5), at(12.0, 15.0), at(15.5, 11.5)]
    d.line(stem, fill=ACC, width=w, joint="curve")
    d.line(chev, fill=ACC, width=w, joint="curve")
    cap = w / 2.0
    for (x, y) in (stem[0], chev[0], chev[2]):
        d.ellipse([x - cap, y - cap, x + cap, y + cap], fill=ACC)
    return im


MASTER = render()


def scaled(size):
    return MASTER.resize((size, size), Image.LANCZOS)


def png_bytes(img):
    import io

    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def dib(img):
    """32bpp BGRA bottom-up DIB + 1bpp AND mask (what Windows wants < 256)."""
    w, h = img.size
    px = img.load()
    body = bytearray()
    for y in range(h - 1, -1, -1):
        for x in range(w):
            r, g, b, a = px[x, y]
            body += bytes((b, g, r, a))
    mask = bytes(((w + 31) // 32 * 4) * h)
    hdr = struct.pack(
        "<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, len(body) + len(mask), 0, 0, 0, 0
    )
    return hdr + bytes(body) + mask


def write_ico(path, sizes):
    entries = []
    for s in sizes:
        img = scaled(s)
        entries.append((s, png_bytes(img) if s >= 256 else dib(img)))
    out = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    offset = 6 + 16 * len(entries)
    blobs = bytearray()
    for s, data in entries:
        out += struct.pack(
            "<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset
        )
        blobs += data
        offset += len(data)
    with open(path, "wb") as f:
        f.write(bytes(out) + bytes(blobs))


def main():
    # The 256px PNG is also the extension icon, so they stay byte-identical.
    png256 = png_bytes(scaled(256))
    for target in (ROOT / "assets" / "icon.png", ROOT / "browser-extension" / "icons" / "icon.png"):
        target.write_bytes(png256)
    write_ico(ROOT / "assets" / "icon.ico", ICO_SIZES)

    # Sanity check: the mark must be centred, with a margin at every size.
    for s in ICO_SIZES:
        img = scaled(s)
        bb = img.getchannel("A").point(lambda v: 255 if v > 8 else 0).getbbox()
        margins = (bb[0], bb[1], s - bb[2], s - bb[3])
        spread = max(margins) - min(margins)
        assert spread <= 1, f"{s}px: off-centre ({margins})"
        assert min(margins) >= 0, f"{s}px: clipped ({margins})"
        print(f"  {s:3}px margins {margins}")


if __name__ == "__main__":
    main()
