#!/usr/bin/env python3
"""C2 of docs/design/vp9.md: SHA-bound corpus acquisition and exact frame comparisons.

Fetch runs with credentials; run never needs them. Per-file records and captured
command output are private. Only summary.json is suitable for public artifacts.
"""
import argparse
import collections
from fractions import Fraction
import hashlib
import json
import os
import pathlib
import re
import resource
import signal
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
ROSTER = ROOT / 'docs/measurements/vp9-native/roster.json'
CONDITION = ROOT / 'docs/measurements/vp9-native/file-condition.json'
PACKETS = 120
ORACLE_FRAMES = 8 * PACKETS + 1
CAPTURE_LIMIT = 64 << 20
MEMORY_LIMIT = 6 << 30


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def integer(value, minimum=None):
    return type(value) is int and (minimum is None or value >= minimum)


def roster(path=ROSTER):
    data = read_json(path)
    groups = [data.get(name) for name in ('development', 'held_out', 'b5')]
    for values, count in zip(groups, (46, 28, 10)):
        require(isinstance(values, list) and len(values) == count, 'incomplete registered roster')
        require(all(isinstance(v, str) and re.fullmatch(r'[0-9a-f]{64}', v) for v in values),
                'invalid registered SHA-256')
        require(len(set(values)) == len(values), 'duplicate registered input')
    development, held, b5 = groups
    require(not set(development) & set(held), 'overlapping registered partitions')
    require(set(b5) <= set(development + held), 'unregistered whole-stream input')
    condition = read_json(CONDITION)
    require(condition['sha256'] in development + held and condition['sha256'] not in b5,
            'truncated-tail input must stay in its registered prefix scope')
    return development + held, set(b5)


def unique_object(pairs):
    out = {}
    for key, value in pairs:
        require(key not in out, 'duplicate JSON key')
        out[key] = value
    return out


def read_json(path):
    return json.loads(pathlib.Path(path).read_text(), object_pairs_hook=unique_object)


def digest(path):
    hasher = hashlib.sha256()
    with pathlib.Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            hasher.update(block)
    return hasher.hexdigest()


def write_json(path, value):
    path = pathlib.Path(path)
    temporary = path.with_suffix(path.suffix + '.part')
    temporary.write_text(json.dumps(value, indent=1) + '\n')
    temporary.replace(path)


def shard(value):
    parts = value.split('/')
    require(len(parts) == 2 and all(p.isdecimal() for p in parts), 'shard must be I/N')
    index, count = map(int, parts)
    require(0 <= index < count <= 74, 'invalid shard')
    return index, count


def fetch(args):
    require(sys.platform == 'linux', 'corpus acquisition requires the registered Linux host')
    sys.path.insert(0, str(ROOT / 'tools/compat-corpus'))
    import round_files
    names, _ = roster()
    index, count = shard(args.shard)
    wanted = names[index::count]
    manifest = round_files.manifest('v1', args.manifests)
    entries = {}
    for entry in manifest:
        sha = entry.get('sha256')
        if sha not in names:
            continue
        require(sha not in entries, 'duplicate manifest input')
        require(entry.get('set') == 'real' and entry.get('ext') in ('mkv', 'webm')
                and integer(entry.get('size'), 1), 'invalid manifest input')
        key = entry.get('key')
        require(isinstance(key, str) and key and '..' not in key.split('/'), 'invalid object key')
        entries[sha] = dict(sha256=sha, size=entry['size'], ext=entry['ext'], family='matroska-webm',
                            key=key if key.startswith('corpus/') else f'corpus/v1/{key}')
    require(set(entries) == set(names), 'manifest does not cover the registered roster')
    directory = pathlib.Path(args.dest)
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    round_files.fetch([entries[name] for name in wanted], directory)
    print(json.dumps(dict(fetched=len(wanted))))


def limits(timeout):
    resource.setrlimit(resource.RLIMIT_AS, (MEMORY_LIMIT, MEMORY_LIMIT))
    resource.setrlimit(resource.RLIMIT_FSIZE, (CAPTURE_LIMIT, CAPTURE_LIMIT))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    resource.setrlimit(resource.RLIMIT_CPU, (timeout + 2, timeout + 2))


