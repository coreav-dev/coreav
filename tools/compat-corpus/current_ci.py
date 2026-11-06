"""Current fixture CI policy. Never use this adapter to judge a frozen round."""
import argparse
import collections
import importlib.util
import json
import os
import pathlib
import re
import shutil
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1] / 'scripts')]
import decoded_output
import oracle_pin
import test_oracle


def reporter():
    spec = importlib.util.spec_from_file_location('current_ci_reporter', HERE / 'run.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def outputs(directory):
    return {p.relative_to(directory).as_posix(): p for p in directory.rglob('*') if p.is_file()}


def copy_mapping(stderr, sources, reference):
    try:
        lines = stderr.decode('utf-8').splitlines()
    except UnicodeDecodeError:
        return ()
    if lines.count('Stream mapping:') != 1:
        return ()
    streams = reference['streams']
    if [s.get('index') for s in streams] != list(range(len(streams))):
        return ()
    mapped = {}
    for line in lines[lines.index('Stream mapping:') + 1:]:
        if line.startswith(('Output #', 'Press [q]', '[out#', 'frame=', 'size=', 'video:')):
            break
        if not line.strip():
            continue
        if re.fullmatch(r'\[mp4 @ 0x[0-9a-fA-F]+\] track \d+: codec frame size is not set', line):
            continue
        match = re.fullmatch(r'\s+Stream #(\d+):(\d+)(?:\[[^]\r\n]*\])? -> #(\d+):(\d+) \(copy\)', line)
        if not match:
            return ()
        file, source, output, index = map(int, match.groups())
        if output != 0 or index in mapped or file >= len(sources) or index >= len(streams):
            return ()
        originals = [s for s in sources[file]['streams'] if s.get('index') == source]
        if len(originals) != 1 or any(originals[0].get(k) != streams[index].get(k)
                                       for k in ('codec_name', 'codec_type')):
            return ()
        mapped[index] = (index, file, source)
    return tuple(mapped[i] for i in range(len(streams))) if set(mapped) == set(range(len(streams))) else ()


def verify(args, bindings, candidate, reference, env, ffmpeg, ffprobe, scratch):
    names = [args[i + 1] for i, a in enumerate(args[:-1]) if a == '-i']
    if not names or any(name not in bindings for name in names):
        return 'unverified', 'source input binding'
    sources, staged = {}, {}
    directory = reference.parent / 'reference-inputs'
    directory.mkdir(exist_ok=True)
    for name in dict.fromkeys(names):
        source = decoded_output.probe(bindings[name], ffprobe, env)
        if source is None:
            return 'unverified', 'source probe'
        sources[name] = source
        path = directory / (str(len(staged)) + bindings[name].suffix)
        shutil.copyfile(bindings[name], path)
        staged[name] = path
    argv = [str(reference / 'output.mp4') if a == 'OUTPUT' else
            str(reference / 'file.mp4') if a == 'FILE' else
            str(staged[a]) if a in staged else a for a in args]
    code, stdout, stderr = decoded_output.invoke([ffmpeg, '-nostdin', '-hide_banner', '-v', 'info', *argv],
                                                env, reference)
    if code != 0:
        return 'unverified', 'reference command'
    if any(not path.is_file() or not decoded_output.same_files(bindings[name], path) for name, path in staged.items()):
        return 'unverified', 'reference modified its input'
    if stdout and args[-1] in ('-', 'pipe:1'):
        (reference / 'stdout').write_bytes(stdout)
    elif stdout:
        return 'unverified', 'reference standard output'
    mine, theirs = outputs(candidate), outputs(reference)
    if set(mine) != set(theirs) or not theirs:
        return 'failed', 'output files'
    try:
        inputs = decoded_output.raw_inputs(args, sources[names[0]])
    except (TypeError, ValueError, ZeroDivisionError):
        return 'unverified', 'raw output description'
    output = args[max(i for i, a in enumerate(args) if a == '-i') + 2:]
    form = dict(zip(output, output[1:])).get('-f')
    for name in sorted(theirs):
        listing = decoded_output.probe(theirs[name], ffprobe, env, inputs)
        if listing is None:
            return 'unverified', 'reference probe'
        if any(s.get('codec_name') in ('png', 'bmp') for s in listing['streams']):
            videos = [s for s in sources[names[0]]['streams'] if s.get('codec_type') == 'video']
            if not videos or any(s.get('pix_fmt') not in ('rgb24', 'bgr24', 'gbrp', 'rgba', 'bgra', 'gbrap')
                                 for s in videos):
                return 'unverified', 'no_oracle: PNG/BMP colour conversion'
        mapping = copy_mapping(stderr, [sources[n] for n in names], listing) if (
            len(theirs) == 1 and form in (None, 'mp4', 'matroska')) else ()
        result = decoded_output.compare(mine[name], theirs[name], ffmpeg, ffprobe, env, scratch, inputs,
                                         copied_streams=tuple(index for index, _, _ in mapping))
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
                bindings = {a: staged for a in row['args'] if a.startswith('INPUT')}
                status, reason = verify(row['args'], bindings, candidate, reference, env, ffmpeg, ffprobe, scratch)
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
    try:
        pair = test_oracle.require()
    except test_oracle.QualificationError as error:
        parser.error(str(error))
    ffmpeg, ffprobe = str(pair.ffmpeg), str(pair.ffprobe)
    legacy = reporter()
    legacy.ENV = dict(legacy.ENV, PATH=str(pair.ffprobe.parent) + os.pathsep + legacy.ENV.get('PATH', ''))
    if pathlib.Path(shutil.which('ffprobe', path=legacy.ENV['PATH']) or 'ffprobe').resolve() != pair.ffprobe.resolve():
        parser.error('PATH does not resolve ffprobe to the qualified oracle')
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
