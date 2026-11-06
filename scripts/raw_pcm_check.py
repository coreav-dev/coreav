"""Verify generated raw S16 commands against the pinned default oracle."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
import test_oracle
from pcm_output_check import fields, native_command, run
from swr_characterize import RATES, pcm, signal


def cases():
    for incoming, outgoing in [*RATES, (11025, 48000), (48000, 32000), (96000, 44100), (44100, 96000), (19400, 1400)]:
        for channels in (1, 2):
            for name, frames in (('lcg-full', 1025), ('extrema', 257)):
                for kind in ('wav', 'raw'):
                    yield incoming, outgoing, channels, name, frames, kind
        if incoming != outgoing and (incoming, outgoing) in RATES:
            for frames in (0, 21, 22, 100):
                yield incoming, outgoing, 1, 'constant', frames, 'wav'
    for rate in (48000, 48001, 64000, 65535, 65536, 96000):
        for channels in (1, 2):
            yield rate, rate, channels, 'constant', 257, 'wav'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def check(coreav, oracle, root):
    ffmpeg, ffprobe = oracle.ffmpeg, oracle.ffprobe
    env = {**os.environ, 'COREAV_STRICT_SAFE': '1', 'COREAV_FALLBACK': str(root / 'absent-ffmpeg')}
    outcomes = []
    replay = []
    for number, (incoming, outgoing, channels, name, frames, kind) in enumerate(cases()):
        source = root / f'{number}.pcm'
        samples = pcm(signal(name, frames), channels, 's16le')
        source.write_bytes(samples)
        ours = root / f'ours-{number}.{kind}'
        theirs = root / f'oracle-{number}.{kind}'
        before = ['-f', 's16le', '-ar', '44.1k' if incoming == 44100 else str(incoming), '-ac', str(channels)]
        after = ['-ar', '44.1k' if outgoing == 44100 else str(outgoing)]
        after += ['-c:a', 'pcm_s16le', '-f', 's16le'] if kind == 'raw' else ['-f', 'wav']
        if run(native_command(coreav, source, ours, before, after), env=env):
            raise AssertionError(f'case {number}: unexpected native stdout')
        run([ffmpeg, '-v', 'error', *before, *AUDIO_DECODE_INPUT, '-i', source, *after, theirs])
        raw = kind == 'raw'
        ours_fields = fields(ffprobe, ours, raw, outgoing, channels)
        reference_fields = fields(ffprobe, theirs, raw, outgoing, channels)
        if ours_fields != reference_fields:
            raise AssertionError(f'case {number}: fields differ: {ours_fields} != {reference_fields}')
        decoded = []
        for path in (ours, theirs):
            decoded.append(path.read_bytes() if raw else run([
                ffmpeg, '-v', 'error', *AUDIO_DECODE_INPUT, '-i', path,
                '-f', 's16le', '-c:a', 'pcm_s16le', '-',
            ]))
        actual, reference = decoded
        if actual != reference or (incoming == outgoing and actual != samples):
            first = next((i for i, (a, b) in enumerate(zip(actual, reference)) if a != b), None)
            raise AssertionError(f'case {number}: samples differ: native {len(actual)} bytes, oracle {len(reference)} bytes; first byte {first}')
        outcomes.append({'input_rate': incoming, 'output_rate': outgoing, 'channels': channels,
                         'signal': name, 'input_frames': frames, 'kind': kind,
                         'output_frames': len(actual) // (channels * 2), 'fields': ours_fields,
                         'result': 'equal'})
        if raw:
            replay.append(f'{source}\t{theirs}\t{incoming}\t{outgoing}\t{channels}')
    (root / 'replay.tsv').write_text('\n'.join(replay) + '\n')
    return outcomes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--oracle-dir', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--work-dir', type=Path)
    args = parser.parse_args()
    try:
        oracle = test_oracle.require(args.oracle_dir / 'ffmpeg', args.oracle_dir / 'ffprobe')
    except test_oracle.QualificationError as error:
        parser.error(str(error))
    if args.work_dir:
        args.work_dir.mkdir(parents=True, exist_ok=False)
        outcomes = check(args.coreav.resolve(), oracle, args.work_dir.resolve())
    else:
        with tempfile.TemporaryDirectory(prefix='coreav-raw-') as temporary:
            outcomes = check(args.coreav.resolve(), oracle, Path(temporary))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    report = {'scope': 'synthetic verification, not judged command coverage',
              'source': subprocess.check_output(['git', '-C', str(Path(__file__).resolve().parents[1]), 'rev-parse', 'HEAD'], text=True, timeout=10).strip(),
              'coreav_sha256': digest(args.coreav), 'ffmpeg_sha256': digest(oracle.ffmpeg),
              'ffprobe_sha256': digest(oracle.ffprobe), 'outcomes': outcomes}
    args.out.write_text(json.dumps(report, indent=2) + '\n')
    print(f'Raw PCM: {len(outcomes)} commands matched complete samples and ffprobe fields')


if __name__ == '__main__':
    main()
