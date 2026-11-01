#!/usr/bin/env python3
"""B4: the speed of `coreav ffprobe` against the pinned `ffprobe`
(docs/design/compat-cli.md, "The measurements", fixed before the run).

    speed.py select --out DIR
        Downloads the held-out files B4 names: from each container (ISO base
        media from corpus v1, Matroska and WebM from corpus v1, transport
        streams from corpus v1-ts-audio), the first 20 held-out `real` files
        of at most 256 MB in the order sha256("coreav-compat-b4:" + sha256).
        Writes DIR/selection.json. R2 credentials come from the environment
        and are never printed.

    speed.py run --coreav PATH --oracle-bin DIR --cpus CPU --dir DIR --out RESULTS.json
        Every native `ffprobe` command of commands-v1 on every selected
        file: `coreav ffprobe` and `ffprobe` alternate, one process at a
        time, wall time of the whole process, after one untimed run of each
        (the page cache). A pair's ratio is the median of CoreAV's rounds
        over the median of ffprobe's. A command coreav refuses is reported
        as refused and has no ratio. Verify the frozen files and pinned
        oracle first, then require both calibration arms to pass before
        any B4 pair runs. Each process has a 600-second timeout.

    speed.py report --results RESULTS.json
        Per command, per container and overall: the median ratio and its
        bootstrap 95% interval (10,000 stratified file-cluster resamples,
        seed 0). Historical unversioned results are not re-scored.

    speed.py r4 --arm r4|aa|null --coreav PATH --ffmpeg PATH --ffprobe PATH
                (--files FILES.json | --media DIR [DIR...]) --scratch DIR
                --out RESULTS.json [--rounds 5] [--cpus 4-7]
        R4 of the 0.1.0 round (docs/design/compat-cli.md, "The 0.1.0 bars"):
        every command of commands-0.1.0, `ffprobe` and `ffmpeg` alike, on
        every file of its families that can take it (R3's rule, from the
        pinned ffprobe's stream listing), in the order of the set and of the
        files. A pair is one untimed run of each side, then ROUNDS rounds
        alternating them, wall time of the whole process, one process at a
        time on the CPUs of --cpus; an `ffmpeg` command writes into
        SCRATCH, which is emptied after every run. The arm sets the sides:
        `r4` CoreAV against FFmpeg, `aa` FFmpeg against itself, `null`
        CoreAV against itself (the gate). FILES.json lists the files in
        their order as {"family", "path"}; RESULTS.json names a file by its
        family and position only.

    speed.py r4-report --results RESULTS.json [--out FILE] [--gate]
        Per command class (the `Job` column), in each family and over all
        of them: the median ratio, its bootstrap 95% interval and the share
        of pairs whose ratio is below 1.00; per command, the median alone.
        Without --gate, a class meets R4 when its median is at most 1.00;
        with --gate (the `aa` and `null` arms), the host passes when every
        class's median over the families is within 0.98 to 1.02 and its
        interval holds 1.00. A group with pairs and no ratio (a class in a
        family, or a class over the families, which alone the gate judges) is
        unmeasured and fails. A pair whose untimed runs both succeeded and
        a timed run then failed or timed out makes the run incomplete, which
        fails it (R4) or fails the gate; a pair whose untimed run failed has
        no ratio, as the plan registers, and is counted by the side that
        failed. Exit 1 when a class misses or the gate fails,
        2 when the run is void (a non-exempt log event or a CoreAV failure
        without a valid refusal record beside a failed oracle),
        3 when no pair has a ratio. A qualifying run-observed strict-safe
        refusal is excluded from timing and counted with its reason group;
        an oracle-failed pair's valid refusal is a separate diagnostic.
        Coverage and diagnostic reasons accompany every speed number.
        `r4` exits 3 when it has no pair to time.

Every run has no gconv modules (release_round.NO_GCONV). Python 3 standard
library only, besides boto3 for `select`.
"""
import argparse
import collections
import hashlib
import json
import math
import os
import pathlib
import random
import select as select_event
import shutil
import statistics
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import release_round  # noqa: E402

ROOT = HERE.parent.parent
COMMANDS = HERE / 'commands-v1' / 'commands.jsonl'
SALT = 'coreav-compat-b4:'
# The first path component of an object key in the private store.
STORE_ROOT = 'corpus'
LIMIT = 256 * 1024 * 1024
PER_CONTAINER = 20
CLASSES = {
    'isobmff': ('v1', {'mp4', 'mov', 'm4a', 'm4b', 'm4v', '3gp'}),
    'matroska': ('v1', {'mkv', 'webm'}),
    'mpegts': ('v1-ts-audio', {'ts', 'm2ts', 'mts', 'm2t'}),
}


def entries(corpus):
    data = json.loads((ROOT / 'corpus' / corpus / 'manifest.json').read_text())
    return data['files'] if isinstance(data, dict) else data


