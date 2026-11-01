import argparse
import gzip
import hashlib
import importlib.util
import json
import math
import pathlib
import random
import re
import statistics

spec = importlib.util.spec_from_file_location('measurement', pathlib.Path(__file__).resolve().parent / 'vp9_release_cost.py')
measurement = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measurement)
ROOT = OUT = None
FILES = []
BUILDS = {}
VARIANTS = ['musl-plain', 'musl-chunk', 'musl-count', 'gnu-pinned-plain',
            'gnu-pinned-count', 'gnu-debian-plain', 'gnu-debian-count']
PLAIN = ['musl-plain', 'gnu-pinned-plain', 'gnu-debian-plain']
CELLS = [(variant, threads, 1) for variant in VARIANTS for threads in [0, 1, 2]]
CELLS += [(variant, 1, 0) for variant in PLAIN]
CELLS += [('musl-plain-aa', 0, 1)]
PROFILE_CELLS = [('musl-plain', 0, 1), ('musl-plain', 1, 1), ('musl-plain', 1, 0),
                 ('musl-chunk', 0, 1), ('musl-count', 0, 1), ('musl-count', 1, 1)]
PROFILE_FILE = '584a16b906bb'
PROFILE_REPETITIONS = 8


def load(root, out):
    global ROOT, OUT, FILES, BUILDS
    ROOT, OUT = root, out
    measurement.ROOT, measurement.OUT = root, out
    FILES = json.loads((root / 'maps.json').read_text())['files']
    BUILDS = json.loads((root / 'builds.json').read_text())


def record(row):
    return measurement.record(row)


def source(file):
    return ROOT / 'files' / (file['sha256'] + '.' + file['ext'])


def command(cell, file, packets=100000000, hashed=False):
    variant, threads, row_mt = cell
    binary = variant.removesuffix('-aa') + ('-hash' if hashed else '')
    arena = (ROOT / 'mimalloc-arena-reserve-kib.txt').read_text().strip()
    return ['/usr/bin/env', 'MIMALLOC_ARENA_RESERVE=' + arena,
            'COREAV_DIAG_THREADS=' + str(threads), 'COREAV_DIAG_ROW_MT=' + str(row_mt),
            ROOT / 'bin' / binary, source(file), '--packets', str(packets),
            '--backend', 'libvpx', '--max-memory-gib', '16', '--in-process']


def decode(cell, file, kind, expected=None, counter=False, digests=None, **labels):
    variant, threads, row_mt = cell

    def read_output():
        result = json.loads((OUT / 'last.stdout').read_text())
        stderr = (OUT / 'last.stderr').read_text()
        configurations = re.findall(r'DIAG_CONFIG threads=(\d+) row_mt=(\d+)', stderr)
        resolved = 4 if threads == 0 else threads
        if configurations != [(str(resolved), str(row_mt))]:
            raise RuntimeError('the configured thread cap or row-MT differs')
        frames = len(result['frames'])
        if expected is not None and frames != expected:
            raise RuntimeError('frame count differs')
        if kind == 'nondecode' and (frames or result['fed']):
            raise RuntimeError('exec+demux+open control decoded or fed packets')
        if kind != 'nondecode' and not frames:
            raise RuntimeError('no decoded frames')
        version = BUILDS['variants'][variant.removesuffix('-aa')]['vpx_version']
        if not result['decoder'].startswith('libvpx ' + version):
            raise RuntimeError('libvpx version differs')
        row = dict(frames=frames, configured_threads=resolved, row_mt=row_mt,
                   decoder=result['decoder'], fed=result['fed'])
        if digests is not None:
            if [frame['md5'] for frame in result['frames']] != digests:
                raise RuntimeError('pixel identity differs')
            row['pixel_md5_sha256'] = hashlib.sha256(''.join(digests).encode()).hexdigest()
        if counter:
            events = {}
            names = {}
            for line in stderr.splitlines():
                parts = line.split(';')
                if len(parts) > 2 and parts[2].split(':')[0] in {'instructions', 'cycles'}:
                    event = parts[2].split(':')[0]
                    if event in events:
                        raise RuntimeError('duplicate hardware event')
                    events[event] = int(parts[0].strip())
                    names[event] = parts[2]
            if set(events) != {'instructions', 'cycles'} or min(events.values()) <= 0:
                raise RuntimeError('missing hardware instruction or cycle count')
            row.update(events, counter_events=[names['instructions'], names['cycles']])
        return row

    cmd = command(cell, file, packets=0 if kind == 'nondecode' else 100000000,
                  hashed=digests is not None)
    if counter:
        cmd = ['/usr/bin/perf', 'stat', '-x', ';', '-e', 'instructions,cycles', '--'] + cmd
    return measurement.measured(cmd, dict(kind=kind, file=file['sha256'][:12], variant=variant,
                                         threads_requested=threads, row_mt_requested=row_mt, **labels), read_output)


