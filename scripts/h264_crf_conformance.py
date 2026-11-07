#!/usr/bin/env python3
"""Generated constant-quality pictures through the existing three-decoder path."""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h264_p_conformance as p


def expected_qps(raw, width, height, frames, crf, interval):
    size = width * height * 3 // 2
    p.g1.require(len(raw), size * frames, 'complete CRF input')
    qps = []
    for frame in range(frames):
        plane = raw[frame * size:frame * size + width * height]
        sad = 0
        for by in range(0, height, 8):
            for bx in range(0, width, 8):
                block = [plane[y * width + x] for y in range(by, min(by + 8, height))
                         for x in range(bx, min(bx + 8, width))]
                mean = sum(block) // len(block)
                sad += sum(abs(value - mean) for value in block)
        activity = sad // (width * height)
        correction = sum(activity >= threshold for threshold in (8, 12, 24, 48)) - 2
        qps.append(min(51, max(10, crf + correction - int(frame % interval == 0))))
    return qps


def pictures(width, height, frames):
    raw = bytearray()
    for frame in range(frames):
        amplitude = (0, 9, 18, 30, 50)[frame % 5]
        for scale in (1, 2, 2):
            for y in range(height // scale):
                for x in range(width // scale):
                    raw.append(128 + amplitude * (1 if x % 2 else -1))
    return raw


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'decoder', 'ffmpeg', 'jm', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    tools, receipt = p.verify_tools(args.driver, args.ffmpeg, args.jm, args.decoder)
    args.output.mkdir(parents=True, exist_ok=False)
    rows = []
    for width, height, frames, interval in ((18, 34, 6, 3), (32, 80, 18, 32), (32, 32, 5, 1)):
        raw = pictures(width, height, frames)
        source = (args.output / f'{width}-{height}.yuv').resolve()
        source.write_bytes(raw)
        for crf in (10, 18, 23, 28, 51):
            for deblocking in (False, True):
                case = (args.output / f'{width}-{height}-{crf}-{deblocking}').resolve()
                case.mkdir()
                stream, recon, own = (case / name for name in ('out.h264', 'recon.yuv', 'coreav.yuv'))
                p.run([args.driver.resolve(), width, height, f'crf:{crf}', frames, source,
                       stream, recon, own, str(deblocking).lower(), interval], case)
                qps = expected_qps(raw, width, height, frames, crf, interval)
                syntax = p.inspect(stream.read_bytes(), width, height, crf, frames,
                                   deblocking, interval, picture_qps=qps)
                p.g1.same_planes(recon, own, len(raw))
                p.decode_oracles(tools, args.jm.resolve(), stream, recon, case, width, height, frames)
                (case / 'syntax.json').write_text(json.dumps(syntax, indent=2) + '\n')
                rows.append(dict(width=width, height=height, crf=crf, frames=frames,
                                 interval=interval, deblocking=deblocking, qps=qps,
                                 stream_sha256=p.g1.digest(stream), conforming=True))
    (args.output / 'summary.json').write_text(json.dumps(dict(tools=receipt, cases=rows), indent=2) + '\n')
    print(f'CRF conformance: {len(rows)} cases, {sum(row["frames"] for row in rows)} pictures')


if __name__ == '__main__':
    main()
