"""Qualify family-0 Opus MP4 presentation without changing decoder tolerances."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import struct
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))

from opus_matroska_padding import matroska, run, wav_samples
from opus_mp4_presentation import metadata
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
from oracle_pin import verify_bin


def profile(data, config, channels):
    if config < 0 or config + 11 > len(data) or channels not in (1, 2) or \
            data[config:config + 2] != bytes((0, channels)) or \
            struct.unpack_from('>h', data, config + 8)[0] != 0 or data[config + 10] != 0:
        raise ValueError('requires mapping family 0, mono or stereo and zero gain')


def presented(raw, actual, start, length, channels, width):
    stride = channels * width
    if start < 0 or length < 0 or len(raw) % stride or (start + length) * stride > len(raw):
        raise ValueError('presentation outside decoded packet work')
    if actual != raw[start * stride:(start + length) * stride]:
        raise ValueError('presentation count, order or terminal samples changed')


def quality(text):
    values = re.findall(r'quality metric: ([^\s]+)', text)
    if len(values) != 1 or not math.isfinite(float(values[0])) or float(values[0]) <= 90:
        raise ValueError('Opus comparison must have quality strictly above 90')
    return float(values[0])


def compare(tool, work, label, reference, native, channels):
    if len(native) != len(reference) or not native or len(native) % (channels * 2):
        raise ValueError('complete reference and native PCM lengths must agree')
    ref, out = work / f'{label}-reference.s16', work / f'{label}-native.s16'
    ref.write_bytes(reference if channels == 2 else b''.join(
        reference[at:at + 2] * 2 for at in range(0, len(reference), 2)))
    out.write_bytes(native)
    result = subprocess.run([str(tool), *(['-s'] if channels == 2 else []), '-r', '48000',
                             str(ref), str(out)], capture_output=True, timeout=30, check=True)
    return quality((result.stdout + result.stderr).decode())


def check(coreav, oracle, tool, work):
    errors = list(verify_bin(oracle))
    if errors:
        raise ValueError('unverified oracle: ' + '; '.join(errors))
    if not tool.is_file():
        raise ValueError('the pinned Opus comparison tool is required')
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    for mode, bitrate, application in [('silk', '12000', 'voip'),
                                       ('hybrid', '24000', 'voip'),
                                       ('celt', '96000', 'lowdelay')]:
        for channels in (1, 2):
            label = f'{mode}-{channels}'
            source = work / f'{label}.f32'
            source.write_bytes(b''.join(struct.pack('<f',
                .23 * math.sin(2 * math.pi * (440 + 113 * channel) * n / 48000) +
                .07 * math.sin(2 * math.pi * (3171 + 71 * channel) * n / 48000))
                for n in range(4800) for channel in range(channels)))
            path = work / f'{label}.mp4'
            run([oracle / 'ffmpeg', '-v', 'error', '-f', 'f32le', '-ar', '48000', '-ac', channels,
                 *AUDIO_DECODE_INPUT, '-i', source, '-c:a', 'libopus', '-b:a', bitrate,
                 '-application', application, '-vbr', 'off', '-movie_timescale', '48000', '-y', path])
            data = path.read_bytes()
            info = metadata(data, channels=channels)
            profile(data, info['config'], channels)
            if not all((packet[0] >> 3 < 12 if mode == 'silk' else
                        12 <= packet[0] >> 3 < 16 if mode == 'hybrid' else packet[0] >> 3 >= 16)
                       for packet in info['packets']):
                raise ValueError('generated packet mode differs from the declared cell')
            head = b'OpusHead' + struct.pack('<BBHIhB', 1, channels, 0, 48000, 0, 0)
            packet_source = work / f'{label}-packets.mka'
            packet_source.write_bytes(matroska(head, info['packets'], 0, {}))
            raw = {}
            for format, width in [('s16', 2), ('f32', 4)]:
                output = work / f'{label}-packets-{format}.wav'
                output.unlink(missing_ok=True)
                summary = json.loads(run([coreav, 'decode-audio', '--format', format,
                                          packet_source, output]))
                raw[format] = wav_samples(output.read_bytes())
                if summary['output']['samples'] != info['packet_samples'] or \
                        summary['output']['sample_rate'] != 48000 or summary['warnings'] or \
                        len(raw[format]) != info['packet_samples'] * channels * width:
                    raise ValueError('incomplete native packet control')
            reference = run([oracle / 'ffmpeg', '-v', 'error', '-c:a', 'libopus',
                             *AUDIO_DECODE_INPUT, '-i', packet_source, '-f', 's16le', '-'])
            whole_quality = compare(tool, work, label, reference, raw['s16'], channels)
            terminal_bytes = 2880 * channels * 2
            terminal_quality = compare(tool, work, label + '-terminal',
                                       reference[-terminal_bytes:], raw['s16'][-terminal_bytes:], channels)
            full = info['packet_samples'] - info['start']
            for length in (1, 4800, full):
                for skip in (info['skip'], 3840):
                    variant = bytearray(data)
                    struct.pack_into('>I', variant, info['edit'] + 8, length)
                    struct.pack_into('>H', variant, info['config'] + 2, skip)
                    variant_path = work / f'{label}-{length}-{skip}.mp4'
                    variant_path.write_bytes(variant)
                    for format, width in [('s16', 2), ('f32', 4)]:
                        for threads in (1, 8):
                            output = work / f'{label}-{length}-{skip}-{format}-{threads}.wav'
                            output.unlink(missing_ok=True)
                            summary = json.loads(run([coreav, 'decode-audio', '--format', format,
                                '--threads', threads, variant_path, output]))
                            if summary['warnings'] or summary['output']['samples'] != length or \
                                    summary['output']['sample_rate'] != 48000 or \
                                    summary['output']['channels'] != channels or summary['timeline']['start'] != 0:
                                raise ValueError('native output differs from the source presentation')
                            presented(raw[format], wav_samples(output.read_bytes()), info['start'],
                                      length, channels, width)
                    rows.append(dict(mode=mode, channels=channels, rate=48000, mapping=0, gain=0,
                        input_sha256=hashlib.sha256(variant).hexdigest(),
                        packet_sha256=hashlib.sha256(b''.join(info['packets'])).hexdigest(),
                        packet_samples=info['packet_samples'], start=info['start'], samples=length,
                        preskip=skip, whole_quality=whole_quality, terminal_quality=terminal_quality))
    return rows


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--oracle-dir', type=Path, required=True)
    parser.add_argument('--opus-compare', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    args.out.unlink(missing_ok=True)
    result = check(args.coreav.resolve(), args.oracle_dir.resolve(),
                   args.opus_compare.resolve(), args.work.resolve())
    args.out.write_text(json.dumps(result, indent=2) + '\n')
    print(f'{len(result)} scoped Opus MP4 presentations qualified')
