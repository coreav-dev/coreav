#!/usr/bin/env python3
"""R3 on media other than the fixtures (docs/design/compat-cli.md, "The 0.1.0
bars"; docs/measurements/compat-0.1.0/README.md, "Before the round", item 2).

    r3.py run --coreav PATH --harness PATH --ffmpeg PATH --ffprobe PATH
              [--mediainfo PATH] (--media DIR [DIR...] | --list FILE)
              --work DIR --records FILE [--jobs N] [--chunk N]
        Per file: the pinned ffprobe lists its streams, and every `ffmpeg`
        command of commands-0.1.0 the file can take (its family, its
        `input_without`, the streams its `-map` selectors need; a join's
        input is a concat list that names the file twice) runs with the
        pinned ffmpeg first. Only the pairs ffmpeg runs to success go on:
        HARNESS (the coreav-cli `ffmpeg` test executable, test
        `the_0_1_0_commands_on_listed_pairs`) runs them through the
        function `coreav ffmpeg` runs, for the writer's summary the binary
        does not print; the `coreav` binary runs every one of them too,
        refusals included, so that its fallback log holds each refusal, and
        must write the same bytes with the same outcome; copy_check.py judges
        every written output by the oracle of its job, and a chunk whose
        check exits abnormally or leaves no fresh report for all its outputs
        fails them. Outputs are deleted chunk by chunk. FILE gets one line
        per pair (the file's name: private). `run` exits as `report` does on
        its own records.

    r3.py report [--records FILE...] [--summaries FILE...] [--report FILE]
                 [--summary FILE] [--bar 98]
        Per command and per family: the pairs ffmpeg ran to success, how
        many passed, and the commands no file could take. A command meets
        R3 when at least BAR% of its counted pairs pass. --summary names no
        file, and the summaries of several runs add up. A command no file
        can take is reported, not counted; one whose files the pinned ffprobe
        or ffmpeg failed on every time is unmeasured and misses. Exit 1 when a
        command misses, 2 when a run is void (its fallback log holds a
        line, a strict-safe refusal included), 3 when no pair was counted.
        `run` exits 3 when it has no file to run.

Both tools run with no gconv modules; the binary's runs share a fallback
log. ffmpeg, ffprobe and MediaInfo are black boxes. Python 3 standard
library only.
"""
import argparse
import collections
import concurrent.futures
import hashlib
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import release_round  # noqa: E402

TIMEOUT = 600
TEST = 'the_0_1_0_commands_on_listed_pairs'


def media_files(opts):
    if opts.list:
        return [pathlib.Path(line.strip()) for line in open(opts.list) if line.strip()]
    out = []
    for d in map(pathlib.Path, opts.media):
        out.extend(sorted(p for p in d.iterdir() if p.is_file()) if d.is_dir() else [d])
    return [p for p in out if release_round.family(p)]


def run(argv, env, timeout=TIMEOUT):
    try:
        return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, env=env,
                              timeout=timeout).returncode
    except subprocess.TimeoutExpired:
        return None


def streams(ffprobe, path, env):
    try:
        done = subprocess.run([ffprobe, '-v', 'error', '-of', 'json', '-show_streams', str(path)],
                              stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=TIMEOUT)
        return json.loads(done.stdout)['streams'] if done.returncode == 0 else None
    except (subprocess.TimeoutExpired, ValueError, KeyError):
        return None


def written(work):
    """The bytes of what a run left in its directory, by name, as a digest."""
    h = hashlib.sha256()
    for p in sorted(pathlib.Path(work).iterdir()):
        h.update(p.name.encode() + b'\0')
        with open(p, 'rb') as f:
            for block in iter(lambda: f.read(1 << 20), b''):
                h.update(block)
    return h.hexdigest()


def binary_outcome(code):
    """The harness status the binary's exit status stands for
    (crates/coreav-cli/src/compat_cmd.rs)."""
    return {0: 'written', 4: 'refused'}.get(code, 'failed')


