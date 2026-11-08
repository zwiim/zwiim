#!/usr/bin/env python3
"""Render every Zwiim app icon from the vector mark (zwiim-mark.svg).

    python3 clients/brand/render-icons.py

Needs rsvg-convert (brew install librsvg). Writes the mobile assets in place
and the Desktop source icon, then runs `tauri icon` for the Desktop sizes.
The mark is drawn on a 298-unit square; each output places it on its own
canvas, so edit the mark here and re-run rather than touching the PNGs.
"""
import pathlib
import re
import subprocess
import tempfile

here = pathlib.Path(__file__).resolve().parent
repo = here.parent.parent
mark = (here / "zwiim-mark.svg").read_text()
body = mark.split(">", 1)[1].rsplit("</svg>", 1)[0]

TILE = "#0D0E10"  # DESIGN.md `code`
UNIT = 298


def svg(size, *layers):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" '
            f'viewBox="0 0 {size} {size}">{"".join(layers)}</svg>')


def placed(x, y, side, content=body):
    """The mark scaled so its 298-unit square spans `side` px at (x, y)."""
    return f'<g transform="translate({x} {y}) scale({side / UNIT})">{content}</g>'


def tile(x, y, side, radius):
    return f'<rect x="{x}" y="{y}" width="{side}" height="{side}" rx="{radius}" fill="{TILE}"/>'


def monochrome(content):
    # Android themed icons: one color, alpha only. The gap keeps the dot apart.
    return re.sub(r'fill="#[0-9A-Fa-f]{6}"|stroke="#[0-9A-Fa-f]{6}"',
                  lambda m: m.group(0).split("=")[0] + '="#FFFFFF"', content)


def render(svg_text, out, size):
    with tempfile.NamedTemporaryFile("w", suffix=".svg", delete=False) as f:
        f.write(svg_text)
    out.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size), f.name, "-o", str(out)], check=True)
    print(f"{out.relative_to(repo)} {size}px")


mobile = repo / "clients/mobile/assets"
# iOS / Expo app icon: full-bleed and opaque; the OS applies the mask.
render(svg(1024, tile(0, 0, 1024, 0), placed(0, 0, 1024)), mobile / "icon.png", 1024)
# Android adaptive foreground: 108dp canvas, the 66dp safe circle holds the mark.
# (Background color is app.json's adaptiveIcon.backgroundColor.)
fg = 1024 * 0.64
render(svg(1024, placed((1024 - fg) / 2, (1024 - fg) / 2, fg)), mobile / "android-icon-foreground.png", 1024)
render(svg(1024, placed((1024 - fg) / 2, (1024 - fg) / 2, fg, monochrome(body))),
       mobile / "android-icon-monochrome.png", 1024)
# Splash: the rounded app tile, on both the light and dark splash backgrounds.
splash = svg(512, tile(0, 0, 512, 512 * 0.2237), placed(0, 0, 512))
render(splash, mobile / "splash-icon.png", 512)
render(splash, mobile / "splash-icon-dark.png", 512)

# macOS app icon grid: an 824px rounded body centred on a 1024px canvas.
desktop_src = repo / "clients/desktop/src-tauri/icons/app-icon.png"
render(svg(1024, tile(100, 100, 824, 185.4), placed(100, 100, 824)), desktop_src, 1024)
subprocess.run(["npx", "--no-install", "tauri", "icon", str(desktop_src),
                "-o", str(desktop_src.parent)], cwd=repo / "clients/desktop", check=True)
# Desktop ships on macOS only (Apple Silicon V1): keep the macOS set that
# tauri.conf.json lists and drop the Windows/iOS/Android extras `tauri icon` adds.
keep = {"32x32.png", "128x128.png", "128x128@2x.png", "icon.icns", "icon.png", desktop_src.name}
for f in desktop_src.parent.iterdir():
    if f.name not in keep:
        subprocess.run(["rm", "-rf", str(f)], check=True)
