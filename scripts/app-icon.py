#!/usr/bin/env python3
"""生成 RRSS 应用图标「R。」的矢量源与应用内品牌标字形。

R 取自随包衬线字（apps/desktop/src/assets/fonts/rrss-serif-600.woff2，Noto Serif SC，OFL），
右下脚改成引向句号的一笔收尾，后接朱红句号。字形转成轮廓写进 SVG，不依赖字体。

  python3 scripts/app-icon.py            # 写 app-icon.svg、app-icon-small.svg 与 brandMarkGlyph.ts
  python3 scripts/app-icon.py merge DIR  # 把 DIR（小图源跑 tauri icon 的输出）里的 16–32 px 并进 icons/

依赖：fonttools、brotli（读 woff2）、Pillow（merge）。改图标后在仓库根目录依次运行：

  python3 scripts/app-icon.py
  apps/desktop/node_modules/.bin/tauri icon crates/rrss-desktop/app-icon.svg -o crates/rrss-desktop/icons
  apps/desktop/node_modules/.bin/tauri icon crates/rrss-desktop/app-icon-small.svg -o /tmp/rrss-icon-small
  python3 scripts/app-icon.py merge /tmp/rrss-icon-small
"""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FONT = ROOT / "apps/desktop/src/assets/fonts/rrss-serif-600.woff2"
DESKTOP = ROOT / "crates/rrss-desktop"
GLYPH_TS = ROOT / "apps/desktop/src/components/brandMarkGlyph.ts"

# Noto Serif SC 600 的 R 右下脚（横向衬线）；替换成向右上收的细尾，尾尖指向句号。
FOOT = "L636 14L579 41L724 32V0Q702 -5 680.0 -8.5Q658 -12 626 -12Z"
TAIL = "L626 44Q652 16 708 20Q752 24 786 62L796 54Q760 -12 694 -13Q656 -14 626 -12Z"
TAIL_TIP = 796

PAPER = {"top": "#f8f5ee", "bottom": "#ebe5d8", "edge": "#c9bfad", "ink": "#26231f", "dot": "#b8392a"}


def glyph() -> str:
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.ttLib import TTFont

    font = TTFont(FONT)
    glyphs = font.getGlyphSet()
    pen = SVGPathPen(glyphs)
    glyphs[font.getBestCmap()[ord("R")]].draw(pen)
    path = pen.getCommands()
    if path.count(FOOT) != 1:
        sys.exit("字体里的 R 和预期不同，FOOT 需要重新对一遍")
    return path.replace(FOOT, TAIL)


def layout(small: bool, span: float):
    """返回 (transform, 句号圆心 x, 半径, 描边加粗)。字体单位，y 向上；小尺寸句号更大、整体更满。"""
    r = 92 if small else 74
    dot_cx = TAIL_TIP + (30 if small else 46) + r
    left, right = 48, dot_cx + r
    s = span / (right - left)
    x0 = (1024 - span) / 2 - left * s
    # 视觉居中：大写字母的重心略高于几何中心。
    base = 512 + 735 * s / 2 - (6 if small else 10)
    return f"translate({x0:.1f} {base:.1f}) scale({s:.4f} {-s:.4f})", dot_cx, r, 34 if small else 0


def desktop_svg(d: str, small: bool) -> str:
    c = PAPER
    transform, cx, r, bold = layout(small, 760 if small else 600)
    inset, width = (12, 24) if small else (4, 8)
    stroke = f' stroke="{c["ink"]}" stroke-width="{bold}" stroke-linejoin="round"' if bold else ""
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
        '<defs><linearGradient id="rrss-paper" x1="0" y1="0" x2="0" y2="1">'
        f'<stop offset="0" stop-color="{c["top"]}"/><stop offset="1" stop-color="{c["bottom"]}"/></linearGradient></defs>'
        '<rect x="64" y="64" width="896" height="896" rx="200" fill="url(#rrss-paper)"/>'
        f'<rect x="{64 + inset}" y="{64 + inset}" width="{896 - 2 * inset}" height="{896 - 2 * inset}" rx="{200 - inset}" '
        f'fill="none" stroke="{c["edge"]}" stroke-opacity="{1 if small else 0.55}" stroke-width="{width}"/>'
        f'<g transform="{transform}"><path d="{d}" fill="{c["ink"]}"{stroke}/>'
        f'<circle cx="{cx}" cy="{r}" r="{r}" fill="{c["dot"]}"/></g></svg>\n'
    )


def glyph_ts(d: str) -> str:
    # 应用内品牌标只有 26–40 px：用小尺寸的加粗与句号比例，铺满墨色方块。
    transform, cx, r, bold = layout(True, 820)
    return (
        "// 由 scripts/app-icon.py 生成，勿手改。品牌标「R。」的字形，viewBox 0 0 1024 1024。\n"
        f'export const BRAND_GLYPH = {{ transform: "{transform}", d: "{d}", stroke: {bold}, dot: {{ cx: {cx}, cy: {r}, r: {r} }} }} as const;\n'
    )


def merge(src: Path) -> None:
    """小图源的 16 / 24 / 32 px 换进 icon.ico，32 与 30 px 的 PNG 也用小图源。"""
    from PIL import Image

    icons = DESKTOP / "icons"
    for name in ("32x32.png", "Square30x30Logo.png"):
        (icons / name).write_bytes((src / name).read_bytes())
    small = {im.size[0]: im for im in _ico_layers(src / "icon.ico") if im.size[0] <= 32}
    large = {im.size[0]: im for im in _ico_layers(icons / "icon.ico") if im.size[0] > 32}
    layers = {**large, **small}
    sizes = sorted(layers)
    biggest = layers[sizes[-1]]
    biggest.save(icons / "icon.ico", format="ICO", sizes=[(s, s) for s in sizes],
                 append_images=[layers[s] for s in sizes[:-1]])
    check = Image.open(icons / "icon.ico")
    print("icon.ico 尺寸：", sorted(check.info["sizes"]))


def _ico_layers(path: Path):
    from PIL import Image

    ico = Image.open(path)
    for size in sorted(ico.info["sizes"]):
        ico.size = size
        yield ico.convert("RGBA").copy()


def main() -> None:
    if len(sys.argv) == 3 and sys.argv[1] == "merge":
        merge(Path(sys.argv[2]))
        return
    d = glyph()
    (DESKTOP / "app-icon.svg").write_text(desktop_svg(d, small=False))
    (DESKTOP / "app-icon-small.svg").write_text(desktop_svg(d, small=True))
    GLYPH_TS.write_text(glyph_ts(d))
    print("已写入 app-icon.svg、app-icon-small.svg 与", GLYPH_TS.relative_to(ROOT))


if __name__ == "__main__":
    main()
