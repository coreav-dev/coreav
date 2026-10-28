#!/usr/bin/env python3
"""Remove or fill the sync sample table (`stss`, ISO/IEC 14496-12 8.6.2) of
each video track, for the cut point fixtures (scripts/gen-fixtures.sh,
docs/design/cut-points.md).

Written from ISO/IEC 14496-12 only. `none` turns the box into a `free` box
of the same size: without it every sample is a sync sample. `all` makes it
list every sample (`stsz` count; a track with compact sample sizes,
`stz2`, is refused). The input must have its `moov` box after
`mdat` (ffmpeg's default), so that resizing `moov` moves no sample; the
script refuses one that does not.

Usage: stss_craft.py INPUT OUTPUT none|all
Python 3 standard library only.
"""
import struct
import sys

PATH = (b'trak', b'mdia', b'minf', b'stbl')


def boxes(data, start, end):
    while start + 8 <= end:
        size = struct.unpack('>I', data[start:start + 4])[0]
        if size < 8 or start + size > end:
            raise SystemExit('unsupported box layout (64-bit or truncated sizes)')
        yield data[start + 4:start + 8], start, start + size
        start += size


def box(kind, body):
    return struct.pack('>I', 8 + len(body)) + kind + body


def handler(data, trak_start, trak_end):
    for kind, begin, stop in boxes(data, trak_start + 8, trak_end):
        if kind == b'mdia':
            for k, b, e in boxes(data, begin + 8, stop):
                if k == b'hdlr':
                    return data[b + 16:b + 20]
    return None


def stbl(data, start, end, mode):
    out, count, found = b'', None, False
    for kind, begin, stop in boxes(data, start, end):
        if kind == b'stsz':
            count = struct.unpack('>I', data[begin + 16:begin + 20])[0]
    if count is None:
        raise SystemExit('no sample size box (stsz); compact sample sizes (stz2) are not supported')
    for kind, begin, stop in boxes(data, start, end):
        if kind == b'stss':
            found = True
            if mode == 'none':
                out += box(b'free', data[begin + 8:stop])
                continue
            out += box(b'stss', bytes(4) + struct.pack('>I', count) + b''.join(
                struct.pack('>I', n) for n in range(1, count + 1)))
            continue
        out += data[begin:stop]
    if not found and mode == 'all':
        out += box(b'stss', bytes(4) + struct.pack('>I', count) + b''.join(
            struct.pack('>I', n) for n in range(1, count + 1)))
    return out


def rewrite(data, start, end, path, mode):
    out = b''
    for kind, begin, stop in boxes(data, start, end):
        if path and kind == path[0]:
            if kind == b'trak' and handler(data, begin, stop) != b'vide':
                out += data[begin:stop]
                continue
            inner = (stbl(data, begin + 8, stop, mode) if len(path) == 1
                     else rewrite(data, begin + 8, stop, path[1:], mode))
            out += box(kind, inner)
        else:
            out += data[begin:stop]
    return out


def main():
    if len(sys.argv) != 4 or sys.argv[3] not in ('none', 'all'):
        raise SystemExit(__doc__)
    data = open(sys.argv[1], 'rb').read()
    tops = list(boxes(data, 0, len(data)))
    kinds = [k for k, _, _ in tops]
    if b'moov' not in kinds or b'mdat' not in kinds or kinds.index(b'moov') < kinds.index(b'mdat'):
        raise SystemExit('the movie box must follow the media data box')
    out = b''
    for kind, begin, stop in tops:
        if kind == b'moov':
            out += box(b'moov', rewrite(data, begin + 8, stop, PATH, sys.argv[3]))
        else:
            out += data[begin:stop]
    open(sys.argv[2], 'wb').write(out)


if __name__ == '__main__':
    main()