def check_manifest():
    names = {file['sha256'][:12] for file in FILES}
    if names != {'a084bd45168c', '70eec00bc7d2', '584a16b906bb', 'bb5d39d47357'}:
        raise RuntimeError('diagnostic roster differs')
    required = {variant for variant in VARIANTS} | {'ffmpeg'}
    required |= {variant + '-hash' for variant in VARIANTS if not variant.endswith('-count')}
    if set(BUILDS['binaries']) != required:
        raise RuntimeError('binary roster differs')
    if set(BUILDS['variants']) != set(VARIANTS) or any(
            not BUILDS['variants'][variant].get('vpx_version') for variant in VARIANTS):
        raise RuntimeError('variant roster or libvpx version differs')


def verify():
    check_manifest()
    for name, expected in BUILDS['binaries'].items():
        with (ROOT / 'bin' / name).open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        record(dict(kind='artifact', name=name, sha256=digest))
        if digest != expected:
            raise RuntimeError('binary hash differs: ' + name)
    for file in FILES:
        with source(file).open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        record(dict(kind='input', file=file['sha256'][:12], sha256=digest))
        if digest != file['sha256']:
            raise RuntimeError('input hash differs')
    for name in ['vp9_release_attribution.py', 'vp9_release_cost.py', 'maps.json', 'builds.json',
                 'mimalloc-arena-reserve-kib.txt', 'gate.mp4']:
        with (ROOT / name).open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        record(dict(kind='artifact', name=name, sha256=digest))


def hashes(file, expected):
    ffmpeg = [ROOT / 'bin/ffmpeg', '-nostdin', '-noautorotate', '-flags', '+unaligned',
              '-i', source(file), '-map', '0:' + str(file['maps']['video']),
              '-fps_mode', 'passthrough', '-f', 'framemd5', '-']
    measurement.measured(ffmpeg, dict(kind='hash', variant='ffmpeg', file=file['sha256'][:12]))
    digests = [line.rsplit(',', 1)[-1].strip() for line in (OUT / 'last.stdout').read_text().splitlines()
               if line and not line.startswith('#')]
    if len(digests) != expected:
        raise RuntimeError('oracle frame count differs')
    for cell in CELLS:
        variant, threads, row_mt = cell
        if variant.endswith(('-count', '-aa')):
            continue
        decode(cell, file, 'hash', expected=expected, digests=digests)
        record(dict(kind='pixel-identity', file=file['sha256'][:12], variant=variant,
                    threads_requested=threads, row_mt_requested=row_mt, frames=expected, matches=True))


def summary(rows, metric, divisor):
    samples = [sum(row[metric] for row in rows if row['round'] == r) / divisor for r in range(5)]
    return dict(median=statistics.median(samples), minimum=min(samples), maximum=max(samples))


def calls():
    plain = OUT / 'calls.jsonl'
    text = plain.read_text() if plain.exists() else gzip.decompress((OUT / 'calls.jsonl.gz').read_bytes()).decode()
    return [json.loads(line) for line in text.splitlines()]