def plan(opts, path, env, scratch):
    """The pairs of one file: (pair, outcome) where outcome is None for a
    pair ffmpeg ran to success, else why it is not counted."""
    fam = release_round.family(path)
    listed = streams(opts.ffprobe, path, env)
    out = []
    for entry, n, command in release_round.commands('ffmpeg'):
        if fam not in release_round.containers(entry, command):
            continue
        pair = {'id': entry['id'], 'job': entry['job'], 'command': n, 'args': command['args'],
                'source': str(path), 'family': fam}
        if listed is None:
            out.append((pair, 'ffprobe fails'))
            continue
        if not release_round.can_take(entry, command, fam, listed):
            out.append((pair, 'cannot take'))
            continue
        base = scratch / f"{entry['id']}.{n}"
        pair['input'] = str(release_round.concat_list(base / 'list', path) if entry['job'] == 'join' else path)
        reference = base / 'ffmpeg'
        reference.mkdir(parents=True)
        code = run([opts.ffmpeg] + release_round.fill(command['args'], pair['input'],
                                                       reference / release_round.output_name(entry['job'])), env)
        ok = code == 0 and any(reference.iterdir())
        shutil.rmtree(reference)
        out.append((pair, None if ok else 'ffmpeg fails'))
    return out


def judge(opts, pairs, scratch, env):
    """Runs one chunk of counted pairs through the harness, the binary and
    copy_check.py: each pair's outcome and, for a failure, why."""
    for pair in pairs:
        base = scratch / f"{pair['id']}.{pair['command']}"
        pair['work'] = str(base / 'coreav')
        pair['output'] = str(base / 'coreav' / release_round.output_name(pair['job']))
        pair['args_filled'] = release_round.fill(pair['args'], pair['input'], pair['output'])
    # The binary runs every pair first, whatever the harness makes of it, so
    # that each refusal writes its line to the run's fallback log.
    binary = {}
    for p in pairs:
        theirs = scratch / f"{p['id']}.{p['command']}" / 'binary'
        shutil.rmtree(theirs, ignore_errors=True)
        theirs.mkdir(parents=True)
        outcome = binary_outcome(run([opts.coreav, 'ffmpeg'] + release_round.fill(
            p['args'], p['input'], theirs / release_round.output_name(p['job'])), env))
        binary[(p['id'], p['command'])] = (outcome, written(theirs) if outcome == 'written' else None)
        shutil.rmtree(theirs)
    listing, records_path = scratch / 'pairs.json', scratch / 'records.json'
    records_path.unlink(missing_ok=True)
    listing.write_text(json.dumps([{**p, 'args': p['args_filled']} for p in pairs]))
    harness_env = {**env, 'COREAV_R3_PAIRS': str(listing), 'COREAV_R3_RECORDS': str(records_path)}
    code = run([opts.harness, '--ignored', '--exact', TEST, '--test-threads', '1'], harness_env,
               timeout=TIMEOUT * len(pairs))
    if code != 0 or not records_path.exists():
        return {(p['id'], p['command']): ('failed', 'harness crashed' if code is not None else 'timeout')
                for p in pairs}
    records = json.loads(records_path.read_text())
    results = {}
    for record in records:
        key = (record['id'], record['command'])
        record['args'] = next(p['args'] for p in pairs if (p['id'], p['command']) == key)
        if record['status'] != 'written' or not record['files']:
            results[key] = ('failed', record['status'] if record['status'] != 'written' else 'no output')
        elif binary[key] != ('written', written(record['work'])):
            results[key] = ('failed', 'binary differs')
    judged = [r for r in records if (r['id'], r['command']) not in results]
    if judged:
        (scratch / 'judged.json').write_text(json.dumps(judged))
        check = scratch / 'check.json'
        check.unlink(missing_ok=True)
        cmd = [sys.executable, '-I', str(HERE / 'copy_check.py'), '--records', str(scratch / 'judged.json'),
               '--ffmpeg', opts.ffmpeg, '--ffprobe', opts.ffprobe, '--jobs', '1', '--report', str(check)]
        if opts.mediainfo:
            cmd += ['--mediainfo', opts.mediainfo]
        with open(opts.log, 'ab') as log:
            code = subprocess.run(cmd, stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=env,
                                  check=False).returncode
        report = json.loads(check.read_text()) if code in (0, 1) and check.exists() else None
        if report and sum(c.get('written', 0) for c in report['counts'].values()) != len(judged):
            report = None
        failed = {}
        for f in (report or {}).get('failures', []):
            failed[(f['id'], tuple(f['args']), f['source'])] = 'selection' if f['selection'] else 'oracle'
        for r in judged:
            key = (r['id'], r['command'])
            if report is None:
                results[key] = ('failed', 'copy_check crashed')
            elif (r['id'], tuple(r['args']), r['source']) in failed:
                results[key] = ('failed', failed[(r['id'], tuple(r['args']), r['source'])])
            else:
                results[key] = ('passed', None)
    return results


