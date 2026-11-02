#!/usr/bin/env python3
"""Run the unchanged registered V2 evaluator on its AVI subset."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import types

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / 'tools/compat-corpus'), str(ROOT / 'scripts')]
import encode_breadth as v2
import oracle_pin


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('coreav', 'ffmpeg', 'ffprobe', 'x264', 'build-log', 'fixtures', 'work'):
        parser.add_argument('--' + name, type=Path, required=True)
    opts = parser.parse_args()
    for name in ('coreav', 'ffmpeg', 'ffprobe', 'x264', 'fixtures', 'work'):
        setattr(opts, name, getattr(opts, name).resolve())
    problems = list(oracle_pin.verify_bin(opts.ffmpeg.parent))
    if problems:
        raise RuntimeError('\n'.join(problems))
    assert opts.ffprobe.parent == opts.ffmpeg.parent
    artifacts = [json.loads(line) for line in opts.build_log.read_text().splitlines() if line.startswith('{')]
    opts.harness, = [a['executable'] for a in artifacts if a.get('reason') == 'compiler-artifact'
                    and a.get('target', {}).get('name') == 'ffmpeg' and a.get('executable')]
    for name in v2.FIXTURES:
        assert (opts.fixtures / name).is_file(), name
    opts.work.mkdir(parents=True, exist_ok=True)
    opts.env = v2.environment()
    os.environ['GCONV_PATH'] = v2.release_round.NO_GCONV
    v2.transcode_check.ENV = opts.env
    opts.mediainfo = None
    opts.tc = types.SimpleNamespace(ffmpeg=opts.ffmpeg, ffprobe=opts.ffprobe, coreav=opts.coreav, decode_oracle=None)
    opts.check = types.SimpleNamespace(ffmpeg=opts.ffmpeg, ffprobe=opts.ffprobe, mediainfo=None,
                                       log=str(opts.work / 'copy-check.log'))
    decoded_oracle = hasattr(v2, 'mp3_encoder_start')
    rows = [(i, entry) for i, entry in v2.commands('v2') if entry['container'] == 'AVI']
    records = []
    report = {'scope': 'AVI subset of the registered V2 commands', 'decoded_oracle_present': decoded_oracle,
              'program_sha256': {k: digest(getattr(opts, k)) for k in ('coreav', 'harness', 'ffmpeg', 'ffprobe', 'x264')},
              'evaluator_sha256': digest(v2.__file__),
              'fixture_sha256': {k: digest(opts.fixtures / k) for k in v2.FIXTURES}, 'records': records}
    try:
        for index, entry in rows:
            record = v2.evaluate(opts, index, entry)
            records.append(record)
            print(json.dumps(record), flush=True)
            assert 'fault' not in record, record
        report['verified_rows'] = [r['index'] for r in records if r['counted']] if decoded_oracle else []
        report['complete'] = len(records) == len(rows)
    finally:
        (opts.work / 'v2.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
