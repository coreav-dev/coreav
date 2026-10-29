#!/usr/bin/env python3
"""Patch fields of an ISO base media file in place, for fixtures whose
values the pinned ffmpeg does not write (scripts/gen-fixtures.sh,
docs/design/compat-cli.md).

Written from ISO/IEC 14496-12 and ISO/IEC 14496-1 only. Every patch keeps
each box's size, so no sample moves.

Usage:
  iso_patch.py INPUT OUTPUT tfdt SECONDS
      Add SECONDS (a whole number) to every track fragment's decode time
      (`tfdt`, 8.8.12) and to the time of every fragment random access
      entry (`tfra`, 8.8.10), in each track's media timescale (`mdhd`).
  iso_patch.py INPUT OUTPUT average TRACK BITS
      Set the average bit rate of the sample entries of the track at
      position TRACK (0 for the first `trak`): the `btrt` box's avgBitrate
      (8.5.2.2) and the `esds` decoder configuration's avgBitrate
      (14496-1 7.2.6.6).
  iso_patch.py INPUT OUTPUT free TRACK TYPE
      Turn every TYPE box among the children of the track's sample entries
      into a `free` box (8.1.2), as if the entry had none: `pasp`, for one.
  iso_patch.py INPUT OUTPUT matrix TRACK INDEX VALUE
      Set element INDEX (0 to 8) of the track header's matrix (`tkhd`,
      8.3.2) to VALUE.
  iso_patch.py INPUT OUTPUT channels TRACK COUNT
      Set the channelcount of the track's version 0 audio sample entries
      (12.2.3).
  iso_patch.py INPUT OUTPUT movie-matrix INDEX VALUE
      Set element INDEX of the movie header's matrix (`mvhd`, 8.2.2).
  iso_patch.py INPUT OUTPUT size TRACK WIDTH HEIGHT
      Set the track header's width and height (`tkhd`, 8.3.2), in whole
      pixels.
  iso_patch.py INPUT OUTPUT chap TRACK ID...
      Set the track IDs of the track's `chap` reference (`tref`, 8.3.3),
      as many as it holds.
  iso_patch.py INPUT OUTPUT unref TRACK
      Turn the track's `tref` box into a `free` box.
  iso_patch.py INPUT OUTPUT amve TRACK LUX X Y
      Turn the `pasp` box of the track's video sample entries, the same
      size, into an ambient viewing environment box (`amve`, ISO/IEC
      23008-2 D.2.39): illuminance LUX in 1/10000 cd/m2, light x and y
      in 1/50000.
  iso_patch.py INPUT OUTPUT cover-type TYPE
      Set the type of the cover picture's `data` atom (`covr` in the
      item list of `moov/udta/meta`, QuickTime well-known types).
Python 3 standard library only.
"""
import struct
import sys

# Sample entry header: 6 reserved bytes and a data reference index, then
# the audio (8.5.2.2, 20 bytes) or visual (8.5.2.2, 70 bytes) fields.
ENTRY_FIELDS = {b'soun': 8 + 20, b'vide': 8 + 70}


def boxes(data, start, end):
    while start + 8 <= end:
        size = struct.unpack('>I', data[start:start + 4])[0]
        if size < 8 or start + size > end:
            raise SystemExit('unsupported box layout (64-bit or truncated sizes)')
        yield data[start + 4:start + 8], start, start + size
        start += size


def find(data, start, end, path):
    """Every box at PATH (a list of types) under [start, end)."""
    for kind, s, e in boxes(data, start, end):
        if kind != path[0]:
            continue
        if len(path) == 1:
            yield s, e
        else:
            yield from find(data, s + 8, e, path[1:])


def timescales(data):
    out = {}
    for s, e in find(data, 0, len(data), [b'moov', b'trak']):
        (tkhd, _), = find(data, s + 8, e, [b'tkhd'])
        at = tkhd + 12 + (16 if data[tkhd + 8] == 1 else 8)
        track_id = struct.unpack('>I', data[at:at + 4])[0]
        (mdhd, _), = find(data, s + 8, e, [b'mdia', b'mdhd'])
        at = mdhd + 12 + (16 if data[mdhd + 8] == 1 else 8)
        out[track_id] = struct.unpack('>I', data[at:at + 4])[0]
    return out


def add(data, at, wide, delta):
    fmt = '>Q' if wide else '>I'
    size = 8 if wide else 4
    value = struct.unpack(fmt, data[at:at + size])[0] + delta
    if value >= 1 << (8 * size):
        raise SystemExit('the shifted time does not fit the field')
    data[at:at + size] = struct.pack(fmt, value)


