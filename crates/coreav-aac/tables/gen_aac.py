#!/usr/bin/env python3
"""Generates crates/coreav-aac/src/tables.rs from the standards the AAC-LC
decoder is written from (docs/design/audio-decode.md, "AAC-LC"), and
checks every table on the way:

- the scalefactor and spectrum Huffman codebooks, from ISO/IEC 14496-3:2009
  Tables 4.A.1 to 4.A.12, which must equal ISO/IEC 13818-7:2004 Tables A.1
  to A.12 entry for entry; each code is complete and prefix-free;
- the scalefactor bands of frames of 1,024 and 960 samples, from ISO/IEC
  14496-3 Tables 4.129 to 4.141 (the 960-sample values in brackets), whose
  1,024-sample values must equal ISO/IEC 13818-7 Tables 45 to 57;
- TNS_MAX_BANDS (14496-3 Table 4.157) and the sample rates (Table 1.18),
  as printed;

and computes the rest, so that the decoder computes no transcendental
function at run time: the sine and Kaiser-Bessel derived windows
(14496-3 4.6.11.3.2), |q|^(4/3) for every quantized value, the
inverse-quantized TNS coefficients (4.6.9.3) and the twiddles of the
IMDCT's FFT.

usage: gen_aac.py --mpeg4 14496-3.pdf --mpeg2 13818-7.pdf --out tables.rs
Needs `pdftotext` (poppler). The PDFs are pinned by SHA-256. Python 3
standard library only.
"""
import argparse
import hashlib
import math
import pathlib
import re
import subprocess

MPEG4_SHA256 = '913b72c340ba90bc9f4fc3bbc2edad8005369076c78a51f147bbbbbe6b5cf20d'
MPEG2_SHA256 = '1a8b8df2adfa2b865e07b5cc008c34f1978077a5019eb229ea676c8abf4ddbeb'

# Spectrum codebooks (14496-3 Table 4.151): number -> (unsigned, dimension,
# largest absolute value); 11's 16 is the escape flag.
CODEBOOKS = {1: (0, 4, 1), 2: (0, 4, 1), 3: (1, 4, 2), 4: (1, 4, 2), 5: (0, 2, 4), 6: (0, 2, 4),
             7: (1, 2, 7), 8: (1, 2, 7), 9: (1, 2, 12), 10: (1, 2, 12), 11: (1, 2, 16)}
SF_ENTRIES = 121

# Sample rates by samplingFrequencyIndex (14496-3 Table 1.18).
SAMPLE_RATES = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350]
# The table of each band table's rates: 14496-3 table numbers, long and
# short, with the rates (in Hz) their "fs [kHz]" line names; and the
# 13818-7 table numbers of the same bands.
BAND_TABLES = [
    ((4.140, 4.141), (96000, 88200), (56, 57)),
    ((4.138, 4.139), (64000,), (54, 55)),
    ((4.129, 4.130), (48000, 44100), (45, 46)),
    ((4.131, 4.130), (32000,), (47, 46)),
    ((4.136, 4.137), (24000, 22050), (52, 53)),
    ((4.134, 4.135), (16000, 12000, 11025), (50, 51)),
    ((4.132, 4.133), (8000,), (48, 49)),
]
# TNS_MAX_BANDS for object types without the PQF filterbank, long and short
# windows, by rate (14496-3 Table 4.157).
TNS_MAX_BANDS = {96000: (31, 9), 88200: (31, 9), 64000: (34, 10), 48000: (40, 14), 44100: (42, 14),
                 32000: (51, 14), 24000: (46, 14), 22050: (46, 14), 16000: (42, 14), 12000: (42, 14),
                 11025: (42, 14), 8000: (39, 14)}