def order(entry):
    return hashlib.sha256((SALT + entry['sha256']).encode()).hexdigest()


def select(opts):
    sys.path.insert(0, str(ROOT / 'corpus' / 'v1' / 'scripts'))
    import common  # noqa: E402
    out = pathlib.Path(opts.out)
    out.mkdir(parents=True, exist_ok=True)
    chosen = []
    for name, (corpus, exts) in CLASSES.items():
        pool = [e for e in entries(corpus)
                if e.get('set') == 'real' and e.get('holdout') and e['ext'] in exts
                and e['size'] <= LIMIT]
        pool.sort(key=order)
        for e in pool[:PER_CONTAINER]:
            key = e['key'] if '/' in e['key'] else '/'.join((STORE_ROOT, corpus, e['key']))
            chosen.append({'class': name, 'sha256': e['sha256'], 'ext': e['ext'],
                           'size': e['size'], 'key': key})
    s3 = common.s3()
    for e in chosen:
        dest = out / f"{e['sha256']}.{e['ext']}"
        if dest.exists() and dest.stat().st_size == e['size']:
            continue
        part = dest.with_suffix(dest.suffix + '.part')
        s3.download_file(common.BUCKET, e['key'], str(part))
        part.rename(dest)
    (out / 'selection.json').write_text(json.dumps(
        [{k: v for k, v in e.items() if k != 'key'} for e in chosen], indent=1) + '\n')
    print(json.dumps({c: sum(1 for e in chosen if e['class'] == c) for c in CLASSES}))


def native_commands():
    out = []
    with open(COMMANDS) as f:
        for line in f:
            r = json.loads(line)
            if r['tool'] == 'ffprobe' and r['label'] == 'native':
                out.append(r['args'])
    return out


B4_METHOD = 'b4-file-cluster-v1'
B4_TIMEOUT = 600


def timed(argv, env, *, raise_oserror=False):
    start = time.perf_counter()
    try:
        with subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL, env=env) as process:
            try:
                fd = os.pidfd_open(process.pid)
                try:
                    ready = select_event.select([fd], [], [], max(0, B4_TIMEOUT - (time.perf_counter() - start)))[0]
                finally:
                    os.close(fd)
                if ready:
                    code = process.wait()
                else:
                    process.kill()
                    code = None
                elapsed = time.perf_counter() - start
            except BaseException:
                process.kill()
                raise
    except OSError:
        if raise_oserror:
            raise
        return time.perf_counter() - start, -1
    return elapsed, code


def eligible(args, family):
    if '-f' not in args:
        return True
    if args[args.index('-f') + 1:][:1] != ['mpegts']:
        raise ValueError('B4 only registers the forced format -f mpegts')
    return family == 'mpegts'


def b4_fallback(log):
    counts = {'lines': 0, 'refused': 0, 'executed': 0, 'invalid': 0}
    for line in pathlib.Path(log).read_text(errors='replace').splitlines() if pathlib.Path(log).exists() else []:
        if not line.strip():
            continue
        counts['lines'] += 1
        try:
            event = json.loads(line)
            if event.get('ran') is not None:
                counts['executed'] += 1
            elif event.get('not_run') == 'strict_safe':
                counts['refused'] += 1
            else:
                counts['invalid'] += 1
        except (ValueError, AttributeError):
            counts['invalid'] += 1
    return {**counts, 'void': bool(counts['executed'] or counts['invalid'])}


def b4_pair(left, right, env, refuse=False):
    warm = [timed(argv, env)[1] for argv in (left, right)]
    row = {'warmup_exit': warm}
    if refuse and warm == [4, 0]:
        return {**row, 'refused': True}
    if warm != [0, 0]:
        return {**row, 'failed': {'phase': 'warmup', 'exit': warm}}
    row.update(a=[], b=[], timed_exit=[])
    for i in range(5):
        a, ac = timed(left, env)
        b, bc = timed(right, env)
        row['a'].append(a)
        row['b'].append(b)
        row['timed_exit'].append([ac, bc])
        if [ac, bc] != [0, 0] or a <= 0 or b <= 0:
            row['failed'] = {'phase': 'timed', 'round': i, 'exit': [ac, bc]}
            return row
    row['ratio'] = statistics.median(row['a']) / statistics.median(row['b'])
    return row


def interval(rows, stratified=True):
    if not rows:
        return None
    clusters = collections.defaultdict(lambda: collections.defaultdict(list))
    for row in rows:
        clusters[row['class'] if stratified else 'all'][row['file']].append(row['ratio'])
    strata = [list(files.values()) for _, files in sorted(clusters.items())]
    rng = random.Random(0)
    meds = []
    for _ in range(10000):
        sample = [ratio for files in strata for cluster in rng.choices(files, k=len(files)) for ratio in cluster]
        meds.append(statistics.median(sample))
    meds.sort()
    return [meds[249], meds[9749]]


