#!/usr/bin/env python3
"""Join the video samples of MP4 files into one track with a sample
description per source (ISO/IEC 14496-12 8.5.2), switched by the sample
to chunk box (8.7.4), for the tests of a track whose samples change
description (docs/design/decode.md, "Several sample descriptions").

Written from ISO/IEC 14496-12 only. Each source holds one video track;
its sample entry is copied as stored, and the samples of each source
become one chunk that uses that entry. A source named again reuses its
first entry. Every source has the first one's media timescale. The output
has no edit list.

Usage:
  stsd_craft.py OUTPUT SOURCE [SOURCE...]
  stsd_craft.py --fixture FFMPEG DIR

--fixture makes the committed fixture with the pinned FFmpeg
(scripts/install-oracle-ffmpeg.sh) and its libx264:
DIR/two-sample-entries.mp4, the samples of a 128x96 source, then an 80x48
one, then the first again, each starting with an IDR picture, and
DIR/two-sample-entries.framemd5, its reference decode with the flags of
V1 (docs/design/compat-cli.md). Python 3 standard library only.
"""
import os
import struct
import subprocess
import sys
import tempfile


def boxes(data, start, end):
    while start + 8 <= end:
        size = struct.unpack('>I', data[start:start + 4])[0]
        if size < 8 or start + size > end:
            raise SystemExit('unsupported box layout (64-bit or truncated sizes)')
        yield data[start + 4:start + 8], start, start + size
        start += size


def child(data, start, end, path):
    for kind, begin, stop in boxes(data, start, end):
        if kind == path[0]:
            if len(path) == 1:
                return begin, stop
            return child(data, begin + 8, stop, path[1:])
    raise SystemExit('missing box ' + b'/'.join(path).decode())


def box(kind, body):
    return struct.pack('>I', 8 + len(body)) + kind + body


def full(kind, version, flags, body):
    return box(kind, struct.pack('>I', version << 24 | flags) + body)


def u32s(data, at, count):
    return list(struct.unpack('>%dI' % count, data[at:at + 4 * count]))


def entries(data, at, width):
    """The entries of a table box at 'at', each 'width' 32-bit fields."""
    count = struct.unpack('>I', data[at + 12:at + 16])[0]
    values = u32s(data, at + 16, width * count)
    return [values[i:i + width] for i in range(0, len(values), width)]


def read_source(path):
    """The video track's sample entry, media timescale, samples (bytes,
    duration, composition offset, sync) in decode order, and entry size."""
    data = open(path, 'rb').read()
    moov = child(data, 0, len(data), [b'moov'])
    for kind, begin, stop in boxes(data, moov[0] + 8, moov[1]):
        if kind != b'trak':
            continue
        hdlr = child(data, begin + 8, stop, [b'mdia', b'hdlr'])
        if data[hdlr[0] + 16:hdlr[0] + 20] == b'vide':
            trak = (begin, stop)
            break
    else:
        raise SystemExit(path + ': no video track')
    mdhd = child(data, trak[0] + 8, trak[1], [b'mdia', b'mdhd'])
    if data[mdhd[0] + 8] != 0:
        raise SystemExit(path + ': mdhd version 1 is not supported')
    timescale = struct.unpack('>I', data[mdhd[0] + 20:mdhd[0] + 24])[0]
    stbl = child(data, trak[0] + 8, trak[1], [b'mdia', b'minf', b'stbl'])
    tables = {kind: begin for kind, begin, _ in boxes(data, stbl[0] + 8, stbl[1])}
    stsd = tables[b'stsd']
    if struct.unpack('>I', data[stsd + 12:stsd + 16])[0] != 1:
        raise SystemExit(path + ': one sample description expected')
    entry_size = struct.unpack('>I', data[stsd + 16:stsd + 20])[0]
    entry = data[stsd + 16:stsd + 16 + entry_size]
    stsz = tables[b'stsz']
    constant, count = struct.unpack('>II', data[stsz + 12:stsz + 20])
    sizes = [constant] * count if constant else u32s(data, stsz + 20, count)
    durations = [d for n, d in entries(data, tables[b'stts'], 2) for _ in range(n)]
    offsets = [0] * count
    if b'ctts' in tables:
        signed = data[tables[b'ctts'] + 8] == 1
        offsets = [struct.unpack('>i' if signed else '>I', struct.pack('>I', v))[0]
                   for n, v in entries(data, tables[b'ctts'], 2) for _ in range(n)]
    sync = set(range(1, count + 1))
    if b'stss' in tables:
        sync = {n for n, in entries(data, tables[b'stss'], 1)}
    if b'stco' not in tables:
        raise SystemExit(path + ': 64-bit chunk offsets (co64) are not supported')
    stsc = entries(data, tables[b'stsc'], 3)
    samples, number = [], 0
    for chunk, (offset,) in enumerate(entries(data, tables[b'stco'], 1), start=1):
        per_chunk = [n for first, n, _ in stsc if first <= chunk][-1]
        for _ in range(per_chunk):
            size = sizes[number]
            samples.append((data[offset:offset + size], durations[number], offsets[number],
                            number + 1 in sync))
            offset += size
            number += 1
    if number != count or len(durations) != count:
        raise SystemExit(path + ': sample tables disagree')
    return entry, timescale, samples, struct.unpack('>HH', entry[32:36])


