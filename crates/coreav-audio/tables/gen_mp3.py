#!/usr/bin/env python3
"""Generates crates/coreav-audio/src/mp3/tables.rs from the public
documents the MP3 decoder is written from (docs/design/audio-decode.md,
"MP3"), and checks every table on the way:

- the Huffman code tables, the preemphasis table and the scalefactor
  bands, from "MPEG-Layer3 Bitstream Syntax and Decoding" (M. Sieler, R.
  Sperschneider, Fraunhofer IIS, issue 1.2, 1997), Tables 23 to 59;
- the analysis window C[i] of ISO/IEC 11172-3 Table 3-C.1 (Annex C), from
  which the synthesis window D[i] = 32 C[i] of Table 3-B.3 follows.

usage: gen_mp3.py --syntax SYNTAX.pdf --annexes ANNEXES.pdf --out tables.rs
Needs `pdftotext` (poppler). The PDFs are pinned by SHA-256.
"""
import argparse
import hashlib
import pathlib
import re
import subprocess

SYNTAX_SHA256 = '9bca5f486a71ab159db1270901357b2a20d0c3a8c02c4ac521f8bb3f3d3910a6'
ANNEXES_SHA256 = '522e75059b77b1dc169b5e9b822b955d729eafb965eee64790e5d3e5eeea949b'

# Huffman table number: (document table, size of x and y, linbits).
PAIR_TABLES = {
    1: (27, 2), 2: (28, 3), 3: (29, 3), 5: (30, 4), 6: (31, 4), 7: (32, 6), 8: (33, 6), 9: (34, 6),
    10: (35, 8), 11: (36, 8), 12: (37, 8), 13: (38, 16), 15: (39, 16), 16: (40, 16), 24: (41, 16),
}
# Tables that reuse another's codes, with their linbits (ISO/IEC 11172-3
# Table 3-B.7; the document's Table 40 and 41 titles).
SHARED = {16: [(16, 1), (17, 2), (18, 3), (19, 4), (20, 6), (21, 8), (22, 10), (23, 13)],
          24: [(24, 4), (25, 5), (26, 6), (27, 7), (28, 8), (29, 9), (30, 11), (31, 13)]}
# Sample rates of the scalefactor band tables, in the document's order:
# long table number, short table number.
BANDS = {8000: (42, 43), 11025: (44, 45), 12000: (46, 47), 16000: (48, 49), 22050: (50, 51),
         24000: (52, 53), 32000: (54, 55), 44100: (56, 57), 48000: (58, 59)}
# Table 60 gives the aliasing coefficients c[i] as decimals.
ALIAS = [-0.6, -0.535, -0.33, -0.185, -0.095, -0.041, -0.0142, -0.0037]
# First-level width of the Huffman lookup tables.
LEVEL1 = 8


def text(pdf, sha):
    data = pathlib.Path(pdf).read_bytes()
    if hashlib.sha256(data).hexdigest() != sha:
        raise SystemExit(f'{pdf}: SHA-256 differs from the pinned document')
    return subprocess.run(['pdftotext', '-layout', pdf, '-'], capture_output=True, text=True, check=True).stdout


def sections(doc):
    """Document table number -> its lines, from 'Table N' to the next."""
    out, current = {}, None
    for line in doc.splitlines():
        m = re.match(r'^\s*Table (\d+)\s*$', line)
        if m:
            current = int(m.group(1))
            out[current] = []
            continue
        if current is not None:
            out[current].append(line)
    return out


def pairs(lines, size):
    """The rows `x y hlen hcod`. The text sometimes runs two columns
    together ("010" for y 0 and hlen 10); hlen is the code's length, so
    the digits before it are split off by it."""
    rows = {}
    for line in lines:
        m = re.match(r'^\s*(\d+(?:\s+\d+)*)\s+([01]+)\s*$', line)
        if not m:
            continue
        code = m.group(2)
        numbers = m.group(1).split()
        hlen = str(len(code))
        if numbers[-1] == hlen and len(numbers) == 3:
            x, y = numbers[0], numbers[1]
        elif len(numbers) == 2 and numbers[1].endswith(hlen) and len(numbers[1]) > len(hlen):
            x, y = numbers[0], numbers[1][:-len(hlen)]
        else:
            continue
        x, y = int(x), int(y)
        if x >= size or y >= size:
            raise SystemExit(f'entry {x},{y} outside a table of {size}')
        if (x, y) in rows:
            raise SystemExit(f'duplicate entry {x},{y}')
        rows[(x, y)] = code
    want = {(x, y) for x in range(size) for y in range(size)}
    if set(rows) != want:
        raise SystemExit(f'entries {sorted(want ^ set(rows))[:5]} missing or extra')
    return rows