# Largest quantized magnitude (14496-3 4.6.1.3) plus the largest pulse
# amplitude (4.6.3.3), which a pulse may add to it.
MAX_QUANT = 8191 + 15
# Kaiser-Bessel kernel alpha for long and short windows (4.6.11.3.2).
KBD_ALPHA = {2048: 4, 1920: 4, 256: 6, 240: 6}
# First-level width of the Huffman lookup tables, at most.
LEVEL1 = 9
# Sizes of the spectrum tables, powers of two so that an index masked to
# them needs no bounds check.
SPECTRUM_ENTRIES = 8192
SPECTRUM_VECTORS = 2048


def text(pdf, sha):
    data = pathlib.Path(pdf).read_bytes()
    if hashlib.sha256(data).hexdigest() != sha:
        raise SystemExit(f'{pdf}: SHA-256 differs from the pinned document')
    run = subprocess.run(['pdftotext', '-layout', pdf, '-'], capture_output=True, check=True)
    # One copy prints a watermark of backquotes and commas beside some rows.
    return re.sub(r'\s*--[`,-]+', '', run.stdout.decode('utf-8', 'replace'))


def sections(doc, prefix):
    """Table number -> its lines, from a title 'Table {prefix}N – ' to the
    next table title of any number."""
    out, current = {}, None
    title = re.compile(r'^\s*Table (\S+?) [–-] ')
    for line in doc.splitlines():
        m = title.match(line)
        if m:
            name = m.group(1)
            current = name[len(prefix):] if name.startswith(prefix) else None
            if current is not None and current not in out:
                out[current] = []
            continue
        if current is not None:
            out[current].append(line)
    return out


def huffman(doc, prefix):
    """Codebook table number (1 to 12) -> list of codes by index, each a
    string of '0' and '1'."""
    tables = sections(doc, prefix)
    row = re.compile(r'^\s*(\d+)\s+(\d+)\s+([0-9a-f]+)(?:\s+(\d+)\s+(\d+)\s+([0-9a-f]+))?\s*$')
    out = {}
    for number in range(1, 13):
        entries = {}
        for line in tables[str(number)]:
            m = row.match(line)
            if not m:
                continue
            groups = m.groups()
            for at in (0, 3):
                if groups[at] is None:
                    continue
                index, length, code = int(groups[at]), int(groups[at + 1]), int(groups[at + 2], 16)
                if index in entries:
                    raise SystemExit(f'{prefix}{number}: index {index} twice')
                if code >> length:
                    raise SystemExit(f'{prefix}{number}: code {code:x} longer than {length} bits')
                entries[index] = format(code, f'0{length}b')
        size = SF_ENTRIES if number == 1 else expected_size(number - 1)
        if sorted(entries) != list(range(size)):
            raise SystemExit(f'{prefix}{number}: {len(entries)} indexes, {size} expected')
        out[number] = [entries[i] for i in range(size)]
    return out


def expected_size(cb):
    unsigned, dim, lav = CODEBOOKS[cb]
    mod = lav + 1 if unsigned else 2 * lav + 1
    return mod ** dim


def check_code(codes, name):
    if sum(2.0 ** -len(c) for c in codes) != 1.0:
        raise SystemExit(f'{name}: Kraft sum is not 1')
    ordered = sorted(codes)
    for a, b in zip(ordered, ordered[1:]):
        if b.startswith(a):
            raise SystemExit(f'{name}: {a} is a prefix of {b}')


def values(cb, index):
    """The n-tuple of codeword `index` of spectrum codebook `cb`
    (14496-3 4.6.3.3)."""
    unsigned, dim, lav = CODEBOOKS[cb]
    mod, off = (lav + 1, 0) if unsigned else (2 * lav + 1, lav)
    out = []
    for power in reversed(range(dim)):
        digit = index // mod ** power
        index -= digit * mod ** power
        out.append(digit - off)
    return out


