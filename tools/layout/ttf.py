"""Minimal TrueType/OpenType metric reader (standard library only).

Reads what text layout needs from a .ttf/.otf (or one face of a .ttc):
units per em, the hhea, OS/2 and head vertical metrics, the Unicode cmap
(formats 4 and 12) and the horizontal advances (hmtx).
"""

import struct


def face_count(data):
    """Faces in a font file (a .ttc holds several)."""
    if data[:4] == b"ttcf":
        return struct.unpack(">I", data[8:12])[0]
    return 1


class Face:
    def __init__(self, data, index=0):
        self.data = data
        offset = 0
        if data[:4] == b"ttcf":
            offset = struct.unpack(">I", data[12 + 4 * index : 16 + 4 * index])[0]
        num_tables = struct.unpack(">H", data[offset + 4 : offset + 6])[0]
        self.tables = {}
        for i in range(num_tables):
            rec = offset + 12 + 16 * i
            tag = data[rec : rec + 4].decode("latin-1")
            _, start, length = struct.unpack(">III", data[rec + 4 : rec + 16])
            self.tables[tag] = (start, length)
        head = self.table("head")
        self.units_per_em = struct.unpack(">H", head[18:20])[0]
        self.mac_style = struct.unpack(">H", head[44:46])[0]
        hhea = self.table("hhea")
        self.hhea_ascender, self.hhea_descender, self.hhea_line_gap = struct.unpack(">hhh", hhea[4:10])
        self.num_hmetrics = struct.unpack(">H", hhea[34:36])[0]
        os2 = self.table("OS/2")
        self.typo_ascender, self.typo_descender, self.typo_line_gap = struct.unpack(">hhh", os2[68:74])
        self.win_ascent, self.win_descent = struct.unpack(">HH", os2[74:78])
        self.fs_selection = struct.unpack(">H", os2[62:64])[0]
        self.x_avg_char_width = struct.unpack(">h", os2[2:4])[0]
        hmtx = self.table("hmtx")
        self.advances = [struct.unpack(">H", hmtx[4 * i : 4 * i + 2])[0] for i in range(self.num_hmetrics)]
        self.cmap = self._read_cmap()
        self.names = self._read_names()

    def name(self, name_id):
        return self.names.get(name_id)

    def _read_names(self):
        """Windows-platform English names by id (1 family, 2 subfamily)."""
        if "name" not in self.tables:
            return {}
        table = self.table("name")
        count, strings = struct.unpack(">HH", table[2:6])
        out = {}
        for i in range(count):
            platform, encoding, language, name_id, length, offset = struct.unpack(
                ">HHHHHH", table[6 + 12 * i : 18 + 12 * i]
            )
            raw = table[strings + offset : strings + offset + length]
            if platform == 3 and language in (0x409, 0) and name_id not in out:
                out[name_id] = raw.decode("utf-16-be", "replace")
            elif platform == 1 and language == 0 and name_id not in out:
                out.setdefault(-name_id, raw.decode("mac-roman", "replace"))
        for key in [k for k in out if k < 0]:
            out.setdefault(-key, out.pop(key))
        return out

    def table(self, tag):
        start, length = self.tables[tag]
        return self.data[start : start + length]

    def _read_cmap(self):
        cmap = self.table("cmap")
        count = struct.unpack(">H", cmap[2:4])[0]
        best = None
        for i in range(count):
            platform, encoding, offset = struct.unpack(">HHI", cmap[4 + 8 * i : 12 + 8 * i])
            fmt = struct.unpack(">H", cmap[offset : offset + 2])[0]
            rank = {(3, 10): 3, (0, 4): 3, (3, 1): 2, (0, 3): 2, (3, 0): 1}.get((platform, encoding), 0)
            if fmt in (4, 12) and (best is None or rank > best[0]):
                best = (rank, offset, fmt)
        if best is None:
            return {}
        _, offset, fmt = best
        out = {}
        if fmt == 4:
            seg_x2 = struct.unpack(">H", cmap[offset + 6 : offset + 8])[0]
            segs = seg_x2 // 2
            ends = offset + 14
            starts = ends + seg_x2 + 2
            deltas = starts + seg_x2
            ranges = deltas + seg_x2
            for s in range(segs):
                end = struct.unpack(">H", cmap[ends + 2 * s : ends + 2 * s + 2])[0]
                start = struct.unpack(">H", cmap[starts + 2 * s : starts + 2 * s + 2])[0]
                delta = struct.unpack(">h", cmap[deltas + 2 * s : deltas + 2 * s + 2])[0]
                range_offset = struct.unpack(">H", cmap[ranges + 2 * s : ranges + 2 * s + 2])[0]
                for c in range(start, end + 1):
                    if c == 0xFFFF:
                        continue
                    if range_offset == 0:
                        glyph = (c + delta) & 0xFFFF
                    else:
                        at = ranges + 2 * s + range_offset + 2 * (c - start)
                        glyph = struct.unpack(">H", cmap[at : at + 2])[0]
                        if glyph:
                            glyph = (glyph + delta) & 0xFFFF
                    if glyph:
                        out[c] = glyph
        else:
            groups = struct.unpack(">I", cmap[offset + 12 : offset + 16])[0]
            for g in range(groups):
                at = offset + 16 + 12 * g
                start, end, glyph = struct.unpack(">III", cmap[at : at + 12])
                for c in range(start, end + 1):
                    out[c] = glyph + c - start
        return out

    def kerning(self):
        """Pair kerning from the classic 'kern' table: {(left glyph, right
        glyph): value in font units}. Horizontal format 0 subtables of the
        Microsoft (version 0) and Apple (version 1) layouts."""
        if "kern" not in self.tables:
            return {}
        table = self.table("kern")
        out = {}
        version = struct.unpack(">H", table[0:2])[0]
        if version == 0:
            count = struct.unpack(">H", table[2:4])[0]
            at = 4
            for _ in range(count):
                _, length, coverage = struct.unpack(">HHH", table[at : at + 6])
                fmt = coverage >> 8
                horizontal = coverage & 1 and not coverage & 6
                if fmt == 0 and horizontal:
                    self._kern_pairs(table, at + 6, out)
                at += length
        elif struct.unpack(">I", table[0:4])[0] == 0x00010000:
            count = struct.unpack(">I", table[4:8])[0]
            at = 8
            for _ in range(count):
                length, coverage = struct.unpack(">IH", table[at : at + 6])
                fmt = coverage & 0xFF
                if fmt == 0 and not coverage & 0xE000:
                    self._kern_pairs(table, at + 8, out)
                at += length
        return out

    @staticmethod
    def _kern_pairs(table, at, out):
        pairs = struct.unpack(">H", table[at : at + 2])[0]
        at += 8
        for i in range(pairs):
            left, right, value = struct.unpack(">HHh", table[at + 6 * i : at + 6 * i + 6])
            out.setdefault((left, right), value)

    def advance(self, codepoint):
        glyph = self.cmap.get(codepoint)
        if glyph is None:
            return None
        return self.advances[min(glyph, self.num_hmetrics - 1)]


def load(path, index=0):
    with open(path, "rb") as handle:
        return Face(handle.read(), index)
