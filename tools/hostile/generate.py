#!/usr/bin/env python3
"""Generate parts 1-3 of the hostile-input suite
and the CVE route files of the stage 3 report. Throwaway spike code, also
run in CI.

Parts:
  1. structural attacks built from the ISO base media specification;
  2. seeded byte mutations of the generated MP4/MOV fixtures;
  3. PixelSmash-class files (MagicYUV in AVI and in MOV), made with the pinned
     ffmpeg binary as a black box;
  6. MPEG transport stream structures ffmpeg never writes ("mpegts"):
     tables that point at themselves or lie about their lengths, PES
     packets that never end, frame headers whose lengths do not advance,
     damaged and duplicated packets, and timestamps at the 33-bit wrap.
  5. Matroska block lacing and content compression ("matroska"):
     lacing edge cases of RFC 9559 10.3 (Xiph, EBML and fixed-size, valid
     and broken) and tracks whose frames are zlib-compressed (a valid one, a
     decompression bomb, broken and truncated streams) or use the other
     encodings, built from the specification;
  4. CVE route files ("cve"): for FFmpeg CVEs whose public description (NVD)
     names the component, a small valid file that default FFmpeg routes to
     that component by content, whatever its extension. They are not
     exploits: they carry no crafted overflow, only the format or codec the
     vulnerable code handles, so a server that runs FFmpeg on uploads reaches
     that code (and the next bug in it) while CoreAV must reject the file at
     probe time. Built from the format specifications or with the pinned
     ffmpeg as a black box.

Public FFmpeg vulnerability reproducer media (reproducers.json, beside this script) is
fetched separately; its files are never committed.

Usage: generate.py OUT_DIR FIXTURE_DIR FFMPEG [--parts structural,mutation,pixelsmash,cve]
Writes OUT_DIR/<name> and OUT_DIR/manifest.json describing each file with the
outcome CoreAV must produce ("reject" or "bounded"): reject means a structured
error at probe time; bounded means any result, but no crash, hang or
out-of-memory.
"""
import json
import pathlib
import struct
import subprocess
import sys
import zlib


def box(kind: bytes, payload: bytes) -> bytes:
    return struct.pack('>I', len(payload) + 8) + kind + payload


def full(kind: bytes, version: int, flags: int, payload: bytes) -> bytes:
    return box(kind, bytes([version]) + flags.to_bytes(3, 'big') + payload)


def minimal_moov(stbl: bytes) -> bytes:
    mvhd = full(b'mvhd', 0, 0, struct.pack('>IIII', 0, 0, 1000, 2000) + b'\0' * 80)
    tkhd = full(b'tkhd', 0, 3, struct.pack('>IIIII', 0, 0, 1, 0, 2000) + b'\0' * 16
                + struct.pack('>9i', 0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000)
                + struct.pack('>II', 320 << 16, 240 << 16))
    mdhd = full(b'mdhd', 0, 0, struct.pack('>IIII', 0, 0, 12800, 25600) + b'\x55\xc4\0\0')
    hdlr = full(b'hdlr', 0, 0, b'\0\0\0\0vide' + b'\0' * 12 + b'VideoHandler\0')
    dref = full(b'dref', 0, 0, struct.pack('>I', 1) + full(b'url ', 0, 1, b''))
    minf = box(b'minf', full(b'vmhd', 0, 1, b'\0' * 8) + box(b'dinf', dref) + stbl)
    mdia = box(b'mdia', mdhd + hdlr + minf)
    return box(b'moov', mvhd + box(b'trak', tkhd + mdia))


def avc1_entry() -> bytes:
    body = b'\0' * 6 + struct.pack('>H', 1) + b'\0' * 16
    body += struct.pack('>HH', 320, 240) + b'\0' * 50
    body += box(b'avcC', bytes([1, 100, 0, 30, 0xff, 0xe0, 0]))
    return box(b'avc1', body)


def good_stbl(sample_count: int = 4, stsz_count=None) -> bytes:
    stsd = full(b'stsd', 0, 0, struct.pack('>I', 1) + avc1_entry())
    stts = full(b'stts', 0, 0, struct.pack('>III', 1, sample_count, 512))
    stsc = full(b'stsc', 0, 0, struct.pack('>IIII', 1, 1, sample_count, 1))
    sizes = struct.pack('>II', 0, sample_count if stsz_count is None else stsz_count)
    sizes += struct.pack('>%dI' % sample_count, *([100] * sample_count))
    stsz = full(b'stsz', 0, 0, sizes)
    stco = full(b'stco', 0, 0, struct.pack('>II', 1, 48))
    return box(b'stbl', stsd + stts + stsc + stsz + stco)


def good_file() -> bytes:
    ftyp = box(b'ftyp', b'isom\0\0\x02\0isom')
    mdat = box(b'mdat', b'\xAB' * 400)
    return ftyp + mdat + minimal_moov(good_stbl())