def lookup(codes, payloads):
    """Two-level lookup table over codes with the given payloads.

    Returns (first-level bits, entries). An entry is a leaf, `payload << 5
    | length` with length 1 to 31 (the whole code's length), or a pointer
    to a subtable, `offset << 10 | bits << 5`, whose `bits` next bits index
    it. The first-level width is the shortest that keeps the table within
    LEVEL1 bits and the longest code."""
    longest = max(len(c) for c in codes)
    level1 = min(LEVEL1, longest)
    first = [None] * (1 << level1)
    longer = {}
    for code, payload in zip(codes, payloads):
        if len(code) <= level1:
            pad = level1 - len(code)
            base = int(code, 2) << pad
            for i in range(1 << pad):
                first[base + i] = (payload << 5) | len(code)
        else:
            longer.setdefault(code[:level1], []).append((code[level1:], payload))
    table = first
    for prefix, rest in sorted(longer.items()):
        bits = max(len(r) for r, _ in rest)
        sub = [None] * (1 << bits)
        for r, payload in rest:
            pad = bits - len(r)
            base = int(r, 2) << pad
            for i in range(1 << pad):
                sub[base + i] = (payload << 5) | (level1 + len(r))
        offset = len(table)
        table[int(prefix, 2)] = (offset << 10) | (bits << 5)
        table.extend(sub)
    if any(e is None for e in table):
        raise SystemExit('lookup table has holes')
    if len(table) >= 1 << 22:
        raise SystemExit('lookup table too large for its offsets')
    return level1, table


def spectrum_payload(cb, index):
    """Leaf payload of a spectrum codeword: `index << 9 | escapes << 7 |
    signs << 4 | nonzero` (main() puts the vector's place in
    SPECTRUM_VECTORS in place of the index), with `nonzero` a bit per value (the first
    lowest) that is not zero in an unsigned codebook, `signs` how many sign
    bits follow the codeword (that many bits set in `nonzero`), and
    `escapes` a bit per value of 16 in codebook 11 (an escape sequence
    follows the signs)."""
    unsigned, _, _ = CODEBOOKS[cb]
    nonzero = escapes = 0
    for at, v in enumerate(values(cb, index)):
        if unsigned and v != 0:
            nonzero |= 1 << at
        if cb == 11 and v == 16:
            escapes |= 1 << at
    signs = bin(nonzero).count('1')
    return index << 9 | escapes << 7 | signs << 4 | nonzero


def spectrum_vectors(cb):
    """|v|^(4/3) of each value of each codeword of spectrum codebook `cb`,
    with the value's sign in a signed codebook, in four lanes (pairs pad
    with zeros)."""
    out = []
    for index in range(expected_size(cb)):
        lanes = []
        for v in values(cb, index):
            m = f32(abs(v) ** (4.0 / 3.0))
            lanes.append(f'-{m}' if v < 0 else m)
        lanes += ['0.0'] * (4 - len(lanes))
        out.append('[' + ', '.join(lanes) + ']')
    return out


def sign_masks():
    """Sign bits of the values of an unsigned codeword: entry `nonzero << 4
    | bits` gives each lane 0x8000_0000 where the value is negative, from
    the codeword's sign bits `bits` (one per bit set in `nonzero`, in lane
    order, the first the highest; 1 negative)."""
    out = []
    for nonzero in range(16):
        count = bin(nonzero).count('1')
        for bits in range(16):
            lanes = [0] * 4
            if bits < 1 << count:
                k = count
                for at in range(4):
                    if nonzero >> at & 1:
                        k -= 1
                        if bits >> k & 1:
                            lanes[at] = 0x8000_0000
            out.append('[' + ', '.join(f'{v:#x}' for v in lanes) + ']')
    return out