def calibrate(selection, directory, coreav, ffprobe, env, log):
    first = {family: next(e for e in selection if e['class'] == family) for family in CLASSES}
    commands = [['-show_format', '-show_streams'], ['-show_packets']]
    arms = {}
    for arm, binary in (('aa', [ffprobe]), ('null', [coreav, 'ffprobe'])):
        rows = []
        for family, entry in first.items():
            path = str(directory / f"{entry['sha256']}.{entry['ext']}")
            for command, args in enumerate(commands):
                argv = binary + args + [path]
                rows.append({'class': family, 'file': entry['file'], 'command': command,
                             **b4_pair(argv, argv, env)})
        successful = [r for r in rows if 'ratio' in r]
        calls = [{'class': r['class'], 'file': r['file'], 'ratio': a / b}
                 for r in successful for a, b in zip(r['a'], r['b'])]
        median = statistics.median(r['ratio'] for r in calls) if calls else None
        ci = interval(calls, stratified=False)
        passed = (len(successful) == len(rows) and median is not None
                  and GATE[0] <= median <= GATE[1] and ci[0] <= 1.0 <= ci[1]
                  and b4_fallback(log)['lines'] == 0)
        arms[arm] = {'commands': commands, 'results': rows, 'median': median, 'ci95': ci, 'passes': passed}
    return arms


def verify_selection(selection, directory):
    seen = set()
    for i, entry in enumerate(selection):
        family, digest = entry['class'], entry['sha256']
        if (family not in CLASSES or entry['ext'] not in CLASSES[family][1]
                or len(digest) != 64 or any(c not in '0123456789abcdef' for c in digest)):
            raise ValueError('invalid B4 selection identity')
        if digest in seen:
            raise ValueError('duplicate file in B4 selection')
        seen.add(digest)
        path = directory / f"{digest}.{entry['ext']}"
        actual = hashlib.sha256()
        with path.open('rb') as source:
            for block in iter(lambda: source.read(1024 * 1024), b''):
                actual.update(block)
        if actual.hexdigest() != digest or path.stat().st_size != entry['size']:
            raise ValueError('B4 file differs from the frozen selection')
        entry['file'] = f'{family}:{i}'
    if {e['class'] for e in selection} != set(CLASSES):
        raise ValueError('B4 calibration requires every registered container')


def run(opts):
    cpus = cpu_list(opts.cpus)
    if len(cpus) != 1 or opts.rounds != 5:
        raise ValueError('B4 requires one pinned CPU and five timed pairs')
    os.sched_setaffinity(0, cpus)
    subprocess.run([sys.executable, str(ROOT / 'scripts' / 'oracle_pin.py'), '--verify-bin', opts.oracle_bin],
                   check=True)
    d = pathlib.Path(opts.dir)
    selection = json.loads((d / 'selection.json').read_text())
    verify_selection(selection, d)
    commands = native_commands()
    if not commands:
        raise ValueError('B4 has no native commands to measure')
    log = pathlib.Path(opts.out).with_suffix('.fallback.jsonl')
    log.unlink(missing_ok=True)
    env = release_round.environment(log)
    ffprobe = str(pathlib.Path(opts.oracle_bin) / 'ffprobe')
    calibration = calibrate(selection, d, opts.coreav, ffprobe, env, log)
    results = []
    for n, args in enumerate(commands if all(a['passes'] for a in calibration.values()) else []):
        for e in selection:
            row = {'command': n, 'class': e['class'], 'file': e['file']}
            if not eligible(args, e['class']):
                results.append({**row, 'ineligible': True})
                continue
            path = str(d / f"{e['sha256']}.{e['ext']}")
            argv = [path if a == 'INPUT' else a for a in args]
            results.append({**row, **b4_pair([opts.coreav, 'ffprobe'] + argv, [ffprobe] + argv, env, refuse=True)})
        print(f'{n + 1}/{len(commands)}', file=sys.stderr, flush=True)
    data = {'method': B4_METHOD, 'sides': ['coreav', 'ffprobe'], 'commands': commands, 'rounds': opts.rounds,
            'selection': [{k: e[k] for k in ('class', 'file')} for e in selection],
            'calibration': calibration, 'fallback_log': b4_fallback(log), 'results': results}
    pathlib.Path(opts.out).write_text(json.dumps(data) + '\n')
    return int(not all(a['passes'] for a in calibration.values()) or data['fallback_log']['void']
               or any(r.get('failed') for r in results))


