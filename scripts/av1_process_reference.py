"""Capture and replay the continuous-session AV1 process reference."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import re
import signal
import statistics
import subprocess
import time

import test_oracle


ARMS = ('sandbox', 'in_process', 'ffmpeg')


class CommandFailure(RuntimeError):
    def __init__(self, status, stdout, stderr):
        super().__init__(f'command failed with status {status}')
        self.stdout, self.stderr = stdout, stderr


def run(command, timeout, environment=None):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             env=environment, start_new_session=True)
    try:
        stdout, stderr = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGKILL)
        child.communicate()
        raise
    elapsed = time.perf_counter() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if child.returncode:
        raise CommandFailure(child.returncode, stdout, stderr)
    return stdout, stderr, {'seconds': elapsed,
                            'user_seconds': after.ru_utime - before.ru_utime,
                            'system_seconds': after.ru_stime - before.ru_stime}


def probe(oracle, path):
    output, _, _ = run([oracle, '-v', 'error', '-select_streams', 'v:0', '-show_data_hash', 'sha256', '-show_streams',
                        '-show_packets', '-of', 'json', str(path)], 120)
    data = json.loads(output)
    video = data['streams'][0]
    packets = data['packets']
    return {'width': video['width'], 'height': video['height'],
            'pixel_format': video['pix_fmt'], 'cadence': video['avg_frame_rate'],
            'duration_seconds': float(video['duration']),
            'frames': len(packets), 'payload_bytes': sum(int(p['size']) for p in packets),
            'packet_hashes_sha256': hashlib.sha256('\n'.join(p['data_hash'] for p in packets).encode()).hexdigest(),
            'keyframes': [i for i, p in enumerate(packets) if 'K' in p['flags']]}


def quality(ffmpeg, source, output, stats):
    _, stderr, _ = run([ffmpeg, '-nostdin', '-v', 'info', '-noautorotate', '-i', str(source),
                       '-noautorotate', '-i', str(output), '-filter_complex',
                       f'[0:v]setpts=PTS-STARTPTS[a];[1:v]setpts=PTS-STARTPTS[b];[a][b]psnr=stats_file={stats}:shortest=1:repeatlast=0',
                       '-an', '-f', 'null', '-'], 300)
    lines = stats.read_text().splitlines()
    summary = stderr.decode(errors='replace').rsplit('average:', 1)[1].split()[0]
    return {'psnr_db': None if summary == 'inf' else float(summary), 'metric_frames': len(lines)}


def raw_call(command, destination, timeout):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    with open(destination, 'wb') as sink:
        child = subprocess.Popen(command, stdout=sink, stderr=subprocess.PIPE, start_new_session=True)
        try:
            _, stderr = child.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise
    elapsed = time.perf_counter() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if child.returncode:
        raise CommandFailure(child.returncode, b'', stderr)
    return {'seconds': elapsed, 'user_seconds': after.ru_utime - before.ru_utime,
            'system_seconds': after.ru_stime - before.ru_stime}


def raw_identity(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        while chunk := stream.read(1 << 20):
            digest.update(chunk)
    return {'bytes': path.stat().st_size, 'sha256': digest.hexdigest()}


def decode_diagnostic(args, oracle, manifest, output):
    results = []
    for item in manifest:
        commands = {
            'coreav': [args.coreav, '--strict-safe', 'ffmpeg', '-i', item['path'], '-an',
                       '-pix_fmt', 'yuv420p', '-f', 'rawvideo', '-'],
            'ffmpeg': [args.ffmpeg, '-nostdin', '-v', 'error', '-noautorotate', '-flags', 'unaligned',
                       '-c:v', 'h264', '-threads', str(len(os.sched_getaffinity(0))), '-i', item['path'], '-an',
                       '-fps_mode', 'passthrough', '-noautoscale', '-pix_fmt', 'yuv420p', '-f', 'rawvideo', '-']}
        reference = [str(oracle.ffmpeg), *commands['ffmpeg'][1:]]
        identities = {}
        temporary = output / 'raw.tmp'
        for arm, command in [*commands.items(), ('oracle', reference)]:
            raw_call(command, temporary, args.timeout)
            identities[arm] = raw_identity(temporary)
            temporary.unlink()
        if not identities['coreav'] == identities['ffmpeg'] == identities['oracle']:
            raise ValueError('decode-only raw sample streams differ')
        source = probe(oracle.ffprobe, item['path'])
        expected = source['frames'] * source['width'] * source['height'] * 3 // 2
        if identities['coreav']['bytes'] != expected:
            raise ValueError('decode-only raw stream is incomplete')
        calls = []
        for round_ in range(3):
            order = ('coreav', 'ffmpeg') if round_ % 2 == 0 else ('ffmpeg', 'coreav')
            for arm in order:
                calls.append({'arm': arm, 'round': round_,
                              **raw_call(commands[arm], os.devnull, args.timeout)})
        results.append({'input': item['label'], 'raw_identity': identities['coreav'], 'calls': calls})
    return results


def summarize(data):
    labels = [source['label'] for source in data['inputs']]
    if not labels or len(labels) != len(set(labels)) or any(row['input'] not in labels for row in data['calls']):
        raise ValueError('invalid input roster')
    results = []
    for source in data['inputs']:
        calls = [row for row in data['calls'] if row['input'] == source['label']]
        if len(calls) != 9 or {(r['arm'], r['round']) for r in calls} != {
                (arm, round_) for arm in ARMS for round_ in range(3)}:
            raise ValueError('incomplete or duplicate call grid')
        groups = {arm: [r for r in calls if r['arm'] == arm] for arm in ARMS}
        settings = groups['sandbox'][0]['receipt']['video']['encoding']
        for row in calls:
            if not math.isfinite(row['seconds']) or row['seconds'] <= 0:
                raise ValueError('invalid elapsed time')
            if row['probe']['frames'] != source['frames'] or row['metric_frames'] != source['frames']:
                raise ValueError('incomplete output or metric frame count')
            if [row['probe']['width'], row['probe']['height']] != source['geometry'] or row['probe']['pixel_format'] != 'yuv420p':
                raise ValueError('changed output geometry or pixel format')
            if row['arm'] != 'ffmpeg':
                video = row['receipt']['video']
                encoding = video['encoding']
                if video['backend'] != 'native' or video['codec'] != 'h264' or video['frames'] != source['frames'] or encoding['segments'] != 1:
                    raise ValueError('wrong decoder, frame count, or session count')
                if row['decode'] is None or row['decode']['frames'] != source['frames']:
                    raise ValueError('missing complete native decoder timing')
                expected = 'sandboxed' if row['arm'] == 'sandbox' else 'in-process-measurement'
                if encoding['isolation'] != expected:
                    raise ValueError('wrong encoder isolation')
            else:
                observation = row['encoder_observation']
                if observation['sessions'] != 1 or observation['version'] != 'v4.2.0':
                    raise ValueError('wrong comparator encoder version or session count')
                if observation['gop_frames'] != settings['keyint'] + 1 or observation['parallelism_level'] != 1:
                    raise ValueError('wrong comparator GOP or parallelism setting')
        baseline = groups['sandbox'][0]
        for row in calls:
            if row['probe']['cadence'] != baseline['probe']['cadence']:
                raise ValueError('changed output cadence')
            if abs(row['probe']['payload_bytes'] / baseline['probe']['payload_bytes'] - 1) > 0.02:
                raise ValueError('video payload differs by more than two percent')
            a, b = row['psnr_db'], baseline['psnr_db']
            if (a is None) != (b is None) or (a is not None and abs(a - b) > 0.20):
                raise ValueError('PSNR differs by more than 0.20 dB')
        times = {arm: statistics.median(r['seconds'] for r in rows) for arm, rows in groups.items()}
        spread = {arm: [min(r['seconds'] for r in rows), max(r['seconds'] for r in rows)] for arm, rows in groups.items()}
        s, i, f = (times[arm] for arm in ARMS)
        results.append({'input': source['label'], 'median_seconds': times, 'range_seconds': spread,
                        'sandbox_over_ffmpeg': s / f, 'in_process_over_ffmpeg': i / f,
                        'gap_seconds': s - f, 'boundary_seconds': s - i, 'residual_seconds': i - f,
                        'boundary_share': (s - i) / (s - f) if s > f else None,
                        'share_unstable': abs(s - f) <= max(hi - lo for lo, hi in spread.values())})
    totals = {arm: sum(r['median_seconds'][arm] for r in results) for arm in ARMS}
    s, i, f = (totals[arm] for arm in ARMS)
    return {'inputs': results, 'aggregate': {'sum_median_seconds': totals,
            'sandbox_over_ffmpeg_geomean': statistics.geometric_mean(r['sandbox_over_ffmpeg'] for r in results),
            'in_process_over_ffmpeg_geomean': statistics.geometric_mean(r['in_process_over_ffmpeg'] for r in results),
            'boundary_seconds': s - i, 'residual_seconds': i - f, 'gap_seconds': s - f,
            'boundary_share': (s - i) / (s - f) if s > f else None}}


def capture(args):
    try:
        oracle = test_oracle.require(args.oracle, args.probe)
    except test_oracle.QualificationError as error:
        raise ValueError(str(error)) from error
    output = args.output.resolve()
    output.mkdir(exist_ok=False)
    manifest = json.loads(args.manifest.read_text())
    data = {'method': 'continuous-av1-process-reference-v1', 'inputs': [], 'calls': [],
            'decoder_workers_requested': len(os.sched_getaffinity(0)),
            'binaries': {name: hashlib.sha256(Path(path).read_bytes()).hexdigest()
                         for name, path in [('coreav', args.coreav), ('ffmpeg', args.ffmpeg), ('oracle', oracle.ffmpeg)]}}
    record = output / 'calls.json'
    try:
        for item in manifest:
            source = Path(item['path']).resolve()
            observed = probe(oracle.ffprobe, source)
            if observed['duration_seconds'] < 30 and not args.synthetic_smoke:
                raise ValueError('reference inputs must be at least thirty seconds')
            source_info = {'label': item['label'], 'sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
                           'frames': observed['frames'], 'geometry': [observed['width'], observed['height']],
                           'cadence': observed['cadence'], 'duration_seconds': observed['duration_seconds']}
            data['inputs'].append(source_info)
            settings = None
            for round_ in range(3):
                order = ARMS[round_:] + ARMS[:round_]
                for arm in order:
                    destination = output / f'{item["label"]}-{round_}-{arm}.mp4'
                    environment = os.environ.copy()
                    if arm != 'ffmpeg':
                        environment['COREAV_MEASURE_AV1_IN_PROCESS'] = '1' if arm == 'in_process' else '0'
                        command = [args.coreav, 'transcode', '--video', 'av1', '--audio', 'none',
                                   '--preset', '8', '--crf', '32', '--timeout', str(args.timeout),
                                   str(source), str(destination)]
                    else:
                        if settings is None:
                            raise ValueError('missing observed encoder settings')
                        color = settings['color']
                        params = f'tune=1:aq-mode=2:lp=1:keyint={settings["keyint"] + 1}:fps-num={settings["cadence"]["num"]}:fps-denom={settings["cadence"]["den"]}'
                        command = [args.ffmpeg, '-nostdin', '-hide_banner', '-noautorotate', '-c:v', 'h264',
                                   '-threads', str(len(os.sched_getaffinity(0))), '-i', str(source), '-map', '0:v:0', '-an', '-fps_mode', 'passthrough',
                                   '-c:v', 'libsvtav1', '-preset', '8', '-crf', '32', '-svtav1-params', params,
                                   '-color_primaries', str(color['primaries']), '-color_trc', str(color['transfer']),
                                   '-colorspace', str(color['matrix']), '-color_range', 'pc' if color['full_range'] else 'tv',
                                   '-chroma_sample_location', 'left', str(destination)]
                    stdout, stderr, timing = run(command, args.timeout, environment)
                    (output / f'{item["label"]}-{round_}-{arm}.stderr').write_bytes(stderr)
                    receipt = json.loads(stdout) if arm != 'ffmpeg' else None
                    diagnostic = [line.split(b' ', 1)[1] for line in stderr.splitlines()
                                  if line.startswith(b'coreav_measurement_decode ')]
                    decode = json.loads(diagnostic[0]) if len(diagnostic) == 1 else None
                    versions = re.findall(rb'SVT \[version\]:\s*SVT-AV1 Encoder Lib (v[^\s]+)', stderr)
                    gops = re.findall(rb'gop size / mini-gop size / key-frame type\s*:\s*(\d+)', stderr)
                    parallelism = re.findall(rb'Level of Parallelism:\s*(\d+)', stderr)
                    observation = {'sessions': len(versions), 'version': versions[0].decode() if len(versions) == 1 else None,
                                   'gop_frames': int(gops[0]) if len(gops) == 1 else None,
                                   'parallelism_level': int(parallelism[0]) if len(parallelism) == 1 else None}
                    if receipt is not None:
                        settings = receipt['video']['encoding']
                    row = {'input': item['label'], 'round': round_, 'arm': arm, **timing,
                           'receipt': receipt, 'decode': decode, 'encoder_observation': observation,
                           'probe': probe(oracle.ffprobe, destination),
                           'output_sha256': hashlib.sha256(destination.read_bytes()).hexdigest(),
                           **quality(oracle.ffmpeg, source, destination, output / 'metric.txt')}
                    data['calls'].append(row)
                    record.write_text(json.dumps(data, indent=2, allow_nan=False) + '\n')
                    print(item['label'], round_, arm, timing['seconds'], flush=True)
        data['decode_diagnostics'] = decode_diagnostic(args, oracle, manifest, output)
        summary = summarize(data)
        (output / 'summary.json').write_text(json.dumps(summary, indent=2, allow_nan=False) + '\n')
    except CommandFailure as error:
        (output / 'failed.stdout').write_bytes(error.stdout)
        (output / 'failed.stderr').write_bytes(error.stderr)
        raise
    finally:
        record.write_text(json.dumps(data, indent=2, allow_nan=False) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    acquire = commands.add_parser('capture')
    acquire.add_argument('--manifest', type=Path, required=True)
    acquire.add_argument('--coreav', required=True)
    acquire.add_argument('--ffmpeg', required=True)
    acquire.add_argument('--oracle', required=True)
    acquire.add_argument('--probe', required=True)
    acquire.add_argument('--output', type=Path, required=True)
    acquire.add_argument('--timeout', type=int, default=600)
    acquire.add_argument('--synthetic-smoke', action='store_true')
    replay = commands.add_parser('summarize')
    replay.add_argument('record', type=Path)
    args = parser.parse_args()
    if args.command == 'capture':
        capture(args)
    else:
        print(json.dumps(summarize(json.loads(args.record.read_text())), indent=2, allow_nan=False))


if __name__ == '__main__':
    main()