def one_file(opts, path, env):
    scratch = pathlib.Path(tempfile.mkdtemp(prefix='r3-', dir=opts.work))
    try:
        planned = plan(opts, path, env, scratch)
        counted = [p for p, why in planned if why is None]
        results = {}
        for i in range(0, len(counted), opts.chunk):
            chunk = counted[i:i + opts.chunk]
            for p in chunk:
                shutil.rmtree(scratch / f"{p['id']}.{p['command']}" / 'coreav', ignore_errors=True)
            results.update(judge(opts, chunk, scratch, env))
            for p in chunk:
                shutil.rmtree(scratch / f"{p['id']}.{p['command']}" / 'coreav', ignore_errors=True)
        lines = []
        for p, why in planned:
            outcome, reason = results.get((p['id'], p['command']), (why, None)) if why is None else (why, None)
            lines.append({'file': path.name, 'family': p['family'], 'id': p['id'], 'command': p['command'],
                          'job': p['job'], 'outcome': outcome, 'reason': reason})
        return lines
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def verdict(summary):
    """The exit status of a summary: 2 void, 3 nothing counted, 1 a command
    missed or unmeasured, else 0."""
    if summary['fallback_log']['void']:
        print('VOID: the fallback log holds a line')
        return 2
    if not any(t['counted'] for t in summary['commands']):
        print('UNMEASURED: no pair that ffmpeg ran to success')
        return release_round.UNMEASURED
    for cmd in summary['unmeasured']:
        print(f'UNMEASURED {cmd}: files took it but the pinned ffprobe or ffmpeg failed on every one, so it misses R3')
    return 1 if summary['missed'] or summary['unmeasured'] else 0


def cmd_run(opts):
    files = media_files(opts)
    if not files:
        print('UNMEASURED: no file to run')
        return release_round.UNMEASURED
    pathlib.Path(opts.work).mkdir(parents=True, exist_ok=True)
    opts.log = str(pathlib.Path(opts.records).with_suffix('.copy_check.log'))
    log = pathlib.Path(opts.work) / 'fallback.log'
    env = release_round.environment(log)
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        per_file = list(pool.map(lambda p: one_file(opts, p, env), files))
    lines = [line for chunk in per_file for line in chunk]
    lines.append({'fallback_log': release_round.fallback_verdict(log)})
    pathlib.Path(opts.records).write_text(''.join(json.dumps(line) + '\n' for line in lines))
    outcomes = collections.Counter(line.get('outcome') for line in lines if 'outcome' in line)
    print(json.dumps({'files': len(files), 'pairs': dict(outcomes)}))
    return verdict(summarize(counts(lines), opts.bar))


def counts(lines):
    """R3's counts from per-pair lines: per (command, family) cell, the
    pairs by outcome; the files per family; the fallback log. They name no
    file, so the counts of several runs add up."""
    log = {'lines': 0, 'refused': 0, 'void': False}
    cells = collections.defaultdict(collections.Counter)
    files = collections.defaultdict(set)
    for line in lines:
        if 'fallback_log' in line:
            add_log(log, line['fallback_log'])
            continue
        files[line['family']].add(line['file'])
        c = cells[f"{line['id']}.{line['command']}|{line['family']}"]
        c[line['outcome']] += 1
        if line['outcome'] == 'failed':
            c['failed: ' + line['reason']] += 1
    return {'fallback_log': log, 'files': {f: len(s) for f, s in files.items()},
            'cells': {k: dict(c) for k, c in cells.items()}}


