#!/usr/bin/env python3
"""Independent bounded P-picture syntax and synthetic three-decoder conformance."""
import argparse
from collections import Counter
import hashlib
import json
import os
import pathlib
import re
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import h264_g1_conformance as g1


ROOT = pathlib.Path(__file__).resolve().parents[1]
QPS = (10, 16, 28, 40, 51)
MODES = ('I_4x4', 'I_16x16', 'P_Skip', 'P_L0_16x16', 'P_L0_16x8',
         'P_L0_8x16', 'P_8x8', 'sub_8x8', 'sub_8x4', 'sub_4x8', 'sub_4x4')
SUBCOUNTS = (1, 2, 2, 4)
FORCED_LAYOUTS = (('P_L0_16x16', (), 1), ('P_L0_16x8', (), 2), ('P_L0_8x16', (), 2),
                  ('P_8x8', (0, 0, 0, 0), 4), ('P_8x8', (1, 1, 1, 1), 8),
                  ('P_8x8', (2, 2, 2, 2), 8), ('P_8x8', (3, 3, 3, 3), 16),
                  ('P_8x8', (0, 1, 2, 3), 9))
INTRA_CBP = (47, 31, 15, 0, 23, 27, 29, 30, 7, 11, 13, 14, 39, 43, 45, 46,
             16, 3, 5, 10, 12, 19, 21, 26, 28, 35, 37, 42, 44, 1, 2, 4,
             8, 17, 18, 20, 24, 6, 9, 22, 25, 32, 33, 34, 36, 40, 38, 41)
INTER_CBP = (0, 16, 1, 2, 4, 8, 32, 3, 5, 10, 12, 15, 47, 7, 11, 13,
             14, 6, 9, 31, 35, 37, 42, 44, 33, 34, 36, 40, 39, 43, 45, 46,
             17, 18, 20, 24, 19, 21, 26, 28, 23, 27, 29, 30, 22, 25, 38, 41)


def bounded(value, low, high, name):
    if not low <= value <= high:
        raise ValueError(f'{name} outside [{low}, {high}]')
    return value


def vlc_tables(path=ROOT / 'crates/coreav-h264-encoder/src/vlc.rs'):
    text = path.read_text()
    if len(text) > 32768:
        raise ValueError('oversized specification table owner')
    tables = {}
    for name, rows, columns in (('TOKENS', 68, 5), ('ZEROS', 15, 16),
                                 ('CHROMA_ZEROS', 3, 16), ('RUNS', 7, 16)):
        matches = re.findall(r'\bconst ' + name + r':[^=]+?=\s*(\[.*?\]);', text, re.S)
        if len(matches) != 1:
            raise ValueError(f'missing or ambiguous literal table {name}')
        literal = matches[0]
        if not re.fullmatch(r'[\[\]\s,0-9]+', literal):
            raise ValueError('nonliteral specification table')
        row_text = re.findall(r'\[([^\[\]]*)\]', literal)
        if len(row_text) != rows:
            raise ValueError('wrong specification table rows')
        result = []
        for row in row_text:
            if not re.fullmatch(r'\s*(?:[0-9]{1,4}\s*,\s*)*[0-9]{1,4}\s*,?\s*', row):
                raise ValueError('malformed specification table row')
            values = tuple(int(v) for v in re.findall(r'[0-9]+', row))
            g1.require(len(values), columns, 'specification table columns')
            for value in values:
                if value and not (1 <= value % 32 <= 16 and value >> 5 < 1 << (value % 32)):
                    raise ValueError('invalid packed specification codeword')
            result.append(values)
        tables[name] = tuple(result)
    return tables


class Bits(g1.Bits):
    def __init__(self, data):
        super().__init__(data)
        if not data or data[-1] == 0:
            raise ValueError('missing RBSP trailing bits')
        self.end = len(data) * 8 - (data[-1] & -data[-1]).bit_length()

    def u(self, n):
        if n > 32 or n < 0 or self.at + n > self.end:
            raise ValueError('truncated syntax before RBSP trailing bits')
        return super().u(n)

    def finish(self):
        g1.require(self.at, self.end, 'unconsumed RBSP payload')