def analyse(units, counts):
    rows = calls()
    if any(row.get('rc', 0) or 'error' in row for row in rows):
        raise RuntimeError('failed invocation')
    result = {}
    for file in FILES:
        name = file['sha256'][:12]
        timed = [row for row in rows if row['kind'] == 'timed' and row['file'] == name]
        expected = {(v, t, mt, r, n) for v, t, mt in CELLS for r in range(5) for n in range(units[name])}
        actual = [(row['variant'], row['threads_requested'], row['row_mt_requested'], row['round'], row['repetition'])
                  for row in timed]
        if len(actual) != len(expected) or set(actual) != expected:
            raise RuntimeError('incomplete or duplicate timing roster')
        if {row['frames'] for row in timed} != {counts[name]}:
            raise RuntimeError('timed frame counts differ')
        nondecode = [row for row in rows if row['kind'] == 'nondecode' and row['file'] == name]
        expected_open = {(v, t, mt, r) for v, t, mt in CELLS for r in range(5)}
        actual_open = [(row['variant'], row['threads_requested'], row['row_mt_requested'], row['round'])
                       for row in nondecode]
        if len(actual_open) != len(expected_open) or set(actual_open) != expected_open:
            raise RuntimeError('incomplete or duplicate nondecode roster')
        if any(row['frames'] or row['fed'] for row in nondecode):
            raise RuntimeError('nondecode control did work')
        counters = [row for row in rows if row['kind'] == 'counter' and row['file'] == name]
        actual_counters = [(row['variant'], row['threads_requested'], row['row_mt_requested']) for row in counters]
        if len(actual_counters) != len(CELLS) or set(actual_counters) != set(CELLS):
            raise RuntimeError('incomplete or duplicate counter roster')
        if any(row['frames'] != counts[name] or min(row['instructions'], row['cycles']) <= 0 for row in counters):
            raise RuntimeError('invalid counter work or events')
        if len({tuple(row['counter_events']) for row in counters}) != 1:
            raise RuntimeError('counter event scope differs between calls')
        identities = [row for row in rows if row['kind'] == 'pixel-identity' and row['file'] == name]
        real = {cell for cell in CELLS if not cell[0].endswith(('-count', '-aa'))}
        actual_hashes = [(row['variant'], row['threads_requested'], row['row_mt_requested']) for row in identities]
        if len(actual_hashes) != len(real) or set(actual_hashes) != real:
            raise RuntimeError('incomplete or duplicate pixel roster')
        if any(row['frames'] != counts[name] or row['matches'] is not True for row in identities):
            raise RuntimeError('pixel identity is not established')
        if name == PROFILE_FILE:
            work = [(row['variant'], row['threads_requested'], row['row_mt_requested'], row['repetition'])
                    for row in rows if row['kind'] == 'profile-work']
            expected_work = {cell + (n,) for cell in PROFILE_CELLS for n in range(PROFILE_REPETITIONS)}
            if len(work) != len(expected_work) or set(work) != expected_work:
                raise RuntimeError('incomplete or duplicate profile roster')
            if any(row['frames'] != counts[name] for row in rows if row['kind'] == 'profile-work'):
                raise RuntimeError('profiled frame counts differ')
            for variant, threads, row_mt in PROFILE_CELLS:
                profile = OUT / f'{variant}-t{threads}-r{row_mt}.profile.txt'
                if not profile.exists() or not profile.read_text().strip():
                    raise RuntimeError('missing function profile')
        result[name] = {}
        for cell in CELLS:
            variant, threads, row_mt = cell
            selected = [row for row in timed if (row['variant'], row['threads_requested'], row['row_mt_requested'])
                        == cell]
            opened = [row for row in nondecode if (row['variant'], row['threads_requested'], row['row_mt_requested'])
                      == cell]
            counter = next(row for row in counters if (row['variant'], row['threads_requested'],
                                                       row['row_mt_requested']) == cell)
            result[name][f'{variant}:t{threads}:r{row_mt}'] = dict(
                summary(selected, 'cpu', units[name]), wall=summary(selected, 'wall', units[name]),
                nondecode_cpu=summary(opened, 'cpu', 1), nondecode_wall=summary(opened, 'wall', 1),
                instructions=counter['instructions'], cycles=counter['cycles'])
    gate = json.loads((OUT / 'gate.json').read_text())
    aa = statistics.median(file['musl-plain-aa:t0:r1']['median'] / file['musl-plain:t0:r1']['median']
                           for file in result.values())
    report = dict(diagnostic_only=True, files=result, identical_control=aa, gate=gate,
                  host_valid=abs(gate['ratio'] - 1) <= 0.05 and abs(aa - 1) <= 0.02)
    (OUT / 'analysis.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def profiles(counts):
    file = next(file for file in FILES if file['sha256'].startswith(PROFILE_FILE))
    for cell in PROFILE_CELLS:
        variant, threads, row_mt = cell
        name = f'{variant}-t{threads}-r{row_mt}'
        cmd = ['/usr/bin/perf', 'record', '-F', '199', '-e', 'cycles', '-g', '-o', OUT / 'perf.data', '--',
               '/usr/bin/python3', '-I', ROOT / 'vp9_release_attribution.py', '--profile-work',
               variant, str(threads), str(row_mt), str(counts[file['sha256'][:12]])]
        measurement.measured(cmd, dict(kind='profile-launcher', variant=variant,
                                      threads_requested=threads, row_mt_requested=row_mt))
        measurement.measured(['/usr/bin/perf', 'report', '--stdio', '--no-children', '--call-graph', 'none',
                              '--sort', 'comm,dso,symbol', '--percent-limit', '0.1', '-i', OUT / 'perf.data'],
                             dict(kind='profile-report', variant=variant, threads_requested=threads,
                                  row_mt_requested=row_mt))
        (OUT / (name + '.profile.txt')).write_bytes((OUT / 'last.stdout').read_bytes())
        (OUT / 'perf.data').unlink(missing_ok=True)
        (OUT / 'perf.data.old').unlink(missing_ok=True)


def replay(evidence):
    load(evidence, evidence)
    check_manifest()
    if (OUT / 'aborted.json').exists():
        raise RuntimeError('an aborted diagnostic cannot be replayed')
    completed = json.loads((OUT / 'completed.json').read_text())
    rows = calls()
    units, counts = {}, {}
    for file in FILES:
        name = file['sha256'][:12]
        warm = [row for row in rows if row['kind'] == 'warm' and row['file'] == name]
        if len(warm) != 1 or (warm[0]['variant'], warm[0]['threads_requested'], warm[0]['row_mt_requested']) \
                != ('musl-plain', 0, 1) or not warm[0].get('frames'):
            raise RuntimeError('a file needs exactly one real warm decode')
        counts[name] = warm[0]['frames']
        units[name] = max(1, math.ceil(1 / warm[0]['wall']))
    if completed != dict(files=4, cells=25, rounds=5, units=units):
        raise RuntimeError('completion record differs from the warm decodes')
    return analyse(units, counts)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--profile-work', nargs=4)
    parser.add_argument('--analyse', type=pathlib.Path)
    args = parser.parse_args()
    if args.analyse:
        result = replay(args.analyse.resolve())
        print(json.dumps(dict(diagnostic_only=True, host_valid=result['host_valid'],
                              identical_control=result['identical_control'])), flush=True)
        if not result['host_valid']:
            raise SystemExit(3)
        return
    load(pathlib.Path.cwd(), pathlib.Path.cwd() / 'out')
    if args.profile_work:
        variant, threads, row_mt, expected = args.profile_work
        file = next(file for file in FILES if file['sha256'].startswith(PROFILE_FILE))
        for repetition in range(PROFILE_REPETITIONS):
            decode((variant, int(threads), int(row_mt)), file, 'profile-work', expected=int(expected),
                   repetition=repetition)
        return
    OUT.mkdir(exist_ok=True)
    try:
        verify()
        record(dict(kind='method', seed=518, cells=CELLS, rounds=5, unit_wall_seconds=1))
        before = measurement.gate('before')
        rng = random.Random(518)
        units, counts = {}, {}
        for file in FILES:
            name = file['sha256'][:12]
            with source(file).open('rb') as stream:
                while stream.read(1 << 24):
                    pass
            warm = decode(('musl-plain', 0, 1), file, 'warm')
            counts[name] = warm['frames']
            units[name] = max(1, math.ceil(1 / warm['wall']))
            for round in range(5):
                order = list(CELLS)
                rng.shuffle(order)
                for position, cell in enumerate(order):
                    decode(cell, file, 'nondecode', expected=0, round=round, position=position)
                    for repetition in range(units[name]):
                        decode(cell, file, 'timed', expected=counts[name], round=round,
                               position=position, repetition=repetition, unit=units[name])
            print(name + ' timing complete', flush=True)
        after = measurement.gate('after')
        (OUT / 'gate.json').write_text(json.dumps(dict(before=before, after=after, ratio=after / before)))
        for file in FILES:
            name = file['sha256'][:12]
            hashes(file, counts[name])
            for cell in CELLS:
                decode(cell, file, 'counter', expected=counts[name], counter=True)
        profiles(counts)
        result = analyse(units, counts)
        (OUT / 'completed.json').write_text(json.dumps(dict(files=4, cells=25, rounds=5, units=units)))
        print('diagnostic complete; no acceptance verdict', flush=True)
        if not result['host_valid']:
            raise SystemExit(3)
    except SystemExit:
        raise
    except BaseException as error:
        (OUT / 'aborted.json').write_text(json.dumps(dict(error=str(error))))
        raise


if __name__ == '__main__':
    main()