def b4_summary(data):
    if data.get('method') != B4_METHOD:
        raise ValueError('historical B4 results cannot be re-scored with the file-cluster method')
    if data['rounds'] != 5:
        raise ValueError('B4 requires five timed pairs')
    groups = {}
    expected = {(n, e['file']): e['class'] for n in range(len(data['commands'])) for e in data['selection']}
    actual = set()
    for r in data['results']:
        key = (r['command'], r['file'])
        if key not in expected or r['class'] != expected[key] or key in actual:
            raise ValueError('unexpected or duplicate B4 command/input pair')
        if bool(r.get('ineligible')) == eligible(data['commands'][r['command']], r['class']):
            raise ValueError('B4 pair eligibility differs from the registered command family')
        if sum((bool(r.get('ineligible')), bool(r.get('refused')), 'failed' in r, 'ratio' in r)) != 1:
            raise ValueError('B4 pair must have exactly one outcome')
        if 'ratio' in r and (not math.isfinite(r['ratio']) or r['ratio'] <= 0):
            raise ValueError('B4 ratios must be finite and positive')
        actual.add(key)
    measured = {(r['command'], r['file']): r for r in data['results'] if 'ratio' in r}
    for (n, file), family in expected.items():
        if not eligible(data['commands'][n], family):
            continue
        for key in (('all', 'all'), ('class', family), ('command', n), ('command-class', f'{n}:{family}')):
            group = groups.setdefault(key, [])
            if (n, file) in measured:
                group.append(measured[(n, file)])
    rows = []
    for (kind, name), ratios in sorted(groups.items(), key=lambda kv: (kv[0][0], str(kv[0][1]))):
        values = [r['ratio'] for r in ratios]
        rows.append({'group': kind, 'name': name, 'n': len(ratios),
                     'files': len({r['file'] for r in ratios}),
                     'median': statistics.median(values) if values else None,
                     'ci95': interval(ratios),
                     'faster_share': sum(x < 1.0 for x in values) / len(values) if values else None})
    refused = sorted({' '.join(data['commands'][r['command']]) for r in data['results'] if r.get('refused')})
    missed = [r for r in rows if r['median'] is not None and r['median'] > 1.0]
    unmeasured = [r for r in rows if r['median'] is None]
    failed = [r for r in data['results'] if r.get('failed')]
    eligible_pairs = {key for key, family in expected.items() if eligible(data['commands'][key[0]], family)}
    coverage = {'total': len(expected), 'ineligible': len(expected) - len(eligible_pairs),
                'refused': sum(r.get('refused', False) for r in data['results']),
                'failed': len(failed), 'timed': len(measured), 'missing': len(eligible_pairs - actual)}
    coverage['eligible'] = coverage['total'] - coverage['ineligible']
    passes = (bool(rows) and not (missed or unmeasured or failed or coverage['missing'])
              and all(data['calibration'][a]['passes'] for a in ('aa', 'null'))
              and not data['fallback_log']['void'])
    return {'method': B4_METHOD, 'rows': rows, 'refused': refused, 'missed': missed,
            'unmeasured': unmeasured, 'failed': failed, 'coverage': coverage,
            'calibration': data['calibration'], 'fallback_log': data['fallback_log'], 'passes': passes}


def report(opts):
    data = json.loads(pathlib.Path(opts.results).read_text())
    out = b4_summary(data)
    if opts.out:
        pathlib.Path(opts.out).write_text(json.dumps(out, indent=1) + '\n')
    for r in out['rows']:
        if r['group'] in ('all', 'class'):
            print(f"{r['group']:6} {r['name']:9} n={r['n']:5} median={r['median']} ci95={r['ci95']}")
    worst = max((r for r in out['rows'] if r['group'] == 'command' and r['median'] is not None),
                key=lambda r: r['median'], default=None)
    if worst:
        print(f"slowest command: {' '.join(data['commands'][worst['name']])} median={worst['median']:.3f}")
    print(json.dumps({k: out[k] for k in ('coverage', 'passes', 'fallback_log')}))
    return 0 if out['passes'] else 1


