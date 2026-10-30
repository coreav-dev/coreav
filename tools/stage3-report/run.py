#!/usr/bin/env python3
"""Regenerate the stage 3 correctness report (docs/design/stage3-report.md).

    python3 -I tools/stage3-report/run.py --work DIR [--steps build,fixtures,corpus,hostile,report]
                                          [--workers N] [--recheck stale|uncovered|errors|all]

The corpus step resumes: files with a record in WORK are not checked again,
unless `--recheck` names them (`stale`: every file whose record other code
made; see `corpus.py` and docs/design/stage3-report.md, "Rechecks", for the
others; `affected` is `corpus.py`'s alone, as it needs the codecs or kinds
a change touches).

Each step writes WORK/results/STEP.json with the commit it ran on; `report`
writes docs/measurements/stage3-correctness.md and .json from whatever
results exist, so a step can be rerun alone. Tools are launched without a
shell; the pinned ffmpeg, ffprobe and MediaInfo must be on PATH.

The build and hostile steps also run the engines of an `extra_hostile`
module beside this file, when this tree has one: its `build(run)` and
`hostile(work, args, suite, run, env)`, and its own options
(`options(parser)`); without it they are native only.
"""
import argparse
import datetime
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
import causes  # noqa: E402
import corpus  # noqa: E402

REPORT = ROOT / 'docs' / 'measurements' / 'stage3-correctness'
STEPS = ('build', 'fixtures', 'corpus', 'hostile', 'report')
RELEASE = ROOT / 'target' / 'release'
try:
    import extra_hostile  # noqa: E402
except ImportError:
    extra_hostile = None


def git(*args):
    return subprocess.run(['git', '-C', str(ROOT), *args], capture_output=True, text=True).stdout.strip()


def provenance():
    return {'commit': git('rev-parse', 'HEAD'), 'dirty': bool(git('status', '--porcelain', '--untracked-files=no')),
            'time_utc': datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}


def env(**extra):
    """The environment of a tool: no R2 credentials, unless the caller passes
    them in `extra` (the corpus step only)."""
    e = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    for key in ('R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY', 'R2_ENDPOINT', 'AWS_ACCESS_KEY_ID',
                'AWS_SECRET_ACCESS_KEY', 'AWS_SESSION_TOKEN'):
        e.pop(key, None)
    e.update(extra)
    return e


def run(command, cwd=ROOT, timeout=3600, **extra):
    """Runs a command; returns (exit status, stdout, stderr)."""
    print('+', ' '.join(str(c) for c in command), flush=True)
    done = subprocess.run([str(c) for c in command], cwd=cwd, capture_output=True, text=True, timeout=timeout,
                          env=env(**extra))
    return done.returncode, done.stdout, done.stderr


def save(work, step, value):
    path = pathlib.Path(work) / 'results' / f'{step}.json'
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({'provenance': provenance(), **value}, indent=1) + '\n')


def versions():
    """The first non-empty line each tool prints for its version."""
    def first(command):
        out = subprocess.run(command, capture_output=True, text=True).stdout
        return next((line.strip() for line in out.split('\n') if line.strip()), None)
    return {'ffmpeg': first(['ffmpeg', '-version']), 'ffprobe': first(['ffprobe', '-version']),
            'mediainfo': next((line.strip() for line in subprocess.run(['mediainfo', '--Version'], capture_output=True,
                               text=True).stdout.split('\n') if 'MediaInfoLib' in line), None)}


# ------------------------------------------------------------------- build

def step_build(work, args):
    status, _, err = run(['cargo', 'build', '--locked', '--release', '-p', 'coreav-cli', '-p', 'coreav-oracle',
                          '-p', 'coreav-isobmff-adapter', '-p', 'coreav-mkv-adapter'], timeout=7200)
    if status:
        sys.exit(f'build failed:\n{err[-2000:]}')
    built = {'coreav_sha256': sha256(RELEASE / 'coreav'), 'tools': versions()}
    if extra_hostile:
        built.update(extra_hostile.build(run))
    save(work, 'build', built)


def sha256(path):
    import hashlib
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


# ---------------------------------------------------------------- fixtures

def first_count(text):
    """The first `N/M` of a checker's summary line, as (N, M): the files or
    outputs that pass."""
    found = re.findall(r'(\d+)/(\d+)', text or '')
    return [int(x) for x in found[0]] if found else None


def count_compat(stdout, report):
    """compat_check.py: files whose every compared key agrees."""
    r = json.loads(pathlib.Path(report).read_text())
    failing = {d.get('file') for d in r['diffs']} | {d['file'] for d in r['rejected_without_known_divergence']}
    return [r['compared_files'] - len(failing), r['compared_files']], f"{r['compared_files']} files, " \
        f"{len(r['diffs'])} differing keys, {len(r['via_known_divergence'])} through known divergences"


