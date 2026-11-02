"""Current fixture CI policy. Never use this adapter to judge a frozen round."""
import argparse
import collections
import importlib.util
import json
import os
import pathlib
import shutil
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1] / 'scripts')]
import decoded_output
import oracle_pin


def reporter():
    spec = importlib.util.spec_from_file_location('current_ci_reporter', HERE / 'run.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def outputs(directory):
    return {p.relative_to(directory).as_posix(): p for p in directory.rglob('*') if p.is_file()}


def verify(args, target, candidate, reference, env, ffmpeg, ffprobe, scratch):
    source = decoded_output.probe(target, ffprobe, env)
    if source is None:
        return 'unverified', 'source probe'
    reference_input = reference.parent / ('reference-input' + target.suffix)
    shutil.copyfile(target, reference_input)
    argv = [str(reference / 'output.mp4') if a == 'OUTPUT' else
            str(reference / 'file.mp4') if a == 'FILE' else
            str(reference_input) if a.startswith('INPUT') else a for a in args]
    code, stdout, _ = decoded_output.invoke([ffmpeg, '-nostdin', '-hide_banner', '-v', 'error', *argv],
                                           env, reference)
    if code != 0:
        return 'unverified', 'reference command'
    if not decoded_output.same_files(target, reference_input):
        return 'unverified', 'reference modified its input'
    if stdout and args[-1] in ('-', 'pipe:1'):
        (reference / 'stdout').write_bytes(stdout)
    elif stdout:
        return 'unverified', 'reference standard output'
    mine, theirs = outputs(candidate), outputs(reference)
    if set(mine) != set(theirs) or not theirs:
        return 'failed', 'output files'
    try:
        inputs = decoded_output.raw_inputs(args, source)
    except (TypeError, ValueError, ZeroDivisionError):
        return 'unverified', 'raw output description'
    for name in sorted(theirs):
        listing = decoded_output.probe(theirs[name], ffprobe, env, inputs)
        if listing is None:
            return 'unverified', 'reference probe'
        if any(s.get('codec_name') in ('png', 'bmp') for s in listing['streams']):
            videos = [s for s in source['streams'] if s.get('codec_type') == 'video']
            if not videos or any(s.get('pix_fmt') not in ('rgb24', 'bgr24', 'gbrp', 'rgba', 'bgra', 'gbrap')
                                 for s in videos):
                return 'unverified', 'no_oracle: PNG/BMP colour conversion'
        result = decoded_output.compare(mine[name], theirs[name], ffmpeg, ffprobe, env, scratch, inputs)
        if result[0] != 'verified':
            return result
    return 'verified', 'exact decoded output'


def refusals(legacy, opts, files, counts, failures, records, ffmpeg, ffprobe):
    target = pathlib.Path(files[0]).resolve() if files else None
    for row in legacy.load_commands():
        if row['tool'] != 'ffmpeg' or row['label'] != 'out-of-scope':
            continue
        with tempfile.TemporaryDirectory(prefix='coreav-current-ci-') as tmp:
            root = pathlib.Path(tmp)
            candidate, reference, scratch = (root / name for name in ('candidate', 'reference', 'scratch'))
            for directory in (candidate, reference, scratch):
                directory.mkdir()
            staged = root / ('input' + target.suffix) if target else None
            if staged:
                shutil.copyfile(target, staged)
            log = root / 'fallback.log'
            env = {**legacy.ENV, 'COREAV_FALLBACK_LOG': str(log)}
            argv = [str(staged) if a.startswith('INPUT') and staged else
                    str(candidate / 'output.mp4') if a == 'OUTPUT' else
                    str(candidate / 'file.mp4') if a == 'FILE' else a for a in row['args']]
            code, out, err = decoded_output.invoke([opts.coreav, 'ffmpeg', *argv], env, candidate)
            wrote = bool(outputs(candidate))
            changed_input = staged and (not staged.is_file() or not decoded_output.same_files(target, staged))
            try:
                events = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
                fallback = any(not isinstance(e, dict) or 'ran' not in e or e['ran'] is not None for e in events)
            except (ValueError, OSError):
                events, fallback = [], True
            if changed_input:
                status, reason = 'failed', 'modified input'
            elif row['args'] in legacy.HELP_QUERIES:
                status, reason = (('help', 'usage') if code == 0 and out == legacy.ffmpeg_usage()
                                  and not err and not wrote and not fallback and not events else ('failed', 'help'))
            elif code in (1, 2, 4) and not out and err.strip() and not wrote and not fallback:
                status, reason = 'refused', 'clean refusal'
            elif code == 0 and not fallback and not events and target and (not out or row['args'][-1] in ('-', 'pipe:1')):
                if row['args'][-1] in ('-', 'pipe:1'):
                    (candidate / 'stdout').write_bytes(out)
                status, reason = verify(row['args'], staged, candidate, reference, env, ffmpeg, ffprobe, scratch)
            else:
                status, reason = 'failed', 'crash, timeout, side effect or invalid outcome'
        record = {'args': row['args'], 'status': status, 'reason': reason, 'exit': code}
        records.append(record)
        counts[{'verified': 'pass', 'refused': 'pass', 'help': 'help'}.get(status, 'fail')] += 1
        if status not in ('verified', 'refused', 'help'):
            failures.append({'bar': 'B1', 'tool': 'ffmpeg', **record})
    return counts, failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['commands'])
    parser.add_argument('--coreav', required=True)
    parser.add_argument('--media', nargs='+', default=[])
    parser.add_argument('--baseline')
    parser.add_argument('--report', required=True)
    parser.add_argument('--top', type=int, default=40)
    parser.add_argument('--jobs', type=int, default=os.cpu_count() or 4)
    opts = parser.parse_args()
    opts.coreav = str(pathlib.Path(opts.coreav).resolve())
    opts.write_baseline = None
    ffmpeg, ffprobe = shutil.which('ffmpeg'), shutil.which('ffprobe')
    if (not ffmpeg or not ffprobe
            or pathlib.Path(ffprobe).resolve() != (pathlib.Path(ffmpeg).parent / 'ffprobe').resolve()
            or list(oracle_pin.verify_bin(pathlib.Path(ffmpeg).parent))):
        parser.error('install the pinned ffmpeg and ffprobe with scripts/install-oracle-ffmpeg.sh')
    legacy = reporter()
    records = []
    legacy.ffmpeg_refusals = lambda o, f, c, errors: refusals(legacy, o, f, c, errors, records, ffmpeg, ffprobe)
    status = legacy.cmd_commands(opts)
    report = json.loads(pathlib.Path(opts.report).read_text())
    report['current_ffmpeg'] = {'counts': dict(collections.Counter(r['status'] for r in records)),
                                'commands': records}
    pathlib.Path(opts.report).write_text(json.dumps(report, indent=1) + '\n')
    return 1 if any(r['status'] in ('failed', 'unverified') for r in records) else status


if __name__ == '__main__':
    sys.exit(main())