def quads(lines):
    rows = {}
    for line in lines:
        m = re.match(r'^\s*([01])\s+([01])\s+([01])\s+([01])\s+(\d+)\s+([01]+)\s*$', line)
        if m:
            v, w, x, y = (int(m.group(i)) for i in range(1, 5))
            hlen, code = int(m.group(5)), m.group(6)
            if hlen != len(code):
                raise SystemExit(f'length {hlen} of code {code}')
            rows[(v, w, x, y)] = code
    if len(rows) != 16:
        raise SystemExit(f'{len(rows)} entries in a quadruple table')
    return rows


def check_code(codes, name):
    """The code is prefix-free and complete (Kraft sum 1)."""
    ordered = sorted(codes)
    for a, b in zip(ordered, ordered[1:]):
        if b.startswith(a):
            raise SystemExit(f'table {name}: {a} is a prefix of {b}')
    kraft = sum(2.0 ** -len(c) for c in codes)
    if abs(kraft - 1.0) > 1e-12:
        raise SystemExit(f'table {name}: Kraft sum {kraft}')


def lookup(codes):
    """Two-level lookup: LEVEL1 bits, then a subtable per longer prefix.
    Entries: leaf `len << 16 | value`; pointer `1 << 31 | bits << 24 |
    offset`. Returns the flat list."""
    first = [None] * (1 << LEVEL1)
    longer = {}
    for code, value in codes.items():
        if len(code) <= LEVEL1:
            pad = LEVEL1 - len(code)
            base = int(code, 2) << pad
            for i in range(1 << pad):
                first[base + i] = (len(code) << 16) | value
        else:
            longer.setdefault(code[:LEVEL1], []).append((code[LEVEL1:], value))
    table = list(first)
    for prefix, rest in sorted(longer.items()):
        bits = max(len(r) for r, _ in rest)
        offset = len(table)
        sub = [None] * (1 << bits)
        for r, value in rest:
            pad = bits - len(r)
            base = int(r, 2) << pad
            for i in range(1 << pad):
                sub[base + i] = ((LEVEL1 + len(r)) << 16) | value
        if offset >= 1 << 24 or bits >= 1 << 7:
            raise SystemExit('lookup table too large')
        table[int(prefix, 2)] = (1 << 31) | (bits << 24) | offset
        table.extend(sub)
    if any(e is None for e in table):
        raise SystemExit('incomplete lookup table')
    return table


def bands(lines, total):
    widths = []
    for line in lines:
        m = re.match(r'^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s*$', line)
        if m:
            sfb, width, start, end = (int(m.group(i)) for i in range(1, 5))
            if sfb != len(widths) or start != sum(widths) or end != start + width - 1:
                raise SystemExit(f'band {sfb} does not follow the previous ones')
            widths.append(width)
    # The document lists the bands that carry scalefactors (21 long, 12
    # short); the last band is the rest of the granule.
    if len(widths) not in (12, 21) or sum(widths) >= total:
        raise SystemExit(f'{len(widths)} bands summing to {sum(widths)} of {total}')
    return widths + [total - sum(widths)]


def window(annexes):
    start = annexes.index('Table 3-C.1')
    region = annexes[start:start + 60000]
    values = {}
    for m in re.finditer(r'C\[\s*(\d+)\]\s*=\s*(-?)\s*(\d\.\d{9})', region):
        i = int(m.group(1))
        if i not in values:
            values[i] = -float(m.group(3)) if m.group(2) else float(m.group(3))
    if sorted(values) != list(range(512)):
        raise SystemExit(f'{len(values)} analysis window coefficients')
    return [values[i] for i in range(512)]


