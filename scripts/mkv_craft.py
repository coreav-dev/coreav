#!/usr/bin/env python3
"""Rewrite a Matroska file into the structures ffmpeg never writes, for the
Matroska fixtures (scripts/gen-fixtures-mkv.sh).

Written from RFC 8794 (EBML) and RFC 9559 (Matroska) only. The input is an
ffmpeg-made file; the output keeps its EBML header, Info, Tracks and
Clusters, and drops SeekHead, Cues and Tags, whose positions the rewrite
would invalidate. Operations, applied to one track (--track, a TrackNumber):

  lace xiph|ebml|fixed  merge runs of --group consecutive SimpleBlocks of the
                        track in one Cluster into one laced SimpleBlock
                        (RFC 9559 10.3)
  strip                 header stripping (ContentCompAlgo 3): remove the
                        bytes every frame of the track starts with, up to
                        --bytes, and declare them in ContentCompSettings
  groups                store the track's frames in BlockGroups: a
                        ReferenceBlock for non-keyframes, BlockDuration
                        from DefaultDuration (RFC 9559 10.4)
  scale                 set TimestampScale to --timestamp-scale; block
                        values are kept, so the file plays at another speed
  live                  unknown-sized Segment and Clusters, no Duration, as a
                        live recording writes them (RFC 9559 23.2)
  pixels W H            set the track's PixelWidth and PixelHeight to W and H
                        (a header that understates the frames' size)
  duration S            set Duration to S seconds (a header that understates
                        the blocks' span)
  annexb                H.264 as Chrome writes it: no CodecPrivate, frames
                        in the byte stream format (start codes, ITU-T H.264
                        Annex B), the record's SPS and PPS before every
                        keyframe

--default-duration NS sets the track's DefaultDuration, with any operation.

Usage: mkv_craft.py OP [ARGS] INPUT OUTPUT [--track N] [--group K] [--bytes K]
                   [--timestamp-scale NS] [--default-duration NS]
Python 3 standard library only.
"""
import argparse
import struct
import sys

EBML = 0x1A45DFA3
SEGMENT = 0x18538067
SEEK_HEAD, INFO, TRACKS, CUES, TAGS, CLUSTER = 0x114D9B74, 0x1549A966, 0x1654AE6B, 0x1C53BB6B, 0x1254C367, 0x1F43B675
TIMESTAMP_SCALE, DURATION = 0x2AD7B1, 0x4489
TRACK_ENTRY, TRACK_NUMBER, FLAG_LACING, DEFAULT_DURATION = 0xAE, 0xD7, 0x9C, 0x23E383
VIDEO, PIXEL_WIDTH, PIXEL_HEIGHT = 0xE0, 0xB0, 0xBA
CODEC_PRIVATE = 0x63A2
START_CODE = b'\x00\x00\x00\x01'
CONTENT_ENCODINGS = 0x6D80
TIMESTAMP, SIMPLE_BLOCK, BLOCK_GROUP, BLOCK = 0xE7, 0xA3, 0xA0, 0xA1
BLOCK_DURATION, REFERENCE_BLOCK = 0x9B, 0xFB
VOID, CRC32 = 0xEC, 0xBF


def vint(data, at):
    """(value, length, all ones) of the VINT at `at` (RFC 8794 4)."""
    first = data[at]
    length = 1
    while length <= 8 and not first & (0x80 >> (length - 1)):
        length += 1
    if length > 8:
        raise ValueError(f'invalid VINT at {at}')
    value = first & ((0x80 >> (length - 1)) - 1)
    for octet in data[at + 1:at + length]:
        value = (value << 8) | octet
    return value, length, value == (1 << (7 * length)) - 1


def element_id(data, at):
    _, length, _ = vint(data, at)
    return int.from_bytes(data[at:at + length], 'big'), length


def children(data, start, end):
    """(id, data start, data end, element start) of each child; an
    unknown-sized child (a Cluster) runs to the next Cluster or `end`."""
    at = start
    while at < end:
        eid, id_len = element_id(data, at)
        size, size_len, unknown = vint(data, at + id_len)
        body = at + id_len + size_len
        if unknown:
            stop = body
            while stop < end:
                cid, cid_len = element_id(data, stop)
                if cid in (CLUSTER, CUES, TAGS, SEEK_HEAD, INFO, TRACKS):
                    break
                csize, csize_len, _ = vint(data, stop + cid_len)
                stop += cid_len + csize_len + csize
            yield eid, body, stop, at
            at = stop
        else:
            yield eid, body, body + size, at
            at = body + size