def count_extract(stdout, report):
    """extract_check.py: extractions that pass every oracle, from its report
    (its last line can be the expected refusal of a stream, not the count)."""
    r = json.loads(pathlib.Path(report).read_text())
    refused = f", {len(r['refused'])} refused as expected" if r.get('refused') else ''
    return [r['passed'], r['total']], f"{r['passed']}/{r['total']} extractions pass every oracle " \
        f"({r['direct']} directly){refused}; {len(r['missing'])} missing"


def count_policy(stdout, report):
    s = json.loads(pathlib.Path(report).read_text())['summary']
    o = s['oracle']
    return [s['cases'] - s['failures'], s['cases']], f"{s['cases']} file and policy cases, {s['failures']} " \
        f"failures; ffprobe-based reference {o['agree'] + o['covered']}/{o['compared']}"


def count_text(stdout, report):
    t = json.loads(pathlib.Path(report).read_text())['totals']
    passed = t['subtitles']['passed'] + t['chapters']['passed']
    total = t['subtitles']['total'] + t['chapters']['total']
    return [passed, total], f"{t['subtitles']['passed']}/{t['subtitles']['total']} subtitle tracks, " \
        f"{t['chapters']['passed']}/{t['chapters']['total']} chapter lists"


def count_split(stdout, report):
    """split_check.py: splits whose every segment passes every oracle."""
    r = json.loads(pathlib.Path(report).read_text())
    k = r['packets']
    return [r['passed'], r['total']], f"{r['passed']}/{r['total']} splits pass every oracle ({r['direct']} " \
        f"directly, {r['passed'] - r['direct']} through known divergences); packets " \
        f"{k['agree']}/{k['agree'] + k['disagree']}"


def count_hls(stdout, report):
    """hls_check.py: packaged inputs that pass every oracle, over those
    and the inputs refused that CI does not expect to be refused; the
    expected refusals are listed, not counted."""
    r = json.loads(pathlib.Path(report).read_text())
    s = r['summary']
    unexpected = sum(1 for x in r['results'] if x.get('unexpected'))
    passed = s['agree'] + s['covered']
    return [passed, s['packaged'] + unexpected], f"{passed}/{s['packaged'] + unexpected} inputs pass every " \
        f"oracle ({s['agree']} directly), {s['refused'] - unexpected} refused as expected, {unexpected} " \
        f"refused unexpectedly; packets {s['packets_agree']}/{s['packets_agree'] + s['packets_disagree']}"


def hls_expected_refused():
    """The inputs the HLS fixture check expects the packager to refuse, as
    ci.yml's isobmff-writers job passes them, so the report runs the check
    as CI does."""
    found = re.findall(r'hls_check\.py[^\n]*(?:\\\n[^\n]*)*?--expect-refused "([^"]*)"',
                       (ROOT / '.github' / 'workflows' / 'ci.yml').read_text())
    if len(found) != 1:
        sys.exit('ci.yml: expected one --expect-refused list for hls_check.py')
    return found[0]


def count_mkv_remux(stdout, report):
    r = json.loads(stdout)
    return list(r['files']), f"{r['files'][0]}/{r['files'][1]} files, packets {r['packets'][0]}/{r['packets'][1]}"


def probe_oracle(work, media, adapter, kd_script, name, extra_kd=()):
    out = work / name
    shutil.rmtree(out, ignore_errors=True)
    for side, extra in (('reference', []), ('candidate', ['--adapter', RELEASE / adapter])):
        status, _, err = run([ROOT / 'scripts' / 'oracle-capture.sh', '--media', media, '--out', out / side,
                              '--levels', 'streams,packets', *extra])
        if status:
            return {'passed': False, 'error': f'{side} capture failed: {err[-300:]}'}
    run([RELEASE / 'coreav-oracle', 'compare', '--reference', out / 'reference', '--candidate', out / 'candidate',
         '--levels', 'streams,packets', '--profile', 'fixtures', '--max-diffs', '1000000',
         '--report', out / 'report.json'])
    status, _, err = run([sys.executable, '-I', ROOT / 'scripts' / kd_script, '--report', out / 'report.json',
                          '--reference', out / 'reference', '--candidate', out / 'candidate',
                          '--out', out / 'known-divergences.json', *extra_kd])
    overlay = json.loads((out / 'known-divergences.json').read_text())
    levels = {level: {k: v for k, v in groups['in_scope'].items() if k not in ('covered_items',)}
              for level, groups in overlay['levels'].items()}
    return {'passed': status == 0 and overlay.get('passed', False), 'levels': levels}


