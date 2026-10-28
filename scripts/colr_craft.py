#!/usr/bin/env python3
"""Set or remove the colour information box (`colr`, ISO/IEC 14496-12
12.1.5) of the first visual sample entry of each track, for the colour
precedence fixtures (scripts/gen-fixtures.sh, docs/design/probe-color.md).

Written from ISO/IEC 14496-12 only. The input must have its `moov` box
after `mdat` (ffmpeg's default), so that resizing `moov` moves no sample;
the script refuses one that does not.

Usage: colr_craft.py INPUT OUTPUT none
       colr_craft.py INPUT OUTPUT nclx PRIMARIES TRANSFER MATRIX FULL_RANGE
       colr_craft.py INPUT OUTPUT nclc PRIMARIES TRANSFER MATRIX
Python 3 standard library only.
"""
import struct
import sys

VISUAL = {b'avc1', b'avc3', b'hvc1', b'hev1', b'av01', b'vp09'}
# Fields of a visual sample entry before its child boxes (12.1.3).
VISUAL_FIELDS = 78
PATH = (b'trak', b'mdia', b'minf', b'stbl', b'stsd')


def boxes(data, start, end):
    while start + 8 <= end:
        size = struct.unpack('>I', data[start:start + 4])[0]
        if size < 8 or start + size > end:
            raise SystemExit('unsupported box layout (64-bit or truncated sizes)')
        yield data[start + 4:start + 8], start, start + size
        start += size


def box(kind, body):
    return struct.pack('>I', 8 + len(body)) + kind + body


def rewrite(data, start, end, path, colr):
    out = b''
    for kind, begin, stop in boxes(data, start, end):
        if path and kind == path[0]:
            # stsd is a full box with an entry count before its entries.
            head = 8 if kind == b'stsd' else 0
            out += box(kind, data[begin + 8:begin + 8 + head]
                       + rewrite(data, begin + 8 + head, stop, path[1:], colr))
        elif not path and kind in VISUAL:
            fields = begin + 8 + VISUAL_FIELDS
            children = b''.join(data[s:e] for k, s, e in boxes(data, fields, stop) if k != b'colr')
            out += box(kind, data[begin + 8:fields] + children + colr)
        else:
            out += data[begin:stop]
    return out


def main():
    source, target, kind, *values = sys.argv[1:]
    if kind == 'none':
        colr = b''
    else:
        payload = kind.encode() + struct.pack('>HHH', *(int(v) for v in values[:3]))
        if kind == 'nclx':
            payload += bytes([0x80 if values[3] == '1' else 0])
        colr = box(b'colr', payload)
    data = open(source, 'rb').read()
    top = list(boxes(data, 0, len(data)))
    kinds = [k for k, _, _ in top]
    if b'mdat' in kinds and kinds.index(b'moov') < kinds.index(b'mdat'):
        raise SystemExit('moov before mdat: resizing it would move the samples')
    out = b''
    for kind_, begin, stop in top:
        if kind_ == b'moov':
            out += box(b'moov', rewrite(data, begin + 8, stop, PATH, colr))
        else:
            out += data[begin:stop]
    open(target, 'wb').write(out)


if __name__ == '__main__':
    main()
