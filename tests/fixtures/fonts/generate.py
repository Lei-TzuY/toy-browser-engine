"""Regenerate tiny public-domain geometric test fonts (requires fontTools).

The outlines below are authored rectangles/triangles, not system-font subsets.
These fixtures exercise missing-glyph fallback; they are not UI fonts.
"""
from pathlib import Path
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen


def glyph(contours):
    pen = TTGlyphPen(None)
    for contour in contours:
        pen.moveTo(contour[0])
        for point in contour[1:]:
            pen.lineTo(point)
        pen.closePath()
    return pen.glyph()


def build(name, fallback):
    order = [".notdef", "space", "A"] + (["cjk"] if fallback else [])
    builder = FontBuilder(1000, isTTF=True)
    builder.setupGlyphOrder(order)
    builder.setupCharacterMap({32: "space", 65: "A", **({0x4E2D: "cjk"} if fallback else {})})
    shapes = {
        ".notdef": glyph([[(50, 0), (450, 0), (450, 700), (50, 700)]]),
        "space": glyph([]),
        "A": glyph([[(50, 0), (550, 0), (300, 700)]]),
    }
    if fallback:
        shapes["cjk"] = glyph([[(100, 100), (800, 100), (800, 700), (100, 700)]])
    builder.setupGlyf(shapes)
    builder.setupHorizontalMetrics({g: (900 if fallback else 600, 0) for g in order})
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupNameTable({"familyName": name, "styleName": "Regular", "uniqueFontIdentifier": name,
                            "fullName": name, "psName": name, "copyright": "Public domain geometric test fixture"})
    builder.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
    builder.setupPost()
    builder.setupMaxp()
    builder.font["head"].created = builder.font["head"].modified = 0
    builder.font.recalcTimestamp = False
    builder.save(Path(__file__).with_name("fallback.ttf" if fallback else "primary.ttf"))


build("ToyPrimaryFixture", False)
build("ToyFallbackFixture", True)
