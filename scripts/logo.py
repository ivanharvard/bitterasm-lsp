#!/usr/bin/env python3
"""Generate the BitterASM logo and icon.

Writes to assets/ (or --out):

    icon.svg, icon.png                 the hexagon, centered, transparent
    icon-white.svg, icon-white.png     the same on a white background
    logo.svg, logo.png                 "Bitter" + hexagon holding "ASM", transparent
    logo-white.svg, logo-white.png     the same on a white background
    logo-dark.svg, logo-dark.png       transparent, with a light "Bitter" for dark backgrounds

and, when writing to assets/, refreshes the VS Code extension and file icons
from icon.svg.

"Bitter" is set in Bitter and "ASM" in JetBrains Mono, both bundled in
assets/fonts, and converted to outlines, so the SVGs render the same
everywhere without the fonts installed.

Requires resvg and usvg (https://github.com/linebender/resvg) and fontconfig.
"""

import argparse
import colorsys
import math
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The hexagon is a piece of a chocolate bar: a face set in the dark groove of
# the mold. The "piece" style adds thin lit and shaded edges between them;
# "flat" only the lit edges, on the top left; "plain" is just the face. The
# other tones are the face's color shifted in lightness.
STYLES = ("flat", "piece", "plain")
CHOCOLATE = "#452b17"  # the piece's face
GROOVE = -0.07         # the mold around it
HIGHLIGHT = 0.06       # edges facing the light, top left
SHADOW = -0.03         # edges facing away
TEXT_DARK = "#000000"
TEXT_LIGHT = "#ffffff"
FONTS = ROOT / "assets/fonts"

# Proportions from the original logo, relative to the hexagon's height.
CAP_HEIGHT = 0.27      # cap height of "Bitter"
ASM_CAP_HEIGHT = 0.29  # cap height of "ASM"
WORD_GAP = 0.03        # between "Bitter" and the hexagon's left point
LOGO_PAD = 0.12        # empty border around the full logo
ASM_NUDGE = -0.015     # optical shift of "ASM" in the hexagon: the A's open
                       # top-left and the M's solid right make true centering look right-heavy

GROOVE_WIDTH = 0.1    # relative to the hexagon's circumradius
EDGE_WIDTH = 0.035    # lit and shaded edges, relative to the circumradius

ICON_SIZE = 1024       # icon canvas, in SVG units and PNG pixels
ICON_PAD = 0.1         # empty border around the icon's hexagon, per side
LOGO_HEX_HEIGHT = 512  # hexagon height in the full logo, in SVG units
LOGO_PNG_SCALE = 2
MEASURE_SIZE = 100     # font size text is measured at; ink scales linearly


def run(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


class Font:
    def __init__(self, spec, weight):
        if Path(spec).is_file():
            self.path = spec
        else:
            self.path = run("fc-match", "-f", "%{file}", spec)
            want = spec.split(":")[0].lower()
            if want not in run("fc-scan", "-f", "%{family}", self.path).lower():
                sys.exit(f"logo.py: no font matching {spec!r} (fc-match gave {self.path}); "
                         "pass a font file instead")
        # A variable font lists one family per named instance; they all agree.
        self.family = run("fc-scan", "-f", "%{family[0]}\n", self.path).splitlines()[0]
        self.weight = weight

    def text(self, id, x, y, size, fill, s):
        return (f'<text id="{id}" x="{x:.2f}" y="{y:.2f}" font-family="{self.family}" '
                f'font-weight="{self.weight}" font-size="{size:.2f}" fill="{fill}">{s}</text>')


def lighten(color, amount):
    h, l, s = colorsys.rgb_to_hls(*(c / 255 for c in bytes.fromhex(color[1:])))
    rgb = colorsys.hls_to_rgb(h, min(max(l + amount, 0), 1), s)
    return "#" + bytes(round(c * 255) for c in rgb).hex()


def hexagon_points(cx, cy, r):
    """Flat-topped regular hexagon with circumradius r centered on (cx, cy)."""
    return [(cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a)))
            for a in range(0, 360, 60)]


def polygon(points, fill):
    d = "M" + " L".join(f"{x:.2f},{y:.2f}" for x, y in points) + " Z"
    return f'<path fill="{fill}" d="{d}"/>'


def hexagon(cx, cy, height, style):
    """The hexagon, height tall, in one of STYLES."""
    r = height / math.sqrt(3)
    if style == "plain":
        return polygon(hexagon_points(cx, cy, r), CHOCOLATE)
    edge = hexagon_points(cx, cy, r * (1 - GROOVE_WIDTH))
    face = hexagon_points(cx, cy, r * (1 - GROOVE_WIDTH - EDGE_WIDTH))
    shaded = style == "piece"
    shape = (polygon(hexagon_points(cx, cy, r), lighten(CHOCOLATE, GROOVE))
             + polygon(edge, lighten(CHOCOLATE, SHADOW) if shaded else CHOCOLATE))
    # The edges between vertices 2 and 5 face up and to the left (SVG y points down).
    shape += polygon(edge[2:6] + face[5:1:-1], lighten(CHOCOLATE, HIGHLIGHT))
    return shape + (polygon(face, CHOCOLATE) if shaded else "")


def svg(width, height, body, background):
    bg = f'<rect width="{width:.2f}" height="{height:.2f}" fill="{background}"/>' if background else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.2f}" height="{height:.2f}" '
            f'viewBox="0 0 {width:.2f} {height:.2f}">{bg}{body}</svg>\n')


