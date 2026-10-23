#!/usr/bin/env python3
"""Generate the breengel suite's font test data (docs/gui/breengel.md sections 8.6 and 10).

Writes 64 TrueType files built from literal tables to testdata/breengel/fonts-corpus/, each
breaking one rule of section 10, and index.txt naming each file's documented error:

    <file> font <Error>                  Font::parse refuses the whole font
    <file> glyph <id> <size> <Error>     the font parses; that glyph at that size is refused

The base font matches the suite's own test font (raster.rs, Ttf::base): unitsPerEm 1000,
glyph 0 and 1 a square, glyph 2 a triangle, glyph 3 a compound of glyph 1. Nothing here
reads or derives from any other font implementation; the layouts are the OpenType
specification's. Output is deterministic: run it again and the files are byte-identical.

It also writes fonts/compound-transform.ttf beside the corpus: a valid font whose compound
glyph uses each component transform, which text/compound-transform renders.

Usage: tools/font-corpus/generate.py [output-dir]
"""

import struct
import sys
from pathlib import Path

SQUARE = [(100, 0), (100, 700), (600, 700), (600, 0)]
TRIANGLE = [(0, 0), (350, 700), (700, 0)]


def be16(v):
    return struct.pack('>H', v & 0xFFFF)


def be32(v):
    return struct.pack('>I', v & 0xFFFFFFFF)


def simple_glyph(contours, bbox=None, flags=None, instructions=b''):
    """Contours of on-curve points (or (x, y, on) triples), coordinates as 16-bit deltas."""
    pts = [p if len(p) == 3 else (p[0], p[1], True) for c in contours for p in c]
    xs, ys = [p[0] for p in pts] or [0], [p[1] for p in pts] or [0]
    box = bbox or (min(xs), min(ys), max(xs), max(ys))
    v = be16(len(contours)) + b''.join(be16(b) for b in box)
    end = 0
    for c in contours:
        end += len(c)
        v += be16(end - 1)
    v += be16(len(instructions)) + instructions
    v += flags if flags is not None else bytes(0x01 if p[2] else 0x00 for p in pts)
    px = py = 0
    for x, _, _ in pts:
        v += be16(x - px)
        px = x
    for _, y, _ in pts:
        v += be16(y - py)
        py = y
    return v


def compound_glyph(parts, more_flags=0):
    """Components (glyph, dx, dy) with word offsets."""
    v = be16(0xFFFF) + b''.join(be16(b) for b in (0, 0, 1000, 1000))
    for i, (g, dx, dy) in enumerate(parts):
        more = 0x0020 if i + 1 < len(parts) else 0
        v += be16(0x0001 | 0x0002 | more | more_flags) + be16(g) + be16(dx) + be16(dy)
    return v


def cmap4(segments, seg_count_x2=None, length=None):
    """One (3, 1) format 4 subtable of segments (start, end, delta, glyph_ids or None)."""
    n = len(segments)
    arrays = sum(len(s[3]) for s in segments if s[3])
    sub = be16(4) + be16(length if length is not None else 16 + 8 * n + 2 * arrays) + be16(0)
    sub += be16(seg_count_x2 if seg_count_x2 is not None else 2 * n) + be16(0) * 3
    sub += b''.join(be16(s[1]) for s in segments) + be16(0)
    sub += b''.join(be16(s[0]) for s in segments)
    sub += b''.join(be16(s[2]) for s in segments)
    at = 0
    for i, s in enumerate(segments):
        if s[3]:
            sub += be16(2 * (n - i) + 2 * at)
            at += len(s[3])
        else:
            sub += be16(0)
    for s in segments:
        for g in s[3] or []:
            sub += be16(g)
    return be16(0) + be16(1) + be16(3) + be16(1) + be32(12) + sub


def cmap12(groups, n_groups=None):
    sub = be16(12) + be16(0) + be32(16 + 12 * len(groups)) + be32(0)
    sub += be32(n_groups if n_groups is not None else len(groups))
    for a, b, g in groups:
        sub += be32(a) + be32(b) + be32(g)
    return be16(0) + be16(1) + be16(3) + be16(10) + be32(12) + sub


BASE_CMAP = cmap4([(0x41, 0x43, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)])


