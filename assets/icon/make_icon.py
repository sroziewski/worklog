"""Draws the worklog icon: a clock on an indigo-to-teal tile, its hands at ten past ten so they
form a tick ("time logged"). Writes worklog.svg, worklog.png (256 px) and worklog.ico (16-256 px)
next to this script from the same geometry, so the three never drift apart.

    python assets/icon/make_icon.py        (needs Pillow)
"""
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
S = 1024  # design units; everything below is in them

TOP_LEFT = (76, 110, 245)       # indigo
BOTTOM_RIGHT = (21, 170, 191)   # teal
HANDS = (36, 52, 120)
ACCENT = (18, 184, 134)         # the tick's tip and the pivot
TILE_RADIUS = 228
CENTER = (512, 530)
FACE_R = 318
RING = 46                       # ring drawn in the tile colours, inside the face edge
TICK_LEN, TICK_W = 46, 30
HOUR = (-58.0, 150, 64)         # (angle from 12 o'clock in degrees, length, width)
MINUTE = (52.0, 255, 58)
PIVOT_R = 42


def hand_end(angle_deg, length):
    a = math.radians(angle_deg)
    return CENTER[0] + length * math.sin(a), CENTER[1] - length * math.cos(a)


def svg():
    hx, hy = hand_end(HOUR[0], HOUR[1])
    mx, my = hand_end(MINUTE[0], MINUTE[1])
    cx, cy = CENTER
    ticks = []
    for i in range(4):
        x1, y1 = cx + (FACE_R - RING - 18) * math.sin(i * math.pi / 2), cy - (FACE_R - RING - 18) * math.cos(i * math.pi / 2)
        x2, y2 = cx + (FACE_R - RING - 18 - TICK_LEN) * math.sin(i * math.pi / 2), cy - (FACE_R - RING - 18 - TICK_LEN) * math.cos(i * math.pi / 2)
        ticks.append(f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}"/>')
    tl = "#%02x%02x%02x" % TOP_LEFT
    br = "#%02x%02x%02x" % BOTTOM_RIGHT
    hands = "#%02x%02x%02x" % HANDS
    accent = "#%02x%02x%02x" % ACCENT
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}" width="{S}" height="{S}">
  <title>worklog</title>
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="{tl}"/>
      <stop offset="1" stop-color="{br}"/>
    </linearGradient>
    <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
      <feDropShadow dx="0" dy="18" stdDeviation="22" flood-color="#0b1a4a" flood-opacity="0.35"/>
    </filter>
  </defs>
  <rect width="{S}" height="{S}" rx="{TILE_RADIUS}" fill="url(#tile)"/>
  <circle cx="{cx}" cy="{cy}" r="{FACE_R}" fill="#ffffff" filter="url(#shadow)"/>
  <circle cx="{cx}" cy="{cy}" r="{FACE_R - RING / 2}" fill="none" stroke="url(#tile)" stroke-width="{RING}" opacity="0.28"/>
  <g stroke="{tl}" stroke-width="{TICK_W}" stroke-linecap="round" opacity="0.55">
    {"".join(ticks)}
  </g>
  <g stroke-linecap="round" fill="none">
    <line x1="{cx}" y1="{cy}" x2="{hx:.1f}" y2="{hy:.1f}" stroke="{hands}" stroke-width="{HOUR[2]}"/>
    <line x1="{cx}" y1="{cy}" x2="{mx:.1f}" y2="{my:.1f}" stroke="{hands}" stroke-width="{MINUTE[2]}"/>
  </g>
  <circle cx="{cx}" cy="{cy}" r="{PIVOT_R}" fill="{accent}"/>
</svg>
'''


def raster(size=1024, oversample=4):
    n = size * oversample
    k = n / S

    # Diagonal gradient: a horizontal ramp rotated 45 degrees, cropped to the tile.
    ramp = Image.linear_gradient("L").rotate(-90).resize((n * 2, n * 2)).rotate(-45, resample=Image.BICUBIC)
    off = (ramp.width - n) // 2
    mask = ramp.crop((off, off, off + n, off + n))
    # The rotated ramp is brightest at the top left, where the SVG gradient starts.
    tile = Image.composite(Image.new("RGB", (n, n), TOP_LEFT), Image.new("RGB", (n, n), BOTTOM_RIGHT), mask)

    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    round_mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(round_mask).rounded_rectangle((0, 0, n - 1, n - 1), radius=TILE_RADIUS * k, fill=255)
    img.paste(tile, (0, 0), round_mask)

    cx, cy = CENTER[0] * k, CENTER[1] * k
    r = FACE_R * k
    shadow = Image.new("L", (n, n), 0)
    ImageDraw.Draw(shadow).ellipse((cx - r, cy - r + 18 * k, cx + r, cy + r + 18 * k), fill=int(255 * 0.35))
    shadow = shadow.filter(ImageFilter.GaussianBlur(22 * k))
    img.paste(Image.new("RGBA", (n, n), (11, 26, 74, 255)), (0, 0), Image.composite(shadow, Image.new("L", (n, n), 0), round_mask))

    d = ImageDraw.Draw(img)
    d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=(255, 255, 255, 255))

    # The faint ring in the tile colours: the gradient seen through a translucent annulus.
    ring = Image.new("L", (n, n), 0)
    rd = ImageDraw.Draw(ring)
    rd.ellipse((cx - r, cy - r, cx + r, cy + r), fill=int(255 * 0.28))
    ri = (FACE_R - RING) * k
    rd.ellipse((cx - ri, cy - ri, cx + ri, cy + ri), fill=0)
    img.paste(tile.convert("RGBA"), (0, 0), ring)

    def stroke(p1, p2, width, colour):
        w = width * k
        d.line((p1, p2), fill=colour, width=int(w))
        for x, y in (p1, p2):
            d.ellipse((x - w / 2, y - w / 2, x + w / 2, y + w / 2), fill=colour)

    tick_colour = tuple(int(c * 0.55 + 255 * 0.45) for c in TOP_LEFT) + (255,)
    for i in range(4):
        a = i * math.pi / 2
        r1, r2 = (FACE_R - RING - 18) * k, (FACE_R - RING - 18 - TICK_LEN) * k
        stroke((cx + r1 * math.sin(a), cy - r1 * math.cos(a)), (cx + r2 * math.sin(a), cy - r2 * math.cos(a)), TICK_W, tick_colour)

    for angle, length, width in (HOUR, MINUTE):
        ex, ey = hand_end(angle, length)
        stroke((cx, cy), (ex * k, ey * k), width, HANDS + (255,))
    pr = PIVOT_R * k
    d.ellipse((cx - pr, cy - pr, cx + pr, cy + pr), fill=ACCENT + (255,))

    return img.resize((size, size), Image.LANCZOS)


def main():
    with open(HERE / "worklog.svg", "w", encoding="utf-8", newline="\n") as f:
        f.write(svg())
    big = raster(1024)
    big.resize((256, 256), Image.LANCZOS).save(HERE / "worklog.png", optimize=True)
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    big.save(HERE / "worklog.ico", sizes=[(s, s) for s in sizes])
    print("wrote", ", ".join(p.name for p in sorted(HERE.glob("worklog.*"))))


if __name__ == "__main__":
    main()