def shift_tfdt(data, seconds):
    scale = timescales(data)
    for s, e in find(data, 0, len(data), [b'moof', b'traf']):
        (tfhd, _), = find(data, s + 8, e, [b'tfhd'])
        track_id = struct.unpack('>I', data[tfhd + 12:tfhd + 16])[0]
        tfdt = list(find(data, s + 8, e, [b'tfdt']))
        if not tfdt:
            raise SystemExit(f'a fragment of track {track_id} has no tfdt')
        (t, _), = tfdt
        add(data, t + 12, data[t + 8] == 1, seconds * scale[track_id])
    for s, e in find(data, 0, len(data), [b'mfra', b'tfra']):
        wide = data[s + 8] == 1
        track_id, sizes, count = struct.unpack('>III', data[s + 12:s + 24])
        # traf, trun and sample number fields of 1 to 4 bytes each.
        numbers = sum(((sizes >> shift) & 3) + 1 for shift in (4, 2, 0))
        at = s + 24
        if at + count * ((16 if wide else 8) + numbers) > e:
            raise SystemExit('tfra entries run past the box')
        for _ in range(count):
            add(data, at, wide, seconds * scale[track_id])
            at += (16 if wide else 8) + numbers


def set_average(data, track, bits):
    traks = list(find(data, 0, len(data), [b'moov', b'trak']))
    if track >= len(traks):
        raise SystemExit(f'no track at position {track}')
    s, e = traks[track]
    (hdlr, _), = find(data, s + 8, e, [b'mdia', b'hdlr'])
    handler = bytes(data[hdlr + 16:hdlr + 20])
    if handler not in ENTRY_FIELDS:
        raise SystemExit('only audio and video tracks have these fields')
    (stsd, stsd_end), = find(data, s + 8, e, [b'mdia', b'minf', b'stbl', b'stsd'])
    patched = 0
    for _, es, ee in boxes(data, stsd + 16, stsd_end):
        start = es + 8 + ENTRY_FIELDS[handler]
        if handler == b'soun' and struct.unpack('>H', data[es + 16:es + 18])[0] != 0:
            raise SystemExit('only version 0 audio sample entries')
        for kind, cs, ce in boxes(data, start, ee):
            if kind == b'btrt':
                data[cs + 16:cs + 20] = struct.pack('>I', bits)
                patched += 1
            elif kind == b'esds':
                patched += patch_esds(data, cs + 12, ce, bits)
    if not patched:
        raise SystemExit('no btrt or esds in the track')


def track_box(data, track):
    traks = list(find(data, 0, len(data), [b'moov', b'trak']))
    if track >= len(traks):
        raise SystemExit(f'no track at position {track}')
    return traks[track]


def entries(data, track):
    # (handler, entry start, entry end) of each sample entry of TRACK.
    s, e = track_box(data, track)
    (hdlr, _), = find(data, s + 8, e, [b'mdia', b'hdlr'])
    handler = bytes(data[hdlr + 16:hdlr + 20])
    (stsd, stsd_end), = find(data, s + 8, e, [b'mdia', b'minf', b'stbl', b'stsd'])
    return [(handler, es, ee) for _, es, ee in boxes(data, stsd + 16, stsd_end)]


def free_boxes(data, track, kind):
    freed = 0
    for handler, es, ee in entries(data, track):
        if handler not in ENTRY_FIELDS:
            raise SystemExit('only audio and video tracks have these fields')
        for child, cs, _ in boxes(data, es + 8 + ENTRY_FIELDS[handler], ee):
            if child == kind:
                data[cs + 4:cs + 8] = b'free'
                freed += 1
    if not freed:
        raise SystemExit(f'no {kind.decode()} box in the track')


def set_matrix(data, track, index, value):
    s, e = track_box(data, track)
    (tkhd, _), = find(data, s + 8, e, [b'tkhd'])
    at = tkhd + (60 if data[tkhd + 8] == 1 else 48) + 4 * index
    data[at:at + 4] = struct.pack('>i', value)


def set_channels(data, track, count):
    for handler, es, _ in entries(data, track):
        if handler != b'soun' or struct.unpack('>H', data[es + 16:es + 18])[0] != 0:
            raise SystemExit('only version 0 audio sample entries')
        data[es + 24:es + 26] = struct.pack('>H', count)


