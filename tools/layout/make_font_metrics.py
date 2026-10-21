#!/usr/bin/env python3
"""Write crates/docrs/src/layout/font_metrics.bin.

Width and line-height tables for estimating where Word breaks lines and
pages: for every font face Word for Mac can use, its units per em, hhea
ascender/descender/line gap (Word for Mac's single line height is their sum
times the size) and the advance width of each code point in CODEPOINTS.
Numbers only: no font file, outline or other glyph data is written.

Word for Mac's single line height is hhea ascender + descender + line gap
for almost every font; tools/layout/line_heights.tsv holds the line height
Word was measured to use for each family (40 pt lines, see
tools/layout/README.md). Where it differs from the formula by more than
0.004 em, the line gap is adjusted so the table gives the measured value.

Word for Mac lays out a font name it cannot resolve in Cambria (or Calibri,
see fonts.rs); a family measured at exactly Cambria's line height is such
a name (an Apple style face Word does not offer by that name) and is left
out, so layout substitutes it as Word does. "Times" is Times New Roman to
Word and is left to the alias.

Faces are taken in Word for Mac's order of precedence (an installed system
font wins over Word's own copy, which wins over a downloaded cloud font),
keyed the way documents name fonts: the family name, with a non-standard
style appended ("Calibri Light", "Aptos SemiBold") and the four standard
styles as bold/italic flags.

    python3 tools/layout/make_font_metrics.py [font directories...]

Symbol fonts (Symbol, Wingdings, Webdings) are measured over their
private-use range U+F020-F0FF, where documents' bullet characters land.

Pair kerning (the classic 'kern' table, which every font Word ships has)
is kept for text faces between the code points of KERN_CODEPOINTS:
documents written by recent Word turn kerning on (`w:kern`), and it moves
line breaks.

Format (little-endian, zlib-compressed): b"DCFM", u16 version (3), u16
code point set count, per set a u16 count and the code points (u32 each),
the kerning code points (u8 count, u32 each),
u16 face count, then per face a u8 name length and UTF-8 family name, u8
flags (1 bold, 2 italic, 4 symbol), u8 code point set, u16 units per em,
i16 ascender, i16 descender (positive), i16 line gap, u16 average width,
one u16 advance per code point of its set (0xFFFF when missing), then a
u16 kerning pair count and per pair the u8 indices of the left and right
code points among the kerning code points and the i16 adjustment, sorted.
"""

import glob
import os
import re
import struct
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ttf  # noqa: E402

OUT = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "../../crates/docrs/src/layout/font_metrics.bin"
)

HOME = os.path.expanduser("~")
DIRECTORIES = [
    "/System/Library/Fonts/Supplemental",
    "/System/Library/Fonts",
    "/Library/Fonts",
    "/Applications/Microsoft Word.app/Contents/Resources/DFonts",
    HOME + "/Library/Group Containers/UBF8T346G9.Office/FontCache/4/CloudFonts",
]

CODEPOINTS = (
    list(range(0x20, 0x7F))
    + list(range(0xA0, 0x180))
    + list(range(0x2010, 0x2028))
    + list(range(0x2030, 0x203B))
    + [0x20AC, 0x2122, 0x2190, 0x2191, 0x2192, 0x2193, 0x2212, 0x25A0, 0x25AA, 0x25CF, 0x2610, 0x2612, 0x2713]
)

SYMBOL_CODEPOINTS = list(range(0xF020, 0xF100))

# Kerning is kept between these (printable ASCII and the typographic
# quotes and dashes): nearly all of running text, at a tenth of the size.
KERN_CODEPOINTS = list(range(0x20, 0x7F)) + [0xA0, 0x2013, 0x2014, 0x2018, 0x2019, 0x201C, 0x201D, 0x2026]
SYMBOL_FAMILIES = ("Symbol", "Wingdings", "Wingdings 2", "Wingdings 3", "Webdings")