def child(command, directory, label, timeout):
    require(sys.platform == 'linux', 'corpus subprocesses require Linux resource limits')
    stdout, stderr = directory / f'{label}.stdout', directory / f'{label}.stderr'
    environment = {k: v for k, v in os.environ.items() if k in ('PATH', 'HOME', 'LANG', 'TMPDIR')}
    environment['LC_ALL'] = 'C'
    with stdout.open('wb') as out, stderr.open('wb') as err:
        process = subprocess.Popen([str(v) for v in command], stdout=out, stderr=err, env=environment,
                                   start_new_session=True, preexec_fn=lambda: limits(timeout))
        try:
            code = process.wait(timeout=timeout)
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
    require(stdout.stat().st_size < CAPTURE_LIMIT and stderr.stat().st_size < CAPTURE_LIMIT,
            'command capture limit')
    require(code == 0, f'{label} failed')
    return stdout, stderr


def native_capture(path, scope, input_sha=None):
    lines = pathlib.Path(path).read_text().splitlines()
    rows = [json.loads(line, object_pairs_hook=unique_object) for line in lines]
    require(len(rows) >= 2 and all(isinstance(r, dict) for r in rows), 'incomplete native capture')
    meta, footer = rows[0], rows[-1]
    require(meta.get('kind') == 'stream' and integer(meta.get('stream_index'), 0), 'invalid native stream')
    require(integer(meta.get('profile'), 0) and meta['profile'] <= 3
            and integer(meta.get('bit_depth')) and meta['bit_depth'] in (8, 10, 12),
            'unknown native eligibility')
    subsampling = meta.get('subsampling')
    require(isinstance(subsampling, list) and len(subsampling) == 2
            and all(integer(v, 0) and v <= 1 for v in subsampling), 'invalid native subsampling')
    clock = meta.get('time_base')
    require(isinstance(clock, list) and len(clock) == 2 and all(integer(v, 1) for v in clock),
            'invalid native clock')
    require(footer.get('kind') == 'complete' and footer.get('scope') == scope, 'missing native completion')
    frames = rows[1:-1]
    if scope == 'inspect':
        require(not frames, 'inspection returned frames')
        return meta, footer, []
    total, fed, emitted = (footer.get(k) for k in ('packets', 'fed', 'frames'))
    require(all(integer(v, 0) for v in (total, fed, emitted)) and total > 0, 'invalid native counts')
    require(fed == (total if scope == 'all' else min(PACKETS, total)), 'incomplete packet coverage')
    require(emitted == len(frames), 'incomplete native frames')
    cutoff = footer.get('cutoff')
    require((cutoff is None) == (scope == 'all' or total <= PACKETS), 'invalid cutoff presence')
    require(cutoff is None or integer(cutoff), 'invalid cutoff')
    condition = read_json(CONDITION)
    expected_condition = condition['id'] if input_sha == condition['sha256'] else None
    require(footer.get('file_condition') == expected_condition, 'missing, stale or unregistered file condition')
    if expected_condition:
        require(scope == 'first120' and fed == PACKETS and cutoff is not None
                and cutoff <= condition['partial_pts']
                and Fraction(*clock) == Fraction(condition['timestamp_scale'], 1_000_000_000),
                'file condition changed its scope or partial-packet clock')
    by_packet = collections.Counter()
    previous = 0
    for number, frame in enumerate(frames):
        require(frame.get('kind') == 'frame' and integer(frame.get('frame'), 0)
                and frame['frame'] == number, 'invalid frame order')
        packet = frame.get('packet')
        require(integer(packet, 0) and previous <= packet < fed, 'invalid frame packet')
        previous = packet
        by_packet[packet] += 1
        require(by_packet[packet] <= 8, 'too many pictures in a packet')
        require(integer(frame.get('pts')), 'invalid frame timestamp')
        width, height = frame.get('width'), frame.get('height')
        require(integer(width, 1) and integer(height, 1), 'invalid frame dimensions')
        require(frame.get('size') == width * height + 2 * ((width + 1) // 2) * ((height + 1) // 2),
                'invalid packed frame size')
        require(isinstance(frame.get('md5'), str) and re.fullmatch('[0-9a-f]{32}', frame['md5']),
                'invalid frame digest')
    coverage = footer.get('coverage')
    require(isinstance(coverage, dict) and all(integer(coverage.get(k), 0)
            for k in ('coded', 'sharpness', 'segment_filter')), 'invalid coverage')
    require(coverage['coded'] <= 8 * fed and coverage['sharpness'] <= coverage['coded']
            and coverage['segment_filter'] <= coverage['coded'], 'invalid coverage counts')
    return meta, footer, frames


def oracle_capture(path):
    clock, rows, md5 = None, [], False
    for line in pathlib.Path(path).read_text().splitlines():
        if line.startswith('#hash:'):
            require(line.split(':', 1)[1].strip() == 'MD5', 'unexpected oracle digest')
            md5 = True
        elif line.startswith('#tb '):
            require(clock is None and line.startswith('#tb 0:'), 'unexpected oracle streams')
            clock = Fraction(line.split(':', 1)[1].strip())
            require(clock > 0, 'invalid oracle clock')
        elif line and not line.startswith('#'):
            columns = [v.strip() for v in line.split(',')]
            require(len(columns) == 6 and columns[0] == '0', 'invalid oracle frame')
            require(re.fullmatch('[0-9a-f]{32}', columns[5]), 'invalid oracle digest')
            pts, size = int(columns[2]), int(columns[4])
            require(size > 0, 'invalid oracle size')
            rows.append(dict(pts=pts, size=size, md5=columns[5]))
    require(md5 and clock is not None, 'missing oracle headers')
    return clock, rows


def judge(meta, footer, mine, clock, theirs):
    require(clock == Fraction(*meta['time_base']), 'oracle clock differs from the Matroska tick')
    cutoff = footer['cutoff']
    if footer['scope'] == 'first120':
        require(len(theirs) <= ORACLE_FRAMES, 'oracle exceeded its independent frame bound')
        if len(theirs) == ORACLE_FRAMES:
            require(cutoff is not None and theirs[-1]['pts'] >= cutoff, 'invalid oracle sentinel')
    expected = [r for r in theirs if cutoff is None or r['pts'] < cutoff]
    actual = [{k: r[k] for k in ('pts', 'size', 'md5')}
              for r in mine if cutoff is None or r['pts'] < cutoff]
    require(expected, 'no independently expected frames')
    require(actual == expected, 'frame count, order, timestamp, size or MD5 differs')
    return len(expected)


def ffmpeg_command(program, path, scope):
    command = [program, '-nostdin', '-v', 'error', '-xerror', '-noautorotate', '-copyts',
               '-threads', '1', '-c:v', 'vp9', '-i', path, '-map', '0:v:0', '-fps_mode',
               'passthrough', '-enc_time_base', 'demux', '-noautoscale']
    if scope == 'first120':
        command += ['-frames:v', str(ORACLE_FRAMES)]
    return command + ['-f', 'framemd5', '-']


def eligible(meta, pixel_format):
    subsampling = tuple(meta['subsampling'])
    depth, profile = meta['bit_depth'], meta['profile']
    require((profile % 2 == 0) == (subsampling == (1, 1))
            and (profile < 2) == (depth == 8), 'invalid VP9 profile configuration')
    suffix = '' if depth == 8 else f'{depth}le'
    chroma = {(1, 1): '420', (1, 0): '422', (0, 0): '444', (0, 1): '440'}[subsampling]
    formats = {f'yuv{chroma}p{suffix}'}
    if subsampling == (0, 0):
        formats.add(f'gbrp{suffix}')
    require(pixel_format in formats, 'independent eligibility observations disagree')
    return profile == 0


def compare(entry, args, b5, private):
    path = pathlib.Path(entry['path'])
    sha = entry['sha256']
    require(path.is_file() and digest(path) == sha, 'input digest mismatch')
    directory = private / sha
    directory.mkdir(mode=0o700, exist_ok=True)
    result = dict(sha256=sha, b5=sha in b5, status='failed')
    try:
        probe, probe_errors = child([args.ffprobe, '-v', 'error', '-show_entries',
                          'stream=index,codec_type,codec_name,pix_fmt,time_base', '-of', 'json', path],
                         directory, 'probe', 120)
        require(probe_errors.stat().st_size == 0, 'oracle probe reported an error')
        streams = read_json(probe).get('streams')
        require(isinstance(streams, list), 'missing oracle streams')
        video = next((s for s in streams if s.get('codec_type') == 'video'), None)
        require(video is not None and video.get('codec_name') == 'vp9', 'oracle first video is not VP9')
        require(isinstance(video.get('pix_fmt'), str) and video['pix_fmt'], 'unknown oracle eligibility')
        inspected, _ = child([args.native, path, 'inspect'], directory, 'inspect', 120)
        meta, _, _ = native_capture(inspected, 'inspect')
        require(video.get('index') == meta['stream_index'], 'selected video stream differs')
        require(Fraction(video['time_base']) == Fraction(*meta['time_base']), 'probe clock differs')
        if not eligible(meta, video['pix_fmt']):
            result.update(status='outside_profile0', pixel_format=video.get('pix_fmt'))
            return result
        scope = 'all' if sha in b5 else 'first120'
        timeout = 3600 if scope == 'all' else 900
        capture, _ = child([args.native, path, scope], directory, 'native', timeout)
        actual_meta, footer, mine = native_capture(capture, scope, sha)
        require(actual_meta == meta, 'native stream metadata changed')
        reference, errors = child(ffmpeg_command(args.ffmpeg, path, scope), directory, 'oracle', timeout)
        require(errors.stat().st_size == 0, 'oracle reported an error')
        clock, theirs = oracle_capture(reference)
        frames = judge(meta, footer, mine, clock, theirs)
        result.update(status='agree', frames=frames, fed=footer['fed'], packets=footer['packets'],
                      coverage=footer['coverage'], scope=scope, file_condition=footer.get('file_condition'))
        return result
    finally:
        require(digest(path) == sha, 'input bytes changed during comparison')


def run(args):
    require(sys.platform == 'linux', 'corpus comparisons require the registered Linux host')
    names, b5 = roster()
    index, count = shard(args.shard)
    expected = names[index::count]
    directory = pathlib.Path(args.files)
    listing = read_json(directory / 'files.json')
    require(isinstance(listing, list) and [e.get('sha256') for e in listing] == expected,
            'fetched inputs do not equal the registered shard')
    for entry in listing:
        require(entry.get('ext') in ('mkv', 'webm'), 'invalid fetched extension')
        path = pathlib.Path(entry['path']).resolve()
        require(path.parent == directory.resolve() and path.name == f"{entry['sha256']}.{entry['ext']}",
                'fetched path leaves the input directory')
    private = pathlib.Path(args.private)
    private.mkdir(mode=0o700, parents=True, exist_ok=True)
    rows = []
    summary = dict(complete=False, expected=len(expected), agree=0, outside_profile0=0, failed=0,
                   shard=[index, count], roster_sha256=digest(ROSTER))
    try:
        for entry in listing:
            try:
                row = compare(entry, args, b5, private)
            except Exception as error:
                row = dict(sha256=entry['sha256'], b5=entry['sha256'] in b5, status='failed',
                           error=type(error).__name__, reason=str(error))
            rows.append(row)
            write_json(private / 'records.json', rows)
        summary['complete'] = True
    finally:
        for key in ('agree', 'outside_profile0', 'failed'):
            summary[key] = sum(r['status'] == key for r in rows)
        summary['processed'] = len(rows)
        summary['frames'] = sum(r.get('frames', 0) for r in rows)
        summary['truncated_tail'] = sum(r.get('file_condition') is not None for r in rows)
        summary['b5'] = {key: sum(r['status'] == key and r.get('b5', False) for r in rows)
                         for key in ('agree', 'outside_profile0', 'failed')}
        summary['coverage'] = {k: sum(r.get('coverage', {}).get(k, 0) for r in rows)
                               for k in ('coded', 'sharpness', 'segment_filter')}
        write_json(pathlib.Path(args.summary), summary)
        print(json.dumps(summary, sort_keys=True))
    return 0 if summary['complete'] and summary['failed'] == 0 else 1


def summarize(args):
    names, b5 = roster()
    require(integer(args.shards, 1) and args.shards <= len(names), 'invalid shard count')
    paths = sorted(pathlib.Path(args.summaries).glob('summary-*.json'))
    require(len(paths) == args.shards, 'missing or extra shard summaries')
    reports = {}
    for path in paths:
        report = read_json(path)
        part = report.get('shard')
        require(isinstance(part, list) and len(part) == 2 and part[1] == args.shards
                and integer(part[0], 0) and part[0] < args.shards, 'invalid summary shard')
        index = part[0]
        require(index not in reports, 'duplicate shard summary')
        require(report.get('roster_sha256') == digest(ROSTER), 'summary roster changed')
        expected = len(names[index::args.shards])
        require(report.get('expected') == expected, 'summary expected count changed')
        require(type(report.get('complete')) is bool, 'invalid completion flag')
        for key in ('agree', 'outside_profile0', 'failed', 'processed', 'frames', 'truncated_tail'):
            require(integer(report.get(key), 0), 'invalid summary count')
        require(report['agree'] + report['outside_profile0'] + report['failed'] == report['processed']
                and report['processed'] <= expected, 'inconsistent summary counts')
        require(not report['complete'] or report['processed'] == expected, 'incomplete claimed completion')
        whole = report.get('b5')
        require(isinstance(whole, dict) and set(whole) == {'agree', 'outside_profile0', 'failed'}
                and all(integer(whole.get(k), 0)
                and whole[k] <= report[k] for k in ('agree', 'outside_profile0', 'failed')),
                'invalid whole-stream counts')
        expected_whole = sum(name in b5 for name in names[index::args.shards])
        require(sum(whole.values()) <= expected_whole
                and (not report['complete'] or sum(whole.values()) == expected_whole),
                'missing whole-stream outcomes')
        require(report['frames'] >= report['agree'], 'empty agreeing stream')
        has_condition = read_json(CONDITION)['sha256'] in names[index::args.shards]
        require(report['truncated_tail'] <= int(has_condition)
                and report['truncated_tail'] <= report['agree']
                and (not report['complete'] or report['failed'] or report['truncated_tail'] == int(has_condition)),
                'missing or unregistered truncated-tail outcome')
        observations = report.get('coverage')
        require(isinstance(observations, dict) and all(integer(observations.get(k), 0)
                for k in ('coded', 'sharpness', 'segment_filter')), 'invalid filter observations')
        require(observations['sharpness'] <= observations['coded']
                and observations['segment_filter'] <= observations['coded'], 'invalid observation counts')
        reports[index] = report
    result = {k: sum(r[k] for r in reports.values())
              for k in ('expected', 'agree', 'outside_profile0', 'failed', 'processed', 'frames', 'truncated_tail')}
    result['b5'] = {k: sum(r['b5'][k] for r in reports.values())
                    for k in ('agree', 'outside_profile0', 'failed')}
    result['coverage'] = {k: sum(r['coverage'][k] for r in reports.values())
                          for k in ('coded', 'sharpness', 'segment_filter')}
    result['pass'] = all(r['complete'] for r in reports.values()) and result['failed'] == 0 and result['agree'] > 0
    write_json(args.out, result)
    print(json.dumps(result, sort_keys=True))
    return 0 if result['pass'] else 1


def preflight(args):
    path = pathlib.Path(args.file).resolve()
    private = pathlib.Path(args.private)
    private.mkdir(mode=0o700, parents=True, exist_ok=True)
    entry = dict(sha256=digest(path), path=str(path))
    result = compare(entry, args, set(), private)
    require(result['status'] == 'agree' and result['frames'] > 0, 'preflight did not compare pixels')
    print(json.dumps(dict(preflight='pass', frames=result['frames'])))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='mode', required=True)
    get = commands.add_parser('fetch')
    get.add_argument('--dest', required=True)
    get.add_argument('--manifests')
    get.add_argument('--shard', default='0/1')
    check = commands.add_parser('run')
    for name in ('files', 'native', 'ffmpeg', 'ffprobe', 'private', 'summary'):
        check.add_argument(f'--{name}', required=True)
    check.add_argument('--shard', default='0/1')
    public = commands.add_parser('preflight')
    for name in ('file', 'native', 'ffmpeg', 'ffprobe', 'private'):
        public.add_argument(f'--{name}', required=True)
    report = commands.add_parser('summarize')
    report.add_argument('--summaries', required=True)
    report.add_argument('--out', required=True)
    report.add_argument('--shards', type=int, required=True)
    args = parser.parse_args()
    try:
        return {'fetch': fetch, 'run': run, 'preflight': preflight, 'summarize': summarize}[args.mode](args)
    except Exception as error:
        print(f'vp9-corpus: {args.mode} failed ({type(error).__name__})', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
