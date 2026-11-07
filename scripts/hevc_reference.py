#!/usr/bin/env python3
"""Decoder-only development reference, without speed acceptance thresholds."""

import argparse
import http.client
import json
import os
import pathlib
import platform
import re
import resource
import ssl
import statistics
import subprocess
import sys
import tempfile
import time
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
sys.path.insert(0, str(ROOT / 'tools/compat-corpus'))
import hevc_conformance as conformance
import oracle_pin
import round_files
import test_oracle

ARMS = ('coreav', 'ffmpeg_one', 'ffmpeg_default')
CONFORMANCE_BITSTREAMS = (
    '09a9c0d78a502ece45f1df02c773b18bb3133fe21a8f1e9046db19bfa68f7c29',
    '26112eeb6f10ad18ffd30334e62bf9dafdba6d4efb07ffe3c5b75562e4639028',
    '3a9089207bedddb0c8db28b410441e37d4cff5dd8301cb8ba7d2f75036e2d50c',
    '3be359e9c70f56e478bb0c0710dcea112252541570f81856cc9f3dba3d988263',
)
CHILD_ENV = {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C', 'HOME': '/nonexistent'}


def command(args, timeout=180):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    result = subprocess.run(list(map(str, args)), capture_output=True, timeout=timeout, check=True,
                            env=CHILD_ENV)
    wall = time.perf_counter() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return result.stdout, dict(wall=wall, user=after.ru_utime - before.ru_utime,
                               system=after.ru_stime - before.ru_stime)


def ffmpeg_command(binary, path, threads, output):
    return [binary, '-hide_banner', '-loglevel', 'error', '-nostdin', '-noautorotate',
            '-flags', 'unaligned', '-c:v', 'hevc', *(['-threads', '1'] if threads else []),
            '-i', path, '-map', '0:v:0', '-an', '-sn', '-dn', '-noautoscale',
            '-fps_mode', 'passthrough', *output]


def verify(native, oracle, source, *, timeout=180):
    raw, _ = command([native, source, 'check'], timeout=timeout)
    rows = [json.loads(line) for line in raw.splitlines()]
    pictures = [row for row in rows if row.get('type') == 'picture']
    if not pictures or rows[-1].get('complete') is not True:
        raise ValueError('incomplete_native')
    depth = pictures[0]['bit_depth']
    if depth not in (8, 10) or any((p['bit_depth'], p['width'], p['height']) !=
            (depth, pictures[0]['width'], pictures[0]['height']) for p in pictures):
        raise ValueError('depth_change')
    metadata, _ = command([oracle.ffprobe, '-v', 'error',
        '-select_streams', 'v:0', '-show_entries', 'stream=pix_fmt,width,height', '-of', 'json', source], timeout=timeout)
    streams = json.loads(metadata)['streams']
    if len(streams) != 1 or (streams[0]['width'], streams[0]['height']) != (pictures[0]['width'], pictures[0]['height']):
        raise ValueError('oracle_geometry')
    pixel_format = streams[0]['pix_fmt']
    if pixel_format not in ({'yuv420p', 'yuvj420p'} if depth == 8 else {'yuv420p10le'}):
        raise ValueError('oracle_pixel_format')
    reference, _ = command(ffmpeg_command(oracle.ffmpeg, source, True,
        ['-pix_fmt', '+' + pixel_format, '-f', 'framemd5', '-']), timeout=timeout)
    dimensions = re.findall(rb'^#dimensions\s+0:\s+(\d+)x(\d+)\s*$', reference, re.M)
    if dimensions != [(str(pictures[0]['width']).encode(), str(pictures[0]['height']).encode())]:
        raise ValueError('oracle_geometry')
    frames = [line.decode().split(',') for line in reference.splitlines()
              if line and not line.startswith(b'#')]
    expected = [(int(row[-2]), row[-1].strip()) for row in frames]
    actual = [(p['output_bytes'], p['output_md5']) for p in pictures]
    if actual != expected:
        raise ValueError('pixel_mismatch')
    return dict(depth=depth, frames=len(pictures), width=pictures[0]['width'],
                height=pictures[0]['height'], output_bytes=sum(p['output_bytes'] for p in pictures),
                output_md5=rows[-1]['output_md5'], oracle_pixel_format=pixel_format)


def check_work(arm, stdout, expected):
    if arm == 'coreav':
        data = json.loads(stdout)
        if data['pictures'] != expected or data['decoded_pictures'] < expected or data['bins'] <= 0:
            raise ValueError('native_work')
    else:
        counts = re.findall(rb'^frame=(\d+)\s*$', stdout, flags=re.M)
        if not counts or int(counts[-1]) != expected:
            raise ValueError('oracle_work')


def summarize(rows):
    result = {}
    for depth in (8, 10):
        selected = [r for r in rows if r.get('status') == 'timed' and r['depth'] == depth]
        ratios = {}
        for reference in ARMS[1:]:
            for metric in ('wall', 'cpu'):
                values = []
                for row in selected:
                    medians = {}
                    for arm in ('coreav', reference):
                        samples = [c for c in row['calls'] if c['arm'] == arm]
                        medians[arm] = statistics.median(
                            c['wall'] if metric == 'wall' else c['user'] + c['system'] for c in samples)
                    values.append(medians['coreav'] / medians[reference])
                ratios[f'{reference}_{metric}'] = statistics.median(values) if values else None
        result[str(depth)] = dict(files=len(selected), median_coreav_over_ffmpeg=ratios)
    return result


def accepted_result(path, expected_candidate):
    if re.fullmatch('[0-9a-f]{40}', expected_candidate) is None:
        raise ValueError('expected_candidate_requires_full_sha')
    report = json.loads(path.read_text())
    if report.get('candidate') != expected_candidate:
        raise ValueError('result_candidate_mismatch')
    if report.get('complete') is not True:
        raise ValueError('result_incomplete')
    return report


def measure(args, oracle, source, identity):
    record = dict(id=identity, input_sha256=conformance.digest(source), stage='verification')
    try:
        record.update(verify(args.native, oracle, source))
        if getattr(args, 'diagnose_only', False):
            record['status'] = 'verified'
            return record
        record['stage'] = 'timing'
        commands = dict(coreav=[args.native, source, 'decode'])
        for arm in ARMS[1:]:
            commands[arm] = ffmpeg_command(oracle.ffmpeg, source, arm == 'ffmpeg_one',
                                           ['-progress', 'pipe:1', '-f', 'null', '-'])
        for arm in ARMS:
            stdout, _ = command(commands[arm])
            check_work(arm, stdout, record['frames'])
        calls = []
        for repeat in range(args.repeats):
            for arm in ARMS[repeat % 3:] + ARMS[:repeat % 3]:
                stdout, receipt = command(commands[arm])
                check_work(arm, stdout, record['frames'])
                calls.append(dict(arm=arm, repeat=repeat, **receipt))
        record.update(status='timed', calls=calls)
    except (ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        record.update(status='not_timed', failure=type(error).__name__)
        if isinstance(error, ValueError):
            record['failure'] = str(error)
        elif isinstance(error, subprocess.TimeoutExpired):
            record['tool'] = 'native' if str(error.cmd[0]) == str(args.native) else 'oracle'
        elif isinstance(error, subprocess.CalledProcessError):
            record['tool'] = 'native' if str(error.cmd[0]) == str(args.native) else 'oracle'
            record['exit_code'] = error.returncode
            if error.stderr and record['tool'] == 'native':
                reason = re.fullmatch(rb'(InvalidData|Unsupported|Limit)\("([A-Za-z0-9 _-]{1,120})"\)\s*', error.stderr)
                if reason:
                    record['native_category'] = reason[1].decode()
                    record['native_reason'] = reason[2].decode()
            elif error.stderr and b'pixel format' in error.stderr.lower():
                record['oracle_reason'] = 'pixel_format'
    return record


def profile(args, source, record, directory):
    output = directory / f"{record['id']}.callgrind"
    stdout, _ = command(['valgrind', '--tool=callgrind', '--error-exitcode=99',
        '--collect-jumps=no', '--dump-instr=no', '--cache-sim=no',
        f'--callgrind-out-file={output}', args.native, source, 'decode'], timeout=1200)
    check_work('coreav', stdout, record['frames'])
    text = output.read_text()
    events = re.search(r'^events: (.+)$', text, re.M)[1].split()
    totals = re.search(r'^summary: (.+)$', text, re.M)[1].split()
    record['instructions'] = int(totals[events.index('Ir')])
    annotation, _ = command(['callgrind_annotate', '--auto=no', '--inclusive=no', '--show=Ir',
                              '--threshold=95', output])
    hot = []
    for line in annotation.decode().splitlines():
        match = re.match(r'^\s*([\d,]+)\s+\(\s*([\d.]+)%\)\s+.*?:((?:coreav_|<coreav_).*)$', line)
        if match:
            symbol = match[3].split(' [')[0]
            hot.append(dict(instructions=int(match[1].replace(',', '')), percent=float(match[2]),
                            function=symbol))
    if not hot:
        raise ValueError('profile_symbols_missing')
    record['hotspots'] = hot[:15]


def fetch(entry, directory, budget):
    for attempt in range(3):
        connection = round_files.pool()
        connection.limits = budget
        try:
            return round_files.fetch_one(connection, entry, directory)
        except (ConnectionError, TimeoutError, http.client.IncompleteRead, ssl.SSLEOFError):
            if attempt == 2:
                raise
        finally:
            connection.close()
        time.sleep(2 ** attempt)


def inputs(args, oracle, directory, pool):
    selected = [r for r in conformance.roster() if r['partition'] == 'development']
    entries = {}
    for name in ('v1', 'v1-ts-audio', 'v2-heldout'):
        key = f'corpus/{name}/manifest.json'
        size, etag = pool.head(key)
        data = json.loads(pool.get(key, size, etag))
        for entry in data['files'] if isinstance(data, dict) else data:
            if entry.get('sha256') in {r['sha256'] for r in selected}:
                key = entry['key']
                entries[entry['sha256']] = dict(entry, key=key if key.startswith('corpus/') else f'corpus/{name}/{key}')
    for index, item in enumerate(selected):
        identity = f'd{index:02}'
        if args.only and identity not in args.only:
            continue
        entry = entries[item['sha256']]
        if (entry['size'], entry['ext']) != (item['size'], item['ext']):
            raise ValueError('input_identity')
        original = fetch(entry, directory, pool.limits)
        prepared = directory / f'd{index:02}.hevc'
        try:
            command([oracle.ffmpeg, '-hide_banner', '-loglevel', 'error', '-nostdin',
                '-i', original, '-map', '0:v:0', '-an', '-sn', '-dn', '-frames:v', str(args.frames),
                '-c:v', 'copy', '-bsf:v', 'hevc_mp4toannexb', '-f', 'hevc', '-y', prepared])
            yield identity, prepared, item['sha256']
        finally:
            original.unlink(missing_ok=True)
    registration = conformance.registration()
    for index, bitstream in enumerate(CONFORMANCE_BITSTREAMS):
        if args.only and f'c{index:02}' not in args.only:
            continue
        case = next(c for c in registration['archives']
                    if any(m['role'] == 'bitstream' and m['sha256'] == bitstream for m in c['extracted']))
        archive = directory / 'conformance.zip'
        conformance.download(registration['listing_url'] + case['archive'], archive,
                             conformance.ARCHIVE_CAP, case['bytes'])
        if conformance.digest(archive) != case['archive_sha256']:
            raise ValueError('conformance_identity')
        member = next(m for m in case['extracted'] if m['role'] == 'bitstream')
        with zipfile.ZipFile(archive) as zipped:
            data = zipped.read(member['name'])
        original = directory / 'conformance.bin'
        original.write_bytes(data)
        if conformance.digest(original) != member['sha256']:
            raise ValueError('conformance_member_identity')
        prepared = directory / f'c{index:02}.hevc'
        command([oracle.ffmpeg, '-hide_banner', '-loglevel', 'error', '-nostdin',
            '-i', original, '-map', '0:v:0', '-frames:v', str(args.frames), '-c:v', 'copy',
            '-f', 'hevc', '-y', prepared])
        yield f'c{index:02}', prepared, member['sha256']


def main():
    if sys.argv[1:2] == ['check-result']:
        parser = argparse.ArgumentParser(description='Accept only the expected decoder commit before reporting results')
        parser.add_argument('check_result')
        parser.add_argument('result', type=pathlib.Path)
        parser.add_argument('--expected-candidate', required=True)
        args = parser.parse_args()
        try:
            report = accepted_result(args.result, args.expected_candidate)
        except ValueError as error:
            parser.error(str(error))
        print(json.dumps(dict(candidate=report['candidate'], summary=summarize(report['files']))))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', type=pathlib.Path, required=True)
    parser.add_argument('--oracle-bin', type=pathlib.Path, required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--candidate', required=True)
    parser.add_argument('--frames', type=int, default=32, choices=range(1, 65))
    parser.add_argument('--repeats', type=int, default=3, choices=range(3, 10))
    parser.add_argument('--only', default='', help='Comma-separated development or conformance IDs')
    parser.add_argument('--diagnose-only', action='store_true')
    args = parser.parse_args()
    if re.fullmatch('[0-9a-f]{40}', args.candidate) is None:
        parser.error('candidate_requires_full_sha')
    args.only = set(args.only.split(',')) if args.only else set()
    if args.only - {f'd{i:02}' for i in range(26)} - {f'c{i:02}' for i in range(4)}:
        parser.error('unknown reference input ID')
    oracle = test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
    args.out.mkdir(parents=True, exist_ok=True)
    report = dict(candidate=args.candidate, oracle=oracle_pin.load()['BUILD'],
        native_sha256=conformance.digest(args.native), frames_cap=args.frames, repeats=args.repeats,
        selected_ids=sorted(args.only), diagnose_only=args.diagnose_only,
        host=dict(cpu=next(line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines()
                           if line.startswith('model name')), kernel=platform.release(),
                  cores=os.cpu_count(), affinity=sorted(os.sched_getaffinity(0)), load=os.getloadavg(),
                  memory_kib=int(re.search(r'MemTotal:\s+(\d+)', pathlib.Path('/proc/meminfo').read_text())[1]),
                  governor=(pathlib.Path('/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor').read_text().strip()
                            if pathlib.Path('/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor').exists() else 'unavailable')),
        scope='whole-process Annex B decode-to-null; startup, framing and teardown included; no container demux or output pixel hashing',
        instruction_scope='whole-process scalar instructions, including in-decoder SEI checks',
        complete=False, files=[])
    pool = round_files.pool()
    pool.limits.max_requests = 4000
    pool.limits.max_bytes = 32 << 30

    def save():
        report['summary'] = summarize(report['files'])
        report['r2_requests'] = pool.limits.requests
        (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')

    try:
        with tempfile.TemporaryDirectory(prefix='hevc-reference-') as temp:
            directory = pathlib.Path(temp)
            profiled = set()
            for identity, source, original_sha in inputs(args, oracle, directory, pool):
                row = measure(args, oracle, source, identity)
                row['source_sha256'] = original_sha
                report['files'].append(row)
                save()
                print(json.dumps(dict(id=identity, status=row['status'], depth=row.get('depth'),
                                      failure=row.get('failure'), native_reason=row.get('native_reason'))), flush=True)
                if row['status'] == 'timed' and identity.startswith('c') and row['depth'] not in profiled:
                    try:
                        small = directory / 'profile.hevc'
                        command([oracle.ffmpeg, '-hide_banner', '-loglevel', 'error', '-nostdin',
                            '-i', source, '-frames:v', '2', '-c:v', 'copy', '-f', 'hevc', '-y', small])
                        profile_row = dict(id=identity, **verify(args.native, oracle, small))
                        profile(args, small, profile_row, directory)
                        row['profile'] = profile_row
                    except (ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
                        row['profile_failure'] = True
                    profiled.add(row['depth'])
                    save()
            report['complete'] = True
    finally:
        pool.close()
        save()


if __name__ == '__main__':
    main()
