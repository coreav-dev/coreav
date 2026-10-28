#!/usr/bin/env python3
"""Probe oracle on the real files of the corpus (docs/design/stage3-report.md).

For each `set: real` file of a family (`isobmff`: MP4, MOV, M4A, 3GP, M4V,
M4B, and `matroska`: MKV, WebM, both of corpus v1; `mpegts`: TS, M2TS, MTS,
M2T of the v1-ts-audio top-up; `mpegts-v2`: the same extensions in draw 5
of corpus v2-heldout, the replacement holdout), one worker downloads it
from R2 (SHA-256 checked, or reads it from `--local` for a draw that is
collected but not yet uploaded), captures `ffprobe` and the adapter at the streams and packets
levels (`scripts/oracle-capture.sh`), compares them
(`coreav-oracle compare`), applies the known divergences
(`scripts/oracle_kd.py` or `scripts/oracle_kd_mkv.py`), captures MediaInfo,
keeps the stream-level captures for the three-way report, and deletes the
download and the packet captures. One record per file goes to OUT/files/; a
stopped run resumes. `summarize` adds the records up per split.

Records and captures name files by SHA-256 only, and
stay outside git; the report commits counts.

  corpus.py run --family isobmff|matroska|mpegts|mpegts-v2 --out DIR --work DIR [--workers N]
                [--limit N] [--recheck stale|affected|uncovered|errors|all] [--codec C,...]
                [--kind K,...] [--only SHA,...] [--local DIR]
  corpus.py summarize --family isobmff|matroska|mpegts|mpegts-v2 --out DIR

Which records to make again (docs/design/stage3-report.md, "Rechecks"):

- `stale`: every file whose record was made by other tools than the
  current ones (its `tools`: the adapter, the comparator, the overlay, the
  capture scripts, this file, the modules the overlay imports, and the
  `ffprobe` and MediaInfo binaries, the documented divergence ids), and the
  failed and missing ones. After any such change that is every file; the
  totals of a fix come from this mode. `all` is for a new corpus, or a
  change the fingerprint does not see (the Python interpreter, the shell
  tools the capture scripts run).
- `affected`: the files a change to some streams can touch, for iterating
  on a fix: the failed and uncovered ones, and every file with a stream,
  on either side's kept capture, whose codec (`codec_name` or
  `codec_fourcc`) is in `--codec` or whose kind (`video`, `audio`,
  `subtitle`, `data`, ...) is in `--kind`. Its totals are provisional, and
  it cannot select a change to format-level fields (the format duration,
  whether a file probes). A selector that matches no stream is refused.
- `uncovered`: the failed and uncovered files only. It cannot see a file
  that agreed and that a change makes disagree; for a first look only.
- `errors`: the failed ones; `all`: every file.

`--only` names files by SHA-256 prefix, within the selection.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
import r2pool  # noqa: E402
sys.path.insert(0, str(ROOT / 'scripts'))
from compat_check import stream_key  # noqa: E402

MANIFEST = ROOT / 'corpus' / 'v1' / 'manifest.json'
TS_AUDIO_MANIFEST = ROOT / 'corpus' / 'v1-ts-audio' / 'manifest.json'
HELDOUT_MANIFEST = ROOT / 'corpus' / 'v2-heldout' / 'manifest.json'
# Each family: where its real files are listed, how that manifest names
# them, the adapter that reads them and the overlay that applies its
# documented divergences. Corpus v1 tags a file's container; the v1-ts-audio
# top-up names it in `family`.
FAMILIES = {
    'isobmff': {'manifest': MANIFEST, 'tags': ('c:isobmff',), 'adapter': 'isobmff-adapter',
                'kd': 'oracle_kd.py'},
    'matroska': {'manifest': MANIFEST, 'tags': ('c:matroska', 'c:webm'), 'adapter': 'mkv-adapter',
                 'kd': 'oracle_kd_mkv.py'},
    # The top-up's manifest names each file by content alone; its objects
    # live under this prefix (docs/corpus.md, "Top-up v1-ts-audio").
    'mpegts': {'manifest': TS_AUDIO_MANIFEST, 'families': ('ts',), 'adapter': 'ts-adapter',
               'kd': 'oracle_kd_ts.py', 'key_prefix': 'corpus/v1-ts-audio/'},
    # The replacement holdout, draw 5 of corpus v2-heldout
    # (docs/corpus.md, "Draw 5"). That manifest holds five
    # draws of several families and already writes the prefix into each
    # key, so the draw and the extensions pick this one out.
    'mpegts-v2': {'manifest': HELDOUT_MANIFEST, 'draw': 5,
                  'exts': ('ts', 'm2ts', 'mts', 'm2t'), 'adapter': 'ts-adapter',
                  'kd': 'oracle_kd_ts.py'},
}
LEVELS = ('streams', 'packets')
TIMEOUT = 3600
# Per capture (scripts/oracle-capture.sh defaults to 120 s): large files
# under several workers can take longer, and a capture stopped by its
# timeout must not read as the tool rejecting the file.
CAPTURE_TIMEOUT = 1500
PART = 8 << 20
CONCURRENCY = 8


def credentials():
    env = os.environ
    creds = {'access': env.get('R2_ACCESS_KEY_ID'), 'secret': env.get('R2_SECRET_ACCESS_KEY'),
             'endpoint': env.get('R2_ENDPOINT')}
    if not all(creds.values()):
        raise SystemExit('corpus.py: R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and R2_ENDPOINT are required')
    return creds


def child_env():
    """The environment of the tools: no credentials."""
    env = dict(os.environ)
    for key in ('R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY', 'R2_ENDPOINT', 'AWS_ACCESS_KEY_ID',
                'AWS_SECRET_ACCESS_KEY'):
        env.pop(key, None)
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    env['COREAV_ORACLE_TIMEOUT'] = str(CAPTURE_TIMEOUT)
    return env


def entries(family):
    """The real files of `family`, in manifest order."""
    spec = FAMILIES[family]
    manifest = json.loads(pathlib.Path(spec['manifest']).read_text())
    real = [f for f in manifest['files'] if f['set'] == 'real']
    if 'draw' in spec:
        drawn = [f for f in real if f.get('draw') == spec['draw']]
        chosen = [f for f in drawn if f.get('ext') in spec['exts']]
        # A draw is collected under its own extensions, so one that is
        # not among them is a file the collection stored under a name it
        # did not mean. Dropping it quietly would leave it out of both
        # the oracle run and the speed run without saying so.
        if len(chosen) != len(drawn):
            other = sorted({f.get('ext') for f in drawn} - set(spec['exts']))
            raise SystemExit(f"corpus.py: draw {spec['draw']} holds real files with other "
                             f"extensions than {spec['exts']}: {other}")
    elif 'families' in spec:
        chosen = [f for f in real if f.get('family') in spec['families']]
    else:
        chosen = [f for f in real if any(t in f.get('tags', ()) for t in spec['tags'])]
    prefix = spec.get('key_prefix', '')
    return [dict(f, key=prefix + f['key']) if prefix else f for f in chosen]


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as handle:
        while chunk := handle.read(1 << 22):
            digest.update(chunk)
    return digest.hexdigest()


def download(entry, dest):
    """Parallel ranged GET of one object, verified by SHA-256."""
    key, size = entry['key'], entry['size']
    limits = r2pool.Limits(requests=20000, bytes_=2 * size + (64 << 20))
    pool = r2pool.Pool(credentials(), CONCURRENCY, limits, timeout=300)
    try:
        _, etag = pool.head(key)
        fd = os.open(dest, os.O_CREAT | os.O_TRUNC | os.O_WRONLY, 0o600)
        try:
            os.ftruncate(fd, size)
            parts = [(o, min(PART, size - o)) for o in range(0, size, PART)]
            with concurrent.futures.ThreadPoolExecutor(CONCURRENCY) as ex:
                received = sum(ex.map(lambda p: pool.get(key, size, etag, p[0], p[1],
                                                         sink=lambda pos, c: os.pwrite(fd, c, pos)), parts))
        finally:
            os.close(fd)
    finally:
        pool.close()
    return received == size and sha256_file(dest) == entry['sha256']


def from_directory(entry, source, dest):
    """One object from a directory of `<sha256>.<ext>` files instead of
    from the bucket, for a draw that has been collected but not yet
    uploaded. The check is the same one the download makes -- the bytes
    must have the SHA-256 the manifest names -- so a record made this
    way is a record of the same file."""
    path = pathlib.Path(source) / f"{entry['sha256']}.{entry['ext']}"
    if not path.is_file():
        return False
    shutil.copyfile(path, dest)
    return sha256_file(dest) == entry['sha256']


def run(command, **kw):
    return subprocess.run(command, capture_output=True, timeout=TIMEOUT, env=child_env(), check=False, **kw)


# The local files other than the adapter, the comparator and the overlay
# that shape a record: the capture scripts, this file, the MediaInfo
# capture, and the modules an overlay imports.
HARNESS = ('scripts/oracle-capture.sh', 'scripts/oracle-ffprobe.sh', 'tools/stage3-report/corpus.py',
           'tools/mediainfo-oracle/mediainfo_oracle.py', 'docs/known-divergences.json',
           'docs/known-divergences-mkv.json', 'docs/known-divergences-ts.json',
           'scripts/compat_check.py')
OVERLAY_IMPORTS = {'isobmff': (), 'matroska': ('scripts/policy_ref.py',), 'mpegts': (),
                   'mpegts-v2': ()}


def binary(command, version_args, marker):
    """A tool on PATH as `version sha256:PREFIX`: the line of its version
    output that holds `marker` (else the first), and its binary's SHA-256;
    None when it is not on PATH."""
    path = shutil.which(command)
    if path is None:
        return None
    lines = subprocess.run([path, *version_args], capture_output=True, text=True, timeout=60,
                           check=False).stdout.splitlines()
    version = next((line.strip() for line in lines if marker in line), lines[0].strip() if lines else '')
    return f'{version} sha256:{sha256_file(path)[:16]}'


def tools(family):
    """What makes a record, stored in it so that a summary can tell records
    made by different code apart and `--recheck stale` can make them again:
    the SHA-256 of the adapter, the comparator and the overlay; one SHA-256
    over the other local files that shape a record (`HARNESS` and the
    overlay's imports); and the version and binary of `ffprobe` and
    MediaInfo."""
    release = pathlib.Path(os.environ.get('COREAV_ADAPTER_DIR', str(ROOT / 'target' / 'release')))
    oracle = pathlib.Path(os.environ.get('COREAV_ORACLE', str(ROOT / 'target' / 'release' / 'coreav-oracle')))
    harness = hashlib.sha256()
    for rel in HARNESS + OVERLAY_IMPORTS[family]:
        harness.update(f'{rel} {sha256_file(ROOT / rel)}\n'.encode())
    return {'adapter': sha256_file(release / FAMILIES[family]['adapter'])[:16],
            # The command whose `--ffprobe-compat` output the compat check reads.
            'coreav': sha256_file(release / 'coreav')[:16],
            'oracle': sha256_file(oracle)[:16],
            'overlay': sha256_file(ROOT / 'scripts' / FAMILIES[family]['kd'])[:16],
            'harness': harness.hexdigest()[:16],
            # As oracle-ffprobe.sh resolves it (`${FFPROBE:-ffprobe}`).
            'ffprobe': binary(os.environ.get('FFPROBE') or 'ffprobe', ['-version'], 'version'),
            'mediainfo': binary('mediainfo', ['--Version'], 'MediaInfoLib')}


def check_file(entry, family, out, work, made_by, local=None):
    """Captures, compares and overlays one file; returns its record."""
    sha = entry['sha256']
    name = f"{sha}.{entry['ext']}"
    record = {'sha256': sha, 'ext': entry['ext'], 'holdout': entry['holdout'], 'size': entry['size'],
              'tools': made_by}
    root = work / sha
    shutil.rmtree(root, ignore_errors=True)
    media = root / 'media'
    media.mkdir(parents=True)
    try:
        start = time.monotonic()
        try:
            ok = (from_directory(entry, local, media / name) if local
                  else download(entry, media / name))
        except Exception as error:  # network or R2 failure: recorded, not fatal
            ok, record['download_error'] = False, type(error).__name__
        record['download_s'] = round(time.monotonic() - start, 2)
        if not ok:
            record['error'] = ('the local copy is missing or has other bytes' if local
                               else 'download or SHA-256 check failed')
            return record
        adapter = os.environ.get('COREAV_ADAPTER_DIR', str(ROOT / 'target' / 'release'))
        captures = {'reference': [], 'candidate': ['--adapter', f"{adapter}/{FAMILIES[family]['adapter']}"]}
        for side, extra in captures.items():
            done = run([str(ROOT / 'scripts' / 'oracle-capture.sh'), '--media', str(media), '--out',
                        str(root / side), '--levels', ','.join(LEVELS), *extra])
            if done.returncode != 0:
                record['error'] = f'{side} capture failed ({done.returncode})'
                return record
            # The capture script writes a bare failure when the tool was
            # stopped (124: its timeout; above 128: a signal); that is the
            # harness, not the tool rejecting the file.
            stopped = re.findall(r'(\w+) failed for .* \(adapter status (\d+)\)', done.stderr.decode(errors='replace'))
            for level, status in stopped:
                if int(status) == 124 or int(status) > 128:
                    record['error'] = (f'{side} {level} capture timed out' if int(status) == 124
                                       else f'{side} {level} capture killed by signal {int(status) - 128}')
                    return record
        oracle = os.environ.get('COREAV_ORACLE', str(ROOT / 'target' / 'release' / 'coreav-oracle'))
        compare = run([oracle, 'compare', '--reference', str(root / 'reference'), '--candidate',
                       str(root / 'candidate'), '--levels', ','.join(LEVELS), '--profile', 'corpus',
                       '--max-diffs', '1000000', '--report', str(root / 'report.json')])
        if compare.returncode not in (0, 1):
            # Killed by a signal (SIGKILL is the kernel's out-of-memory kill:
            # the comparator holds every packet of the file) or failed.
            signal_number = -compare.returncode if compare.returncode < 0 else compare.returncode - 128
            if compare.returncode < 0 or compare.returncode > 128:
                record['error'] = ('comparison killed for memory (SIGKILL)' if signal_number == 9
                                   else f'comparison killed by signal {signal_number}')
            else:
                record['error'] = f'compare failed ({compare.returncode})'
            return record
        kd = [sys.executable, '-I', str(ROOT / 'scripts' / FAMILIES[family]['kd']), '--report',
              str(root / 'report.json'), '--reference', str(root / 'reference'), '--candidate',
              str(root / 'candidate'), '--out', str(root / 'kd.json')]
        # The overlays whose predicates read the file itself.
        if FAMILIES[family]['kd'] in ('oracle_kd.py', 'oracle_kd_ts.py'):
            kd += ['--media', str(media)]
        overlay = run(kd)
        if not (root / 'kd.json').exists():
            record['error'] = f'known-divergence overlay failed ({overlay.returncode})'
            return record
        levels = json.loads((root / 'kd.json').read_text())['levels']
        record['levels'] = {level: dict(groups['all'])
                            for level, groups in levels.items()}
        # Every key `--ffprobe-compat` prints against ffprobe's, with this file's own overlay for the known divergences.
        # Its own extension in scope: every rejection of a file ffprobe reads
        # needs a known divergence. The captures' time bound, not the
        # script's default.
        compat = run([sys.executable, '-I', str(ROOT / 'scripts' / 'compat_check.py'), '--coreav',
                      f'{adapter}/coreav', '--ffprobe', os.environ.get('FFPROBE') or 'ffprobe',
                      '--media', str(media), '--kd-overlay', str(root / 'kd.json'), '--min-file-rate', '0',
                      '--timeout', str(CAPTURE_TIMEOUT), '--in-scope', f".{entry['ext']}",
                      '--report', str(root / 'compat.json')])
        record['compat'] = compat_record(root / 'compat.json', compat.returncode)
        for side in ('reference', 'candidate'):
            record[f'{side}_probe_ok'] = json.loads(
                (root / side / f'{name}.streams.json').read_text()).get('probe_ok')
        # Stream-level captures and MediaInfo for the three-way report.
        mediainfo = run([sys.executable, '-I', str(ROOT / 'tools' / 'mediainfo-oracle' / 'mediainfo_oracle.py'),
                         'capture', '--media', str(media), '--out', str(out / 'mediainfo')])
        record['mediainfo_ok'] = mediainfo.returncode == 0
        for side, keep in (('reference', 'ffprobe'), ('candidate', 'coreav')):
            (out / keep).mkdir(parents=True, exist_ok=True)
            shutil.copyfile(root / side / f'{name}.streams.json', out / keep / f'{name}.streams.json')
        return record
    finally:
        shutil.rmtree(root, ignore_errors=True)


def compat_record(report, status):
    """One file's compat check: per key, `direct`, `kd` (agrees through a
    known divergence checked on the file) or `differs`; whether the file was
    compared, and whether CoreAV rejected a file ffprobe reads without a
    known divergence to excuse it."""
    if not report.exists():
        return {'error': f'compat check failed ({status})'}
    r = json.loads(report.read_text())
    via_kd = {stream_key(d['key']) for d in r['via_known_divergence']}
    keys = {key: ('differs' if counts['agreeing_files'] < counts['files']
                  else 'kd' if key in via_kd else 'direct')
            for key, counts in r['keys'].items()}
    return {'compared': r['compared_files'] > 0, 'keys': keys,
            'rejected_unexplained': bool(r['rejected_without_known_divergence'])}


def kept_streams(out, entry):
    """The streams of both sides' kept stream captures of `entry`."""
    name = f"{entry['sha256']}.{entry['ext']}.streams.json"
    streams = []
    for side in ('ffprobe', 'coreav'):
        path = pathlib.Path(out) / side / name
        if path.exists():
            try:
                streams += json.loads(path.read_text()).get('streams') or []
            except ValueError:
                pass
    return streams