# System fonts that are user-interface, emoji or non-Latin script fonts;
# their Latin glyphs are not what a document using them is about.
SKIP_PREFIXES = (".", "Apple Color Emoji", "LastResort", "Noto Sans", "Noto Serif", "STIX", "Zapf Dingbats", "Bodoni Ornaments", "Segoe UI Emoji", "Marlett", "MT Extra", "Bookshelf Symbol", "MS Reference Specialty", "Monotype Sorts")


# CJK, Indic, Thai and other script fonts: documents name them for that
# script, not for Latin text.
SKIP_FAMILIES = re.compile(
    r"^(Hiragino|Songti|Heiti|ST[A-Z]|PingFang|MS (P?Gothic|P?Mincho|UI Gothic)|Yu (Gothic|Mincho)|Meiryo"
    r"|Malgun|Gulim|Dotum|Batang|Gungsuh|P?MingLiU|N?SimSun|SimHei|KaiTi|FangSong|DengXian|Microsoft "
    r"(YaHei|JhengHei|Himalaya|Yi Baiti|Tai Le|New Tai Lue)|HG|Apple SD Gothic|Apple Symbols|Kohinoor|Mukta"
    r"|Myanmar|Sukhumvit|Thonburi|Ayuthaya|Krungthep|Silom|Sathu|Angsana|Cordia|TH Sarabun|Mangal|Latha"
    r"|Gautami|Kartika|Tunga|Nyala|Mongolian Baiti|Plantagenet|Euphemia|Kefa|Shree Devanagari|Dubai|David"
    r"|Segoe UI (Historic|Symbol)|System Font|InaiMathi|Lao |Khmer|Arial Unicode|Cambria Math)"
    r"|(Sangam)? MN$"
)


def style_of(subfamily):
    """(extra style words, bold, italic) of a subfamily name."""
    words = subfamily.replace("Oblique", "Italic").split()
    italic = "Italic" in words
    words = [w for w in words if w != "Italic"]
    bold = words == ["Bold"]
    if words in (["Regular"], ["Roman"], ["Plain"], ["Book"], ["Normal"], ["Bold"], []):
        words = []
    return " ".join(words), bold, italic


def faces():
    """Every readable face, by (name, bold, italic): its copies in
    precedence order."""
    found = {}
    for source, directory in enumerate(DIRECTORIES):
        paths = sorted(glob.glob(os.path.join(directory, "**", "*"), recursive=True))
        for path in paths:
            if not path.lower().endswith((".ttf", ".ttc", ".otf")):
                continue
            try:
                data = open(path, "rb").read()
                count = ttf.face_count(data)
            except OSError:
                continue
            for index in range(count):
                try:
                    face = ttf.Face(data, index)
                except Exception:  # noqa: BLE001 - unreadable faces are skipped
                    continue
                family, subfamily = face.name(1), face.name(2)
                if not family or not subfamily or family.startswith(SKIP_PREFIXES) or SKIP_FAMILIES.search(family):
                    continue
                symbol = family in SYMBOL_FAMILIES
                if not symbol and not all(face.advance(c) for c in range(0x41, 0x5B)):
                    continue
                extra, bold, italic = style_of(subfamily)
                name = f"{family} {extra}" if extra and not family.endswith(extra) else family
                copies = found.setdefault((name.lower(), bold, italic), [])
                if not any(c[5] == source for c in copies):
                    copies.append((name, bold, italic, face, symbol, source))
    return found


def kerning_pairs(face, codepoints):
    """Kerning between code points of the set, as (left index, right
    index, value), sorted."""
    try:
        kern = face.kerning()
    except Exception:  # noqa: BLE001 - a malformed table kerns nothing
        kern = {}
    if not kern:
        return []
    by_glyph = {}
    for index, c in enumerate(codepoints):
        glyph = face.cmap.get(c)
        if glyph:
            by_glyph.setdefault(glyph, []).append(index)
    pairs = []
    for (left, right), value in kern.items():
        if value == 0:
            continue
        for i in by_glyph.get(left, ()):
            for j in by_glyph.get(right, ()):
                pairs.append((i, j, value))
    return sorted(pairs)


def formula(face):
    return (face.hhea_ascender - face.hhea_descender + face.hhea_line_gap) / face.units_per_em