def add_log(into, log):
    into['lines'] += log['lines']
    into['refused'] += log['refused']
    into['void'] = into['void'] or log['void']


def merge(parts):
    out = {'fallback_log': {'lines': 0, 'refused': 0, 'void': False}, 'files': collections.Counter(),
           'cells': collections.defaultdict(collections.Counter)}
    for part in parts:
        add_log(out['fallback_log'], part['fallback_log'])
        out['files'].update(part['files'])
        for k, c in part['cells'].items():
            out['cells'][k].update(c)
    return {'fallback_log': out['fallback_log'], 'files': dict(out['files']),
            'cells': {k: dict(c) for k, c in out['cells'].items()}}


def summarize(c, bar):
    """R3 from its counts: per command (`id.n`) over its families, the
    counted pairs (those ffmpeg ran to success), how many passed, and
    whether at least BAR% did."""
    commands = collections.defaultdict(collections.Counter)
    rows = []
    for key in sorted(c['cells']):
        cmd, fam = key.split('|')
        rows.append({'command': cmd, 'family': fam, **c['cells'][key]})
        commands[cmd].update(c['cells'][key])
    totals = []
    for cmd, t in sorted(commands.items()):
        counted = t['passed'] + t['failed']
        totals.append({'command': cmd, 'counted': counted, 'passed': t['passed'],
                       'rate': round(100.0 * t['passed'] / counted, 2) if counted else None,
                       'meets': t['passed'] * 100.0 >= bar * counted if counted else None})
    unmeasured = [cmd for cmd, t in sorted(commands.items())
                  if not t['passed'] + t['failed'] and (t['ffmpeg fails'] or t['ffprobe fails'])]
    return {'bar': bar, 'fallback_log': c['fallback_log'], 'files': dict(sorted(c['files'].items())),
            'cells': rows, 'commands': totals,
            'missed': [t['command'] for t in totals if t['meets'] is False],
            'unmeasured': unmeasured,
            'taken_by_no_file': [t['command'] for t in totals if not t['counted'] and t['command'] not in unmeasured],
            'counts': c}


def cmd_report(opts):
    lines = [json.loads(line) for path in opts.records for line in open(path) if line.strip()]
    parts = [counts(lines)] + [json.loads(pathlib.Path(p).read_text())['counts'] for p in opts.summaries]
    summary = summarize(merge(parts), opts.bar)
    if opts.summary:
        pathlib.Path(opts.summary).write_text(json.dumps(summary, indent=1) + '\n')
    if opts.report:
        failures = [line for line in lines if line.get('outcome') == 'failed']
        pathlib.Path(opts.report).write_text(json.dumps({**summary, 'failures': failures}, indent=1) + '\n')
    print(json.dumps({'files': summary['files'], 'commands': len(summary['commands']),
                      'missed': summary['missed'], 'unmeasured': summary['unmeasured'],
                      'taken_by_no_file': summary['taken_by_no_file'],
                      'fallback_log': summary['fallback_log']}))
    return verdict(summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='mode', required=True)
    p = sub.add_parser('run')
    p.add_argument('--coreav', required=True)
    p.add_argument('--harness', required=True)
    p.add_argument('--ffmpeg', required=True)
    p.add_argument('--ffprobe', required=True)
    p.add_argument('--mediainfo')
    p.add_argument('--media', nargs='+', default=[])
    p.add_argument('--list')
    p.add_argument('--work', required=True)
    p.add_argument('--records', required=True)
    p.add_argument('--jobs', type=int, default=4)
    p.add_argument('--chunk', type=int, default=16, help='pairs whose outputs are on disk at once, per file')
    p.add_argument('--bar', type=float, default=98.0)
    p = sub.add_parser('report')
    p.add_argument('--records', nargs='+', default=[])
    p.add_argument('--summaries', nargs='+', default=[], help='--summary files of other runs, added up')
    p.add_argument('--report')
    p.add_argument('--summary')
    p.add_argument('--bar', type=float, default=98.0)
    opts = parser.parse_args()
    return cmd_run(opts) if opts.mode == 'run' else cmd_report(opts)


if __name__ == '__main__':
    sys.exit(main())
