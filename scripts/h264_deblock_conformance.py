#!/usr/bin/env python3
"""Untimed synthetic opt-in H.264 deblocking conformance in Linux Actions."""
import argparse
import json
import os
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import h264_g1_conformance as g1


QPS = tuple(range(10, 52))
FRAMES = 3
COLOUR = (False, 1, 1, 1, 0)
CASES = (('gradient', 18, 22), ('vertical', 34, 18),
         ('horizontal', 18, 34), ('crossed', 32, 32))
BASELINE = {
    'input_sha256': 'e8f5e52209d52d33ed84051982282de1003f25cd60c5ae11959e9337efb94387',
    'stream_sha256': '1808185d05b060ba833902740a4b67f19d36253b0bc48610c6047c1235e26708',
    'reconstruction_sha256': '9c1f508de55ff166ad1efa68887c7808abf472ded97c318f603ab415b7ae1997',
}


def synthetic(width, height, kind):
    if kind == 'gradient':
        return g1.synthetic(width, height, FRAMES, kind)
    if kind not in ('vertical', 'horizontal', 'crossed'):
        raise ValueError('unknown deblocking fixture')
    out = bytearray()
    for frame in range(FRAMES):
        for c in range(3):
            w, h = (width, height) if c == 0 else (width // 2, height // 2)
            for y in range(h):
                for x in range(w):
                    if kind == 'crossed':
                        if c == 0:
                            v = 48 + (x * 7 + y * 11 + frame * 13) % 160
                        elif c == 1:
                            v = 64 + (x * 5 + y * 3 + frame * 17) % 96
                        else:
                            v = 112 + (x * 3 + y * 7 + frame * 11) % 96
                    else:
                        along, across = (x, y) if kind == 'vertical' else (y, x)
                        if c == 0:
                            v = 48 + 3 * (along // 4) + frame * 2
                        elif c == 1:
                            v = 72 + 5 * (across // 4) + frame * 3
                        else:
                            v = 144 + 7 * (along // 4) + frame * 5
                    out.append(v)
    return out


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--driver', type=pathlib.Path, required=True)
    p.add_argument('--ffmpeg', type=pathlib.Path, required=True)
    p.add_argument('--jm', type=pathlib.Path, required=True)
    p.add_argument('--output', type=pathlib.Path, required=True)
    args = p.parse_args(argv)
    driver, ffmpeg, jm, out = (path.resolve() for path in
                               (args.driver, args.ffmpeg, args.jm, args.output))
    out.mkdir(parents=True, exist_ok=False)
    report = {'complete': False, 'expected_cells': len(CASES) * len(QPS),
              'tools': {}, 'disabled_baseline': None, 'witness': None, 'cells': []}
    try:
        if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'linux':
            raise ValueError('media conformance runs only in Linux Actions, not on developer machines')
        problems = list(g1.oracle_pin.verify_bin(ffmpeg.parent))
        if problems:
            raise ValueError('; '.join(problems))
        report['tools'] = {name: {'sha256': g1.digest(path)} for name, path in
                           (('driver', driver), ('ffmpeg', ffmpeg),
                            ('ffprobe', ffmpeg.with_name('ffprobe')), ('jm', jm))}
        raw = out / 'disabled-gradient.yuv'
        raw.write_bytes(g1.synthetic(18, 22, FRAMES, 'gradient'))
        baseline = {'input_sha256': g1.digest(raw)}
        report['disabled_baseline'] = baseline
        g1.require(baseline['input_sha256'], BASELINE['input_sha256'], 'disabled input SHA-256')
        baseline.update(g1.compare(driver, ffmpeg, jm, raw, out / 'disabled-baseline',
                                   18, 22, 28, FRAMES, colour=COLOUR))
        for field, expected in BASELINE.items():
            g1.require(baseline[field], expected, f'disabled baseline {field}')
        for kind, width, height in CASES:
            raw = out / f'{kind}-{width}x{height}.yuv'
            raw.write_bytes(synthetic(width, height, kind))
            input_sha256 = g1.digest(raw)
            for qp in QPS:
                cell = g1.compare(driver, ffmpeg, jm, raw, out / f'{raw.stem}-q{qp}',
                                  width, height, qp, FRAMES, colour=COLOUR, deblocking=True)
                cell.update(fixture=kind, input_sha256=input_sha256, deblocking=True)
                report['cells'].append(cell)
                if (kind, width, height, qp) == ('gradient', 18, 22, 28):
                    g1.require(input_sha256, baseline['input_sha256'], 'witness identical input')
                    report['witness'] = {
                        'fixture': kind, 'qp': qp,
                        'disabled_sha256': baseline['reconstruction_sha256'],
                        'enabled_sha256': cell['reconstruction_sha256'],
                        'changed': cell['reconstruction_sha256'] != baseline['reconstruction_sha256'],
                    }
                    g1.require(report['witness']['changed'], True, 'enabled filter changes reconstructed samples')
        g1.require(len(report['cells']), report['expected_cells'], 'complete enabled matrix')
        if report['witness'] is None:
            raise ValueError('missing enabled-filter reconstruction witness')
        report['complete'] = True
    finally:
        (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['cells'])} enabled conformance cells, all three decoders match; filter witness changed")


if __name__ == '__main__':
    main()
