"""Times `coreav` through the npm launcher against the native command and the
FFmpeg command line (docs/design/npm-cli.md, "Bars").

    COREAV_BIN=target/edge/coreav python3 -I run.py INPUTS OUT [--path native|wasi] [--only N] [--cache DIR]

The timed command is the installed `coreav`: the main package and the
Linux x86-64 native package, packed and installed with npm into a new
project, so that `coreav` is the native binary (`--path native`, the
default), or the WASI build run by Node (`--path wasi`, the first run's
command). The fixed cost of each way to start the command is timed apart:
`--version` through the installed command, the Node launcher, the WASI
build and `npx`.

Needs `packages/cli/scripts/build.sh` and `scripts/platform.sh linux-x64` run first, `ffmpeg` and `ffprobe` on
PATH, and R2 credentials (bench/shared/benchlib.py) for the inputs, which are
downloaded once into the cache directory and verified by SHA-256. Writes
OUT/meta.json and one line per file to OUT/results.jsonl; a rerun skips the
files already there. Each command's times are wall times in seconds."""
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
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE.parent / 'shared'))
from benchlib import R2, append_jsonl, download, machine, sha256_file, write_json  # noqa: E402

PACKAGE = ROOT / 'packages' / 'cli'
MODULE = PACKAGE / 'wasm' / 'coreav.wasm'
NATIVE_PACKAGE = PACKAGE / 'npm' / 'linux-x64'
POLICY = ROOT / 'docs' / 'schema' / 'policy-examples' / 'web-upload.json'
ROUNDS = 5
TIMEOUT = 600
CLIP_SECONDS = 10
FF = ['-nostdin', '-v', 'error', '-y']


def run(cmd, cwd=None):
    started = time.perf_counter()
    try:
        done = subprocess.run(cmd, capture_output=True, timeout=TIMEOUT, cwd=cwd)
    except subprocess.TimeoutExpired:
        return {'seconds': time.perf_counter() - started, 'code': None, 'stdout': b'', 'stderr': b''}
    return {'seconds': time.perf_counter() - started, 'code': done.returncode,
            'stdout': done.stdout, 'stderr': done.stderr}


