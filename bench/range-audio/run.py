"""The range-read audio extraction measurement (README.md): audio
extraction over HTTP range reads of a presigned R2 URL by `coreav extract-audio URL`, against FFmpeg reading the
same URL, each output checked against the native build's on a downloaded
copy.

    python3 -I run.py INPUTS OUT [--held-out] [--engines cli] [--trials 3]
                     [--limit N] [--only SHA,...] [--max-requests N]

INPUTS lists objects ({"files": [...]}, or with --held-out a held-out set's
"held_out", as sets.py writes it). OUT gets results.jsonl (one line per object, resumable) and
meta.json. Per object, the engines' blocks and FFmpeg's (`--trials` runs
each) run in an order rotated by the object's index; each block starts
with a discarded warm-up run. FFmpeg always runs, so that each run's
ratios have their own baseline. `--engines` names the engines: `cli`, and
any other engine an `extra_engines` module beside this file provides (its
`ENGINES`), which may add options of its own. meta.json records the commit
measured. R2 requests are counted from the readers' own statistics and
capped (`--max-requests`, the run stops before an object would pass it);
FFmpeg's requests are not counted by it and are few (one open request,
plus one per seek)."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'shared'))
from benchlib import (R2, ROOT, append_jsonl, download, machine, sha256_file, tool_versions,  # noqa: E402
                      write_json)

# Engines beside the CLI, when this tree has them: {name: engine}, each with
# `per_object` (the most R2 requests one object may cost it), `options`
# (adds its options to the parser), `versions()`, `run(record, url, args)`
# (fills the record with its rows) and `requests(record)`.
sys.path.insert(0, str(HERE))
try:
    from extra_engines import ENGINES as EXTRA  # noqa: E402
except ImportError:
    EXTRA = {}

NATIVE = ROOT / 'target' / 'edge' / 'edge-native'
CLI = ROOT / 'target' / 'dist' / 'coreav'
# R2 requests one object may cost the CLI at most: the warm-up and the
# trials, each under its reader's cap of 10,000.
CLI_PER_OBJECT = 10_000 * 4


def native_extract(path, scratch):
    out = scratch / 'native.m4a'
    out.unlink(missing_ok=True)
    done = subprocess.run([str(NATIVE), 'run', 'extractAudio', str(path), '{}', str(out)],
                          capture_output=True, text=True, timeout=3600)
    if done.returncode not in (0, 1):
        raise RuntimeError(f'edge-native failed: {done.stderr.strip()}')
    result = {'ok': done.returncode == 0, 'document': json.loads(done.stdout)}
    if result['ok']:
        result['sha256'] = sha256_file(out)
        result['bytes'] = out.stat().st_size
    return result


def ffmpeg_run(url, stream, scratch):
    out = scratch / 'ffmpeg.m4a'
    out.unlink(missing_ok=True)
    cmd = ['ffmpeg', '-nostdin', '-v', 'error', '-i', url, '-map', f'0:{stream}', '-c', 'copy', '-f', 'mp4',
           '-y', str(out)]
    started = time.perf_counter()
    try:
        done = subprocess.run(cmd, capture_output=True, timeout=3600)
        ok = done.returncode == 0
    except subprocess.TimeoutExpired:
        ok = False
    result = {'ok': ok, 'seconds': time.perf_counter() - started}
    if out.exists():
        result['bytes'] = out.stat().st_size
    return result


def cli_run(url, scratch):
    out = scratch / 'cli.m4a'
    out.unlink(missing_ok=True)
    env = {**os.environ, 'COREAV_REMOTE_STATS': '1'}
    started = time.perf_counter()
    try:
        done = subprocess.run([str(CLI), 'extract-audio', url, str(out)], capture_output=True, text=True,
                              env=env, timeout=3600)
        code = done.returncode
        stderr = done.stderr
    except subprocess.TimeoutExpired:
        code, stderr = None, ''
    result = {'ok': code == 0, 'seconds': time.perf_counter() - started, 'exit': code}
    for line in stderr.splitlines():
        if line.startswith('{"remote_stats"'):
            result['stats'] = json.loads(line)['remote_stats']
    if result['ok']:
        result['sha256'] = sha256_file(out)
        result['bytes'] = out.stat().st_size
    elif stderr:
        # The CLI's message, without the URL (a bearer token).
        result['error'] = stderr.replace(url, '<url>')[-500:]
    return result


def requests_of(record):
    rows = [*record.get('cli', []), *([record['cli_warmup']] if 'cli_warmup' in record else [])]
    n = sum((row.get('stats') or {}).get('requests', 0) for row in rows)
    return n + sum(engine.requests(record) for engine in EXTRA.values())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('inputs')
    parser.add_argument('out')
    parser.add_argument('--held-out', action='store_true', help="INPUTS is a held-out set (sets.py): use its held_out list")
    parser.add_argument('--engines', default='cli')
    parser.add_argument('--trials', type=int, default=3)
    parser.add_argument('--limit', type=int)
    parser.add_argument('--only', default='')
    for engine in EXTRA.values():
        engine.options(parser)
    parser.add_argument('--max-requests', type=int, default=400_000,
                        help='R2 requests the whole run may make (Class B, 0.36 USD per million)')
    args = parser.parse_args()
    data = json.loads(Path(args.inputs).read_text())
    files = (data['held_out'] if args.held_out else data['files'])[:args.limit]
    if args.only:
        files = [f for f in files if any(f['sha256'].startswith(p) for p in args.only.split(','))]
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    results = out / 'results.jsonl'
    done, spent = set(), 0
    if results.exists():
        for line in results.read_text().splitlines():
            record = json.loads(line)
            done.add(record['sha256'])
            spent += requests_of(record)
    engines = set(args.engines.split(','))
    known = {'cli', *EXTRA}
    if not engines or not engines <= known:
        raise SystemExit(f'--engines takes one or more of {", ".join(sorted(known))}')
    per_object = (CLI_PER_OBJECT if 'cli' in engines else 0) + sum(e.per_object for n, e in EXTRA.items()
                                                                   if n in engines)
    commit = subprocess.run(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], capture_output=True,
                            text=True).stdout.strip()
    dirty = subprocess.run(['git', '-C', str(ROOT), 'status', '--porcelain', '--', 'crates', 'packages', 'tools'],
                           capture_output=True, text=True).stdout.strip() != ''
    # A run is one commit and one set of engines: resuming into another is refused.
    if (out / 'meta.json').exists():
        before = json.loads((out / 'meta.json').read_text())
        if before.get('engines') != sorted(engines) or before.get('commit') != commit:
            raise SystemExit(f'{out} holds a run of other engines or another commit; use another OUT')
    write_json(out / 'meta.json', {'inputs': args.inputs, 'held_out': args.held_out, 'machine': machine(),
                                   'engines': sorted(engines), 'commit': commit, 'dirty': dirty,
                                   'tools': {**tool_versions(), **{k: v for n, e in EXTRA.items() if n in engines
                                                                   for k, v in e.versions().items()}},
                                   'native_sha256': sha256_file(NATIVE),
                                   'cli_sha256': sha256_file(CLI) if 'cli' in engines else None,
                                   'started': time.time(),
                                   'trials': args.trials, 'max_requests': args.max_requests})
    r2 = R2()
    scratch = Path(tempfile.mkdtemp(prefix='range-audio-'))
    try:
        for index, entry in enumerate(files):
            if entry['sha256'] in done:
                continue
            if spent + per_object > args.max_requests:
                print(f'stopping: {spent} requests made, the cap is {args.max_requests}', file=sys.stderr)
                break
            record = {'sha256': entry['sha256'], 'size': entry['size'], 'index': index,
                      'stratum': entry.get('stratum')}
            path = scratch / 'input'
            record['download_seconds'] = download(r2, entry, path)
            record['native'] = native_extract(path, scratch)
            path.unlink()
            stream = ((record['native']['document'] or {}).get('stream') or {}).get('stream_index')
            blocks = [*(n for n in EXTRA if n in engines), 'ffmpeg', *(['cli'] if 'cli' in engines else [])]
            shift = index % len(blocks)
            record['order'] = blocks[shift:] + blocks[:shift]
            for block in record['order']:
                # A fresh URL per block: a block may start more than an hour
                # after the object's first on the largest objects.
                url = r2.presign(entry['key'])
                if block in EXTRA:
                    EXTRA[block].run(record, url, args)
                elif block == 'ffmpeg':
                    if stream is not None:
                        record['ffmpeg_warmup'] = ffmpeg_run(url, stream, scratch)
                    record['ffmpeg'] = ([ffmpeg_run(url, stream, scratch) for _ in range(args.trials)]
                                        if stream is not None else [])
                else:
                    record['cli_warmup'] = cli_run(url, scratch)
                    record['cli'] = [cli_run(url, scratch) for _ in range(args.trials)]
            spent += requests_of(record)
            append_jsonl(results, record)
            med = lambda rows: sorted(r['seconds'] for r in rows)[len(rows) // 2] if rows else None  # noqa: E731
            print(f'{index + 1}/{len(files)} {entry["sha256"][:12]} {entry["size"] >> 20} MiB '
                  + ' '.join(f'{b} {med(record.get(b, []))}' for b in record['order'])
                  + f' requests so far {spent}', file=sys.stderr, flush=True)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


if __name__ == '__main__':
    main()