class Renderer:
    def __init__(self, fonts, tmp):
        self.font_args = ["--skip-system-fonts"]
        for path in dict.fromkeys(f.path for f in fonts):
            self.font_args += ["--use-font-file", path]
        self.tmp = Path(tmp)

    def measure(self, *items):
        """Ink bounding box (x, y, w, h) of each (font, word) at MEASURE_SIZE, set at the origin."""
        # resvg reports boxes in canvas space, so push the text well inside it.
        off = MEASURE_SIZE * 10
        body = "".join(font.text(f"w{i}", off, off, MEASURE_SIZE, "#000", word)
                       for i, (font, word) in enumerate(items))
        src = self.tmp / "measure.svg"
        src.write_text(svg(off * 4, off * 4, body, None))
        # Queried as <text>, resvg reports the line box; outlined, the ink.
        outlined = self.tmp / "measure-outlined.svg"
        run("usvg", *self.font_args, str(src), str(outlined))
        boxes = {}
        for line in run("resvg", "--query-all", str(outlined)).splitlines():
            id, x, y, w, h = line.split(",")
            boxes[id] = (float(x) - off, float(y) - off, float(w), float(h))
        return [boxes[f"w{i}"] for i in range(len(items))]

    def write(self, out, name, image, png_scale):
        source, width, height = image
        src = self.tmp / f"{name}.svg"
        src.write_text(source)
        dst = out / f"{name}.svg"
        # usvg outlines the text so the result doesn't depend on installed fonts,
        # but drops the viewBox, without which the SVG won't scale.
        run("usvg", *self.font_args, "--coordinates-precision", "2", str(src), str(dst))
        dst.write_text(dst.read_text().replace(
            "<svg ", f'<svg viewBox="0 0 {width:.2f} {height:.2f}" ', 1))
        run("resvg", "-w", str(round(width * png_scale)), str(dst), str(out / f"{name}.png"))
        return dst


def icon(background, style):
    s = ICON_SIZE
    height = s * (1 - 2 * ICON_PAD) * math.sqrt(3) / 2
    return svg(s, s, hexagon(s / 2, s / 2, height, style), background), s, s


def logo(r, word_font, asm_font, background, style, word_fill):
    H = LOGO_HEX_HEIGHT
    hex_w = H * 2 / math.sqrt(3)
    pad = H * LOGO_PAD

    # Size each font by the height of its capitals, then measure the words.
    word_cap, asm_cap = (box[3] for box in r.measure((word_font, "H"), (asm_font, "H")))
    word_size = MEASURE_SIZE * H * CAP_HEIGHT / word_cap
    asm_size = MEASURE_SIZE * H * ASM_CAP_HEIGHT / asm_cap
    word_box, asm_box = r.measure((word_font, "Bitter"), (asm_font, "ASM"))
    bx, _, bw, _ = (v * word_size / MEASURE_SIZE for v in word_box)
    ax, ay, aw, ah = (v * asm_size / MEASURE_SIZE for v in asm_box)

    # Both words share a baseline that centers the caps of "ASM" in the hexagon.
    cy = pad + H / 2
    baseline = cy - (ay + ah / 2)
    hex_cx = pad + bw + H * WORD_GAP + hex_w / 2
    body = (word_font.text("bitter", pad - bx, baseline, word_size, word_fill, "Bitter")
            + hexagon(hex_cx, cy, H, style)
            + asm_font.text("asm", hex_cx + H * ASM_NUDGE - aw / 2 - ax, baseline,
                            asm_size, TEXT_LIGHT, "ASM"))
    width, height = hex_cx + hex_w / 2 + pad, H + 2 * pad
    return svg(width, height, body, background), width, height


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--word-font", default=str(FONTS / "Bitter.ttf"),
                   help='font file or fontconfig pattern for "Bitter" (default: Bitter)')
    p.add_argument("--word-weight", type=int, default=500)
    p.add_argument("--asm-font", default=str(FONTS / "JetBrainsMono.ttf"),
                   help='font file or fontconfig pattern for "ASM" (default: JetBrains Mono)')
    p.add_argument("--asm-weight", type=int, default=700)
    p.add_argument("--style", choices=STYLES, default=STYLES[0],
                   help=f"how the hexagon is drawn (default: {STYLES[0]})")
    p.add_argument("--out", type=Path, default=ROOT / "assets", help="output directory")
    args = p.parse_args()

    for tool in ("resvg", "usvg", "fc-match", "fc-scan"):
        if not shutil.which(tool):
            sys.exit(f"logo.py: {tool} not found on PATH")

    word_font = Font(args.word_font, args.word_weight)
    asm_font = Font(args.asm_font, args.asm_weight)
    args.out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        r = Renderer([word_font, asm_font], tmp)
        icon_svg = r.write(args.out, "icon", icon(None, args.style), 1)
        r.write(args.out, "icon-white", icon("#ffffff", args.style), 1)
        for name, bg, fill in (("logo", None, TEXT_DARK),
                               ("logo-white", "#ffffff", TEXT_DARK),
                               ("logo-dark", None, TEXT_LIGHT)):
            r.write(args.out, name, logo(r, word_font, asm_font, bg, args.style, fill),
                    LOGO_PNG_SCALE)

    if args.out.resolve() == (ROOT / "assets").resolve():
        vscode = ROOT / "editors/vscode"
        run("resvg", "-w", "256", str(icon_svg), str(vscode / "images/icon.png"))
        shutil.copyfile(icon_svg, vscode / "icons/basm-file-icon.svg")
    print(f"wrote {args.out} using {word_font.family} and {asm_font.family}")


if __name__ == "__main__":
    main()