def structural(out: pathlib.Path) -> list:
    cases = []

    def emit(name, data, outcome, note):
        (out / name).write_bytes(data)
        cases.append({'name': name, 'part': 'structural', 'outcome': outcome, 'note': note})

    # Size bombs.
    emit('size_past_end.mp4', box(b'ftyp', b'isom') + struct.pack('>I', 0x7fffffff) + b'moov',
         'reject', 'box size runs past the end of the file')
    emit('size64_overflow.mp4', box(b'ftyp', b'isom') + struct.pack('>I', 1) + b'mdat'
         + struct.pack('>Q', 0xffffffffffffffff), 'reject', '64-bit size near u64::MAX')
    emit('size_below_header.mp4', box(b'ftyp', b'isom') + struct.pack('>I', 3) + b'moov',
         'reject', 'declared size smaller than the 8-byte header')

    # Deep nesting.
    for levels in (10_000, 100_000):
        data = box(b'ftyp', b'isom\0\0\x02\0')
        for i in range(levels):
            data += struct.pack('>I', 8 * (levels - i)) + (b'moov' if i == 0 else b'trak')
        emit(f'nested_{levels}.mp4', data, 'reject', f'{levels} nested boxes')

    # Huge counts.
    emit('stsz_count_max.mp4', good_file_with(good_stbl(4, stsz_count=0xffffffff)),
         'reject', 'sample count 2^32-1 with a tiny size table')
    big_stco = full(b'stco', 0, 0, struct.pack('>I', 0xffffffff) + struct.pack('>I', 48))
    stbl = box(b'stbl', full(b'stsd', 0, 0, struct.pack('>I', 1) + avc1_entry())
               + full(b'stts', 0, 0, struct.pack('>III', 1, 4, 512))
               + full(b'stsc', 0, 0, struct.pack('>IIII', 1, 1, 4, 1))
               + full(b'stsz', 0, 0, struct.pack('>II', 100, 4)) + big_stco)
    emit('stco_count_huge.mp4', good_file_with(stbl), 'reject', 'chunk count far beyond the box')
    many_tl = box(b'ftyp', b'isom\0\0\x02\0') + box(b'free', b'') * 10_001
    emit('top_level_boxes.mp4', many_tl, 'reject', 'more than 10,000 top-level boxes')

    # Truncation.
    good = good_file()
    for fraction, label in ((0.5, 'half'), (0.95, 'most')):
        emit(f'truncated_{label}.mp4', good[: int(len(good) * fraction)], 'reject',
             f'first {int(fraction * 100)}% of a valid file')

    # Inconsistent tables.
    bad_stts = box(b'stbl', full(b'stsd', 0, 0, struct.pack('>I', 1) + avc1_entry())
                   + full(b'stts', 0, 0, struct.pack('>III', 1, 99, 512))
                   + full(b'stsc', 0, 0, struct.pack('>IIII', 1, 1, 4, 1))
                   + full(b'stsz', 0, 0, struct.pack('>II', 100, 4)) + full(b'stco', 0, 0, struct.pack('>II', 1, 48)))
    emit('stts_sample_mismatch.mp4', good_file_with(bad_stts), 'bounded',
         'stts sample count disagrees with stsz; track invalidated, file still probes')
    zero_ts = minimal_moov(good_stbl()).replace(struct.pack('>I', 12800), struct.pack('>I', 0), 1)
    emit('zero_timescale.mp4', box(b'ftyp', b'isom') + box(b'mdat', b'\xAB' * 400) + zero_ts,
         'reject', 'media timescale of zero')

    # Movie fragments that declare samples the file does not back. A
    # `trun` can put every per-sample value in the `trex` and `tfhd`
    # defaults, so its count alone claims the fragment. Probe never
    # reads a fragment, so each file is `bounded`: it probes, and the
    # commands that do read the fragment (`--ffprobe-compat`, which scans
    # by default, and `--scan-fragments`) must refuse it rather than report
    # what it claims. FFmpeg 7.0.2 refuses the first three at its header
    # and reads the fourth, whose count is small.
    emit('trun_count_max.mp4', fragmented_file(sample_count=0xffffffff), 'bounded',
         'a kilobyte whose one trun declares 2^32-1 samples of 100 bytes (5.45 years of video)')
    emit('trun_count_max_empty_samples.mp4',
         fragmented_file(sample_count=0xffffffff, default_size=0), 'bounded',
         'the same count with a default sample size of zero, so no byte of data backs it')
    emit('trun_no_sample_size.mp4', fragmented_file(sample_count=4, default_size=0), 'bounded',
         'four samples whose only size is a default sample size of zero')
    emit('trun_count_past_mdat.mp4', fragmented_file(sample_count=5), 'bounded',
         'five samples of 100 bytes where the mdat holds four')
    # The measured residue: samples that write a size of zero
    # cost four bytes each in the trun, so the box bounds them, and CoreAV
    # reads this file as it reads a stsz of zero sizes. FFmpeg refuses any
    # zero-sized sample in a trun, so this one is a known divergence in the
    # other direction, kept here to be measured rather than assumed away.
    emit('trun_zero_sample_sizes.mp4', fragmented_file(sample_count=250, per_sample_size=0),
         'bounded', '250 samples that each write a size of zero')

    return cases