def select(entries_, out, mode, current, codecs=(), kinds=()):
    """The entries whose record to make again under recheck `mode` (see the
    module documentation); `current` is `tools(family)`, the code that would
    make it now."""
    codecs, kinds = set(codecs), set(kinds)

    def wanted(entry):
        path = pathlib.Path(out) / 'files' / f"{entry['sha256']}.json"
        if not path.exists() or mode == 'all':
            return True
        record = json.loads(path.read_text())
        if 'error' in record:
            return mode in ('errors', 'uncovered', 'affected', 'stale')
        if mode == 'stale':
            return record.get('tools') != current
        uncovered = any(level.get('uncovered') for level in record.get('levels', {}).values())
        if mode == 'uncovered':
            return uncovered
        if mode == 'affected':
            return uncovered or any(
                s.get('codec_name') in codecs or s.get('codec_fourcc') in codecs or s.get('kind') in kinds
                for s in kept_streams(out, entry))
        return False

    return [e for e in entries_ if wanted(e)]


def cmd_run(args):
    out, work = pathlib.Path(args.out), pathlib.Path(args.work)
    (out / 'files').mkdir(parents=True, exist_ok=True)
    codecs = [c for c in (args.codec or '').split(',') if c]
    kinds = [k for k in (args.kind or '').split(',') if k]
    if args.recheck == 'affected' and not (codecs or kinds):
        print('--recheck affected needs --codec or --kind', file=sys.stderr)
        return 2
    if args.recheck != 'affected' and (codecs or kinds):
        print('--codec and --kind go with --recheck affected only', file=sys.stderr)
        return 2
    made_by = tools(args.family)
    missing_tools = [name for name in ('ffprobe', 'mediainfo') if made_by[name] is None]
    if missing_tools:
        # Without them every capture would fail or be empty, quietly.
        print(f"not on PATH: {', '.join(missing_tools)}", file=sys.stderr)
        return 2
    todo = select(entries(args.family), out, args.recheck, made_by, codecs, kinds)
    if args.recheck == 'affected':
        # A selector that names no stream of any file (a misspelt codec,
        # say) would quietly make this an `uncovered` recheck.
        extra = len(todo) - len(select(entries(args.family), out, 'uncovered', made_by))
        print(f'{extra} files beyond the uncovered and failed ones have a stream of {codecs + kinds}', flush=True)
        if extra == 0:
            print('the selector adds no file to the uncovered and failed ones: check its spelling '
                  '(codec_name or codec_fourcc, and kind, as the kept stream captures write them), '
                  'or use --recheck uncovered, which selects the same files', file=sys.stderr)
            return 2
    if args.only:
        prefixes = args.only.split(',')
        todo = [e for e in todo if any(e['sha256'].startswith(p) for p in prefixes)]
    if args.limit:
        todo = todo[:args.limit]
    print(f'{len(todo)} files to check', flush=True)
    # Which code produced the records: one line per run.
    git = lambda *a: subprocess.run(['git', '-C', str(ROOT), *a], capture_output=True, text=True).stdout.strip()  # noqa: E731
    release = pathlib.Path(os.environ.get('COREAV_ADAPTER_DIR', str(ROOT / 'target' / 'release')))
    run_log = {'time_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()), 'commit': git('rev-parse', 'HEAD'),
               'dirty': bool(git('status', '--porcelain', '--untracked-files=no')), 'files': len(todo),
               'recheck': args.recheck, 'codecs': codecs, 'kinds': kinds,
               'adapter_sha256': sha256_file(release / FAMILIES[args.family]['adapter']),
               'oracle_kd_sha256': sha256_file(ROOT / 'scripts' / FAMILIES[args.family]['kd'])}
    if todo:
        with open(out / 'runs.jsonl', 'a') as log:
            log.write(json.dumps(run_log) + '\n')

    def one(entry):
        try:
            record = check_file(entry, args.family, out, work, made_by, args.local)
        except subprocess.TimeoutExpired:
            record = {'sha256': entry['sha256'], 'ext': entry['ext'], 'holdout': entry['holdout'],
                      'error': 'timeout'}
        path = out / 'files' / f"{entry['sha256']}.json"
        path.with_suffix('.tmp').write_text(json.dumps(record) + '\n')
        path.with_suffix('.tmp').replace(path)
        return record

    with concurrent.futures.ThreadPoolExecutor(args.workers) as pool:
        for n, record in enumerate(pool.map(one, todo), 1):
            if 'error' in record or n % 50 == 0:
                print(f"{n}/{len(todo)} {record['sha256'][:12]} {record.get('error', 'ok')}", flush=True)
    return 0