# The sides of each arm of the 0.1.0 round: the program on the left of a
# ratio, and on the right.
ARMS = {'r4': ('coreav', 'ffmpeg'), 'aa': ('ffmpeg', 'ffmpeg'), 'null': ('coreav', 'coreav')}
GATE = (0.98, 1.02)
R4_METHOD = 'r4-refusal-coverage-v1'
R4_REASON_GROUPS = {
    'a codec that CoreAV does not copy': 'codec_container',
    'PCM audio is not copied to MP4': 'codec_container',
    'MP4 has no standard sample entry for Vorbis': 'codec_container',
    'only AAC audio can be copied into a clip': 'codec_container',
    'only AAC audio can be extracted': 'codec_container',
    ('VP8 in MP4 does not play in Safari; keep the WebM, which plays in Chrome, Firefox and Safari, '
     'or re-encode to VP9, H.264 or AV1 outside CoreAV'): 'codec_container',
    'a stream that CoreAV cannot copy to MP4': 'codec_container',
    'copying a subtitle stream': 'codec_container',
    'timecode tracks are not copied to MP4': 'codec_container',
    '-fflags +igndts': 'option',
    '-map_metadata -1 on a stream with a language': 'option',
    '-map_metadata 0:s:0': 'option',
    'a selected stream that no -c copy option covers (it would be encoded)': 'option',
    'a selection that maps a stream twice or out of the input\'s order': 'layout',
    'a clip or a segment of other streams than one video and one audio stream': 'layout',
    'a concat list of a file of unknown duration': 'layout',
    'a selected stream does not exist or is not allowed': 'layout',
    'no video is presented in the clip\'s range': 'layout',
    'a join of files whose streams end at different times': 'layout',
    'a join of files whose tracks have several edits': 'layout',
    'a join without video': 'layout',
    'the clip presents no audio': 'layout',
    'the track has no samples': 'layout',
    'the first H.264 keyframe has no parameter sets': 'layout',
    'no video sync sample to start from': 'layout',
    'a segment without video': 'layout',
}


def cpu_list(text):
    """`4-7` or `4,5,6,7` as a set of CPU numbers."""
    out = set()
    for part in text.split(','):
        lo, _, hi = part.partition('-')
        out.update(range(int(lo), int(hi or lo) + 1))
    return out


def r4_files(opts):
    if opts.files:
        return [(e['family'], pathlib.Path(e['path'])) for e in json.loads(pathlib.Path(opts.files).read_text())]
    out = []
    for d in map(pathlib.Path, opts.media):
        for p in sorted(d.iterdir()) if d.is_dir() else [d]:
            if release_round.family(p):
                out.append((release_round.family(p), p))
    return out


def listed_streams(ffprobe, path, env):
    try:
        done = subprocess.run([ffprobe, '-v', 'error', '-of', 'json', '-show_streams', str(path)],
                              stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=600)
        return json.loads(done.stdout)['streams'] if done.returncode == 0 else None
    except (subprocess.TimeoutExpired, ValueError, KeyError):
        return None


def r4_pairs(opts, files, env):
    """(entry, index, command, file label, family, path) for every pair the
    arm times, in the order of the set and of the files."""
    labels, n = [], collections.Counter()
    for fam, path in files:
        labels.append(f'{fam}:{n[fam]}')
        n[fam] += 1
    listings = {}
    out = []
    for entry, i, command in [(e, i, c) for e in release_round.entries() for i, c in enumerate(e['commands'])]:
        for label, (fam, path) in zip(labels, files):
            if fam not in release_round.containers(entry, command):
                continue
            if entry['tool'] == 'ffmpeg':
                if path not in listings:
                    listings[path] = listed_streams(opts.ffprobe, path, env)
                if listings[path] is None or not release_round.can_take(entry, command, fam, listings[path]):
                    continue
            out.append((entry, i, command, label, fam, path))
    return out


def clear(directory):
    for p in directory.iterdir():
        shutil.rmtree(p) if p.is_dir() else p.unlink()


def timed_run(argv, env, scratch):
    """One process, wall time from start to exit; SCRATCH is emptied after
    it, so every run writes to an empty directory."""
    try:
        return timed(argv, env, raise_oserror=True)
    finally:
        clear(scratch)


def r4_invocation(argv, env, scratch, call_log, aggregate_log, side, tool):
    call_log.unlink(missing_ok=True)
    elapsed, code = timed_run(argv, {**env, 'COREAV_FALLBACK_LOG': str(call_log)}, scratch)
    raw = call_log.read_bytes() if call_log.exists() else b''
    if raw:
        with aggregate_log.open('ab') as aggregate:
            aggregate.write(raw)
    lines = [line for line in raw.splitlines() if line.strip()]
    try:
        records = [json.loads(line) for line in lines]
    except (ValueError, UnicodeError):
        records = [None]
    refusal = bool(lines) and raw.endswith(b'\n') and side == 'coreav' and code == 4 and all(
        isinstance(record, dict) and record.get('not_run') == 'strict_safe'
        and 'ran' in record and record['ran'] is None and record.get('tool') == tool
        and isinstance(record.get('reason'), str) and bool(record['reason'].strip())
        for record in records)
    return {'elapsed': elapsed, 'exit': code, 'lines': len(lines), 'refusal': refusal,
            'reasons': sorted({record['reason'] for record in records}) if refusal else []}


