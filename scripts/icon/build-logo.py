"""Builds every Clipward logo SVG from one geometry: a clip (a wide rounded
block) cut on a slant, its two halves slid apart. One colour throughout.

  resources/clipward-icon.svg        app icon master (vector, 1024 x 1024)
  resources/clipward-mark.svg        the emblem alone, for dark backgrounds
  resources/clipward-logo.svg        emblem + wordmark, for dark backgrounds
  resources/clipward-logo-light.svg  emblem + wordmark, for light backgrounds

The wordmark is Geist SemiBold converted to outlines, so the SVGs render the
same everywhere without the font installed. Rasterize the app icons as
scripts/icon/README.md describes.

Needs: pip install fonttools brotli uharfbuzz
Usage: python3 scripts/icon/build-logo.py
"""
import io
from pathlib import Path

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

ROOT = Path(__file__).resolve().parents[2]
FONT = ROOT / 'resources/fonts/Geist-Variable.woff2'

# Emblem in a 1024 box: the clip, then the cut through it.
X0, X1, Y0, Y1 = 300, 724, 392, 632  # the clip's box
RADIUS = 56
# The cut: a slanted line through the clip, (x at y=0) to (x at y=1024).
CUT_TOP, CUT_BOTTOM = 560, 464
GAP = 18    # half the gap between the halves
SLIDE = 30  # how far each half slides: the left one down, the right one up

WEIGHT = 600
TRACKING = -18  # font units (1000 per em): slightly tight, for display size
CAP_RATIO = 0.5   # wordmark cap height relative to the lockup height
MARK_GAP = 0.22   # emblem to wordmark, relative to the lockup height


def emblem(ink: str, prefix: str) -> str:
    """The cut clip in a 1024 box, filled with `ink`."""
    clip = f'x="{X0}" y="{Y0}" width="{X1 - X0}" height="{Y1 - Y0}" rx="{RADIUS}" fill="{ink}"'
    return f'''<defs>
    <clipPath id="{prefix}-left"><polygon points="0,0 {CUT_TOP - GAP},0 {CUT_BOTTOM - GAP},1024 0,1024"/></clipPath>
    <clipPath id="{prefix}-right"><polygon points="{CUT_TOP + GAP},0 1024,0 1024,1024 {CUT_BOTTOM + GAP},1024"/></clipPath>
  </defs>
  <g transform="translate(0 {SLIDE})"><rect {clip} clip-path="url(#{prefix}-left)"/></g>
  <g transform="translate(0 -{SLIDE})"><rect {clip} clip-path="url(#{prefix}-right)"/></g>'''


def icon() -> str:
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024" role="img" aria-label="Clipward">
  <title>Clipward</title>
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#1B1C21"/><stop offset="1" stop-color="#0C0D10"/>
    </linearGradient>
    <linearGradient id="rim" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.14"/><stop offset="0.5" stop-color="#FFFFFF" stop-opacity="0"/>
    </linearGradient>
  </defs>
  <rect x="84" y="84" width="856" height="856" rx="196" fill="url(#tile)"/>
  <rect x="85.5" y="85.5" width="853" height="853" rx="194.5" fill="none" stroke="url(#rim)" stroke-width="3"/>
  {emblem('#F4F5F7', 'icon')}
</svg>
'''


def outline(text: str):
    font = instancer.instantiateVariableFont(TTFont(FONT), {'wght': WEIGHT})
    font.flavor = None
    buf = io.BytesIO()
    font.save(buf)

    hb_font = hb.Font(hb.Face(hb.Blob(buf.getvalue())))
    hb_buf = hb.Buffer()
    hb_buf.add_str(text)
    hb_buf.guess_segment_properties()
    hb.shape(hb_font, hb_buf, {'kern': True, 'liga': True})

    glyph_set = font.getGlyphSet()
    order = font.getGlyphOrder()
    path = ''
    x = 0.0
    count = len(hb_buf.glyph_infos)
    for i, (info, pos) in enumerate(zip(hb_buf.glyph_infos, hb_buf.glyph_positions)):
        pen = SVGPathPen(glyph_set, ntos=lambda v: f'{v:.1f}'.rstrip('0').rstrip('.'))
        glyph_set[order[info.codepoint]].draw(TransformPen(pen, (1, 0, 0, -1, x + pos.x_offset, -pos.y_offset)))
        path += pen.getCommands()
        x += pos.x_advance + (TRACKING if i < count - 1 else 0)
    return path, x, font['OS/2'].sCapHeight


def main():
    (ROOT / 'resources/clipward-icon.svg').write_text(icon())
    print('wrote resources/clipward-icon.svg')

    # The emblem's visible extent inside its 1024 box (the clip plus the slide either way).
    left, top, right, bottom = X0, Y0 - SLIDE, X1, Y1 + SLIDE
    box_w, box_h = right - left, bottom - top
    view = f'{left} {top} {box_w} {box_h}'
    (ROOT / 'resources/clipward-mark.svg').write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view}" role="img" aria-label="Clipward">\n'
        f'  <title>Clipward</title>\n  {emblem("#FAFAFA", "mark")}\n</svg>\n')
    print('wrote resources/clipward-mark.svg')

    text_d, text_width, cap_height = outline('Clipward')
    height = 100
    mark_w = height * box_w / box_h
    scale = height * CAP_RATIO / cap_height
    text_x = mark_w + height * MARK_GAP
    baseline = height / 2 + height * CAP_RATIO / 2
    width = text_x + text_width * scale
    for filename, color in {'clipward-logo.svg': '#FAFAFA', 'clipward-logo-light.svg': '#09090B'}.items():
        svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:.2f} {height}" role="img" aria-label="Clipward">
  <title>Clipward</title>
  <svg width="{mark_w:.2f}" height="{height}" viewBox="{view}">
  {emblem(color, 'lockup')}
  </svg>
  <path transform="translate({text_x:.2f} {baseline:.2f}) scale({scale:.6f})" fill="{color}" d="{text_d}"/>
</svg>
'''
        (ROOT / 'resources' / filename).write_text(svg)
        print(f'wrote resources/{filename} ({width:.1f} x {height})')


if __name__ == '__main__':
    main()