def add_counts(t, counts):
    for key in ('items', 'direct', 'via_kd', 'uncovered'):
        t[key] += counts[key]
    for kd, n in counts['by_kd'].items():
        t['by_kd'][kd] = t['by_kd'].get(kd, 0) + n


def add(total, levels):
    """Adds one record's levels. Packets are in the reference's packet unit;
    `per_sample` adds up the comparison's unit (docs/design/stage3-report.md,
    "Packet unit"), the same counts for a file whose overlay lists none
    (Matroska)."""
    for level, counts in levels.items():
        t = total.setdefault(level, {'items': 0, 'direct': 0, 'via_kd': 0, 'uncovered': 0, 'by_kd': {}})
        if not isinstance(counts, dict) or 'items' not in counts:
            continue
        add_counts(t, counts)
        if level == 'packets':
            add_counts(t.setdefault('per_sample', {'items': 0, 'direct': 0, 'via_kd': 0, 'uncovered': 0,
                                                   'by_kd': {}}), counts.get('per_sample', counts))


def summarize(out, family):
    """Counts per split: files checked, errors, ffprobe failures, and the
    streams and packets levels (direct, via known divergence, uncovered)."""
    records = {p.stem: json.loads(p.read_text()) for p in (pathlib.Path(out) / 'files').glob('*.json')}
    expected = entries(family)
    groups = {'development': [e for e in expected if not e['holdout']],
              'holdout': [e for e in expected if e['holdout']], 'all': expected}
    summary = {}
    for group, members in groups.items():
        levels, errors, missing, ffprobe_failed = {}, {}, 0, 0
        uncovered_files = []
        # Uncovered disagreements by field: files per stream field, packets
        # per packet field (the first field of each file's examples).
        stream_fields, packet_fields = {}, {}
        # The entries that cover each file covered at the streams level.
        covered_kd_sets = []
        # --ffprobe-compat per key: files compared, and how each agreed.
        compat = {'keys': {}, 'compared': 0, 'rejected_unexplained': 0, 'errors': 0, 'records': 0, 'not_run': 0}
        for e in members:
            r = records.get(e['sha256'])
            if r is None:
                missing += 1
                continue
            if 'error' in r:
                errors[r['error']] = errors.get(r['error'], 0) + 1
                continue
            if r.get('reference_probe_ok') is False:
                ffprobe_failed += 1
            add(levels, r['levels'])
            c = r.get('compat')
            if c is None:
                # A record made before the compat check: it counts as
                # unchecked for the compat bar.
                compat['not_run'] += 1
            else:
                compat['records'] += 1
                if 'error' in c:
                    compat['errors'] += 1
                else:
                    compat['compared'] += int(c['compared'])
                    compat['rejected_unexplained'] += int(c['rejected_unexplained'])
                    for key, state in c['keys'].items():
                        t = compat['keys'].setdefault(key, {'files': 0, 'direct': 0, 'kd': 0, 'differs': 0})
                        t['files'] += 1
                        t[state] += 1
            if any(r['levels'][level]['uncovered'] for level in r['levels']):
                uncovered_files.append(e['sha256'])
            for item in r['levels'].get('streams', {}).get('covered_items', []):
                covered_kd_sets.append(sorted(item.get('kd', [])))
            for item in r['levels'].get('streams', {}).get('uncovered_items', []):
                for field in {re.sub(r'/\d+/', '/N/', f) for f in item.get('fields', [])}:
                    stream_fields[field] = stream_fields.get(field, 0) + 1
            for item in r['levels'].get('packets', {}).get('uncovered_items', []):
                field = re.sub(r'/\d+', '/N', (item.get('examples') or ['?'])[0])
                packet_fields[field] = packet_fields.get(field, 0) + item.get('units', 0)
        made_by = sorted({json.dumps(r.get('tools'), sort_keys=True) for r in records.values()
                          if r['sha256'] in {e['sha256'] for e in members}})
        summary[group] = {'files': len(members), 'checked': len(members) - missing - sum(errors.values()),
                          'tools': [json.loads(t) for t in made_by], 'mixed_tools': len(made_by) > 1,
                          'missing': missing, 'errors': errors, 'ffprobe_failed': ffprobe_failed,
                          'levels': levels, 'files_with_uncovered': sorted(uncovered_files),
                          'covered_kd_sets': covered_kd_sets,
                          'compat': {**compat, 'keys': dict(sorted(compat['keys'].items()))},
                          'uncovered_stream_fields': dict(sorted(stream_fields.items(), key=lambda x: -x[1])),
                          'uncovered_packet_fields': dict(sorted(packet_fields.items(), key=lambda x: -x[1]))}
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n', 1)[0])
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('run')
    p.add_argument('--family', choices=sorted(FAMILIES), required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--work', required=True)
    p.add_argument('--workers', type=int, default=4)
    p.add_argument('--limit', type=int, default=0)
    p.add_argument('--recheck', choices=('stale', 'affected', 'uncovered', 'errors', 'all'))
    p.add_argument('--codec', help='with --recheck affected: codec names or fourccs, comma separated')
    p.add_argument('--kind', help='with --recheck affected: stream kinds, comma separated')
    p.add_argument('--only')
    p.add_argument('--local', help='a directory of <sha256>.<ext> files to read instead of the '
                                   'bucket, for a draw not yet uploaded')
    p = sub.add_parser('summarize')
    p.add_argument('--family', choices=sorted(FAMILIES), required=True)
    p.add_argument('--out', required=True)
    args = parser.parse_args()
    if args.command == 'run':
        return cmd_run(args)
    print(json.dumps(summarize(args.out, args.family), indent=1))
    return 0


if __name__ == '__main__':
    sys.exit(main())
