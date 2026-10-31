#!/usr/bin/env python3
"""The Opus bars of docs/design/transcode.md ("Opus bars"): coreav
transcode --audio opus against FFmpeg's -c:a libopus on a set this script
makes with the same FFmpeg, judged on O1 to O4 as the note fixes them.
Speed is recorded for information only.

    opus_encode_check.py --coreav BIN --ffmpeg FFMPEG --ffprobe FFPROBE \
        --work DIR --out REPORT.json

Exits 1 when a bar is missed, 2 when the set cannot be made. Standard
library only; FFmpeg is a black box.
"""

import argparse
import array
import json
import math
import os
import statistics
import subprocess
import sys
import time

DURATION = 10
VIDEO = ['-f', 'lavfi', '-i', f'testsrc2=size=160x120:rate=25:duration={DURATION}']
SWEEP = f'0.5*sin(2*PI*50*{DURATION}/log(300)*(pow(300,t/{DURATION})-1))'
SOURCES = [
    # name, audio input, audio options, container, judged on O4
    ('S1', f'sine=frequency=440:sample_rate=48000:duration={DURATION}',
     ['-ac', '1', '-c:a', 'flac'], 'mp4', True),
    ('S2', f'aevalsrc=0.5*sin(2*PI*440*t)|0.5*sin(2*PI*660*t):s=48000:c=stereo:d={DURATION}',
     ['-c:a', 'flac'], 'mp4', True),
    ('S3', f'aevalsrc={SWEEP}|{SWEEP}:s=48000:c=stereo:d={DURATION}',
     ['-c:a', 'flac'], 'mkv', True),
    ('S4', f'anoisesrc=color=pink:sample_rate=48000:amplitude=0.3:seed=1:duration={DURATION}',
     ['-ac', '2', '-c:a', 'flac'], 'mp4', False),
    ('S5', f'aevalsrc=(random(0)*2-1)*0.5*(0.5+0.5*sin(2*PI*4*t)):s=48000:c=mono:d={DURATION}',
     ['-c:a', 'libmp3lame', '-b:a', '192k'], 'mkv', False),
    ('S6', f'sine=frequency=1000:sample_rate=44100:duration={DURATION}',
     ['-ac', '2', '-c:a', 'flac'], 'mp4', False),
]
RATES = [None, 32, 128]
RUNS = 3


def run(args, **kwargs):
    return subprocess.run(args, stdin=subprocess.DEVNULL, capture_output=True, **kwargs)


def timed(args):
    start = time.perf_counter()
    done = run(args)
    return done, time.perf_counter() - start


def arms(coreav, ffmpeg, source, ours, theirs, rate):
    """The two command lines of one input, as the note fixes them: CoreAV
    with `--audio opus` and FFmpeg with `-c:a libopus`, each with the
    rate (kbit/s) when one is given."""
    ours_args = [coreav, 'transcode', source, ours, '--force', '--audio', 'opus',
                 '--encoder', 'ffmpeg', '--ffmpeg', ffmpeg]
    theirs_args = [ffmpeg, '-nostdin', '-v', 'error', '-y', '-i', source,
                   '-c:v', 'libx264', '-c:a', 'libopus']
    if rate:
        ours_args += ['--audio-bitrate', str(rate)]
        theirs_args += ['-b:a', f'{rate}k']
    return ours_args, theirs_args + [theirs]


