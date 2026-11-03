"""Verify the registered supplemental tuples with the unchanged reference command."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import shutil
import sys
import types

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / 'scripts'), str(ROOT / 'tools/compat-corpus')]
import encode_breadth as v2
import oracle_pin
import v2_fixture_inputs as inputs


def verify(opts, row):
    entry = json.loads(inputs.COMMANDS.read_text())['commands'][row['index']]
    attempts = []
    base = opts.work / str(row['index'])
    for number, assignment in enumerate(row['assignments']):
        run = v2.prepare(opts, entry, base, str(number), assignment)
        try:
            result = v2.execute([opts.ffmpeg, *run.args], run.cwd, opts.env,
                                v2.REFERENCE_SECONDS, entry['output'] == '-')
            ok = v2.succeeded(result, entry['output'], run.cwd)
            stderr = result.stderr.replace(str(opts.work), '$WORK')
            attempts.append({'assignment': assignment, 'exit': result.code, 'timed_out': result.timed_out,
                             'output_written': bool(v2.produced(entry['output'], run.cwd)),
                             'ok': ok, 'stderr': '\n'.join(stderr.splitlines()[-40:])})
            if ok:
                break
        finally:
            shutil.rmtree(base / str(number))
    return {'index': row['index'], 'ok': any(a['ok'] for a in attempts), 'attempts': attempts}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ffmpeg', type=Path, required=True)
    parser.add_argument('--fixtures', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--only', type=int, nargs='+')
    opts = parser.parse_args()
    for name in ('ffmpeg', 'fixtures', 'work', 'report'):
        setattr(opts, name, getattr(opts, name).resolve())
    if sys.platform == 'darwin':
        import oracle_pin_macos
        problems = list(oracle_pin_macos.verify_bin(opts.ffmpeg.parent))
    else:
        problems = list(oracle_pin.verify_bin(opts.ffmpeg.parent))
    if opts.ffmpeg.name != 'ffmpeg' or problems:
        raise ValueError('reference fixture verification requires the pinned oracle: ' + '; '.join(problems))
    rows = [r for r in inputs.DOCUMENT['cases'] if r['assignments']]
    if opts.only is not None:
        selected = set(opts.only)
        if selected - {r['index'] for r in rows}:
            raise ValueError('requested case has no supplemental assignment')
        rows = [r for r in rows if r['index'] in selected]
    opts.env = v2.environment()
    opts.work.mkdir(parents=True, exist_ok=False)
    with ThreadPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(lambda row: verify(opts, row), rows))
    opts.report.parent.mkdir(parents=True, exist_ok=True)
    opts.report.write_text(json.dumps({'scope': 'reference assignment only; no candidate evaluation',
                                      'registration': inputs.provenance(), 'cases': results,
                                      'without_supplemental_assignment': [
                                          {'index': r['index'], 'reason': r['reason']}
                                          for r in inputs.DOCUMENT['cases'] if not r['assignments']
                                      ]}, indent=2) + '\n')
    for result in results:
        if not result['ok']:
            print(f"stage=reference case={result['index']} reason=no_successful_supplemental_tuple", file=sys.stderr)
            for attempt in result['attempts']:
                print(json.dumps(attempt), file=sys.stderr)
    return int(any(not r['ok'] for r in results))


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f'stage=reference reason=tool_error detail={error}', file=sys.stderr)
        sys.exit(2)
