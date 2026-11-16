#!/usr/bin/env python3
"""Export Kitsunarr's SVG mark for the app, README and repository social preview.

Requires Python 3, rsvg-convert (librsvg) and Pillow. Artwork stays in SVG; raster
exports are rendered from that source rather than maintained separately.
"""
from pathlib import Path
import io
import subprocess
import xml.etree.ElementTree as ET

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
BRAND = ROOT / "branding"
ASSETS = ROOT / "crates/kitsunarr-web/assets"
MARK = BRAND / "kitsunarr-mark.svg"
SVG_NS = "http://www.w3.org/2000/svg"


def render(svg: bytes, width: int, height: int) -> bytes:
    return subprocess.run(
        ["rsvg-convert", "-w", str(width), "-h", str(height)],
        input=svg, stdout=subprocess.PIPE, check=True,
    ).stdout


def document(width: int, height: int, title: str, content: str) -> bytes:
    return (
        f'<svg xmlns="{SVG_NS}" viewBox="0 0 {width} {height}" '
        f'role="img" aria-label="{title}">\n{content}\n</svg>\n'
    ).encode()


def main() -> None:
    master = MARK.read_bytes()
    root = ET.fromstring(master)
    ET.register_namespace("", SVG_NS)
    art = "\n".join(
        ET.tostring(child, encoding="unicode").strip()
        for child in root
        if child.tag not in {f"{{{SVG_NS}}}title", f"{{{SVG_NS}}}desc"}
    )
    ASSETS.mkdir(parents=True, exist_ok=True)
    (ASSETS / "favicon.svg").write_bytes(master)
    for filename, size in [
        ("favicon-32.png", 32),
        ("apple-touch-icon.png", 180),
        ("icon-192.png", 192),
        ("icon-512.png", 512),
    ]:
        (ASSETS / filename).write_bytes(render(master, size, size))
    (BRAND / "kitsunarr-mark.png").write_bytes(render(master, 512, 512))
    # ICO encodes multiple independently rendered resolutions from the vector source.
    icons = [Image.open(io.BytesIO(render(master, size, size))) for size in (16, 32, 48, 64)]
    icons[-1].save(ASSETS / "favicon.ico", format="ICO", append_images=icons[:-1],
                   sizes=[(size, size) for size in (16, 32, 48, 64)], bitmap_format="png")
    for theme, ink in [("light", "#332333"), ("dark", "#fff4e4")]:
        wordmark = document(560, 128, "Kitsunarr", f'''<g>{art}</g>
<text x="148" y="89" fill="{ink}" font-family="DejaVu Sans, sans-serif" font-weight="700" font-size="68" letter-spacing="-2">Kitsunarr</text>''')
        (BRAND / f"kitsunarr-wordmark-{theme}.svg").write_bytes(wordmark)
    social = document(1280, 640, "Kitsunarr — TV, anime and movies", f'''<defs>
  <radialGradient id="glow"><stop stop-color="#f28c28" stop-opacity=".12"/><stop offset="1" stop-color="#f28c28" stop-opacity="0"/></radialGradient>
</defs>
<rect width="1280" height="640" fill="#10141c"/>
<circle cx="300" cy="305" r="320" fill="url(#glow)"/>
<rect x="24" y="24" width="1232" height="592" rx="30" fill="none" stroke="#30313c" stroke-width="2"/>
<g transform="translate(96 100) scale(3.25)">{art}</g>
<text x="548" y="285" fill="#fff4e4" font-family="DejaVu Sans, sans-serif" font-weight="700" font-size="80" letter-spacing="-3">Kitsunarr</text>
<text x="552" y="342" fill="#b7bfcd" font-family="DejaVu Sans, sans-serif" font-size="30">TV, anime &amp; movies.</text>
<text x="554" y="404" fill="#f5ad63" font-family="DejaVu Sans, sans-serif" font-size="16" letter-spacing="2">SELF-HOSTED · OPEN SOURCE</text>
<text x="64" y="580" fill="#9ba6b8" font-family="DejaVu Sans, sans-serif" font-size="18">github.com/KvasFan/Kitsunarr</text>''')
    (BRAND / "github-social-preview.svg").write_bytes(social)
    (BRAND / "github-social-preview.png").write_bytes(render(social, 1280, 640))
    print("Exported app icons, README wordmarks and GitHub social preview.")


if __name__ == "__main__":
    main()
