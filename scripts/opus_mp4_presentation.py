"""Check Opus MP4 edits independently of informative dOps PreSkip."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))

from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
from oracle_pin import verify_bin


def run(args):
    result = subprocess.run(list(map(str, args)), capture_output=True, timeout=30, check=True)
    return result.stdout


def boxes(data, start, end):
    at = start
    while at < end:
        size, kind = struct.unpack_from('>I4s', data, at)
        if size < 8 or at + size > end:
            raise ValueError('invalid generated MP4 box')
        yield kind, at + 8, at + size
        at += size


def metadata(data):
    found = {}

    def visit(start, end):
        for kind, begin, finish in boxes(data, start, end):
            if kind in found:
                raise ValueError('duplicate generated MP4 box')
            found[kind] = (begin, finish)
            if kind in (b'moov', b'trak', b'edts', b'mdia', b'minf', b'stbl'):
                visit(begin, finish)
            elif kind == b'stsd':
                visit(begin + 8, finish)
            elif kind == b'Opus':
                visit(begin + 28, finish)

    visit(0, len(data))
    movie = struct.unpack_from('>I', data, found[b'mvhd'][0] + 12)[0]
    media = struct.unpack_from('>I', data, found[b'mdhd'][0] + 12)[0]
    edit = found[b'elst'][0]
    if movie != 48000 or media != 48000 or data[edit:edit + 8] != b'\0\0\0\0\0\0\0\1':
        raise ValueError('unregistered generated edit clocks or structure')
    length, start, rate, fraction = struct.unpack_from('>Iihh', data, edit + 8)
    if length != 4800 or start < 0 or rate != 1 or fraction != 0:
        raise ValueError('unexpected generated edit')
    config = found[b'dOps'][0]
    if data[config:config + 2] != b'\0\1':
        raise ValueError('expected mono Opus configuration')
    size_table = found[b'stsz'][0]
    size, count = struct.unpack_from('>II', data, size_table + 4)
    sizes = [size] * count if size else list(struct.unpack_from(f'>{count}I', data, size_table + 12))
    chunks = found[b'stco'][0]
    chunk_map = found[b'stsc'][0]
    if struct.unpack_from('>I', data, chunks + 4)[0] != 1 or \
            struct.unpack_from('>4I', data, chunk_map + 4) != (1, 1, count, 1):
        raise ValueError('expected one generated chunk')
    at = struct.unpack_from('>I', data, chunks + 8)[0]
    packets = []
    begin, end = found[b'mdat']
    for size in sizes:
        if not begin <= at < at + size <= end:
            raise ValueError('generated packet outside media')
        packets.append(data[at:at + size])
        at += size
    if not packets:
        raise ValueError('expected generated Opus packets')
    for packet in packets:
        config_id = packet[0] >> 3
        frame = (120 << (config_id & 3)) if config_id >= 16 else (
            (480 << (config_id & 1)) if config_id >= 12 else (480, 960, 1920, 2880)[config_id & 3])
        code = packet[0] & 3
        frames = 1 if code == 0 else (packet[1] & 63 if code == 3 else 2)
        if frame * frames != 960:
            raise ValueError('expected 20 ms packets')
    return dict(edit=edit, config=config, start=start, skip=struct.unpack_from('>H', data, config + 2)[0],
                packet_samples=len(packets) * 960, packets=packets)


def wav_samples(data):
    if data[:4] != b'RIFF' or data[8:12] != b'WAVE':
        raise ValueError('expected WAV output')
    at = 12
    while at + 8 <= len(data):
        kind, size = struct.unpack_from('<4sI', data, at)
        if kind == b'data':
            if at + 8 + size != len(data):
                raise ValueError('incomplete WAV data')
            return data[at + 8:]
        at += 8 + size + (size & 1)
    raise ValueError('missing WAV data')


def check(coreav, oracle, work):
    errors = list(verify_bin(oracle))
    if errors:
        raise ValueError('unverified oracle: ' + '; '.join(errors))
    work.mkdir(parents=True, exist_ok=True)
    pcm = work / 'source.f32'
    pcm.write_bytes(b''.join(struct.pack('<f', .2 * math.sin(2 * math.pi * 440 * n / 48000))
                             for n in range(4800)))
    rows = []
    for mode, bitrate, application in [('silk', '12000', 'voip'), ('hybrid', '24000', 'voip'),
                                       ('celt', '96000', 'lowdelay')]:
        source = work / f'{mode}.mp4'
        run([oracle / 'ffmpeg', '-v', 'error', '-f', 'f32le', '-ar', '48000', '-ac', '1',
             *AUDIO_DECODE_INPUT, '-i', pcm, '-c:a', 'libopus', '-b:a', bitrate,
             '-application', application, '-vbr', 'off', '-movie_timescale', '48000', '-y', source])
        data = source.read_bytes()
        info = metadata(data)
        configs = {packet[0] >> 3 for packet in info['packets']}
        if not all((config < 12 if mode == 'silk' else 12 <= config < 16 if mode == 'hybrid'
                    else config >= 16) for config in configs):
            raise AssertionError('generated mode did not match its label')
        full = info['packet_samples'] - info['start']
        if full <= 4800:
            raise AssertionError('fixture needs terminal packet padding')
        native = {}
        for length in [full, 4800]:
            for skip in [info['skip'], 3840]:
                variant = bytearray(data)
                struct.pack_into('>I', variant, info['edit'] + 8, length)
                struct.pack_into('>H', variant, info['config'] + 2, skip)
                path = work / f'{mode}-{length}-{skip}.mp4'
                path.write_bytes(variant)
                reference = run([oracle / 'ffmpeg', '-v', 'error', *AUDIO_DECODE_INPUT,
                                 '-i', path, '-f', 'f32le', '-'])
                oracle_count = len(reference) // 4
                if oracle_count != full:
                    raise AssertionError('pinned default oracle presentation changed')
                for format, width in [('s16', 2), ('f32', 4)]:
                    for threads in [1, 8]:
                        output = work / f'{mode}-{length}-{skip}-{format}-{threads}.wav'
                        output.unlink(missing_ok=True)
                        summary = json.loads(run([coreav, 'decode-audio', '--format', format,
                                                  '--threads', threads, path, output]))
                        samples = wav_samples(output.read_bytes())
                        if summary['output']['samples'] != length or len(samples) != length * width or \
                                summary['warnings'] or summary['timeline']['start'] != 0:
                            raise AssertionError('native output does not preserve the edit')
                        key = (length, format)
                        if key in native and samples != native[key]:
                            raise AssertionError('informative PreSkip or threads changed output')
                        native[key] = samples
                        if length == 4800 and samples != native[(full, format)][:4800 * width]:
                            raise AssertionError('finite edit is not the exact native prefix')
                rows.append(dict(mode=mode, edit_start=info['start'], edit_samples=length,
                                 dops_preskip=skip, packet_samples=info['packet_samples'],
                                 oracle_samples=oracle_count, native_samples=length,
                                 oracle_agrees=oracle_count == length,
                                 input_sha256=hashlib.sha256(variant).hexdigest()))
    return rows


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--oracle-dir', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    result = check(args.coreav.resolve(), args.oracle_dir.resolve(), args.work.resolve())
    args.out.write_text(json.dumps(result, indent=2) + '\n')
    print(f'{len(result)} Opus MP4 presentations checked')
