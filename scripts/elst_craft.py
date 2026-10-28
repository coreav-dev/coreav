#!/usr/bin/env python3
"""Replace the edit list (`edts`/`elst`, ISO/IEC 14496-12 8.6.5 and 8.6.6)
of one track, for the edit list fixtures (scripts/gen-fixtures.sh,
docs/design/edit-lists.md).

Written from ISO/IEC 14496-12 only. The input must have its `moov` box
after `mdat` (ffmpeg's default), so that resizing `moov` moves no sample;
the script refuses one that does not.

Usage: elst_craft.py INPUT OUTPUT TRACK [EDIT ...]
TRACK is the track's position in the file (0 for the first `trak`). Each
EDIT is DURATION:MEDIA_TIME[:RATE], the duration in movie ticks, the media
time in media ticks (-1 for an empty edit) and the rate as a decimal (1 by
default; 0 for a dwell). No EDIT removes the edit list. Version 1 entries
are written when a value needs 64 bits.
Python 3 standard library only.
"""
import struct
import sys


def boxes(data, start, end):
    while start + 8 <= end:
        size = struct.unpack('>I', data[start:start + 4])[0]
        if size < 8 or start + size > end:
            raise SystemExit('unsupported box layout (64-bit or truncated sizes)')
        yield data[start + 4:start + 8], start, start + size
        start += size


def box(kind, body):
    return struct.pack('>I', 8 + len(body)) + kind + body


def elst(edits):
    wide = any(d > 0xFFFFFFFF or not -0x80000000 <= t <= 0x7FFFFFFF for d, t, _ in edits)
    body = struct.pack('>BxxxI', 1 if wide else 0, len(edits))
    for duration, media_time, rate in edits:
        whole = int(rate)
        fraction = round((rate - whole) * 0x10000)
        body += struct.pack('>Qq' if wide else '>Ii', duration, media_time) + struct.pack('>hH', whole, fraction)
    return box(b'edts', box(b'elst', body))


def rewrite_trak(data, begin, stop, edits):
    children = [(k, s, e) for k, s, e in boxes(data, begin + 8, stop) if k != b'edts']
    out = b''
    for kind, s, e in children:
        out += data[s:e]
        # The edit box follows the track header (8.3.1).
        if kind == b'tkhd' and edits:
            out += elst(edits)
    return box(b'trak', out)


def parse(edit):
    parts = edit.split(':')
    rate = float(parts[2]) if len(parts) > 2 else 1.0
    return int(parts[0]), int(parts[1]), rate


def main():
    source, target, track, *values = sys.argv[1:]
    edits = [parse(v) for v in values]
    data = open(source, 'rb').read()
    top = list(boxes(data, 0, len(data)))
    kinds = [k for k, _, _ in top]
    if b'mdat' in kinds and kinds.index(b'moov') < kinds.index(b'mdat'):
        raise SystemExit('moov before mdat: resizing it would move the samples')
    out = b''
    for kind, begin, stop in top:
        if kind != b'moov':
            out += data[begin:stop]
            continue
        body, position = b'', 0
        for child, s, e in boxes(data, begin + 8, stop):
            if child == b'trak':
                body += rewrite_trak(data, s, e, edits) if position == int(track) else data[s:e]
                position += 1
            else:
                body += data[s:e]
        if position <= int(track):
            raise SystemExit(f'no track at position {track}')
        out += box(b'moov', body)
    open(target, 'wb').write(out)


if __name__ == '__main__':
    main()
