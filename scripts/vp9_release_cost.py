import argparse
import gzip
import hashlib
import json
import math
import os
import pathlib
import random
import statistics
import time

ROOT = pathlib.Path.cwd()
OUT = ROOT / 'out'
FILES = []
ARMS = ['ff', 'rel', 'alloc', 'deb', 'ff_aa']
LARGE = {'f51a56ed5718', '9ffa165e752a', 'ea684713a5be'}

def record(row):
    with (OUT / 'calls.jsonl').open('a') as stream:
        stream.write(json.dumps(row) + '\n')
    return row

def measured(cmd, label, read_output=None):
    env = {key: value for key, value in os.environ.items() if not key.startswith('MIMALLOC_')}
    arena = None
    if label.get('arm') in {'alloc', 'alloc-hash'} and label['kind'] in {'warm', 'timed', 'hash', 'profile'}:
        arena = (ROOT / 'mimalloc-arena-reserve-kib.txt').read_text().strip()
        env['MIMALLOC_ARENA_RESERVE'] = arena
    start = time.monotonic()
    pid = os.fork()
    if pid == 0:
        try:
            for number, name in [(1, 'last.stdout'), (2, 'last.stderr')]:
                fd = os.open(OUT / name, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                os.dup2(fd, number)
            os.execve(str(cmd[0]), [str(arg) for arg in cmd], env)
        except BaseException:
            os._exit(127)
    _, status, usage = os.wait4(pid, 0)
    row = dict(label, rc=os.waitstatus_to_exitcode(status), wall=time.monotonic() - start,
               cpu=usage.ru_utime + usage.ru_stime, maxrss_kb=usage.ru_maxrss,
               minor_faults=usage.ru_minflt, major_faults=usage.ru_majflt)
    if arena is not None:
        row['allocator_arena_reserve_kib'] = int(arena)
    if row['rc']:
        record(row)
        raise RuntimeError('command failed: ' + json.dumps(label))
    if read_output is not None:
        try:
            row.update(read_output())
        except BaseException as error:
            record(dict(row, error=str(error)))
            raise
    return record(row)

def source(file):
    return ROOT / 'files' / (file['sha256'] + '.' + file['ext'])

def command(arm, file):
    if arm.startswith('ff'):
        return [ROOT / 'bin/ffmpeg', '-nostdin', '-noautorotate', '-flags', '+unaligned', '-i', source(file),
                '-map', '0:' + str(file['maps']['video']), '-fps_mode', 'passthrough', '-f', 'null', '-']
    return [ROOT / ('bin/do-' + arm), source(file), '--packets', '100000000', '--backend', 'libvpx',
            '--max-memory-gib', '16', '--in-process']

def decode_call(arm, file, label, expected=None):
    def read_output():
        if arm.startswith('ff'):
            import re
            matches = re.findall(rb'frame=\s*(\d+)', (OUT / 'last.stderr').read_bytes())
            frames = int(matches[-1]) if matches else None
        else:
            frames = len(json.loads((OUT / 'last.stdout').read_text())['frames'])
        if not frames or (expected is not None and frames != expected):
            raise RuntimeError('missing or inconsistent frame count: ' + str(frames))
        return dict(frames=frames)
    return measured(command(arm, file), dict(label, file=file['sha256'][:12], arm=arm), read_output)

def gate(which):
    cmd = [ROOT / 'bin/ffmpeg', '-nostdin', '-noautorotate', '-flags', '+unaligned', '-i', ROOT / 'gate.mp4',
           '-map', '0:v:0', '-c:v', 'libx264', '-preset', 'medium', '-crf', '23', '-an',
           '-fps_mode', 'passthrough', '-y', '/dev/shm/vp9-gate.mp4']
    measured(cmd, dict(kind='gate-warm', which=which))
    rows = [measured(cmd, dict(kind='gate', which=which, round=r)) for r in range(3)]
    return statistics.median(row['wall'] for row in rows)

def hashes(file):
    results = {}
    for arm in ['rel-hash', 'alloc-hash']:
        cmd = [ROOT / ('bin/do-' + arm), source(file), '--packets', '100000000', '--backend', 'libvpx',
               '--max-memory-gib', '16', '--in-process']
        measured(cmd, dict(kind='hash', arm=arm, file=file['sha256'][:12]))
        result = json.loads((OUT / 'last.stdout').read_text())
        frames = result['frames']
        if not frames or any(not frame['md5'] for frame in frames):
            raise RuntimeError('missing pixel hashes')
        results[arm] = frames
        (OUT / (file['sha256'][:12] + '-' + arm + '.json')).write_text(json.dumps(result))
        record(dict(kind='hash-result', arm=arm, file=file['sha256'][:12], frames=len(frames)))
    if results['rel-hash'] != results['alloc-hash']:
        raise RuntimeError('candidate pixel or metadata mismatch')
    ff = [ROOT / 'bin/ffmpeg', '-nostdin', '-noautorotate', '-flags', '+unaligned', '-i', source(file),
          '-map', '0:' + str(file['maps']['video']), '-fps_mode', 'passthrough', '-f', 'framemd5', '-']
    measured(ff, dict(kind='hash', arm='ff', file=file['sha256'][:12]))
    text = (OUT / 'last.stdout').read_text()
    digests = [line.rsplit(',', 1)[-1].strip() for line in text.splitlines() if line and not line.startswith('#')]
    (OUT / (file['sha256'][:12] + '-ff.framemd5')).write_text(text)
    if digests != [frame['md5'] for frame in results['alloc-hash']]:
        raise RuntimeError('oracle pixel mismatch')
    record(dict(kind='hash-result', arm='ff', file=file['sha256'][:12], frames=len(digests)))
    record(dict(kind='pixel-identity', file=file['sha256'][:12], frames=len(digests), matches=True))
    return len(digests)

def transcode(file):
    hashes = []
    for arm in ['stock', 'alloc']:
        output = ROOT / 'video.mp4'
        cmd = [ROOT / ('bin/coreav-' + arm), 'transcode', source(file), output, '--encoder', 'ffmpeg',
               '--ffmpeg', ROOT / 'bin/ffmpeg', '--preset', 'medium', '--crf', '23', '--audio', 'none', '--force']
        measured(cmd, dict(kind='transcode', arm=arm, file=file['sha256'][:12]))
        video = ROOT / 'video.h264'
        measured([ROOT / 'bin/ffmpeg', '-v', 'error', '-nostdin', '-i', output, '-map', '0:v:0',
                  '-c', 'copy', '-f', 'h264', '-y', video], dict(kind='demux', arm=arm, file=file['sha256'][:12]))
        digest = hashlib.sha256(video.read_bytes()).hexdigest()
        hashes.append(digest)
        record(dict(kind='transcode-hash', arm=arm, file=file['sha256'][:12], sha256=digest))
        output.unlink()
        video.unlink()
    if hashes[0] != hashes[1]:
        raise RuntimeError('transcode byte mismatch')

def verify_artifacts():
    prefixes = {file['sha256'][:12] for file in FILES}
    if len(FILES) != 10 or len(prefixes) != 10 or not LARGE <= prefixes:
        raise RuntimeError('the complete ten-file roster is required')
    builds = json.loads((ROOT / 'builds.json').read_text())
    for name, expected in builds['binaries'].items():
        path = ROOT / 'bin' / name
        with path.open('rb') as stream:
            actual = hashlib.file_digest(stream, 'sha256').hexdigest()
        record(dict(kind='artifact', name=name, sha256=actual))
        if actual != expected:
            raise RuntimeError('binary hash mismatch: ' + name)
    for file in FILES:
        with source(file).open('rb') as stream:
            actual = hashlib.file_digest(stream, 'sha256').hexdigest()
        record(dict(kind='input', file=file['sha256'][:12], sha256=actual))
        if actual != file['sha256']:
            raise RuntimeError('input hash mismatch')
    for name in ['gate.mp4', 'maps.json', 'builds.json', 'vp9_release_cost.py', 'mimalloc-arena-reserve-kib.txt']:
        with (ROOT / name).open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        record(dict(kind='artifact', name=name, sha256=digest))

def profiles():
    for file in FILES:
        if file['sha256'].startswith(('584a16', 'ea6847')):
            label = dict(kind='profile', arm='alloc', file=file['sha256'][:12])
            measured(['/usr/bin/perf', 'record', '-F', '199', '-e', 'cycles', '-g', '-o', OUT / 'perf.data', '--']
                     + command('alloc', file), label)
            measured(['/usr/bin/perf', 'report', '--stdio', '--no-children', '--call-graph', 'none',
                      '--sort', 'comm,dso,symbol', '--percent-limit', '0.1', '-i', OUT / 'perf.data'],
                     dict(kind='profile-report', file=file['sha256'][:12]))
            (OUT / (file['sha256'][:12] + '-alloc.profile.txt')).write_bytes((OUT / 'last.stdout').read_bytes())
            (OUT / 'perf.data').unlink(missing_ok=True)
            (OUT / 'perf.data.old').unlink(missing_ok=True)

def analyse():
    completed = json.loads((OUT / 'completed.json').read_text())
    if (OUT / 'aborted.json').exists() or completed != dict(files=10, arms=ARMS, rounds=5):
        raise RuntimeError('an incomplete or aborted run cannot pass')
    calls = OUT / 'calls.jsonl'
    text = calls.read_text() if calls.exists() else gzip.decompress((OUT / 'calls.jsonl.gz').read_bytes()).decode()
    rows = [json.loads(line) for line in text.splitlines()]
    if any(row.get('rc', 0) or 'error' in row for row in rows):
        raise RuntimeError('a failed invocation cannot enter analysis')
    by_file = {}
    for file in FILES:
        name = file['sha256'][:12]
        timed = [row for row in rows if row['kind'] == 'timed' and row['file'] == name]
        units = {row['unit'] for row in timed}
        if len(units) != 1:
            raise RuntimeError('missing or inconsistent repetition unit')
        unit = next(iter(units))
        roster = {(arm, r, repetition) for arm in ARMS for r in range(5) for repetition in range(unit)}
        actual = [(row['arm'], row['round'], row['repetition']) for row in timed]
        if len(actual) != len(roster) or set(actual) != roster:
            raise RuntimeError('incomplete or duplicate timing roster')
        arena = int((ROOT / 'mimalloc-arena-reserve-kib.txt').read_text().strip())
        if any(row.get('allocator_arena_reserve_kib') != arena for row in timed if row['arm'] == 'alloc'):
            raise RuntimeError('candidate allocator configuration differs from the worker')
        counts = {row['frames'] for row in timed}
        hashed = [row for row in rows if row['kind'] == 'hash-result' and row['file'] == name]
        if len(counts) != 1 or len(hashed) != 3 or {row['arm'] for row in hashed} != {'rel-hash', 'alloc-hash', 'ff'}:
            raise RuntimeError('incomplete hash roster')
        if {row['frames'] for row in hashed} != counts:
            raise RuntimeError('timed and hashed frame counts differ')
        identity = [row for row in rows if row['kind'] == 'pixel-identity' and row['file'] == name]
        if len(identity) != 1 or identity[0].get('matches') is not True or identity[0]['frames'] not in counts:
            raise RuntimeError('pixel identity is not confirmed')
        stats = {}
        for arm in ARMS:
            stats[arm] = dict(maxrss_kb=max(row['maxrss_kb'] for row in timed if row['arm'] == arm))
            for metric in ['cpu', 'wall']:
                values = [sum(row[metric] for row in timed if row['arm'] == arm and row['round'] == r) / unit
                          for r in range(5)]
                stats[arm][metric] = dict(median=statistics.median(values), minimum=min(values), maximum=max(values))
        ratios = {pair: stats[numerator]['cpu']['median'] / stats[denominator]['cpu']['median']
                  for pair, numerator, denominator in [('alloc/deb', 'alloc', 'deb'), ('alloc/rel', 'alloc', 'rel'),
                                                       ('alloc/ff', 'alloc', 'ff'), ('rel/ff', 'rel', 'ff'),
                                                       ('deb/ff', 'deb', 'ff'), ('ff_aa/ff', 'ff_aa', 'ff')]}
        by_file[name] = dict(stats=stats, ratios=ratios, frames=next(iter(counts)))
    summary = {}
    for pair in next(iter(by_file.values()))['ratios']:
        values = [file['ratios'][pair] for file in by_file.values()]
        rng = random.Random(518)
        samples = sorted(statistics.median(rng.choices(values, k=len(values))) for _ in range(10000))
        summary[pair] = dict(median=statistics.median(values), lower=samples[249], upper=samples[9749],
                             below_one=sum(value < 1 for value in values), files=len(values))
    gate = json.loads((OUT / 'gate.json').read_text())
    large = statistics.median(by_file[name]['ratios']['alloc/deb'] for name in LARGE)
    checks = dict(large_parity=large <= 1.02,
                  no_file_regression=max(file['ratios']['alloc/rel'] for file in by_file.values()) <= 1.02,
                  overall_parity=summary['alloc/deb']['median'] <= 1.03,
                  host_gate=abs(gate['ratio'] - 1) <= 0.05,
                  identical_control=abs(summary['ff_aa/ff']['median'] - 1) <= 0.02)
    transcodes = [row for row in rows if row['kind'] == 'transcode-hash']
    if len(transcodes) != 6 or {(row['file'], row['arm']) for row in transcodes} != {(name, arm) for name in LARGE for arm in ['stock', 'alloc']}:
        raise RuntimeError('incomplete transcode roster')
    if any(len({row['sha256'] for row in transcodes if row['file'] == name}) != 1 for name in LARGE):
        raise RuntimeError('transcode bytes differ')
    result = dict(files=by_file, summary=summary, large_median=large, gate=gate, checks=checks,
                  passed=all(checks.values()), host_valid=checks['host_gate'] and checks['identical_control'])
    (OUT / 'analysis.json').write_text(json.dumps(result, indent=2) + '\n')
    return result

def report():
    result = analyse()
    print(json.dumps(dict(checks=result['checks'], summary=result['summary'])), flush=True)
    if not result['host_valid']:
        raise SystemExit(3)
    if not result['passed']:
        raise SystemExit(1)

def main():
    global ROOT, OUT, FILES
    parser = argparse.ArgumentParser()
    parser.add_argument('--analyse', type=pathlib.Path)
    args = parser.parse_args()
    if args.analyse:
        ROOT = OUT = args.analyse.resolve()
    FILES = json.loads((ROOT / 'maps.json').read_text())['files']
    if args.analyse:
        report()
        return
    OUT.mkdir(exist_ok=True)
    try:
        verify_artifacts()
        before = gate('before')
        timed_counts = {}
        for index, file in enumerate(FILES):
            with source(file).open('rb') as stream:
                while stream.read(1 << 24):
                    pass
            warm = decode_call('ff', file, dict(kind='warm'))
            unit = max(1, math.ceil(2.0 / warm['wall']))
            counts = set()
            for round in range(5):
                shift = (round + index) % len(ARMS)
                order = ARMS[shift:] + ARMS[:shift]
                if index % 2:
                    order.reverse()
                for arm in order:
                    for repetition in range(unit):
                        row = decode_call(arm, file, dict(kind='timed', round=round, repetition=repetition, unit=unit), warm['frames'])
                        counts.add(row['frames'])
            if counts == {None} or len(counts) != 1 or not next(iter(counts)):
                raise RuntimeError('frame count mismatch')
            timed_counts[file['sha256'][:12]] = next(iter(counts))
            print(file['sha256'][:12] + ' timing complete', flush=True)
        after = gate('after')
        (OUT / 'gate.json').write_text(json.dumps(dict(before=before, after=after, ratio=after / before)))
        for file in FILES:
            count = hashes(file)
            if count != timed_counts[file['sha256'][:12]]:
                raise RuntimeError('timed and hashed frame counts differ')
            print(file['sha256'][:12] + ' hashes agree ' + str(count), flush=True)
        for file in FILES:
            if file['sha256'][:12] in LARGE:
                transcode(file)
                print(file['sha256'][:12] + ' transcode bytes agree', flush=True)
        profiles()
        (OUT / 'completed.json').write_text(json.dumps(dict(files=len(FILES), arms=ARMS, rounds=5)))
        report()
    except SystemExit:
        raise
    except BaseException as error:
        (OUT / 'aborted.json').write_text(json.dumps(dict(error=str(error))))
        raise


if __name__ == '__main__':
    main()