def operations(src, duration):
    """[(name, coreav arguments, output suffix or None, FFmpeg command builder)]."""
    probe = ['ffprobe', '-v', 'error', '-of', 'json', '-show_format', '-show_streams', str(src)]
    ops = [
        ('probe', ['--strict-safe', 'ffprobe', '-v', 'error', '-of', 'json', '-show_format', '-show_streams', str(src)], None, lambda out, summary: probe),
        ('check', ['check', '--policy', str(POLICY), str(src)], None, lambda out, summary: probe),
        ('chapters', ['chapters', str(src)], None,
         lambda out, summary: ['ffprobe', '-v', 'error', '-of', 'json', '-show_chapters', str(src)]),
        ('extract-audio', ['extract-audio', str(src), 'OUT'], '.m4a',
         lambda out, summary: ['ffmpeg', *FF, '-i', str(src), '-map', '0:a:0', '-c', 'copy', '-f', 'mp4', out]),
    ]
    # The clips: [0, 10 s) and [floor(d/2), floor(d/2) + 10 s).
    starts = [('clip-head', 0)]
    if duration is not None:
        starts.append(('clip-middle', int(duration // 2)))
    for name, start in starts:
        ops.append((name, ['clip', '--start', str(start), '--end', str(start + CLIP_SECONDS), str(src), 'OUT'],
                    '.mp4', lambda out, summary, start=start: [
                        'ffmpeg', *FF, '-ss', str(start), '-i', str(src), '-t', str(CLIP_SECONDS),
                        '-map', '0:v:0', '-map', '0:a:0?', '-c', 'copy', '-f', 'mp4', out]))

    def remux(extra):
        def build(out, summary):
            maps = [arg for s in summary['streams'] for arg in ('-map', f'0:{s["stream_index"]}')]
            return ['ffmpeg', *FF, '-i', str(src), *maps, '-c', 'copy', *extra, '-movflags', '+faststart', out]
        return build

    ops.append(('remux', ['remux', str(src), 'OUT'], '.mp4', remux([])))
    ops.append(('sanitize', ['sanitize', str(src), 'OUT'], '.mp4', remux(['-map_metadata', '-1', '-map_chapters', '-1'])))
    # As docs/design/split.md, with the streams `coreav split` keeps by default.
    ops.append(('split', ['split', '--every', str(CLIP_SECONDS), str(src), 'OUT'], '/%03d.mp4',
                lambda out, summary: ['ffmpeg', *FF, '-i', str(src), '-map', '0:v:0', '-map', '0:a:0?', '-c', 'copy',
                                      '-f', 'segment', '-segment_time', str(CLIP_SECONDS), '-reset_timestamps', '1',
                                      out]))
    ops.append(('extract-subtitles', ['extract-subtitles', str(src), 'OUT'], '.vtt',
                lambda out, summary: ['ffmpeg', *FF, '-i', str(src), '-map', '0:s:0', '-f', 'webvtt', out]))
    return ops


def read_output(path):
    """The output's bytes; for a pattern (`split`), every file in its
    directory, by name."""
    if '%' in path:
        files = sorted(Path(path).parent.iterdir())
        return [(f.name, f.read_bytes()) for f in files] or None
    try:
        return Path(path).read_bytes()
    except FileNotFoundError:
        return None


def remove(path):
    """Removes an output; a pattern's directory is emptied."""
    if '%' in path:
        shutil.rmtree(Path(path).parent, ignore_errors=True)
        Path(path).parent.mkdir()
        return
    Path(path).unlink(missing_ok=True)


def measure_op(native, launcher, op, scratch):
    name, args, suffix, ffmpeg_cmd = op
    out = str(scratch / f'out{suffix or ""}')
    ff_out = str(scratch / f'ffmpeg{suffix or ""}')
    for path in (out, ff_out):
        if '%' in path:
            remove(path)
    argv = [out if a == 'OUT' else a for a in args]
    commands = {'launcher': [*launcher, *argv], 'native': [native, *argv]}
    record = {'op': name}

    # Once each, for the correctness bar and the case's status.
    remove(out)
    first = {'native': run(commands['native'])}
    native_bytes = read_output(out)
    remove(out)
    first['launcher'] = run(commands['launcher'])
    launcher_bytes = read_output(out)
    remove(out)
    a, b = first['native'], first['launcher']
    record['native_code'] = a['code']
    record['equal'] = (a['code'] == b['code'] and a['stdout'] == b['stdout'] and a['stderr'] == b['stderr']
                       and native_bytes == launcher_bytes)
    if not record['equal']:
        record['difference'] = {'codes': [a['code'], b['code']],
                                'stdout': a['stdout'] == b['stdout'], 'stderr': a['stderr'] == b['stderr'],
                                'output': native_bytes == launcher_bytes,
                                'launcher_stderr': b['stderr'][:400].decode('utf-8', 'replace')}
    ok = a['code'] == 0 or (name == 'check' and a['code'] == 1 and a['stdout'])
    if not ok:
        record['status'] = 'coreav_refused'
        record['error'] = a['stderr'][:300].decode('utf-8', 'replace')
        return record
    summary = json.loads(a['stdout']) if a['stdout'] else None
    commands['ffmpeg'] = ffmpeg_cmd(ff_out, summary)
    remove(ff_out)
    ff = run(commands['ffmpeg'])
    remove(ff_out)
    if ff['code'] != 0:
        record['status'] = 'ffmpeg_failed'
        record['error'] = ff['stderr'][-300:].decode('utf-8', 'replace')
        return record

    order = ['launcher', 'native', 'ffmpeg']
    times = {tool: [] for tool in order}
    for round_ in range(ROUNDS):
        for tool in order[round_ % 3:] + order[:round_ % 3]:
            target = ff_out if tool == 'ffmpeg' else out
            remove(target)
            result = run(commands[tool])
            remove(target)
            times[tool].append(result['seconds'] if result['code'] == (0 if tool == 'ffmpeg' else a['code']) else None)
    record['status'] = 'timed'
    record['times'] = times
    return record


def install(scratch):
    """The main package and the Linux x86-64 package, packed and installed
    with npm (scripts on) into a new project; returns the project."""
    project = scratch / 'project'
    project.mkdir()
    tarballs = []
    for directory in (NATIVE_PACKAGE, PACKAGE):
        packed = subprocess.run(['npm', 'pack', '--silent', '--pack-destination', str(scratch)],
                                cwd=directory, capture_output=True, text=True, check=True)
        tarballs.append(str(scratch / packed.stdout.strip().splitlines()[-1]))
    (project / 'package.json').write_text('{"name": "npx-bench", "private": true}\n')
    subprocess.run(['npm', 'install', '--no-audit', '--no-fund', '--silent', *tarballs],
                   cwd=project, check=True, capture_output=True)
    command = project / 'node_modules' / 'coreav' / 'bin' / 'coreav'
    if command.read_bytes() != (NATIVE_PACKAGE / 'bin' / 'coreav').read_bytes():
        raise SystemExit('the installed coreav command is not the native binary')
    return project


def launcher_costs(project):
    """`--version` 20 times each: the installed command (the native binary),
    the Node launcher, the WASI build and `npx`."""
    package = project / 'node_modules' / 'coreav'
    ways = {
        'installed': [str(project / 'node_modules' / '.bin' / 'coreav')],
        'node_launcher': ['node', str(package / 'lib' / 'main.js')],
        'wasi': ['node', str(package / 'lib' / 'wasi.mjs')],
        'npx': ['npx', '--no-install', 'coreav'],
    }
    result = {key: [] for key in ways}
    for _ in range(20):
        for key, cmd in ways.items():
            done = run([*cmd, '--version'], cwd=project)
            if done['code'] != 0:
                raise SystemExit(f'{key} --version failed: {done["stderr"][:300]!r}')
            result[key].append(done['seconds'])
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('inputs')
    parser.add_argument('out')
    parser.add_argument('--only', type=int, help='the first N files (development checks)')
    parser.add_argument('--cache', default='/tmp/npx-cache')
    parser.add_argument('--path', choices=('native', 'wasi'), default='native')
    args = parser.parse_args()
    native = os.environ.get('COREAV_BIN')
    if not native or not MODULE.exists() or not (NATIVE_PACKAGE / 'bin' / 'coreav').exists():
        raise SystemExit('needs COREAV_BIN, packages/cli/scripts/build.sh and scripts/platform.sh linux-x64')
    inputs = json.loads(Path(args.inputs).read_text())
    files = inputs['files'][:args.only] if args.only else inputs['files']
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    cache = Path(args.cache)
    cache.mkdir(parents=True, exist_ok=True)
    results = out / 'results.jsonl'
    done = set()
    if results.exists():
        done = {json.loads(line)['sha256'] for line in results.read_text().splitlines() if line.strip()}

    def version(cmd):
        try:
            return subprocess.run(cmd, capture_output=True, text=True, timeout=60).stdout.splitlines()[0]
        except Exception:
            return None

    with tempfile.TemporaryDirectory(prefix='npx-bench-') as tmp:
        scratch = Path(tmp)
        project = install(scratch)
        package = project / 'node_modules' / 'coreav'
        launcher = ([str(project / 'node_modules' / '.bin' / 'coreav')] if args.path == 'native'
                    else ['node', str(package / 'lib' / 'wasi.mjs')])
        build = (sha256_file(MODULE), sha256_file(native), sha256_file(NATIVE_PACKAGE / 'bin' / 'coreav'))
        meta_path = out / 'meta.json'
        if meta_path.exists():
            # A rerun continues the same build and path only.
            meta = json.loads(meta_path.read_text())
            recorded = (meta['module_sha256'], meta['native_sha256'], meta.get('package_binary_sha256'))
            if recorded != build or meta.get('path', 'wasi') != args.path:
                raise SystemExit('the build or the path differs from the run in OUT')
        else:
            write_json(meta_path, {
                'inputs': {k: inputs[k] for k in ('source', 'manifest_sha256', 'count', 'bytes')},
                'path': args.path,
                'module_sha256': build[0], 'module_bytes': MODULE.stat().st_size,
                'native_sha256': build[1], 'package_binary_sha256': build[2],
                'versions': {'node': version(['node', '--version']), 'npm': version(['npm', '--version']),
                             'ffmpeg': version(['ffmpeg', '-version']), 'coreav': version([native, '--version'])},
                'machine': machine(), 'rounds': ROUNDS,
                'npm_overhead': launcher_costs(project),
            })
        r2 = None
        for index, entry in enumerate(files, 1):
            sha = entry['sha256']
            if sha in done:
                continue
            src = cache / f'{sha}.{entry["ext"]}'
            if not src.exists() or src.stat().st_size != entry['size']:
                r2 = r2 or R2()
                download(r2, entry, src)
            src.read_bytes()  # into the page cache
            probe = run([native, '--strict-safe', 'ffprobe', '-v', 'error', '-of', 'json', '-show_format', '-show_streams', str(src)])
            duration = None
            if probe['code'] == 0:
                try:
                    duration = float(json.loads(probe['stdout'])['format']['duration'])
                except (KeyError, TypeError, ValueError):
                    duration = None
            work = scratch / 'work'
            shutil.rmtree(work, ignore_errors=True)
            work.mkdir()
            ops = [measure_op(native, launcher, op, work) for op in operations(src, duration)]
            if duration is None:
                ops.append({'op': 'clip-middle', 'status': 'skipped', 'equal': True,
                            'reason': 'the native probe gave no duration'})
            append_jsonl(results, {'sha256': sha, 'size': entry['size'], 'ext': entry['ext'],
                                   'duration': duration, 'ops': ops})
            timed = sum(1 for o in ops if o['status'] == 'timed')
            unequal = [o['op'] for o in ops if not o['equal']]
            print(f'{index}/{len(files)} {sha[:12]} {entry["size"] >> 20} MiB timed {timed}'
                  + (f' UNEQUAL {unequal}' if unequal else ''), flush=True)


if __name__ == '__main__':
    main()