def r4_coverage(rows, sides):
    counts = dict.fromkeys(('eligible', 'refused', 'timed', 'failed', 'oracle_failed', 'unprocessed'), 0)
    groups = dict.fromkeys(('codec_container', 'option', 'layout', 'unknown'), 0)
    reasons = collections.Counter()
    oracle_refusal_pairs = 0
    oracle_refusal_reasons = collections.Counter()
    for row in rows:
        counts['eligible'] += 1
        outcome = row.get('outcome')
        if outcome is None:
            if 'ratio' in row:
                outcome = 'timed'
            elif row.get('refused'):
                outcome = 'refused'
            elif 'a' not in row and any(side == 'ffmpeg' and code != 0
                                       for side, code in zip(sides, row.get('exit', []))):
                outcome = 'oracle_failed'
            else:
                outcome = 'failed'
        if outcome == 'eligible' or outcome not in counts:
            raise ValueError('unknown R4 pair outcome')
        counts[outcome] += 1
        if outcome == 'refused':
            groups[row.get('refusal_group', 'unknown')] += 1
            reasons.update(set(row.get('refusal_reasons', [])))
        if outcome == 'oracle_failed' and row.get('oracle_refusal_reasons'):
            oracle_refusal_pairs += 1
            oracle_refusal_reasons.update(set(row['oracle_refusal_reasons']))
    return {**counts, 'timed_share': counts['timed'] / counts['eligible'] if counts['eligible'] else None,
            'refusal_groups': groups, 'refusal_reasons': dict(sorted(reasons.items())),
            'oracle_refusal_pairs': oracle_refusal_pairs,
            'oracle_refusal_reasons': dict(sorted(oracle_refusal_reasons.items()))}


def r4(opts):
    os.sched_setaffinity(0, cpu_list(opts.cpus))
    root = pathlib.Path(opts.scratch)
    out_dir, lists = root / 'out', root / 'lists'
    out_dir.mkdir(parents=True, exist_ok=True)
    lists.mkdir(parents=True, exist_ok=True)
    clear(out_dir)
    log = root / 'fallback.log'
    if log.exists():
        log.unlink()
    call_log = root / 'invocation.log'
    env = release_round.environment(log)
    files = r4_files(opts)
    pairs = r4_pairs(opts, files, env)
    if not pairs:
        print('UNMEASURED: no pair to time')
        return release_round.UNMEASURED
    left, right = ARMS[opts.arm]
    results = [{'id': entry['id'], 'command': i, 'class': entry['job'], 'family': fam,
                'file': label, 'outcome': 'unprocessed'} for entry, i, _, label, fam, _ in pairs]
    commands = [{'id': entry['id'], 'command': i, 'class': entry['job']}
                for entry in release_round.entries() for i, _ in enumerate(entry['commands'])]
    log_lines = 0
    refusal_lines = excluded_lines = diagnostic_lines = 0
    void, aborted, row = False, True, None
    try:
        preflight_lines = [line for line in log.read_bytes().splitlines() if line.strip()] if log.exists() else []
        log_lines = len(preflight_lines)
        void = bool(preflight_lines)
        for k, (entry, _, command, label, _, path) in enumerate(pairs):
            row = results[k]
            row['outcome'] = 'failed'
            source = path
            if entry['job'] == 'join':
                source = release_round.concat_list(lists / label.replace(':', '-'), path)
            argv = release_round.fill(command['args'], source, out_dir / release_round.output_name(entry['job']))
            program = {'coreav': [opts.coreav, entry['tool']],
                       'ffmpeg': [opts.ffmpeg if entry['tool'] == 'ffmpeg' else opts.ffprobe]}
            a, b = program[left] + argv, program[right] + argv
            calls = []
            for argv_, side in ((a, left), (b, right)):
                call = r4_invocation(argv_, env, out_dir, call_log, log, side, entry['tool'])
                calls.append(call)
                log_lines += call['lines']
                refusal_lines += call['lines'] if call['refusal'] else 0
            warm = tuple(call['exit'] for call in calls)
            lines = sum(call['lines'] for call in calls)
            refused = 'coreav' in (left, right) and all(
                call['refusal'] if side == 'coreav' else call['exit'] == 0 and not call['lines']
                for call, side in zip(calls, (left, right)))
            if warm != (0, 0):
                row['exit'] = list(warm)
                row['refused'] = refused
                if refused:
                    reasons = sorted({reason for call in calls for reason in call['reasons']})
                    groups = {R4_REASON_GROUPS.get(reason, 'unknown') for reason in reasons}
                    row.update(outcome='refused', refusal_reasons=reasons,
                               refusal_group=next(iter(groups)) if len(groups) == 1 else 'unknown')
                    excluded_lines += lines
                else:
                    oracle_failed = any(side == 'ffmpeg' and code != 0
                                        for side, code in zip((left, right), warm))
                    if oracle_failed:
                        row['outcome'] = 'oracle_failed'
                    if oracle_failed and 'coreav' in (left, right) and all(
                            call['refusal'] if side == 'coreav' else not call['lines']
                            for call, side in zip(calls, (left, right))):
                        row['oracle_refusal_reasons'] = sorted(
                            {reason for call in calls for reason in call['reasons']})
                        diagnostic_lines += lines
                    else:
                        void |= bool(lines) or (oracle_failed and any(
                            side == 'coreav' and code != 0 for side, code in zip((left, right), warm)))
                continue
            if lines:
                void = True
                row.update(exit=list(warm), log_error=True)
                continue
            times, codes = ([], []), set()
            row.update(a=times[0], b=times[1])
            timed_logs = False
            for _ in range(opts.rounds):
                for side, argv_, name in ((0, a, left), (1, b, right)):
                    call = r4_invocation(argv_, env, out_dir, call_log, log, name, entry['tool'])
                    times[side].append(call['elapsed'])
                    codes.add(call['exit'])
                    log_lines += call['lines']
                    refusal_lines += call['lines'] if call['refusal'] else 0
                    timed_logs |= bool(call['lines'])
            void |= timed_logs
            if codes == {0} and not timed_logs:
                row.update(outcome='timed', ratio=statistics.median(times[0]) / statistics.median(times[1]))
            else:
                row['exit'] = sorted(c if c is not None else -1 for c in codes)
            if (k + 1) % 100 == 0 or k + 1 == len(pairs):
                print(f'{k + 1}/{len(pairs)}', file=sys.stderr, flush=True)
        aborted = False
    except BaseException:
        if row is not None and row['outcome'] == 'failed':
            row['refused'] = False
        raise
    finally:
        pathlib.Path(opts.out).write_text(json.dumps(
            {'set': '0.1.0', 'method': R4_METHOD, 'arm': opts.arm, 'sides': [left, right],
             'rounds': opts.rounds, 'cpus': opts.cpus, 'aborted': aborted,
             'files': dict(collections.Counter(f for f, _ in files)), 'commands': commands,
             'fallback_log': {'lines': log_lines, 'refusal_lines': refusal_lines,
                              'excluded_lines': excluded_lines, 'diagnostic_lines': diagnostic_lines,
                              'void': void}, 'results': results}) + '\n')
    return 0