def make_sources(ffmpeg, work):
    paths = {}
    for name, audio, options, container, _ in SOURCES:
        path = os.path.join(work, f'{name}.{container}')
        done = run([ffmpeg, '-nostdin', '-v', 'error', '-y', *VIDEO, '-f', 'lavfi', '-i', audio,
                    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', *options, path])
        if done.returncode != 0:
            sys.stderr.write(f'cannot make {name}: {done.stderr.decode(errors="replace")}\n')
            sys.exit(2)
        paths[name] = path
    return paths


def probe_audio(ffprobe, path):
    done = run([ffprobe, '-v', 'error', '-select_streams', 'a', '-show_entries',
                'stream=codec_name,start_pts,duration_ts,initial_padding,bit_rate,channels',
                '-of', 'json', path])
    streams = json.loads(done.stdout or b'{}').get('streams') or [{}]
    return streams[0]


def pcm(ffmpeg, path):
    """The PCM O4 compares: the audio of `path` decoded by FFmpeg to 48 kHz
    float, or None when FFmpeg fails or gives no samples."""
    done = run([ffmpeg, '-nostdin', '-v', 'error', '-i', path, '-map', '0:a', '-ar', '48000',
                '-f', 'f32le', '-'])
    return samples(done.returncode, done.stdout)


def samples(returncode, stdout):
    """The float samples of a decode that exited with `returncode`, or None
    when it failed or gave none."""
    if returncode != 0 or len(stdout) < 4:
        return None
    decoded = array.array('f')
    decoded.frombytes(stdout[:len(stdout) // 4 * 4])
    if sys.byteorder != 'little':
        decoded.byteswap()
    return decoded


def snr(reference, decoded):
    n = min(len(reference), len(decoded))
    signal = sum(x * x for x in reference[:n])
    noise = sum((x - y) * (x - y) for x, y in zip(reference[:n], decoded[:n]))
    if noise == 0:
        return math.inf
    return 10 * math.log10(signal / noise)


def quality(reference, decoded):
    """The SNR of `decoded` against `reference`, or None when either has
    no samples: snr would score that as infinite."""
    if not reference or not decoded:
        return None
    return snr(reference, decoded)


def o4(ours, theirs):
    """O4 on one input; a missing SNR on either arm fails it."""
    return ours is not None and theirs is not None and ours >= theirs - 1.0


def libopus_vendor(ffmpeg, work):
    """The vendor string FFmpeg's libopus writes in an Ogg Opus header."""
    path = os.path.join(work, 'vendor.opus')
    run([ffmpeg, '-nostdin', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'sine=d=0.1',
         '-c:a', 'libopus', path])
    try:
        data = open(path, 'rb').read()
    except OSError:
        return None
    at = data.find(b'libopus ')
    if at < 0:
        return None
    end = at
    while end < len(data) and 32 <= data[end] < 127:
        end += 1
    return data[at:end].decode()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--coreav', required=True)
    parser.add_argument('--ffmpeg', required=True)
    parser.add_argument('--ffprobe', required=True)
    parser.add_argument('--work', required=True)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    os.makedirs(args.work, exist_ok=True)
    sources = make_sources(args.ffmpeg, args.work)
    references = {name: pcm(args.ffmpeg, sources[name]) for name, *_, tonal in SOURCES if tonal}
    rows = []
    for name, _, _, _, tonal in SOURCES:
        for rate in RATES:
            label = f'{name}-{rate or "default"}'
            ours = os.path.join(args.work, f'{label}.coreav.mp4')
            theirs = os.path.join(args.work, f'{label}.ffmpeg.mp4')
            ours_args, theirs_args = arms(args.coreav, args.ffmpeg, sources[name], ours, theirs, rate)
            times = {'coreav': [], 'ffmpeg': []}
            summary = None
            failed = None
            for _ in range(RUNS):
                done, took = timed(ours_args)
                times['coreav'].append(took)
                if done.returncode != 0:
                    failed = done.stderr.decode(errors='replace')
                    break
                summary = json.loads(done.stdout)
                done, took = timed(theirs_args)
                times['ffmpeg'].append(took)
                if done.returncode != 0:
                    failed = 'ffmpeg: ' + done.stderr.decode(errors='replace')
                    break
            row = {'input': label, 'tonal': tonal, 'failed': failed}
            rows.append(row)
            if failed:
                continue
            encoded = summary['encoded_audio']
            decode = run([args.ffmpeg, '-nostdin', '-v', 'error', '-i', ours, '-map', '0:a',
                          '-f', 'null', '-'])
            ours_probe = probe_audio(args.ffprobe, ours)
            theirs_probe = probe_audio(args.ffprobe, theirs)
            row.update({
                'summary': encoded,
                'coreav': ours_probe,
                'ffmpeg': theirs_probe,
                'o1': decode.returncode == 0 and not decode.stderr,
                'o1_messages': decode.stderr.decode(errors='replace'),
                'bit_rate_ratio': int(ours_probe.get('bit_rate', 0)) / max(1, int(theirs_probe.get('bit_rate', 1))),
                'seconds': {arm: statistics.median(t) for arm, t in times.items()},
            })
            row['o2'] = (
                ours_probe.get('codec_name') == 'opus'
                and ours_probe.get('start_pts') == 0
                and ours_probe.get('initial_padding') == encoded['pre_skip']
                and ours_probe.get('duration_ts') == encoded['samples']
                and abs(ours_probe.get('duration_ts', 0) - theirs_probe.get('duration_ts', 0)) <= 48
            )
            if tonal:
                row['snr_coreav'] = quality(references[name], pcm(args.ffmpeg, ours))
                row['snr_ffmpeg'] = quality(references[name], pcm(args.ffmpeg, theirs))
                row['o4'] = o4(row['snr_coreav'], row['snr_ffmpeg'])
    judged = [r for r in rows if not r['failed']]
    ratios = [r['bit_rate_ratio'] for r in judged]
    bars = {
        'O1': len(judged) == len(rows) and all(r['o1'] for r in judged),
        'O2': len(judged) == len(rows) and all(r['o2'] for r in judged),
        'O3': (len(judged) == len(rows) and all(0.85 <= x <= 1.15 for x in ratios)
               and 0.95 <= statistics.median(ratios) <= 1.05),
        'O4': len(judged) == len(rows) and all(r['o4'] for r in judged if r['tonal']),
    }
    report = {
        'ffmpeg_libopus': libopus_vendor(args.ffmpeg, args.work),
        'inputs': len(rows),
        'failed': [r['input'] for r in rows if r['failed']],
        'o1_met': sum(1 for r in judged if r['o1']),
        'o2_met': sum(1 for r in judged if r['o2']),
        'bit_rate_ratio': {'min': min(ratios, default=None), 'median': statistics.median(ratios) if ratios else None,
                           'max': max(ratios, default=None)},
        'snr_delta_db': sorted(round(r['snr_coreav'] - r['snr_ffmpeg'], 3) for r in judged
                               if r['tonal'] and None not in (r['snr_coreav'], r['snr_ffmpeg'])),
        'o4_missing_pcm': [r['input'] for r in judged
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