def three_way(oracle_dir, media, title, private=False):
    mi = oracle_dir / 'mediainfo'
    run([sys.executable, '-I', ROOT / 'tools' / 'mediainfo-oracle' / 'mediainfo_oracle.py', 'capture',
         '--media', media, '--out', mi])
    command = [sys.executable, '-I', ROOT / 'tools' / 'mediainfo-oracle' / 'mediainfo_oracle.py', 'report',
               '--ffprobe', oracle_dir / 'reference', '--coreav', oracle_dir / 'candidate', '--mediainfo', mi,
               '--out', oracle_dir / 'three-way.json', '--title', title]
    run(command + (['--private'] if private else []))
    return json.loads((oracle_dir / 'three-way.json').read_text())['summary']


def checker(name, command, cwd=ROOT, counter=None, report=None, **extra):
    """Runs a fixture checker; its exit status decides, `counter` reads its
    counts (else the first N/M of its last line). A report left by an earlier
    run in the same WORK is removed first, so that a check that stops before
    writing one shows no count rather than the old one."""
    if report is not None:
        pathlib.Path(report).unlink(missing_ok=True)
    status, out, err = run(command, cwd=cwd, **extra)
    last = out.strip().split('\n')[-1] if out.strip() else ''
    try:
        count, summary = counter(out, report) if counter else (first_count(last), last[:300])
    except (OSError, ValueError, KeyError):
        count, summary = None, last[:300]
    return {'name': name, 'passed': status == 0, 'status': status, 'count': count, 'summary': summary,
            'stderr_tail': err[-300:] if status else ''}