def fragmented_file(sample_count: int = 4, default_size: int = 100, data_samples: int = 4,
                    per_sample_size: int = None) -> bytes:
    """A one-fragment movie: an empty `moov` with a `mvex`, a `moof` whose
    `trun` carries only a count and a data offset, and an `mdat` holding
    `data_samples` samples of 100 bytes. Every per-sample value comes from
    the `trex` defaults, so `sample_count` is all that says how large the
    fragment is."""
    ftyp = box(b'ftyp', b'iso6\0\0\x02\0')
    stbl = box(b'stbl', full(b'stsd', 0, 0, struct.pack('>I', 1) + avc1_entry())
               + full(b'stts', 0, 0, struct.pack('>I', 0))
               + full(b'stsc', 0, 0, struct.pack('>I', 0))
               + full(b'stsz', 0, 0, struct.pack('>II', 0, 0))
               + full(b'stco', 0, 0, struct.pack('>I', 0)))
    trex = full(b'trex', 0, 0, struct.pack('>IIIII', 1, 1, 512, default_size, 0))
    # The same movie box as the other files, with a `mvex` added.
    moov = box(b'moov', minimal_moov(stbl)[8:] + box(b'mvex', trex))
    tfhd = full(b'tfhd', 0, 0x020000, struct.pack('>I', 1))  # default-base-is-moof
    tfdt = full(b'tfdt', 0, 0, struct.pack('>I', 0))
    # 0x200: a size per sample, which replaces the trex default for them.
    sizes = b'' if per_sample_size is None else struct.pack('>I', per_sample_size) * sample_count
    flags = 0x1 if per_sample_size is None else 0x201
    moof_len = 8 + 16 + 8 + len(tfhd) + len(tfdt) + 20 + len(sizes)
    trun = full(b'trun', 0, flags, struct.pack('>II', sample_count, moof_len + 8) + sizes)
    moof = box(b'moof', full(b'mfhd', 0, 0, struct.pack('>I', 1)) + box(b'traf', tfhd + tfdt + trun))
    assert len(moof) == moof_len, (len(moof), moof_len)
    return ftyp + moov + moof + box(b'mdat', b'\xAB' * (data_samples * 100))


def good_file_with(stbl: bytes) -> bytes:
    ftyp = box(b'ftyp', b'isom\0\0\x02\0isom')
    mdat = box(b'mdat', b'\xAB' * 400)
    return ftyp + mdat + minimal_moov(stbl)


def mutations(out: pathlib.Path, fixtures: pathlib.Path) -> list:
    cases = []
    state = 0x1234_5678_9abc_def0

    def rng():
        nonlocal state
        state ^= (state << 13) & 0xffffffffffffffff
        state ^= state >> 7
        state ^= (state << 17) & 0xffffffffffffffff
        return state

    sources = sorted(p for p in fixtures.glob('*')
                     if p.suffix in ('.mp4', '.mov', '.m4a') and p.stat().st_size < 400_000)
    for src in sources:
        base = bytearray(src.read_bytes())
        for variant in range(3):
            data = bytearray(base)
            for _ in range(1 + rng() % 4):
                at = rng() % len(data)
                data[at] = rng() & 0xff
            name = f'mut_{src.stem}_{variant}{src.suffix}'
            (out / name).write_bytes(data)
            cases.append({'name': name, 'part': 'mutation', 'outcome': 'bounded',
                          'note': f'seeded mutation of {src.name}'})
    return cases


def pixelsmash(out: pathlib.Path, ffmpeg: str) -> list:
    cases = []
    src = ['-f', 'lavfi', '-i', 'color=c=red:s=64x64:d=1', '-c:v', 'magicyuv']
    # CVE-2026-8461 (PixelSmash) is a heap out-of-bounds write in FFmpeg's
    # MagicYUV decoder, reached by probing or thumbnailing an uploaded file.
    # These files put MagicYUV in a container; CoreAV must reject the codec at
    # probe time, so the decoder is never reached. The MOV file is the direct
    # test: CoreAV parses the container, finds a non-allowlisted codec and
    # rejects it. AVI and MKV are unsupported containers, also rejected.
    targets = [('magicyuv.avi', []), ('magicyuv.mov', []), ('magicyuv.mkv', [])]
    for name, extra in targets:
        path = out / name
        result = subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', *src, *extra, str(path)],
                                capture_output=True, timeout=60)
        if result.returncode != 0 or not path.exists():
            # A suite without one of its CVE files would still pass.
            sys.exit(f'could not make {name}: {result.stderr.decode()[:200]}')
        cases.append({'name': name, 'part': 'pixelsmash', 'cve': 'CVE-2026-8461', 'outcome': 'reject',
                      'note': 'MagicYUV video (PixelSmash class); codec and/or container outside the allowlist'})
    return cases


# Part 4: (file name, CVE, NVD description of the component, how the file is
# made). `ffmpeg` arguments make the file with the pinned binary; `text` is
# written as is. Every file is named so that a server keeping the uploader's
# name would hand it to FFmpeg as is; FFmpeg detects the format by content.
CVE_ROUTES = [
    ('cve-2024-7055_pnm.mp4', 'CVE-2024-7055',
     'heap-based buffer overflow in pnm_decode_frame (libavcodec/pnmdec.c), FFmpeg up to 7.0.1',
     {'ffmpeg': ['-frames:v', '1', '-c:v', 'ppm', '-f', 'image2pipe']},
     'a PPM image named .mp4: FFmpeg probes it as ppm_pipe and decodes it with the PNM decoder'),
    ('cve-2020-35965_exr.mp4', 'CVE-2020-35965',
     'out-of-bounds write in decode_frame (libavcodec/exr.c), FFmpeg 4.3.1',
     {'ffmpeg': ['-frames:v', '1', '-c:v', 'exr', '-f', 'image2pipe']},
     'an OpenEXR image named .mp4: FFmpeg probes it as exr_pipe and decodes it with the EXR decoder'),
    ('cve-2020-35965_exr.mkv', 'CVE-2020-35965',
     'out-of-bounds write in decode_frame (libavcodec/exr.c), FFmpeg 4.3.1',
     {'ffmpeg': ['-frames:v', '2', '-c:v', 'exr', '-f', 'matroska']},
     'EXR video in Matroska: CoreAV parses the container and must reject the codec'),
    ('cve-2024-36618_avi.mp4', 'CVE-2024-36618',
     'integer overflow in the AVI demuxer (libavformat), FFmpeg n6.1.1',
     {'ffmpeg': ['-frames:v', '2', '-c:v', 'mpeg4', '-f', 'avi']},
     'an AVI file named .mp4: FFmpeg probes it with the AVI demuxer'),
    ('cve-2024-35366_sbg.mp4', 'CVE-2024-35366',
     'integer overflow in parse_options (libavformat/sbgdec.c), FFmpeg n6.1.1',
     {'text': 'base: 200+10/50\nNOW base\n'},
     'an SBaGen script named .mp4: FFmpeg probes it with the SBG demuxer and synthesizes audio from it'),
]