def run_length(values):
    out = []
    for value in values:
        if out and out[-1][1] == value:
            out[-1][0] += 1
        else:
            out.append([1, value])
    return out


def table(kind, version, rows):
    return full(kind, version, 0, struct.pack('>I', len(rows)) + b''.join(
        struct.pack('>%dI' % len(row), *row) for row in rows))


def craft(output, paths):
    sources, parts = {}, []
    for path in paths:
        if path not in sources:
            sources[path] = (len(sources) + 1, read_source(path))
        parts.append(sources[path])
    descriptions = [source[0] for _, source in sorted(sources.values(), key=lambda s: s[0])]
    timescale = parts[0][1][1]
    if any(source[1] != timescale for _, source in parts):
        raise SystemExit('the sources have different media timescales')
    samples = [s for _, source in parts for s in source[2]]
    width, height = parts[0][1][3]
    ftyp = box(b'ftyp', b'isom' + struct.pack('>I', 512) + b'isomiso2avc1mp41')
    chunk_offsets, at = [], len(ftyp) + 8
    for _, source in parts:
        chunk_offsets.append(at)
        at += sum(len(s[0]) for s in source[2])
    duration = sum(s[1] for s in samples)
    movie_duration = duration * 1000 // timescale
    offsets = [s[2] for s in samples]
    stbl = box(b'stbl', b''.join([
        full(b'stsd', 0, 0, struct.pack('>I', len(descriptions)) + b''.join(descriptions)),
        table(b'stts', 0, run_length([s[1] for s in samples])),
        table(b'ctts', 1 if min(offsets) < 0 else 0,
              [(n, v & 0xFFFFFFFF) for n, v in run_length(offsets)]),
        table(b'stss', 0, [(i,) for i, s in enumerate(samples, start=1) if s[3]]),
        table(b'stsc', 0, [(chunk, len(source[2]), description)
                           for chunk, (description, source) in enumerate(parts, start=1)]),
        full(b'stsz', 0, 0, struct.pack('>II', 0, len(samples)) + b''.join(
            struct.pack('>I', len(s[0])) for s in samples)),
        table(b'stco', 0, [(offset,) for offset in chunk_offsets]),
    ]))
    identity = struct.pack('>9I', 0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000)
    minf = box(b'minf', full(b'vmhd', 0, 1, bytes(8)) + box(b'dinf', full(
        b'dref', 0, 0, struct.pack('>I', 1) + full(b'url ', 0, 1, b''))) + stbl)
    mdia = box(b'mdia', b''.join([
        full(b'mdhd', 0, 0, struct.pack('>IIIIHH', 0, 0, timescale, duration, 0x55C4, 0)),
        full(b'hdlr', 0, 0, bytes(4) + b'vide' + bytes(12) + b'VideoHandler\0'),
        minf,
    ]))
    tkhd = full(b'tkhd', 0, 3, struct.pack('>IIIII', 0, 0, 1, 0, movie_duration) + bytes(8)
                + bytes(8) + identity + struct.pack('>II', width << 16, height << 16))
    mvhd = full(b'mvhd', 0, 0, struct.pack('>IIII', 0, 0, 1000, movie_duration)
                + struct.pack('>IH', 0x10000, 0x100) + bytes(10) + identity + bytes(24)
                + struct.pack('>I', 2))
    moov = box(b'moov', mvhd + box(b'trak', tkhd + mdia))
    with open(output, 'wb') as f:
        f.write(ftyp + box(b'mdat', b''.join(s[0] for s in samples)) + moov)


FIXTURE = 'two-sample-entries'


def fixture(ffmpeg, directory):
    video = os.path.join(directory, FIXTURE + '.mp4')
    with tempfile.TemporaryDirectory() as work:
        sources = []
        for name, pattern, size in (('a', 'testsrc', '128x96'), ('b', 'testsrc2', '80x48')):
            path = os.path.join(work, name + '.mp4')
            subprocess.run([ffmpeg, '-nostdin', '-hide_banner', '-loglevel', 'error', '-y',
                            '-f', 'lavfi', '-i', f'{pattern}=size={size}:rate=25',
                            '-frames:v', '10', '-c:v', 'libx264', '-threads', '1',
                            '-pix_fmt', 'yuv420p', '-g', '10', '-bf', '2', path], check=True)
            sources.append(path)
        craft(video, [sources[0], sources[1], sources[0]])
    with open(os.path.join(directory, FIXTURE + '.framemd5'), 'wb') as f:
        subprocess.run([ffmpeg, '-nostdin', '-v', 'error', '-noautorotate', '-ignore_editlist', '1',
                        '-flags', 'unaligned', '-i', video, '-map', '0:0',
                        '-fps_mode', 'passthrough', '-noautoscale', '-f', 'framemd5', '-'],
                       stdout=f, check=True)


def main():
    if len(sys.argv) == 4 and sys.argv[1] == '--fixture':
        fixture(sys.argv[2], sys.argv[3])
    elif len(sys.argv) >= 3 and not sys.argv[1].startswith('-'):
        craft(sys.argv[1], sys.argv[2:])
    else:
        raise SystemExit(__doc__)


if __name__ == '__main__':
    main()
