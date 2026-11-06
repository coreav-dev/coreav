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
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
from oracle_pin import load
from test_oracle import verify_bin


def run(args, *, env=None):
    return subprocess.run(list(map(str, args)), capture_output=True, timeout=120, env=env)


def native(coreav, source, options, *, strict=True):
    input_args = [arg.removesuffix(':a') for arg in AUDIO_DECODE_INPUT]
    return [coreav, *(['--strict-safe'] if strict else []), 'ffmpeg', *input_args,
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


def compare_wav(actual, reference, native_pcm, oracle_pcm, channels, rate):
    a = wav(actual, 's16', channels, rate)
    b = wav(reference, 's16', channels, rate)
    if not a or a != native_pcm or b != oracle_pcm:
        raise ValueError('complete WAV payload differs from its qualified PCM')
    if actual[:-len(a)] != reference[:-len(b)]:
        raise ValueError('complete S16 WAV framing or metadata differs')


def pns_check(oracle, source, directory, metadata):
    import numpy as np
    import aac_pns

    reference = run([oracle / 'ffmpeg', '-v', 'error', '-f', 'aac', *AUDIO_DECODE_INPUT,
                     '-i', directory / 'packets.aac', '-c:a', 'pcm_f32le', '-f', 'f32le', '-'])
    if reference.returncode or reference.stderr:
        raise ValueError('untrimmed oracle failed or reported damage')
    channels, packets = metadata['channels'], metadata['packets']
    native_raw = np.fromfile(directory / 'native.f32', dtype='<f4').reshape(-1, channels)
    oracle_raw = np.frombuffer(reference.stdout, dtype='<f4').reshape(-1, channels)
    spectra = np.fromfile(directory / 'spectra.f32', dtype='<f4').reshape(packets, channels, 1024)
    metrics = aac_pns.compare(native_raw, oracle_raw, metadata, spectra)
    probe = run([oracle / 'ffprobe', '-v', 'error', '-select_streams', 'a:0',
                 '-show_streams', '-show_packets', '-of', 'json', source])
    if probe.returncode or probe.stderr:
        raise ValueError('PNS presentation probe failed')
    skip, count = presentation_interval(metadata, json.loads(probe.stdout))
    metrics.update(presented_samples=count, leading_skip=skip,
                   trailing_discard=packets * 1024 - skip - count)
    return metrics, native_raw[skip:skip + count], oracle_raw[skip:skip + count]


def presentation_interval(metadata, listing):
    packets = metadata['packets']
    listed = listing['packets']
    if len(listed) != packets or any(int(packet['pos']) != frame['offset']
                                    for packet, frame in zip(listed, metadata['frames'])):
        raise ValueError('PNS presentation does not preserve the complete stored packet sequence')
    skip = 0
    discard = 0
    for index, packet in enumerate(listed):
        if int(packet['pts']) != int(packet['dts']) or int(packet['duration']) <= 0:
            raise ValueError('unsupported PNS presentation clock')
        for item in packet.get('side_data_list', []):
            if item.get('side_data_type') != 'Skip Samples':
                raise ValueError('unsupported PNS side data')
            terminal = int(item.get('discard_padding', 0))
            if terminal < 0 or terminal and index != len(listed) - 1:
                raise ValueError('unsupported PNS padding placement')
            if index == len(listed) - 1:
                discard = terminal
            value = int(item.get('skip_samples', 0))
            if value < 0 or value and index != 0:
                raise ValueError('unsupported PNS skip placement')
            if index == 0:
                skip = value
    end = int(listing['streams'][0]['duration_ts'])
    if metadata['no_edits']:
        end = metadata['media_duration']
        if metadata['gapless_end'] is not None:
            end = min(end, metadata['gapless_end'])
    last = listed[-1]
    pts, duration = int(last['pts']), max(1024, int(last['duration']))
    padding = max(0, duration - (end - pts)) if pts < end else 0
    if discard and discard != padding:
        raise ValueError('PNS padding disagrees with the independently predicted endpoint')
    raw_count = packets * 1024
    last_skip = max(0, skip - (raw_count - 1024))
    count = raw_count - skip - min(padding, max(0, 1024 - last_skip))
    if count <= 0 or skip < 0 or skip + count > raw_count:
        raise ValueError('invalid PNS presentation extent')
    return skip, count


def check(coreav, oracle, source, work, diagnostic, capture=None):
    with tempfile.TemporaryDirectory(prefix='aac-diagnostic-', dir=work) as path:
        directory = Path(path)
        captured = run([diagnostic, source, directory])
        metadata, pns = None, None
        if captured.returncode == 0:
            metadata = json.loads((directory / 'metadata.json').read_text())
            noise = sum(len(channel['noise']) for frame in metadata['frames'] for channel in frame['channels'])
            if noise and not metadata['unsafe']:
                pns = pns_check(oracle, source, directory, metadata)
        row = check_output(coreav, oracle, source, work, metadata, pns, capture)
        if captured.returncode:
            row['diagnostic_refusal'] = captured.stderr.decode()[-500:].strip()
        return row


def check_output(coreav, oracle, source, work, metadata, pns, capture=None):
    row = {'sha256': hashlib.sha256(source.read_bytes()).hexdigest(), 'formats': {}}
    probe = run([oracle / 'ffprobe', '-v', 'error', '-select_streams', 'a:0',
                 '-show_entries', 'stream=sample_rate,channels,profile', '-of', 'json', source])
    if probe.returncode:
        raise ValueError('oracle probe failed')
    stream = json.loads(probe.stdout)['streams'][0]
    row.update(rate=int(stream['sample_rate']), channels=stream['channels'], profile=stream['profile'])
    refusal = None
    floats = None
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
        if metadata is None or 'all_ms_intensity' in metadata['unsafe'] or metadata['unsafe'] and any(
                channel['noise'] for frame in metadata['frames'] for channel in frame['channels']):
            raise ValueError('native command admitted unsupported diagnostic predicates')
        if refusal:
            raise ValueError('eligibility differs across formats')
        reference = run([oracle / 'ffmpeg', '-v', 'error', *AUDIO_DECODE_INPUT,
                         '-i', source, '-map', '0:a:0', '-c:a', codec, '-f', fmt + 'le', '-'])
        if reference.returncode or reference.stderr:
            raise ValueError('oracle decode failed or reported damage')
        if pns is None:
            result = compare(actual.stdout, reference.stdout, fmt, row['channels'])
        else:
            import numpy as np
            import aac_pns
            metrics, native_presented, oracle_presented = pns
            count = metrics['presented_samples']
            width = 4 if fmt == 'f32' else 2
            if len(actual.stdout) != count * width * row['channels'] or len(reference.stdout) != len(actual.stdout):
                raise ValueError('PNS complete presented sample counts differ')
            if fmt == 'f32':
                floats = [np.frombuffer(data, dtype='<f4').reshape(-1, row['channels'])
                          for data in (actual.stdout, reference.stdout)]
                if not np.array_equal(floats[0], native_presented) or not np.array_equal(floats[1], oracle_presented):
                    raise ValueError('presented PCM does not equal its independently predicted untrimmed interval')
                result = {'samples': count, 'domain': 'non_pns_projection',
                          'max_error': metrics['max_projection_f32_error']}
            else:
                if floats is None:
                    raise ValueError('missing complete F32 packing reference')
                errors = [int(np.max(np.abs(np.frombuffer(data, dtype='<i2').reshape(-1, row['channels']).astype(np.int64)
                                            - aac_pns.quantize_s16(values))))
                          for data, values in zip((actual.stdout, reference.stdout), floats)]
                if max(errors) > 1:
                    raise ValueError('PNS S16 packing differs from its own F32 samples')
                result = {'samples': count, 'domain': 'non_pns_projection',
                          'max_error': metrics['max_projection_s16_error'],
                          'native_packing_lsb': errors[0], 'oracle_packing_lsb': errors[1]}
            row['pns'] = metrics
        destination = work / 'candidate.pcm'
        file_output = run([*base, '-y', '-f', fmt + 'le', destination])
        if file_output.returncode or destination.read_bytes() != actual.stdout:
            raise ValueError('file and stdout packing differ')
        wave_base = [*base, *(['-map_metadata', '-1'] if fmt == 's16' else [])]
        wave = run([*wave_base, '-f', 'wav', 'pipe:'])
        if wave.returncode:
            raise ValueError('native WAV failed')
        if wav(wave.stdout, fmt, row['channels'], row['rate']) != actual.stdout:
            raise ValueError('WAV stdout packing differs from complete raw output')
        if fmt == 's16':
            pin_wave = run([oracle / 'ffmpeg', '-v', 'error', *AUDIO_DECODE_INPUT,
                            '-i', source, '-map', '0:a:0', '-map_metadata', '-1',
                            '-c:a', codec, '-f', 'wav', 'pipe:'])
            if pin_wave.returncode or pin_wave.stderr:
                raise ValueError('pinned WAV stdout failed')
            compare_wav(wave.stdout, pin_wave.stdout, actual.stdout, reference.stdout,
                        row['channels'], row['rate'])
        wave_file = work / 'candidate.wav'
        wave = run([*wave_base, '-y', '-f', 'wav', wave_file])
        if wave.returncode:
            raise ValueError('native WAV file failed')
        if wav(wave_file.read_bytes(), fmt, row['channels'], row['rate']) != actual.stdout:
            raise ValueError('WAV file packing differs from complete raw output')
        if fmt == 's16':
            pin_file = work / 'reference.wav'
            pin_wave = run([oracle / 'ffmpeg', '-v', 'error', '-y', *AUDIO_DECODE_INPUT,
                            '-i', source, '-map', '0:a:0', '-map_metadata', '-1',
                            '-c:a', codec, '-f', 'wav', pin_file])
            if pin_wave.returncode or pin_wave.stderr:
                raise ValueError('pinned WAV file failed')
            compare_wav(wave_file.read_bytes(), pin_file.read_bytes(), actual.stdout,
                        reference.stdout, row['channels'], row['rate'])
        row['formats'][fmt] = result
        if capture is not None:
            capture[fmt] = (actual.stdout, reference.stdout)
    row['status'] = 'ineligible' if refusal else 'pass'
    if refusal:
        row['reason'] = refusal
        if metadata is not None:
            row['predicate_failures'] = metadata['unsafe']
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


def generate(oracle, work, diagnostic):
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
    with tempfile.TemporaryDirectory(prefix='pns-fixtures-', dir=work) as temporary:
        fixtures = Path(temporary)
        made = run([diagnostic, '--fixtures', fixtures])
        if made.returncode:
            raise ValueError('synthetic PNS packet construction failed')
        for source in sorted(fixtures.glob('*.aac')):
            target = work / (source.stem + '.m4a')
            copied = run([oracle / 'ffmpeg', '-v', 'error', '-y', '-f', 'aac',
                          '-i', source, '-c:a', 'copy', target])
            if copied.returncode:
                raise ValueError('synthetic PNS packet remux failed')
            paths.append(target)
    return paths


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--oracle-dir', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--pns-diagnostic', type=Path, required=True)
    parser.add_argument('files', type=Path, nargs='*')
    args = parser.parse_args()
    args.out.unlink(missing_ok=True)
    errors = list(verify_bin(args.oracle_dir))
    if errors:
        raise ValueError('unverified pinned oracle')
    args.work.mkdir(parents=True, exist_ok=True)
    files = args.files or generate(args.oracle_dir, args.work, args.pns_diagnostic)
    if not args.files:
        destination_checks(args.coreav, args.work / 'tone-48000-1.m4a', args.work)
    rows = []
    for source in files:
        try:
            row = check(args.coreav, args.oracle_dir, source, args.work, args.pns_diagnostic)
            expected = 'ineligible' if source.name in ('all-ms-intensity.m4a',
                'noncanonical-gapless.m4a', 'pns-tns.m4a', 'pns-ms.m4a',
                'pns-illegal-window.m4a') else 'pass'
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
        'pns_checker_sha256': hashlib.sha256(Path(__file__).with_name('aac_pns.py').read_bytes()).hexdigest(),
        'diagnostic_sha256': hashlib.sha256(args.pns_diagnostic.read_bytes()).hexdigest(),
        'rows': rows}, indent=2) + '\n')
    return int(not rows or any(row['status'] == 'fail' for row in rows))


if __name__ == '__main__':
    sys.exit(main())