def band_rows(lines):
    """(num_swb, num_swb at 960 samples or None, offsets) from a band
    table's lines: the offsets by band and then the end, each with its
    bracketed 960-sample value ('-' for none) or None. Rows are pairs of
    band and offset; the end is a lone offset, 1,024 or 128. Parsing stops
    once every band and the end are read (the page may go on)."""
    num = num960 = end = None
    offsets = {}
    for line in lines:
        m = re.match(r'^\s*num_swb_(?:long|short)_?(?:window)?\s+(\d+)(?:\s+\((\d+)\))?', line)
        if m:
            num = int(m.group(1))
            num960 = int(m.group(2)) if m.group(2) else None
            continue
        if not line.split() or not all(re.fullmatch(r'\d+|\(\S*?\)', t) for t in line.split()):
            continue
        numbers, brackets = [], {}
        for t in line.split():
            if t.startswith('('):
                brackets[len(numbers) - 1] = t[1:-1]
            else:
                numbers.append(int(t))
        pairs = numbers
        if len(numbers) % 2:
            if numbers[-1] not in (1024, 128):
                continue  # a page number
            end = (numbers[-1], brackets.get(len(numbers) - 1))
            pairs = numbers[:-1]
        for at in range(0, len(pairs), 2):
            swb = pairs[at]
            if swb in offsets:
                raise SystemExit(f'band {swb} twice')
            offsets[swb] = (pairs[at + 1], brackets.get(at + 1))
        if num is not None and end is not None and sorted(offsets) == list(range(num)):
            break
    if num is None or sorted(offsets) != list(range(num)) or end is None:
        raise SystemExit(f'band table: num_swb {num}, bands {sorted(offsets)}, end {end}')
    table = [offsets[i] for i in range(num)] + [end]
    plain = [o for o, _ in table]
    if any(b <= a for a, b in zip(plain, plain[1:])) or plain[0] != 0:
        raise SystemExit(f'band table not increasing: {plain}')
    return num, num960, table


def bands960(num960, table, frame):
    """The offsets of a 960 (or 120) sample frame: an offset bracketed '-'
    is dropped, one bracketed with a value takes it; the last offset must
    then be the frame length, and the only one changed."""
    out = []
    for offset, bracket in table:
        if bracket == '-':
            continue
        if bracket is not None:
            out.append(int(bracket))
            break
        out.append(offset)
    if out[-1] != frame or any(b <= a for a, b in zip(out, out[1:])):
        raise SystemExit(f'960-sample band table {out}')
    if num960 is not None and len(out) - 1 != num960:
        raise SystemExit(f'960-sample band table has {len(out) - 1} bands, not {num960}')
    return out