def bootstrap(ratios, rng):
    """The median's bootstrap 95% interval: 10,000 resamples of the pairs."""
    meds = sorted(statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(10000))
    return [meds[249], meds[9749]]


def r4_summary(data, gate):
    rng = random.Random(0)
    groups = collections.defaultdict(list)
    commands = collections.defaultdict(list)
    for r in data['results']:
        if 'ratio' not in r:
            continue
        groups[(r['class'], r['family'])].append(r['ratio'])
        groups[(r['class'], 'all')].append(r['ratio'])
        commands[f"{r['id']}.{r['command']}"].append(r['ratio'])
    rows = []
    for (cls, fam), ratios in sorted(groups.items()):
        median, ci = statistics.median(ratios), bootstrap(ratios, rng)
        members = [r for r in data['results'] if r['class'] == cls and (fam == 'all' or r['family'] == fam)]
        row = {'class': cls, 'family': fam, 'n': len(ratios), 'median': median, 'ci95': ci,
               'coverage': r4_coverage(members, data['sides']),
               'faster_share': sum(x < 1.0 for x in ratios) / len(ratios)}
        if gate:
            row['passes'] = GATE[0] <= median <= GATE[1] and ci[0] <= 1.0 <= ci[1] if fam == 'all' else None
        else:
            row.update(meets=median <= 1.0, faster=ci[1] < 1.0)
        rows.append(row)
    catalog = {f"{r['id']}.{r['command']}": r for r in data.get('commands', data['results'])}
    per_command = []
    for command in sorted(catalog):
        ratios = commands[command]
        members = [r for r in data['results'] if f"{r['id']}.{r['command']}" == command]
        per_command.append({'command': command, 'n': len(ratios),
                            'median': statistics.median(ratios) if ratios else None,
                            'coverage': r4_coverage(members, data['sides'])})
    no_ratio = collections.Counter(
        'unprocessed' if r.get('outcome') == 'unprocessed'
        else 'refused' if r.get('refused') else 'timed run failed' if 'a' in r
        else 'invalid fallback log' if r.get('log_error')
        else 'infrastructure failure' if 'exit' not in r
        else f"untimed run failed: {' and '.join(s for s, c in zip(data['sides'], r['exit']) if c != 0) or 'neither'}"
        for r in data['results'] if 'ratio' not in r)
    incomplete = [f"{r['id']}.{r['command']} {r['file']}" for r in data['results'] if 'ratio' not in r and 'a' in r]
    failing = [r for r in rows if r.get('passes') is False or r.get('meets') is False]
    present = {key for r in data['results'] for key in ((r['class'], r['family']), (r['class'], 'all'))}
    unmeasured = sorted(f'{cls} {fam}' for cls, fam in present - set(groups) if not gate or fam == 'all')
    coverage_groups = [{'class': cls, 'family': fam,
                        **r4_coverage([r for r in data['results'] if r['class'] == cls
                                       and (fam == 'all' or r['family'] == fam)], data['sides'])}
                       for cls, fam in sorted(present)]
    coverage = r4_coverage(data['results'], data['sides'])
    return {'arm': data['arm'], 'sides': data['sides'], 'files': data['files'], 'gate': gate,
            'method': data.get('method'), 'aborted': data.get('aborted', False) or coverage['unprocessed'] > 0,
            'coverage': coverage,
            'coverage_groups': coverage_groups,
            'fallback_log': data['fallback_log'], 'pairs': len(data['results']), 'no_ratio': dict(no_ratio),
            'classes': rows, 'commands': per_command, 'unmeasured': unmeasured, 'incomplete': incomplete,
            'failing': [f"{r['class']} {r['family']}" for r in failing] + unmeasured}


