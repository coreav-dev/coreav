#!/usr/bin/env python3
"""Synthetic and existing-fixture G1 conformance. No quality or timing work."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import oracle_pin


def digest(path):
    h = hashlib.sha256()
    with pathlib.Path(path).open('rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def capture(command, cwd, timeout, record=None):
    command = list(map(str, command))
    path = pathlib.Path(cwd)
    result = None
    timed_out = False
    try:
        result = subprocess.run(command, cwd=cwd, check=False, capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired as error:
        result = error
        timed_out = True
        raise
    finally:
        if result is not None:
            name = record or f'process-{len(list(path.glob("process-*.json"))):03}.json'
            (path / name).write_text(json.dumps({'argv': command, 'exit_code': getattr(result, 'returncode', None),
                                                'timed_out': timed_out,
                                                'stdout': (result.stdout or b'').decode(errors='replace'),
                                                'stderr': (result.stderr or b'').decode(errors='replace')}, indent=2) + '\n')
    if result.returncode:
        sys.stderr.write(result.stderr.decode(errors='replace'))
    result.check_returncode()
    return result


def run(command, cwd):
    return capture(command, cwd, 180).stdout


class Bits:
    def __init__(self, data):
        self.data = data
        self.at = 0

    def u(self, n):
        if n < 0 or self.at + n > len(self.data) * 8:
            raise ValueError('truncated bitstream')
        v = 0
        for _ in range(n):
            v = v * 2 + ((self.data[self.at // 8] >> (7 - self.at % 8)) & 1)
            self.at += 1
        return v

    def ue(self):
        n = 0
        while self.u(1) == 0:
            n += 1
            if n > 31:
                raise ValueError('overlong Exp-Golomb')
        return (1 << n) - 1 + self.u(n)

    def se(self):
        v = self.ue()
        return (v + 1) // 2 if v & 1 else -v // 2


def units(data):
    if not data.startswith(b'\0\0\0\1'):
        raise ValueError('driver must write four-byte Annex B delimiters')
    return data[4:].split(b'\0\0\0\1')


def rbsp(nal):
    out = bytearray()
    zeros = 0
    for b in nal[1:]:
        if zeros == 2 and b == 3:
            zeros = 0
            continue
        out.append(b)
        zeros = zeros + 1 if b == 0 else 0
    return Bits(out)


def require(value, expected, name):
    if value != expected:
        raise ValueError(f'{name}: expected {expected!r}, got {value!r}')


def inspect(data, width, height, qp, frames, colour=(False, 1, 1, 1, 0)):
    nals = units(data)
    require(len(nals), frames * 3, 'NAL count')
    for frame in range(frames):
        sps, pps, idr = nals[frame * 3:frame * 3 + 3]
        require([n[0] for n in (sps, pps, idr)], [0x67, 0x68, 0x65], 'NAL headers')
        r = rbsp(sps)
        require((r.u(8), r.u(8), r.u(8)), (66, 0xc0, 51), 'profile constraints level')
        require([r.ue() for _ in range(4)], [0, 0, 2, 1], 'SPS fixed fields')
        require(r.u(1), 0, 'frame number gaps')
        coded_width, coded_height = (r.ue() + 1) * 16, (r.ue() + 1) * 16
        require((coded_width, coded_height), ((width + 15) // 16 * 16, (height + 15) // 16 * 16), 'coded geometry')
        require((r.u(1), r.u(1)), (1, 1), 'progressive frame')
        crop = [r.ue() for _ in range(4)] if r.u(1) else [0] * 4
        require(crop, [0, (coded_width - width) // 2, 0, (coded_height - height) // 2], 'crop')
        require([r.u(1) for _ in range(4)], [1, 0, 0, 1], 'VUI presence')
        require(r.u(3), 5, 'video format')
        require(bool(r.u(1)), colour[0], 'range')
        require(r.u(1), 1, 'colour description')
        require(tuple(r.u(8) for _ in range(3)), colour[1:4], 'colour')
        require(r.u(1), 1, 'chroma location present')
        require((r.ue(), r.ue()), (colour[4], colour[4]), 'chroma location')
        require(r.u(1), 1, 'timing present')
        require((r.u(32), r.u(32), r.u(1)), (1, 60, 1), 'nominal timing')
        require([r.u(1) for _ in range(4)], [0, 0, 0, 0], 'HRD and restrictions')
        r = rbsp(pps)
        require([r.ue(), r.ue(), r.u(1), r.u(1), r.ue(), r.ue(), r.ue(), r.u(1), r.u(2)], [0] * 9, 'PPS coding tools')
        require([r.se() for _ in range(3)], [0, 0, 0], 'QP offsets')
        require([r.u(1) for _ in range(3)], [1, 1, 0], 'PPS flags')
        r = rbsp(idr)
        require([r.ue(), r.ue(), r.ue(), r.u(4), r.ue(), r.u(1), r.u(1), r.se(), r.ue()], [0, 7, 0, 0, frame % 2, 0, 0, qp - 26, 1], 'IDR slice header')


def same_planes(expected, actual, size):
    a, b = pathlib.Path(expected), pathlib.Path(actual)
    require((a.stat().st_size, b.stat().st_size), (size, size), 'complete visible planes')
    require(digest(a), digest(b), 'reconstruction SHA-256')


def native_format(info, width, height, full_range):
    require((info['width'], info['height']), (width, height), 'oracle visible dimensions')
    require(info['color_range'], 'pc' if full_range else 'tv', 'oracle signalled range')
    if info['pix_fmt'] not in ('yuv420p', 'yuvj420p'):
        raise ValueError('oracle output is not native 8-bit planar 4:2:0')
    return '+' + info['pix_fmt']


def inspect_anchor(data, width, height, qp, frames):
    markers = list(re.finditer(b'\x00\x00\x00\x01|\x00\x00\x01', data))
    if not markers or markers[0].start() != 0:
        raise ValueError('invalid baseline Annex B')
    nals = [data[m.end():markers[i + 1].start() if i + 1 < len(markers) else len(data)] for i, m in enumerate(markers)]
    require([n[0] & 31 for n in nals], [7, 8, 5] * frames, 'baseline repeated SPS PPS IDR')
    for at in range(0, len(nals), 3):
        sps, pps, idr = nals[at:at + 3]
        r = rbsp(sps)
        require(r.u(8), 66, 'baseline profile')
        require(r.u(8) & 0xc0, 0xc0, 'baseline constraints')
        require(r.u(8), 51, 'baseline level')
        require(r.ue(), 0, 'baseline SPS id')
        log_frame = r.ue() + 4
        require(r.ue(), 2, 'baseline POC type')
        refs = r.ue()
        if refs > 1:
            raise ValueError('baseline reference limit')
        require(r.u(1), 0, 'baseline gaps')
        cw, ch = (r.ue() + 1) * 16, (r.ue() + 1) * 16
        require((r.u(1), r.u(1)), (1, 1), 'baseline progressive frame')
        crop = [r.ue() for _ in range(4)] if r.u(1) else [0] * 4
        require((cw - 2 * (crop[0] + crop[1]), ch - 2 * (crop[2] + crop[3])), (width, height), 'baseline visible crop')
        require(r.u(1), 1, 'baseline VUI')
        if r.u(1):
            aspect = r.u(8)
            if aspect == 255:
                r.u(16)
                r.u(16)
        if r.u(1):
            r.u(1)
        require(r.u(1), 1, 'baseline video signal')
        r.u(3)
        require((r.u(1), r.u(1)), (0, 1), 'baseline limited range and colour')
        require(tuple(r.u(8) for _ in range(3)), (1, 1, 1), 'baseline BT.709')
        if r.u(1):
            require((r.ue(), r.ue()), (0, 0), 'baseline chroma location')
        require(r.u(1), 1, 'baseline timing present')
        require((r.u(32), r.u(32), r.u(1)), (1, 60, 1), 'baseline nominal timing')
        require((r.u(1), r.u(1)), (0, 0), 'baseline no HRD')
        r = rbsp(pps)
        require([r.ue(), r.ue(), r.u(1), r.u(1), r.ue(), r.ue(), r.ue(), r.u(1), r.u(2)], [0] * 9, 'baseline coding tools')
        initial = 26 + r.se()
        r.se()
        require(r.se(), 0, 'baseline chroma QP offset')
        require(r.u(1), 1, 'baseline deblocking control')
        r.u(1)
        require(r.u(1), 0, 'baseline redundant slices')
        r = rbsp(idr)
        require(r.ue(), 0, 'baseline first macroblock')
        require(r.ue() % 5, 2, 'baseline I slice')
        require(r.ue(), 0, 'baseline PPS id')
        require(r.u(log_frame), 0, 'baseline frame number')
        r.ue()
        require((r.u(1), r.u(1)), (0, 0), 'baseline IDR marking')
        require(initial + r.se(), qp, 'baseline fixed slice QP')
        require(r.ue(), 1, 'baseline deblocking disabled')


def compare(driver, ffmpeg, jm, raw, output, width, height, qp, frames, colour=(False, 1, 1, 1, 0)):
    driver, ffmpeg, jm, raw, output = (path.resolve() for path in (driver, ffmpeg, jm, raw, output))
    output.mkdir(parents=True, exist_ok=False)
    stream, recon, own = (output / n for n in ('output.h264', 'encoder.yuv', 'coreav.yuv'))
    run([driver, width, height, qp, frames, raw, stream, recon, own,
         str(colour[0]).lower(), *colour[1:]], cwd=output)
    inspect(stream.read_bytes(), width, height, qp, frames, colour)
    size = width * height * 3 // 2 * frames
    same_planes(recon, own, size)
    oracle = output / 'ffmpeg.yuv'
    probe = json.loads(run([ffmpeg.with_name('ffprobe'), '-v', 'error', '-select_streams', 'v:0',
                            '-show_entries', 'stream=width,height,pix_fmt,color_range', '-of', 'json', stream], cwd=output))
    require(len(probe['streams']), 1, 'one oracle video stream')
    pixel_format = native_format(probe['streams'][0], width, height, colour[0])
    run([ffmpeg, '-nostdin', '-hide_banner', '-loglevel', 'error', '-xerror', '-err_detect', 'explode',
         '-hwaccel', 'none', '-threads', '1', '-c:v', 'h264', '-i', stream,
         '-map', '0:v:0', '-an', '-sn', '-dn', '-c:v', 'rawvideo', '-pix_fmt', pixel_format,
         '-fps_mode', 'passthrough', '-f', 'rawvideo', oracle], cwd=output)
    same_planes(recon, oracle, size)
    reference = output / 'jm.yuv'
    log = run([jm, '-i', stream.resolve(), '-o', reference.resolve()], cwd=output)
    (output / 'jm.log').write_bytes(log)
    same_planes(recon, reference, size)
    return {'width': width, 'height': height, 'frames': frames, 'qp': qp,
            'stream_sha256': digest(stream), 'reconstruction_sha256': digest(recon),
            'bytes': stream.stat().st_size, 'conforming': True}


def synthetic(width, height, frames, kind):
    out = bytearray()
    for frame in range(frames):
        for c in range(3):
            w, h = (width, height) if c == 0 else (width // 2, height // 2)
            for y in range(h):
                for x in range(w):
                    if kind == 'flat':
                        v = (16, 128, 240)[(frame + c) % 3]
                    elif kind == 'edges':
                        v = 16 + 219 * (((x + frame) // 4 + (y // 8) + c) % 2)
                    elif kind == 'impulses':
                        v = 235 if x == (frame * 3 + c) % w and y == (frame * 5 + c) % h else 64 + c * 40
                    else:
                        v = 16 + ((x * 3 + y * 5 + frame * 17 + c * 31) % 220)
                    out.append(v)
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--driver', type=pathlib.Path, required=True)
    p.add_argument('--ffmpeg', type=pathlib.Path, required=True)
    p.add_argument('--jm', type=pathlib.Path, required=True)
    p.add_argument('--output', type=pathlib.Path, required=True)
    p.add_argument('--fixtures', type=pathlib.Path, help='existing generated non-judged fixture directory')
    args = p.parse_args()
    args.driver = args.driver.resolve()
    args.ffmpeg = args.ffmpeg.resolve()
    args.jm = args.jm.resolve()
    if args.fixtures:
        args.fixtures = args.fixtures.resolve()
    if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'linux':
        raise ValueError('media conformance runs only in Linux Actions, not on developer machines')
    problems = list(oracle_pin.verify_bin(args.ffmpeg.parent))
    if problems:
        raise ValueError('; '.join(problems))
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {'tools': {n: {'sha256': digest(v)} for n, v in [('driver', args.driver), ('ffmpeg', args.ffmpeg), ('jm', args.jm)]}, 'cells': [], 'complete': False}
    try:
        for kind in ('flat', 'gradient', 'edges', 'impulses'):
            for width, height in [(16, 16), (18, 22), (48, 34)]:
                raw = out / f'{kind}-{width}-{height}.yuv'
                raw.write_bytes(synthetic(width, height, 3, kind))
                for qp in (10, 11, 12, 13, 14, 15, 16, 22, 28, 34, 40, 51):
                    report['cells'].append(compare(args.driver.resolve(), args.ffmpeg.resolve(), args.jm.resolve(), raw, out / f'{raw.stem}-q{qp}', width, height, qp, 3))
        if args.fixtures:
            fixture = args.fixtures / 'basic.mp4'
            if not fixture.is_file():
                raise ValueError('required existing basic.mp4 fixture is missing')
            raw = out / 'existing-fixture.yuv'
            run([args.ffmpeg, '-nostdin', '-v', 'error', '-xerror', '-hwaccel', 'none', '-threads', '1',
                 '-i', fixture, '-frames:v', '3', '-an', '-pix_fmt', '+yuv420p', '-f', 'rawvideo', raw], cwd=out)
            probe = json.loads(run([args.ffmpeg.with_name('ffprobe'), '-v', 'error', '-select_streams', 'v:0', '-show_entries', 'stream=width,height', '-of', 'json', fixture], cwd=out))['streams'][0]
            for qp in (16, 22, 28, 34, 40):
                report['cells'].append(compare(args.driver.resolve(), args.ffmpeg.resolve(), args.jm.resolve(), raw, out / f'existing-q{qp}', probe['width'], probe['height'], qp, 3))
        raw = out / 'declared-metadata.yuv'
        raw.write_bytes(synthetic(18, 22, 3, 'gradient'))
        report['cells'].append(compare(args.driver.resolve(), args.ffmpeg.resolve(), args.jm.resolve(), raw,
                                       out / 'declared-metadata', 18, 22, 28, 3, (True, 6, 6, 6, 5)))
        report['complete'] = True
    finally:
        (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['cells'])} conformance cells, all three decoders match")


if __name__ == '__main__':
    main()
