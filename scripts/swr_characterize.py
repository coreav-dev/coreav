import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
from oracle_pin import verify_bin

RATES = [(44100, 48000), (44100, 22050), (44100, 16000),
         (48000, 44100), (48000, 22050), (48000, 16000),
         (8000, 44100), (8000, 8000), (44100, 44100), (48000, 48000)]
FORMS = {'s16le': ('h', 32768), 's32le': ('i', 2147483648),
         'f32le': ('f', 1), 'f64le': ('d', 1)}


def signal(name, length):
    if name.startswith('impulse-'):
        where = name.removeprefix('impulse-')
        position = {'first': 0, 'second': 1, 'middle': length // 2,
                    'middle-next': length // 2 + 1, 'penultimate': length - 2,
                    'last': length - 1}[where]
        return [0.25 if n == position else 0.0 for n in range(length)]
    if name == 'constant':
        return [0.25] * length
    if name == 'step':
        return [0.0 if n < length // 2 else 0.25 for n in range(length)]
    if name == 'alternating':
        return [0.25 if n % 2 else -0.25 for n in range(length)]
    if name == 'irregular':
        return [(((n * 4051 + 97) % 8192) - 4096) / 32768 for n in range(length)]
    if name == 'zero':
        return [0.0] * length
    raise ValueError('unknown signal')


def pcm(values, channels, form):
    code, scale = FORMS[form]
    samples = [value * ((-1 if c % 2 else 1) / (2 ** c))
               for value in values for c in range(channels)]
    if code in ('h', 'i'):
        samples = [int(v * scale) for v in samples]
    return struct.pack('<' + code * len(samples), *samples)


def cases():
    names = ['impulse-first', 'impulse-second', 'impulse-middle',
             'impulse-middle-next', 'impulse-penultimate', 'impulse-last',
             'constant', 'step', 'alternating', 'irregular']
    for incoming, outgoing in RATES:
        for name in names:
            for scalar in (False, True):
                yield incoming, outgoing, 'f32le', 'f32le', 1, name, 257, scalar, None
        for length in (0, 1, 2, 15, 16, 17, 31, 32, 33, 64, 65, 127, 128, 129):
            yield incoming, outgoing, 'f32le', 'f32le', 1, 'constant', length, False, None
        for channels in (2, 3):
            yield incoming, outgoing, 'f32le', 'f32le', channels, 'irregular', 257, False, None
        for source, target in (('s16le', 's16le'), ('s32le', 's32le'),
                               ('f32le', 's16le'), ('f32le', 's32le'), ('f32le', 'f64le')):
            for name in ('impulse-middle', 'constant', 'irregular'):
                yield incoming, outgoing, source, target, 1, name, 257, False, None
        for block in (1, 7, 31):
            yield incoming, outgoing, 'f32le', 'f32le', 2, 'irregular', 257, False, block


def capture(ffmpeg, directory):
    problems = list(verify_bin(ffmpeg.parent))
    if problems:
        raise RuntimeError('\n'.join(problems))
    directory.mkdir(parents=True, exist_ok=False)
    rows = []
    started = time.monotonic()
    total = 0
    root = Path(__file__).resolve().parents[1]
    provenance = {
        'schema': 'coreav/swr-characterization/1',
        'source': subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip(),
        'ffmpeg_sha256': hashlib.sha256(ffmpeg.read_bytes()).hexdigest(),
        'ffmpeg_version': subprocess.check_output([str(ffmpeg), '-version'], timeout=10, text=True),
        'script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'plan_sha256': hashlib.sha256((root / 'docs/design/audio-swr-parity-plan.md').read_bytes()).hexdigest(),
        'complete': False,
    }
    try:
        for index, case in enumerate(cases()):
            incoming, outgoing, source, target, channels, name, length, scalar, block = case
            remaining = 300 - (time.monotonic() - started)
            if index >= 768 or remaining <= 0:
                raise RuntimeError('characterization call or time cap')
            data = pcm(signal(name, length), channels, source)
            args = [str(ffmpeg), '-v', 'error', '-nostdin']
            if scalar:
                args += ['-cpuflags', '0']
            args += ['-f', source, '-ar', str(incoming), '-ac', str(channels),
                     *AUDIO_DECODE_INPUT, '-i', 'pipe:0']
            if block is not None:
                args += ['-af', f'asetnsamples=n={block}:p=0']
            args += ['-ar', str(outgoing), '-dither_method', 'none',
                     '-c:a', 'pcm_' + target, '-f', target, 'pipe:1']
            result = subprocess.run(args, input=data, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, timeout=min(10, remaining))
            total += len(data) + len(result.stdout) + len(result.stderr)
            if total > 32 << 20:
                raise RuntimeError('characterization output cap')
            if result.returncode and length:
                raise RuntimeError(f'case {index} failed: {result.stderr.decode(errors="replace")}')
            stem = f'{index:04d}'
            (directory / (stem + '.input')).write_bytes(data)
            (directory / (stem + '.output')).write_bytes(result.stdout)
            rows.append(dict(zip(
                ('incoming', 'outgoing', 'source', 'target', 'channels', 'signal', 'frames', 'scalar', 'block'), case),
                id=stem, args=['ffmpeg', *args[1:]], status=result.returncode,
                stderr=result.stderr.decode(errors='replace'),
                input_sha256=hashlib.sha256(data).hexdigest(),
                output_sha256=hashlib.sha256(result.stdout).hexdigest(),
                output_bytes=len(result.stdout)))
        provenance['complete'] = True
    finally:
        provenance['cases'] = len(rows)
        provenance['elapsed_seconds'] = time.monotonic() - started
        (directory / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
        (directory / 'cases.json').write_text(json.dumps(rows, indent=2) + '\n')
    print(json.dumps({'complete': provenance['complete'], 'cases': len(rows), 'bytes': total}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description='Capture bounded synthetic swr behavior, not command coverage')
    parser.add_argument('--ffmpeg', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    options = parser.parse_args()
    capture(options.ffmpeg.resolve(), options.out.resolve())
