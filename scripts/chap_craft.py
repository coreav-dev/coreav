#!/usr/bin/env python3
"""Give one track a `chap` track reference (`tref`, ISO/IEC 14496-12 8.3.3;
chapter lists in the Apple QuickTime File Format), for the chapter
fixtures (scripts/gen-fixtures.sh). Any track reference box the track
already has is replaced.

Written from ISO/IEC 14496-12 only. The input must have its `moov` box
after `mdat` (ffmpeg's default), so that resizing `moov` moves no sample;
the script refuses one that does not.

Usage: chap_craft.py INPUT OUTPUT TRACK ID...
TRACK is the position of the track that holds the reference (0 for the
first `trak`); each ID is a `track_ID` it names.
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


def rewrite_trak(data, begin, stop, ids):
    out = b''
    kinds = [k for k, _, _ in boxes(data, begin + 8, stop)]
    if b'tkhd' not in kinds:
        raise SystemExit('the track has no tkhd box')
    for kind, s, e in boxes(data, begin + 8, stop):
        if kind == b'tref':
            continue
        out += data[s:e]
        # After the track header, as ffmpeg writes it.
        if kind == b'tkhd':
            out += box(b'tref', box(b'chap', b''.join(struct.pack('>I', i) for i in ids)))
    return box(b'trak', out)


def main():
    source, target, track, *values = sys.argv[1:]
    ids = [int(v) for v in values]
    if not ids:
        raise SystemExit('name at least one track ID')
    data = open(source, 'rb').read()
    top = list(boxes(data, 0, len(data)))
    kinds = [k for k, _, _ in top]
    if kinds.count(b'moov') != 1:
        raise SystemExit('not one moov box')
    if b'mdat' in kinds[kinds.index(b'moov'):]:
        raise SystemExit('an mdat after moov: resizing moov would move its samples')
    if int(track) < 0:
        raise SystemExit('TRACK must be 0 or more')
    out = b''
    for kind, begin, stop in top:
        if kind != b'moov':
            out += data[begin:stop]
            continue
        body, position = b'', 0
        for child, s, e in boxes(data, begin + 8, stop):
            if child == b'trak':
                body += rewrite_trak(data, s, e, ids) if position == int(track) else data[s:e]
                position += 1
            else:
                body += data[s:e]
        if position <= int(track):
            raise SystemExit(f'no track at position {track}')
        out += box(b'moov', body)
    open(target, 'wb').write(out)


if __name__ == '__main__':
    main()