def rbsp(nal):
    out = bytearray()
    zeros = 0
    for at, value in enumerate(nal[1:], 1):
        if zeros == 2:
            if value == 3:
                if at + 1 >= len(nal) or nal[at + 1] > 3:
                    raise ValueError('invalid emulation prevention')
                zeros = 0
                continue
            if value <= 2:
                raise ValueError('unescaped start-code emulation')
        out.append(value)
        zeros = zeros + 1 if value == 0 else 0
    return Bits(out)


def read_vlc(r, packed):
    codes = {(v % 32, v >> 5): i for i, v in enumerate(packed) if v}
    code = 0
    for length in range(1, 17):
        code = code * 2 + r.u(1)
        if (length, code) in codes:
            return codes[length, code]
    raise ValueError('invalid finite VLC')


def residual_block(r, maximum, nc, tables):
    column = 4 if nc == -1 else 0 if nc < 2 else 1 if nc < 4 else 2 if nc < 8 else 3
    token = read_vlc(r, [row[column] for row in tables['TOKENS']])
    total, trailing = divmod(token, 4)
    if total > maximum or trailing > min(total, 3):
        raise ValueError('invalid coefficient token')
    if not total:
        return 0
    suffix = int(total > 10 and trailing < 3)
    r.u(trailing)
    for i in range(trailing, total):
        prefix = 0
        while r.u(1) == 0:
            prefix += 1
            bounded(prefix, 0, 15, 'Constrained Baseline level_prefix')
        size = 4 if prefix == 14 and suffix == 0 else prefix - 3 if prefix >= 15 else suffix
        code = (min(15, prefix) << suffix) + r.u(size)
        if prefix >= 15 and suffix == 0:
            code += 15
        if i == trailing and trailing < 3:
            code += 2
        level = (code + 2) // 2 if code % 2 == 0 else (-code - 1) // 2
        suffix = max(1, suffix)
        if abs(level) > 3 << (suffix - 1) and suffix < 6:
            suffix += 1
    zeros = 0
    if total < maximum:
        name = 'CHROMA_ZEROS' if maximum == 4 else 'ZEROS'
        zeros = read_vlc(r, tables[name][total - 1])
        bounded(zeros, 0, maximum - total, 'total_zeros')
    for _ in range(total - 1):
        if zeros:
            run = read_vlc(r, tables['RUNS'][min(zeros, 7) - 1])
            bounded(run, 0, zeros, 'run_before')
            zeros -= run
    return total