def encode_id(eid):
    return eid.to_bytes((eid.bit_length() + 7) // 8, 'big')


def encode_size(size, unknown=False):
    if unknown:
        return b'\x01\xff\xff\xff\xff\xff\xff\xff'
    for length in range(1, 9):
        if size < (1 << (7 * length)) - 1:
            return ((1 << (7 * length)) | size).to_bytes(length, 'big')
    raise ValueError('size too large')


def element(eid, payload, unknown=False):
    return encode_id(eid) + encode_size(len(payload), unknown) + payload


def uint(value):
    return value.to_bytes(max(1, (value.bit_length() + 7) // 8), 'big')


def sint(value):
    for length in range(1, 9):
        if -(1 << (8 * length - 1)) <= value < (1 << (8 * length - 1)):
            return value.to_bytes(length, 'big', signed=True)
    raise ValueError('integer too large')


class Block:
    def __init__(self, track, timestamp, flags, frames):
        self.track, self.timestamp, self.flags, self.frames = track, timestamp, flags, frames

    @classmethod
    def parse(cls, payload):
        track, length, _ = vint(payload, 0)
        timestamp = struct.unpack('>h', payload[length:length + 2])[0]
        flags = payload[length + 2]
        if (flags >> 1) & 3:
            raise ValueError('input is already laced')
        return cls(track, timestamp, flags, [payload[length + 3:]])

    def encode(self, lacing=None):
        track = encode_size(self.track)
        flags = self.flags & ~0x06
        body = b''.join(self.frames)
        if len(self.frames) == 1:
            return track + struct.pack('>h', self.timestamp) + bytes([flags]) + body
        count = bytes([len(self.frames) - 1])
        sizes = b''
        if lacing == 'xiph':
            flags |= 0x02
            for frame in self.frames[:-1]:
                size = len(frame)
                sizes += b'\xff' * (size // 255) + bytes([size % 255])
        elif lacing == 'ebml':
            flags |= 0x06
            previous = None
            for frame in self.frames[:-1]:
                if previous is None:
                    sizes += encode_size(len(frame))
                else:
                    delta = len(frame) - previous
                    length = 1
                    while not -(1 << (7 * length - 1)) + 1 <= delta <= (1 << (7 * length - 1)):
                        length += 1
                    raw = delta + (1 << (7 * length - 1)) - 1
                    sizes += ((1 << (7 * length)) | raw).to_bytes(length, 'big')
                previous = len(frame)
        elif lacing == 'fixed':
            flags |= 0x04
            if len({len(f) for f in self.frames}) != 1:
                raise ValueError('fixed lacing needs equal frame sizes')
        else:
            raise ValueError(f'unknown lacing {lacing}')
        return track + struct.pack('>h', self.timestamp) + bytes([flags]) + count + sizes + body


def avcc_parameter_sets(record):
    """(NAL unit length size, SPS and PPS NAL units) of an avcC record
    (ISO/IEC 14496-15 5.3.3.1)."""
    length_size = (record[4] & 3) + 1
    units, at = [], 5
    for count_mask in (0x1F, 0xFF):
        count = record[at] & count_mask
        at += 1
        for _ in range(count):
            size = int.from_bytes(record[at:at + 2], 'big')
            units.append(record[at + 2:at + 2 + size])
            at += 2 + size
    return length_size, units


def to_annex_b(frame, length_size, prefix=b''):
    """A length-prefixed frame as a byte stream, after `prefix`."""
    out, at = bytearray(prefix), 0
    while at + length_size <= len(frame):
        size = int.from_bytes(frame[at:at + length_size], 'big')
        out += START_CODE + frame[at + length_size:at + length_size + size]
        at += length_size + size
    return bytes(out)


def rewrite_track_entry(payload, number, op, args, prefix):
    out = b''
    is_target = False
    for eid, start, end, _ in children(payload, 0, len(payload)):
        if eid == TRACK_NUMBER and int.from_bytes(payload[start:end], 'big') == number:
            is_target = True
    default_duration = None
    for eid, start, end, _ in children(payload, 0, len(payload)):
        value = payload[start:end]
        if eid in (VOID, CRC32):
            continue
        if is_target and op == 'lace' and eid == FLAG_LACING:
            value = uint(1)
        if is_target and args.default_duration and eid == DEFAULT_DURATION:
            continue
        if eid == DEFAULT_DURATION:
            default_duration = int.from_bytes(value, 'big')
        if is_target and op == 'annexb' and eid == CODEC_PRIVATE:
            args.record = value
            continue
        if is_target and op == 'pixels' and eid == VIDEO:
            video = b''
            for vid, vstart, vend, _ in children(value, 0, len(value)):
                part = value[vstart:vend]
                if vid == PIXEL_WIDTH:
                    part = uint(args.pixels[0])
                elif vid == PIXEL_HEIGHT:
                    part = uint(args.pixels[1])
                video += element(vid, part)
            value = video
        out += element(eid, value)
    if is_target and args.default_duration:
        out += element(DEFAULT_DURATION, uint(args.default_duration))
        default_duration = args.default_duration
    if is_target and op == 'strip':
        # ContentEncoding: order 0, scope 1 (frames), type 0 (compression),
        # ContentCompression: algorithm 3, settings = the stripped bytes.
        compression = element(0x4254, uint(3)) + element(0x4255, prefix)
        encoding = (element(0x5031, uint(0)) + element(0x5032, uint(1)) + element(0x5033, uint(0))
                    + element(0x5034, compression))
        out += element(CONTENT_ENCODINGS, element(0x6240, encoding))
    return out, (default_duration if is_target else None)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('op', choices=['lace', 'strip', 'groups', 'scale', 'live', 'pixels',
                                       'duration', 'annexb'])
    parser.add_argument('rest', nargs='+')
    parser.add_argument('--track', type=int, default=1)
    parser.add_argument('--group', type=int, default=4)
    parser.add_argument('--bytes', type=int, default=4)
    parser.add_argument('--timestamp-scale', type=int, default=500000)
    parser.add_argument('--default-duration', type=int)
    args = parser.parse_intermixed_args()
    if args.op == 'lace':
        lacing, src, dst = args.rest
    elif args.op == 'pixels':
        width, height, src, dst = args.rest
        args.pixels, lacing = (int(width), int(height)), None
    elif args.op == 'duration':
        seconds, src, dst = args.rest
        args.seconds, lacing = float(seconds), None
    else:
        lacing = None
        src, dst = args.rest
    data = open(src, 'rb').read()
    (header_id, header_start, header_end, _), = [next(children(data, 0, len(data)))]
    if header_id != EBML:
        raise SystemExit('not an EBML file')
    seg_id, seg_start, seg_end, _ = next(children(data, header_end, len(data)))
    if seg_id != SEGMENT:
        raise SystemExit('no Segment')
    top = list(children(data, seg_start, seg_end))

    clusters = []
    for eid, start, end, _ in top:
        if eid != CLUSTER:
            continue
        items = []
        for cid, cstart, cend, _ in children(data, start, end):
            payload = data[cstart:cend]
            if cid == SIMPLE_BLOCK:
                items.append(('block', Block.parse(payload), None))
            elif cid == BLOCK_GROUP:
                group = {g: data[gs:ge] for g, gs, ge, _ in children(data, cstart, cend)}
                block = Block.parse(group[BLOCK])
                if REFERENCE_BLOCK not in group:
                    # A BlockGroup without ReferenceBlock is a keyframe (RFC
                    # 9559 10.4); keep that when it becomes a SimpleBlock.
                    block.flags |= 0x80
                items.append(('block', block, group))
            elif cid not in (VOID, CRC32):
                items.append(('raw', cid, payload))
        clusters.append(items)

    prefix = b''
    if args.op == 'strip':
        frames = [b.frames[0] for c in clusters for kind, b, _ in c
                  if kind == 'block' and b.track == args.track]
        prefix = frames[0][:args.bytes]
        while prefix and not all(f.startswith(prefix) for f in frames):
            prefix = prefix[:-1]
        if not prefix:
            raise SystemExit('the frames share no leading bytes')

    out_top = b''
    default_duration = None
    for eid, start, end, _ in top:
        payload = data[start:end]
        if eid == INFO:
            info = b''
            for iid, istart, iend, _ in children(data, start, end):
                value = data[istart:iend]
                if iid in (VOID, CRC32) or (args.op == 'live' and iid == DURATION):
                    continue
                if iid == TIMESTAMP_SCALE and args.op == 'scale':
                    value = uint(args.timestamp_scale)
                if iid == DURATION and args.op == 'duration':
                    scale = next((int.from_bytes(data[a:b], 'big')
                                  for i, a, b, _ in children(data, start, end)
                                  if i == TIMESTAMP_SCALE), 1_000_000)
                    value = struct.pack('>d', args.seconds * 1e9 / scale)
                info += element(iid, value)
            out_top += element(INFO, info)
        elif eid == TRACKS:
            tracks = b''
            for tid, tstart, tend, _ in children(data, start, end):
                if tid != TRACK_ENTRY:
                    continue
                entry, duration = rewrite_track_entry(data[tstart:tend], args.track, args.op, args, prefix)
                default_duration = default_duration or duration
                tracks += element(TRACK_ENTRY, entry)
            out_top += element(TRACKS, tracks)
    # TimestampScale applies to block values; DefaultDuration is in ns.
    scale = 1_000_000
    for eid, start, end, _ in top:
        if eid == INFO:
            for iid, istart, iend, _ in children(data, start, end):
                if iid == TIMESTAMP_SCALE:
                    scale = int.from_bytes(data[istart:iend], 'big')

    for items in clusters:
        run = []

        def flush():
            if run:
                merged = Block(run[0].track, run[0].timestamp,
                               0x80 if all(b.flags & 0x80 for b in run) else 0,
                               [b.frames[0] for b in run])
                nonlocal_body.append(element(SIMPLE_BLOCK, merged.encode(lacing)))
                run.clear()

        nonlocal_body = []
        for kind, value, group in items:
            if kind == 'raw':
                flush()
                nonlocal_body.append(element(value, group))
                continue
            block = value
            if block.track != args.track:
                flush()
                if group is None:
                    nonlocal_body.append(element(SIMPLE_BLOCK, block.encode()))
                else:
                    block.flags &= ~0x80
                    parts = b''.join(element(g, v) for g, v in group.items() if g != BLOCK)
                    nonlocal_body.append(element(BLOCK_GROUP, element(BLOCK, block.encode()) + parts))
                continue
            if args.op == 'lace':
                run.append(block)
                if len(run) == args.group:
                    flush()
            elif args.op == 'strip':
                block.frames = [block.frames[0][len(prefix):]]
                nonlocal_body.append(element(SIMPLE_BLOCK, block.encode()))
            elif args.op == 'annexb':
                length_size, units = avcc_parameter_sets(args.record)
                sets = b''.join(START_CODE + u for u in units) if block.flags & 0x80 else b''
                block.frames = [to_annex_b(block.frames[0], length_size, sets)]
                nonlocal_body.append(element(SIMPLE_BLOCK, block.encode()))
            elif args.op == 'groups':
                keyframe = bool(block.flags & 0x80)
                block.flags &= ~0x80
                parts = element(BLOCK, block.encode())
                if not keyframe:
                    parts += element(REFERENCE_BLOCK, sint(-1))
                if default_duration:
                    parts += element(BLOCK_DURATION, uint(round(default_duration / scale)))
                nonlocal_body.append(element(BLOCK_GROUP, parts))
            else:
                nonlocal_body.append(element(SIMPLE_BLOCK, block.encode()))
        flush()
        body = b''.join(nonlocal_body)
        out_top += element(CLUSTER, body, unknown=args.op == 'live')

    header = data[0:header_end]
    segment = element(SEGMENT, out_top, unknown=args.op == 'live')
    with open(dst, 'wb') as out:
        out.write(header + segment)


if __name__ == '__main__':
    sys.exit(main())
