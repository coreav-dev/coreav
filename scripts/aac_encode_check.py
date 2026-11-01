#!/usr/bin/env python3
"""The AAC bars of docs/design/transcode.md ("AAC bars"): coreav transcode
--audio aac against FFmpeg's -c:a aac on the Opus bars' six sources, made
the same way (scripts/opus_encode_check.py), judged on A1 to A4 as the note
fixes them. Speed is recorded for information only.

    aac_encode_check.py --coreav BIN --ffmpeg FFMPEG --ffprobe FFPROBE \
        --work DIR --out REPORT.json

Exits 1 when a bar is missed, 2 when the set cannot be made. Standard
library only; FFmpeg is a black box.
"""

import argparse
import json
import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import opus_encode_check as opus  # noqa: E402

RATES = [None, 64, 192]
# The sources A4 judges: the tonal ones, at 48 and 44.1 kHz.
TONAL = {'S1', 'S2', 'S3', 'S6'}


def arms(coreav, ffmpeg, source, ours, theirs, rate):
    """The two command lines of one input, as the note fixes them: CoreAV
    with `--audio aac` and FFmpeg with `-c:a aac`, each with the rate
    (kbit/s) when one is given."""
    ours_args = [coreav, 'transcode', source, ours, '--force', '--audio', 'aac',
                 '--encoder', 'ffmpeg', '--ffmpeg', ffmpeg]
    theirs_args = [ffmpeg, '-nostdin', '-v', 'error', '-y', '-i', source,
                   '-c:v', 'libx264', '-c:a', 'aac']
    if rate:
        ours_args += ['--audio-bitrate', str(rate)]
        theirs_args += ['-b:a', f'{rate}k']
    return ours_args, theirs_args + [theirs]


def timeline(ours, theirs, summary, source_rate):
    """A2 on one input: AAC-LC at the source's rate, presented from 0, as
    many samples as the summary and the FFmpeg arm give."""
    return (
        ours.get('codec_name') == 'aac'
        and ours.get('profile') == 'LC'
        and ours.get('sample_rate') == source_rate
        and ours.get('start_pts') == 0
        and ours.get('duration_ts') == summary['samples']
        and ours.get('duration_ts') == theirs.get('duration_ts')
    )


def a4(ours, theirs):
    """A4 on one input; a missing SNR on either arm fails it."""
    return ours is not None and theirs is not None and ours >= theirs - 0.5


def judge(rows):
    """The bars over every row; an input that failed misses all four."""
    judged = [r for r in rows if not r['failed']]
    ratios = [r['bit_rate_ratio'] for r in judged]
    whole = bool(rows) and len(judged) == len(rows)
    return {
        'A1': whole and all(r['a1'] for r in judged),
        'A2': whole and all(r['a2'] for r in judged),
        'A3': (whole and all(0.95 <= x <= 1.05 for x in ratios)
               and 0.98 <= statistics.median(ratios) <= 1.02),
        'A4': whole and all(r['a4'] for r in judged if r['tonal']),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--coreav', required=True)
    parser.add_argument('--ffmpeg', required=True)
    parser.add_argument('--ffprobe', required=True)
    parser.add_argument('--work', required=True)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    os.makedirs(args.work, exist_ok=True)
    sources = opus.make_sources(args.ffmpeg, args.work)
    references = {name: opus.pcm(args.ffmpeg, sources[name]) for name in TONAL}
    rows = []
    for name, *_ in opus.SOURCES:
        source_rate = opus.probe_audio(args.ffprobe, sources[name]).get('sample_rate')
        tonal = name in TONAL
        for rate in RATES:
            label = f'{name}-{rate or "default"}'
            ours = os.path.join(args.work, f'{label}.coreav.mp4')
            theirs = os.path.join(args.work, f'{label}.ffmpeg.mp4')
            ours_args, theirs_args = arms(args.coreav, args.ffmpeg, sources[name], ours, theirs, rate)
            times = {'coreav': [], 'ffmpeg': []}
            summary = None
            failed = None
            for _ in range(opus.RUNS):
                done, took = opus.timed(ours_args)
                times['coreav'].append(took)
                if done.returncode != 0:
                    failed = done.stderr.decode(errors='replace')
                    break
                summary = json.loads(done.stdout)
                done, took = opus.timed(theirs_args)
                times['ffmpeg'].append(took)
                if done.returncode != 0:
                    failed = 'ffmpeg: ' + done.stderr.decode(errors='replace')
                    break
            row = {'input': label, 'tonal': tonal, 'failed': failed}
            rows.append(row)
            if failed:
                continue
            encoded = summary['encoded_audio']
            decode = opus.run([args.ffmpeg, '-nostdin', '-v', 'error', '-i', ours, '-map', '0:a',
                               '-f', 'null', '-'])
            ours_probe = opus.probe_audio(args.ffprobe, ours)
            theirs_probe = opus.probe_audio(args.ffprobe, theirs)
            row.update({
                'summary': encoded,
                'coreav': ours_probe,
                'ffmpeg': theirs_probe,
                'a1': decode.returncode == 0 and not decode.stderr,
                'a1_messages': decode.stderr.decode(errors='replace'),
                'a2': timeline(ours_probe, theirs_probe, encoded, source_rate),
                'bit_rate_ratio': int(ours_probe.get('bit_rate', 0)) / max(1, int(theirs_probe.get('bit_rate', 1))),
                'seconds': {arm: statistics.median(t) for arm, t in times.items()},
            })
            if tonal:
                row['snr_coreav'] = opus.quality(references[name], opus.pcm(args.ffmpeg, ours))
                row['snr_ffmpeg'] = opus.quality(references[name], opus.pcm(args.ffmpeg, theirs))
                row['a4'] = a4(row['snr_coreav'], row['snr_ffmpeg'])
    judged = [r for r in rows if not r['failed']]
    ratios = [r['bit_rate_ratio'] for r in judged]
    bars = judge(rows)
    report = {
        'inputs': len(rows),
        'failed': [r['input'] for r in rows if r['failed']],
        'a1_met': sum(1 for r in judged if r['a1']),
        'a2_met': sum(1 for r in judged if r['a2']),
        'bit_rate_ratio': {'min': min(ratios, default=None), 'median': statistics.median(ratios) if ratios else None,
                           'max': max(ratios, default=None)},
        'snr_delta_db': sorted(round(r['snr_coreav'] - r['snr_ffmpeg'], 3) for r in judged
                               if r['tonal'] and None not in (r['snr_coreav'], r['snr_ffmpeg'])),
        'a4_missing_pcm': [r['input'] for r in judged
                           if r['tonal'] and None in (r['snr_coreav'], r['snr_ffmpeg'])],
        'seconds_ratio_median': statistics.median(
            [r['seconds']['coreav'] / r['seconds']['ffmpeg'] for r in judged]) if judged else None,
        'bars': bars,
        'rows': rows,
    }
    with open(args.out, 'w') as f:
        json.dump(report, f, indent=1, default=str)
    print(json.dumps({k: v for k, v in report.items() if k != 'rows'}, indent=1, default=str))
    sys.exit(0 if all(bars.values()) else 1)


if __name__ == '__main__':
    main()