def parameter_sets(sps, pps, width, height):
    r = rbsp(sps)
    g1.require((r.u(8), r.u(8), r.u(8)), (66, 0xc0, 51), 'profile constraints level')
    g1.require([r.ue() for _ in range(4)], [0, 0, 2, 1], 'SPS fixed fields')
    g1.require(r.u(1), 0, 'frame number gaps')
    cw, ch = (r.ue() + 1) * 16, (r.ue() + 1) * 16
    g1.require((cw, ch), ((width + 15) // 16 * 16, (height + 15) // 16 * 16), 'coded geometry')
    g1.require((r.u(1), r.u(1)), (1, 1), 'progressive frame')
    crop = [r.ue() for _ in range(4)] if r.u(1) else [0] * 4
    g1.require(crop, [0, (cw - width) // 2, 0, (ch - height) // 2], 'crop')
    g1.require([r.u(1) for _ in range(4)], [1, 0, 0, 1], 'VUI presence')
    g1.require((r.u(3), r.u(1), r.u(1)), (5, 0, 1), 'limited video signal')
    g1.require([r.u(8) for _ in range(3)], [1, 1, 1], 'BT.709 colour')
    g1.require((r.u(1), r.ue(), r.ue()), (1, 0, 0), 'left chroma location')
    g1.require((r.u(1), r.u(32), r.u(32), r.u(1)), (1, 1, 60, 1), 'nominal timing')
    g1.require([r.u(1) for _ in range(4)], [0] * 4, 'HRD and restrictions')
    r.finish()
    r = rbsp(pps)
    g1.require([r.ue(), r.ue(), r.u(1), r.u(1), r.ue(), r.ue(), r.ue(), r.u(1), r.u(2)], [0] * 9, 'PPS coding tools')
    g1.require([r.se() for _ in range(3)], [0] * 3, 'QP offsets')
    g1.require([r.u(1) for _ in range(3)], [1, 1, 0], 'PPS flags')
    r.finish()


class Picture:
    def __init__(self, width, height, tables, previous):
        self.mbw, self.mbh = (width + 15) // 16, (height + 15) // 16
        self.grids = [[None] * (self.mbw * self.mbh * blocks * blocks) for blocks in (4, 2, 2)]
        self.tables = tables
        self.previous = previous
        self.modes = Counter()
        self.counts = []
        self.bits = []
        self.partitions = []

    def context(self, component, x, y):
        stride = self.mbw * (4 if component == 0 else 2)
        grid = self.grids[component]
        neighbours = ([grid[y * stride + x - 1]] if x else []) + ([grid[(y - 1) * stride + x]] if y else [])
        if any(n is None for n in neighbours):
            raise ValueError('coefficient neighbour not yet parsed')
        return (sum(neighbours) + (len(neighbours) == 2)) // len(neighbours) if neighbours else 0

    def block(self, r, component, x, y, maximum, coded):
        stride = self.mbw * (4 if component == 0 else 2)
        self.grids[component][y * stride + x] = residual_block(r, maximum, self.context(component, x, y), self.tables) if coded else 0

    def admit(self, count, mode, bits, subtypes=()):
        bounded(self.previous + count, 0, 16, 'MaxMvsPer2Mb')
        bounded(bits, 0, 3200, 'macroblock_layer bits')
        self.previous = count
        self.counts.append(count)
        self.bits.append(bits)
        self.modes[mode] += 1
        self.partitions.append({'mode': mode, 'subtypes': list(subtypes)})

    def skip(self, address):
        for component, blocks in enumerate((4, 2, 2)):
            stride = self.mbw * blocks
            x, y = address % self.mbw * blocks, address // self.mbw * blocks
            for dy in range(blocks):
                for dx in range(blocks):
                    self.grids[component][(y + dy) * stride + x + dx] = 0
        self.admit(1, 'P_Skip', 0)

    def macroblock(self, r, address, inter):
        start = r.at
        mbtype = bounded(r.ue(), 0, 29 if inter else 24, 'mb_type (I_PCM unsupported)')
        intra = not inter or mbtype >= 5
        t = mbtype - 5 if inter and intra else mbtype
        i16 = intra and t != 0
        count = 0
        subtypes = ()
        if intra:
            mode = 'I_16x16' if i16 else 'I_4x4'
            if not i16:
                for _ in range(16):
                    if not r.u(1):
                        r.u(3)
            bounded(r.ue(), 0, 3, 'intra_chroma_pred_mode')
        else:
            if t in (3, 4):
                subtypes = [bounded(r.ue(), 0, 3, 'sub_mb_type') for _ in range(4)]
                count = sum(SUBCOUNTS[s] for s in subtypes)
                mode = 'P_8x8'
                for s in subtypes:
                    self.modes[('sub_8x8', 'sub_8x4', 'sub_4x8', 'sub_4x4')[s]] += 1
            else:
                count = (1, 2, 2)[t]
                mode = ('P_L0_16x16', 'P_L0_16x8', 'P_L0_8x16')[t]
            for _ in range(count * 2):
                bounded(r.se(), -32768, 32767, 'mvd_l0 syntax in quarter samples')
        if i16:
            cbp = 15 * ((t - 1) // 12) + 16 * (((t - 1) // 4) % 3)
        else:
            cbp = (INTRA_CBP if intra else INTER_CBP)[bounded(r.ue(), 0, 47, 'coded_block_pattern')]
        if cbp or i16:
            g1.require(r.se(), 0, 'fixed mb_qp_delta')
        x, y = address % self.mbw * 4, address // self.mbw * 4
        if i16:
            residual_block(r, 16, self.context(0, x, y), self.tables)
        for block in range(16):
            dx = (block // 4 % 2) * 2 + block % 2
            dy = block // 8 * 2 + block // 2 % 2
            self.block(r, 0, x + dx, y + dy, 15 if i16 else 16, bool(cbp & (1 << (block // 4))))
        for _ in range(2):
            if cbp // 16:
                residual_block(r, 4, -1, self.tables)
        for component in (1, 2):
            for block in range(4):
                self.block(r, component, x // 2 + block % 2, y // 2 + block // 2, 15, cbp // 16 == 2)
        self.admit(count, mode, r.at - start, subtypes)


def inspect(data, width, height, qp, frames, deblocking=False, interval=20):
    if width % 2 or height % 2:
        raise ValueError('4:2:0 geometry must be even')
    bounded(width, 16, 1920, 'width')
    bounded(height, 16, 1080, 'height')
    bounded(qp, 10, 51, 'QP')
    bounded(frames, 1, 120, 'frames')
    bounded(interval, 1, 120, 'IDR interval')
    if len(data) > frames * 1000012:
        raise ValueError('sequence exceeds NAL byte envelope')
    nals = g1.units(data)
    g1.require(len(nals), frames * 3, 'NAL count')
    if any(not n for n in nals):
        raise ValueError('empty NAL')
    tables = vlc_tables()
    pictures, previous = [], 0
    for frame in range(frames):
        sps, pps, slice_nal = nals[frame * 3:frame * 3 + 3]
        idr = frame % interval == 0
        g1.require([n[0] for n in (sps, pps, slice_nal)], [0x67, 0x68, 0x65 if idr else 0x61], 'NAL headers and GOP order')
        bounded(len(sps) + len(pps) + len(slice_nal), 1, 1000000, 'access unit NAL bytes')
        g1.require((sps, pps), tuple(nals[:2]), 'identical parameter bytes')
        if frame == 0:
            parameter_sets(sps, pps, width, height)
        r = rbsp(slice_nal)
        g1.require([r.ue(), r.ue(), r.ue(), r.u(4)], [0, 7 if idr else 5, 0, (frame % interval) % 16], 'slice identity and frame_num')
        if idr:
            g1.require((r.ue(), r.u(1), r.u(1)), ((frame // interval) % 2, 0, 0), 'IDR identity and marking')
        else:
            g1.require((r.u(1), r.u(1), r.u(1)), (0, 0, 0), 'one reference, no list modification, sliding marking')
        g1.require(r.se(), qp - 26, 'fixed slice QP')
        g1.require(r.ue(), 0 if deblocking else 1, 'deblocking control')
        if deblocking:
            g1.require((r.se(), r.se()), (0, 0), 'zero deblocking offsets')
        picture = Picture(width, height, tables, previous)
        address, total = 0, picture.mbw * picture.mbh
        while address < total:
            if not idr:
                run = bounded(r.ue(), 0, total - address, 'mb_skip_run')
                for _ in range(run):
                    picture.skip(address)
                    address += 1
                if address == total:
                    break
            picture.macroblock(r, address, not idr)
            address += 1
        r.finish()
        previous = picture.previous
        pictures.append({'frame': frame, 'idr': idr, 'frame_num': (frame % interval) % 16,
                         'mvcnt': picture.counts, 'macroblock_bits': picture.bits,
                         'partitions': picture.partitions,
                         'modes': dict(picture.modes)})
    return pictures


def luma_sample(sample, xq, yq):
    x, fx = divmod(xq, 4)
    y, fy = divmod(yq, 4)
    taps = (1, -5, 20, 20, -5, 1)

    def horizontal(xx, yy):
        return sum(t * sample(xx + i - 2, yy) for i, t in enumerate(taps))

    def clip(value):
        return max(0, min(255, value))

    g, right, below = sample(x, y), sample(x + 1, y), sample(x, y + 1)
    b = clip((horizontal(x, y) + 16) >> 5)
    h = clip((sum(t * sample(x, y + i - 2) for i, t in enumerate(taps)) + 16) >> 5)
    j = clip((sum(t * horizontal(x, y + i - 2) for i, t in enumerate(taps)) + 512) >> 10)
    m = clip((sum(t * sample(x + 1, y + i - 2) for i, t in enumerate(taps)) + 16) >> 5)
    s = clip((horizontal(x, y + 1) + 16) >> 5)
    pairs = (((g, g), (g, h), (h, h), (below, h)),
             ((g, b), (b, h), (h, j), (h, s)),
             ((b, b), (b, j), (j, j), (j, s)),
             ((right, b), (b, m), (j, m), (m, s)))
    a, b = pairs[fx][fy]
    return (a + b + 1) >> 1


def chroma_sample(sample, xq, yq):
    x, fx = divmod(xq, 8)
    y, fy = divmod(yq, 8)
    return ((8 - fx) * (8 - fy) * sample(x, y) + fx * (8 - fy) * sample(x + 1, y)
            + (8 - fx) * fy * sample(x, y + 1) + fx * fy * sample(x + 1, y + 1) + 32) >> 6


def synthetic(width, height, frames, kind, phase=(0, 0)):
    if kind not in ('still', 'reset', 'integer', 'cropped', 'chroma', 'opposing', 'abrupt', 'phase'):
        raise ValueError('unknown P fixture')
    out = bytearray()
    for frame in range(frames):
        for component in range(3):
            w, h = (width, height) if component == 0 else (width // 2, height // 2)
            reference = [48 + (x * (7 + component * 4) + y * (11 - component * 2)
                               + (x // 3 ^ y // 3) * 13 + component * 37) % 160
                         for y in range(h) for x in range(w)]

            def sample(x, y):
                return reference[max(0, min(h - 1, y)) * w + max(0, min(w - 1, x))]

            for y in range(h):
                for x in range(w):
                    dx, dy = 0, 0
                    if kind in ('integer', 'cropped'):
                        dx, dy = -4 * frame, 4 * frame
                    elif kind == 'phase' and frame:
                        dx, dy = phase[0] - 4, phase[1] - 4
                    elif kind == 'opposing':
                        dx, dy = (4 if x < w // 2 else -4) * frame, (4 if y < h // 2 else -4) * frame
                    elif kind == 'chroma' and component:
                        dx, dy = (component * 3 - 4) * frame, (4 - component) * frame
                    if kind == 'still':
                        value = (72, 104, 152)[component]
                    elif kind == 'abrupt' and frame % 2:
                        value = 255 - sample(x, y)
                    else:
                        value = (luma_sample(sample, x * 4 + dx, y * 4 + dy) if component == 0
                                 else chroma_sample(sample, x * 8 + dx, y * 8 + dy))
                    out.append(value)
    return bytes(out)


def cells():
    base = (('integer', 16, 16, 3, 20, 10), ('cropped', 18, 22, 3, 20, 16),
            ('chroma', 16, 16, 3, 20, 28), ('opposing', 32, 16, 3, 20, 40),
            ('abrupt', 16, 16, 3, 20, 51), ('still', 16, 16, 19, 20, 28),
            ('reset', 16, 16, 7, 3, 28))
    result = []
    for kind, width, height, frames, interval, qp in base:
        for enabled in (False, True):
            result.append(dict(kind=kind, width=width, height=height, frames=frames,
                               interval=interval, qp=qp, deblocking=enabled, phase=(0, 0)))
    for x in range(4):
        for y in range(4):
            for enabled in (False, True):
                result.append(dict(kind='phase', width=16, height=16, frames=2, interval=20,
                                   qp=28, deblocking=enabled, phase=(x, y)))
    return result


def run(command, cwd):
    result = g1.capture(command, cwd, 180)
    if result.stderr or re.search(rb'\b(?:warning|error|fatal|concealing|concealment|corrupted|corruption)\b', result.stdout, re.I):
        raise ValueError('decoder diagnostics are not conforming')
    return result.stdout


def forced_inventory(directory):
    manifest = directory / 'manifest.tsv'
    bounded(manifest.stat().st_size, 1, 8192, 'forced manifest bytes')
    lines = manifest.read_bytes().decode('utf-8').splitlines()
    g1.require(lines[0], 'layout\tqp\tdeblocking', 'forced manifest header')
    expected = {(layout, qp, enabled) for layout in range(8) for qp in QPS for enabled in (0, 1)}
    rows = []
    for line in lines[1:]:
        if not re.fullmatch(r'[0-7]\t(?:10|16|28|40|51)\t[01]', line):
            raise ValueError('invalid forced manifest triple')
        rows.append(tuple(int(value) for value in line.split('\t')))
    if len(rows) != len(set(rows)):
        raise ValueError('duplicate forced manifest triple')
    g1.require(set(rows), expected, 'exact forced manifest inventory')
    names = {'manifest.tsv'}
    entries = []
    for layout, qp, enabled in sorted(expected):
        entry = {'layout': layout, 'qp': qp, 'deblocking': enabled}
        for suffix in ('h264', 'yuv'):
            name = f'forced-{qp}-{enabled}-{layout}.{suffix}'
            path = directory / name
            names.add(name)
            if path.is_symlink() or not path.is_file():
                raise ValueError('missing or nonregular forced fixture')
            if suffix == 'h264':
                bounded(path.stat().st_size, 1, 2000024, 'forced stream bytes')
            else:
                g1.require(path.stat().st_size, 16 * 16 * 3, 'forced reconstruction size')
            entry[suffix] = {'name': name, 'sha256': g1.digest(path), 'bytes': path.stat().st_size}
        entries.append(entry)
    if manifest.is_symlink() or {path.name for path in directory.iterdir()} != names:
        raise ValueError('extra or nonregular forced inventory entry')
    inventory = {'manifest_sha256': g1.digest(manifest), 'entries': entries}
    inventory['sha256'] = hashlib.sha256(json.dumps(inventory, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    return inventory


def fixture_unchanged(directory, entry):
    for suffix in ('h264', 'yuv'):
        record = entry[suffix]
        path = directory / record['name']
        if path.is_symlink() or not path.is_file():
            raise ValueError('forced fixture changed after inventory')
        g1.require(path.stat().st_size, record['bytes'], 'forced fixture unchanged size')
        g1.require(g1.digest(path), record['sha256'], 'forced fixture unchanged bytes')


def forced_layout(syntax, layout):
    mode, subtypes, count = FORCED_LAYOUTS[layout]
    g1.require(len(syntax), 2, 'forced two-picture sequence')
    g1.require(syntax[1]['partitions'], [{'mode': mode, 'subtypes': list(subtypes)}], 'forced emitted partition layout')
    g1.require(syntax[1]['mvcnt'], [count], 'forced emitted MvCnt')


def compare(driver, ffmpeg, jm, raw, output, width, height, qp, frames, deblocking, interval):
    driver, ffmpeg, jm, raw, output = (path.resolve() for path in (driver, ffmpeg, jm, raw, output))
    output.mkdir(parents=True, exist_ok=False)
    stream, recon, own = (output / name for name in ('output.h264', 'encoder.yuv', 'coreav.yuv'))
    run([driver, width, height, qp, frames, raw, stream, recon, own,
         str(deblocking).lower(), interval], output)
    bounded(stream.stat().st_size, 1, frames * 1000012, 'sequence byte envelope')
    syntax = inspect(stream.read_bytes(), width, height, qp, frames, deblocking, interval)
    (output / 'syntax.json').write_text(json.dumps(syntax, indent=2) + '\n')
    size = width * height * 3 // 2 * frames
    g1.same_planes(recon, own, size)
    decode_oracles(ffmpeg, jm, stream, recon, output, width, height, frames)
    modes = Counter()
    for picture in syntax:
        modes.update(picture['modes'])
    return dict(width=width, height=height, frames=frames, qp=qp, interval=interval,
                deblocking=deblocking, modes=dict(modes), input_sha256=g1.digest(raw),
                stream_sha256=g1.digest(stream), reconstruction_sha256=g1.digest(recon),
                bytes=stream.stat().st_size, conforming=True)


def decode_oracles(ffmpeg, jm, stream, recon, output, width, height, frames):
    size = width * height * 3 // 2 * frames
    probe = json.loads(run([ffmpeg.with_name('ffprobe'), '-v', 'warning', '-select_streams', 'v:0',
                            '-count_frames', '-show_entries',
                            'stream=width,height,pix_fmt,color_range,color_space,color_transfer,color_primaries,chroma_location,profile,level,nb_read_frames',
                            '-of', 'json', stream], output))
    g1.require(len(probe['streams']), 1, 'one oracle stream')
    info = probe['streams'][0]
    pixel_format = g1.native_format(info, width, height, False)
    g1.require(pixel_format, '+yuv420p', 'limited native format')
    for field, expected in (('color_space', 'bt709'), ('color_transfer', 'bt709'),
                            ('color_primaries', 'bt709'), ('chroma_location', 'left'),
                            ('profile', 'Constrained Baseline'), ('level', 51), ('nb_read_frames', str(frames))):
        g1.require(info.get(field), expected, f'oracle {field}')
    oracle = output / 'ffmpeg.yuv'
    run([ffmpeg, '-nostdin', '-hide_banner', '-loglevel', 'warning', '-xerror', '-err_detect', 'explode',
         '-hwaccel', 'none', '-threads', '1', '-c:v', 'h264', '-i', stream,
         '-map', '0:v:0', '-an', '-sn', '-dn', '-c:v', 'rawvideo', '-pix_fmt', pixel_format,
         '-fps_mode', 'passthrough', '-f', 'rawvideo', oracle], output)
    g1.same_planes(recon, oracle, size)
    reference = output / 'jm.yuv'
    log = run([jm, '-i', stream, '-o', reference], output)
    (output / 'jm.log').write_bytes(log)
    g1.same_planes(recon, reference, size)


def compare_forced(decoder, ffmpeg, jm, directory, entry, output):
    output.mkdir(parents=True, exist_ok=False)
    fixture_unchanged(directory, entry)
    stream, recon = (directory / entry[suffix]['name'] for suffix in ('h264', 'yuv'))
    syntax = inspect(stream.read_bytes(), 16, 16, entry['qp'], 2, bool(entry['deblocking']), 32)
    (output / 'syntax.json').write_text(json.dumps(syntax, indent=2) + '\n')
    forced_layout(syntax, entry['layout'])
    own = output / 'coreav.yuv'
    with own.open('wb') as sink:
        result = g1.capture([decoder, '--kernels', 'scalar', '--threads', '1', stream], output,
                            180, stdout_file=sink)
    if result.stderr:
        raise ValueError('decoder diagnostics are not conforming')
    g1.same_planes(recon, own, 16 * 16 * 3)
    decode_oracles(ffmpeg, jm, stream, recon, output, 16, 16, 2)
    fixture_unchanged(directory, entry)
    modes = Counter()
    for picture in syntax:
        modes.update(picture['modes'])
    return dict(layout=entry['layout'], qp=entry['qp'], deblocking=entry['deblocking'],
                modes=dict(modes), stream_sha256=entry['h264']['sha256'],
                reconstruction_sha256=entry['yuv']['sha256'], conforming=True)


def verify_tools(driver, ffmpeg, jm, decoder):
    paths = (('driver', driver), ('ffmpeg', ffmpeg), ('ffprobe', ffmpeg.with_name('ffprobe')),
             ('jm', jm), ('decoder', decoder))
    for _, path in paths:
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError('required executable missing or not executable')
    problems = list(g1.oracle_pin.verify_bin(ffmpeg.parent))
    if problems:
        raise ValueError('; '.join(problems))
    pin = dict(line.split('=', 1) for line in (ROOT / 'scripts/jm-decoder.env').read_text().splitlines() if line)
    receipt = (jm.parent / 'jm-build.txt').read_text()
    for line in (f'JM_VERSION={pin["JM_VERSION"]}', f'JM_SHA256={pin["JM_SHA256"]}',
                 "BUILD_COMMAND=make -C JM/ldecod -j2 CC='cc -fcommon'"):
        if line not in receipt.splitlines():
            raise ValueError('JM build receipt does not match pin')
    if not any(re.fullmatch(re.escape(g1.digest(jm)) + r'  .*/ldecod', line) for line in receipt.splitlines()):
        raise ValueError('JM executable hash differs from build receipt')
    return {'executables': {name: {'sha256': g1.digest(path)} for name, path in paths},
            'ffmpeg_pin': g1.oracle_pin.load(), 'jm_pin': pin, 'jm_build': receipt,
            'vlc_sha256': g1.digest(ROOT / 'crates/coreav-h264-encoder/src/vlc.rs')}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'decoder', 'ffmpeg', 'jm', 'forced-fixtures', 'output'):
        parser.add_argument('--' + name, type=pathlib.Path, required=True)
    args = parser.parse_args(argv)
    driver, decoder, ffmpeg, jm, fixtures, out = (path.resolve() for path in
        (args.driver, args.decoder, args.ffmpeg, args.jm, args.forced_fixtures, args.output))
    out.mkdir(parents=True, exist_ok=False)
    roster = cells()
    report = {'complete': False, 'expected_cells': len(roster), 'expected_forced_cells': 80,
              'roster': roster, 'tools': {}, 'cells': [], 'forced_cells': [], 'forced_inventory': {},
              'unselected_modes': list(MODES), 'limitations': [
                  'Derived motion-vector prediction and bounds are covered by Rust fixtures, not this syntax reader.',
                  'Generated fractional inputs do not prove that the encoder selected each interpolation phase.',
                  'The registered QP sample is not an all-QP P-picture sweep.']}
    try:
        if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'linux':
            raise ValueError('media conformance runs only in Linux Actions, not on developer machines')
        report['tools'] = verify_tools(driver, ffmpeg, jm, decoder)
        inventory = forced_inventory(fixtures)
        report['forced_inventory'] = inventory
        selected = Counter()
        for i, cell in enumerate(roster):
            raw = out / f'{i:03}-{cell["kind"]}.yuv'
            raw.write_bytes(synthetic(cell['width'], cell['height'], cell['frames'], cell['kind'], cell['phase']))
            result = compare(driver, ffmpeg, jm, raw, out / f'{raw.stem}-decoded',
                             **{key: cell[key] for key in ('width', 'height', 'qp', 'frames', 'deblocking', 'interval')})
            result.update(kind=cell['kind'], phase=cell['phase'])
            report['cells'].append(result)
            selected.update(result['modes'])
            report['unselected_modes'] = [mode for mode in MODES if not selected[mode]]
        g1.require(len(report['cells']), len(roster), 'complete P matrix')
        for mode in ('P_Skip', 'P_L0_16x16'):
            if not selected[mode]:
                raise ValueError(f'missing automatically selected witness {mode}')
        report['selected_modes'] = dict(selected)
        covered = selected.copy()
        for entry in inventory['entries']:
            name = f'forced-{entry["qp"]}-{entry["deblocking"]}-{entry["layout"]}'
            result = compare_forced(decoder, ffmpeg, jm, fixtures, entry, out / name)
            report['forced_cells'].append(result)
            covered.update(result['modes'])
        g1.require(len(report['forced_cells']), 80, 'complete forced matrix')
        g1.require(forced_inventory(fixtures), inventory, 'unchanged complete forced inventory')
        report['covered_modes'] = dict(covered)
        report['uncovered_partition_modes'] = [mode for mode in MODES[2:] if not covered[mode]]
        g1.require(report['uncovered_partition_modes'], [], 'all partition and subtype witnesses decoded')
        report['complete'] = True
    finally:
        (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['cells'])} generated and {len(report['forced_cells'])} forced P conformance cells, all three decoders match")


if __name__ == '__main__':
    main()