class Ttf:
    def __init__(self):
        self.units_per_em = 1000
        self.glyphs = [simple_glyph([SQUARE]), simple_glyph([SQUARE]), simple_glyph([TRIANGLE]), compound_glyph([(1, 100, 0)])]
        self.num_glyphs = None
        self.max_points = 4
        self.max_composite_points = 8
        self.long_metrics = [(700, 100), (700, 100), (720, 0), (800, 100)]
        self.trailing_lsb = []
        self.num_h_metrics = None
        self.cmap = BASE_CMAP
        self.loca = None
        self.loca_format = 1
        self.index_to_loc_format = None
        self.version = 0x00010000
        self.tables = {}
        self.drop = set()
        self.records = {}

    def add(self, glyph, metric=(700, 0)):
        self.glyphs.append(glyph)
        self.long_metrics.append(metric)
        return len(self.glyphs) - 1

    def build(self):
        glyf, loca = b'', []
        for g in self.glyphs:
            loca.append(len(glyf))
            glyf += g
            glyf += b'\0' * (-len(glyf) % 4)
        loca.append(len(glyf))
        loca = self.loca if self.loca is not None else loca
        loca_bytes = b''.join(be32(o) if self.loca_format == 1 else be16(o // 2) for o in loca)
        num_glyphs = self.num_glyphs if self.num_glyphs is not None else len(self.glyphs)
        nhm = self.num_h_metrics if self.num_h_metrics is not None else len(self.long_metrics)
        ilf = self.index_to_loc_format if self.index_to_loc_format is not None else self.loca_format
        head = be32(0x00010000) + be32(0x00010000) + be32(0) + be32(0x5F0F3CF5) + be16(0) + be16(self.units_per_em)
        head += b'\0' * 16 + b''.join(be16(b) for b in (0, 0, 1000, 1000)) + be16(0) + be16(8) + be16(2) + be16(ilf) + be16(0)
        hhea = be32(0x00010000) + be16(800) + be16(-200) + be16(0) + be16(800) + b'\0' * 22 + be16(nhm)
        maxp = be32(0x00010000) + be16(num_glyphs) + be16(self.max_points) + be16(4) + be16(self.max_composite_points) + be16(2) + be16(2) + b'\0' * 12 + be16(4) + be16(2)
        hmtx = b''.join(be16(a) + be16(l) for a, l in self.long_metrics) + b''.join(be16(l) for l in self.trailing_lsb)
        tables = {b'cmap': self.cmap, b'glyf': glyf, b'head': head, b'hhea': hhea, b'hmtx': hmtx, b'loca': loca_bytes, b'maxp': maxp}
        tables.update(self.tables)
        order = [t for t in sorted(tables) if t not in self.drop]
        out = be32(self.version) + be16(len(order)) + be16(64) + be16(2) + be16(48)
        offset = 12 + 16 * len(order)
        body = b''
        for tag in order:
            data = tables[tag]
            off, length = self.records.get(tag, (offset, len(data)))
            out += tag + be32(0) + be32(off) + be32(length)
            body += data + b'\0' * (-len(data) % 4)
            offset = 12 + 16 * len(order) + len(body)
        return out + body


def glyph2(glyph):
    t = Ttf()
    t.glyphs[2] = glyph
    return t


def corpus():
    """(name, bytes, index entry) for each of the 64 files."""
    out = []

    def font(name, data, error):
        out.append((name, data, f'font {error}'))

    def glyph(name, t, g, size, error):
        out.append((name, t.build(), f'glyph {g} {size} {error}'))

    full = Ttf().build()
    font('empty', b'', 'Truncated')
    font('header-short', full[:11], 'Truncated')
    font('directory-short', full[:12 + 16 * 3], 'Truncated')
    for tag, record in ((b'head', lambda n: (n + 4, 54)), (b'glyf', lambda n: (12 + 16 * 7, n)), (b'cmap', lambda n: (0xFFFFFFF0, 64))):
        t = Ttf()
        t.records[tag] = record(len(full))
        font(f'record-{tag.decode()}-past-file', t.build(), f'BadTable({tag.decode()})')
    for tag in (b'head', b'hhea', b'maxp', b'cmap', b'hmtx', b'loca', b'glyf'):
        t = Ttf()
        t.drop.add(tag)
        font(f'missing-{tag.decode()}', t.build(), f'MissingTable({tag.decode()})')
    for tag, cut in ((b'head', 20), (b'hhea', 20), (b'maxp', 4), (b'maxp', 16)):
        t = Ttf()
        t.tables[tag] = table_of(full, tag)[:cut]
        font(f'{tag.decode()}-{cut}-bytes', t.build(), 'Truncated')
    for upem in (0, 16385, 65535):
        t = Ttf()
        t.units_per_em = upem
        font(f'units-per-em-{upem}', t.build(), 'BadUnitsPerEm')
    t = Ttf()
    t.index_to_loc_format = 2
    font('index-to-loc-format-2', t.build(), 'BadTable(head)')
    t = Ttf()
    t.loca = [0, 20, 12, 40, 60]
    font('loca-decreasing', t.build(), 'BadLoca')
    t = Ttf()
    good = loca_of(t)
    t.loca = good[:-1] + [good[-2] - 4]
    font('loca-last-decreasing', t.build(), 'BadLoca')
    t = Ttf()
    t.num_glyphs = 6
    font('loca-fewer-than-maxp', t.build(), 'GlyphCountMismatch')
    t = Ttf()
    t.num_glyphs = 2
    t.num_h_metrics = 2
    font('loca-more-than-maxp', t.build(), 'GlyphCountMismatch')
    t = Ttf()
    t.index_to_loc_format = 0
    font('loca-long-entries-short-format', t.build(), 'GlyphCountMismatch')
    segs = [(0x41, 0x43, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)]
    for name, cmap in (
        ('cmap4-segcountx2-odd', cmap4(segs, seg_count_x2=3)),
        ('cmap4-segcountx2-zero', cmap4(segs, seg_count_x2=0)),
        ('cmap4-end-codes-unsorted', cmap4([(0x50, 0x52, 1 - 0x50, None), (0x41, 0x43, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)])),
        ('cmap4-end-codes-repeated', cmap4([(0x41, 0x43, 1 - 0x41, None), (0x43, 0x43, 0, None), (0xFFFF, 0xFFFF, 1, None)])),
        ('cmap4-last-end-not-ffff', cmap4([(0x41, 0x43, 1 - 0x41, None)])),
        ('cmap4-range-offset-outside', cmap4_range_offset(4000)),
        ('cmap4-arrays-past-length', cmap4(segs, seg_count_x2=40)),
        ('cmap12-overlapping', cmap12([(0x41, 0x45, 1), (0x44, 0x46, 2)])),
        ('cmap12-unsorted', cmap12([(0x50, 0x51, 1), (0x41, 0x42, 2)])),
        ('cmap12-inverted', cmap12([(0x45, 0x41, 1)])),
        ('cmap12-groups-past-length', cmap12([(0x41, 0x42, 1)], n_groups=1000)),
    ):
        t = Ttf()
        t.cmap = cmap
        font(name, t.build(), 'BadCmap')
    t = Ttf()
    t.cmap = be16(0) + be16(1) + be16(3) + be16(1) + be32(4000)
    font('cmap-subtable-past-table', t.build(), 'Truncated')
    t = Ttf()
    t.cmap = be16(0) + be16(9) + be16(3) + be16(1) + be32(12)
    font('cmap-records-past-table', t.build(), 'Truncated')
    t = Ttf()
    t.cmap = BASE_CMAP[:12] + be16(4) + be16(2000) + BASE_CMAP[16:]
    font('cmap4-length-past-table', t.build(), 'Truncated')
    t = Ttf()
    t.tables[b'hmtx'] = b''.join(be16(a) + be16(l) for a, l in t.long_metrics[:3])
    font('hmtx-short', t.build(), 'BadHmtx')
    for nhm in (0, 5):
        t = Ttf()
        t.num_h_metrics = nhm
        font(f'hmtx-number-of-hmetrics-{nhm}', t.build(), 'BadHmtx')
    t = Ttf()
    t.version = 0x4F54544F
    font('sfnt-version-otto', t.build(), 'BadTable(sfnt)')

    t = Ttf()
    good = loca_of(t)
    t.loca = good[:3] + [good[4] + 40, good[4] + 80]
    glyph('loca-glyph-past-glyf', t, 3, 24, 'GlyphOutOfRange')
    g = bytearray(simple_glyph([[(0, 0), (10, 10), (20, 0)], [(30, 0), (40, 10), (50, 0)]]))
    g[10:14] = bytes([0, 5, 0, 2])
    glyph('glyf-end-points-decreasing', glyph2(bytes(g)), 2, 24, 'BadGlyph')
    glyph('glyf-points-above-maxp', glyph2(simple_glyph([[(0, 0), (10, 10), (20, 0), (30, 10), (40, 0), (50, 10)]])), 2, 24, 'TooManyPoints')
    t = glyph2(be16(1) + be16(0) * 4 + be16(65535) + be16(0) + b'\x01' * 16)
    t.max_points = 65535
    glyph('glyf-65536-points', t, 2, 24, 'TooManyPoints')
    glyph('glyf-flag-repeat-past-points', glyph2(be16(1) + be16(0) * 2 + be16(100) * 2 + be16(3) + be16(0) + bytes([0x09, 200]) + b'\0' * 16), 2, 24, 'BadGlyph')
    glyph('glyf-flags-cut', glyph2(be16(1) + be16(0) * 2 + be16(100) * 2 + be16(3) + be16(0) + b'\x01\x01'), 2, 24, 'BadGlyph')
    glyph('glyf-x-cut', glyph2(be16(1) + be16(0) * 2 + be16(100) * 2 + be16(3) + be16(0) + b'\x01' * 4 + be16(5) * 2), 2, 24, 'BadGlyph')
    glyph('glyf-y-cut', glyph2(be16(1) + be16(0) * 2 + be16(100) * 2 + be16(3) + be16(0) + b'\x01' * 4 + be16(5) * 4 + be16(5)), 2, 24, 'BadGlyph')
    glyph('glyf-instructions-past-glyph', glyph2(be16(1) + be16(0) * 2 + be16(100) * 2 + be16(3) + be16(500) + b'\0' * 8), 2, 24, 'BadGlyph')
    glyph('glyf-header-cut', glyph2(be16(1) + be16(0) * 2), 2, 24, 'BadGlyph')
    t = Ttf()
    for depth in range(1, 10):
        t.add(compound_glyph([(1 if depth == 1 else 3 + depth - 1, 10, 0)]))
    glyph('compound-nested-9', t, 12, 24, 'CompoundTooDeep')
    t = Ttf()
    t.add(compound_glyph([(1, i, 0) for i in range(257)]))
    glyph('compound-257-components', t, 4, 24, 'CompoundTooDeep')
    t = Ttf()
    t.add(compound_glyph([(4, 0, 0)]))
    glyph('compound-contains-itself', t, 4, 24, 'CompoundCycle')
    t = Ttf()
    t.add(compound_glyph([(5, 0, 0)]))
    t.add(compound_glyph([(4, 0, 0)]))
    glyph('compound-two-cycle', t, 4, 24, 'CompoundCycle')
    t = Ttf()
    t.add(compound_glyph([(5, 0, 0)]))
    t.add(compound_glyph([(6, 0, 0)]))
    t.add(compound_glyph([(1, 0, 0), (4, 0, 0)]))
    glyph('compound-three-cycle', t, 4, 24, 'CompoundCycle')
    glyph('compound-component-past-font', glyph2(compound_glyph([(99, 0, 0)])), 2, 24, 'BadComponent')
    glyph('compound-component-cut', glyph2(compound_glyph([(1, 0, 0)])[:16]), 2, 24, 'BadGlyph')
    t = glyph2(be16(0xFFFF) + be16(0) * 2 + be16(1000) * 2 + be16(0x0001 | 0x0020) + be16(1) + be16(0) + be16(0)
               + be16(0x0001) + be16(1) + be16(40) + be16(0))
    glyph('compound-point-match-past-points', t, 2, 24, 'BadGlyph')
    # 130 quadratics whose control points swing across the whole coordinate range, under
    # a header box that claims 10 units: at 1,024 px each subdivides to 512 segments.
    curve = []
    for i in range(130):
        curve.append((i * 4, 0, True))
        curve.append((i * 4 + 2, 32767 if i % 2 else -32768, False))
    t = glyph2(simple_glyph([curve], bbox=(0, 0, 10, 10)))
    t.max_points = 260
    glyph('glyf-flattens-past-65536-segments', t, 2, 1024, 'GlyphTooComplex')
    t = glyph2(simple_glyph([[(-32768, -32768), (-32768, 32767), (32767, 32767), (32767, -32768)]]))
    glyph('glyf-bitmap-past-4s', t, 2, 24, 'GlyphTooExpensive')
    return out


def transform_component(glyph, dx, dy, flag, values, more):
    """A word-offset component with a scale (0x0008), x and y scale (0x0040) or 2x2 (0x0080)."""
    v = be16(0x0001 | 0x0002 | flag | (0x0020 if more else 0)) + be16(glyph) + be16(dx) + be16(dy)
    for x in values:
        v += be16(round(x * 16384))
    return v


def compound_transform_font():
    """The valid font text/compound-transform renders: 'B' is glyph 1, an F with a rounded
    bowl beside it; 'A' is glyph 2, three copies of glyph 1 scaled by 0.5, by (1.5, 0.75)
    and through a 30-degree rotation."""
    t = Ttf()
    f_shape = [(0, 0), (0, 700), (400, 700), (400, 600), (100, 600), (100, 400), (300, 400), (300, 300), (100, 300), (100, 0)]
    bowl = [(575, 0, True), (700, 0, False), (700, 125, True), (700, 250, False), (575, 250, True), (450, 250, False), (450, 125, True), (450, 0, False)]
    t.glyphs = [simple_glyph([SQUARE]), simple_glyph([f_shape, bowl])]
    t.max_points = 18
    t.max_composite_points = 54
    cos30, sin30 = 0.8660254037844386, 0.5
    parts = (transform_component(1, 0, 0, 0x0008, [0.5], True)
             + transform_component(1, 500, 0, 0x0040, [1.5, 0.75], True)
             + transform_component(1, 1900, 100, 0x0080, [cos30, sin30, -sin30, cos30], False))
    t.glyphs.append(be16(0xFFFF) + b''.join(be16(b) for b in (-350, 0, 2600, 1000)) + parts)
    t.long_metrics = [(700, 100), (750, 0), (2700, 0)]
    t.cmap = cmap4([(0x41, 0x42, 0, [2, 1]), (0xFFFF, 0xFFFF, 1, None)])
    return t.build()


def table_of(file, tag):
    n = struct.unpack('>H', file[4:6])[0]
    for i in range(n):
        r = 12 + 16 * i
        if file[r:r + 4] == tag:
            off, length = struct.unpack('>II', file[r + 8:r + 16])
            return file[off:off + length]
    raise KeyError(tag)


def loca_of(t):
    data = table_of(t.build(), b'loca')
    return [struct.unpack('>I', data[i:i + 4])[0] for i in range(0, len(data), 4)]


def cmap4_range_offset(value):
    cmap = bytearray(cmap4([(0x41, 0x43, 0, [1, 2, 3]), (0xFFFF, 0xFFFF, 1, None)]))
    at = 12 + 16 + 3 * 2 * 2
    cmap[at:at + 2] = be16(value)
    return bytes(cmap)


def main():
    root = Path(__file__).resolve().parents[2]
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else root / 'testdata/breengel/fonts-corpus'
    files = corpus()
    assert len(files) == 64, f'{len(files)} corpus files; the contract has 64'
    out_dir.mkdir(parents=True, exist_ok=True)
    for old in out_dir.glob('*.ttf'):
        old.unlink()
    lines = []
    for i, (name, data, entry) in enumerate(files, 1):
        file = f'{i:02d}-{name}.ttf'
        (out_dir / file).write_bytes(data)
        lines.append(f'{file} {entry}')
    (out_dir / 'index.txt').write_text(
        '# The breengel malformed font corpus (docs/gui/breengel.md section 10), generated by\n'
        '# tools/font-corpus/generate.py. Each line: <file> font <Error>, or\n'
        '# <file> glyph <id> <size> <Error> for a defect confined to one glyph.\n'
        + '\n'.join(lines) + '\n')
    print(f'wrote {len(files)} files and index.txt to {out_dir}')
    fonts_dir = out_dir.parent / 'fonts'
    fonts_dir.mkdir(parents=True, exist_ok=True)
    (fonts_dir / 'compound-transform.ttf').write_bytes(compound_transform_font())
    print(f'wrote {fonts_dir / "compound-transform.ttf"}')


if __name__ == '__main__':
    main()