def kbd(n, alpha):
    """Rising half of the Kaiser-Bessel derived window of length n."""
    def i0(x):
        total, term, k = 1.0, 1.0, 1
        while term > 1e-30 * total:
            term *= (x / (2 * k)) ** 2
            total += term
            k += 1
        return total
    kernel = [i0(math.pi * alpha * math.sqrt(1.0 - ((p - n / 4) / (n / 4)) ** 2)) for p in range(n // 2 + 1)]
    total = sum(kernel)
    out, run = [], 0.0
    for p in range(n // 2):
        run += kernel[p]
        out.append(math.sqrt(run / total))
    return out


def sine(n):
    return [math.sin(math.pi / n * (i + 0.5)) for i in range(n // 2)]


def check_window(half, name):
    """Princen-Bradley: w(i)^2 + w(i + n/2)^2 = 1, with the falling half
    the mirror of the rising one."""
    m = len(half)
    for i in range(m):
        if abs(half[i] ** 2 + half[m - 1 - i] ** 2 - 1.0) > 1e-12:
            raise SystemExit(f'{name}: not power complementary at {i}')


def f32(x):
    r = repr(float(x))
    return r if ('e' in r or '.' in r) else r + '.0'


def rust_array(name, ty, items, per_line=8, doc=None):
    out = []
    if doc:
        out += [f'/// {line}' if line else '///' for line in doc.split('\n')]
    out.append(f'pub(crate) static {name}: [{ty}; {len(items)}] = [')
    for at in range(0, len(items), per_line):
        out.append('    ' + ' '.join(f'{v},' for v in items[at:at + per_line]))
    out.append('];')
    return '\n'.join(out) + '\n'


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--mpeg4', required=True)
    p.add_argument('--mpeg2', required=True)
    p.add_argument('--out', required=True)
    args = p.parse_args()
    mpeg4 = text(args.mpeg4, MPEG4_SHA256)
    mpeg2 = text(args.mpeg2, MPEG2_SHA256)

    codes = huffman(mpeg4, '4.A.')
    if codes != huffman(mpeg2, 'A.'):
        raise SystemExit('the Huffman codebooks of 14496-3 and 13818-7 differ')
    for number, table in codes.items():
        check_code(table, f'Table 4.A.{number}')

    out = ['// @generated by crates/coreav-aac/tables/gen_aac.py from ISO/IEC 14496-3:2009',
           '// and ISO/IEC 13818-7:2004 (docs/design/audio-decode.md, "AAC-LC"). Do not edit.',
           '#![allow(clippy::unreadable_literal, clippy::excessive_precision, clippy::approx_constant)]', '']
    out.append('/// A Huffman lookup table: the first-level width and the entries (leaf\n'
               '/// `payload << 5 | length`, or subtable `offset << 10 | bits << 5`).\n'
               'pub(crate) struct Lookup {\n    pub(crate) level1: u32,\n'
               '    pub(crate) entries: &\'static [u32],\n}\n')
    level1, table = lookup(codes[1], list(range(SF_ENTRIES)))
    out.append(rust_array('SCALEFACTOR_ENTRIES', 'u32', table, 8,
                          'Scalefactor codebook (Table 4.A.1): payload the codeword index (the\n'
                          'difference plus 60).'))
    out.append(f'pub(crate) static SCALEFACTOR: Lookup = Lookup {{ level1: {level1}, entries: &SCALEFACTOR_ENTRIES }};\n')
    # The spectrum codebooks in one table each (gen_aac.py, `spectrum_payload`):
    # first levels of 9 bits at SPECTRUM_BASE[cb - 1], subtables after them;
    # leaves index one table of value vectors.
    entries, bases, vectors, vector_bases = [], [], [], []
    for cb in range(1, 12):
        vector_bases.append(len(vectors))
        payloads = [((vector_bases[-1] + i) << 9) | (spectrum_payload(cb, i) & 511)
                    for i in range(len(codes[cb + 1]))]
        vectors += spectrum_vectors(cb)
        level1, table = lookup(codes[cb + 1], payloads)
        if level1 != LEVEL1:
            raise SystemExit(f'codebook {cb}: first level of {level1} bits')
        base = len(entries)
        bases.append(base)
        entries += [e if e & 31 else e + (base << 10) for e in table]
    if len(entries) > SPECTRUM_ENTRIES or len(vectors) > SPECTRUM_VECTORS:
        raise SystemExit('spectrum tables too large')
    entries += [0] * (SPECTRUM_ENTRIES - len(entries))
    vectors += ['[0.0, 0.0, 0.0, 0.0]'] * (SPECTRUM_VECTORS - len(vectors))
    out.append(rust_array('SPECTRUM_ENTRIES', 'u32', entries, 8,
                          'The spectrum codebooks 1 to 11 (Tables 4.A.2 to 4.A.12): from\n'
                          '`SPECTRUM_BASE[cb - 1]`, a first level of 9 bits, then the subtables;\n'
                          'leaf payload `vector << 9 | escapes << 7 | signs << 4 | nonzero`, the\n'
                          'vector in `SPECTRUM_VECTORS`, the codeword\'s index that less\n'
                          '`VECTOR_BASE[cb - 1]` (gen_aac.py, `spectrum_payload`); subtable\n'
                          'offsets are from the table\'s start. Zeros after.'))
    out.append(rust_array('SPECTRUM_BASE', 'u32', bases, 11,
                          'Where each spectrum codebook\'s first level starts in `SPECTRUM_ENTRIES`.'))
    out.append(rust_array('SPECTRUM_VECTORS', '[f32; 4]', vectors, 2,
                          '|v|^(4/3) of the values of each codeword of the spectrum codebooks,\n'
                          'with the sign in the signed ones, four lanes (pairs pad with zeros), by\n'
                          'codebook from `VECTOR_BASE[cb - 1]` then by index. Zeros after.'))
    out.append(rust_array('VECTOR_BASE', 'u32', vector_bases, 11,
                          'Where each spectrum codebook\'s vectors start in `SPECTRUM_VECTORS`.'))
    out.append(rust_array('SIGNS', '[u32; 4]', sign_masks(), 2,
                          'Sign masks of an unsigned codeword\'s values, by `nonzero << 4 | bits`\n'
                          '(gen_aac.py, `sign_masks`).'))

    m4 = sections(mpeg4, '4.')
    m2 = sections(mpeg2, '')
    long1024, short128, long960, short120 = [], [], [], []
    by_rate = {}
    for (lt, st), rates, (l2, s2) in BAND_TABLES:
        lname, sname = f'{lt:.3f}'[2:], f'{st:.3f}'[2:]
        _, l960, lrows = band_rows(m4[lname])
        _, s960, srows = band_rows(m4[sname])
        ltab, stab = [o for o, _ in lrows], [o for o, _ in srows]
        if ltab[-1] != 1024 or stab[-1] != 128:
            raise SystemExit(f'Table 4.{lname}/4.{sname}: ends {ltab[-1]}, {stab[-1]}')
        same = ([o for o, _ in band_rows(m2[str(l2)])[2]] == ltab
                and [o for o, _ in band_rows(m2[str(s2)])[2]] == stab)
        if not same:
            raise SystemExit(f'Table 4.{lname}/4.{sname} differ from 13818-7 Tables {l2}/{s2}')
        entry = (ltab, stab, bands960(l960, lrows, 960), bands960(s960, srows, 120))
        for rate in rates:
            by_rate[rate] = entry
    for rate in SAMPLE_RATES[:12]:
        ltab, stab, l960, s120 = by_rate[rate]
        long1024.append(ltab)
        short128.append(stab)
        long960.append(l960)
        short120.append(s120)

    def bands_static(name, tables, doc):
        lines = [f'/// {doc}', f'pub(crate) static {name}: [&[u16]; 12] = [']
        for t in tables:
            lines.append('    &[' + ', '.join(str(v) for v in t) + '],')
        lines.append('];\n')
        return '\n'.join(lines)

    out.append(rust_array('SAMPLE_RATES', 'u32', SAMPLE_RATES, 13,
                          'Sample rates by samplingFrequencyIndex (Table 1.18).'))
    out.append(bands_static('BANDS_LONG_1024', long1024,
                            'Scalefactor band offsets, with the end, of long windows of frames of 1,024\n'
                            '/// samples, by band table index (the index of the rate in `SAMPLE_RATES`).'))
    out.append(bands_static('BANDS_SHORT_128', short128, 'The same for the short windows of frames of 1,024 samples.'))
    out.append(bands_static('BANDS_LONG_960', long960, 'The same for long windows of frames of 960 samples.'))
    out.append(bands_static('BANDS_SHORT_120', short120, 'The same for the short windows of frames of 960 samples.'))
    out.append(rust_array('TNS_MAX_BANDS_LONG', 'u8', [TNS_MAX_BANDS[r][0] for r in SAMPLE_RATES[:12]], 12,
                          'TNS_MAX_BANDS of long windows by band table index (Table 4.157).'))
    out.append(rust_array('TNS_MAX_BANDS_SHORT', 'u8', [TNS_MAX_BANDS[r][1] for r in SAMPLE_RATES[:12]], 12,
                          'TNS_MAX_BANDS of short windows by band table index (Table 4.157).'))

    for n, name in ((2048, 'LONG_1024'), (256, 'SHORT_128'), (1920, 'LONG_960'), (240, 'SHORT_120')):
        s = sine(n)
        k = kbd(n, KBD_ALPHA[n])
        check_window(s, f'sine {n}')
        check_window(k, f'KBD {n}')
        out.append(rust_array(f'SINE_{name}', 'f32', [f32(v) for v in s], 6,
                              f'Rising half of the sine window of length {n} (4.6.11.3.2).'))
        out.append(rust_array(f'KBD_{name}', 'f32', [f32(v) for v in k], 6,
                              f'Rising half of the Kaiser-Bessel derived window of length {n}, alpha\n'
                              f'{KBD_ALPHA[n]} (4.6.11.3.2).'))

    out.append(rust_array('POW43', 'f32', [f32(q ** (4.0 / 3.0)) for q in range(MAX_QUANT + 1)], 6,
                          '|q|^(4/3) for every magnitude a quantized value can have, a pulse\n'
                          'added (4.6.1.3, 4.6.3.3).'))
    out.append(rust_array('EIGHTH_POWERS', 'f32', [f32(2.0 ** (r / 8.0)) for r in range(8)], 4,
                          '2^(r/8) for r from 0 to 7: the fractions of scalefactor gains (steps of\n'
                          '2^(1/4)) and of coupling gains (steps down to 2^(1/8), Table 4.154).'))

    for bits in (3, 4):
        iqfac = ((1 << (bits - 1)) - 0.5) / (math.pi / 2.0)
        iqfac_m = ((1 << (bits - 1)) + 0.5) / (math.pi / 2.0)
        vals = []
        for raw in range(1 << bits):
            t = raw - (1 << bits) if raw >= 1 << (bits - 1) else raw
            vals.append(f32(math.sin(t / (iqfac if t >= 0 else iqfac_m))))
        out.append(rust_array(f'TNS_COEF_{bits}', 'f32', vals, 4,
                              f'Inverse-quantized TNS coefficients of {bits}-bit resolution, by the\n'
                              'coefficient as a signed value of that width, two\'s complement\n'
                              '(4.6.9.3, tns_decode_coef()).'))

    for m in (1024, 960):
        quarter = m // 2
        tw = [(math.cos(-math.pi * (j + 0.125) / m), math.sin(-math.pi * (j + 0.125) / m)) for j in range(quarter)]
        out.append(rust_array(f'IMDCT_TWIDDLE_{m}', '[f32; 2]', [f'[{f32(c)}, {f32(s)}]' for c, s in tw], 4,
                              f'e^(-i pi (j + 1/8) / {m}): the pre- and post-twiddles of the IMDCT of\n'
                              f'{m} coefficients (docs/design/audio-decode.md, "AAC-LC").'))
        short = m // 8
        tw = [(math.cos(-math.pi * (j + 0.125) / short), math.sin(-math.pi * (j + 0.125) / short))
              for j in range(short // 2)]
        out.append(rust_array(f'IMDCT_TWIDDLE_{short}', '[f32; 2]', [f'[{f32(c)}, {f32(s)}]' for c, s in tw], 4,
                              f'The same for {short} coefficients.'))
        tw = [(math.cos(-2 * math.pi * k / quarter), math.sin(-2 * math.pi * k / quarter)) for k in range(quarter)]
        out.append(rust_array(f'FFT_ROOTS_{quarter}', '[f32; 2]', [f'[{f32(c)}, {f32(s)}]' for c, s in tw], 4,
                              f'e^(-2 pi i k / {quarter}): the roots of unity of the FFT of {quarter} points\n'
                              f'(and, every eighth, of {quarter // 8}).'))

    pathlib.Path(args.out).write_text('\n'.join(out))


if __name__ == '__main__':
    main()