def r4_report(opts):
    summary = r4_summary(json.loads(pathlib.Path(opts.results).read_text()), opts.gate)
    if opts.out:
        pathlib.Path(opts.out).write_text(json.dumps(summary, indent=1) + '\n')
    def coverage(counts):
        share = f"{counts['timed_share']:.1%}" if counts['timed_share'] is not None else 'unmeasured'
        return (f"refused={counts['refused']} eligible={counts['eligible']} "
                f"timed={counts['timed']} failed={counts['failed']} oracle_failed={counts['oracle_failed']} "
                f"oracle_refusal_pairs={counts['oracle_refusal_pairs']} "
                f"oracle_refusal_reasons={json.dumps(counts['oracle_refusal_reasons'], sort_keys=True)} "
                f"unprocessed={counts['unprocessed']} timed_share={share}")

    if summary['aborted']:
        print(f"ABORTED: the arm did not finish; {coverage(summary['coverage'])}")
    for r in summary['classes']:
        if summary['aborted']:
            verdict = 'ABORTED'
        elif opts.gate:
            verdict = {True: 'pass', False: 'FAIL', None: '(not gated)'}[r['passes']]
        else:
            verdict = 'meets' if r['meets'] else 'MISSES'
        print(f"{r['class']:14} {r['family']:13} n={r['n']:5} median={r['median']:.3f} {coverage(r['coverage'])} "
              f"ci95=[{r['ci95'][0]:.3f}, {r['ci95'][1]:.3f}] faster={r['faster_share']:.0%} {verdict}")
    for row in summary['commands']:
        median = f"{row['median']:.3f}" if row['median'] is not None else 'unmeasured'
        print(f"{row['command']:14} median={median} {coverage(row['coverage'])}")
    for group in summary['unmeasured']:
        print(f'UNMEASURED {group}: pairs ran but none has a ratio, so it fails')
    for pair in summary['incomplete']:
        print(f'INCOMPLETE {pair}: a timed run failed after both untimed runs succeeded, so the run fails')
    print(json.dumps({k: summary[k] for k in ('arm', 'pairs', 'no_ratio', 'failing', 'unmeasured', 'incomplete',
                                              'fallback_log', 'coverage', 'coverage_groups', 'commands', 'aborted')}))
    if summary['aborted']:
        return 1
    if summary['fallback_log']['void']:
        print('VOID: a non-exempt log event or invalid CoreAV failure')
        return 2
    if not summary['classes']:
        print('UNMEASURED: no pair has a ratio')
        return release_round.UNMEASURED
    return 1 if summary['failing'] or summary['incomplete'] else 0


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='mode', required=True)
    p = sub.add_parser('select')
    p.add_argument('--out', required=True)
    p = sub.add_parser('run')
    p.add_argument('--coreav', required=True)
    p.add_argument('--oracle-bin', required=True)
    p.add_argument('--cpus', required=True)
    p.add_argument('--dir', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--rounds', type=int, default=5)
    p = sub.add_parser('report')
    p.add_argument('--results', required=True)
    p.add_argument('--out')
    p = sub.add_parser('r4')
    p.add_argument('--arm', choices=sorted(ARMS), required=True)
    p.add_argument('--coreav', required=True)
    p.add_argument('--ffmpeg', required=True)
    p.add_argument('--ffprobe', required=True)
    p.add_argument('--files')
    p.add_argument('--media', nargs='+', default=[])
    p.add_argument('--scratch', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--rounds', type=int, default=5)
    p.add_argument('--cpus', default='4-7', help='the CPUs every timed process runs on')
    p = sub.add_parser('r4-report')
    p.add_argument('--results', required=True)
    p.add_argument('--out')
    p.add_argument('--gate', action='store_true')
    opts = parser.parse_args()
    return {'select': select, 'run': run, 'report': report, 'r4': r4, 'r4-report': r4_report}[opts.mode](opts)


if __name__ == '__main__':
    sys.exit(main())