def choose(found, measured):
    """One copy per face. Word for Mac takes the system copy before its
    own, except where Word's measured line height shows it uses another
    copy (Symbol: its bundled copy; the system one is Apple's): the first
    source whose regular face matches the measurement wins."""
    chosen = []
    for key in sorted(found):
        family = key[0]
        copies = found[key]
        target = measured.get(family)
        pick = copies[0]
        if target is not None:
            regular = found.get((family, False, False), copies)
            sources = [c[5] for c in regular if abs(formula(c[3]) - target) <= 0.004]
            for copy in copies:
                if sources and copy[5] == sources[0]:
                    pick = copy
                    break
        chosen.append(pick[:5])
    return chosen


LINE_HEIGHTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "line_heights.tsv")


def measured_line_heights():
    out = {}
    if os.path.exists(LINE_HEIGHTS):
        for line in open(LINE_HEIGHTS, encoding="utf-8"):
            parts = line.rstrip("\n").split("\t")
            if len(parts) >= 2 and not line.startswith("#"):
                try:
                    out[parts[0].lower()] = float(parts[1])
                except ValueError:
                    pass
    return out


def main():
    # Extra directories (e.g. Liberation, a common LibreOffice leftover)
    # are searched after Word's own.
    DIRECTORIES.extend(sys.argv[1:])
    sets = [CODEPOINTS, SYMBOL_CODEPOINTS]
    out = bytearray(b"DCFM")
    out += struct.pack("<HH", 3, len(sets))
    for codepoints in sets:
        out += struct.pack("<H", len(codepoints))
        out += b"".join(struct.pack("<I", c) for c in codepoints)
    out += struct.pack("<B", len(KERN_CODEPOINTS))
    out += b"".join(struct.pack("<I", c) for c in KERN_CODEPOINTS)
    measured = measured_line_heights()
    substituted = {
        family
        for family, em in measured.items()
        if abs(em - measured.get("cambria", 0)) < 0.0003 and not family.startswith("cambria")
    }
    substituted.add("times")
    entries = [e for e in choose(faces(), measured) if e[0].lower() not in substituted]
    # The family's regular face decides whether its line height deviates.
    regular = {name.lower(): face for name, bold, italic, face, _ in entries if not bold and not italic}
    adjusted = 0
    kerned = 0
    out += struct.pack("<H", len(entries))
    for name, bold, italic, face, symbol in entries:
        encoded = name.encode("utf-8")[:255]
        out += struct.pack("<B", len(encoded)) + encoded
        out += struct.pack("<BB", (1 if bold else 0) | (2 if italic else 0) | (4 if symbol else 0), 1 if symbol else 0)
        line_gap = face.hhea_line_gap
        target = measured.get(name.lower())
        base = regular.get(name.lower(), face)
        formula = (base.hhea_ascender - base.hhea_descender + base.hhea_line_gap) / base.units_per_em
        if target and abs(target - formula) > 0.004:
            line_gap = round(target * face.units_per_em) - (face.hhea_ascender - face.hhea_descender)
            adjusted += 1
        out += struct.pack(
            "<HhhhH",
            face.units_per_em,
            face.hhea_ascender,
            -face.hhea_descender,
            max(min(line_gap, 32767), -32768),
            max(face.x_avg_char_width, 0),
        )
        for c in sets[1 if symbol else 0]:
            advance = face.advance(c)
            out += struct.pack("<H", 0xFFFF if advance is None else min(advance, 0xFFFE))
        pairs = [] if symbol else kerning_pairs(face, KERN_CODEPOINTS)
        kerned += len(pairs)
        out += struct.pack("<H", len(pairs))
        for left, right, value in pairs:
            out += struct.pack("<BBh", left, right, value)
    packed = zlib.compress(bytes(out), 9)
    with open(OUT, "wb") as handle:
        handle.write(packed)
    families = sorted({entry[0] for entry in entries})
    print(f"{os.path.normpath(OUT)}: {len(entries)} faces, {len(families)} families, {adjusted} line heights from measurement, {kerned} kerning pairs, {len(out)} bytes, {len(packed)} compressed")


if __name__ == "__main__":
    main()