def cve_routes(out: pathlib.Path, ffmpeg: str) -> list:
    cases = []
    src = ['-f', 'lavfi', '-i', 'color=c=red:s=64x64:d=1']
    for name, cve, component, make, note in CVE_ROUTES:
        path = out / name
        if 'text' in make:
            path.write_text(make['text'])
        else:
            result = subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', *src, *make['ffmpeg'],
                                     str(path)], capture_output=True, timeout=60)
            if result.returncode != 0 or not path.exists():
                # A suite without one of its CVE files would still pass.
                sys.exit(f'could not make {name}: {result.stderr.decode()[:200]}')
        cases.append({'name': name, 'part': 'cve', 'cve': cve, 'component': component, 'outcome': 'reject',
                      'note': note})
    return cases


def declare_size(path: pathlib.Path, fourcc: bytes, width: int, height: int) -> None:
    """Writes a size into the sample entry `fourcc` and the track header."""
    data = bytearray(path.read_bytes())
    entry = data.find(fourcc, data.find(b'stsd'))
    track = data.find(b'tkhd')
    if data.find(b'stsd') < 0 or entry < 0 or track < 0:
        sys.exit(f'could not find the sample entry or track header of {path.name}')
    # VisualSampleEntry: width and height 24 bytes after the type's end;
    # tkhd (version 0): width and height (16.16) 72 bytes after it.
    struct.pack_into('>HH', data, entry + 28, width, height)
    struct.pack_into('>II', data, track + 76, width << 16, height << 16)
    path.write_bytes(bytes(data))


def limits(out: pathlib.Path, ffmpeg: str) -> list:
    """Files within every container and frame-size limit whose decoding
    would not fit a decoder's memory budget (docs/design/decode.md,
    "Memory"), each declaring 16384x8192 (2^27 pixels, the decode limit)
    in its sample entry and track header:
    - ProRes 4444: about 1 GiB a decoded frame; eight frames and one
      thread just over the 8 GiB budget, and far over the 1 GiB the hostile
      check runs under;
    - AV1 4:4:4 at 10 bits: 768 MiB a frame, and dav1d's 8 references and
      one frame in flight over 1 GiB too.
    `coreav thumbnail` must refuse each with `limit` before a decoder
    starts, never be killed for memory."""
    files = [
        ('memory_16384x8192.mov', b'ap4h', ['-c:v', 'prores_ks', '-profile:v', '4444', '-pix_fmt', 'yuva444p10le'],
         'a frame declared at 16384x8192: its decoding needs more memory than the budget'),
        ('memory_av1_16384x8192.mp4', b'av01',
         ['-c:v', 'libaom-av1', '-cpu-used', '8', '-pix_fmt', 'yuv444p10le'],
         'an AV1 frame declared at 16384x8192: its decoding needs more memory than the budget'),
    ]
    cases = []
    for name, fourcc, codec, note in files:
        path = out / name
        result = subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i',
                                 'color=c=red:s=64x64:d=2', *codec, str(path)],
                                capture_output=True, timeout=60)
        if result.returncode != 0 or not path.exists():
            sys.exit(f'could not make {path.name}: {result.stderr.decode()[:200]}')
        declare_size(path, fourcc, 16384, 8192)
        cases.append({'name': path.name, 'part': 'limits', 'outcome': 'bounded', 'thumbnail_kind': 'limit',
                      'note': note})
    return cases


# ------------------------------------------------------------- Matroska

