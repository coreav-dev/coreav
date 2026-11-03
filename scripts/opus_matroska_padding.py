"""Check signed padding against known Opus packet work and the pinned oracle."""

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
    return subprocess.run(list(map(str, args)), capture_output=True, timeout=30, check=True).stdout


def ogg_packets(data):
    packets, pending = [], bytearray()
    at = 0
    eos = None
    while at < len(data):
        if data[at:at + 4] != b'OggS' or len(data) < at + 27:
            raise ValueError('invalid generated Ogg page')
        count = data[at + 26]
        laces = data[at + 27:at + 27 + count]
        body = at + 27 + count
        for length in laces:
            pending.extend(data[body:body + length])
            body += length
            if length < 255:
                packets.append(bytes(pending))
                pending.clear()
        if body > len(data):
            raise ValueError('truncated generated Ogg page')
        if data[at + 5] & 4:
            eos = struct.unpack_from('<Q', data, at + 6)[0]
        at = body
    if pending or eos is None or len(packets) < 3 or not packets[0].startswith(b'OpusHead') or \
            not packets[1].startswith(b'OpusTags'):
        raise ValueError('incomplete generated Opus stream')
    return packets[0], packets[2:], eos


def packet_samples(packet):
    config = packet[0] >> 3
    frame = (120 << (config & 3)) if config >= 16 else (
        (480 << (config & 1)) if config >= 12 else (480, 960, 1920, 2880)[config & 3])
    code = packet[0] & 3
    return frame * (1 if code == 0 else packet[1] & 63 if code == 3 else 2)


def element(identity, data):
    for width in range(1, 9):
        if len(data) < (1 << (7 * width)) - 1:
            return identity.to_bytes((identity.bit_length() + 7) // 8, 'big') \
                + ((1 << (7 * width)) | len(data)).to_bytes(width, 'big') + data
    raise ValueError('element too long')


def uint(identity, value):
    return element(identity, value.to_bytes(max(1, (value.bit_length() + 7) // 8), 'big'))


def matroska(head, packets, delay, padding):
    ebml = element(0x1A45DFA3, uint(0x4286, 1) + uint(0x42F7, 1) + uint(0x42F2, 4)
                   + uint(0x42F3, 8) + element(0x4282, b'matroska') + uint(0x4287, 4) + uint(0x4285, 2))
    info = element(0x1549A966, uint(0x2AD7B1, 125000))
    audio = element(0xE1, element(0xB5, struct.pack('>d', 48000.0)) + uint(0x9F, head[9]))
    track = element(0x1654AE6B, element(0xAE, uint(0xD7, 1) + uint(0x73C5, 1)
                    + uint(0x83, 2) + element(0x86, b'A_OPUS') + element(0x63A2, head)
                    + uint(0x56AA, delay) + uint(0x56BB, 80000000) + audio))
    blocks = bytearray(uint(0xE7, 0))
    samples = 0
    for index, packet in enumerate(packets):
        block = element(0xA1, b'\x81' + struct.pack('>h', samples // 6) + b'\0' + packet)
        if index in padding:
            block += element(0x75A2, padding[index].to_bytes(8, 'big', signed=True))
        blocks.extend(element(0xA0, block))
        samples += packet_samples(packet)
    return ebml + element(0x18538067, info + track + element(0x1F43B675, bytes(blocks)))


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
    verify_bin(oracle)
    work.mkdir(parents=True, exist_ok=True)
    source = work / 'source.f32'
    source.write_bytes(b''.join(struct.pack('<f', .2 * math.sin(2 * math.pi * 440 * n / 48000))
                                for n in range(4800)))
    rows = []
    for mode, bitrate, application, channels in [('silk', '12000', 'voip', 1),
                                                ('hybrid', '24000', 'voip', 1),
                                                ('celt', '96000', 'lowdelay', 2)]:
        path = work / f'{mode}.ogg'
        run([oracle / 'ffmpeg', '-v', 'error', '-f', 'f32le', '-ar', '48000', '-ac', '1',
             *AUDIO_DECODE_INPUT, '-i', source, '-ac', channels, '-c:a', 'libopus', '-b:a', bitrate,
             '-application', application, '-vbr', 'off', '-y', path])
        head, packets, eos = ogg_packets(path.read_bytes())
        counts = [packet_samples(packet) for packet in packets]
        total = sum(counts)
        skip = struct.unpack_from('<H', head, 10)[0]
        configs = [packet[0] >> 3 for packet in packets]
        if head[9] != channels or eos - skip != 4800 or not all(count == 960 for count in counts) or \
                not all((config < 12 if mode == 'silk' else 12 <= config < 16 if mode == 'hybrid'
                         else config >= 16) for config in configs):
            raise AssertionError('generated mode or presentation differs')
        tail = total - eos
        if not 0 <= tail <= counts[-1]:
            raise AssertionError('invalid generated end trimming')
        delay_ns, tail_ns = skip * 1000000000 // 48000, tail * 1000000000 // 48000
        raw_head = bytearray(head)
        struct.pack_into('<H', raw_head, 10, 0)
        raw_input = work / f'{mode}-untrimmed.mka'
        raw_input.write_bytes(matroska(raw_head, packets, 0, {}))
        for format, width in [('s16', 2), ('f32', 4)]:
            stride = width * channels
            output = work / f'{mode}-raw-{format}.wav'
            output.unlink(missing_ok=True)
            raw_summary = json.loads(run([coreav, 'decode-audio', '--format', format, raw_input, output]))
            raw = wav_samples(output.read_bytes())
            if len(raw) != total * stride or raw_summary['output']['samples'] != total:
                raise AssertionError('incomplete untrimmed decoder control')
            for label, middle in [('end', 0), ('positive', 1000000), ('negative', -1000000)]:
                padding = {len(packets) - 1: tail_ns}
                if middle:
                    padding[1] = middle
                data = matroska(head, packets, delay_ns, padding)
                path = work / f'{mode}-{label}.mka'
                path.write_bytes(data)
                cut_start = 960 if middle < 0 else 1920 - 48
                cut_end = cut_start + 48
                wanted = raw[:cut_start * stride] + raw[cut_end * stride:] if middle else raw
                wanted = wanted[skip * stride:(len(wanted) // stride - tail) * stride]
                expected = 4800 - (48 if middle else 0)
                if len(wanted) != expected * stride:
                    raise AssertionError('invalid independent trim model')
                reference = run([oracle / 'ffmpeg', '-v', 'error', *AUDIO_DECODE_INPUT,
                                 '-i', path, '-f', 'f32le', '-'])
                if len(reference) != expected * channels * 4:
                    raise AssertionError('pinned oracle disagrees with specified count')
                for threads in [1, 8]:
                    output = work / f'{mode}-{label}-{format}-{threads}.wav'
                    output.unlink(missing_ok=True)
                    summary = json.loads(run([coreav, 'decode-audio', '--format', format,
                                              '--threads', threads, path, output]))
                    if summary['output']['samples'] != expected or summary['warnings'] or \
                            wav_samples(output.read_bytes()) != wanted:
                        raise AssertionError('signed padding changed count or ordered samples')
                rows.append(dict(mode=mode, channels=channels, format=format, padding=label,
                                 packet_samples=total, codec_delay_samples=skip, tail_samples=tail,
                                 expected_samples=expected, oracle_samples=expected,
                                 input_sha256=hashlib.sha256(data).hexdigest()))
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
    print(f'{len(result)} signed Opus padding controls checked')