def step_fixtures(work, args):
    # Read first: a workflow it cannot read stops the step before the hour
    # of work below.
    hls_refused = hls_expected_refused()
    fx, fxm = work / 'fixtures', work / 'fixtures-mkv'
    for script, target in (('gen-fixtures.sh', fx), ('gen-fixtures-mkv.sh', fxm)):
        shutil.rmtree(target, ignore_errors=True)
        status, _, err = run(['bash', ROOT / 'scripts' / script, target], timeout=3600,
                             COREAV_REQUIRE_ALL_FIXTURES='1')
        if status:
            sys.exit(f'{script} failed:\n{err[-2000:]}')
    generated = {k: sum(1 for line in (d / 'MANIFEST').read_text().split('\n') if line.startswith('generated '))
                 for k, d in (('isobmff', fx), ('matroska', fxm))}
    result = {'generated': generated, 'probe': {}, 'checks': [], 'mediainfo': {}}
    result['probe']['isobmff'] = probe_oracle(work, fx, 'isobmff-adapter', 'oracle_kd.py', 'oracle-isobmff',
                                              ['--media', fx])
    # KD-MKV-DECLARED-SIZE reads the files' track headers, as in the mkv-fixtures job.
    result['probe']['matroska'] = probe_oracle(work, fxm, 'mkv-adapter', 'oracle_kd_mkv.py', 'oracle-matroska',
                                               ['--media', fxm])
    kd = work / 'oracle-isobmff' / 'known-divergences.json'
    coreav = RELEASE / 'coreav'
    py = [sys.executable, '-I']
    checks = result['checks']
    checks.append(checker('probe --ffprobe-compat, MP4/MOV', py + [ROOT / 'scripts' / 'compat_check.py',
                          '--coreav', coreav, '--media', fx, '--kd-overlay', kd,
                          '--report', work / 'compat.json'], counter=count_compat, report=work / 'compat.json'))
    kd_mkv = work / 'oracle-matroska' / 'known-divergences.json'
    checks.append(checker('probe --ffprobe-compat, Matroska', py + [ROOT / 'scripts' / 'compat_check.py',
                          '--coreav', coreav, '--media', fxm, '--kd-overlay', kd_mkv,
                          '--report', work / 'compat-mkv.json'],
                          counter=count_compat, report=work / 'compat-mkv.json'))
    mediainfo = shutil.which('mediainfo')
    cargo_test = ['cargo', 'test', '--locked', '--release', '-p', 'coreav-isobmff-write', '--test']
    for name, test, var, out in (('extract-audio', 'extract', 'COREAV_EXTRACT_OUT', 'extract'),
                                 ('clip', 'clip', 'COREAV_CLIP_OUT', 'clip'),
                                 ('remux', 'remux', 'COREAV_REMUX_OUT', 'remux'),
                                 ('sanitize', 'remux', 'COREAV_SANITIZE_OUT', 'sanitize'),
                                 ('split', 'split', 'COREAV_SPLIT_OUT', 'split')):
        target = work / out
        shutil.rmtree(target, ignore_errors=True)
        filt = 'fixtures_sanitize_round_trip' if name == 'sanitize' else 'fixtures_round_trip'
        status, _, err = run(cargo_test + [test, filt, '--', '--ignored'], timeout=3600,
                             COREAV_FIXTURES=str(fx), **{var: str(target)})
        if status:
            checks.append({'name': name, 'passed': False, 'status': status, 'summary': err[-300:]})
            continue
        script = {'extract-audio': 'extract_check.py', 'clip': 'clip_check.py',
                  'split': 'split_check.py'}.get(name, 'remux_check.py')
        command = py + [ROOT / 'scripts' / script, '--manifest', target / 'manifest.json', '--mediainfo', mediainfo,
                        '--report', work / f'{out}.json']
        if name == 'extract-audio':
            command += ['--media', fx, '--kd-overlay', kd]
            checks.append(checker(name, command, counter=count_extract, report=work / f'{out}.json'))
            continue
        if name == 'split':
            checks.append(checker(name, command, counter=count_split, report=work / f'{out}.json'))
            continue
        checks.append(checker(name, command))
    # HLS: the round trip through coreav-isobmff, then the packages checked
    # against ffprobe, ffmpeg and MediaInfo (the isobmff-writers job).
    status, _, err = run(cargo_test + ['hls', 'fixtures_round_trip', '--', '--ignored'], timeout=3600,
                         COREAV_FIXTURES=str(fx))
    if status:
        checks.append({'name': 'hls package', 'passed': False, 'status': status, 'summary': err[-300:]})
    else:
        checks.append(checker('hls package', py + [ROOT / 'scripts' / 'hls_check.py', '--coreav', coreav,
                              '--media', fx, '--mediainfo', mediainfo, '--master-decode',
                              '--expect-refused', hls_refused, '--report', work / 'hls.json'],
                              counter=count_hls, report=work / 'hls.json'))
    checks.append(checker('extract-subtitles and chapters', py + [ROOT / 'scripts' / 'text_check.py', '--coreav',
                          coreav, '--media', fx, '--mediainfo', mediainfo, '--kd-overlay', kd,
                          '--report', work / 'text.json'], counter=count_text, report=work / 'text.json'))
    policies = ['--policy', ROOT / 'scripts' / 'policy_fixtures.json']
    for p in sorted((ROOT / 'docs' / 'schema' / 'policy-examples').glob('*.json')):
        policies += ['--policy', p]
    checks.append(checker('check (upload policy)', py + [ROOT / 'scripts' / 'policy_check.py', '--coreav', coreav,
                          *policies, '--media', fx, '--ffprobe', 'ffprobe', '--oracle-frames',
                          '--report', work / 'policy.json'], counter=count_policy, report=work / 'policy.json'))
    checks.append(checker('Matroska and WebM remux', py + [ROOT / 'scripts' / 'mkv_remux_check.py', 'fixtures',
                          '--coreav', coreav, '--media', fxm, '--out', work / 'mkv-remux', '--mediainfo',
                          mediainfo], counter=count_mkv_remux))
    result['mediainfo']['isobmff'] = three_way(work / 'oracle-isobmff', fx,
                                               'Three-way stream report on the MP4/MOV fixtures')
    result['mediainfo']['matroska'] = three_way(work / 'oracle-matroska', fxm,
                                                'Three-way stream report on the Matroska fixtures')
    save(work, 'fixtures', result)


# ------------------------------------------------------------------ corpus

def step_corpus(work, args):
    result = {'families': {}, 'mediainfo': {}}
    for family in ('isobmff', 'matroska', 'mpegts'):
        out = work / f'corpus-{family}'
        status, _, err = run([sys.executable, '-I', HERE / 'corpus.py', 'run', '--family', family, '--out', out,
                              '--work', work / f'work-{family}', '--workers', str(args.workers),
                              *(['--recheck', args.recheck] if args.recheck else [])],
                             timeout=6 * 3600, **corpus_credentials())
        if status:
            sys.exit(f'corpus run failed ({family}):\n{err[-2000:]}')
        result['families'][family] = corpus.summarize(out, family)
        result['families'][family]['causes'] = causes.group(out)[1]
        runs = out / 'runs.jsonl'
        result['families'][family]['runs'] = ([json.loads(line) for line in runs.read_text().split('\n') if line]
                                              if runs.exists() else [])
        mi = out / 'three-way.json'
        run([sys.executable, '-I', ROOT / 'tools' / 'mediainfo-oracle' / 'mediainfo_oracle.py', 'report',
             '--ffprobe', out / 'ffprobe', '--coreav', out / 'coreav', '--mediainfo', out / 'mediainfo',
             '--out', mi, '--private', '--title',
             f'Three-way stream report, real {family} files of the corpus'])
        result['mediainfo'][family] = json.loads(mi.read_text())['summary']
    save(work, 'corpus', result)