def ebml_id(value: int) -> bytes:
    return value.to_bytes((value.bit_length() + 7) // 8, 'big')


def ebml_size(size: int) -> bytes:
    # The shortest VINT (RFC 8794 4); all-ones values are reserved.
    for length in range(1, 9):
        if size < (1 << (7 * length)) - 1:
            return (size | (1 << (7 * length))).to_bytes(length, 'big')
    raise ValueError('size too large')


def mel(value: int, payload: bytes) -> bytes:
    return ebml_id(value) + ebml_size(len(payload)) + payload


def muint(value: int, number: int) -> bytes:
    return mel(value, number.to_bytes(max(1, (number.bit_length() + 7) // 8), 'big'))


def mkv_file(doc_type: str, track: bytes, clusters: list) -> bytes:
    header = mel(0x1A45DFA3, muint(0x4286, 1) + muint(0x42F7, 1) + muint(0x42F2, 4) + muint(0x42F3, 8)
                 + mel(0x4282, doc_type.encode()) + muint(0x4287, 4) + muint(0x4285, 2))
    info = mel(0x1549A966, muint(0x2AD7B1, 1_000_000) + mel(0x4489, struct.pack('>d', 1000.0))
               + mel(0x4D80, b'coreav-hostile') + mel(0x5741, b'coreav-hostile'))
    return header + mel(0x18538067, info + mel(0x1654AE6B, track) + b''.join(clusters))


def opus_track(extra: bytes = b'') -> bytes:
    head = b'OpusHead' + bytes([1, 2]) + struct.pack('<HIhB', 312, 48000, 0, 0)
    return mel(0xAE, muint(0xD7, 1) + muint(0x73C5, 1) + muint(0x83, 2) + mel(0x86, b'A_OPUS')
               + mel(0x63A2, head) + mel(0xE1, mel(0xB5, struct.pack('>d', 48000.0)) + muint(0x9F, 2)) + extra)


def text_track(encoding: bytes) -> bytes:
    return mel(0xAE, muint(0xD7, 1) + muint(0x73C5, 1) + muint(0x83, 0x11) + mel(0x86, b'S_TEXT/UTF8')
               + muint(0x9C, 0) + encoding)


def encodings(algo=None, scope=1, kind=0, settings=b'', chain=False) -> bytes:
    """ContentEncodings with one ContentEncoding (two with `chain`)."""
    def one(order):
        comp = b''
        if kind == 0:
            comp = mel(0x5034, (muint(0x4254, algo) if algo is not None else b'')
                       + (mel(0x4255, settings) if settings else b''))
        return mel(0x6240, muint(0x5031, order) + muint(0x5032, scope) + muint(0x5033, kind) + comp)
    return mel(0x6D80, one(0) + (one(1) if chain else b''))


def cluster(timestamp: int, blocks: list) -> bytes:
    return mel(0x1F43B675, muint(0xE7, timestamp) + b''.join(blocks))


def simple_block(body: bytes, flags: int = 0x80, timestamp: int = 0) -> bytes:
    """A SimpleBlock of track 1: `body` is what follows the flags octet."""
    return mel(0xA3, b'\x81' + struct.pack('>h', timestamp) + bytes([flags]) + body)


def xiph(sizes: list) -> bytes:
    out = bytearray()
    for size in sizes:
        out += b'\xff' * (size // 255) + bytes([size % 255])
    return bytes(out)


def matroska(out: pathlib.Path) -> list:
    cases = []

    def add(name, data, note, outcome='bounded'):
        (out / name).write_bytes(data)
        cases.append({'name': name, 'part': 'matroska', 'outcome': outcome, 'note': note})

    def opus(blocks, name, note, track=None):
        add(name, mkv_file('webm', track or opus_track(), [cluster(0, blocks)]), note)

    # Xiph lacing (flags 0x02): a valid lace with long runs of 255.
    sizes = [70_000, 1, 300]
    frames = b''.join(bytes([i]) * n for i, n in enumerate(sizes)) + b'\x07' * 20
    opus([simple_block(bytes([3]) + xiph(sizes) + frames, 0x82)], 'mkv_xiph_long_runs.webm',
         'Xiph lacing with sizes of 70,000 (runs of 255), 1 and 300')
    # A run of 255 that goes past the end of the block.
    opus([simple_block(bytes([1]) + b'\xff' * 40, 0x82)], 'mkv_xiph_run_past_block.webm',
         'a Xiph lace size whose run of 255 octets ends past the block')
    # Lace sizes that add up to more than the block.
    opus([simple_block(bytes([2]) + xiph([200, 200]) + b'\0' * 100, 0x82)], 'mkv_xiph_sizes_exceed.webm',
         'Xiph lace sizes that add up to more than the block holds')
    # 256 frames of size 0 then data (the most frames a lace holds).
    opus([simple_block(bytes([255]) + b'\0' * 255 + b'\x01' * 8, 0x82)], 'mkv_xiph_256_frames.webm',
         'Xiph lacing with 256 frames, 255 of them empty')
    # EBML lacing (flags 0x06): a negative size from the differences.
    opus([simple_block(bytes([2]) + b'\x8a' + b'\x80' + b'\0' * 40, 0x86)], 'mkv_ebml_negative.webm',
         'EBML lacing whose second size, from a signed difference, is negative')
    # A first size as an 8-octet VINT larger than the file.
    opus([simple_block(bytes([1]) + b'\x01\x00\x00\x10\x00\x00\x00\x00' + b'\0' * 40, 0x86)],
         'mkv_ebml_huge_size.webm', 'EBML lacing with a first size of 2^44')
    # All-ones (reserved) VINTs as sizes, and differences that overflow.
    opus([simple_block(bytes([3]) + b'\xff' + b'\x01\xff\xff\xff\xff\xff\xff\xff' * 2 + b'\0' * 40, 0x86)],
         'mkv_ebml_reserved_vints.webm', 'EBML lacing with all-ones VINTs as the size and differences')
    # Fixed-size lacing (flags 0x04) that does not divide the block.
    opus([simple_block(bytes([2]) + b'\0' * 10, 0x84)], 'mkv_fixed_not_dividing.webm',
         'fixed-size lacing of 3 frames over 10 bytes')
    # 256 frames of size 0.
    opus([simple_block(bytes([255]), 0x84)], 'mkv_fixed_256_empty.webm',
         'fixed-size lacing of 256 empty frames')
    # The lacing bits set, but the block ends before the frame count.
    opus([simple_block(b'', 0x82)], 'mkv_lace_count_missing.webm',
         'a laced block that ends before its frame count')
    # A laced block of a track whose FlagLacing is 0.
    opus([simple_block(bytes([1]) + xiph([10]) + b'\0' * 20, 0x82)], 'mkv_lacing_flag_off.webm',
         'a laced block in a track with FlagLacing 0', opus_track(muint(0x9C, 0)))
    # A laced Block in a BlockGroup, with a BlockDuration.
    group = mel(0xA0, mel(0xA1, b'\x81\x00\x00\x02' + bytes([1]) + xiph([10]) + b'\0' * 20) + muint(0x9B, 40))
    opus([group], 'mkv_laced_blockgroup.webm', 'a Xiph-laced Block in a BlockGroup with a BlockDuration')
    # Many blocks of 256 tiny frames each: the work bound.
    block = simple_block(bytes([255]) + b'\x01' * 255 + b'\x02' * 256, 0x82)
    add('mkv_many_laced_blocks.webm', mkv_file('webm', opus_track(), [cluster(i, [block] * 100)
                                                                        for i in range(40)]),
        '4,000 Xiph-laced blocks of 256 frames each')

    # zlib content compression (ContentCompAlgo 0) of a text track.
    def text(name, note, encoding, frames):
        blocks = [mel(0xA0, mel(0xA1, b'\x81' + struct.pack('>h', 100 * i) + b'\0' + frame) + muint(0x9B, 90))
                  for i, frame in enumerate(frames)]
        add(name, mkv_file('matroska', text_track(encoding), [cluster(0, blocks)]), note)

    lines = [f'line {i}: hello world'.encode() for i in range(5)]
    text('mkv_zlib_text.mkv', 'S_TEXT/UTF8 frames compressed with zlib', encodings(algo=0),
         [zlib.compress(line) for line in lines])
    # A frame that inflates to 256 MiB from about 256 KiB.
    bomb = zlib.compress(bytes(256 << 20), 9)
    text('mkv_zlib_bomb.mkv', 'a zlib frame that inflates to 256 MiB', encodings(algo=0), [bomb])
    stream = zlib.compress(bytes(range(256)) * 16)
    text('mkv_zlib_truncated.mkv', 'a zlib frame cut short', encodings(algo=0), [stream[:len(stream) // 2]])
    text('mkv_zlib_garbage.mkv', 'frames that are not zlib streams', encodings(algo=0),
         [b'not zlib at all', b'\x78\x9c\xff\xff\xff\xff'])
    text('mkv_zlib_chain.mkv', 'zlib then header stripping: a chain of encodings',
         encodings(algo=0, chain=True), [zlib.compress(line) for line in lines])
    text('mkv_zlib_private_scope.mkv', 'zlib applied to the codec private data (scope 2)',
         encodings(algo=0, scope=2), [zlib.compress(line) for line in lines])
    text('mkv_bzlib.mkv', 'bzlib compression (ContentCompAlgo 1)', encodings(algo=1), lines)
    text('mkv_lzo.mkv', 'lzo compression (ContentCompAlgo 2)', encodings(algo=2), lines)
    text('mkv_header_strip_huge.mkv', 'header stripping with 512 KiB of ContentCompSettings',
         encodings(algo=3, settings=b'\xab' * (512 << 10)), lines)
    # zlib on an audio track, which a remux would have to copy.
    add('mkv_zlib_audio.webm', mkv_file('webm', opus_track(encodings(algo=0)),
                                        [cluster(0, [simple_block(zlib.compress(b'\xfc' + b'\0' * 60))])]),
        'an Opus track whose frames are zlib-compressed')
    return cases


# --------------------------------------------- MPEG transport streams

TS_SYNC = 0x47


def ts_packet(pid: int, start: bool, payload: bytes, counter: int,
              error: bool = False, scrambled: int = 0) -> bytes:
    """One 188-byte transport packet, padded with an adaptation field."""
    head = bytes([TS_SYNC,
                  (int(error) << 7) | (int(start) << 6) | (pid >> 8),
                  pid & 0xFF,
                  (scrambled << 6) | 0x10 | (counter & 0x0F)])
    body = payload[:184]
    if len(body) < 184:
        pad = 184 - len(body)
        head = head[:3] + bytes([head[3] | 0x20])
        body = (bytes([pad - 1]) + (b'\x00' + b'\xff' * (pad - 2) if pad > 1 else b'')) + body
    return head + body


class Ts:
    """A writer of 188-byte packets with continuity counters."""

    def __init__(self):
        self.out = bytearray()
        self.counters = {}

    def packet(self, pid, start, payload, **kw):
        counter = self.counters.get(pid, 0)
        self.counters[pid] = (counter + 1) & 0x0F
        self.out += ts_packet(pid, start, payload, counter, **kw)

    def unit(self, pid, data, **kw):
        for i in range(0, max(len(data), 1), 184):
            self.packet(pid, i == 0, data[i:i + 184], **kw)

    def section(self, pid, table_id, ident, body, version=0, crc=None, number=0, last=0):
        length = 5 + len(body) + 4
        s = bytes([table_id, 0xB0 | (length >> 8), length & 0xFF,
                   ident >> 8, ident & 0xFF, 0xC1 | ((version & 0x1F) << 1), number, last]) + body
        s += struct.pack('>I', ts_crc32(s) if crc is None else crc)
        self.unit(pid, b'\x00' + s)

    def pat(self, programs, **kw):
        body = b''.join(bytes([n >> 8, n & 0xFF, 0xE0 | (pid >> 8), pid & 0xFF])
                        for n, pid in programs)
        self.section(0, 0x00, 1, body, **kw)

    def pmt(self, pid, program, streams, **kw):
        body = bytearray(b'\xe1\x00\xf0\x00')
        for kind, es, *rest in streams:
            descriptors = rest[0] if rest else b''
            body += bytes([kind, 0xE0 | (es >> 8), es & 0xFF,
                           0xF0 | (len(descriptors) >> 8), len(descriptors) & 0xFF])
            body += descriptors
        self.section(pid, 0x02, program, bytes(body), **kw)

    def pes(self, pid, stream_id, pts, payload, length=None, **kw):
        declared = len(payload) + 8 if length is None else length
        pes = bytes([0, 0, 1, stream_id, declared >> 8, declared & 0xFF, 0x80, 0x80, 5])
        pes += bytes([0x21 | ((pts >> 30 & 7) << 1), (pts >> 22) & 0xFF,
                      ((pts >> 15) & 0x7F) << 1 | 1, (pts >> 7) & 0xFF, (pts & 0x7F) << 1 | 1])
        self.unit(pid, pes + payload, **kw)


def ts_crc32(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte << 24
        for _ in range(8):
            crc = ((crc << 1) ^ 0x04C11DB7) & 0xFFFFFFFF if crc & 0x80000000 else (crc << 1) & 0xFFFFFFFF
    return crc


def ts_adts(length: int) -> bytes:
    """An ADTS frame of AAC LC, 48 kHz, stereo, `length` bytes."""
    head = bytes([0xFF, 0xF1, 0x4C, 0x80 | ((length >> 11) & 3), (length >> 3) & 0xFF,
                  ((length & 7) << 5) | 0x1F, 0xFC])
    return head + b'\x21' * max(length - 7, 0)


def mpegts(out: pathlib.Path) -> list:
    """Transport stream structures ffmpeg never writes (ISO/IEC 13818-1):
    tables that point at themselves or lie about their lengths, PES packets
    that never end, frame headers whose lengths do not advance, and streams
    whose packets are lost, duplicated or scrambled."""
    cases = []

    def add(name, data, note, outcome='bounded'):
        (out / name).write_bytes(bytes(data))
        cases.append({'name': name, 'part': 'mpegts', 'outcome': outcome, 'note': note})

    def audio():
        t = Ts()
        t.pat([(1, 0x1000)])
        t.pmt(0x1000, 1, [(0x0F, 0x101)])
        return t

    # Tables that point at themselves or at every PID.
    t = Ts()
    t.pat([(1, 0)])
    add('ts_pat_points_at_itself.ts', t.out, 'a PAT whose program map table is PID 0')
    # A Program Association Table in four sections of 200 programs, past the
    # 256 the limits allow, each naming a program map table.
    t = Ts()
    for part in range(4):
        t.pat([(200 * part + n, 0x1000 + 200 * part + n) for n in range(1, 201)],
              number=part, last=3)
    t.pmt(0x1001, 1, [(0x0F, 0x101)])
    add('ts_many_programs.ts', t.out, '800 programs in four Program Association Table sections')
    # Eight program map tables of 200 streams each: 1,600 streams.
    t = Ts()
    t.pat([(n, 0x1000 + n) for n in range(1, 9)])
    for program in range(1, 9):
        t.pmt(0x1000 + program, program,
              [(0x0F, 0x200 + 200 * program + n) for n in range(200)])
    add('ts_many_streams.ts', t.out, '1,600 streams across eight program map tables')
    t = Ts()
    t.pat([(1, 0x1000)])
    t.pmt(0x1000, 1, [(0x0F, 0x101, b'\x05\xff' + b'A' * 200)])
    add('ts_descriptor_past_section.ts', t.out,
        'an elementary stream descriptor whose length runs past its section')
    t = Ts()
    t.pat([(1, 0x1000)], crc=0)
    t.pmt(0x1000, 1, [(0x0F, 0x101)], crc=0)
    add('ts_bad_section_crc.ts', t.out, 'a PAT and a PMT whose CRC-32 is wrong')
    t = Ts()
    t.packet(0, True, b'\xb0' + b'\x00' * 183)
    t.packet(0, False, b'\x00' * 184)
    add('ts_pointer_past_payload.ts', t.out, 'a section pointer field past the packet payload')

    # Program map tables that never settle.
    t = Ts()
    t.pat([(1, 0x1000)])
    for version in range(400):
        t.pmt(0x1000, 1, [(0x0F, 0x101 + (version % 7))], version=version)
    add('ts_pmt_version_churn.ts', t.out, 'a program map table with a new version 400 times')

    # PES packets that never end, or that lie about their length.
    t = audio()
    t.pes(0x101, 0xC0, 90_000, b'\x21' * 60_000, length=0)
    add('ts_unbounded_pes.ts', t.out, 'a PES packet without a length, 60 KiB of payload')
    t = audio()
    t.pes(0x101, 0xC0, 90_000, ts_adts(100), length=0xFFFF)
    add('ts_pes_length_past_file.ts', t.out, 'a PES packet declaring 64 KiB and ending at once')
    t = audio()
    for i in range(400):
        t.packet(0x101, True, b'\x00\x00\x01\xc0\x00\x00\x80\x80\x05' + b'\x21' * 20)
    add('ts_only_unit_starts.ts', t.out, '400 PES packets each started and never finished')
    t = audio()
    t.packet(0x101, False, b'\x21' * 184)
    add('ts_payload_without_start.ts', t.out, 'payload on a stream PID with no unit start')

    # Frame headers whose lengths do not advance the reader.
    t = audio()
    t.pes(0x101, 0xC0, 90_000, bytes([0xFF, 0xF1, 0x4C, 0x80, 0x00, 0x1F, 0xFC]) * 2_000)
    add('ts_adts_zero_length.ts', t.out, '2,000 ADTS headers that declare a length of zero')
    t = audio()
    t.pes(0x101, 0xC0, 90_000, ts_adts(8_000)[:300])
    add('ts_adts_frame_cut.ts', t.out, 'an ADTS frame of 8 KiB with 300 bytes present')
    t = Ts()
    t.pat([(1, 0x1000)])
    t.pmt(0x1000, 1, [(0x06, 0x101, b'\x05\x04Opus')])
    t.pes(0x101, 0xBD, 90_000, b'\x7f\xe0' + b'\xff' * 400 + b'\x00')
    add('ts_opus_huge_control.ts', t.out, 'an Opus access unit whose control header declares 100 KiB')
    t = Ts()
    t.pat([(1, 0x1000)])
    t.pmt(0x1000, 1, [(0x06, 0x101, b'\x05\x04EAC3')])
    # An independent frame then eight dependent substreams, repeated.
    frame = bytes([0x0B, 0x77, 0x00, 0x3F, 0x00, 0x00]) + b'\x00' * 250
    dependent = bytes([0x0B, 0x77, 0x40, 0x3F, 0x00, 0x00]) + b'\x00' * 250
    t.pes(0x101, 0xBD, 90_000, (frame + dependent * 20) * 4)
    add('ts_eac3_dependent_run.ts', t.out, 'twenty dependent E-AC-3 substreams after each frame')

    # Damaged packets: lost sync, duplicates, continuity gaps, scrambling.
    t = audio()
    t.pes(0x101, 0xC0, 90_000, ts_adts(100))
    tail = Ts()
    tail.counters = dict(t.counters)
    tail.pes(0x101, 0xC0, 91_920, ts_adts(100))
    data = bytearray(t.out) + bytes(b for b in range(1, 200) if b != TS_SYNC) * 400 + tail.out
    add('ts_lost_sync_long.ts', data,
        '78 KiB without a sync byte between the tables and the rest of the stream')
    add('ts_all_sync_bytes.ts', bytes([TS_SYNC]) * (188 * 200),
        'a file of sync bytes only: packets everywhere, no tables')
    t = audio()
    packet = ts_packet(0x101, True, b'\x00\x00\x01\xc0\x00\x00\x80\x00\x00' + ts_adts(100), 0)
    t.out += packet * 500
    add('ts_duplicate_run.ts', t.out, 'the same transport packet 500 times over')
    t = audio()
    t.pes(0x101, 0xC0, 90_000, ts_adts(100), error=True)
    add('ts_transport_error.ts', t.out, 'a stream whose packets all set the transport error indicator')
    t = audio()
    t.pes(0x101, 0xC0, 90_000, ts_adts(100), scrambled=2)
    add('ts_scrambled.ts', t.out, 'scrambled packets on a stream PID')
    t = audio()
    for i in range(300):
        t.packet(0x101, i == 0, b'\x21' * 184)
        t.counters[0x101] = 0
    add('ts_counter_stuck.ts', t.out, 'a continuity counter that never advances')

    # Timestamps: the 33-bit wrap, and times that go backwards.
    t = audio()
    t.pes(0x101, 0xC0, (1 << 33) - 1_920, ts_adts(100))
    t.pes(0x101, 0xC0, 0, ts_adts(100))
    add('ts_wrap_edge.ts', t.out, 'a timestamp one frame before the 33-bit wrap, then zero')
    t = audio()
    for i in range(200):
        t.pes(0x101, 0xC0, (1 << 33) - 1 - i * 1_920, ts_adts(100))
    add('ts_time_backwards.ts', t.out, '200 PES packets whose times run backwards from the wrap')

    # The other packet sizes, and a prefix that mimics them.
    t = audio()
    t.pes(0x101, 0xC0, 90_000, ts_adts(100))
    m2ts = bytearray()
    for i in range(0, len(t.out), 188):
        m2ts += struct.pack('>I', 0x4000_0000 | i) + t.out[i:i + 188]
    add('ts_m2ts_prefix.m2ts', m2ts, '192-byte packets with a BDAV arrival timestamp')
    add('ts_sync_stride_only.ts',
        b''.join(bytes([TS_SYNC]) + bytes((i * 31 + 7) % 251 for i in range(187)) for i in range(300)),
        'sync bytes at a 188-byte stride with no tables behind them')
    return cases


PARTS = ['structural', 'mutation', 'pixelsmash', 'cve', 'matroska', 'mpegts', 'limits']


def main():
    args = sys.argv[1:]
    parts = PARTS
    if len(args) == 5 and args[3] == '--parts':
        parts = args[4].split(',')
        args = args[:3]
    if len(args) != 3 or any(p not in PARTS for p in parts):
        sys.exit('usage: generate.py OUT_DIR FIXTURE_DIR FFMPEG '
                 '[--parts structural,mutation,pixelsmash,cve,matroska,mpegts,limits]')
    out = pathlib.Path(args[0])
    out.mkdir(parents=True, exist_ok=True)
    fixtures = pathlib.Path(args[1])
    makers = {'structural': lambda: structural(out), 'mutation': lambda: mutations(out, fixtures),
              'matroska': lambda: matroska(out), 'mpegts': lambda: mpegts(out),
              'pixelsmash': lambda: pixelsmash(out, args[2]), 'cve': lambda: cve_routes(out, args[2]),
              'limits': lambda: limits(out, args[2])}
    cases = [case for part in parts for case in makers[part]()]
    manifest = {'schema': 'coreav-w1-hostile/1', 'parts': parts, 'cases': cases}
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'{len(cases)} hostile files in {out}')


if __name__ == '__main__':
    main()