def rust_array(name, ty, items, per_line=8, doc=None):
    lines = [f'/// {doc}' if doc else '', f'pub(crate) static {name}: [{ty}; {len(items)}] = [']
    for i in range(0, len(items), per_line):
        lines.append('    ' + ' '.join(f'{v},' for v in items[i:i + per_line]))
    lines.append('];')
    return '\n'.join(l for l in lines if l != '') + '\n'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--syntax', required=True)
    parser.add_argument('--annexes', required=True)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    doc = sections(text(args.syntax, SYNTAX_SHA256))
    annexes = text(args.annexes, ANNEXES_SHA256)

    out = ['// Generated by crates/coreav-audio/tables/gen_mp3.py from the documents',
           '// it names; do not edit. Regenerate it when the generator changes.',
           '//! The tables of the MP3 decoder (`docs/design/audio-decode.md`, "MP3").',
           '#![cfg_attr(rustfmt, rustfmt::skip)]', '']
    # Huffman pair tables.
    flat, index = [], {}
    for number, (table, size) in sorted(PAIR_TABLES.items()):
        codes = pairs(doc[table], size)
        check_code(codes.values(), number)
        entries = lookup({code: (x << 4) | y for (x, y), code in codes.items()})
        index[number] = (len(flat), size)
        flat.extend(entries)
    out.append(rust_array('PAIR_LOOKUP', 'u32', [f'0x{e:08x}' for e in flat], 6,
                          'The pair Huffman tables (ISO/IEC 11172-3 Table 3-B.7, tables 1 to 31): two-level lookups, '
                          'entries `len << 16 | x << 4 | y` or `1 << 31 | bits << 24 | offset`.'))
    # table_select -> (offset into PAIR_LOOKUP, linbits); 0, 4 and 14 have no codes.
    select = []
    for number in range(32):
        base = 16 if 16 <= number <= 23 else 24 if number >= 24 else number
        if base not in index:
            select.append('(u32::MAX, 0)')
            continue
        linbits = 0
        for n, l in SHARED.get(base, []):
            if n == number:
                linbits = l
        select.append(f'({index[base][0]}, {linbits})')
    out.append(rust_array('PAIR_TABLES', '(u32, u32)', select, 4,
                          'Per `table_select`: the table\'s start in `PAIR_LOOKUP` (`u32::MAX` for the tables without '
                          'codes: 0, 4 and 14) and its linbits.'))
    # Quadruple tables A and B.
    for name, table in (('QUAD_A', 24), ('QUAD_B', 25)):
        codes = quads(doc[table])
        check_code(codes.values(), name)
        entries = lookup({code: (v << 3) | (w << 2) | (x << 1) | y for (v, w, x, y), code in codes.items()})
        out.append(rust_array(f'{name}_LOOKUP', 'u32', [f'0x{e:08x}' for e in entries], 6,
                              f'Count1 table {name[-1]}: entries `len << 16 | v << 3 | w << 2 | x << 1 | y`.'))
    # Preemphasis.
    pretab = []
    for line in doc[23]:
        m = re.match(r'^\s*(\d+?)\s*(\d)\s*$', line.strip()) if line.strip() else None
        nums = re.findall(r'\d+', line)
        if len(nums) == 2:
            pretab.append(int(nums[1]))
        elif len(nums) == 1 and len(nums[0]) == 2:
            # Rows 0 to 9 print as "sfb value" without a gap ("00", "10").
            pretab.append(int(nums[0][1]))
    if pretab != [0] * 11 + [1, 1, 1, 1, 2, 2, 3, 3, 3, 2] or len(pretab) != 21:
        raise SystemExit(f'pretab {pretab}')
    out.append(rust_array('PRETAB', 'u8', pretab + [0], 11, 'Preemphasis (`pretab`), 21 long bands and one more of 0.'))
    # Scalefactor bands.
    long_rows, short_rows = [], []
    for rate, (lt, st) in BANDS.items():
        long_rows.append('[' + ', '.join(str(w) for w in bands(doc[lt], 576)) + ']')
        short_rows.append('[' + ', '.join(str(w) for w in bands(doc[st], 192)) + ']')
    rates = ', '.join(str(r) for r in BANDS)
    out.append(f'/// The sample rates of the band tables, in order: {rates}.\n'
               f'pub(crate) const BAND_RATES: [u32; 9] = [{rates}];\n')
    out.append('/// Widths of the 22 long-block scalefactor bands, per rate of `BAND_RATES`.\n'
               'pub(crate) const LONG_BANDS: [[u16; 22]; 9] = [\n' +
               ''.join(f'    {r},\n' for r in long_rows) + '];\n')
    out.append('/// Widths of the 13 short-block scalefactor bands (per window), per rate of `BAND_RATES`.\n'
               'pub(crate) const SHORT_BANDS: [[u16; 13]; 9] = [\n' +
               ''.join(f'    {r},\n' for r in short_rows) + '];\n')
    # Aliasing coefficients: c[i] from Table 60 of the document (Table 3-B.9).
    if not all(f'{c}'.lstrip('-') in '\n'.join(doc[60]) for c in ALIAS):
        raise SystemExit('aliasing coefficients differ from Table 60')
    out.append(rust_array('ALIAS', 'f64', [repr(c) for c in ALIAS], 8,
                          'The aliasing reduction coefficients c[i] (Table 3-B.9).'))
    # Synthesis window.
    c = window(annexes)
    out.append(rust_array('ANALYSIS_WINDOW', 'f64', [repr(v) for v in c], 4,
                          'The analysis window C[i] of ISO/IEC 11172-3 Table 3-C.1; the synthesis window of Table 3-B.3 '
                          'is D[i] = 32 C[i].'))
    pathlib.Path(args.out).write_text('\n'.join(out))


if __name__ == '__main__':
    main()
