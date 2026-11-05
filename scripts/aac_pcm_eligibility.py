"""Compare complete AAC PCM presentations with the pinned audio oracle."""

import argparse
import array
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
from oracle_pin import load, verify_bin


def run(args, *, env=None):
    return subprocess.run(list(map(str, args)), capture_output=True, timeout=120, env=env)


def native(coreav, source, options, *, strict=True):
    return [coreav, *(['--strict-safe'] if strict else []), 'ffmpeg', '-threads', '1',
            '-i', source, *options]


def compare(actual, reference, fmt, channels):
    code, width, limit = ('f', 4, 2 ** -16) if fmt == 'f32' else ('h', 2, 1)
    if len(actual) != len(reference) or not actual or len(actual) % (width * channels):
        raise ValueError('complete sample counts differ or are empty')
    a, b = array.array(code), array.array(code)
    a.frombytes(actual)
    b.frombytes(reference)
    if sys.byteorder != 'little':
        a.byteswap()
        b.byteswap()
    maximum = 0
    for x, y in zip(a, b):
        if not math.isfinite(x) or not math.isfinite(y):
            raise ValueError('nonfinite PCM sample')
        maximum = max(maximum, abs(x - y))
    if maximum > limit:
        raise ValueError(f'{fmt} maximum error {maximum} exceeds {limit}')
    return {'samples': len(actual) // (width * channels), 'max_error': maximum}


def wav(data, fmt, channels, rate):
    if data[:4] != b'RIFF' or data[8:12] != b'WAVE':
        raise ValueError('missing WAVE header')
    size = struct.unpack_from('<I', data, 4)[0]
    if size != 0xffffffff and size + 8 != len(data):
        raise ValueError('incorrect WAVE RIFF extent')
    at = 12
    valid_format = False
    while at + 8 <= len(data):
        kind, size = struct.unpack_from('<4sI', data, at)
        if kind == b'fmt ':
            tag, count, sample_rate, _, _, bits = struct.unpack_from('<HHIIHH', data, at + 8)
            if tag == 0xfffe:
                tag = struct.unpack_from('<H', data, at + 32)[0]
            valid_format = (count, sample_rate, tag, bits) == (
                channels, rate, 3 if fmt == 'f32' else 1, 32 if fmt == 'f32' else 16)
        if kind == b'data':
            if not valid_format:
                raise ValueError('incorrect WAVE sample format, rate or channels')
            if size != 0xffffffff and at + 8 + size != len(data):
                raise ValueError('incorrect WAVE data extent')
            return data[at + 8:] if size == 0xffffffff else data[at + 8:at + 8 + size]
        at += 8 + size + (size & 1)
    raise ValueError('missing WAVE data')


def check(coreav, oracle, source, work):
    row = {'sha256': hashlib.sha256(source.read_bytes()).hexdigest(), 'formats': {}}
    probe = run([oracle / 'ffprobe', '-v', 'error', '-select_streams', 'a:0',
                 '-show_entries', 'stream=sample_rate,channels,profile', '-of', 'json', source])
    if probe.returncode:
        raise ValueError('oracle probe failed')
    stream = json.loads(probe.stdout)['streams'][0]
    row.update(rate=int(stream['sample_rate']), channels=stream['channels'], profile=stream['profile'])
    refusal = None
    for fmt, codec in [('f32', 'pcm_f32le'), ('s16', 'pcm_s16le')]:
        base = native(coreav, source, ['-map', '0:a:0', '-c:a', codec])
        actual = run([*base, '-f', fmt + 'le', 'pipe:'])
        if actual.returncode == 4 and b'AAC decode tolerance requires' in actual.stderr:
            if row['formats']:
                raise ValueError('eligibility differs across formats')
            if actual.stdout:
                raise ValueError('ineligible stream wrote samples')
            refusal = actual.stderr.decode().strip()
            destination = work / 'preserved.pcm'
            destination.write_bytes(b'preserved')
            refused = run([*base, '-y', '-f', fmt + 'le', destination])
            if refused.returncode != 4 or destination.read_bytes() != b'preserved':
                raise ValueError('ineligible stream changed its destination')
            wave = run([*base, '-f', 'wav', 'pipe:'])
            if wave.returncode != 4 or wave.stdout:
                raise ValueError('ineligible stream wrote a WAVE header')
            env = {**os.environ, 'COREAV_FALLBACK': str(oracle / 'ffmpeg')}
            env.pop('COREAV_STRICT_SAFE', None)
            fallback = run([*native(coreav, source, ['-map', '0:a:0', '-c:a', codec], strict=False),
                            '-f', fmt + 'le', 'pipe:'], env=env)
            reference = run([oracle / 'ffmpeg', '-v', 'error', *AUDIO_DECODE_INPUT,
                             '-i', source, '-map', '0:a:0', '-c:a', codec, '-f', fmt + 'le', '-'])
            if fallback.returncode or reference.returncode or fallback.stdout != reference.stdout:
                raise ValueError('ineligible stream did not replay through the pinned fallback')
            continue
        if actual.returncode:
            raise ValueError('native command failed: ' + actual.stderr.decode()[-500:])
        if refusal:
            raise ValueError('eligibility differs across formats')
        reference = run([oracle / 'ffmpeg', '-v', 'error', *AUDIO_DECODE_INPUT,
                         '-i', source, '-map', '0:a:0', '-c:a', codec, '-f', fmt + 'le', '-'])
        if reference.returncode or reference.stderr:
            raise ValueError('oracle decode failed or reported damage')
        result = compare(actual.stdout, reference.stdout, fmt, row['channels'])
        destination = work / 'candidate.pcm'
        file_output = run([*base, '-y', '-f', fmt + 'le', destination])
        if file_output.returncode or destination.read_bytes() != actual.stdout:
            raise ValueError('file and stdout packing differ')
        wave = run([*base, '-f', 'wav', 'pipe:'])
        if wave.returncode:
            raise ValueError('native WAV failed')
        compare(wav(wave.stdout, fmt, row['channels'], row['rate']), reference.stdout, fmt, row['channels'])
        wave_file = work / 'candidate.wav'
        wave = run([*base, '-y', '-f', 'wav', wave_file])
        if wave.returncode:
            raise ValueError('native WAV file failed')
        compare(wav(wave_file.read_bytes(), fmt, row['channels'], row['rate']), reference.stdout, fmt, row['channels'])
        row['formats'][fmt] = result
    row['status'] = 'ineligible' if refusal else 'pass'
    if refusal:
        row['reason'] = refusal
    return row


def rewrite(data, replacements):
    out, at = bytearray(), 0
    while at < len(data):
        size, kind = struct.unpack_from('>I4s', data, at)
        if size < 8 or at + size > len(data):
            raise ValueError('invalid generated MP4 box')
        payload = data[at + 8:at + size]
        if kind in (b'moov', b'trak', b'edts', b'mdia', b'minf', b'stbl'):
            payload = rewrite(payload, replacements)
        if kind in replacements:
            kind, payload = replacements[kind](payload)
        out.extend(struct.pack('>I4s', len(payload) + 8, kind) + payload)
        at += size
    return bytes(out)


def controls(source, work):
    data = source.read_bytes()
    values = {}

    def record(kind):
        def save(payload):
            values[kind] = payload
            return kind, payload
        return save

    rewrite(data, {kind: record(kind) for kind in (b'mdhd', b'elst', b'stts')})
    mdhd, elst = values[b'mdhd'], values[b'elst']
    if mdhd[0] or elst[:8] != bytes(7) + b'\x01':
        raise ValueError('unexpected generated version or edit count')
    duration, start, rate, fraction = struct.unpack_from('>Iihh', elst, 8)
    if rate != 1 or fraction or start < 0:
        raise ValueError('unexpected generated edit rate')
    paths = []
    no_edits = {b'elst': lambda payload: (b'free', payload)}
    variants = {'no-edit': no_edits,
                'partial-listing': {b'elst': lambda _: (b'elst', bytes(4) + struct.pack('>I', 1)
                    + struct.pack('>Iihh', 1, start, 1, 0))},
                'leading-empty': {b'elst': lambda _: (b'elst', bytes(4) + struct.pack('>I', 2)
                    + struct.pack('>Iihh', 1, -1, 1, 0) + struct.pack('>Iihh', duration, start, 1, 0))}}
    shortened = bytearray(mdhd)
    struct.pack_into('>I', shortened, 16, struct.unpack_from('>I', mdhd, 16)[0] - 28)
    variants['short-media-header'] = {**no_edits, b'mdhd': lambda _: (b'mdhd', bytes(shortened))}
    irregular = bytearray(values[b'stts'])
    struct.pack_into('>I', irregular, 12, struct.unpack_from('>I', irregular, 12)[0] + 1)
    variants['irregular-clock'] = {**no_edits, b'stts': lambda _: (b'stts', bytes(irregular))}
    table = values[b'stts']
    count = sum(struct.unpack_from('>I', table, at)[0] for at in range(8, len(table), 8))

    def box(kind, payload):
        return struct.pack('>I4s', len(payload) + 8, kind) + payload

    text = (f' 00000000 00000040 00000000 {count * 1024 - 64:016X}'
            + ' 00000000' * 8).encode()
    item = box(b'----', box(b'mean', bytes(4) + b'com.apple.iTunes')
               + box(b'name', bytes(4) + b'iTunSMPB')
               + box(b'data', struct.pack('>II', 1, 0) + text))
    handler = box(b'hdlr', bytes(8) + b'mdirappl' + bytes(8) + b'\0')
    tags = box(b'udta', box(b'meta', bytes(4) + handler + box(b'ilst', item)))
    variants['itunes-delay'] = {**no_edits, b'udta': lambda _: (b'udta', tags[8:])}
    for label, edits in variants.items():
        path = work / (label + '.m4a')
        path.write_bytes(rewrite(data, edits))
        paths.append(path)
    noncanonical = work / 'noncanonical-gapless.m4a'
    canonical = (work / 'itunes-delay.m4a').read_bytes()
    marker = canonical.index(b'mdirappl')
    noncanonical.write_bytes(canonical[:marker] + b'noneappl' + canonical[marker + 8:])
    paths.append(noncanonical)
    return paths


def destination_checks(coreav, source, work):
    target, link = work / 'protected.pcm', work / 'linked.pcm'
    target.write_bytes(b'protected')
    link.unlink(missing_ok=True)
    link.symlink_to(target.resolve())
    base = native(coreav, source, ['-c:a', 'pcm_s16le', '-y', '-f', 's16le'])
    result = run([*base, link])
    if result.returncode != 4 or not link.is_symlink() or target.read_bytes() != b'protected':
        raise ValueError('symlink destination was changed')
    target.chmod(0o444)
    try:
        result = run([*base, target])
        if result.returncode != 4 or target.read_bytes() != b'protected':
            raise ValueError('read-only destination was changed')
    finally:
        target.chmod(0o644)
    if hasattr(os, 'mkfifo'):
        fifo = work / 'destination.fifo'
        fifo.unlink(missing_ok=True)
        os.mkfifo(fifo)
        result = run([*base, fifo])
        import stat
        if result.returncode != 4 or not stat.S_ISFIFO(fifo.stat().st_mode):
            raise ValueError('FIFO destination was changed')


def generate(oracle, work):
    paths = []
    for rate in (22050, 44100, 48000):
        for channels in (1, 2):
            for kind, expression in [('tone', '0.2*sin(2*PI*431*t)'),
                                     ('switching', '0.1*sin(2*PI*431*t)+if(lt(mod(t,0.04),0.001),0.7,0)')]:
                path = work / f'{kind}-{rate}-{channels}.m4a'
                expression = '|'.join(expression.replace('431', str(431 + c * 107)) for c in range(channels))
                encoded = run([oracle / 'ffmpeg', '-v', 'error', '-y', '-f', 'lavfi',
                               '-i', f"aevalsrc='{expression}':s={rate}:d=0.25",
                               '-c:a', 'aac', '-aac_pns', '0', path])
                if encoded.returncode:
                    raise ValueError('generated encode failed')
                paths.append(path)
    paths.extend(controls(work / 'tone-48000-1.m4a', work))
    path = work / 'pns.m4a'
    encoded = run([oracle / 'ffmpeg', '-v', 'error', '-y', '-f', 'lavfi',
                   '-i', 'sine=frequency=440:sample_rate=48000:duration=2', '-c:a', 'aac', path])
    if encoded.returncode:
        raise ValueError('generated PNS encode failed')
    paths.append(path)
    path = work / 'all-ms-intensity.m4a'
    encoded = run([oracle / 'ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i',
                   "aevalsrc='0.2*sin(2*PI*431*t)|0.2*sin(2*PI*538*t)':s=48000:d=0.5",
                   '-c:a', 'aac', '-aac_pns', '0', '-aac_ms', '1', '-b:a', '32k', path])
    if encoded.returncode:
        raise ValueError('generated all-MS encode failed')
    paths.append(path)
    return paths


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--oracle-dir', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('files', type=Path, nargs='*')
    args = parser.parse_args()
    args.out.unlink(missing_ok=True)
    errors = list(verify_bin(args.oracle_dir))
    if errors:
        raise ValueError('unverified pinned oracle')
    args.work.mkdir(parents=True, exist_ok=True)
    files = args.files or generate(args.oracle_dir, args.work)
    if not args.files:
        destination_checks(args.coreav, args.work / 'tone-48000-1.m4a', args.work)
    rows = []
    for source in files:
        try:
            row = check(args.coreav, args.oracle_dir, source, args.work)
            expected = 'ineligible' if source.name in ('pns.m4a', 'all-ms-intensity.m4a',
                                                       'noncanonical-gapless.m4a') else 'pass'
            if not args.files and row['status'] != expected:
                raise ValueError('generated fixture changed its expected eligibility')
        except (ValueError, subprocess.TimeoutExpired) as error:
            row = {'sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
                   'status': 'fail', 'reason': str(error)}
        rows.append(row)
        print(json.dumps(row), flush=True)
    args.out.write_text(json.dumps({
        'schema': 'coreav.aac-pcm-qualification/1',
        'oracle': load()['BUILD'],
        'binary_sha256': hashlib.sha256(args.coreav.read_bytes()).hexdigest(),
        'qualifier_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'rows': rows}, indent=2) + '\n')
    return int(not rows or any(row['status'] == 'fail' for row in rows))


if __name__ == '__main__':
    sys.exit(main())