def set_movie_matrix(data, index, value):
    (mvhd, _), = find(data, 0, len(data), [b'moov', b'mvhd'])
    at = mvhd + (56 if data[mvhd + 8] == 1 else 44) + 4 * index
    data[at:at + 4] = struct.pack('>i', value)


def set_size(data, track, width, height):
    s, e = track_box(data, track)
    (tkhd, _), = find(data, s + 8, e, [b'tkhd'])
    at = tkhd + (60 if data[tkhd + 8] == 1 else 48) + 36
    data[at:at + 8] = struct.pack('>II', width << 16, height << 16)


def set_chap(data, track, ids):
    s, e = track_box(data, track)
    (chap, chap_end), = find(data, s + 8, e, [b'tref', b'chap'])
    if (chap_end - chap - 8) // 4 != len(ids):
        raise SystemExit('give as many IDs as the reference holds')
    data[chap + 8:chap_end] = b''.join(struct.pack('>I', i) for i in ids)


def unref(data, track):
    s, e = track_box(data, track)
    (tref, _), = find(data, s + 8, e, [b'tref'])
    data[tref + 4:tref + 8] = b'free'


def set_ambient(data, track, lux, x, y):
    found = 0
    for handler, es, ee in entries(data, track):
        if handler != b'vide':
            raise SystemExit('only video tracks have an ambient viewing environment')
        for child, cs, ce in boxes(data, es + 8 + ENTRY_FIELDS[handler], ee):
            if child == b'pasp' and ce - cs == 16:
                data[cs + 4:ce] = b'amve' + struct.pack('>IHH', lux, x, y)
                found += 1
    if not found:
        raise SystemExit('no pasp box in the track')


def set_cover_type(data, kind):
    (meta, meta_end), = find(data, 0, len(data), [b'moov', b'udta', b'meta'])
    # `meta` is a full box: its children follow version and flags.
    (cover, _), = find(data, meta + 12, meta_end, [b'ilst', b'covr', b'data'])
    data[cover + 8:cover + 12] = struct.pack('>I', kind)


def descriptor(data, at, end):
    """Tag, body start and body end of the descriptor at AT (14496-1 8.3.3)."""
    tag, size, at = data[at], 0, at + 1
    for _ in range(4):
        byte = data[at]
        at += 1
        size = size << 7 | byte & 0x7F
        if not byte & 0x80:
            break
    if at + size > end:
        raise SystemExit('truncated descriptor')
    return tag, at, at + size


def patch_esds(data, at, end, bits):
    tag, body, body_end = descriptor(data, at, end)
    if tag != 0x03:
        raise SystemExit('esds without an ES descriptor')
    flags = data[body + 2]
    at = body + 3
    if flags & 0x80:
        at += 2
    if flags & 0x40:
        at += 1 + data[at]
    if flags & 0x20:
        at += 2
    tag, config, _ = descriptor(data, at, body_end)
    if tag != 0x04:
        raise SystemExit('ES descriptor without a decoder configuration')
    # objectTypeIndication, streamType and bufferSizeDB, maxBitrate, avgBitrate.
    if config + 13 > body_end:
        raise SystemExit('decoder configuration too short')
    data[config + 9:config + 13] = struct.pack('>I', bits)
    return 1


def main():
    source, target, operation, *values = sys.argv[1:]
    data = bytearray(open(source, 'rb').read())
    if operation == 'tfdt':
        shift_tfdt(data, int(values[0]))
    elif operation == 'average':
        set_average(data, int(values[0]), int(values[1]))
    elif operation == 'free':
        free_boxes(data, int(values[0]), values[1].encode())
    elif operation == 'matrix':
        set_matrix(data, int(values[0]), int(values[1]), int(values[2]))
    elif operation == 'channels':
        set_channels(data, int(values[0]), int(values[1]))
    elif operation == 'movie-matrix':
        set_movie_matrix(data, int(values[0]), int(values[1]))
    elif operation == 'size':
        set_size(data, int(values[0]), int(values[1]), int(values[2]))
    elif operation == 'chap':
        set_chap(data, int(values[0]), [int(v) for v in values[1:]])
    elif operation == 'unref':
        unref(data, int(values[0]))
    elif operation == 'amve':
        set_ambient(data, int(values[0]), int(values[1]), int(values[2]), int(values[3]))
    elif operation == 'cover-type':
        set_cover_type(data, int(values[0]))
    else:
        raise SystemExit(f'unknown operation {operation}')
    open(target, 'wb').write(data)


if __name__ == '__main__':
    main()