def corpus_credentials():
    keys = ('R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY', 'R2_ENDPOINT')
    if not all(os.environ.get(k) for k in keys):
        sys.exit('the corpus step needs R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and R2_ENDPOINT')
    return {k: os.environ[k] for k in keys}


# ----------------------------------------------------------------- hostile

def step_hostile(work, args):
    suite = work / 'hostile'
    shutil.rmtree(suite, ignore_errors=True)
    fx = work / 'fixtures'
    if not (fx / 'MANIFEST').exists():
        sys.exit('the hostile step needs the fixtures step first')
    status, out, err = run([sys.executable, '-I', ROOT / 'tools' / 'hostile' / 'generate.py',
                            suite, fx, shutil.which('ffmpeg')])
    if status:
        sys.exit(f'hostile generation failed:\n{err[-2000:]}')
    manifest = json.loads((suite / 'manifest.json').read_text())
    status, out, err = run([sys.executable, '-I', ROOT / 'scripts' / 'cli_hostile_check.py', '--coreav',
                            RELEASE / 'coreav', '--suite', suite], timeout=7200)
    native = json.loads(out) if out.strip().startswith('{') else {'error': err[-500:]}
    native['passed'] = status == 0
    extra = extra_hostile.hostile(work, args, suite, run, env) if extra_hostile else None
    rows = extra['rows'] if extra else None
    by_part = {}
    for case in manifest['cases']:
        by_part.setdefault(case['part'], {'reject': 0, 'bounded': 0})[case['outcome']] += 1
    cves = [{k: case.get(k) for k in ('name', 'cve', 'component', 'outcome')}
            for case in manifest['cases'] if case.get('cve')]
    # A reject case whose probe succeeds must have dropped every stream: the
    # cases the extra engines' native reference probed, or, without them,
    # every reject case.
    if rows is not None:
        candidates = [row['name'] for row in rows
                      if row['expect'] == 'reject' and row['native']['probe'] == 'ok']
    else:
        candidates = [case['name'] for case in manifest['cases'] if case['outcome'] == 'reject']
    reject_ok = []
    for name in candidates:
        status_, doc, _ = run([RELEASE / 'coreav', 'probe', suite / name])
        if rows is not None or status_ == 0:
            streams = json.loads(doc).get('streams', []) if status_ == 0 else []
            reject_ok.append({'name': name, 'allowed_streams': len(streams)})
    result = {'cases': len(manifest['cases']), 'by_part': by_part, 'cves': cves, 'native': native}
    if extra:
        result.update(extra['before'])
    result['reject_cases_probed_ok'] = reject_ok
    if extra:
        result.update(extra['after'])
    result['manifest_sha256'] = sha256(suite / 'manifest.json')
    save(work, 'hostile', result)


# ------------------------------------------------------------------ report

def load(work, step):
    path = pathlib.Path(work) / 'results' / f'{step}.json'
    return json.loads(path.read_text()) if path.exists() else None


def pct(n, d):
    return f'{100 * n / d:.2f}%' if d else 'n/a'


def agree(level):
    return level['direct'] + level['via_kd']


def step_report(work, args):
    import report
    results = {step: load(work, step) for step in STEPS if step not in ('report',)}
    data, text = report.render(results)
    REPORT.with_suffix('.json').write_text(json.dumps(data, indent=1) + '\n')
    REPORT.with_suffix('.md').write_text(text)
    print(f'wrote {REPORT}.md and .json')


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n', 1)[0])
    parser.add_argument('--work', required=True)
    parser.add_argument('--steps', default=','.join(STEPS))
    parser.add_argument('--workers', type=int, default=5)
    parser.add_argument('--recheck', choices=('stale', 'uncovered', 'errors', 'all'))
    if extra_hostile:
        extra_hostile.options(parser)
    args = parser.parse_args()
    work = pathlib.Path(args.work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    steps = args.steps.split(',')
    for step in steps:
        if step not in STEPS:
            sys.exit(f'unknown step {step}; steps: {", ".join(STEPS)}')
    for step in STEPS:
        if step in steps:
            print(f'== {step}', flush=True)
            globals()[f'step_{step}'](work, args)
    return 0


if __name__ == '__main__':
    sys.exit(main())
